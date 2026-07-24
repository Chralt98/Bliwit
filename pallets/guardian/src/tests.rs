//! `pallet-guardian` unit tests (15 §4.1): per-extrinsic × origin-misuse ×
//! error-path coverage, allowance/limit boundaries, the maintenance crank, and a
//! seeded shell-vs-core differential. The `test-engineer` extends this suite.

use crate::mock::*;
use crate::{pallet::*, Error, Event, ReviewVerdict};
use frame_support::{
    assert_noop, assert_ok,
    traits::{
        fungible::{InspectHold, Mutate},
        Hooks, StorageVersion,
    },
    BoundedVec,
};
use guardian_core::{
    ActionId, DispatchContext, GuardianOrigin, GuardianPower, PendingAction, PlaybookId,
    PlaybookTrigger, ProposalStatus, TriggerState, ACTION_EXPIRY_BLOCKS, FORCE_RERUN_WINDOW_BLOCKS,
    GUARDIAN_BOND, GUARDIAN_SEATS,
};
use sp_core::crypto::AccountId32;
use sp_runtime::DispatchError;

fn hash(n: u8) -> futarchy_primitives::H256 {
    [n; 32]
}

fn last_events() -> Vec<Event<Test>> {
    frame_system::Pallet::<Test>::events()
        .into_iter()
        .filter_map(|r| {
            if let RuntimeEvent::Guardian(e) = r.event {
                Some(e)
            } else {
                None
            }
        })
        .collect()
}

/// Reach the five-of-seven threshold on the action with id 0 (proposer acct(1)
/// already counts, so three more approvals bring it to four, then acct(5)).
fn approve_to_dispatch() {
    for n in 2..=5u8 {
        assert_ok!(Guardian::approve_action(RuntimeOrigin::signed(acct(n)), 0));
    }
}

fn reset_review_scheduler() {
    NextReferendum::set(100);
    ReviewSchedulingFails::set(false);
    ReviewRefundFailsFor::set(None);
    ScheduledReviews::set(Vec::new());
    CancelledReviews::set(Vec::new());
    RefundedReviews::set(Vec::new());
}

fn seat_hold(member: &AccountId32) -> futarchy_primitives::Balance {
    <Balances as InspectHold<AccountId32>>::balance_on_hold(
        &RuntimeHoldReason::Guardian(crate::HoldReason::SeatBond),
        member,
    )
}

#[test]
fn genesis_seeds_council_and_try_state_holds() {
    new_test_ext().execute_with(|| {
        assert!(Members::<Test>::get().is_some());
        assert_eq!(MemberBonds::<Test>::get(), [GUARDIAN_BOND; 7]);
        for member in members() {
            assert_eq!(
                <Balances as InspectHold<AccountId32>>::balance_on_hold(
                    &RuntimeHoldReason::Guardian(crate::HoldReason::SeatBond),
                    &member,
                ),
                GUARDIAN_BOND
            );
        }
        assert_ok!(Guardian::do_try_state());
    });
}

#[test]
fn storage_v1_backfills_failed_delay_reverse_join() {
    new_test_ext().execute_with(|| {
        StorageVersion::new(0).put::<Guardian>();
        PendingActions::<Test>::put(
            BoundedVec::try_from(vec![PendingAction {
                id: 7,
                proposer: acct(1).into(),
                power: GuardianPower::DelayOnce { pid: 42 },
                justification_hash: hash(7),
                created_at: 0,
                expires_at: ACTION_EXPIRY_BLOCKS,
                dispatched: true,
            }])
            .expect("single pending action fits"),
        );
        VetoReviewReferenda::<Test>::insert(7, 101);

        let _ = <Guardian as Hooks<u64>>::on_runtime_upgrade();

        assert_eq!(StorageVersion::get::<Guardian>(), StorageVersion::new(1));
        assert_eq!(VetoReviewActions::<Test>::get(42), Some(7));
    });
}

#[test]
fn storage_v1_migration_stays_retryable_on_ambiguous_review() {
    new_test_ext().execute_with(|| {
        StorageVersion::new(0).put::<Guardian>();
        VetoReviewReferenda::<Test>::insert(8, 101);

        let _ = <Guardian as Hooks<u64>>::on_runtime_upgrade();

        assert_eq!(StorageVersion::get::<Guardian>(), StorageVersion::new(0));
        assert!(!VetoReviewActions::<Test>::contains_key(42));
    });
}

#[test]
fn storage_v1_migration_stays_retryable_on_malformed_existing_reverse_join() {
    new_test_ext().execute_with(|| {
        StorageVersion::new(0).put::<Guardian>();
        PendingActions::<Test>::put(
            BoundedVec::try_from(vec![PendingAction {
                id: 7,
                proposer: acct(1).into(),
                power: GuardianPower::DelayOnce { pid: 42 },
                justification_hash: hash(7),
                created_at: 0,
                expires_at: ACTION_EXPIRY_BLOCKS,
                dispatched: true,
            }])
            .expect("single pending action fits"),
        );
        VetoReviewReferenda::<Test>::insert(7, 101);
        VetoReviewActions::<Test>::insert(99, 7);

        let _ = <Guardian as Hooks<u64>>::on_runtime_upgrade();

        assert_eq!(StorageVersion::get::<Guardian>(), StorageVersion::new(0));
    });
}

#[test]
fn set_members_is_values_origin_only() {
    new_test_ext_empty().execute_with(|| {
        // A non-values signed origin is refused.
        assert_noop!(
            Guardian::set_members(RuntimeOrigin::signed(acct(1)), members()),
            DispatchError::BadOrigin
        );
        assert_ok!(Guardian::set_members(values_origin(), members()));
        assert!(Members::<Test>::get().is_some());
        // Duplicate seat rejected by the core.
        let mut dup = members();
        dup[1] = dup[0].clone();
        assert_noop!(
            Guardian::set_members(values_origin(), dup),
            Error::<Test>::DuplicateMember
        );
    });
}

#[test]
fn all_kernel_playbooks_are_genesis_registered_and_admin_can_fail_closed() {
    new_test_ext().execute_with(|| {
        for id in guardian_core::ALL_PLAYBOOKS {
            assert!(PlaybookRegistered::<Test>::get(id));
        }
        assert_eq!(PlaybookRegistered::<Test>::iter_keys().count(), 6);
        assert_noop!(
            Guardian::set_playbook_registered(
                RuntimeOrigin::signed(acct(1)),
                PlaybookId::Depeg,
                false,
            ),
            DispatchError::BadOrigin
        );
        assert_ok!(Guardian::set_playbook_registered(
            values_origin(),
            PlaybookId::Depeg,
            false,
        ));
        set_triggers(all_triggers());
        assert_ok!(Guardian::propose_action(
            RuntimeOrigin::signed(acct(1)),
            GuardianPower::ActivatePlaybook {
                id: PlaybookId::Depeg,
                trigger: PlaybookTrigger::DepegMedian,
                expiry: 100,
                target: None,
            },
            hash(1),
        ));
        for n in 2..=4u8 {
            assert_ok!(Guardian::approve_action(RuntimeOrigin::signed(acct(n)), 0));
        }
        assert_noop!(
            Guardian::approve_action(RuntimeOrigin::signed(acct(5)), 0),
            Error::<Test>::PlaybookNotRegistered
        );
        assert_ok!(Guardian::set_playbook_registered(
            values_origin(),
            PlaybookId::Depeg,
            true,
        ));
        assert_ok!(Guardian::approve_action(RuntimeOrigin::signed(acct(5)), 0));
        assert!(Guardian::playbook_active(PlaybookId::Depeg));
        assert_ok!(Guardian::do_try_state());
    });
}

#[test]
fn oracle_void_requires_a_target_and_other_playbooks_reject_one() {
    new_test_ext().execute_with(|| {
        set_triggers(all_triggers());
        for (action, power) in [
            (
                0,
                GuardianPower::ActivatePlaybook {
                    id: PlaybookId::OracleVoid,
                    trigger: PlaybookTrigger::OracleDeadlock,
                    expiry: 100,
                    target: None,
                },
            ),
            (
                1,
                GuardianPower::ActivatePlaybook {
                    id: PlaybookId::Depeg,
                    trigger: PlaybookTrigger::DepegMedian,
                    expiry: 100,
                    target: Some(7),
                },
            ),
        ] {
            assert_ok!(Guardian::propose_action(
                RuntimeOrigin::signed(acct(1)),
                power,
                hash(1),
            ));
            for n in 2..=4u8 {
                assert_ok!(Guardian::approve_action(
                    RuntimeOrigin::signed(acct(n)),
                    action,
                ));
            }
            assert_noop!(
                Guardian::approve_action(RuntimeOrigin::signed(acct(5)), action),
                Error::<Test>::BadPlaybookTarget
            );
        }
    });
}

#[test]
fn propose_requires_membership() {
    new_test_ext().execute_with(|| {
        let outsider = AccountId32::from([250u8; 32]);
        assert_noop!(
            Guardian::propose_action(
                RuntimeOrigin::signed(outsider),
                GuardianPower::SuspendOnGate,
                hash(1)
            ),
            Error::<Test>::NotMember
        );
    });
}

#[test]
fn five_of_seven_dispatches_schedules_review_with_referendum() {
    new_test_ext().execute_with(|| {
        reset_review_scheduler();
        set_triggers(guardian_core::TriggerState {
            gate_breach: true,
            ..guardian_core::TriggerState::none()
        });
        assert_ok!(Guardian::propose_action(
            RuntimeOrigin::signed(acct(1)),
            GuardianPower::SuspendOnGate,
            hash(9)
        ));
        approve_to_dispatch();
        assert!(PendingActions::<Test>::get()[0].dispatched);
        assert_eq!(ReviewDeadlines::<Test>::get().len(), 1);
        assert_eq!(
            ScheduledReviews::get(),
            vec![(0, ReviewVerdict::Ratify, 100)]
        );
        assert_eq!(ReviewReferenda::<Test>::get(0), Some(100));
        assert_eq!(VetoReviewReferenda::<Test>::get(0), None);
        assert_eq!(NextReferendum::get(), 101);
        // The frozen `ReviewScheduled { action, referendum }` carries the index
        // the scheduler seam returned (mock starts at 100).
        assert!(last_events().iter().any(|e| matches!(
            e,
            Event::ReviewScheduled {
                action: 0,
                referendum: 100
            }
        )));
        assert!(last_events()
            .iter()
            .any(|e| matches!(e, Event::GuardianAction { action_id: 0, .. })));
    });
}

#[test]
fn amended_review_deadline_moves_new_records_without_rewriting_existing_records() {
    new_test_ext().execute_with(|| {
        set_epoch(7);
        set_triggers(guardian_core::TriggerState {
            gate_breach: true,
            ..guardian_core::TriggerState::none()
        });

        assert_ok!(Guardian::propose_action(
            RuntimeOrigin::signed(acct(1)),
            GuardianPower::SuspendOnGate,
            hash(1)
        ));
        approve_span(0, 2, 5);
        assert_eq!(
            ReviewDeadlines::<Test>::get()
                .iter()
                .map(|review| review.deadline_epoch)
                .collect::<Vec<_>>(),
            [7u32.saturating_add(guardian_core::REVIEW_DEADLINE_EPOCHS)]
        );

        ReviewDeadlineEpochValue::set(4);
        assert_ok!(Guardian::propose_action(
            RuntimeOrigin::signed(acct(1)),
            GuardianPower::SuspendOnGate,
            hash(2)
        ));
        approve_span(1, 2, 5);
        let reviews = ReviewDeadlines::<Test>::get();
        assert_eq!(
            reviews
                .iter()
                .map(|review| review.deadline_epoch)
                .collect::<Vec<_>>(),
            [9, 11]
        );

        ReviewDeadlineEpochValue::set(1);
        let snapshotted = ReviewDeadlines::<Test>::get();
        assert_eq!(
            snapshotted
                .iter()
                .map(|review| review.deadline_epoch)
                .collect::<Vec<_>>(),
            [9, 11]
        );
    });
}

#[test]
fn unavailable_review_scheduler_rolls_back_the_dispatching_approval() {
    new_test_ext().execute_with(|| {
        set_triggers(guardian_core::TriggerState {
            gate_breach: true,
            ..guardian_core::TriggerState::none()
        });
        assert_ok!(Guardian::propose_action(
            RuntimeOrigin::signed(acct(1)),
            GuardianPower::SuspendOnGate,
            hash(9)
        ));
        for n in 2..=4u8 {
            assert_ok!(Guardian::approve_action(RuntimeOrigin::signed(acct(n)), 0));
        }
        let before_events = frame_system::Pallet::<Test>::events();
        ReviewSchedulingFails::set(true);
        assert_noop!(
            Guardian::approve_action(RuntimeOrigin::signed(acct(5)), 0),
            DispatchError::Other("review scheduler unavailable")
        );
        assert!(!PendingActions::<Test>::get()[0].dispatched);
        assert_eq!(Approvals::<Test>::get().len(), 4);
        assert!(ReviewDeadlines::<Test>::get().is_empty());
        assert_eq!(frame_system::Pallet::<Test>::events(), before_events);
    });
}

#[test]
fn force_rerun_pre_execution_only_and_once() {
    new_test_ext().execute_with(|| {
        set_status(ProposalStatus::Executed, false);
        assert_ok!(Guardian::propose_action(
            RuntimeOrigin::signed(acct(1)),
            GuardianPower::ForceRerun { pid: 7 },
            hash(0)
        ));
        for n in 2..=4u8 {
            assert_ok!(Guardian::approve_action(RuntimeOrigin::signed(acct(n)), 0));
        }
        // The fifth approval tries to dispatch against an Executed proposal.
        assert_noop!(
            Guardian::approve_action(RuntimeOrigin::signed(acct(5)), 0),
            Error::<Test>::NotRerunnable
        );
    });
}

#[test]
fn ratify_action_is_values_origin_only() {
    new_test_ext().execute_with(|| {
        set_triggers(guardian_core::TriggerState {
            gate_breach: true,
            ..guardian_core::TriggerState::none()
        });
        assert_ok!(Guardian::propose_action(
            RuntimeOrigin::signed(acct(1)),
            GuardianPower::SuspendOnGate,
            hash(2)
        ));
        approve_to_dispatch();
        assert_noop!(
            Guardian::ratify_action(RuntimeOrigin::signed(acct(1)), 0),
            DispatchError::BadOrigin
        );
        assert_ok!(Guardian::ratify_action(values_origin(), 0));
        assert!(ReviewDeadlines::<Test>::get()[0].ratified);
        assert!(!ReviewFrontingOf::<Test>::contains_key(0));
        for member in members().into_iter().take(5) {
            assert_eq!(
                <Balances as InspectHold<AccountId32>>::balance_on_hold(
                    &RuntimeHoldReason::Guardian(crate::HoldReason::SeatBond),
                    &member,
                ),
                GUARDIAN_BOND
            );
        }
    });
}

#[test]
fn ledger_freeze_requires_drift_and_one_renewal() {
    // limit-coverage: pb-ledger-freeze.duration, pb-ledger-freeze.renewal
    new_test_ext().execute_with(|| {
        // The playbook's absolute expiry is bounded to 14 days from dispatch.
        set_triggers(guardian_core::TriggerState {
            ledger_drift: true,
            ..guardian_core::TriggerState::none()
        });
        let now = u32::try_from(System::block_number()).expect("mock block fits u32");
        let expiry = now
            .saturating_add(guardian_core::HOLD_MAX_BLOCKS)
            .saturating_add(1);
        assert_ok!(Guardian::propose_action(
            RuntimeOrigin::signed(acct(1)),
            GuardianPower::ActivatePlaybook {
                id: PlaybookId::LedgerFreeze,
                trigger: PlaybookTrigger::LedgerDrift,
                expiry,
                target: None,
            },
            hash(1)
        ));
        for n in 2..=4u8 {
            assert_ok!(Guardian::approve_action(RuntimeOrigin::signed(acct(n)), 0));
        }
        assert_noop!(
            Guardian::approve_action(RuntimeOrigin::signed(acct(5)), 0),
            Error::<Test>::DurationTooLong
        );
    });

    new_test_ext().execute_with(|| {
        // No drift trigger ⇒ activation refused at the dispatching approval.
        set_triggers(guardian_core::TriggerState::none());
        assert_ok!(Guardian::propose_action(
            RuntimeOrigin::signed(acct(1)),
            GuardianPower::ActivatePlaybook {
                id: PlaybookId::LedgerFreeze,
                trigger: PlaybookTrigger::LedgerDrift,
                expiry: 100,
                target: None,
            },
            hash(1)
        ));
        for n in 2..=4u8 {
            assert_ok!(Guardian::approve_action(RuntimeOrigin::signed(acct(n)), 0));
        }
        assert_noop!(
            Guardian::approve_action(RuntimeOrigin::signed(acct(5)), 0),
            Error::<Test>::TriggerInactive
        );

        // With drift live, activation succeeds and exactly one renewal is allowed.
        set_triggers(guardian_core::TriggerState {
            ledger_drift: true,
            ..guardian_core::TriggerState::none()
        });
        assert_ok!(Guardian::propose_action(
            RuntimeOrigin::signed(acct(1)),
            GuardianPower::ActivatePlaybook {
                id: PlaybookId::LedgerFreeze,
                trigger: PlaybookTrigger::LedgerDrift,
                expiry: 100,
                target: None,
            },
            hash(1)
        ));
        for n in 2..=5u8 {
            assert_ok!(Guardian::approve_action(RuntimeOrigin::signed(acct(n)), 1));
        }
        assert!(Guardian::playbook_active(PlaybookId::LedgerFreeze));
        assert_noop!(
            Guardian::renew_playbook(RuntimeOrigin::signed(acct(1)), PlaybookId::LedgerFreeze),
            DispatchError::BadOrigin
        );
        assert_ok!(Guardian::renew_playbook(
            values_origin(),
            PlaybookId::LedgerFreeze
        ));
        assert_noop!(
            Guardian::renew_playbook(values_origin(), PlaybookId::LedgerFreeze),
            Error::<Test>::RenewalNotAllowed
        );
    });
}

#[test]
fn ledger_freeze_effect_lifts_and_reengages_with_live_drift_without_deleting_record() {
    new_test_ext().execute_with(|| {
        set_triggers(TriggerState {
            ledger_drift: true,
            ..TriggerState::none()
        });
        assert_ok!(Guardian::propose_action(
            RuntimeOrigin::signed(acct(1)),
            GuardianPower::ActivatePlaybook {
                id: PlaybookId::LedgerFreeze,
                trigger: PlaybookTrigger::LedgerDrift,
                expiry: 100,
                target: None,
            },
            hash(33),
        ));
        approve_to_dispatch();
        assert!(LedgerFreezeEffect::get());
        assert_eq!(ActivePlaybooks::<Test>::get().len(), 1);

        set_triggers(TriggerState::none());
        run_to_block(2);
        assert!(!LedgerFreezeEffect::get());
        assert_eq!(ActivePlaybooks::<Test>::get().len(), 1);
        assert!(!last_events()
            .iter()
            .any(|event| matches!(event, Event::PlaybookExpired { .. })));

        set_triggers(TriggerState {
            ledger_drift: true,
            ..TriggerState::none()
        });
        run_to_block(3);
        assert!(LedgerFreezeEffect::get());
        assert_eq!(ActivePlaybooks::<Test>::get().len(), 1);
        assert_eq!(
            LedgerFreezeEffectTransitions::get(),
            vec![true, false, true]
        );
    });
}

#[test]
fn delay_once_allowance_resets_across_epochs() {
    new_test_ext().execute_with(|| {
        set_status(ProposalStatus::Queued, false);
        // Two delay_once dispatches exhaust the per-epoch allowance.
        for pid in 0..2u64 {
            assert_ok!(Guardian::propose_action(
                RuntimeOrigin::signed(acct(1)),
                GuardianPower::DelayOnce { pid },
                hash(0)
            ));
            for n in 2..=5u8 {
                assert_ok!(Guardian::approve_action(
                    RuntimeOrigin::signed(acct(n)),
                    pid as u32
                ));
            }
        }
        // Third in the same epoch is refused.
        assert_ok!(Guardian::propose_action(
            RuntimeOrigin::signed(acct(1)),
            GuardianPower::DelayOnce { pid: 2 },
            hash(0)
        ));
        for n in 2..=4u8 {
            assert_ok!(Guardian::approve_action(RuntimeOrigin::signed(acct(n)), 2));
        }
        assert_noop!(
            Guardian::approve_action(RuntimeOrigin::signed(acct(5)), 2),
            Error::<Test>::AllowanceExhausted
        );
        // Next epoch resets the allowance.
        set_epoch(1);
        assert_ok!(Guardian::approve_action(RuntimeOrigin::signed(acct(5)), 2));
    });
}

#[test]
fn maintenance_crank_expires_playbooks_and_slashes_overdue_reviews() {
    new_test_ext().execute_with(|| {
        set_triggers(guardian_core::TriggerState {
            gate_breach: true,
            ..guardian_core::TriggerState::none()
        });
        assert_ok!(Guardian::propose_action(
            RuntimeOrigin::signed(acct(1)),
            GuardianPower::SuspendOnGate,
            hash(2)
        ));
        approve_to_dispatch();
        // Review deadline is `current_epoch + 2`; strict enforcement fails
        // only after that boundary, at epoch 3 (SQ-45).
        set_epoch(2);
        run_to_block(2);
        assert!(MemberBonds::<Test>::get()
            .iter()
            .all(|bond| *bond == GUARDIAN_BOND));
        set_epoch(3);
        run_to_block(3);
        let bonds = MemberBonds::<Test>::get();
        assert!(bonds[..5].iter().all(|b| *b == GUARDIAN_BOND / 2));
        assert!(bonds[5..].iter().all(|b| *b == GUARDIAN_BOND));
        for member in members().into_iter().take(5) {
            assert_eq!(
                <Balances as InspectHold<AccountId32>>::balance_on_hold(
                    &RuntimeHoldReason::Guardian(crate::HoldReason::SeatBond),
                    &member,
                ),
                GUARDIAN_BOND / 2
            );
        }
        assert!(last_events()
            .iter()
            .any(|e| matches!(e, Event::ReviewFailed { action: 0, .. })));
        assert_ok!(Guardian::do_try_state());
    });
}

// ============================================================================
// Extended 15 §4.1 obligation set (test-engineer). Every extrinsic × every
// reachable Error variant × origin misuse; every 06 §5.2 power; the pallet
// bounds; the maintenance crank; and a seeded shell-vs-core differential.
// ============================================================================

/// All eight verified triggers live (06 §6.2).
fn all_triggers() -> TriggerState {
    TriggerState {
        depeg: true,
        migration_halt: true,
        oracle_deadlock: true,
        gate_breach: true,
        dead_man: true,
        void_in_flight: true,
        reserve_health: true,
        ledger_drift: true,
    }
}

/// Approve `id` with members `from..=to` (proposer's own approval already counts).
fn approve_span(id: ActionId, from: u8, to: u8) {
    for n in from..=to {
        assert_ok!(Guardian::approve_action(RuntimeOrigin::signed(acct(n)), id));
    }
}

// ------------------------------------------------ NotInitialized / origins --

#[test]
fn workflow_calls_before_council_exists_are_not_initialized() {
    // 06 §5.1: every workflow call needs an elected council. On an empty
    // genesis, propose/approve/ratify/renew all fail `NotInitialized` (the
    // values-origin calls pass their origin check first, then hit the missing
    // council).
    new_test_ext_empty().execute_with(|| {
        assert_noop!(
            Guardian::propose_action(
                RuntimeOrigin::signed(acct(1)),
                GuardianPower::SuspendOnGate,
                hash(0)
            ),
            Error::<Test>::NotInitialized
        );
        assert_noop!(
            Guardian::approve_action(RuntimeOrigin::signed(acct(1)), 0),
            Error::<Test>::NotInitialized
        );
        assert_noop!(
            Guardian::ratify_action(values_origin(), 0),
            Error::<Test>::NotInitialized
        );
        assert_noop!(
            Guardian::renew_playbook(values_origin(), PlaybookId::LedgerFreeze),
            Error::<Test>::NotInitialized
        );
    });
}

#[test]
fn set_members_root_and_none_are_bad_origin() {
    new_test_ext_empty().execute_with(|| {
        // 06 §3.2 row 5: only `ConstitutionalValues`. Root (bootstrap sudo) and
        // unsigned are refused; the signed-non-values path is pinned already in
        // `set_members_is_values_origin_only`.
        assert_noop!(
            Guardian::set_members(RuntimeOrigin::root(), members()),
            DispatchError::BadOrigin
        );
        assert_noop!(
            Guardian::set_members(RuntimeOrigin::none(), members()),
            DispatchError::BadOrigin
        );
    });
}

#[test]
fn propose_action_origin_misuse_and_membership() {
    new_test_ext().execute_with(|| {
        // `Signed` workflow call (06 §8): root/unsigned never satisfy
        // `ensure_signed`.
        assert_noop!(
            Guardian::propose_action(RuntimeOrigin::root(), GuardianPower::SuspendOnGate, hash(0)),
            DispatchError::BadOrigin
        );
        assert_noop!(
            Guardian::propose_action(RuntimeOrigin::none(), GuardianPower::SuspendOnGate, hash(0)),
            DispatchError::BadOrigin
        );
        // A signed non-member is refused inside the core (06 §5.1).
        assert_noop!(
            Guardian::propose_action(
                RuntimeOrigin::signed(acct(250)),
                GuardianPower::SuspendOnGate,
                hash(0)
            ),
            Error::<Test>::NotMember
        );
    });
}

// --------------------------------------------------- approve_action errors --

#[test]
fn approve_action_error_paths_are_exact() {
    new_test_ext().execute_with(|| {
        set_triggers(all_triggers());
        // Unknown action id.
        assert_noop!(
            Guardian::approve_action(RuntimeOrigin::signed(acct(2)), 0),
            Error::<Test>::ActionNotFound
        );
        assert_ok!(Guardian::propose_action(
            RuntimeOrigin::signed(acct(1)),
            GuardianPower::SuspendOnGate,
            hash(0)
        ));
        // Non-member, then origin misuse.
        assert_noop!(
            Guardian::approve_action(RuntimeOrigin::signed(acct(250)), 0),
            Error::<Test>::NotMember
        );
        assert_noop!(
            Guardian::approve_action(RuntimeOrigin::root(), 0),
            DispatchError::BadOrigin
        );
        assert_noop!(
            Guardian::approve_action(RuntimeOrigin::none(), 0),
            DispatchError::BadOrigin
        );
        // The proposer already auto-approved: a second approval is a duplicate.
        assert_noop!(
            Guardian::approve_action(RuntimeOrigin::signed(acct(1)), 0),
            Error::<Test>::DuplicateApproval
        );
        // Drive to dispatch, then a sixth approval hits `AlreadyDispatched`.
        approve_span(0, 2, 5);
        assert!(PendingActions::<Test>::get()[0].dispatched);
        assert_noop!(
            Guardian::approve_action(RuntimeOrigin::signed(acct(6)), 0),
            Error::<Test>::AlreadyDispatched
        );
        assert_ok!(Guardian::do_try_state());
    });
}

#[test]
fn approve_action_after_expiry_is_rejected() {
    new_test_ext().execute_with(|| {
        assert_ok!(Guardian::propose_action(
            RuntimeOrigin::signed(acct(1)),
            GuardianPower::SuspendOnGate,
            hash(0)
        ));
        // 06 §5.1: proposals expire un-dispatched after 3 days. Created at block
        // 1 ⇒ `expires_at = 1 + ACTION_EXPIRY_BLOCKS`; one block past that fails.
        frame_system::Pallet::<Test>::set_block_number(u64::from(ACTION_EXPIRY_BLOCKS) + 2);
        assert_noop!(
            Guardian::approve_action(RuntimeOrigin::signed(acct(2)), 0),
            Error::<Test>::ActionExpired
        );
        assert_ok!(Guardian::do_try_state());
    });
}

// ----------------------------------------------------- ratify / renew errors --

#[test]
fn ratify_action_origin_and_review_errors() {
    new_test_ext().execute_with(|| {
        set_triggers(all_triggers());
        // 06 §5.4: no review record yet ⇒ `ReviewNotFound` (origin is valid).
        assert_noop!(
            Guardian::ratify_action(values_origin(), 0),
            Error::<Test>::ReviewNotFound
        );
        assert_ok!(Guardian::propose_action(
            RuntimeOrigin::signed(acct(1)),
            GuardianPower::SuspendOnGate,
            hash(0)
        ));
        approve_span(0, 2, 5);
        // 06 §3.2 row 6: `ConstitutionalValues` only — member/root/unsigned refused.
        assert_noop!(
            Guardian::ratify_action(RuntimeOrigin::signed(acct(1)), 0),
            DispatchError::BadOrigin
        );
        assert_noop!(
            Guardian::ratify_action(RuntimeOrigin::root(), 0),
            DispatchError::BadOrigin
        );
        assert_noop!(
            Guardian::ratify_action(RuntimeOrigin::none(), 0),
            DispatchError::BadOrigin
        );
        assert_ok!(Guardian::ratify_action(values_origin(), 0));
        assert!(ReviewDeadlines::<Test>::get()[0].ratified);
        assert!(last_events()
            .iter()
            .any(|e| matches!(e, Event::ActionRatified { action: 0 })));
        // A second ratification of the same review is refused.
        assert_noop!(
            Guardian::ratify_action(values_origin(), 0),
            Error::<Test>::AlreadyRatified
        );
        assert_ok!(Guardian::do_try_state());
    });
}

#[test]
fn renew_playbook_origin_and_admissibility_errors() {
    new_test_ext().execute_with(|| {
        // 06 §3.2 row 6 / §6.3: `ConstitutionalValues` only.
        assert_noop!(
            Guardian::renew_playbook(RuntimeOrigin::signed(acct(1)), PlaybookId::LedgerFreeze),
            DispatchError::BadOrigin
        );
        assert_noop!(
            Guardian::renew_playbook(RuntimeOrigin::root(), PlaybookId::LedgerFreeze),
            DispatchError::BadOrigin
        );
        assert_noop!(
            Guardian::renew_playbook(RuntimeOrigin::none(), PlaybookId::LedgerFreeze),
            DispatchError::BadOrigin
        );
        // 06 §6.3: only PB-LEDGER-FREEZE renews — any other id is inadmissible.
        assert_noop!(
            Guardian::renew_playbook(values_origin(), PlaybookId::Depeg),
            Error::<Test>::RenewalNotAllowed
        );
        // LedgerFreeze that is not currently active is also inadmissible.
        assert_noop!(
            Guardian::renew_playbook(values_origin(), PlaybookId::LedgerFreeze),
            Error::<Test>::RenewalNotAllowed
        );
        assert_ok!(Guardian::do_try_state());
    });
}

// ------------------------------------------------------ powers (06 §5.2) --

#[test]
fn pause_intake_duration_cap_and_one_per_four_epochs() {
    new_test_ext().execute_with(|| {
        // Bound: `until` beyond `now + HOLD_MAX_BLOCKS` (≤ 14 days) ⇒ DurationTooLong.
        assert_ok!(Guardian::propose_action(
            RuntimeOrigin::signed(acct(1)),
            GuardianPower::PauseIntake {
                until: guardian_core::HOLD_MAX_BLOCKS + 100
            },
            hash(0)
        ));
        for n in 2..=4u8 {
            assert_ok!(Guardian::approve_action(RuntimeOrigin::signed(acct(n)), 0));
        }
        assert_noop!(
            Guardian::approve_action(RuntimeOrigin::signed(acct(5)), 0),
            Error::<Test>::DurationTooLong
        );

        // Allowance 1 per 4 epochs: first pause at epoch 0 dispatches.
        assert_ok!(Guardian::propose_action(
            RuntimeOrigin::signed(acct(1)),
            GuardianPower::PauseIntake { until: 500 },
            hash(0)
        ));
        approve_span(1, 2, 5);
        // A second pause inside the same window is refused…
        assert_ok!(Guardian::propose_action(
            RuntimeOrigin::signed(acct(1)),
            GuardianPower::PauseIntake { until: 500 },
            hash(0)
        ));
        for n in 2..=4u8 {
            assert_ok!(Guardian::approve_action(RuntimeOrigin::signed(acct(n)), 2));
        }
        assert_noop!(
            Guardian::approve_action(RuntimeOrigin::signed(acct(5)), 2),
            Error::<Test>::AllowanceExhausted
        );
        // …still refused three epochs in (window measured from first use)…
        set_epoch(3);
        assert_noop!(
            Guardian::approve_action(RuntimeOrigin::signed(acct(5)), 2),
            Error::<Test>::AllowanceExhausted
        );
        // …and admitted once the 4-epoch window rolls over.
        set_epoch(4);
        assert_ok!(Guardian::approve_action(RuntimeOrigin::signed(acct(5)), 2));
        assert!(PendingActions::<Test>::get()[2].dispatched);
        assert_ok!(Guardian::do_try_state());
    });
}

#[test]
fn delay_once_queued_only_allowance_and_once_ever() {
    new_test_ext().execute_with(|| {
        // 06 §5.2: one *queued* proposal only. A Trading target is not delayable.
        set_status(ProposalStatus::Trading, false);
        assert_ok!(Guardian::propose_action(
            RuntimeOrigin::signed(acct(1)),
            GuardianPower::DelayOnce { pid: 1 },
            hash(0)
        ));
        for n in 2..=4u8 {
            assert_ok!(Guardian::approve_action(RuntimeOrigin::signed(acct(n)), 0));
        }
        assert_noop!(
            Guardian::approve_action(RuntimeOrigin::signed(acct(5)), 0),
            Error::<Test>::NotRerunnable
        );
        // Flip the same proposal to Queued: the (rolled-back) fifth approval now
        // dispatches, recording the rerun.
        set_status(ProposalStatus::Queued, false);
        assert_ok!(Guardian::approve_action(RuntimeOrigin::signed(acct(5)), 0));
        assert!(RerunUsed::<Test>::get().contains(&1));
        // Once ever (06 §5.3): a second delay on pid 1 ⇒ AlreadyRerun, allowance
        // notwithstanding.
        assert_ok!(Guardian::propose_action(
            RuntimeOrigin::signed(acct(1)),
            GuardianPower::DelayOnce { pid: 1 },
            hash(0)
        ));
        for n in 2..=4u8 {
            assert_ok!(Guardian::approve_action(RuntimeOrigin::signed(acct(n)), 1));
        }
        assert_noop!(
            Guardian::approve_action(RuntimeOrigin::signed(acct(5)), 1),
            Error::<Test>::AlreadyRerun
        );
        assert_ok!(Guardian::do_try_state());
    });
}

#[test]
fn force_rerun_rerunnable_states_once_ever_and_cross_kind() {
    new_test_ext().execute_with(|| {
        // Extended is rerunnable (06 §5.3): pid 2 force-reruns and is recorded.
        set_status(ProposalStatus::Extended, false);
        assert_ok!(Guardian::propose_action(
            RuntimeOrigin::signed(acct(1)),
            GuardianPower::ForceRerun { pid: 2 },
            hash(0)
        ));
        approve_span(0, 2, 5);
        assert!(RerunUsed::<Test>::get().contains(&2));
        // A proposal already inside a rerun cannot be re-run (in_rerun flag).
        set_status(ProposalStatus::Trading, true);
        assert_ok!(Guardian::propose_action(
            RuntimeOrigin::signed(acct(1)),
            GuardianPower::ForceRerun { pid: 3 },
            hash(0)
        ));
        for n in 2..=4u8 {
            assert_ok!(Guardian::approve_action(RuntimeOrigin::signed(acct(n)), 1));
        }
        assert_noop!(
            Guardian::approve_action(RuntimeOrigin::signed(acct(5)), 1),
            Error::<Test>::AlreadyRerun
        );
        // `Rerun` status itself is post-execution ⇒ not rerunnable.
        set_status(ProposalStatus::Rerun, false);
        assert_ok!(Guardian::propose_action(
            RuntimeOrigin::signed(acct(1)),
            GuardianPower::ForceRerun { pid: 4 },
            hash(0)
        ));
        for n in 2..=4u8 {
            assert_ok!(Guardian::approve_action(RuntimeOrigin::signed(acct(n)), 2));
        }
        assert_noop!(
            Guardian::approve_action(RuntimeOrigin::signed(acct(5)), 2),
            Error::<Test>::NotRerunnable
        );
        // One guardian rerun of *either* kind per proposal, ever (06 §5.3): pid 2
        // was force-run, so a later delay_once on it is refused even in a fresh
        // epoch with a Queued target.
        set_epoch(1);
        set_status(ProposalStatus::Queued, false);
        assert_ok!(Guardian::propose_action(
            RuntimeOrigin::signed(acct(1)),
            GuardianPower::DelayOnce { pid: 2 },
            hash(0)
        ));
        for n in 2..=4u8 {
            assert_ok!(Guardian::approve_action(RuntimeOrigin::signed(acct(n)), 3));
        }
        assert_noop!(
            Guardian::approve_action(RuntimeOrigin::signed(acct(5)), 3),
            Error::<Test>::AlreadyRerun
        );
        assert_ok!(Guardian::do_try_state());
    });
}

#[test]
fn force_rerun_allowance_is_one_per_epoch() {
    new_test_ext().execute_with(|| {
        // 06 §5.2: force_rerun allowance is 1/epoch. Two distinct rerunnable pids
        // in one epoch: the first dispatches, the second exhausts the allowance.
        set_status(ProposalStatus::Queued, false);
        assert_ok!(Guardian::propose_action(
            RuntimeOrigin::signed(acct(1)),
            GuardianPower::ForceRerun { pid: 10 },
            hash(0)
        ));
        approve_span(0, 2, 5);
        assert_ok!(Guardian::propose_action(
            RuntimeOrigin::signed(acct(1)),
            GuardianPower::ForceRerun { pid: 11 },
            hash(0)
        ));
        for n in 2..=4u8 {
            assert_ok!(Guardian::approve_action(RuntimeOrigin::signed(acct(n)), 1));
        }
        assert_noop!(
            Guardian::approve_action(RuntimeOrigin::signed(acct(5)), 1),
            Error::<Test>::AllowanceExhausted
        );
        // Next epoch resets the allowance.
        set_epoch(1);
        assert_ok!(Guardian::approve_action(RuntimeOrigin::signed(acct(5)), 1));
        assert!(RerunUsed::<Test>::get().contains(&11));
        assert_ok!(Guardian::do_try_state());
    });
}

#[test]
fn suspend_on_gate_requires_gate_breach() {
    new_test_ext().execute_with(|| {
        // 06 §5.2: freeze the queue only while the hard-gate daily breach flag is
        // live. Without it the dispatching approval fails `TriggerInactive`.
        set_triggers(TriggerState::none());
        assert_ok!(Guardian::propose_action(
            RuntimeOrigin::signed(acct(1)),
            GuardianPower::SuspendOnGate,
            hash(0)
        ));
        for n in 2..=4u8 {
            assert_ok!(Guardian::approve_action(RuntimeOrigin::signed(acct(n)), 0));
        }
        assert_noop!(
            Guardian::approve_action(RuntimeOrigin::signed(acct(5)), 0),
            Error::<Test>::TriggerInactive
        );
        // With the flag set it dispatches.
        set_triggers(TriggerState {
            gate_breach: true,
            ..TriggerState::none()
        });
        assert_ok!(Guardian::approve_action(RuntimeOrigin::signed(acct(5)), 0));
        assert!(PendingActions::<Test>::get()[0].dispatched);
        assert_ok!(Guardian::do_try_state());
    });
}

/// The six enumerated playbooks (06 §6.2), each with a valid `(id, trigger)`
/// pairing (HaltIntake accepts any of three triggers; here gate_breach).
fn playbook_pairs() -> [(PlaybookId, PlaybookTrigger); 6] {
    [
        (PlaybookId::Depeg, PlaybookTrigger::DepegMedian),
        (PlaybookId::Migration, PlaybookTrigger::MigrationHalt),
        (PlaybookId::OracleVoid, PlaybookTrigger::OracleDeadlock),
        (PlaybookId::HaltIntake, PlaybookTrigger::GateBreach),
        (PlaybookId::Reserve, PlaybookTrigger::ReserveHealth),
        (PlaybookId::LedgerFreeze, PlaybookTrigger::LedgerDrift),
    ]
}

#[test]
fn each_playbook_activates_on_its_matching_live_trigger() {
    new_test_ext().execute_with(|| {
        set_triggers(all_triggers());
        for (i, (pb, trig)) in playbook_pairs().iter().enumerate() {
            let aid = i as ActionId;
            assert_ok!(Guardian::propose_action(
                RuntimeOrigin::signed(acct(1)),
                GuardianPower::ActivatePlaybook {
                    id: *pb,
                    trigger: *trig,
                    expiry: 100_000,
                    target: (*pb == PlaybookId::OracleVoid).then_some(0),
                },
                hash(1)
            ));
            approve_span(aid, 2, 5);
            assert!(Guardian::playbook_active(*pb));
            // 02 §6 frozen schema: PlaybookActivated { id, trigger, expiry }.
            assert!(last_events().iter().any(|e| matches!(
                e,
                Event::PlaybookActivated { id, trigger, expiry }
                    if id == pb && trigger == trig && *expiry == 100_000
            )));
        }
        assert_eq!(
            ActivePlaybooks::<Test>::get().len() as u32,
            crate::MAX_ACTIVE_PLAYBOOKS
        );
        assert_ok!(Guardian::do_try_state());
    });
}

#[test]
fn activate_playbook_rejects_wrong_pairing_and_inactive_trigger() {
    new_test_ext().execute_with(|| {
        // Wrong pairing is refused *before* liveness (BadPlaybookTrigger), even
        // with every trigger live.
        set_triggers(all_triggers());
        assert_ok!(Guardian::propose_action(
            RuntimeOrigin::signed(acct(1)),
            GuardianPower::ActivatePlaybook {
                id: PlaybookId::Depeg,
                trigger: PlaybookTrigger::LedgerDrift,
                expiry: 100,
                target: None,
            },
            hash(1)
        ));
        for n in 2..=4u8 {
            assert_ok!(Guardian::approve_action(RuntimeOrigin::signed(acct(n)), 0));
        }
        assert_noop!(
            Guardian::approve_action(RuntimeOrigin::signed(acct(5)), 0),
            Error::<Test>::BadPlaybookTrigger
        );
        // Correct pairing but the trigger is not live ⇒ TriggerInactive.
        set_triggers(TriggerState::none());
        assert_ok!(Guardian::propose_action(
            RuntimeOrigin::signed(acct(1)),
            GuardianPower::ActivatePlaybook {
                id: PlaybookId::Depeg,
                trigger: PlaybookTrigger::DepegMedian,
                expiry: 100,
                target: None,
            },
            hash(1)
        ));
        for n in 2..=4u8 {
            assert_ok!(Guardian::approve_action(RuntimeOrigin::signed(acct(n)), 1));
        }
        assert_noop!(
            Guardian::approve_action(RuntimeOrigin::signed(acct(5)), 1),
            Error::<Test>::TriggerInactive
        );
        assert_ok!(Guardian::do_try_state());
    });
}

#[test]
fn frozen_event_schema_force_rerun_guardian_action_and_review() {
    new_test_ext().execute_with(|| {
        set_status(ProposalStatus::Queued, false);
        assert_ok!(Guardian::propose_action(
            RuntimeOrigin::signed(acct(1)),
            GuardianPower::ForceRerun { pid: 7 },
            hash(3)
        ));
        approve_span(0, 2, 5);
        let now = frame_system::Pallet::<Test>::block_number() as u32;
        // 02 §6: ForceRerun { pid, justification_hash, window_end }.
        assert!(last_events().iter().any(|e| matches!(
            e,
            Event::ForceRerun { pid: 7, window_end, .. }
                if *window_end == now + FORCE_RERUN_WINDOW_BLOCKS
        )));
        assert!(last_events()
            .iter()
            .any(|e| matches!(e, Event::GuardianAction { action_id: 0, .. })));
        assert!(last_events().iter().any(|e| matches!(
            e,
            Event::ReviewScheduled {
                action: 0,
                referendum: 100
            }
        )));
        assert_ok!(Guardian::do_try_state());
    });
}

// ---------------------------------------------------- limit / boundary --

#[test]
fn pending_actions_fill_to_max_then_reject() {
    new_test_ext().execute_with(|| {
        for _ in 0..crate::MAX_PENDING_ACTIONS {
            assert_ok!(Guardian::propose_action(
                RuntimeOrigin::signed(acct(1)),
                GuardianPower::SuspendOnGate,
                hash(0)
            ));
        }
        assert_eq!(
            PendingActions::<Test>::get().len() as u32,
            crate::MAX_PENDING_ACTIONS
        );
        assert_noop!(
            Guardian::propose_action(
                RuntimeOrigin::signed(acct(1)),
                GuardianPower::SuspendOnGate,
                hash(0)
            ),
            Error::<Test>::TooManyPending
        );
        assert_ok!(Guardian::do_try_state());
    });
}

#[test]
fn all_six_playbooks_fill_and_reactivation_renews_in_place() {
    // 06 §6.2: playbooks are enumerated singletons. All six distinct playbooks
    // fill the slots; re-activating an already-active non-LedgerFreeze playbook
    // renews its expiry in place rather than appending a duplicate (A10 Codex
    // finding: duplicates could otherwise exhaust the slots).
    new_test_ext().execute_with(|| {
        set_triggers(all_triggers());
        for (i, (pb, trig)) in playbook_pairs().iter().enumerate() {
            assert_ok!(Guardian::propose_action(
                RuntimeOrigin::signed(acct(1)),
                GuardianPower::ActivatePlaybook {
                    id: *pb,
                    trigger: *trig,
                    expiry: 100_000,
                    target: (*pb == PlaybookId::OracleVoid).then_some(0),
                },
                hash(1)
            ));
            approve_span(i as ActionId, 2, 5);
        }
        assert_eq!(
            ActivePlaybooks::<Test>::get().len() as u32,
            crate::MAX_ACTIVE_PLAYBOOKS
        );
        // Re-activate Depeg (already active) with a later expiry.
        assert_ok!(Guardian::propose_action(
            RuntimeOrigin::signed(acct(1)),
            GuardianPower::ActivatePlaybook {
                id: PlaybookId::Depeg,
                trigger: PlaybookTrigger::DepegMedian,
                expiry: 200_000,
                target: None,
            },
            hash(1)
        ));
        approve_span(6, 2, 5);
        // Still six records (renewed in place, not appended); Depeg's expiry moved.
        assert_eq!(
            ActivePlaybooks::<Test>::get().len() as u32,
            crate::MAX_ACTIVE_PLAYBOOKS
        );
        let depeg = ActivePlaybooks::<Test>::get()
            .into_iter()
            .find(|p| p.id == PlaybookId::Depeg)
            .expect("Depeg active");
        assert_eq!(depeg.expiry, 200_000);
        assert_ok!(Guardian::do_try_state());
    });
}

#[test]
fn duplicate_depeg_activations_never_block_ledger_freeze() {
    // The security property behind the dedup (A10 Codex finding): repeatedly
    // activating a non-ledger playbook cannot exhaust the six slots, so an honest
    // council's PB-LEDGER-FREEZE always has room.
    new_test_ext().execute_with(|| {
        set_triggers(all_triggers());
        for round in 0..6u32 {
            assert_ok!(Guardian::propose_action(
                RuntimeOrigin::signed(acct(1)),
                GuardianPower::ActivatePlaybook {
                    id: PlaybookId::Depeg,
                    trigger: PlaybookTrigger::DepegMedian,
                    expiry: 100_000 + round,
                    target: None,
                },
                hash(1)
            ));
            approve_span(round as ActionId, 2, 5);
        }
        // Six activations collapsed into a single Depeg record.
        assert_eq!(ActivePlaybooks::<Test>::get().len(), 1);
        // LedgerFreeze still activates.
        assert_ok!(Guardian::propose_action(
            RuntimeOrigin::signed(acct(1)),
            GuardianPower::ActivatePlaybook {
                id: PlaybookId::LedgerFreeze,
                trigger: PlaybookTrigger::LedgerDrift,
                expiry: 100_000,
                target: None,
            },
            hash(1)
        ));
        approve_span(6, 2, 5);
        assert!(Guardian::playbook_active(PlaybookId::LedgerFreeze));
        assert_ok!(Guardian::do_try_state());
    });
}

#[test]
fn duplicate_active_ledger_freeze_is_rejected_through_the_shell() {
    new_test_ext().execute_with(|| {
        set_triggers(TriggerState {
            ledger_drift: true,
            ..TriggerState::none()
        });
        for (aid, expiry) in [(0u32, 100_000u32), (1, 120_000)] {
            assert_ok!(Guardian::propose_action(
                RuntimeOrigin::signed(acct(1)),
                GuardianPower::ActivatePlaybook {
                    id: PlaybookId::LedgerFreeze,
                    trigger: PlaybookTrigger::LedgerDrift,
                    expiry,
                    target: None,
                },
                hash(1)
            ));
            if aid == 0 {
                approve_span(aid, 2, 5);
            } else {
                for n in 2..=4u8 {
                    assert_ok!(Guardian::approve_action(
                        RuntimeOrigin::signed(acct(n)),
                        aid
                    ));
                }
                // 06 §6.3: a single live PB-LEDGER-FREEZE — the dedup fires.
                assert_noop!(
                    Guardian::approve_action(RuntimeOrigin::signed(acct(5)), aid),
                    Error::<Test>::PlaybookAlreadyActive
                );
            }
        }
        assert_eq!(ActivePlaybooks::<Test>::get().len(), 1);
        assert_ok!(Guardian::do_try_state());
    });
}

// ------------------------------------------------- maintenance crank --

#[test]
fn maintenance_crank_expires_playbook_and_reverts() {
    new_test_ext().execute_with(|| {
        set_triggers(TriggerState {
            ledger_drift: true,
            ..TriggerState::none()
        });
        // Short-lived freeze: expiry at block 5.
        assert_ok!(Guardian::propose_action(
            RuntimeOrigin::signed(acct(1)),
            GuardianPower::ActivatePlaybook {
                id: PlaybookId::LedgerFreeze,
                trigger: PlaybookTrigger::LedgerDrift,
                expiry: 5,
                target: None,
            },
            hash(1)
        ));
        approve_span(0, 2, 5);
        assert!(Guardian::playbook_active(PlaybookId::LedgerFreeze));
        // 06 §6.2: expiry emits PlaybookExpired and reverts the playbook.
        run_to_block(6);
        assert!(!Guardian::playbook_active(PlaybookId::LedgerFreeze));
        assert!(last_events().iter().any(|e| matches!(
            e,
            Event::PlaybookExpired {
                id: PlaybookId::LedgerFreeze
            }
        )));
        assert_ok!(Guardian::do_try_state());
    });
}

#[test]
fn ratified_review_is_not_slashed_by_the_crank() {
    new_test_ext().execute_with(|| {
        set_triggers(TriggerState {
            gate_breach: true,
            ..TriggerState::none()
        });
        assert_ok!(Guardian::propose_action(
            RuntimeOrigin::signed(acct(1)),
            GuardianPower::SuspendOnGate,
            hash(2)
        ));
        approve_span(0, 2, 5);
        // Ratify within the window (06 §5.4), then crank well past the deadline.
        assert_ok!(Guardian::ratify_action(values_origin(), 0));
        set_epoch(5);
        run_to_block(3);
        // No slash: bonds are intact and no ReviewFailed was emitted.
        assert_eq!(MemberBonds::<Test>::get(), [GUARDIAN_BOND; GUARDIAN_SEATS]);
        assert!(!last_events()
            .iter()
            .any(|e| matches!(e, Event::ReviewFailed { .. })));
        assert_ok!(Guardian::do_try_state());
    });
}

// ============================================================================
// Seeded shell-vs-core randomized differential (the A10 analogue of A1's
// 600-step ledger differential; the key correctness artifact). A deterministic
// LCG drives the SAME operation sequence into the pallet (via extrinsics) and a
// reference `guardian_core::Guardian` aggregate (driven directly with a
// DispatchContext mirrored from the mock's status/trigger feeds). After every
// step the full decomposed storage MUST equal the oracle's aggregate. Ranges
// are kept slack (small pid/expiry sets) so neither a pallet-only cap
// (`MAX_RERUN_USED`, …) nor action expiry can desync the two.
// ============================================================================

/// Deterministic LCG (PCG/Knuth MMIX multiplier + increment) — tests must never
/// use ambient randomness. High bits are consumed (they mix best).
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0
    }
    /// Uniform in `0..n`, drawn from the well-mixed high bits.
    fn below(&mut self, n: u64) -> u64 {
        (self.next() >> 33) % n
    }
    fn boolean(&mut self) -> bool {
        self.next() >> 63 != 0
    }
}

/// The genesis council as raw 32-byte core accounts (`members()` mirror).
fn members_raw() -> [[u8; 32]; GUARDIAN_SEATS] {
    core::array::from_fn(|i| [(i as u8) + 1; 32])
}

/// A signed origin + its raw core account: mostly a random council member, and
/// (1-in-`outsider_in`) a non-member so the `NotMember` path is exercised too.
fn signer(rng: &mut Lcg, outsider_in: u64) -> (RuntimeOrigin, [u8; 32]) {
    if rng.below(outsider_in) == 0 {
        (RuntimeOrigin::signed(acct(250)), [250u8; 32])
    } else {
        let m = rng.below(GUARDIAN_SEATS as u64) as u8 + 1;
        (RuntimeOrigin::signed(acct(m)), [m; 32])
    }
}

/// A random power with mostly-valid but occasionally out-of-bound parameters
/// (over-long durations, mismatched `(id, trigger)` pairings).
fn rng_power(rng: &mut Lcg, now: u32) -> GuardianPower {
    match rng.below(6) {
        0 => {
            let until = if rng.below(5) == 0 {
                now.saturating_add(300_000) // > HOLD_MAX ⇒ DurationTooLong
            } else {
                now + rng.below(1_000) as u32 + 1
            };
            GuardianPower::PauseIntake { until }
        }
        1 => GuardianPower::DelayOnce { pid: rng.below(8) },
        2 => GuardianPower::ForceRerun { pid: rng.below(8) },
        3 | 4 => {
            let (id, trigger) = match rng.below(8) {
                0 => (PlaybookId::Depeg, PlaybookTrigger::DepegMedian),
                1 => (PlaybookId::Migration, PlaybookTrigger::MigrationHalt),
                2 => (PlaybookId::OracleVoid, PlaybookTrigger::OracleDeadlock),
                3 => (PlaybookId::HaltIntake, PlaybookTrigger::GateBreach),
                4 => (PlaybookId::HaltIntake, PlaybookTrigger::DeadMan),
                5 => (PlaybookId::Reserve, PlaybookTrigger::ReserveHealth),
                6 => (PlaybookId::LedgerFreeze, PlaybookTrigger::LedgerDrift),
                _ => (PlaybookId::Depeg, PlaybookTrigger::LedgerDrift), // mismatch
            };
            let expiry = if rng.below(6) == 0 {
                now.saturating_add(300_000)
            } else {
                now + rng.below(5_000) as u32 + 1
            };
            GuardianPower::ActivatePlaybook {
                id,
                trigger,
                expiry,
                target: (id == PlaybookId::OracleVoid).then_some(rng.below(8) as u32),
            }
        }
        _ => GuardianPower::SuspendOnGate,
    }
}

fn rng_status(rng: &mut Lcg) -> (ProposalStatus, bool) {
    let s = match rng.below(6) {
        0 => ProposalStatus::Trading,
        1 => ProposalStatus::Extended,
        2 => ProposalStatus::Queued,
        3 => ProposalStatus::Executed,
        4 => ProposalStatus::Rerun,
        _ => ProposalStatus::Other,
    };
    (s, rng.below(4) == 0)
}

fn rng_triggers(rng: &mut Lcg) -> TriggerState {
    let b = rng.next();
    TriggerState {
        depeg: b & 1 != 0,
        migration_halt: b & (1 << 1) != 0,
        oracle_deadlock: b & (1 << 2) != 0,
        gate_breach: b & (1 << 3) != 0,
        dead_man: b & (1 << 4) != 0,
        void_in_flight: b & (1 << 5) != 0,
        reserve_health: b & (1 << 6) != 0,
        ledger_drift: b & (1 << 7) != 0,
    }
}

/// Byte-for-byte reimplementation of the pallet's `dispatch_context_for`: the
/// trigger set is the live feed; the proposal status is the live feed only when
/// the action's power targets a pid, else `(Other, false)`.
fn mirror_ctx(
    g: &guardian_core::Guardian,
    id: ActionId,
    status: (ProposalStatus, bool),
    triggers: TriggerState,
) -> DispatchContext {
    let (proposal_status, in_rerun) = g
        .pending
        .iter()
        .find(|a| a.id == id)
        .and_then(|a| match a.power {
            GuardianPower::DelayOnce { pid } | GuardianPower::ForceRerun { pid } => Some(pid),
            _ => None,
        })
        .map(|_| status)
        .unwrap_or((ProposalStatus::Other, false));
    DispatchContext {
        proposal_status,
        in_rerun,
        triggers,
    }
}

/// Every observable field of the decomposed storage vs the oracle aggregate.
fn assert_shell_matches_oracle(g: &guardian_core::Guardian, step: u32) {
    assert_eq!(
        Members::<Test>::get().expect("council present"),
        g.members,
        "members diverged at step {step}"
    );
    assert_eq!(
        MemberBonds::<Test>::get(),
        g.member_bonds,
        "member_bonds diverged at step {step}"
    );
    assert_eq!(
        PendingActions::<Test>::get().into_inner(),
        g.pending,
        "pending diverged at step {step}"
    );
    assert_eq!(
        Approvals::<Test>::get().into_inner(),
        g.approvals,
        "approvals diverged at step {step}"
    );
    assert_eq!(
        ReviewDeadlines::<Test>::get().into_inner(),
        g.reviews,
        "reviews diverged at step {step}"
    );
    assert_eq!(
        ActivePlaybooks::<Test>::get().into_inner(),
        g.active_playbooks,
        "active_playbooks diverged at step {step}"
    );
    assert_eq!(
        RerunUsed::<Test>::get().into_inner(),
        g.rerun_used,
        "rerun_used diverged at step {step}"
    );
    assert_eq!(
        NextActionId::<Test>::get(),
        g.next_action_id,
        "next_action_id diverged at step {step}"
    );
    let a = Allowances::<Test>::get();
    assert_eq!(
        a.delay_used_this_epoch, g.delay_used_this_epoch,
        "delay allowance diverged at step {step}"
    );
    assert_eq!(
        a.force_rerun_used_this_epoch, g.force_rerun_used_this_epoch,
        "force_rerun allowance diverged at step {step}"
    );
    assert_eq!(
        a.pause_window_start, g.pause_used_epoch_window_start,
        "pause window start diverged at step {step}"
    );
    assert_eq!(
        a.pause_used_in_window, g.pause_used_in_window,
        "pause allowance diverged at step {step}"
    );
}

#[test]
fn shell_and_core_agree_over_randomized_guardian_operations() {
    new_test_ext().execute_with(|| {
        // This differential targets the frame-free state machine. Real hold
        // custody has dedicated tests below, so suppress fronting here to keep
        // the oracle and shell state spaces identical across 500 operations.
        ReviewDepositValue::set(0);
        let mut rng = Lcg(0xA10B_1EA0_17C0_FFEE);
        let mut oracle = guardian_core::Guardian::new(members_raw()).unwrap();
        let mut epoch: u32 = 0;
        // The maintenance crank now reaps terminal reviews, so `ReviewDeadlines`
        // is empty by loop end even though dispatches occurred — track the peak
        // to prove the dispatch path was actually exercised.
        let mut peak_reviews = 0usize;

        for step in 0..500u32 {
            // Advance the epoch monotonically in small strides. The pallet only
            // applies the reset lazily (on the next sync_epoch / on_initialize),
            // so the oracle mirrors set_epoch at exactly those points below.
            if rng.below(3) == 0 {
                epoch += 1 + rng.below(2) as u32;
                set_epoch(epoch);
            }
            // Vary the cross-pallet feeds every step (adversarial DispatchContext).
            let status = rng_status(&mut rng);
            let triggers = rng_triggers(&mut rng);
            set_status(status.0, status.1);
            set_triggers(triggers);

            let now = frame_system::Pallet::<Test>::block_number() as u32;

            // Mirror FRAME's transactional dispatch: an extrinsic that returns
            // Err rolls back ALL of its storage writes — including the lazy
            // `sync_epoch` allowance reset. Snapshot the oracle and restore it
            // whenever the shell call fails, so the two stay bit-identical. (The
            // maintenance crank runs in `on_initialize`, a hook whose writes
            // commit unconditionally — it is never restored.)
            let snapshot = oracle.clone();
            match rng.below(10) {
                0..=2 => {
                    let power = rng_power(&mut rng, now);
                    let (origin, who) = signer(&mut rng, 8);
                    let jh = hash((step % 251) as u8);
                    oracle.set_epoch(epoch); // mirrors the shell's sync_epoch
                    let shell = Guardian::propose_action(origin, power, jh);
                    let model = oracle
                        .propose_action(who, power, jh, now)
                        .map(|_| ())
                        .map_err(crate::Pallet::<Test>::map_core_error);
                    assert_eq!(shell, model, "propose result diverged at step {step}");
                    if shell.is_err() {
                        oracle = snapshot;
                    }
                }
                3..=6 => {
                    // Concentrate most approvals on the few most-recent actions
                    // so 5-of-7 actually accumulates and real dispatches (hence
                    // reviews, slashing, playbooks) happen every run; a quarter
                    // stay fully random to exercise ActionNotFound / stale ids.
                    let n = oracle.next_action_id;
                    let id = if n == 0 || rng.below(4) == 0 {
                        rng.below(u64::from(n) + 2) as ActionId
                    } else {
                        n.saturating_sub(1 + rng.below(4) as u32)
                    };
                    let (origin, who) = signer(&mut rng, 10);
                    oracle.set_epoch(epoch);
                    let ctx = mirror_ctx(&oracle, id, status, triggers);
                    let shell = Guardian::approve_action(origin, id);
                    let model = oracle
                        .approve_action(who, id, now, ctx)
                        .map(|_| ())
                        .map_err(crate::Pallet::<Test>::map_core_error);
                    assert_eq!(
                        shell, model,
                        "approve result diverged at step {step} (id {id})"
                    );
                    if shell.is_err() {
                        oracle = snapshot;
                    }
                }
                7 => {
                    let id = rng.below(u64::from(oracle.next_action_id) + 2) as ActionId;
                    if rng.below(4) == 0 {
                        // Intentional origin misuse (BadOrigin on both sides).
                        let shell = Guardian::ratify_action(RuntimeOrigin::signed(acct(1)), id);
                        let model = oracle
                            .ratify_action(GuardianOrigin::Signed, id)
                            .map_err(crate::Pallet::<Test>::map_core_error);
                        assert_eq!(shell, model, "ratify(bad origin) diverged at step {step}");
                        if shell.is_err() {
                            oracle = snapshot;
                        }
                    } else {
                        let shell = Guardian::ratify_action(values_origin(), id);
                        let model = oracle
                            .ratify_action(GuardianOrigin::ConstitutionalValues, id)
                            .map_err(crate::Pallet::<Test>::map_core_error);
                        assert_eq!(shell, model, "ratify result diverged at step {step}");
                        if shell.is_err() {
                            oracle = snapshot;
                        }
                    }
                }
                8 => {
                    let id = if rng.boolean() {
                        PlaybookId::LedgerFreeze
                    } else {
                        PlaybookId::Depeg // non-LedgerFreeze ⇒ RenewalNotAllowed
                    };
                    oracle.set_epoch(epoch);
                    let shell = Guardian::renew_playbook(values_origin(), id);
                    let model = oracle
                        .renew_playbook(GuardianOrigin::ConstitutionalValues, id, now)
                        .map_err(crate::Pallet::<Test>::map_core_error);
                    assert_eq!(shell, model, "renew result diverged at step {step}");
                    if shell.is_err() {
                        oracle = snapshot;
                    }
                }
                _ => {
                    // The maintenance crank across a few blocks: on_initialize
                    // runs sync_epoch + expire_playbooks + enforce_reviews +
                    // reap_terminal. All are idempotent, so a single mirrored
                    // call at the final block/epoch reproduces the end state.
                    let target = now + 1 + rng.below(40) as u32;
                    run_to_block(target);
                    let after = frame_system::Pallet::<Test>::block_number() as u32;
                    oracle.set_epoch(epoch);
                    oracle.expire_playbooks(after);
                    let _ = oracle.enforce_reviews(epoch);
                    oracle.reap_terminal(after);
                }
            }

            oracle.events.clear(); // events are drained, not stored — not compared
            assert_shell_matches_oracle(&oracle, step);
            peak_reviews = peak_reviews.max(oracle.reviews.len());

            if step % 50 == 49 {
                assert_ok!(Guardian::do_try_state());
                oracle.try_state().unwrap();
            }
        }

        assert_ok!(Guardian::do_try_state());
        oracle.try_state().unwrap();
        // The run must actually reach the interesting states, not idle in
        // BadOrigin/TooManyPending: prove real proposals were made and real
        // guardian actions dispatched (each writes a review record).
        assert!(
            NextActionId::<Test>::get() >= 30,
            "differential barely proposed ({} actions)",
            NextActionId::<Test>::get()
        );
        assert!(
            peak_reviews > 0,
            "differential never dispatched a guardian action (no review ever created)"
        );
    });
}

// ------------------------------------------- re-election / reaping / recall --

#[test]
fn re_election_clears_pending_and_approvals_blocking_stale_dispatch() {
    // 06 §5.1/§5.4 (spec-reviewer major: stale-approval 5-of-7 bypass). A
    // re-election drops the outgoing council's un-dispatched actions + approvals
    // so a recalled member's live approval can never carry over.
    new_test_ext().execute_with(|| {
        set_triggers(all_triggers());
        assert_ok!(Guardian::propose_action(
            RuntimeOrigin::signed(acct(1)),
            GuardianPower::SuspendOnGate,
            hash(1)
        ));
        approve_span(0, 2, 4); // proposer + three = four of five
        assert_eq!(PendingActions::<Test>::get().len(), 1);
        assert_eq!(Approvals::<Test>::get().len(), 4);

        let new_council: [AccountId32; 7] = core::array::from_fn(|i| acct((i as u8) + 10));
        for member in &new_council {
            assert_ok!(<Balances as Mutate<AccountId32>>::mint_into(
                member,
                2 * GUARDIAN_BOND
            ));
        }
        assert_ok!(Guardian::set_members(values_origin(), new_council));

        assert!(PendingActions::<Test>::get().is_empty());
        assert!(Approvals::<Test>::get().is_empty());
        assert_ok!(Guardian::do_try_state());
    });
}

#[test]
fn maintenance_crank_reaps_dispatched_action_and_terminal_review() {
    // 06 §5.1/§5.4 (spec-reviewer majors: lifetime→concurrent caps). The crank
    // reclaims a dispatched action and its ratified review.
    new_test_ext().execute_with(|| {
        set_triggers(all_triggers());
        assert_ok!(Guardian::propose_action(
            RuntimeOrigin::signed(acct(1)),
            GuardianPower::SuspendOnGate,
            hash(2)
        ));
        approve_to_dispatch();
        assert!(PendingActions::<Test>::get()[0].dispatched);
        assert_eq!(ReviewDeadlines::<Test>::get().len(), 1);

        assert_ok!(Guardian::ratify_action(values_origin(), 0));
        run_to_block(2);
        assert!(PendingActions::<Test>::get().is_empty());
        assert!(ReviewDeadlines::<Test>::get().is_empty());
        assert_ok!(Guardian::do_try_state());
    });
}

#[test]
fn review_failure_slashes_and_schedules_recall() {
    // 06 §5.4: a missed 2-epoch deadline slashes each approver 50% AND
    // auto-schedules a recall referendum on the `guardian` track. The
    // ordinary ratify review is settled at the deadline; the separate veto
    // referendum remains bounded until T12 (SQ-311).
    new_test_ext().execute_with(|| {
        reset_review_scheduler();
        set_status(ProposalStatus::Queued, false);
        assert_ok!(Guardian::propose_action(
            RuntimeOrigin::signed(acct(1)),
            GuardianPower::DelayOnce { pid: 4 },
            hash(4)
        ));
        approve_to_dispatch();
        assert_eq!(
            ScheduledReviews::get(),
            vec![
                (0, ReviewVerdict::Ratify, 100),
                (0, ReviewVerdict::UpholdVeto, 101),
            ]
        );
        assert_eq!(ReviewReferenda::<Test>::get(0), Some(100));
        assert_eq!(VetoReviewReferenda::<Test>::get(0), Some(101));
        let fronting = ReviewFrontingOf::<Test>::get(0);
        assert!(fronting.is_some());
        let Some(fronting) = fronting else {
            return;
        };
        assert_eq!(
            fronting.slices.iter().copied().sum::<u128>(),
            2 * ReviewDepositValue::get()
        );

        set_epoch(3); // strictly after dispatch epoch (0) + deadline (2)
        run_to_block(3);
        assert!(last_events()
            .iter()
            .any(|e| matches!(e, Event::ReviewFailed { action: 0, .. })));
        assert!(last_events().iter().any(|e| matches!(
            e,
            Event::RecallScheduled {
                action: 0,
                referendum: 102
            }
        )));
        assert_eq!(CancelledReviews::get(), vec![100]);
        assert_eq!(RefundedReviews::get(), vec![100]);
        assert_eq!(MemberBonds::<Test>::get()[..5], [GUARDIAN_BOND / 2; 5]);
        let failed_fronting =
            ReviewFrontingOf::<Test>::get(0).expect("failed delay retains veto fronting");
        for member in members().iter().take(5) {
            let position = members()
                .iter()
                .position(|candidate| candidate == member)
                .expect("member is seated");
            assert_eq!(
                seat_hold(member) + failed_fronting.slices[position],
                GUARDIAN_BOND / 2
            );
        }
        assert!(FailedActions::<Test>::contains_key(0));
        assert!(!ReviewReferenda::<Test>::contains_key(0));
        assert_eq!(VetoReviewReferenda::<Test>::get(0), Some(101));
        assert_eq!(VetoReviewActions::<Test>::get(4), Some(0));
        assert_eq!(
            failed_fronting.slices.iter().copied().sum::<u128>(),
            ReviewDepositValue::get()
        );
        assert_ok!(Guardian::do_try_state());
    });
}

#[test]
fn t12_closes_only_veto_and_leaves_accountability_review_live() {
    new_test_ext().execute_with(|| {
        reset_review_scheduler();
        set_status(ProposalStatus::Queued, false);
        assert_ok!(Guardian::propose_action(
            RuntimeOrigin::signed(acct(1)),
            GuardianPower::DelayOnce { pid: 42 },
            hash(42)
        ));
        approve_to_dispatch();

        // T12 happens in the next epoch's Seed phase, before the separate
        // accountability deadline. Closing it must not erase ratification or
        // the core action record.
        assert_ok!(Guardian::close_review_window(42));
        assert_eq!(CancelledReviews::get(), vec![101]);
        assert_eq!(RefundedReviews::get(), vec![101]);
        assert_eq!(ReviewReferenda::<Test>::get(0), Some(100));
        assert!(!VetoReviewReferenda::<Test>::contains_key(0));
        assert!(!VetoReviewActions::<Test>::contains_key(42));
        assert_eq!(
            ReviewFrontingOf::<Test>::get(0)
                .expect("ordinary review remains fronted")
                .slices
                .iter()
                .copied()
                .sum::<u128>(),
            ReviewDepositValue::get()
        );
        assert_ok!(Guardian::do_try_state());

        // The ordinary review still reaches the strict deadline and follows
        // the normal slash/recall path with no surviving veto liability.
        set_epoch(3);
        run_to_block(3);
        assert!(last_events()
            .iter()
            .any(|event| matches!(event, Event::ReviewFailed { action: 0, .. })));
        assert!(!ReviewFrontingOf::<Test>::contains_key(0));
        assert!(!ReviewReferenda::<Test>::contains_key(0));
        assert!(!VetoReviewReferenda::<Test>::contains_key(0));
        assert_ok!(Guardian::do_try_state());
    });
}

#[test]
fn close_review_window_refuses_a_wrong_reverse_join_before_side_effects() {
    new_test_ext().execute_with(|| {
        reset_review_scheduler();
        set_status(ProposalStatus::Queued, false);
        assert_ok!(Guardian::propose_action(
            RuntimeOrigin::signed(acct(1)),
            GuardianPower::DelayOnce { pid: 42 },
            hash(42)
        ));
        approve_to_dispatch();
        let action = VetoReviewActions::<Test>::get(42).expect("reverse join");
        VetoReviewActions::<Test>::insert(99, action);

        assert_noop!(
            Guardian::close_review_window(99),
            Error::<Test>::FailedActionNotFound
        );
        assert!(CancelledReviews::get().is_empty());
        assert!(VetoReviewReferenda::<Test>::contains_key(action));
    });
}

#[test]
fn recall_before_t12_keeps_returned_veto_fronting_releasable() {
    new_test_ext().execute_with(|| {
        reset_review_scheduler();
        set_status(ProposalStatus::Queued, false);
        assert_ok!(Guardian::propose_action(
            RuntimeOrigin::signed(acct(1)),
            GuardianPower::DelayOnce { pid: 77 },
            hash(77)
        ));
        approve_to_dispatch();
        set_epoch(3);
        run_to_block(3);

        // Recall may enact before the keeper observes T12. The review deposit
        // remains in the sovereign until the proposal-specific close.
        assert_ok!(Guardian::recall(values_origin(), 0));
        assert_eq!(PendingBondReleases::<Test>::get().len(), 5);
        assert_ok!(Guardian::close_review_window(77));
        assert!(!ReviewFrontingOf::<Test>::contains_key(0));
        assert!(!VetoReviewActions::<Test>::contains_key(77));

        set_epoch(4);
        run_to_block(4);
        assert!(PendingBondReleases::<Test>::get().is_empty());
        for member in members().into_iter().take(5) {
            assert_eq!(seat_hold(&member), 0);
        }
        assert_ok!(Guardian::do_try_state());
    });
}

#[test]
fn three_concurrent_failed_reviews_settle_independently_with_bounded_liability() {
    new_test_ext().execute_with(|| {
        reset_review_scheduler();
        set_status(ProposalStatus::Queued, false);
        for (action, power) in [
            (0, GuardianPower::DelayOnce { pid: 10 }),
            (1, GuardianPower::DelayOnce { pid: 11 }),
            (2, GuardianPower::ForceRerun { pid: 12 }),
        ] {
            assert_ok!(Guardian::propose_action(
                RuntimeOrigin::signed(acct(1)),
                power,
                hash(action as u8),
            ));
            for member in 2..=5u8 {
                assert_ok!(Guardian::approve_action(
                    RuntimeOrigin::signed(acct(member)),
                    action,
                ));
            }
        }
        assert_eq!(ReviewDeadlines::<Test>::get().len(), 3);
        assert_eq!(
            ScheduledReviews::get(),
            vec![
                (0, ReviewVerdict::Ratify, 100),
                (0, ReviewVerdict::UpholdVeto, 101),
                (1, ReviewVerdict::Ratify, 102),
                (1, ReviewVerdict::UpholdVeto, 103),
                (2, ReviewVerdict::Ratify, 104),
            ]
        );
        assert_eq!(NextReferendum::get(), 105);
        for action in 0..2 {
            let fronting = ReviewFrontingOf::<Test>::get(action);
            assert!(fronting.is_some());
            let Some(fronting) = fronting else {
                return;
            };
            assert_eq!(
                fronting.slices.iter().copied().sum::<u128>(),
                2 * ReviewDepositValue::get()
            );
        }
        let force_fronting = ReviewFrontingOf::<Test>::get(2);
        assert!(force_fronting.is_some());
        let Some(force_fronting) = force_fronting else {
            return;
        };
        assert_eq!(
            force_fronting.slices.iter().copied().sum::<u128>(),
            ReviewDepositValue::get()
        );
        assert_ok!(Guardian::do_try_state());

        // Isolate the middle review's refund failure. The first and third
        // reviews must commit independently instead of sharing one rollback.
        ReviewRefundFailsFor::set(Some(102));
        set_epoch(3);
        run_to_block(3);
        assert!(FailedActions::<Test>::contains_key(0));
        assert!(!FailedActions::<Test>::contains_key(1));
        assert!(FailedActions::<Test>::contains_key(2));
        assert!(ReviewFrontingOf::<Test>::contains_key(1));
        assert!(ReviewFrontingOf::<Test>::contains_key(0));
        assert!(!ReviewFrontingOf::<Test>::contains_key(2));
        assert!(!ReviewReferenda::<Test>::contains_key(0));
        assert_eq!(ReviewReferenda::<Test>::get(1), Some(102));
        assert!(!ReviewReferenda::<Test>::contains_key(2));
        assert_eq!(VetoReviewReferenda::<Test>::get(0), Some(101));
        assert_eq!(VetoReviewReferenda::<Test>::get(1), Some(103));
        assert!(!VetoReviewReferenda::<Test>::contains_key(2));

        let stranded = ReviewFrontingOf::<Test>::get(1);
        assert!(stranded.is_some(), "failed review must remain retryable");
        let Some(stranded) = stranded else {
            return;
        };
        assert_eq!(
            stranded.slices.iter().copied().sum::<u128>(),
            2 * ReviewDepositValue::get()
        );
        let bonds = MemberBonds::<Test>::get();
        for (position, bond) in bonds.iter().take(5).enumerate() {
            assert!(
                *bond >= stranded.slices[position],
                "the remaining obligation covers the stranded fronting slice"
            );
        }
        assert_ok!(Guardian::do_try_state());

        // Retrying only the failed review consumes the remaining bounded
        // liability. No exact transfer can demand more than the live hold.
        ReviewRefundFailsFor::set(None);
        run_to_block(4);
        assert!(FailedActions::<Test>::contains_key(0));
        assert!(FailedActions::<Test>::contains_key(1));
        assert!(FailedActions::<Test>::contains_key(2));
        assert!(ReviewFrontingOf::<Test>::contains_key(0));
        assert!(ReviewFrontingOf::<Test>::contains_key(1));
        assert!(!ReviewFrontingOf::<Test>::contains_key(2));
        assert_eq!(VetoReviewReferenda::<Test>::get(0), Some(101));
        assert_eq!(VetoReviewReferenda::<Test>::get(1), Some(103));

        // Both delayed actions remain admissible for T24 until their
        // proposal-specific T12 close is observed.
        assert_ok!(Guardian::close_review_window(10));
        assert_ok!(Guardian::close_review_window(11));
        for action in 0..3 {
            assert!(FailedActions::<Test>::contains_key(action));
            assert!(!ReviewReferenda::<Test>::contains_key(action));
            assert!(!VetoReviewReferenda::<Test>::contains_key(action));
            assert!(!ReviewFrontingOf::<Test>::contains_key(action));
        }
        let residual_obligation = 2 * ReviewDepositValue::get() / 5;
        assert!(MemberBonds::<Test>::get()[..5]
            .iter()
            .all(|bond| *bond == residual_obligation));
        let reason = RuntimeHoldReason::Guardian(crate::HoldReason::SeatBond);
        for member in members().iter().take(5) {
            assert_eq!(
                <Balances as InspectHold<AccountId32>>::balance_on_hold(&reason, member),
                residual_obligation,
                "unexpected seat hold after all veto closes for {member:?}"
            );
        }
        assert_ok!(Guardian::do_try_state());
    });
}

#[test]
fn delay_ratification_cancels_uphold_and_refunds_both_reviews() {
    new_test_ext().execute_with(|| {
        reset_review_scheduler();
        set_status(ProposalStatus::Queued, false);
        assert_ok!(Guardian::propose_action(
            RuntimeOrigin::signed(acct(1)),
            GuardianPower::DelayOnce { pid: 42 },
            hash(8)
        ));
        approve_to_dispatch();

        assert_eq!(
            ScheduledReviews::get(),
            vec![
                (0, ReviewVerdict::Ratify, 100),
                (0, ReviewVerdict::UpholdVeto, 101),
            ]
        );
        assert_eq!(ReviewReferenda::<Test>::get(0), Some(100));
        assert_eq!(VetoReviewReferenda::<Test>::get(0), Some(101));
        let fronting = ReviewFrontingOf::<Test>::get(0);
        assert!(fronting.is_some());
        let Some(fronting) = fronting else {
            return;
        };
        assert_eq!(
            fronting.slices.iter().copied().sum::<u128>(),
            2 * ReviewDepositValue::get()
        );
        for (position, member) in members().iter().take(5).enumerate() {
            assert_eq!(
                seat_hold(member),
                GUARDIAN_BOND.saturating_sub(fronting.slices[position])
            );
        }

        assert_ok!(Guardian::ratify_action(values_origin(), 0));

        assert_eq!(CancelledReviews::get(), vec![101]);
        assert_eq!(RefundedReviews::get(), vec![100, 101]);
        assert!(ReviewDeadlines::<Test>::get()[0].ratified);
        assert!(!ReviewReferenda::<Test>::contains_key(0));
        assert!(!VetoReviewReferenda::<Test>::contains_key(0));
        assert!(!ReviewFrontingOf::<Test>::contains_key(0));
        for member in members().iter().take(5) {
            assert_eq!(seat_hold(member), GUARDIAN_BOND);
        }
        assert_ok!(Guardian::do_try_state());
    });
}

#[test]
fn underfunded_set_members_is_fully_atomic() {
    new_test_ext().execute_with(|| {
        let before_members = Members::<Test>::get();
        let before_holds: Vec<_> = members()
            .iter()
            .map(|member| {
                <Balances as InspectHold<AccountId32>>::balance_on_hold(
                    &RuntimeHoldReason::Guardian(crate::HoldReason::SeatBond),
                    member,
                )
            })
            .collect();
        let new_council: [AccountId32; 7] = core::array::from_fn(|i| acct((i as u8) + 20));
        assert!(Guardian::set_members(values_origin(), new_council).is_err());
        assert_eq!(Members::<Test>::get(), before_members);
        assert!(PendingBondReleases::<Test>::get().is_empty());
        let after_holds: Vec<_> = members()
            .iter()
            .map(|member| {
                <Balances as InspectHold<AccountId32>>::balance_on_hold(
                    &RuntimeHoldReason::Guardian(crate::HoldReason::SeatBond),
                    member,
                )
            })
            .collect();
        assert_eq!(after_holds, before_holds);
    });
}

#[test]
fn outgoing_bonds_release_one_epoch_after_re_election() {
    new_test_ext().execute_with(|| {
        let outgoing = members();
        let incoming: [AccountId32; 7] = core::array::from_fn(|i| acct((i as u8) + 30));
        for member in &incoming {
            assert_ok!(<Balances as Mutate<AccountId32>>::mint_into(
                member,
                2 * GUARDIAN_BOND
            ));
        }
        assert_ok!(Guardian::set_members(values_origin(), incoming));
        assert_eq!(PendingBondReleases::<Test>::get().len(), 7);
        for member in &outgoing {
            assert_eq!(
                <Balances as InspectHold<AccountId32>>::balance_on_hold(
                    &RuntimeHoldReason::Guardian(crate::HoldReason::SeatBond),
                    member,
                ),
                GUARDIAN_BOND
            );
        }
        set_epoch(1);
        run_to_block(2);
        assert!(PendingBondReleases::<Test>::get().is_empty());
        for member in outgoing {
            assert_eq!(
                <Balances as InspectHold<AccountId32>>::balance_on_hold(
                    &RuntimeHoldReason::Guardian(crate::HoldReason::SeatBond),
                    &member,
                ),
                0
            );
        }
        assert_ok!(Guardian::do_try_state());
    });
}

#[test]
fn recall_vacates_approvers_and_releases_residual_bonds_one_epoch_later() {
    new_test_ext().execute_with(|| {
        set_triggers(all_triggers());
        assert_ok!(Guardian::propose_action(
            RuntimeOrigin::signed(acct(1)),
            GuardianPower::SuspendOnGate,
            hash(7)
        ));
        approve_to_dispatch();
        set_epoch(3);
        run_to_block(3);
        assert_ok!(Guardian::recall(values_origin(), 0));
        let seats = Members::<Test>::get().expect("council remains initialized");
        assert_eq!(seats.iter().filter(|seat| seat.is_none()).count(), 5);
        assert_eq!(PendingBondReleases::<Test>::get().len(), 5);

        assert_ok!(Guardian::propose_action(
            RuntimeOrigin::signed(acct(6)),
            GuardianPower::SuspendOnGate,
            hash(8)
        ));
        assert_ok!(Guardian::approve_action(RuntimeOrigin::signed(acct(7)), 1));
        assert!(!PendingActions::<Test>::get()[0].dispatched);

        set_epoch(4);
        run_to_block(4);
        assert!(PendingBondReleases::<Test>::get().is_empty());
        for member in members().into_iter().take(5) {
            assert_eq!(
                <Balances as InspectHold<AccountId32>>::balance_on_hold(
                    &RuntimeHoldReason::Guardian(crate::HoldReason::SeatBond),
                    &member,
                ),
                0
            );
        }
        assert_ok!(Guardian::do_try_state());
    });
}

#[test]
fn uphold_veto_is_delay_only_and_atomic_with_the_epoch_callback() {
    new_test_ext().execute_with(|| {
        reset_review_scheduler();
        set_status(ProposalStatus::Queued, false);
        assert_ok!(Guardian::propose_action(
            RuntimeOrigin::signed(acct(1)),
            GuardianPower::DelayOnce { pid: 42 },
            hash(9)
        ));
        approve_to_dispatch();
        assert_eq!(ReviewReferenda::<Test>::get(0), Some(100));
        assert_eq!(VetoReviewReferenda::<Test>::get(0), Some(101));
        assert_eq!(
            ScheduledReviews::get(),
            vec![
                (0, ReviewVerdict::Ratify, 100),
                (0, ReviewVerdict::UpholdVeto, 101),
            ]
        );
        VetoFails::set(true);
        assert_noop!(
            Guardian::uphold_veto(values_origin(), 0),
            DispatchError::Other("veto callback unavailable")
        );
        assert!(!ReviewDeadlines::<Test>::get()[0].ratified);
        assert!(ReviewFrontingOf::<Test>::contains_key(0));
        assert!(CancelledReviews::get().is_empty());
        assert!(RefundedReviews::get().is_empty());

        VetoFails::set(false);
        assert_ok!(Guardian::uphold_veto(values_origin(), 0));
        assert_eq!(CancelledReviews::get(), vec![100]);
        assert_eq!(RefundedReviews::get(), vec![100, 101]);
        assert!(ReviewDeadlines::<Test>::get()[0].ratified);
        assert!(!ReviewReferenda::<Test>::contains_key(0));
        assert!(!VetoReviewReferenda::<Test>::contains_key(0));
        assert!(!ReviewFrontingOf::<Test>::contains_key(0));
        for member in members().iter().take(5) {
            assert_eq!(seat_hold(member), GUARDIAN_BOND);
        }
        assert_ok!(Guardian::do_try_state());
    });

    new_test_ext().execute_with(|| {
        reset_review_scheduler();
        set_triggers(all_triggers());
        assert_ok!(Guardian::propose_action(
            RuntimeOrigin::signed(acct(1)),
            GuardianPower::SuspendOnGate,
            hash(10)
        ));
        approve_to_dispatch();
        assert_noop!(
            Guardian::uphold_veto(values_origin(), 0),
            Error::<Test>::NotDelayAction
        );
    });
}

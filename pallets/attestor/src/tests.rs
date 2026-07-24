//! `pallet-attestor` unit tests (15 §4.1): per-extrinsic × origin-misuse ×
//! error-path coverage, challenge/quorum boundaries, pallet storage bounds, and
//! try-state assertions.

use crate::mock::*;
use crate::{pallet::*, Error, Event};
use attestor_core::{
    AttestorParams, ChallengeStatus, ATTESTOR_BOND, CHALLENGE_BOND, CHALLENGE_WINDOW_BLOCKS,
    FALSE_EJECTION_THRESHOLD,
};
use frame_support::{assert_noop, assert_ok, traits::fungible::InspectHold};
use sp_runtime::DispatchError;

fn hash(n: u8) -> futarchy_primitives::H256 {
    [n; 32]
}

fn attestor_events() -> Vec<Event<Test>> {
    frame_system::Pallet::<Test>::events()
        .into_iter()
        .filter_map(|record| match record.event {
            RuntimeEvent::Attestor(event) => Some(event),
            _ => None,
        })
        .collect()
}

/// The core `[u8; 32]` view of a mock account, for `AttestorInfo` lookups.
fn core_acct(n: u8) -> futarchy_primitives::AccountId {
    acct(n).into()
}

fn attest_two(pid: u64, artifact_hash: futarchy_primitives::H256) {
    assert_ok!(Attestor::attest(
        RuntimeOrigin::signed(acct(1)),
        pid,
        artifact_hash,
        hash(11),
    ));
    assert_ok!(Attestor::attest(
        RuntimeOrigin::signed(acct(2)),
        pid,
        artifact_hash,
        hash(12),
    ));
}

#[test]
fn genesis_seeds_members_and_try_state_holds() {
    new_test_ext().execute_with(|| {
        let seeded = Members::<Test>::get();
        assert_eq!(seeded.len(), 3);
        assert!(seeded.iter().all(|member| {
            member.bond == ATTESTOR_BOND && member.false_count == 0 && member.active
        }));
        assert!(Attestations::<Test>::get().is_empty());
        assert_eq!(NextAttestationId::<Test>::get(), 0);
        assert_ok!(Attestor::do_try_state());
    });
}

#[test]
fn try_state_rejects_an_ejected_member_marked_active() {
    new_test_ext().execute_with(|| {
        Members::<Test>::mutate(|members| {
            let member = members
                .iter_mut()
                .next()
                .expect("genesis seeds three attestors");
            member.false_count = FALSE_EJECTION_THRESHOLD;
            member.active = true;
        });

        assert!(Attestor::do_try_state().is_err());
    });
}

#[test]
fn set_members_is_values_origin_only_and_validates_membership() {
    // limit-coverage: att.min_members
    new_test_ext_empty().execute_with(|| {
        assert_noop!(
            Attestor::set_members(RuntimeOrigin::none(), members()),
            DispatchError::BadOrigin
        );
        assert_noop!(
            Attestor::set_members(RuntimeOrigin::signed(acct(1)), members()),
            DispatchError::BadOrigin
        );
        assert_noop!(
            Attestor::set_members(RuntimeOrigin::root(), members()),
            DispatchError::BadOrigin
        );
        assert_noop!(
            Attestor::set_members(values_origin(), vec![acct(1), acct(2)]),
            Error::<Test>::TooFewMembers
        );

        let mut duplicate = members();
        duplicate[1] = duplicate[0].clone();
        assert_noop!(
            Attestor::set_members(values_origin(), duplicate),
            Error::<Test>::DuplicateMember
        );

        let oversized = (1..=17u8).map(acct).collect();
        assert_noop!(
            Attestor::set_members(values_origin(), oversized),
            Error::<Test>::TooManyAttestors
        );

        assert_ok!(Attestor::set_members(values_origin(), members()));
        assert_eq!(Members::<Test>::get().len(), 3);
        assert!(matches!(
            attestor_events().last(),
            Some(Event::MembersSet { members }) if members.len() == 3
        ));
        assert_ok!(Attestor::do_try_state());
    });
}

#[test]
fn attest_requires_initialization_signed_active_member_and_unique_assertion() {
    new_test_ext_empty().execute_with(|| {
        assert_noop!(
            Attestor::attest(RuntimeOrigin::signed(acct(1)), 7, hash(1), hash(2)),
            Error::<Test>::NotInitialized
        );
    });

    new_test_ext().execute_with(|| {
        assert_noop!(
            Attestor::attest(RuntimeOrigin::root(), 7, hash(1), hash(2)),
            DispatchError::BadOrigin
        );
        assert_noop!(
            Attestor::attest(RuntimeOrigin::none(), 7, hash(1), hash(2)),
            DispatchError::BadOrigin
        );
        assert_noop!(
            Attestor::attest(RuntimeOrigin::signed(acct(9)), 7, hash(1), hash(2)),
            Error::<Test>::NotMember
        );

        assert_ok!(Attestor::attest(
            RuntimeOrigin::signed(acct(1)),
            7,
            hash(1),
            hash(2),
        ));
        assert_eq!(Attestations::<Test>::get().len(), 1);
        assert_eq!(NextAttestationId::<Test>::get(), 1);
        assert!(matches!(
            attestor_events().last(),
            Some(Event::AttestationSubmitted {
                attestation_id: 0,
                pid: 7,
                artifact_hash,
                attestor,
            }) if *artifact_hash == hash(1) && *attestor == acct(1)
        ));
        assert_noop!(
            Attestor::attest(RuntimeOrigin::signed(acct(1)), 7, hash(1), hash(3)),
            Error::<Test>::DuplicateAttestation
        );
        assert_eq!(Attestations::<Test>::get().len(), 1);
        assert_eq!(NextAttestationId::<Test>::get(), 1);
    });
}

#[test]
fn two_distinct_attestors_form_quorum_only_after_window() {
    new_test_ext().execute_with(|| {
        set_block(10);
        attest_two(9, hash(7));

        assert!(!Attestor::has_quorum(9, hash(7)));
        set_block(u64::from(10 + CHALLENGE_WINDOW_BLOCKS));
        assert!(!Attestor::has_quorum(9, hash(7)));
        set_block(u64::from(11 + CHALLENGE_WINDOW_BLOCKS));
        assert!(Attestor::has_quorum(9, hash(7)));
        assert_eq!(Attestor::attestations_for(9).len(), 2);
        assert!(Attestor::open_challenges().is_empty());
    });
}

#[test]
fn one_or_rejected_attestation_never_forms_quorum() {
    new_test_ext().execute_with(|| {
        set_block(1);
        assert_ok!(Attestor::attest(
            RuntimeOrigin::signed(acct(1)),
            9,
            hash(7),
            hash(11),
        ));
        set_block(u64::from(CHALLENGE_WINDOW_BLOCKS + 2));
        assert!(!Attestor::has_quorum(9, hash(7)));

        set_block(1);
        assert_ok!(Attestor::attest(
            RuntimeOrigin::signed(acct(2)),
            9,
            hash(7),
            hash(12),
        ));
        assert_ok!(Attestor::challenge_attestation(
            RuntimeOrigin::signed(acct(9)),
            0,
            hash(5),
            CHALLENGE_BOND,
        ));
        assert_ok!(Attestor::resolve_challenge(ratify_origin(), 0, false));
        set_block(u64::from(CHALLENGE_WINDOW_BLOCKS + 2));
        assert!(!Attestor::has_quorum(9, hash(7)));
    });
}

#[test]
fn open_challenge_suppresses_quorum_until_attestation_is_upheld() {
    new_test_ext().execute_with(|| {
        set_block(1);
        attest_two(9, hash(7));
        assert_ok!(Attestor::challenge_attestation(
            RuntimeOrigin::signed(acct(9)),
            0,
            hash(5),
            CHALLENGE_BOND,
        ));
        assert_eq!(Attestor::open_challenges().len(), 1);
        assert!(matches!(
            attestor_events().last(),
            Some(Event::AttestationChallenged {
                attestation_id: 0,
                challenger,
                evidence_hash,
            }) if *challenger == acct(9) && *evidence_hash == hash(5)
        ));

        set_block(u64::from(CHALLENGE_WINDOW_BLOCKS + 2));
        assert!(!Attestor::has_quorum(9, hash(7)));
        assert_ok!(Attestor::resolve_challenge(ratify_origin(), 0, true));
        assert!(Attestor::has_quorum(9, hash(7)));
        assert!(Attestor::open_challenges().is_empty());
        assert!(matches!(
            attestor_events().last(),
            Some(Event::ChallengeResolved {
                attestation_id: 0,
                upheld: true,
                loser,
                slashed,
            }) if *loser == acct(9) && *slashed == CHALLENGE_BOND / 2
        ));
        assert!(matches!(
            Attestations::<Test>::get()[0].challenge,
            Some(ChallengeStatus::Upheld)
        ));
    });
}

#[test]
fn challenge_is_signed_only_and_checks_id_bond_window_and_single_open_case() {
    // limit-coverage: att.window
    new_test_ext_empty().execute_with(|| {
        assert_noop!(
            Attestor::challenge_attestation(
                RuntimeOrigin::signed(acct(9)),
                0,
                hash(3),
                CHALLENGE_BOND,
            ),
            Error::<Test>::NotInitialized
        );
    });

    new_test_ext().execute_with(|| {
        set_block(1);
        assert_ok!(Attestor::attest(
            RuntimeOrigin::signed(acct(1)),
            1,
            hash(1),
            hash(2),
        ));
        assert_noop!(
            Attestor::challenge_attestation(RuntimeOrigin::root(), 0, hash(3), CHALLENGE_BOND),
            DispatchError::BadOrigin
        );
        assert_noop!(
            Attestor::challenge_attestation(RuntimeOrigin::none(), 0, hash(3), CHALLENGE_BOND),
            DispatchError::BadOrigin
        );
        assert_noop!(
            Attestor::challenge_attestation(
                RuntimeOrigin::signed(acct(9)),
                99,
                hash(3),
                CHALLENGE_BOND,
            ),
            Error::<Test>::AttestationNotFound
        );
        assert_noop!(
            Attestor::challenge_attestation(
                RuntimeOrigin::signed(acct(9)),
                0,
                hash(3),
                CHALLENGE_BOND - 1,
            ),
            Error::<Test>::ChallengeBondTooSmall
        );

        assert_ok!(Attestor::challenge_attestation(
            RuntimeOrigin::signed(acct(9)),
            0,
            hash(3),
            CHALLENGE_BOND,
        ));
        let challenge_reason: RuntimeHoldReason = crate::HoldReason::ChallengeBond.into();
        assert_eq!(Balances::balance_on_hold(&challenge_reason, &acct(8)), 0);
        assert_noop!(
            Attestor::challenge_attestation(
                RuntimeOrigin::signed(acct(8)),
                0,
                hash(4),
                CHALLENGE_BOND,
            ),
            Error::<Test>::ChallengeAlreadyOpen
        );
        assert_eq!(
            Balances::balance_on_hold(&challenge_reason, &acct(8)),
            0,
            "a failed challenge must not strand its native hold"
        );
    });

    new_test_ext().execute_with(|| {
        set_block(1);
        assert_ok!(Attestor::attest(
            RuntimeOrigin::signed(acct(1)),
            1,
            hash(1),
            hash(2),
        ));
        set_block(u64::from(CHALLENGE_WINDOW_BLOCKS + 2));
        assert_noop!(
            Attestor::challenge_attestation(
                RuntimeOrigin::signed(acct(9)),
                0,
                hash(3),
                CHALLENGE_BOND,
            ),
            Error::<Test>::ChallengeWindowClosed
        );
    });

    // The genesis-default boundary remains inclusive at the stored deadline.
    new_test_ext().execute_with(|| {
        set_block(1);
        assert_ok!(Attestor::attest(
            RuntimeOrigin::signed(acct(1)),
            1,
            hash(1),
            hash(2),
        ));
        set_block(u64::from(CHALLENGE_WINDOW_BLOCKS + 1));
        assert_ok!(Attestor::challenge_attestation(
            RuntimeOrigin::signed(acct(9)),
            0,
            hash(3),
            CHALLENGE_BOND,
        ));
    });

    // Amend the mock provider, then observe the creation-time window move.
    // Existing records retain their snapshotted deadline.
    new_test_ext().execute_with(|| {
        set_block(1);
        assert_ok!(Attestor::attest(
            RuntimeOrigin::signed(acct(1)),
            10,
            hash(1),
            hash(2),
        ));
        AttestorParamsValue::set(AttestorParams {
            challenge_window: 5,
            ..AttestorParams::DEFAULT
        });
        assert_ok!(Attestor::attest(
            RuntimeOrigin::signed(acct(1)),
            11,
            hash(1),
            hash(2),
        ));
        assert_ok!(Attestor::attest(
            RuntimeOrigin::signed(acct(1)),
            12,
            hash(1),
            hash(2),
        ));
        let attestations = Attestations::<Test>::get();
        assert_eq!(
            attestations[0].challenge_deadline,
            CHALLENGE_WINDOW_BLOCKS + 1
        );
        assert_eq!(attestations[1].challenge_deadline, 6);
        assert_eq!(attestations[2].challenge_deadline, 6);

        set_block(6);
        assert_ok!(Attestor::challenge_attestation(
            RuntimeOrigin::signed(acct(9)),
            1,
            hash(3),
            CHALLENGE_BOND,
        ));
        set_block(7);
        assert_noop!(
            Attestor::challenge_attestation(
                RuntimeOrigin::signed(acct(9)),
                2,
                hash(3),
                CHALLENGE_BOND,
            ),
            Error::<Test>::ChallengeWindowClosed
        );
        assert_ok!(Attestor::challenge_attestation(
            RuntimeOrigin::signed(acct(9)),
            0,
            hash(3),
            CHALLENGE_BOND,
        ));
    });
}

#[test]
fn amended_attestor_bond_changes_only_new_member_challenge_floor_and_snapshot() {
    new_test_ext().execute_with(|| {
        assert_ok!(Attestor::attest(
            RuntimeOrigin::signed(acct(1)),
            1,
            hash(1),
            hash(2),
        ));

        let amended_bond = ATTESTOR_BOND.saturating_mul(2);
        AttestorParamsValue::set(AttestorParams {
            bond: amended_bond,
            ..AttestorParams::DEFAULT
        });
        assert_noop!(
            Attestor::challenge_attestation(
                RuntimeOrigin::signed(acct(9)),
                0,
                hash(3),
                CHALLENGE_BOND.saturating_sub(1),
            ),
            Error::<Test>::ChallengeBondTooSmall
        );
        assert_ok!(Attestor::challenge_attestation(
            RuntimeOrigin::signed(acct(9)),
            0,
            hash(3),
            CHALLENGE_BOND,
        ));
        assert_ok!(Attestor::resolve_challenge(ratify_origin(), 0, false));
        let core_account: futarchy_primitives::AccountId = acct(1).into();
        let slashed_bond = Members::<Test>::get()
            .into_iter()
            .find(|member| member.account == core_account)
            .map(|member| member.bond);
        assert_eq!(slashed_bond, Some(CHALLENGE_BOND));
        assert!(matches!(
            attestor_events().last(),
            Some(Event::ChallengeResolved {
                attestation_id: 0,
                upheld: false,
                slashed,
                ..
            }) if *slashed == CHALLENGE_BOND
        ));

        assert_ok!(Attestor::set_members(
            values_origin(),
            vec![acct(4), acct(5), acct(6)],
        ));
        assert!(Members::<Test>::get()
            .iter()
            .all(|member| member.bond == amended_bond));

        assert_ok!(Attestor::attest(
            RuntimeOrigin::signed(acct(4)),
            2,
            hash(4),
            hash(5),
        ));
        assert_noop!(
            Attestor::challenge_attestation(
                RuntimeOrigin::signed(acct(9)),
                1,
                hash(6),
                CHALLENGE_BOND,
            ),
            Error::<Test>::ChallengeBondTooSmall
        );
        assert_ok!(Attestor::challenge_attestation(
            RuntimeOrigin::signed(acct(9)),
            1,
            hash(6),
            ATTESTOR_BOND,
        ));
        assert_ok!(Attestor::resolve_challenge(ratify_origin(), 1, false));
        let post_amend_account: futarchy_primitives::AccountId = acct(4).into();
        let post_amend_bond = Members::<Test>::get()
            .into_iter()
            .find(|member| member.account == post_amend_account)
            .map(|member| member.bond);
        assert_eq!(post_amend_bond, Some(ATTESTOR_BOND));
        assert!(matches!(
            attestor_events().last(),
            Some(Event::ChallengeResolved {
                attestation_id: 1,
                upheld: false,
                slashed,
                ..
            }) if *slashed == ATTESTOR_BOND
        ));
    });
}

#[test]
fn odd_bonds_round_challenge_floor_and_both_forfeits_up() {
    new_test_ext().execute_with(|| {
        AttestorParamsValue::set(AttestorParams {
            bond: 3,
            ..AttestorParams::DEFAULT
        });
        assert_ok!(Attestor::set_members(
            values_origin(),
            vec![acct(4), acct(5), acct(6)],
        ));

        assert_ok!(Attestor::attest(
            RuntimeOrigin::signed(acct(4)),
            1,
            hash(1),
            hash(2),
        ));
        assert_noop!(
            Attestor::challenge_attestation(RuntimeOrigin::signed(acct(9)), 0, hash(3), 1),
            Error::<Test>::ChallengeBondTooSmall
        );
        assert_ok!(Attestor::challenge_attestation(
            RuntimeOrigin::signed(acct(9)),
            0,
            hash(3),
            2,
        ));
        assert_ok!(Attestor::resolve_challenge(ratify_origin(), 0, false));
        let false_attestor: futarchy_primitives::AccountId = acct(4).into();
        assert_eq!(
            Members::<Test>::get()
                .into_iter()
                .find(|member| member.account == false_attestor)
                .map(|member| member.bond),
            Some(1),
        );
        assert!(matches!(
            attestor_events().last(),
            Some(Event::ChallengeResolved { slashed: 2, .. })
        ));

        assert_ok!(Attestor::attest(
            RuntimeOrigin::signed(acct(5)),
            2,
            hash(4),
            hash(5),
        ));
        assert_ok!(Attestor::challenge_attestation(
            RuntimeOrigin::signed(acct(9)),
            1,
            hash(6),
            3,
        ));
        // The caller-supplied challenge bond is not `att.bond`, but the same
        // claimant-adverse 50% ceiling applies to either losing side.
        assert_ok!(Attestor::resolve_challenge(ratify_origin(), 1, true));
        assert!(matches!(
            attestor_events().last(),
            Some(Event::ChallengeResolved {
                attestation_id: 1,
                upheld: true,
                slashed: 2,
                ..
            })
        ));
    });
}

#[test]
fn recall_no_longer_strands_an_open_challenge_on_the_adverse_branch() {
    // SUPERSEDES `false_resolution_rejects_if_the_losing_attestor_is_no_longer_stored`,
    // which pinned the SQ-262 defect as intended behaviour: a recall used to drop
    // the departing attestor's row outright, so `resolve_challenge(id, false)`
    // returned `NotMember` and the challenge stayed `Open` forever. That is the
    // residual SQ-248 recorded — "a for-cause recall shields the attestor from
    // adjudication". Seating now retains a liable member as an inactive row, so
    // the adverse verdict lands and the record reaches a terminal state.
    //
    // The core's `NotMember` guard on that branch survives as a defensive check
    // (G-1); it is no longer reachable through dispatch, and the core try-state
    // invariant "every open challenge has a member row" is what keeps it so.
    new_test_ext().execute_with(|| {
        set_block(1);
        assert_ok!(Attestor::attest(
            RuntimeOrigin::signed(acct(1)),
            1,
            hash(1),
            hash(2),
        ));
        assert_ok!(Attestor::challenge_attestation(
            RuntimeOrigin::signed(acct(9)),
            0,
            hash(3),
            CHALLENGE_BOND,
        ));
        assert_ok!(Attestor::set_members(
            values_origin(),
            vec![acct(4), acct(5), acct(6)],
        ));

        assert_ok!(Attestor::resolve_challenge(ratify_origin(), 0, false));
        assert!(matches!(
            Attestations::<Test>::get()[0].challenge,
            Some(ChallengeStatus::Rejected)
        ));
        assert_ok!(Attestor::do_try_state());
    });
}

#[test]
fn resolve_challenge_is_ratify_origin_only_and_requires_an_open_challenge() {
    new_test_ext_empty().execute_with(|| {
        assert_noop!(
            Attestor::resolve_challenge(ratify_origin(), 0, true),
            Error::<Test>::NotInitialized
        );
    });

    new_test_ext().execute_with(|| {
        assert_ok!(Attestor::attest(
            RuntimeOrigin::signed(acct(1)),
            1,
            hash(1),
            hash(2),
        ));
        assert_noop!(
            Attestor::resolve_challenge(RuntimeOrigin::signed(acct(1)), 0, true),
            DispatchError::BadOrigin
        );
        assert_noop!(
            Attestor::resolve_challenge(RuntimeOrigin::root(), 0, true),
            DispatchError::BadOrigin
        );
        assert_noop!(
            Attestor::resolve_challenge(RuntimeOrigin::none(), 0, true),
            DispatchError::BadOrigin
        );
        // Attestation 0 exists but has no open challenge (precise error).
        assert_noop!(
            Attestor::resolve_challenge(ratify_origin(), 0, true),
            Error::<Test>::NoOpenChallenge
        );
        assert_ok!(Attestor::challenge_attestation(
            RuntimeOrigin::signed(acct(9)),
            0,
            hash(3),
            CHALLENGE_BOND,
        ));
        assert_ok!(Attestor::resolve_challenge(ratify_origin(), 0, true));
        // Re-resolving an already-resolved (Upheld) challenge: the attestation
        // exists but no longer carries an open challenge.
        assert_noop!(
            Attestor::resolve_challenge(ratify_origin(), 0, true),
            Error::<Test>::NoOpenChallenge
        );
    });
}

#[test]
fn two_false_attestations_slash_then_eject_the_attestor() {
    new_test_ext().execute_with(|| {
        for pid in 1..=2u64 {
            assert_ok!(Attestor::attest(
                RuntimeOrigin::signed(acct(1)),
                pid,
                hash(1),
                hash(2),
            ));
            let id = (pid - 1) as u32;
            assert_ok!(Attestor::challenge_attestation(
                RuntimeOrigin::signed(acct(9)),
                id,
                hash(3),
                CHALLENGE_BOND,
            ));
            assert_ok!(Attestor::resolve_challenge(ratify_origin(), id, false));
        }

        let core_account: futarchy_primitives::AccountId = acct(1).into();
        let member = Members::<Test>::get()
            .into_iter()
            .find(|member| member.account == core_account)
            .expect("seeded attestor remains represented after ejection");
        assert_eq!(member.bond, ATTESTOR_BOND / 4);
        assert_eq!(member.false_count, 2);
        assert!(!member.active);
        assert!(attestor_events().iter().any(|event| matches!(
            event,
            Event::AttestorEjected { who } if *who == acct(1)
        )));
        assert_noop!(
            Attestor::attest(RuntimeOrigin::signed(acct(1)), 3, hash(1), hash(2)),
            Error::<Test>::NotMember
        );
        assert_ok!(Attestor::do_try_state());
    });
}

#[test]
fn attestation_id_overflow_is_a_rejected_noop() {
    new_test_ext().execute_with(|| {
        NextAttestationId::<Test>::put(u32::MAX);
        assert_noop!(
            Attestor::attest(RuntimeOrigin::signed(acct(1)), 1, hash(1), hash(2)),
            Error::<Test>::Overflow
        );
        assert!(Attestations::<Test>::get().is_empty());
        assert_eq!(NextAttestationId::<Test>::get(), u32::MAX);
    });
}

#[test]
fn challenge_deadline_overflow_is_a_rejected_noop() {
    new_test_ext().execute_with(|| {
        set_block(u64::from(u32::MAX));
        assert_noop!(
            Attestor::attest(RuntimeOrigin::signed(acct(1)), 1, hash(1), hash(2)),
            Error::<Test>::Overflow
        );
        assert!(Attestations::<Test>::get().is_empty());
        assert_eq!(NextAttestationId::<Test>::get(), 0);
    });
}

#[test]
fn invalid_genesis_members_are_rejected() {
    let invalid_sets = [
        vec![acct(1), acct(2)],
        vec![acct(1), acct(1), acct(2)],
        (1..=17u8).map(acct).collect(),
    ];
    for invalid in invalid_sets {
        let result = std::panic::catch_unwind(move || {
            new_test_ext_with(crate::GenesisConfig::<Test> {
                members: invalid,
                _config: Default::default(),
            });
        });
        assert!(result.is_err());
    }
}

#[test]
fn recall_after_attest_is_valid_and_drops_recalled_attestor_from_quorum() {
    // 06 §7: recall is a first-class guardian-track action and `has_quorum`
    // counts only active members, so a lawful `set_members` recall that leaves a
    // now-non-member's historical attestation in the ledger MUST NOT trip
    // try_state (A10 spec-reviewer major; the over-strict core clause was
    // relaxed). The recalled attestor's attestation simply stops counting.
    new_test_ext().execute_with(|| {
        set_block(1);
        attest_two(9, hash(7));
        // Recall acct(1) through the real values-track `set_members` path.
        assert_ok!(Attestor::set_members(
            values_origin(),
            vec![acct(2), acct(3), acct(4)]
        ));
        assert_ok!(Attestor::do_try_state());
        // Only acct(2) remains an active attestor for (9, hash(7)); quorum needs
        // two distinct active attestors, so it is no longer met.
        set_block(u64::from(CHALLENGE_WINDOW_BLOCKS + 2));
        assert!(!Attestor::has_quorum(9, hash(7)));
    });
}

#[test]
fn recall_retains_a_liable_attestor_so_an_open_challenge_still_resolves() {
    // SQ-262 (06 §7; 09 §2.4). A values-track recall removes governance
    // authority immediately, but it MUST NOT destroy the departing attestor's
    // slash basis while a challenge against one of their attestations is still
    // open: the challenge would otherwise be unresolvable forever (the adverse
    // verdict returns `NotMember`), which permanently suppresses the attestation
    // from quorum and permanently shields the attestor from a `false_count`
    // strike — a for-cause recall would launder the offense.
    new_test_ext().execute_with(|| {
        set_block(1);
        assert_ok!(Attestor::attest(
            RuntimeOrigin::signed(acct(1)),
            1,
            hash(1),
            hash(2),
        ));
        assert_ok!(Attestor::challenge_attestation(
            RuntimeOrigin::signed(acct(9)),
            0,
            hash(3),
            CHALLENGE_BOND,
        ));
        // Recall acct(1); the incoming roster does not include them.
        assert_ok!(Attestor::set_members(
            values_origin(),
            vec![acct(4), acct(5), acct(6)],
        ));

        // Governance authority is gone immediately: the recalled attestor is
        // held in the independent liability vector and cannot attest again.
        let retained = Liabilities::<Test>::get()
            .into_iter()
            .find(|member| member.account == core_acct(1))
            .expect("a liable attestor is retained as the slash basis");
        assert_eq!(retained.bond, ATTESTOR_BOND);
        assert_noop!(
            Attestor::attest(RuntimeOrigin::signed(acct(1)), 2, hash(1), hash(2)),
            Error::<Test>::NotMember
        );
        // The liability row does not count toward the active registry floor.
        assert_eq!(
            Members::<Test>::get()
                .iter()
                .filter(|member| member.active)
                .count(),
            3
        );

        // The adverse verdict now lands against the retained basis.
        assert_ok!(Attestor::resolve_challenge(ratify_origin(), 0, false));
        let after = Liabilities::<Test>::get()
            .into_iter()
            .find(|member| member.account == core_acct(1))
            .expect("still retained until the record is reaped");
        assert_eq!(after.false_count, 1);
        assert_eq!(after.bond, ATTESTOR_BOND - (ATTESTOR_BOND / 2));
        assert!(matches!(
            Attestations::<Test>::get()[0].challenge,
            Some(ChallengeStatus::Rejected)
        ));
        assert_ok!(Attestor::do_try_state());
    });
}

#[test]
fn cause_removal_revokes_records_and_reap_releases_native_hold() {
    new_test_ext().execute_with(|| {
        set_block(1);
        assert_ok!(Attestor::attest(
            RuntimeOrigin::signed(acct(1)),
            41,
            hash(41),
            hash(42),
        ));
        let reason: RuntimeHoldReason = crate::HoldReason::AttestorBond.into();
        assert_eq!(Balances::balance_on_hold(&reason, &acct(1)), ATTESTOR_BOND);

        assert_ok!(Attestor::remove_for_cause(
            values_origin(),
            acct(1),
            hash(99),
        ));
        assert!(!Members::<Test>::get()
            .iter()
            .any(|member| member.account == core_acct(1)));
        assert!(Liabilities::<Test>::get()
            .iter()
            .any(|liability| liability.account == core_acct(1)));
        assert!(Revocations::<Test>::get()
            .iter()
            .any(|revocation| revocation.attestation_id == 0));
        assert!(attestor_events().iter().any(|event| matches!(
            event,
            Event::AttestorRemovedForCause { who, .. } if *who == acct(1)
        )));

        ProposalTerminal::set(true);
        set_block(u64::from(CHALLENGE_WINDOW_BLOCKS + 2));
        assert_ok!(Attestor::reap_attestation(
            RuntimeOrigin::signed(acct(9)),
            0
        ));
        assert!(Attestations::<Test>::get().is_empty());
        assert!(Liabilities::<Test>::get().is_empty());
        assert!(Revocations::<Test>::get().is_empty());
        assert_eq!(Balances::balance_on_hold(&reason, &acct(1)), 0);
        assert_ok!(Attestor::do_try_state());
    });
}

#[test]
fn record_quorum_survives_rotation_but_cause_revocation_fails_closed() {
    new_test_ext().execute_with(|| {
        set_block(1);
        attest_two(42, hash(42));
        set_block(u64::from(CHALLENGE_WINDOW_BLOCKS + 2));
        assert_ok!(Attestor::set_members(
            values_origin(),
            vec![acct(3), acct(4), acct(5)],
        ));
        assert!(!Attestor::has_quorum(42, hash(42)));
        assert!(Attestor::has_record_quorum(42, hash(42)));
    });

    new_test_ext().execute_with(|| {
        set_block(1);
        attest_two(42, hash(42));
        // A cause-aware departure invalidates a committed record before the
        // routine rotation, and the record-only quorum then fails closed.
        assert_ok!(Attestor::remove_for_cause(
            values_origin(),
            acct(1),
            hash(88)
        ));
        assert!(!Attestor::has_record_quorum(42, hash(42)));
    });
}

#[test]
fn recall_preserves_a_continuing_members_strike_count() {
    // SQ-262 / 06 §7 ejection discipline. Re-electing a roster must not wipe a
    // continuing member's `false_count`, or one no-op `set_members` resets every
    // strike and the second-false ejection threshold is unreachable. 13 §1's
    // `att.bond` row (SQ-248) says the *bond* re-binds at seating; nothing
    // re-binds the strike count.
    new_test_ext().execute_with(|| {
        set_block(1);
        assert_ok!(Attestor::attest(
            RuntimeOrigin::signed(acct(1)),
            1,
            hash(1),
            hash(2),
        ));
        assert_ok!(Attestor::challenge_attestation(
            RuntimeOrigin::signed(acct(9)),
            0,
            hash(3),
            CHALLENGE_BOND,
        ));
        assert_ok!(Attestor::resolve_challenge(ratify_origin(), 0, false));
        assert_eq!(
            Members::<Test>::get()
                .iter()
                .find(|member| member.account == core_acct(1))
                .map(|member| member.false_count),
            Some(1)
        );

        // Re-elect the same roster: the strike survives, the bond re-binds.
        assert_ok!(Attestor::set_members(
            values_origin(),
            vec![acct(1), acct(2), acct(3)],
        ));
        let member = Members::<Test>::get()
            .into_iter()
            .find(|member| member.account == core_acct(1))
            .expect("continuing member");
        assert_eq!(member.false_count, 1);
        assert_eq!(member.bond, ATTESTOR_BOND);
        assert!(member.active);

        // A second adjudicated-false attestation therefore still ejects.
        assert_ok!(Attestor::attest(
            RuntimeOrigin::signed(acct(1)),
            2,
            hash(1),
            hash(2),
        ));
        assert_ok!(Attestor::challenge_attestation(
            RuntimeOrigin::signed(acct(9)),
            1,
            hash(3),
            CHALLENGE_BOND,
        ));
        assert_ok!(Attestor::resolve_challenge(ratify_origin(), 1, false));
        assert!(attestor_events()
            .iter()
            .any(|event| matches!(event, Event::AttestorEjected { .. })));
        assert_ok!(Attestor::do_try_state());
    });
}

#[test]
fn recall_drops_an_attestor_carrying_no_outstanding_liability() {
    // The retention of SQ-262 is bounded: once every window has closed and no
    // challenge is open, the departing row carries no slash basis and is
    // removed, so recalls cannot accumulate dead rows against the 16-seat bound.
    new_test_ext().execute_with(|| {
        set_block(1);
        assert_ok!(Attestor::attest(
            RuntimeOrigin::signed(acct(1)),
            1,
            hash(1),
            hash(2),
        ));
        // Window closes with no challenge: the record is unchallengeable.
        set_block(u64::from(CHALLENGE_WINDOW_BLOCKS + 2));
        assert_ok!(Attestor::set_members(
            values_origin(),
            vec![acct(4), acct(5), acct(6)],
        ));
        assert_eq!(Members::<Test>::get().len(), 3);
        assert!(!Members::<Test>::get()
            .iter()
            .any(|member| member.account == core_acct(1)));
        assert_ok!(Attestor::do_try_state());
    });
}

#[test]
fn resolve_without_open_challenge_is_a_precise_error() {
    new_test_ext().execute_with(|| {
        assert_ok!(Attestor::attest(
            RuntimeOrigin::signed(acct(1)),
            1,
            hash(1),
            hash(2),
        ));
        // Attestation 0 exists but carries no open challenge.
        assert_noop!(
            Attestor::resolve_challenge(ratify_origin(), 0, true),
            Error::<Test>::NoOpenChallenge
        );
        // A non-existent id is still a distinct AttestationNotFound.
        assert_noop!(
            Attestor::resolve_challenge(ratify_origin(), 99, true),
            Error::<Test>::AttestationNotFound
        );
    });
}

#[test]
fn attestation_storage_cap_rejects_without_mutating_existing_rows() {
    new_test_ext().execute_with(|| {
        for pid in 0..256u64 {
            assert_ok!(Attestor::attest(
                RuntimeOrigin::signed(acct(1)),
                pid,
                hash(1),
                hash(2),
            ));
        }
        assert_eq!(Attestations::<Test>::get().len(), 256);
        assert_eq!(NextAttestationId::<Test>::get(), 256);
        assert_noop!(
            Attestor::attest(RuntimeOrigin::signed(acct(1)), 256, hash(1), hash(2)),
            Error::<Test>::TooManyAttestations
        );
        assert_eq!(Attestations::<Test>::get().len(), 256);
        assert_eq!(NextAttestationId::<Test>::get(), 256);
    });
}

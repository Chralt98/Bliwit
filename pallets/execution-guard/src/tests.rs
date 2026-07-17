use super::*;
use crate::mock::ExecutionGuard as GuardPallet;
use crate::mock::*;
use crate::pallet::PendingUpgrade as PendingUpgradeStorage;
use frame_support::{
    assert_noop, assert_ok,
    dispatch::{DispatchErrorWithPostInfo, GetDispatchInfo, Pays, PostDispatchInfo},
    traits::Hooks,
    weights::Weight,
};
use futarchy_primitives::{keeper::CrankClass, DispatchOutcomeCode, RejectReason};
use parity_scale_codec::Encode;
use sp_runtime::{traits::Dispatchable, DispatchError};

macro_rules! assert_noop {
    (GuardPallet::execute($($args:tt)*), $error:expr $(,)?) => {{
        frame_support::assert_noop!(
            GuardPallet::execute($($args)*),
            DispatchErrorWithPostInfo {
                post_info: PostDispatchInfo {
                    actual_weight: Some(<() as crate::WeightInfo>::execute(MAX_CALLS_BOUND)),
                    pays_fee: Pays::Yes,
                },
                error: ($error).into(),
            }
        );
    }};
    ($call:expr, $error:expr $(,)?) => {{
        frame_support::assert_noop!($call, $error);
    }};
}

fn ratify_origin() -> RuntimeOrigin {
    RuntimeOrigin::from(pallet_origins::Origin::ConstitutionalValues)
}

fn bounded_code(code: &[u8]) -> RuntimeCode<Test> {
    RuntimeCode::<Test>::try_from(code.to_vec()).expect("test code is bounded")
}

fn setup_param(pid: ProposalId, value: u32) {
    assert_ok!(enqueue_calls(
        pid,
        futarchy_primitives::ProposalClass::Param,
        vec![param_call(value)],
        vec![CallDomain::Param],
    ));
    run_to_maturity(pid);
}

fn model_queue_item(pid: ProposalId, payload_hash: H256, payload_len: u32) -> QueuedExecution {
    queued_item(
        pid,
        futarchy_primitives::ProposalClass::Param,
        payload_hash,
        payload_len,
        vec![CallDomain::Param],
    )
    .into()
}

fn setup_upgrade(pid: ProposalId, code: &[u8], referendum: u32) {
    let code_hash = hash(code);
    AttestationArtifact::set(Some((7, code_hash)));
    assert_ok!(enqueue_code(pid, authorize_call(code_hash), 7, referendum));
    assert_ok!(GuardPallet::ratify(ratify_origin(), pid, referendum));
    run_to_maturity(pid);
}

fn last_guard_event() -> Option<Event<Test>> {
    System::events()
        .into_iter()
        .rev()
        .find_map(|record| match record.event {
            RuntimeEvent::ExecutionGuard(event) => Some(event),
            _ => None,
        })
}

#[test]
fn execute_happy_path_dispatches_with_class_origin_and_records_terminal_state() {
    new_test_ext().execute_with(|| {
        setup_param(1, 41);
        assert_ok!(GuardPallet::execute(RuntimeOrigin::signed(keeper()), 1));

        assert_eq!(pallet_test_dispatch::Value::<Test>::get(), 41);
        assert!(!Queue::<Test>::contains_key(1));
        assert!(HeldResources::<Test>::get().is_empty());
        assert_eq!(Unpinned::get().len(), 1);
        assert_eq!(epoch_calls(), vec![EpochCall::Executed(1)]);
        let records = ExecutionRecords::<Test>::get();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].pid, 1);
        assert_eq!(records[0].result, DispatchOutcomeCode::Ok);
        assert!(matches!(
            last_guard_event(),
            Some(Event::Executed { pid: 1, .. })
        ));
        assert_ok!(GuardPallet::do_try_state());
    });
}

#[test]
fn execute_post_info_includes_inner_actual_weight_and_never_exceeds_precharge() {
    new_test_ext().execute_with(|| {
        assert_ok!(enqueue_calls(
            1,
            ProposalClass::Param,
            vec![weighted_call(41)],
            vec![CallDomain::Param],
        ));
        run_to_maturity(1);

        let post = GuardPallet::execute(RuntimeOrigin::signed(keeper()), 1)
            .expect("weighted execution succeeds");
        let expected =
            <() as crate::WeightInfo>::execute(1).saturating_add(Weight::from_parts(400, 40));
        assert_eq!(post.actual_weight, Some(expected));
        assert!(expected.all_lte(GuardPallet::execute_precharge()));
    });
}

#[test]
fn execute_falls_back_to_declared_inner_weight_when_post_info_is_absent() {
    new_test_ext().execute_with(|| {
        let call = param_call(7);
        let declared = call.get_dispatch_info().total_weight();
        assert_ok!(enqueue_calls(
            1,
            ProposalClass::Param,
            vec![call],
            vec![CallDomain::Param],
        ));
        run_to_maturity(1);

        let post = GuardPallet::execute(RuntimeOrigin::signed(keeper()), 1)
            .expect("fallback-weight execution succeeds");
        let expected = <() as crate::WeightInfo>::execute(1).saturating_add(declared);
        assert_eq!(post.actual_weight, Some(expected));
        assert!(expected.all_lte(GuardPallet::execute_precharge()));
    });
}

#[test]
fn execute_reject_refunds_the_inner_ceiling_to_checks_only() {
    new_test_ext().execute_with(|| {
        setup_param(1, 9);
        GuardianHeld::set(vec![1]);

        let error = GuardPallet::execute(RuntimeOrigin::signed(keeper()), 1)
            .expect_err("guardian hold rejects before dispatch");
        assert_eq!(error.error, Error::<Test>::GuardianHold.into());
        let checks_only = <() as crate::WeightInfo>::execute(MAX_CALLS_BOUND);
        assert_eq!(error.post_info.actual_weight, Some(checks_only));
        assert!(checks_only.all_lte(GuardPallet::execute_precharge()));
        assert!(Queue::<Test>::contains_key(1));
        assert_eq!(pallet_test_dispatch::Value::<Test>::get(), 0);
    });
}

#[test]
fn post_dispatch_failure_still_charges_consumed_inner_weight() {
    new_test_ext().execute_with(|| {
        let code = b"post-dispatch-weight";
        let code_hash = hash(code);
        let declared = authorize_call(code_hash).get_dispatch_info().total_weight();
        setup_upgrade(1, code, 9);
        ReleaseRefuses::set(true);

        let error = GuardPallet::execute(RuntimeOrigin::signed(keeper()), 1)
            .expect_err("release-channel callback rejects after inner dispatch");
        assert_eq!(error.error, DispatchError::Other("release channel refused"));
        let expected = <() as crate::WeightInfo>::execute(1).saturating_add(declared);
        assert_eq!(error.post_info.actual_weight, Some(expected));
        assert!(expected.all_lte(GuardPallet::execute_precharge()));
        assert!(Queue::<Test>::contains_key(1));
    });
}

#[test]
fn keeper_rebate_is_exactly_once_for_terminal_execute_and_zero_for_error() {
    new_test_ext().execute_with(|| {
        setup_param(1, 41);
        RecordKeeperRebates::set(true);
        assert_ok!(GuardPallet::execute(RuntimeOrigin::signed(keeper()), 1));
        assert_eq!(KeeperRebates::get(), vec![(keeper(), CrankClass::General)]);

        // A terminal item is no longer executable and cannot earn twice.
        assert_noop!(
            GuardPallet::execute(RuntimeOrigin::signed(keeper()), 1),
            Error::<Test>::NotFound
        );
        assert_eq!(KeeperRebates::get(), vec![(keeper(), CrankClass::General)]);
    });
}

#[test]
fn prequeue_reap_never_rebates_the_epoch_authority_protocol_account() {
    new_test_ext().execute_with(|| {
        commit_payload(1, [1; 32]);
        assert_ok!(GuardPallet::ratify(ratify_origin(), 1, 88));
        RecordKeeperRebates::set(true);

        assert_ok!(GuardPallet::reap_prequeue_ratification(
            RuntimeOrigin::signed(epoch_account()),
            1,
        ));
        assert!(KeeperRebates::get().is_empty());

        // The cleanup API is idempotent; its successful no-op earns nothing.
        assert_ok!(GuardPallet::reap_prequeue_ratification(
            RuntimeOrigin::signed(epoch_account()),
            1,
        ));
        assert!(KeeperRebates::get().is_empty());
    });
}

#[test]
fn every_callable_surface_rejects_origin_misuse() {
    new_test_ext().execute_with(|| {
        let (payload_hash, payload_len) = put_preimage(&[param_call(1)]);
        commit_payload(1, payload_hash);
        let item = queued_item(
            1,
            futarchy_primitives::ProposalClass::Param,
            payload_hash,
            payload_len,
            vec![CallDomain::Param],
        );
        assert_noop!(
            GuardPallet::enqueue(RuntimeOrigin::signed(keeper()), item, false),
            DispatchError::BadOrigin
        );

        setup_param(1, 1);
        for origin in [RuntimeOrigin::root(), RuntimeOrigin::none()] {
            assert_noop!(GuardPallet::execute(origin, 1), DispatchError::BadOrigin);
        }
        for origin in [RuntimeOrigin::root(), RuntimeOrigin::none()] {
            assert_noop!(
                GuardPallet::apply_authorized_upgrade(origin, bounded_code(b"x")),
                DispatchError::BadOrigin
            );
        }
        for origin in [RuntimeOrigin::root(), RuntimeOrigin::none()] {
            assert_noop!(
                GuardPallet::expire_failed_execution(origin, 1),
                DispatchError::BadOrigin
            );
        }
        for origin in [
            RuntimeOrigin::signed(keeper()),
            RuntimeOrigin::root(),
            RuntimeOrigin::none(),
        ] {
            assert_noop!(GuardPallet::ratify(origin, 1, 9), DispatchError::BadOrigin);
        }
        for origin in [RuntimeOrigin::root(), RuntimeOrigin::none()] {
            assert_noop!(
                GuardPallet::reject_stale(origin, 1),
                DispatchError::BadOrigin
            );
        }
    });
}

#[test]
fn enqueue_validates_frozen_preconditions_and_queue_bound() {
    new_test_ext().execute_with(|| {
        let (payload_hash, payload_len) = put_preimage(&[param_call(1)]);
        commit_payload(1, payload_hash);

        let mut bad = queued_item(
            1,
            futarchy_primitives::ProposalClass::Param,
            payload_hash,
            payload_len,
            vec![CallDomain::Param],
        );
        bad.payload_len = MAX_PAYLOAD_BYTES.saturating_add(1);
        assert_noop!(
            GuardPallet::enqueue(RuntimeOrigin::signed(epoch_account()), bad, false),
            Error::<Test>::PayloadTooLarge
        );

        let mut bad = queued_item(
            1,
            futarchy_primitives::ProposalClass::Param,
            payload_hash,
            payload_len,
            vec![CallDomain::Param],
        );
        bad.payload_len = bad.payload_len.saturating_add(1);
        assert_noop!(
            GuardPallet::enqueue(RuntimeOrigin::signed(epoch_account()), bad, false),
            Error::<Test>::BadPreimage
        );

        let mut bad = queued_item(
            1,
            futarchy_primitives::ProposalClass::Param,
            payload_hash,
            payload_len,
            vec![CallDomain::Param],
        );
        bad.version_constraint = spec(99);
        assert_noop!(
            GuardPallet::enqueue(RuntimeOrigin::signed(epoch_account()), bad, false),
            Error::<Test>::StaleQueue
        );

        let mut bad = queued_item(
            1,
            futarchy_primitives::ProposalClass::Param,
            payload_hash,
            payload_len,
            vec![CallDomain::Param],
        );
        bad.maturity = bad.maturity.saturating_add(1);
        assert_noop!(
            GuardPallet::enqueue(RuntimeOrigin::signed(epoch_account()), bad, false),
            Error::<Test>::NotMature
        );

        let bad = queued_item(
            1,
            futarchy_primitives::ProposalClass::Param,
            payload_hash,
            payload_len,
            vec![CallDomain::Treasury],
        );
        assert_noop!(
            GuardPallet::enqueue(RuntimeOrigin::signed(epoch_account()), bad, false),
            Error::<Test>::CapabilityDenied
        );

        for pid in 1..=MAX_QUEUE as ProposalId {
            assert_ok!(enqueue_calls(
                pid,
                futarchy_primitives::ProposalClass::Param,
                vec![param_call(pid as u32)],
                vec![CallDomain::Param],
            ));
        }
        assert_noop!(
            enqueue_calls(
                1_000,
                futarchy_primitives::ProposalClass::Param,
                vec![param_call(1_000)],
                vec![CallDomain::Param],
            ),
            Error::<Test>::QueueFull
        );
        assert_eq!(Queue::<Test>::count(), MAX_QUEUE_BOUND);
        assert_ok!(GuardPallet::do_try_state());
    });
}

#[test]
fn core_seed_rejects_per_entry_domain_and_lock_overflow() {
    new_test_ext().execute_with(|| {
        let (payload_hash, payload_len) = put_preimage(&[param_call(1)]);
        let stored = queued_item(
            1,
            futarchy_primitives::ProposalClass::Param,
            payload_hash,
            payload_len,
            vec![CallDomain::Param],
        );

        let mut state = execution_guard_core::ExecutionGuard::new(spec(1));
        let mut too_many_locks: QueuedExecution = stored.clone().into();
        too_many_locks.meters_declared = vec![[1; 8]; MAX_RESOURCE_LOCKS + 1];
        state.queue.push(too_many_locks);
        assert_noop!(GuardPallet::seed_core(state), Error::<Test>::TooManyLocks);

        let mut state = execution_guard_core::ExecutionGuard::new(spec(1));
        let mut too_many_domains: QueuedExecution = stored.into();
        too_many_domains.declared_domains = vec![CallDomain::Public; MAX_DECLARED_DOMAINS + 1];
        state.queue.push(too_many_domains);
        assert_noop!(GuardPallet::seed_core(state), Error::<Test>::TooManyDomains);
    });
}

#[test]
fn ordered_check_1_rejects_cancelled_immature_and_expired_entries() {
    new_test_ext().execute_with(|| {
        setup_param(1, 1);
        Queue::<Test>::mutate(1, |queued| {
            queued.as_mut().expect("queued").cancelled = true;
        });
        assert_noop!(
            GuardPallet::execute(RuntimeOrigin::signed(keeper()), 1),
            Error::<Test>::Cancelled
        );
    });
    new_test_ext().execute_with(|| {
        assert_ok!(enqueue_calls(
            1,
            futarchy_primitives::ProposalClass::Param,
            vec![param_call(1)],
            vec![CallDomain::Param],
        ));
        assert_noop!(
            GuardPallet::execute(RuntimeOrigin::signed(keeper()), 1),
            Error::<Test>::NotMature
        );
    });
    new_test_ext().execute_with(|| {
        setup_param(1, 1);
        let grace_end = Queue::<Test>::get(1).expect("queued").grace_end;
        System::set_block_number(grace_end.saturating_add(1).into());
        assert_noop!(
            GuardPallet::execute(RuntimeOrigin::signed(keeper()), 1),
            Error::<Test>::GraceExpired
        );
    });
}

#[test]
fn ordered_check_2_rederives_hash_and_rejects_preimage_swap() {
    new_test_ext().execute_with(|| {
        setup_param(1, 1);
        let committed = Queue::<Test>::get(1).expect("queued").payload_hash;
        let swapped = vec![param_call(99)].encode();
        PreimageData::set(vec![(committed, swapped)]);
        assert_noop!(
            GuardPallet::execute(RuntimeOrigin::signed(keeper()), 1),
            Error::<Test>::BadPreimage
        );
        assert_eq!(pallet_test_dispatch::Value::<Test>::get(), 0);
    });
    new_test_ext().execute_with(|| {
        setup_param(1, 1);
        PreimageFetchRequests::set(Vec::new());
        Queue::<Test>::mutate(1, |queued| {
            queued.as_mut().expect("queued").payload_len = 1;
        });
        assert_noop!(
            GuardPallet::execute(RuntimeOrigin::signed(keeper()), 1),
            Error::<Test>::BadPreimage
        );
        assert!(PreimageFetchRequests::get().is_empty());
    });
    new_test_ext().execute_with(|| {
        setup_param(1, 1);
        PreimageData::set(Vec::new());
        assert_noop!(
            GuardPallet::execute(RuntimeOrigin::signed(keeper()), 1),
            Error::<Test>::BadPreimage
        );
    });
}

#[test]
fn ordered_checks_3_and_4_reject_stale_version_and_bad_ratification_binding() {
    new_test_ext().execute_with(|| {
        setup_param(1, 1);
        CurrentSpecName::<Test>::put(spec(2));
        assert_noop!(
            GuardPallet::execute(RuntimeOrigin::signed(keeper()), 1),
            Error::<Test>::StaleQueue
        );
    });
    new_test_ext().execute_with(|| {
        let code = b"ratification-bound-runtime";
        let code_hash = hash(code);
        AttestationArtifact::set(Some((7, code_hash)));
        assert_ok!(enqueue_code(1, authorize_call(code_hash), 7, 42));
        run_to_maturity(1);
        assert_noop!(
            GuardPallet::execute(RuntimeOrigin::signed(keeper()), 1),
            Error::<Test>::NotRatified
        );
        assert_ok!(GuardPallet::ratify(ratify_origin(), 1, 42));
        Ratifications::<Test>::mutate(1, |record| {
            record.as_mut().expect("ratification").payload_hash = [99; 32];
        });
        assert_noop!(
            GuardPallet::execute(RuntimeOrigin::signed(keeper()), 1),
            Error::<Test>::NotRatified
        );
    });
}

#[test]
fn ordered_check_5_fails_closed_for_forged_underquorum_and_challenged_attestations() {
    // A forged queue row has no queue-time-frozen attestation binding.
    new_test_ext().execute_with(|| {
        let code = b"attested-runtime";
        setup_upgrade(1, code, 7);
        AttestationBindings::<Test>::remove(1);
        assert_noop!(
            GuardPallet::execute(RuntimeOrigin::signed(keeper()), 1),
            Error::<Test>::AttestationMissing
        );
    });
    // Under-quorum attestations are rejected at queue admission.
    new_test_ext().execute_with(|| {
        let code = b"attested-runtime";
        let code_hash = hash(code);
        AttestationArtifact::set(Some((7, code_hash)));
        AttestationQuorum::set(false);
        assert_noop!(
            enqueue_code(1, authorize_call(code_hash), 7, 7),
            Error::<Test>::AttestationMissing
        );
    });
    // A later challenge/revocation makes the frozen record unavailable.
    new_test_ext().execute_with(|| {
        let code = b"attested-runtime";
        setup_upgrade(1, code, 7);
        AttestationPresent::set(false);
        assert_noop!(
            GuardPallet::execute(RuntimeOrigin::signed(keeper()), 1),
            Error::<Test>::AttestationMissing
        );
        assert!(PendingUpgradeStorage::<Test>::get().is_none());
        assert!(release_log().is_empty());
    });
}

#[test]
fn ordered_checks_6_to_10_reject_capability_meter_lock_guardian_and_freezes() {
    new_test_ext().execute_with(|| {
        setup_param(1, 1);
        Queue::<Test>::mutate(1, |queued| {
            queued.as_mut().expect("queued").class = futarchy_primitives::ProposalClass::Treasury;
        });
        assert_noop!(
            GuardPallet::execute(RuntimeOrigin::signed(keeper()), 1),
            Error::<Test>::CapabilityDenied
        );
    });
    new_test_ext().execute_with(|| {
        setup_param(1, 1);
        BlockedMeters::<Test>::put(BoundedVec::try_from(vec![[1; 8]]).expect("one blocked meter"));
        assert_noop!(
            GuardPallet::execute(RuntimeOrigin::signed(keeper()), 1),
            Error::<Test>::MetersBlocked
        );
    });
    new_test_ext().execute_with(|| {
        setup_param(1, 1);
        HeldResources::<Test>::kill();
        assert_noop!(
            GuardPallet::execute(RuntimeOrigin::signed(keeper()), 1),
            Error::<Test>::ResourceLockMissing
        );
    });
    new_test_ext().execute_with(|| {
        setup_param(1, 1);
        GuardianHeld::set(vec![1]);
        assert_noop!(
            GuardPallet::execute(RuntimeOrigin::signed(keeper()), 1),
            Error::<Test>::GuardianHold
        );
    });
    for freeze in 0..4 {
        new_test_ext().execute_with(|| {
            setup_param(1, 1);
            match freeze {
                0 => HardGateBreach::<Test>::put(true),
                1 => DeadManFreeze::<Test>::put(true),
                2 => MigrationHalt::<Test>::put(true),
                _ => LedgerFrozen::set(true),
            }
            assert_noop!(
                GuardPallet::execute(RuntimeOrigin::signed(keeper()), 1),
                Error::<Test>::FreezeActive
            );
        });
    }
}

#[test]
fn ordered_check_11_enforces_payload_call_domain_and_safety_bounds() {
    new_test_ext().execute_with(|| {
        let calls = (0..=MAX_CALLS)
            .map(|value| param_call(value as u32))
            .collect::<Vec<_>>();
        assert_ok!(enqueue_calls(
            1,
            futarchy_primitives::ProposalClass::Param,
            calls,
            vec![CallDomain::Param],
        ));
        run_to_maturity(1);
        assert_noop!(
            GuardPallet::execute(RuntimeOrigin::signed(keeper()), 1),
            Error::<Test>::TooManyCalls
        );
    });
    new_test_ext().execute_with(|| {
        setup_param(1, 1);
        let oversized = vec![0; MAX_PAYLOAD_BYTES as usize + 1];
        let oversized_hash = hash(&oversized);
        PreimageData::set(vec![(oversized_hash, oversized)]);
        commit_payload(1, oversized_hash);
        Queue::<Test>::mutate(1, |queued| {
            let queued = queued.as_mut().expect("queued");
            queued.payload_hash = oversized_hash;
            queued.payload_len = MAX_PAYLOAD_BYTES.saturating_add(1);
        });
        assert_noop!(
            GuardPallet::execute(RuntimeOrigin::signed(keeper()), 1),
            Error::<Test>::PayloadTooLarge
        );
    });
    new_test_ext().execute_with(|| {
        setup_param(1, 1);
        Queue::<Test>::mutate(1, |queued| {
            queued.as_mut().expect("queued").declared_domains =
                BoundedVec::try_from(vec![CallDomain::Public]).expect("one domain");
        });
        assert_noop!(
            GuardPallet::execute(RuntimeOrigin::signed(keeper()), 1),
            Error::<Test>::BadDomainDeclaration
        );
    });
    new_test_ext().execute_with(|| {
        assert_ok!(enqueue_calls(
            1,
            futarchy_primitives::ProposalClass::Param,
            vec![wrapped_call(WrapperKind::Proxy, CallDomain::Param)],
            vec![CallDomain::Param],
        ));
        run_to_maturity(1);
        assert_noop!(
            GuardPallet::execute(RuntimeOrigin::signed(keeper()), 1),
            Error::<Test>::SafetyFilter
        );
    });
}

#[test]
fn i11_domain_escape_and_every_closed_wrapper_variant_are_fail_closed() {
    new_test_ext().execute_with(|| {
        assert_ok!(enqueue_calls(
            1,
            futarchy_primitives::ProposalClass::Param,
            vec![wrapped_call(WrapperKind::Batch, CallDomain::Public)],
            vec![CallDomain::Public],
        ));
        Queue::<Test>::mutate(1, |queued| {
            queued.as_mut().expect("queued").declared_domains =
                BoundedVec::try_from(vec![CallDomain::Param]).expect("one domain");
        });
        run_to_maturity(1);
        assert_noop!(
            GuardPallet::execute(RuntimeOrigin::signed(keeper()), 1),
            Error::<Test>::BadDomainDeclaration
        );
    });

    let wrappers = [
        WrapperKind::Batch,
        WrapperKind::BatchAll,
        WrapperKind::ForceBatch,
        WrapperKind::DispatchAs,
        WrapperKind::AsDerivative,
        WrapperKind::WithWeight,
        WrapperKind::Proxy,
        WrapperKind::ProxyAnnounced,
        WrapperKind::AsMulti,
        WrapperKind::AsMultiThreshold1,
        WrapperKind::Sudo,
        WrapperKind::Sudo,
    ];
    for (index, kind) in wrappers.into_iter().enumerate() {
        new_test_ext().execute_with(|| {
            let call = wrapped_call(kind, CallDomain::Treasury);
            let (payload_hash, payload_len) = put_preimage(&[call]);
            commit_payload(index as ProposalId + 1, payload_hash);
            let mut item = queued_item(
                index as ProposalId + 1,
                futarchy_primitives::ProposalClass::Param,
                payload_hash,
                payload_len,
                vec![CallDomain::Param],
            );
            // Simulate a hostile decoder projection after the queue commitment.
            // The actual leaf is re-derived and cannot escape the class envelope.
            item.declared_domains =
                BoundedVec::try_from(vec![CallDomain::Param]).expect("one domain");
            assert_ok!(GuardPallet::enqueue(
                RuntimeOrigin::signed(epoch_account()),
                item,
                false
            ));
            run_to_maturity(index as ProposalId + 1);
            assert_noop!(
                GuardPallet::execute(RuntimeOrigin::signed(keeper()), index as ProposalId + 1),
                Error::<Test>::BadDomainDeclaration
            );
        });
    }
}

#[test]
fn closed_wrapper_policy_reaches_safety_filter_for_matching_privileged_domains() {
    let denied = [
        WrapperKind::DispatchAs,
        WrapperKind::AsDerivative,
        WrapperKind::Proxy,
        WrapperKind::ProxyAnnounced,
        WrapperKind::AsMulti,
        WrapperKind::AsMultiThreshold1,
    ];
    for (index, kind) in denied.into_iter().enumerate() {
        new_test_ext().execute_with(|| {
            assert_ok!(enqueue_calls(
                index as ProposalId + 1,
                futarchy_primitives::ProposalClass::Treasury,
                vec![wrapped_call(kind, CallDomain::Treasury)],
                vec![CallDomain::Treasury],
            ));
            run_to_maturity(index as ProposalId + 1);
            assert_noop!(
                GuardPallet::execute(RuntimeOrigin::signed(keeper()), index as ProposalId + 1),
                Error::<Test>::SafetyFilter
            );
        });
    }

    // Atomic/non-elevating wrappers stay usable for a leaf already authorized
    // by the class origin; best-effort Batch/ForceBatch are tested separately.
    for kind in [WrapperKind::BatchAll, WrapperKind::WithWeight] {
        new_test_ext().execute_with(|| {
            assert_ok!(enqueue_calls(
                1,
                futarchy_primitives::ProposalClass::Treasury,
                vec![wrapped_call(kind, CallDomain::Treasury)],
                vec![CallDomain::Treasury],
            ));
            run_to_maturity(1);
            assert_ok!(GuardPallet::execute(RuntimeOrigin::signed(keeper()), 1));
        });
    }
}

#[test]
fn d1_best_effort_batch_wrappers_are_rejected_by_the_guard_model() {
    for kind in [WrapperKind::Batch, WrapperKind::ForceBatch] {
        new_test_ext().execute_with(|| {
            assert_ok!(enqueue_calls(
                1,
                futarchy_primitives::ProposalClass::Param,
                vec![wrapped_call(kind, CallDomain::Param)],
                vec![CallDomain::Param],
            ));
            run_to_maturity(1);
            assert_noop!(
                GuardPallet::execute(RuntimeOrigin::signed(keeper()), 1),
                Error::<Test>::SafetyFilter
            );
            assert!(Queue::<Test>::contains_key(1));
        });
    }
    new_test_ext().execute_with(|| {
        assert_ok!(enqueue_calls(
            1,
            futarchy_primitives::ProposalClass::Param,
            vec![wrapped_call(WrapperKind::BatchAll, CallDomain::Param)],
            vec![CallDomain::Param],
        ));
        run_to_maturity(1);
        assert_ok!(GuardPallet::execute(RuntimeOrigin::signed(keeper()), 1));
    });
}

#[test]
fn r4_nested_call_budget_is_aggregate_across_top_level_batch() {
    new_test_ext().execute_with(|| {
        // Nine individually valid BatchAll wrappers each count as wrapper +
        // leaf: 18 recursive calls total, while the top-level Vec has only 9.
        let calls = (0..9)
            .map(|_| wrapped_call(WrapperKind::BatchAll, CallDomain::Param))
            .collect::<Vec<_>>();
        assert_ok!(enqueue_calls(
            1,
            futarchy_primitives::ProposalClass::Param,
            calls,
            vec![CallDomain::Param],
        ));
        run_to_maturity(1);
        assert_noop!(
            GuardPallet::execute(RuntimeOrigin::signed(keeper()), 1),
            Error::<Test>::TooManyCalls
        );
        assert!(Queue::<Test>::contains_key(1));
    });
}

#[test]
fn i10_only_exact_authorize_and_apply_upgrade_paths_can_reach_internal_root() {
    new_test_ext().execute_with(|| {
        let apply = RuntimeCall::System(frame_system::Call::apply_authorized_upgrade {
            code: b"not-authorized".to_vec(),
        });
        let (payload_hash, payload_len) = put_preimage(&[apply]);
        commit_payload(1, payload_hash);
        let item = queued_item(
            1,
            futarchy_primitives::ProposalClass::Code,
            payload_hash,
            payload_len,
            vec![CallDomain::InternalRootApplyUpgrade],
        );
        assert_noop!(
            GuardPallet::enqueue(RuntimeOrigin::signed(epoch_account()), item, false),
            Error::<Test>::CapabilityDenied
        );

        let arbitrary_system = RuntimeCall::System(frame_system::Call::remark {
            remark: b"root?".to_vec(),
        });
        let (payload_hash, payload_len) = put_preimage(&[arbitrary_system]);
        commit_payload(2, payload_hash);
        let item = queued_item(
            2,
            futarchy_primitives::ProposalClass::Param,
            payload_hash,
            payload_len,
            vec![CallDomain::Public],
        );
        assert_ok!(GuardPallet::enqueue(
            RuntimeOrigin::signed(epoch_account()),
            item,
            false
        ));
        run_to_maturity(2);
        assert_noop!(
            GuardPallet::execute(RuntimeOrigin::signed(keeper()), 2),
            Error::<Test>::BadDomainDeclaration
        );

        let code = b"exact-authorized-runtime";
        setup_upgrade(3, code, 33);
        assert_ok!(GuardPallet::execute(RuntimeOrigin::signed(keeper()), 3));
        assert_eq!(
            PendingUpgradeStorage::<Test>::get().expect("pending").hash,
            hash(code)
        );
    });
}

#[test]
fn native_signed_system_upgrade_calls_are_blocked_by_base_call_filter() {
    new_test_ext().execute_with(|| {
        let authorize_error = authorize_call(hash(b"runtime"))
            .dispatch(RuntimeOrigin::signed(keeper()))
            .expect_err("signed native authorize must be filtered")
            .error;
        assert_eq!(
            authorize_error,
            frame_system::Error::<Test>::CallFiltered.into()
        );

        let apply_error = RuntimeCall::System(frame_system::Call::apply_authorized_upgrade {
            code: b"runtime".to_vec(),
        })
        .dispatch(RuntimeOrigin::signed(keeper()))
        .expect_err("signed native apply must be filtered")
        .error;
        assert_eq!(
            apply_error,
            frame_system::Error::<Test>::CallFiltered.into()
        );
    });
}

#[test]
fn ratify_binds_referendum_and_payload_and_emits_frozen_event() {
    new_test_ext().execute_with(|| {
        let code = b"ratified-runtime";
        let code_hash = hash(code);
        AttestationArtifact::set(Some((7, code_hash)));
        assert_ok!(enqueue_code(1, authorize_call(code_hash), 7, 77));
        assert_noop!(
            GuardPallet::ratify(ratify_origin(), 1, 78),
            Error::<Test>::NotRatified
        );
        assert_ok!(GuardPallet::ratify(ratify_origin(), 1, 77));
        let record = Ratifications::<Test>::get(1).expect("ratification");
        assert_eq!(record.referendum_index, 77);
        assert_eq!(
            record.payload_hash,
            Queue::<Test>::get(1).expect("queued").payload_hash
        );
        assert!(matches!(
            last_guard_event(),
            Some(Event::Ratified {
                pid: 1,
                referendum_index: 77
            })
        ));
        assert_noop!(
            GuardPallet::ratify(ratify_origin(), 1, 77),
            Error::<Test>::NotRatified
        );
    });
}

#[test]
fn prequeue_ratification_is_bound_consumed_and_epoch_reap_is_narrow() {
    new_test_ext().execute_with(|| {
        let code = b"prequeue-ratified-runtime";
        let code_hash = hash(code);
        let (payload_hash, _) = put_preimage(&[authorize_call(code_hash)]);
        commit_payload(1, payload_hash);
        assert_ok!(GuardPallet::ratify(ratify_origin(), 1, 77));
        let record = Ratifications::<Test>::get(1).expect("prequeue ratification");
        assert_eq!(record.payload_hash, payload_hash);
        assert_eq!(record.referendum_index, 77);
        assert!(matches!(
            last_guard_event(),
            Some(Event::Ratified {
                pid: 1,
                referendum_index: 77
            })
        ));

        AttestationArtifact::set(Some((7, code_hash)));
        assert_ok!(enqueue_code(1, authorize_call(code_hash), 7, 77));
        assert!(Queue::<Test>::get(1).expect("queued").ratification_passed);
        assert_eq!(Ratifications::<Test>::get(1), Some(record));

        assert_noop!(
            GuardPallet::reap_prequeue_ratification(RuntimeOrigin::signed(keeper()), 1),
            DispatchError::BadOrigin
        );
        assert_noop!(
            GuardPallet::reap_prequeue_ratification(RuntimeOrigin::signed(epoch_account()), 1),
            Error::<Test>::CapabilityDenied
        );
        assert_eq!(Ratifications::<Test>::get(1), Some(record));
    });

    new_test_ext().execute_with(|| {
        commit_payload(2, [2; 32]);
        assert_ok!(GuardPallet::ratify(ratify_origin(), 2, 88));
        assert_noop!(
            GuardPallet::reap_prequeue_ratification(RuntimeOrigin::signed(keeper()), 2),
            DispatchError::BadOrigin
        );
        assert_ok!(GuardPallet::reap_prequeue_ratification(
            RuntimeOrigin::signed(epoch_account()),
            2
        ));
        assert!(!Ratifications::<Test>::contains_key(2));
    });
}

#[test]
fn r2_dequeue_terminal_is_idempotent_and_clears_all_guard_owned_state() {
    new_test_ext().execute_with(|| {
        let code = b"terminal-cleanup-runtime";
        setup_upgrade(1, code, 17);
        let payload_hash = Queue::<Test>::get(1).expect("queued").payload_hash;
        assert!(Ratifications::<Test>::contains_key(1));
        assert!(Expedited::<Test>::contains_key(1));
        assert!(AttestationBindings::<Test>::contains_key(1));
        assert!(!HeldResources::<Test>::get().is_empty());

        assert_ok!(GuardPallet::dequeue_terminal(1));
        assert!(!Queue::<Test>::contains_key(1));
        assert!(HeldResources::<Test>::get().is_empty());
        assert!(!Ratifications::<Test>::contains_key(1));
        assert!(!Expedited::<Test>::contains_key(1));
        assert!(!AttestationBindings::<Test>::contains_key(1));
        assert!(!PreimageData::get()
            .iter()
            .any(|(candidate, _)| *candidate == payload_hash));
        assert_eq!(Unpinned::get(), vec![payload_hash]);

        assert_ok!(GuardPallet::dequeue_terminal(1));
        assert_eq!(Unpinned::get(), vec![payload_hash]);
        assert_ok!(GuardPallet::do_try_state());
    });
}

#[test]
fn r2_forty_sequential_terminal_dequeues_do_not_exhaust_the_queue() {
    new_test_ext().execute_with(|| {
        for pid in 1..=40 {
            assert_ok!(enqueue_calls(
                pid,
                futarchy_primitives::ProposalClass::Param,
                vec![param_call(pid as u32)],
                vec![CallDomain::Param],
            ));
            assert_ok!(GuardPallet::dequeue_terminal(pid));
            assert_eq!(Queue::<Test>::count(), 0);
            assert!(HeldResources::<Test>::get().is_empty());
        }
        assert_eq!(Unpinned::get().len(), 40);
        assert_ok!(GuardPallet::do_try_state());
    });
}

#[test]
fn upgrade_enforces_lead_time_hash_version_and_release_channel_rollback() {
    new_test_ext().execute_with(|| {
        let code = b"runtime-v2";
        setup_upgrade(1, code, 7);
        assert_ok!(GuardPallet::execute(RuntimeOrigin::signed(keeper()), 1));
        let pending = PendingUpgradeStorage::<Test>::get().expect("pending upgrade");
        assert!(!Ratifications::<Test>::contains_key(1));
        assert!(!Expedited::<Test>::contains_key(1));
        assert!(!AttestationBindings::<Test>::contains_key(1));
        assert_eq!(pending.hash, hash(code));
        assert_eq!(pending.target_spec_version, 2);
        assert_eq!(release_log(), vec![(2, pending.authorized_at, false)]);

        assert_noop!(
            GuardPallet::apply_authorized_upgrade(
                RuntimeOrigin::signed(keeper()),
                bounded_code(code)
            ),
            Error::<Test>::DescriptorLeadTime
        );
        System::set_block_number(pending.applicable_at.into());
        assert_noop!(
            GuardPallet::apply_authorized_upgrade(
                RuntimeOrigin::signed(keeper()),
                bounded_code(b"swapped-runtime")
            ),
            Error::<Test>::UpgradeHashMismatch
        );
        ObservedSpecVersion::set(Some(99));
        assert_noop!(
            GuardPallet::apply_authorized_upgrade(
                RuntimeOrigin::signed(keeper()),
                bounded_code(code)
            ),
            Error::<Test>::UpgradeVersionMismatch
        );
        ObservedSpecVersion::set(Some(2));
        ObservedSpecName::set(b"wrong-spec-name".to_vec());
        assert_noop!(
            GuardPallet::apply_authorized_upgrade(
                RuntimeOrigin::signed(keeper()),
                bounded_code(code)
            ),
            Error::<Test>::UpgradeVersionMismatch
        );
        ObservedSpecName::set(b"test".to_vec());
        ObservedSpecVersion::set(Some(2));
        ReleaseRefuses::set(true);
        assert_ok!(GuardPallet::apply_authorized_upgrade(
            RuntimeOrigin::signed(keeper()),
            bounded_code(code)
        ));
        assert!(GuardPallet::validation_code_applied().is_err());
        assert_eq!(PendingUpgradeStorage::<Test>::get(), Some(pending));
        assert_eq!(CurrentSpecName::<Test>::get(), Some(spec(1)));
        assert_eq!(release_log(), vec![(2, pending.authorized_at, false)]);

        ReleaseRefuses::set(false);
        assert_ok!(GuardPallet::validation_code_applied());
        assert!(PendingUpgradeStorage::<Test>::get().is_none());
        assert!(PendingUpgradeCheckpoint::<Test>::get().is_none());
        assert_eq!(CurrentSpecName::<Test>::get(), Some(spec(2)));
        assert_eq!(release_log().last(), Some(&(2, 0, true)));
        assert!(matches!(
            last_guard_event(),
            Some(Event::UpgradeApplied {
                code_hash,
                spec_version: 2
            }) if code_hash == hash(code)
        ));
    });
}

#[test]
fn scheduled_upgrade_abort_restores_status_quo_and_emits_distinct_event() {
    new_test_ext().execute_with(|| {
        let code = b"relay-aborted-runtime";
        setup_upgrade(1, code, 7);
        assert_ok!(GuardPallet::execute(RuntimeOrigin::signed(keeper()), 1));
        let pending = PendingUpgradeStorage::<Test>::get().expect("pending upgrade");
        System::set_block_number(pending.applicable_at.into());
        assert_ok!(GuardPallet::apply_authorized_upgrade(
            RuntimeOrigin::signed(keeper()),
            bounded_code(code),
        ));
        UpgradeSchedulingPerformed::set(true);
        let _ = GuardPallet::on_initialize(System::block_number());
        assert_eq!(ScheduledUpgrade::<Test>::get(), Some(hash(code)));

        assert_ok!(GuardPallet::validation_code_aborted());

        assert!(PendingUpgradeStorage::<Test>::get().is_none());
        assert!(PendingUpgradeCheckpoint::<Test>::get().is_none());
        assert!(ScheduledUpgrade::<Test>::get().is_none());
        assert_eq!(CurrentSpecName::<Test>::get(), Some(spec(1)));
        assert_eq!(release_log().last(), Some(&(2, 0, true)));
        assert!(matches!(
            last_guard_event(),
            Some(Event::UpgradeAborted { code_hash }) if code_hash == hash(code)
        ));
    });
}

#[test]
fn r1_apply_uses_non_root_and_authorize_is_the_only_internal_root() {
    new_test_ext().execute_with(|| {
        let code = b"origin-audited-runtime";
        setup_upgrade(1, code, 7);
        assert_ok!(GuardPallet::execute(RuntimeOrigin::signed(keeper()), 1));
        let pending = PendingUpgradeStorage::<Test>::get().expect("pending upgrade");
        assert_eq!(
            UpgradeDispatchOrigins::get(),
            vec![UpgradeDispatchOrigin::Root]
        );

        System::set_block_number(pending.applicable_at.into());
        assert_ok!(GuardPallet::apply_authorized_upgrade(
            RuntimeOrigin::signed(keeper()),
            bounded_code(code),
        ));
        let origins = UpgradeDispatchOrigins::get();
        assert_eq!(
            origins,
            vec![UpgradeDispatchOrigin::Root, UpgradeDispatchOrigin::Signed]
        );
        assert_eq!(
            origins
                .iter()
                .filter(|origin| **origin == UpgradeDispatchOrigin::Root)
                .count(),
            1
        );
    });
}

#[test]
fn r3_execute_rechecks_live_attestation_quorum_after_queue() {
    new_test_ext().execute_with(|| {
        let code = b"quorum-recalled-runtime";
        setup_upgrade(1, code, 7);
        AttestationQuorum::set(false);

        assert_noop!(
            GuardPallet::execute(RuntimeOrigin::signed(keeper()), 1),
            Error::<Test>::AttestationMissing
        );
        assert!(Queue::<Test>::contains_key(1));
        assert!(PendingUpgradeStorage::<Test>::get().is_none());
        assert!(UpgradeDispatchOrigins::get().is_empty());
        assert!(release_log().is_empty());
    });
}

#[test]
fn keeper_calls_report_missing_and_nonfailed_entries_without_side_effects() {
    new_test_ext().execute_with(|| {
        assert_noop!(
            GuardPallet::apply_authorized_upgrade(
                RuntimeOrigin::signed(keeper()),
                bounded_code(b"no-pending-upgrade")
            ),
            Error::<Test>::NoPendingUpgrade
        );
        assert_noop!(
            GuardPallet::expire_failed_execution(RuntimeOrigin::signed(keeper()), 99),
            Error::<Test>::NotFound
        );
        assert_noop!(
            GuardPallet::reject_stale(RuntimeOrigin::signed(keeper()), 99),
            Error::<Test>::NotFound
        );
        assert_noop!(
            GuardPallet::ratify(ratify_origin(), 99, 1),
            Error::<Test>::NotFound
        );

        setup_param(1, 1);
        assert_noop!(
            GuardPallet::expire_failed_execution(RuntimeOrigin::signed(keeper()), 1),
            Error::<Test>::NotFound
        );
    });
}

#[test]
fn bad_upgrade_payload_and_arithmetic_overflow_are_pre_dispatch_noops() {
    new_test_ext().execute_with(|| {
        let code = b"duplicate-authorize-runtime";
        let code_hash = hash(code);
        let calls = vec![authorize_call(code_hash), authorize_call(code_hash)];
        let (payload_hash, payload_len) = put_preimage(&calls);
        commit_payload(1, payload_hash);
        AttestationArtifact::set(Some((7, code_hash)));
        let mut item = queued_item(
            1,
            futarchy_primitives::ProposalClass::Code,
            payload_hash,
            payload_len,
            vec![CallDomain::InternalRootAuthorizeUpgrade],
        );
        item.attestation_id = Some(7);
        item.ratify_ref = Some(1);
        assert_noop!(
            GuardPallet::enqueue(RuntimeOrigin::signed(epoch_account()), item, false),
            Error::<Test>::BadUpgradePayload
        );
        assert!(!Queue::<Test>::contains_key(1));
    });

    new_test_ext().execute_with(|| {
        System::set_block_number(u64::from(u32::MAX));
        let (payload_hash, payload_len) = put_preimage(&[param_call(1)]);
        commit_payload(1, payload_hash);
        let item = queued_item(
            1,
            futarchy_primitives::ProposalClass::Param,
            payload_hash,
            payload_len,
            vec![CallDomain::Param],
        );
        assert_noop!(
            GuardPallet::enqueue(RuntimeOrigin::signed(epoch_account()), item, false),
            Error::<Test>::Overflow
        );
        assert!(!Queue::<Test>::contains_key(1));
    });
}

#[test]
fn upgrade_authorization_rejects_hash_mismatch_pending_conflict_and_spacing() {
    new_test_ext().execute_with(|| {
        let code = b"runtime-v2";
        setup_upgrade(1, code, 1);
        AttestationBindings::<Test>::insert(1, (7, [44; 32]));
        assert_noop!(
            GuardPallet::execute(RuntimeOrigin::signed(keeper()), 1),
            Error::<Test>::AttestationMissing
        );
    });
    new_test_ext().execute_with(|| {
        let code = b"runtime-v2";
        setup_upgrade(1, code, 1);
        PendingUpgradeStorage::<Test>::put(execution_guard_core::PendingUpgrade {
            hash: [1; 32],
            authorized_at: 0,
            applicable_at: DESCRIPTOR_LEAD_TIME,
            target_spec_version: 2,
        });
        PendingUpgradeCheckpoint::<Test>::put(([1; 32], [2; 32]));
        assert_noop!(
            GuardPallet::execute(RuntimeOrigin::signed(keeper()), 1),
            Error::<Test>::PendingUpgradeExists
        );
    });
    new_test_ext().execute_with(|| {
        let code = b"runtime-v2";
        setup_upgrade(1, code, 1);
        LastUpgradeAuthorized::<Test>::put(System::block_number() as BlockNumber);
        assert_noop!(
            GuardPallet::execute(RuntimeOrigin::signed(keeper()), 1),
            Error::<Test>::MetersBlocked
        );
    });
}

#[test]
fn code_spacing_exact_boundary_and_expedited_zero_spacing_are_recorded() {
    new_test_ext().execute_with(|| {
        Grace::set(100);
        let code = b"spacing-boundary-runtime";
        setup_upgrade(1, code, 1);
        LastUpgradeAuthorized::<Test>::put(3);
        System::set_block_number(22);
        assert_noop!(
            GuardPallet::execute(RuntimeOrigin::signed(keeper()), 1),
            Error::<Test>::MetersBlocked
        );
        System::set_block_number(23);
        assert_ok!(GuardPallet::execute(RuntimeOrigin::signed(keeper()), 1));
        assert_eq!(UpgradeSpacingHistory::<Test>::get().last(), Some(&(23, 20)));
        assert_ok!(GuardPallet::do_try_state());
    });

    new_test_ext().execute_with(|| {
        let code = b"expedited-runtime";
        let code_hash = hash(code);
        let call = authorize_call(code_hash);
        let (payload_hash, payload_len) = put_preimage(&[call]);
        commit_payload(1, payload_hash);
        AttestationArtifact::set(Some((7, code_hash)));
        MigrationHalt::<Test>::put(true);
        LastUpgradeAuthorized::<Test>::put(1);
        let mut item = queued_item(
            1,
            futarchy_primitives::ProposalClass::Code,
            payload_hash,
            payload_len,
            vec![CallDomain::InternalRootAuthorizeUpgrade],
        );
        item.attestation_id = Some(7);
        item.ratify_ref = Some(9);
        assert_ok!(GuardPallet::enqueue(
            RuntimeOrigin::signed(epoch_account()),
            item,
            true
        ));
        assert_ok!(GuardPallet::ratify(ratify_origin(), 1, 9));
        run_to_maturity(1);
        let executed_at = System::block_number() as BlockNumber;
        assert_ok!(GuardPallet::execute(RuntimeOrigin::signed(keeper()), 1));
        assert_eq!(
            UpgradeSpacingHistory::<Test>::get().last(),
            Some(&(executed_at, 0))
        );
        assert!(!Expedited::<Test>::contains_key(1));
        assert_ok!(GuardPallet::do_try_state());
    });
}

#[test]
fn g1_mid_batch_failure_rolls_back_nested_dispatch_but_persists_t18() {
    new_test_ext().execute_with(|| {
        set_dispatch_failure(true);
        assert_ok!(enqueue_calls(
            1,
            futarchy_primitives::ProposalClass::Param,
            vec![param_call(7), failing_call(8)],
            vec![CallDomain::Param],
        ));
        run_to_maturity(1);
        RecordKeeperRebates::set(true);
        assert_ok!(GuardPallet::execute(RuntimeOrigin::signed(keeper()), 1));

        assert_eq!(pallet_test_dispatch::Value::<Test>::get(), 0);
        assert!(!System::events().iter().any(|record| matches!(
            record.event,
            RuntimeEvent::TestDispatch(pallet_test_dispatch::Event::ValueSet(_))
        )));
        let queued = Queue::<Test>::get(1).expect("retryable queue entry");
        assert!(queued.failed_at.is_some());
        assert_eq!(epoch_calls(), vec![EpochCall::Failed(1)]);
        assert!(matches!(
            last_guard_event(),
            Some(Event::ExecutionFailed { pid: 1, .. })
        ));
        assert!(KeeperRebates::get().is_empty());
    });
}

#[test]
fn g1_epoch_and_release_refusals_roll_back_all_runtime_storage() {
    new_test_ext().execute_with(|| {
        setup_param(1, 9);
        EpochRefuses::set(true);
        let error = GuardPallet::execute(RuntimeOrigin::signed(keeper()), 1)
            .expect_err("epoch callback rejects after inner dispatch");
        assert_eq!(error.error, Error::<Test>::DispatchFailed.into());
        let expected = <() as crate::WeightInfo>::execute(1)
            .saturating_add(param_call(9).get_dispatch_info().total_weight());
        assert_eq!(error.post_info.actual_weight, Some(expected));
        assert_eq!(pallet_test_dispatch::Value::<Test>::get(), 0);
        assert!(Queue::<Test>::contains_key(1));
        assert!(ExecutionRecords::<Test>::get().is_empty());
        assert!(epoch_calls().is_empty());
    });
    new_test_ext().execute_with(|| {
        let code = b"release-refusal-runtime";
        setup_upgrade(1, code, 8);
        ReleaseRefuses::set(true);
        assert!(GuardPallet::execute(RuntimeOrigin::signed(keeper()), 1).is_err());
        assert!(Queue::<Test>::contains_key(1));
        assert!(PendingUpgradeStorage::<Test>::get().is_none());
        assert!(PendingUpgradeCheckpoint::<Test>::get().is_none());
        assert!(ExecutionRecords::<Test>::get().is_empty());
        assert!(epoch_calls().is_empty());
        assert!(release_log().is_empty());
    });
}

#[test]
fn t18_t23_retry_uses_same_committed_call_and_then_succeeds() {
    new_test_ext().execute_with(|| {
        set_dispatch_failure(true);
        assert_ok!(enqueue_calls(
            1,
            futarchy_primitives::ProposalClass::Param,
            vec![failing_call(55)],
            vec![CallDomain::Param],
        ));
        run_to_maturity(1);
        assert_ok!(GuardPallet::execute(RuntimeOrigin::signed(keeper()), 1));
        let failed_at = Queue::<Test>::get(1)
            .and_then(|queued| queued.failed_at)
            .expect("T18 opens retry window");
        assert_eq!(pallet_test_dispatch::Value::<Test>::get(), 0);

        set_dispatch_failure(false);
        System::set_block_number(failed_at.saturating_add(1).into());
        assert_ok!(GuardPallet::execute(RuntimeOrigin::signed(keeper()), 1));
        assert_eq!(pallet_test_dispatch::Value::<Test>::get(), 55);
        assert!(!Queue::<Test>::contains_key(1));
        assert_eq!(
            epoch_calls(),
            vec![EpochCall::Failed(1), EpochCall::Executed(1)]
        );
        assert_eq!(ExecutionRecords::<Test>::get().len(), 2);
    });
}

#[test]
fn t22_expiry_is_closed_during_retry_window_and_terminal_after_it() {
    new_test_ext().execute_with(|| {
        set_dispatch_failure(true);
        assert_ok!(enqueue_calls(
            1,
            futarchy_primitives::ProposalClass::Param,
            vec![failing_call(1)],
            vec![CallDomain::Param],
        ));
        run_to_maturity(1);
        assert_ok!(GuardPallet::execute(RuntimeOrigin::signed(keeper()), 1));
        let failed_at = Queue::<Test>::get(1)
            .and_then(|queued| queued.failed_at)
            .expect("T18");
        RecordKeeperRebates::set(true);
        System::set_block_number(failed_at.saturating_add(RETRY_WINDOW).into());
        assert_noop!(
            GuardPallet::expire_failed_execution(RuntimeOrigin::signed(keeper()), 1),
            Error::<Test>::RetryWindowOpen
        );
        assert!(KeeperRebates::get().is_empty());
        System::set_block_number(
            failed_at
                .saturating_add(RETRY_WINDOW)
                .saturating_add(1)
                .into(),
        );
        assert_ok!(GuardPallet::expire_failed_execution(
            RuntimeOrigin::signed(keeper()),
            1
        ));
        assert!(!Queue::<Test>::contains_key(1));
        assert!(HeldResources::<Test>::get().is_empty());
        assert_eq!(Unpinned::get().len(), 1);
        assert_eq!(
            epoch_calls(),
            vec![EpochCall::Failed(1), EpochCall::RetryExhausted(1)]
        );
        assert_eq!(KeeperRebates::get(), vec![(keeper(), CrankClass::General)]);
        assert_noop!(
            GuardPallet::expire_failed_execution(RuntimeOrigin::signed(keeper()), 1),
            Error::<Test>::NotFound
        );
        assert_eq!(KeeperRebates::get(), vec![(keeper(), CrankClass::General)]);
        assert_ok!(GuardPallet::do_try_state());
    });
}

#[test]
fn reject_stale_is_permissionless_deterministic_and_terminal() {
    new_test_ext().execute_with(|| {
        setup_param(1, 1);
        RecordKeeperRebates::set(true);
        assert_noop!(
            GuardPallet::reject_stale(RuntimeOrigin::signed(keeper()), 1),
            Error::<Test>::StaleQueue
        );
        assert!(KeeperRebates::get().is_empty());
        CurrentSpecName::<Test>::put(spec(2));
        assert_ok!(GuardPallet::reject_stale(
            RuntimeOrigin::signed(keeper()),
            1
        ));
        assert!(!Queue::<Test>::contains_key(1));
        assert!(HeldResources::<Test>::get().is_empty());
        assert_eq!(
            epoch_calls(),
            vec![EpochCall::Rejected(1, RejectReason::StaleQueue)]
        );
        assert!(matches!(
            last_guard_event(),
            Some(Event::Rejected {
                pid: 1,
                reason: RejectReason::StaleQueue
            })
        ));
        assert_eq!(KeeperRebates::get(), vec![(keeper(), CrankClass::General)]);
        assert_noop!(
            GuardPallet::reject_stale(RuntimeOrigin::signed(keeper()), 1),
            Error::<Test>::NotFound
        );
        assert_eq!(KeeperRebates::get(), vec![(keeper(), CrankClass::General)]);
    });
}

#[test]
fn r2_keeper_terminal_endpoints_tolerate_epoch_already_advanced_and_cleanup() {
    new_test_ext().execute_with(|| {
        setup_param(1, 1);
        CurrentSpecName::<Test>::put(spec(2));
        EpochTerminal::set(vec![1]);
        assert_ok!(GuardPallet::reject_stale(
            RuntimeOrigin::signed(keeper()),
            1,
        ));
        assert!(!Queue::<Test>::contains_key(1));
        assert!(HeldResources::<Test>::get().is_empty());
        assert!(epoch_calls().is_empty());
    });

    new_test_ext().execute_with(|| {
        set_dispatch_failure(true);
        assert_ok!(enqueue_calls(
            1,
            futarchy_primitives::ProposalClass::Param,
            vec![failing_call(1)],
            vec![CallDomain::Param],
        ));
        run_to_maturity(1);
        assert_ok!(GuardPallet::execute(RuntimeOrigin::signed(keeper()), 1));
        let failed_at = Queue::<Test>::get(1)
            .and_then(|queued| queued.failed_at)
            .expect("retry window opened");
        System::set_block_number(
            failed_at
                .saturating_add(RETRY_WINDOW)
                .saturating_add(1)
                .into(),
        );
        EpochTerminal::set(vec![1]);
        assert_ok!(GuardPallet::expire_failed_execution(
            RuntimeOrigin::signed(keeper()),
            1,
        ));
        assert!(!Queue::<Test>::contains_key(1));
        assert!(HeldResources::<Test>::get().is_empty());
        assert_eq!(epoch_calls(), vec![EpochCall::Failed(1)]);
    });
}

#[test]
fn r5_payload_weight_ceiling_matches_the_kernel_ratio() {
    let max_block = frame_support::weights::Weight::from_parts(4_000, 400);
    let expected = max_block
        .saturating_mul(futarchy_primitives::kernel::PROP_MAX_WEIGHT_NUM)
        .saturating_div(futarchy_primitives::kernel::PROP_MAX_WEIGHT_DEN);
    assert_eq!(GuardPallet::payload_weight_ceiling(max_block), expected);
}

#[test]
fn execution_record_ring_evicts_fifo_at_256() {
    new_test_ext().execute_with(|| {
        for pid in 1..=MAX_EXECUTION_RECORDS as ProposalId + 1 {
            setup_param(pid, pid as u32);
            assert_ok!(GuardPallet::execute(RuntimeOrigin::signed(keeper()), pid));
        }
        let records = ExecutionRecords::<Test>::get();
        assert_eq!(records.len(), MAX_EXECUTION_RECORDS);
        assert_eq!(records[0].pid, 2);
        assert_eq!(records[MAX_EXECUTION_RECORDS - 1].pid, 257);
        assert_ok!(GuardPallet::do_try_state());
    });
}

#[test]
fn try_state_covers_bounds_bindings_locks_and_i7_i17_envelope() {
    new_test_ext().execute_with(|| {
        setup_param(1, 1);
        assert_ok!(GuardPallet::do_try_state());
        <GuardPallet as Hooks<u64>>::integrity_test();
    });
    new_test_ext().execute_with(|| {
        setup_param(1, 1);
        HeldResources::<Test>::kill();
        assert!(GuardPallet::do_try_state().is_err());
    });
    new_test_ext().execute_with(|| {
        setup_param(1, 1);
        EpochTerminal::set(vec![1]);
        assert!(GuardPallet::do_try_state().is_err());
    });
    new_test_ext().execute_with(|| {
        Ratifications::<Test>::insert(
            99,
            RatificationRecord {
                referendum_index: 1,
                payload_hash: [1; 32],
                ratified_at: 1,
            },
        );
        assert!(GuardPallet::do_try_state().is_err());
    });
    new_test_ext().execute_with(|| {
        PendingUpgradeStorage::<Test>::put(execution_guard_core::PendingUpgrade {
            hash: [1; 32],
            authorized_at: 1,
            applicable_at: DESCRIPTOR_LEAD_TIME.saturating_add(1),
            target_spec_version: 2,
        });
        assert!(GuardPallet::do_try_state().is_err());
    });
    new_test_ext().execute_with(|| {
        CodeSpacing::set(0);
        assert!(GuardPallet::do_try_state().is_err());
    });
    new_test_ext().execute_with(|| {
        BlockedMeters::<Test>::put(
            BoundedVec::try_from(vec![[1; 8], [1; 8]]).expect("two meters fit"),
        );
        assert!(GuardPallet::do_try_state().is_err());
    });
}

struct DifferentialEpoch;

impl execution_guard_core::EpochHandoff for DifferentialEpoch {
    fn mark_executed(&mut self, _pid: ProposalId) -> Result<(), CoreError> {
        Ok(())
    }
    fn mark_failed_executed(&mut self, _pid: ProposalId) -> Result<(), CoreError> {
        Ok(())
    }
    fn retry_exhausted_to_measurement(&mut self, _pid: ProposalId) -> Result<(), CoreError> {
        Ok(())
    }
    fn reject_or_stale(
        &mut self,
        _pid: ProposalId,
        _reason: RejectReason,
    ) -> Result<(), CoreError> {
        Ok(())
    }
}

fn assert_shell_matches_core(model: &execution_guard_core::ExecutionGuard) {
    let mut shell_queue = Queue::<Test>::iter_values()
        .map(QueuedExecution::from)
        .collect::<Vec<_>>();
    shell_queue.sort_by_key(|queued| queued.pid);
    let mut model_queue = model.queue.clone();
    model_queue.sort_by_key(|queued| queued.pid);
    assert_eq!(shell_queue.encode(), model_queue.encode());
    assert_eq!(
        ExecutionRecords::<Test>::get().encode(),
        model.records.encode()
    );
    assert_eq!(PendingUpgradeStorage::<Test>::get(), model.pending_upgrade);
    assert_eq!(
        CurrentSpecName::<Test>::get(),
        Some(model.current_spec_name.clone())
    );
    let mut shell_held = HeldResources::<Test>::get().into_inner();
    shell_held.sort();
    let mut model_held = model.held_resources.clone();
    model_held.sort();
    assert_eq!(shell_held.encode(), model_held.encode());
    assert_eq!(
        BlockedMeters::<Test>::get().into_inner(),
        model.blocked_meters
    );
    assert_eq!(HardGateBreach::<Test>::get(), model.hard_gate_breach);
    assert_eq!(DeadManFreeze::<Test>::get(), model.dead_man_freeze);
    assert_eq!(MigrationHalt::<Test>::get(), model.migration_halt);
}

#[test]
fn seeded_randomized_over_400_step_shell_core_differential_is_byte_identical() {
    new_test_ext().execute_with(|| {
        let mut model = execution_guard_core::ExecutionGuard::new(spec(1));
        let mut epoch = DifferentialEpoch;
        let mut steps = 0usize;
        let mut rng = 0x6a09_e667u32;

        for pid in 1..=200u64 {
            rng = rng.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let call = param_call(rng);
            let (payload_hash, payload_len) = put_preimage(core::slice::from_ref(&call));
            commit_payload(pid, payload_hash);
            let stored = queued_item(
                pid,
                futarchy_primitives::ProposalClass::Param,
                payload_hash,
                payload_len,
                vec![CallDomain::Param],
            );
            let core_item: QueuedExecution = stored.clone().into();
            let resource = core_item.meters_declared[0];

            let shell_result =
                GuardPallet::enqueue(RuntimeOrigin::signed(epoch_account()), stored, false);
            model.held_resources.push((pid, resource));
            let core_result = model.enqueue(GuardOrigin::EpochDecision, core_item);
            assert_eq!(
                shell_result.is_ok(),
                core_result.is_ok(),
                "enqueue step {steps}"
            );
            assert!(matches!(
                model.events.last(),
                Some(CoreEvent::Enqueued { pid: event_pid, .. }) if *event_pid == pid
            ));
            assert!(matches!(
                last_guard_event(),
                Some(Event::Enqueued { pid: event_pid, .. }) if event_pid == pid
            ));
            model.events.clear();
            steps = steps.saturating_add(1);
            assert_shell_matches_core(&model);

            // Seeded operation selection injects zero to two duplicate
            // admissions. Each rejection is a byte-for-byte no-op in both
            // implementations, so the differential covers acceptance as well
            // as the success-only transition path.
            for _ in 0..(rng % 3) {
                let shell_events = System::event_count();
                let shell_result = GuardPallet::enqueue(
                    RuntimeOrigin::signed(epoch_account()),
                    queued_item(
                        pid,
                        futarchy_primitives::ProposalClass::Param,
                        payload_hash,
                        payload_len,
                        vec![CallDomain::Param],
                    ),
                    false,
                );
                let core_result = model.enqueue(
                    GuardOrigin::EpochDecision,
                    model_queue_item(pid, payload_hash, payload_len),
                );
                assert_eq!(
                    shell_result.is_ok(),
                    core_result.is_ok(),
                    "duplicate step {steps}"
                );
                assert!(shell_result.is_err());
                assert_eq!(System::event_count(), shell_events);
                assert!(model.events.is_empty());
                steps = steps.saturating_add(1);
                assert_shell_matches_core(&model);
            }

            run_to_maturity(pid);
            let now = System::block_number() as BlockNumber;
            let shell_result = GuardPallet::execute(RuntimeOrigin::signed(keeper()), pid);
            let core_result = model.complete_prevalidated(
                GuardOrigin::Signed,
                &mut epoch,
                pid,
                DispatchOutcomeCode::Ok,
                now,
                None,
            );
            assert_eq!(
                shell_result.is_ok(),
                core_result.is_ok(),
                "execute step {steps}"
            );
            assert!(matches!(
                model.events.last(),
                Some(CoreEvent::Executed { pid: event_pid, .. }) if *event_pid == pid
            ));
            assert!(matches!(
                last_guard_event(),
                Some(Event::Executed { pid: event_pid, .. }) if event_pid == pid
            ));
            model.events.clear();
            steps = steps.saturating_add(1);
            assert_shell_matches_core(&model);
        }

        assert_eq!(steps, 578);
        assert_ok!(GuardPallet::do_try_state());
    });
}

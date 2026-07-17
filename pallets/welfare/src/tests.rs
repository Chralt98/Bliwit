use crate::mock::*;
use crate::*;
use frame_support::{assert_noop, assert_ok, BoundedVec};
use futarchy_primitives::{keeper::CrankClass, FixedU64};

fn bounded(specs: Vec<MetricSpec>) -> BoundedSpecSet {
    BoundedVec::try_from(specs).expect("test spec set is bounded")
}

fn components(s: u64, c: u64, p: u64, a: u64) -> Vec<ComponentValue> {
    vec![
        ComponentValue {
            id: 1,
            value: FixedU64(s),
        },
        ComponentValue {
            id: 2,
            value: FixedU64(c),
        },
        ComponentValue {
            id: 3,
            value: FixedU64(p),
        },
        ComponentValue {
            id: 4,
            value: FixedU64(a),
        },
    ]
}

#[test]
fn genesis_seeds_the_frontend_named_metric_specs() {
    new_test_ext().execute_with(|| {
        assert_eq!(MetricSpecs::<Test>::iter().count(), 1);
        assert_eq!(Snapshots::<Test>::iter().count(), 0);
        assert_eq!(GateBreachFlags::<Test>::iter().count(), 0);
        assert_eq!(SampledGateDays::<Test>::iter().count(), 0);
        assert_eq!(Welfare::welfare_state().specs, vec![(1, genesis_specs(1))]);
        assert_ok!(Welfare::seed(&Welfare::welfare_state()));
    });
}

#[test]
fn register_spec_happy_path_deposits_core_event() {
    new_test_ext().execute_with(|| {
        CurrentEpochValue::set(0);
        assert_ok!(Welfare::register_spec(
            RuntimeOrigin::signed(governance_acc()),
            2,
            bounded(default_specs(2)),
        ));
        System::assert_last_event(RuntimeEvent::Welfare(Event::MetricSpecRegistered {
            version: 2,
        }));
        assert_eq!(MetricSpecs::<Test>::iter().count(), 2);
    });
}

#[test]
fn register_spec_rejects_closed_origin_misuse_set() {
    new_test_ext().execute_with(|| {
        for origin in [
            RuntimeOrigin::root(),
            RuntimeOrigin::signed(nobody()),
            RuntimeOrigin::none(),
        ] {
            assert_noop!(
                Welfare::register_spec(origin, 2, bounded(default_specs(2))),
                sp_runtime::DispatchError::BadOrigin
            );
        }
    });
}

#[test]
fn keeper_calls_reject_unsigned_and_root_origins() {
    new_test_ext().execute_with(|| {
        assert_noop!(
            Welfare::record_snapshot(RuntimeOrigin::none(), 7, 1),
            sp_runtime::DispatchError::BadOrigin
        );
        assert_noop!(
            Welfare::record_snapshot(RuntimeOrigin::root(), 7, 1),
            sp_runtime::DispatchError::BadOrigin
        );
        assert_noop!(
            Welfare::record_daily_gate(RuntimeOrigin::none(), 7, 0, 1),
            sp_runtime::DispatchError::BadOrigin
        );
        assert_noop!(
            Welfare::record_daily_gate(RuntimeOrigin::root(), 7, 0, 1),
            sp_runtime::DispatchError::BadOrigin
        );
    });
}

#[test]
fn snapshot_happy_path_persists_and_emits() {
    new_test_ext().execute_with(|| {
        assert_ok!(Welfare::record_snapshot(
            RuntimeOrigin::signed(keeper()),
            7,
            1,
        ));
        let snapshot = Snapshots::<Test>::get((7, 1)).expect("snapshot was stored by key");
        assert_eq!(snapshot.welfare, FixedU64(ONE));
        System::assert_last_event(RuntimeEvent::Welfare(Event::SnapshotRecorded {
            epoch: 7,
            spec_version: 1,
            welfare: FixedU64(ONE),
        }));
    });
}

#[test]
fn daily_gate_happy_path_persists_and_emits() {
    new_test_ext().execute_with(|| {
        DailyInput::set(components(800_000_000, ONE, ONE, ONE));
        assert_ok!(Welfare::record_daily_gate(
            RuntimeOrigin::signed(keeper()),
            7,
            3,
            1,
        ));
        let flags = GateBreachFlags::<Test>::get(7).expect("gate flags were stored by epoch");
        assert!(flags.s_breached);
        assert_eq!(flags.day_bitmap, [1 << 3, 0]);
        assert_eq!(SampledGateDays::<Test>::get(7), Some([1 << 3, 0]));
        System::assert_last_event(RuntimeEvent::Welfare(Event::GateBreachRecorded {
            epoch: 7,
            day: 3,
            s_breached: true,
            c_breached: false,
        }));
    });
}

#[test]
fn healthy_daily_gate_marks_sampling_without_marking_a_breach() {
    new_test_ext().execute_with(|| {
        DailyInput::set(components(ONE, ONE, ONE, ONE));
        assert_ok!(Welfare::record_daily_gate(
            RuntimeOrigin::signed(keeper()),
            7,
            4,
            1,
        ));

        let flags = GateBreachFlags::<Test>::get(7).expect("epoch gate record exists");
        assert!(!flags.s_breached);
        assert!(!flags.c_breached);
        assert_eq!(flags.day_bitmap, [0, 0]);
        assert_eq!(SampledGateDays::<Test>::get(7), Some([1 << 4, 0]));
    });
}

#[test]
fn keeper_rebates_only_after_useful_snapshot_and_daily_gate_work() {
    new_test_ext().execute_with(|| {
        RecordKeeperRebates::set(true);

        assert_noop!(
            Welfare::record_snapshot(RuntimeOrigin::signed(keeper()), FINALIZED_NOW, 1),
            Error::<Test>::EpochNotFinalized
        );
        assert!(KeeperRebates::get().is_empty());

        assert_ok!(Welfare::record_snapshot(
            RuntimeOrigin::signed(keeper()),
            7,
            1,
        ));
        assert_eq!(
            KeeperRebates::get(),
            vec![(keeper(), CrankClass::DecisionCritical)]
        );

        // The duplicate/error retry cannot become a rebate drain vector.
        assert_noop!(
            Welfare::record_snapshot(RuntimeOrigin::signed(keeper()), 7, 1),
            Error::<Test>::DuplicateSnapshot
        );
        assert_noop!(
            Welfare::record_daily_gate(RuntimeOrigin::signed(keeper()), 7, 0, 99),
            Error::<Test>::SpecNotFound
        );
        assert_eq!(
            KeeperRebates::get(),
            vec![(keeper(), CrankClass::DecisionCritical)]
        );

        assert_ok!(Welfare::record_daily_gate(
            RuntimeOrigin::signed(keeper()),
            7,
            0,
            1,
        ));
        assert_eq!(
            GateBreachFlags::<Test>::get(7)
                .expect("healthy epoch gate record exists")
                .day_bitmap,
            [0, 0]
        );
        assert_eq!(SampledGateDays::<Test>::get(7), Some([1, 0]));
        assert_eq!(
            KeeperRebates::get(),
            vec![
                (keeper(), CrankClass::DecisionCritical),
                (keeper(), CrankClass::General),
            ]
        );
        // An identical successful re-record is repeat-tolerant but state-neutral,
        // so it cannot drain the keeper meter.
        assert_ok!(Welfare::record_daily_gate(
            RuntimeOrigin::signed(keeper()),
            7,
            0,
            1,
        ));
        assert_eq!(
            KeeperRebates::get(),
            vec![
                (keeper(), CrankClass::DecisionCritical),
                (keeper(), CrankClass::General),
            ]
        );
        // Re-recording the same day with a newly breached gate advances the
        // epoch-wide latch and therefore earns one further rebate.
        DailyInput::set(components(800_000_000, ONE, ONE, ONE));
        assert_ok!(Welfare::record_daily_gate(
            RuntimeOrigin::signed(keeper()),
            7,
            0,
            1,
        ));
        assert_eq!(
            KeeperRebates::get(),
            vec![
                (keeper(), CrankClass::DecisionCritical),
                (keeper(), CrankClass::General),
                (keeper(), CrankClass::General),
            ]
        );
        assert_eq!(
            GateBreachFlags::<Test>::get(7)
                .expect("augmented epoch gate record exists")
                .day_bitmap,
            [1, 0]
        );
        assert_eq!(SampledGateDays::<Test>::get(7), Some([1, 0]));
    });
}

#[test]
fn duplicate_spec_version_is_rejected_without_storage_change() {
    new_test_ext().execute_with(|| {
        let before = MetricSpecs::<Test>::iter().collect::<Vec<_>>();
        assert_noop!(
            Welfare::register_spec(
                RuntimeOrigin::signed(governance_acc()),
                1,
                bounded(default_specs(1)),
            ),
            Error::<Test>::DuplicateSpecVersion
        );
        assert_eq!(MetricSpecs::<Test>::iter().collect::<Vec<_>>(), before);
    });
}

#[test]
fn bad_activation_epoch_is_rejected() {
    new_test_ext().execute_with(|| {
        CurrentEpochValue::set(5);
        let mut specs = default_specs(2);
        specs[0].activation_epoch = 6;
        assert_noop!(
            Welfare::register_spec(RuntimeOrigin::signed(governance_acc()), 2, bounded(specs),),
            Error::<Test>::BadActivationEpoch
        );
    });
}

#[test]
fn missing_metric_discipline_is_rejected() {
    new_test_ext().execute_with(|| {
        CurrentEpochValue::set(0);
        let mut specs = default_specs(2);
        specs[0].has_challenge_procedure = false;
        assert_noop!(
            Welfare::register_spec(RuntimeOrigin::signed(governance_acc()), 2, bounded(specs),),
            Error::<Test>::MissingMetricDiscipline
        );
    });
}

#[test]
fn bad_weight_sum_is_rejected() {
    new_test_ext().execute_with(|| {
        CurrentEpochValue::set(0);
        let mut specs = default_specs(2);
        specs[1].weight = FixedU64(ONE - 1);
        assert_noop!(
            Welfare::register_spec(RuntimeOrigin::signed(governance_acc()), 2, bounded(specs),),
            Error::<Test>::BadWeightSum
        );
    });
}

#[test]
fn bad_epsilon_floor_and_source_class_are_rejected() {
    new_test_ext().execute_with(|| {
        CurrentEpochValue::set(0);
        let mut specs = default_specs(2);
        specs[0].epsilon_floor = FixedU64(EPSILON_PILLAR.0 - 1);
        assert_noop!(
            Welfare::register_spec(RuntimeOrigin::signed(governance_acc()), 2, bounded(specs),),
            Error::<Test>::BadEpsilonFloor
        );

        let mut specs = default_specs(2);
        specs[3].source = SourceClass::Onchain;
        assert_noop!(
            Welfare::register_spec(RuntimeOrigin::signed(governance_acc()), 2, bounded(specs),),
            Error::<Test>::BadSourceClass
        );
    });
}

#[test]
fn genesis_spec_is_active_from_epoch_one() {
    // 05 §4.6 cold start: the genesis MetricSpec activates at epoch 1, so W₁ is
    // computable. The ext-builder clock is finalized-high, so epoch 1 is past.
    new_test_ext().execute_with(|| {
        assert_ok!(Welfare::record_snapshot(
            RuntimeOrigin::signed(keeper()),
            1,
            1,
        ));
        assert!(Snapshots::<Test>::contains_key((1, 1)));
        assert_ok!(Welfare::record_daily_gate(
            RuntimeOrigin::signed(keeper()),
            1,
            0,
            1,
        ));
        assert!(GateBreachFlags::<Test>::contains_key(1));
    });
}

#[test]
fn post_genesis_spec_before_activation_is_rejected() {
    new_test_ext().execute_with(|| {
        // Register v2 post-genesis (clock 5) activating at epoch 10 (>= 5 + 2).
        CurrentEpochValue::set(5);
        assert_ok!(Welfare::register_spec(
            RuntimeOrigin::signed(governance_acc()),
            2,
            bounded(specs_activating(2, 10)),
        ));
        // Epoch 9 is finalized (clock 20) but still before v2's activation (10).
        CurrentEpochValue::set(20);
        assert_noop!(
            Welfare::record_snapshot(RuntimeOrigin::signed(keeper()), 9, 2),
            Error::<Test>::SpecNotActive
        );
        assert_noop!(
            Welfare::record_daily_gate(RuntimeOrigin::signed(keeper()), 9, 0, 2),
            Error::<Test>::SpecNotActive
        );
        assert!(!Snapshots::<Test>::contains_key((9, 2)));
        assert!(!GateBreachFlags::<Test>::contains_key(9));
    });
}

#[test]
fn snapshot_for_an_unfinalized_or_future_epoch_is_rejected() {
    // 05 §4.6: only a finalized (strictly past) epoch may be snapshotted. The
    // current epoch (still in progress) and any future epoch are rejected, so an
    // early keeper cannot lock a wrong W or consume the bounded window early.
    new_test_ext().execute_with(|| {
        CurrentEpochValue::set(7);
        for epoch in [7u32, 8, 100] {
            assert_noop!(
                Welfare::record_snapshot(RuntimeOrigin::signed(keeper()), epoch, 1),
                Error::<Test>::EpochNotFinalized
            );
        }
        assert_eq!(Snapshots::<Test>::iter().count(), 0);
        // Once the clock passes epoch 7, its snapshot becomes admissible.
        CurrentEpochValue::set(8);
        assert_ok!(Welfare::record_snapshot(
            RuntimeOrigin::signed(keeper()),
            7,
            1,
        ));
    });
}

#[test]
fn daily_gate_for_an_unfinalized_or_future_epoch_is_rejected() {
    new_test_ext().execute_with(|| {
        CurrentEpochValue::set(7);
        for epoch in [7u32, 8, 100] {
            assert_noop!(
                Welfare::record_daily_gate(RuntimeOrigin::signed(keeper()), epoch, 0, 1),
                Error::<Test>::EpochNotFinalized
            );
        }
        assert_eq!(GateBreachFlags::<Test>::iter().count(), 0);
        CurrentEpochValue::set(8);
        assert_ok!(Welfare::record_daily_gate(
            RuntimeOrigin::signed(keeper()),
            7,
            0,
            1,
        ));
    });
}

#[test]
fn metric_inputs_are_scoped_by_spec_version() {
    new_test_ext().execute_with(|| {
        // Register v2 with the two-epoch lead honored (clock 0 → activation 2),
        // then advance the clock so epoch 7 is finalized before the cranks.
        CurrentEpochValue::set(0);
        assert_ok!(Welfare::register_spec(
            RuntimeOrigin::signed(governance_acc()),
            2,
            bounded(default_specs(2)),
        ));
        CurrentEpochValue::set(FINALIZED_NOW);
        OnchainInputsByVersion::set(vec![
            (1, components(ONE, ONE, ONE, ONE)),
            (2, components(ONE, 900_000_000, ONE, ONE)),
        ]);

        assert_ok!(Welfare::record_snapshot(
            RuntimeOrigin::signed(keeper()),
            7,
            1,
        ));
        assert_ok!(Welfare::record_snapshot(
            RuntimeOrigin::signed(keeper()),
            7,
            2,
        ));
        let v1 = Snapshots::<Test>::get((7, 1)).expect("version 1 snapshot exists");
        let v2 = Snapshots::<Test>::get((7, 2)).expect("version 2 snapshot exists");
        assert_ne!(v1.c_onchain, v2.c_onchain);
    });
}

#[test]
fn duplicate_snapshot_is_rejected() {
    new_test_ext().execute_with(|| {
        assert_ok!(Welfare::record_snapshot(
            RuntimeOrigin::signed(keeper()),
            7,
            1,
        ));
        assert_noop!(
            Welfare::record_snapshot(RuntimeOrigin::signed(keeper()), 7, 1),
            Error::<Test>::DuplicateSnapshot
        );
        assert_eq!(Snapshots::<Test>::iter().count(), 1);
    });
}

#[test]
fn missing_spec_is_rejected() {
    new_test_ext().execute_with(|| {
        assert_noop!(
            Welfare::record_snapshot(RuntimeOrigin::signed(keeper()), 7, 99),
            Error::<Test>::SpecNotFound
        );
        assert_noop!(
            Welfare::record_daily_gate(RuntimeOrigin::signed(keeper()), 7, 0, 99),
            Error::<Test>::SpecNotFound
        );
    });
}

#[test]
fn metric_spec_history_accepts_16_and_rejects_17th() {
    // limit-coverage: MetricSpecs
    new_test_ext().execute_with(|| {
        CurrentEpochValue::set(0);
        for version in 2..=16 {
            assert_ok!(Welfare::register_spec(
                RuntimeOrigin::signed(governance_acc()),
                version,
                bounded(default_specs(version)),
            ));
        }
        assert_eq!(MetricSpecs::<Test>::iter().count(), MAX_METRIC_SPECS);
        assert_noop!(
            Welfare::register_spec(
                RuntimeOrigin::signed(governance_acc()),
                17,
                bounded(default_specs(17)),
            ),
            Error::<Test>::TooManyMetricSpecs
        );
    });
}

#[test]
fn snapshot_history_accepts_20_and_rejects_21st() {
    // limit-coverage: Snapshots
    new_test_ext().execute_with(|| {
        for epoch in 2..MAX_SNAPSHOTS as u32 + 2 {
            assert_ok!(Welfare::record_snapshot(
                RuntimeOrigin::signed(keeper()),
                epoch,
                1,
            ));
        }
        assert_noop!(
            Welfare::record_snapshot(RuntimeOrigin::signed(keeper()), MAX_SNAPSHOTS as u32 + 2, 1,),
            Error::<Test>::TooManySnapshots
        );
    });
}

#[test]
fn gate_history_accepts_20_epochs_and_rejects_21st() {
    new_test_ext().execute_with(|| {
        for epoch in 2..MAX_GATE_FLAGS as u32 + 2 {
            assert_ok!(Welfare::record_daily_gate(
                RuntimeOrigin::signed(keeper()),
                epoch,
                0,
                1,
            ));
        }
        assert_noop!(
            Welfare::record_daily_gate(
                RuntimeOrigin::signed(keeper()),
                MAX_GATE_FLAGS as u32 + 2,
                0,
                1,
            ),
            Error::<Test>::TooManyGateFlags
        );
    });
}

#[test]
fn prune_rolls_the_snapshot_and_gate_windows() {
    new_test_ext().execute_with(|| {
        for epoch in 2..MAX_SNAPSHOTS as u32 + 2 {
            assert_ok!(Welfare::record_snapshot(
                RuntimeOrigin::signed(keeper()),
                epoch,
                1,
            ));
            assert_ok!(Welfare::record_daily_gate(
                RuntimeOrigin::signed(keeper()),
                epoch,
                0,
                1,
            ));
        }
        assert_eq!(Snapshots::<Test>::iter().count(), MAX_SNAPSHOTS);
        assert_eq!(GateBreachFlags::<Test>::iter().count(), MAX_GATE_FLAGS);
        assert_eq!(SampledGateDays::<Test>::iter().count(), MAX_GATE_FLAGS);

        assert_ok!(Welfare::prune(3));
        assert!(!Snapshots::<Test>::contains_key((2, 1)));
        assert!(!GateBreachFlags::<Test>::contains_key(2));
        assert!(!SampledGateDays::<Test>::contains_key(2));
        assert_eq!(MetricSpecs::<Test>::iter().count(), 1);

        let next = MAX_SNAPSHOTS as u32 + 2;
        assert_ok!(Welfare::record_snapshot(
            RuntimeOrigin::signed(keeper()),
            next,
            1,
        ));
        assert_ok!(Welfare::record_daily_gate(
            RuntimeOrigin::signed(keeper()),
            next,
            0,
            1,
        ));
        assert!(Snapshots::<Test>::contains_key((next, 1)));
        assert!(GateBreachFlags::<Test>::contains_key(next));
        assert!(SampledGateDays::<Test>::contains_key(next));
        assert_eq!(Snapshots::<Test>::iter().count(), MAX_SNAPSHOTS);
        assert_eq!(GateBreachFlags::<Test>::iter().count(), MAX_GATE_FLAGS);
        assert_eq!(SampledGateDays::<Test>::iter().count(), MAX_GATE_FLAGS);
    });
}

#[test]
fn injected_component_vector_over_limit_is_rejected() {
    new_test_ext().execute_with(|| {
        OnchainInput::set(
            (0..=MAX_COMPONENTS_PER_SPEC as u16)
                .map(|id| ComponentValue {
                    id,
                    value: FixedU64(ONE),
                })
                .collect(),
        );
        assert_noop!(
            Welfare::record_snapshot(RuntimeOrigin::signed(keeper()), 7, 1),
            Error::<Test>::TooManyComponents
        );
        DailyInput::set(OnchainInput::get());
        assert_noop!(
            Welfare::record_daily_gate(RuntimeOrigin::signed(keeper()), 7, 0, 1),
            Error::<Test>::TooManyComponents
        );
    });
}

#[test]
fn try_state_passes_after_representative_sequence() {
    new_test_ext().execute_with(|| {
        CurrentEpochValue::set(0);
        assert_ok!(Welfare::register_spec(
            RuntimeOrigin::signed(governance_acc()),
            2,
            bounded(default_specs(2)),
        ));
        CurrentEpochValue::set(FINALIZED_NOW);
        assert_ok!(Welfare::record_snapshot(
            RuntimeOrigin::signed(keeper()),
            7,
            1,
        ));
        assert_ok!(Welfare::record_daily_gate(
            RuntimeOrigin::signed(keeper()),
            7,
            0,
            1,
        ));
        assert_ok!(Welfare::do_try_state());
    });
}

#[test]
fn try_state_rejects_a_snapshot_stored_under_the_wrong_map_key() {
    new_test_ext().execute_with(|| {
        assert_ok!(Welfare::record_snapshot(
            RuntimeOrigin::signed(keeper()),
            7,
            1,
        ));
        let snapshot = Snapshots::<Test>::take((7, 1)).expect("snapshot exists");
        Snapshots::<Test>::insert((8, 1), snapshot);
        assert!(Welfare::do_try_state().is_err());
    });
}

#[test]
fn try_state_rejects_an_orphan_sampled_gate_marker() {
    new_test_ext().execute_with(|| {
        SampledGateDays::<Test>::insert(7, [1, 0]);
        assert!(!GateBreachFlags::<Test>::contains_key(7));
        assert!(Welfare::do_try_state().is_err());
    });
}

#[test]
fn live_param_flip_changes_gate_and_welfare() {
    new_test_ext().execute_with(|| {
        OnchainInput::set(components(ONE, 900_000_000, 500_000_000, ONE));
        assert_ok!(Welfare::record_snapshot(
            RuntimeOrigin::signed(keeper()),
            7,
            1,
        ));
        let before = Snapshots::<Test>::get((7, 1)).expect("first snapshot exists");

        ThetaCHi::set(FixedU64(990_000_000));
        WP::set(FixedU64(650_000_000));
        WA::set(FixedU64(350_000_000));
        assert_ok!(Welfare::record_snapshot(
            RuntimeOrigin::signed(keeper()),
            8,
            1,
        ));
        let after = Snapshots::<Test>::get((8, 1)).expect("second snapshot exists");
        assert!(after.gate_c.0 < before.gate_c.0);
        assert_ne!(after.welfare, before.welfare);
    });
}

#[test]
fn invalid_live_params_fail_closed_as_bad_params() {
    new_test_ext().execute_with(|| {
        WP::set(FixedU64(900_000_000));
        WA::set(FixedU64(400_000_000));
        assert_noop!(
            Welfare::record_snapshot(RuntimeOrigin::signed(keeper()), 7, 1),
            Error::<Test>::BadParams
        );
        assert_eq!(Snapshots::<Test>::iter().count(), 0);

        WP::set(crate::W_P);
        WA::set(crate::W_A);
        ThetaSLo::set(FixedU64(crate::THETA_S_LO.0 - 1));
        assert_noop!(
            Welfare::record_daily_gate(RuntimeOrigin::signed(keeper()), 7, 0, 1),
            Error::<Test>::BadParams
        );
        assert_eq!(GateBreachFlags::<Test>::iter().count(), 0);
    });
}

#[test]
fn compute_settlement_dispatches_scalar_gates_and_baseline() {
    new_test_ext().execute_with(|| {
        assert_ok!(Welfare::record_snapshot(
            RuntimeOrigin::signed(keeper()),
            11,
            1,
        ));
        assert_ok!(Welfare::record_snapshot(
            RuntimeOrigin::signed(keeper()),
            12,
            1,
        ));

        DailyInput::set(components(800_000_000, ONE, ONE, ONE));
        assert_ok!(Welfare::record_daily_gate(
            RuntimeOrigin::signed(keeper()),
            11,
            0,
            1,
        ));
        DailyInput::set(components(ONE, 800_000_000, ONE, ONE));
        assert_ok!(Welfare::record_daily_gate(
            RuntimeOrigin::signed(keeper()),
            12,
            1,
            1,
        ));

        LedgerCalls::set(Vec::new());
        assert_ok!(Welfare::compute_settlement(
            10,
            1,
            SettleTarget::Proposal {
                pid: 42,
                has_gate_books: true,
            },
        ));
        assert_eq!(
            LedgerCalls::get(),
            vec![
                LedgerCall::Scalar(42, FixedU64(ONE)),
                LedgerCall::Gate(42, GateKind::Survival, true),
                LedgerCall::Gate(42, GateKind::Security, true),
            ]
        );
        System::assert_last_event(RuntimeEvent::Welfare(Event::SettlementComputed {
            epoch: 10,
            spec_version: 1,
            score: FixedU64(ONE),
        }));

        LedgerCalls::set(Vec::new());
        assert_ok!(Welfare::compute_settlement(
            10,
            1,
            SettleTarget::Proposal {
                pid: 43,
                has_gate_books: false,
            },
        ));
        assert_eq!(
            LedgerCalls::get(),
            vec![LedgerCall::Scalar(43, FixedU64(ONE))]
        );

        assert_ok!(Welfare::compute_settlement(10, 1, SettleTarget::Baseline,));
        assert_eq!(
            LedgerCalls::get().last(),
            Some(&LedgerCall::Baseline(10, FixedU64(ONE)))
        );
    });
}

#[test]
fn ledger_failure_is_atomic_and_emits_no_settlement_event() {
    new_test_ext().execute_with(|| {
        assert_ok!(Welfare::record_snapshot(
            RuntimeOrigin::signed(keeper()),
            11,
            1,
        ));
        assert_ok!(Welfare::record_snapshot(
            RuntimeOrigin::signed(keeper()),
            12,
            1,
        ));
        let before_state = Welfare::welfare_state();
        let before_events = System::events();
        LedgerCalls::set(Vec::new());
        LedgerFailure::set(Some(LedgerCall::Gate(42, GateKind::Security, false)));

        assert_noop!(
            Welfare::compute_settlement(
                10,
                1,
                SettleTarget::Proposal {
                    pid: 42,
                    has_gate_books: true,
                },
            ),
            sp_runtime::DispatchError::Other("injected ledger failure")
        );
        assert_eq!(Welfare::welfare_state(), before_state);
        assert_eq!(System::events(), before_events);
        assert!(LedgerCalls::get().is_empty());
        assert!(!System::events().iter().any(|record| matches!(
            record.event,
            RuntimeEvent::Welfare(Event::SettlementComputed { .. })
        )));
    });
}

fn assert_last_matches_core(event: CoreEvent) {
    let expected = match event {
        CoreEvent::MetricSpecRegistered { version } => {
            RuntimeEvent::Welfare(Event::MetricSpecRegistered { version })
        }
        CoreEvent::SnapshotRecorded {
            epoch,
            spec_version,
            welfare,
        } => RuntimeEvent::Welfare(Event::SnapshotRecorded {
            epoch,
            spec_version,
            welfare,
        }),
        CoreEvent::GateBreachRecorded {
            epoch,
            day,
            s_breached,
            c_breached,
        } => RuntimeEvent::Welfare(Event::GateBreachRecorded {
            epoch,
            day,
            s_breached,
            c_breached,
        }),
        CoreEvent::SettlementComputed {
            epoch,
            spec_version,
            score,
        } => RuntimeEvent::Welfare(Event::SettlementComputed {
            epoch,
            spec_version,
            score,
        }),
    };
    System::assert_last_event(expected);
}

#[test]
fn shell_matches_core_over_400_step_fixed_seed_sequence() {
    new_test_ext().execute_with(|| {
        let mut core = WelfareState::new();
        core.register_metric_spec(0, 1, genesis_specs(1))
            .expect("seed spec is valid");
        core.events.clear();
        let params = CoreWelfareParams::DEFAULT;
        let mut seed = 0x6d2b_79f5_u64;

        for step in 0..400u32 {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let selector = (seed >> 61) as u8 % 5;
            let version = 1 + ((seed >> 8) % 19) as u16;
            LedgerCalls::set(Vec::new());
            let mut expected_ledger_calls = Vec::new();
            let expected_ok = match selector {
                0 => {
                    let version = 2 + ((seed >> 16) % 18) as u16;
                    // Register at the live clock with the two-epoch lead honored,
                    // exactly as the extrinsic does; both sides use the same
                    // `now`, so shell ≡ core holds. These post-genesis specs
                    // activate at `now + 2`, past every snapshot epoch below, so
                    // the snapshot steps exercise the SpecNotActive mirror while
                    // the genesis version (active from epoch 1) drives success.
                    let now = CurrentEpochValue::get();
                    let specs = specs_activating(version, now + 2);
                    let core_result = core.register_metric_spec(now, version, specs.clone());
                    let pallet_result = Welfare::register_spec(
                        RuntimeOrigin::signed(governance_acc()),
                        version,
                        bounded(specs),
                    );
                    let expected_ok = core_result.is_ok();
                    assert_eq!(pallet_result.is_ok(), expected_ok, "register step {step}");
                    expected_ok
                }
                1 => {
                    let epoch = 100 + ((seed >> 20) % 25) as u32;
                    let c = 850_000_000 + (seed % 150_000_001);
                    let p = 500_000_000 + ((seed >> 7) % 500_000_001);
                    let values = components(ONE, c, p, ONE);
                    OnchainInput::set(values.clone());
                    let core_result =
                        core.record_snapshot(epoch, version, values, FixedU64(ONE), &params);
                    let pallet_result =
                        Welfare::record_snapshot(RuntimeOrigin::signed(keeper()), epoch, version);
                    let expected_ok = core_result.is_ok();
                    assert_eq!(pallet_result.is_ok(), expected_ok, "snapshot step {step}");
                    expected_ok
                }
                2 => {
                    let epoch = 200 + ((seed >> 24) % 25) as u32;
                    let day = ((seed >> 32) % 64) as u8;
                    let s = 800_000_000 + (seed % 200_000_001);
                    let c = 800_000_000 + ((seed >> 6) % 200_000_001);
                    let values = components(s, c, ONE, ONE);
                    DailyInput::set(values.clone());
                    let core_result = core.record_daily_gate(epoch, day, version, values, &params);
                    let pallet_result = Welfare::record_daily_gate(
                        RuntimeOrigin::signed(keeper()),
                        epoch,
                        day,
                        version,
                    );
                    let expected_ok = core_result.is_ok();
                    assert_eq!(pallet_result.is_ok(), expected_ok, "daily step {step}");
                    expected_ok
                }
                3 => {
                    let cohort = 99 + ((seed >> 28) % 25) as u32;
                    let core_result = core.compute_settlement(cohort, version);
                    if let Ok(score) = core_result {
                        expected_ledger_calls.push(LedgerCall::Baseline(cohort, score));
                    }
                    let pallet_result =
                        Welfare::compute_settlement(cohort, version, SettleTarget::Baseline);
                    let expected_ok = core_result.is_ok();
                    assert_eq!(pallet_result.is_ok(), expected_ok, "settle step {step}");
                    expected_ok
                }
                _ => {
                    let cohort = 99 + ((seed >> 28) % 25) as u32;
                    let pid = 42_u64 + u64::from((seed >> 40) as u8);
                    let core_result = core.compute_settlement(cohort, version);
                    if let Ok(score) = core_result {
                        let first = core.gate_breach(cohort + 1);
                        let second = core.gate_breach(cohort + 2);
                        expected_ledger_calls.extend([
                            LedgerCall::Scalar(pid, score),
                            LedgerCall::Gate(
                                pid,
                                GateKind::Survival,
                                first.s_breached || second.s_breached,
                            ),
                            LedgerCall::Gate(
                                pid,
                                GateKind::Security,
                                first.c_breached || second.c_breached,
                            ),
                        ]);
                    }
                    let pallet_result = Welfare::compute_settlement(
                        cohort,
                        version,
                        SettleTarget::Proposal {
                            pid,
                            has_gate_books: true,
                        },
                    );
                    let expected_ok = core_result.is_ok();
                    assert_eq!(
                        pallet_result.is_ok(),
                        expected_ok,
                        "proposal settle step {step}"
                    );
                    expected_ok
                }
            };

            assert_eq!(
                LedgerCalls::get(),
                expected_ledger_calls,
                "ledger calls diverged at step {step}"
            );

            if expected_ok {
                assert_eq!(core.events.len(), 1, "event cardinality at step {step}");
                assert_last_matches_core(core.events[0]);
            } else {
                assert!(core.events.is_empty(), "failed core emitted at step {step}");
            }
            core.events.clear();

            let mut shell = Welfare::welfare_state();
            shell.specs.sort_by_key(|(version, _)| *version);
            shell
                .snapshots
                .sort_by_key(|snapshot| (snapshot.epoch, snapshot.spec_version));
            shell.gate_flags.sort_by_key(|(epoch, _)| *epoch);
            core.specs.sort_by_key(|(version, _)| *version);
            core.snapshots
                .sort_by_key(|snapshot| (snapshot.epoch, snapshot.spec_version));
            core.gate_flags.sort_by_key(|(epoch, _)| *epoch);
            assert_eq!(shell.specs, core.specs, "specs diverged at step {step}");
            assert_eq!(
                shell.snapshots, core.snapshots,
                "snapshots diverged at step {step}"
            );
            assert_eq!(
                shell.gate_flags, core.gate_flags,
                "gate flags diverged at step {step}"
            );
        }
    });
}

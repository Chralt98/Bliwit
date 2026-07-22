use crate::mock::*;
use crate::*;
use frame_support::{assert_noop, assert_ok, BoundedVec};
use futarchy_primitives::{keeper::CrankClass, kernel, FixedU64};

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
fn snapshot_deadline_is_strict_and_a_due_snapshot_advances_it() {
    new_test_ext().execute_with(|| {
        let due_epoch = 7;
        let due_at = TestSnapshotSchedule::snapshot_due(due_epoch);
        assert!(due_at.is_some(), "mock epoch schedule must be finite");
        let due_at = due_at.unwrap_or_default();
        SnapshotDeadline::<Test>::put(SnapshotProgress {
            last_snapshot_epoch: Some(due_epoch - 1),
            due_epoch,
        });
        let boundary =
            due_at.checked_add(futarchy_primitives::kernel::DEAD_MAN_SNAPSHOT_OVERDUE_BLOCKS);
        assert!(boundary.is_some(), "test deadline must fit");
        let boundary = boundary.unwrap_or_default();

        assert!(!Welfare::snapshot_overdue(boundary.saturating_sub(1)));
        assert!(!Welfare::snapshot_overdue(boundary));
        assert!(Welfare::snapshot_overdue(boundary.saturating_add(1)));

        assert_ok!(Welfare::record_snapshot(
            RuntimeOrigin::signed(keeper()),
            due_epoch,
            1,
        ));
        let progress = SnapshotDeadline::<Test>::get();
        assert!(progress.is_some(), "deadline remains armed");
        let progress = progress.unwrap_or(SnapshotProgress {
            last_snapshot_epoch: None,
            due_epoch: 0,
        });
        assert_eq!(progress.last_snapshot_epoch, Some(due_epoch));
        assert_eq!(progress.due_epoch, due_epoch.saturating_add(1));
        let next_due = TestSnapshotSchedule::snapshot_due(due_epoch.saturating_add(1));
        assert!(
            next_due.is_some(),
            "next mock epoch schedule must be finite"
        );
        let next_due = next_due.unwrap_or_default();
        assert_eq!(
            TestSnapshotSchedule::snapshot_due(progress.due_epoch),
            Some(next_due)
        );
        assert!(!Welfare::snapshot_overdue(boundary.saturating_add(1)));
        assert!(Welfare::do_try_state().is_ok());
    });
}

#[test]
fn first_snapshot_does_not_become_overdue_before_its_activation_epoch_close() {
    new_test_ext().execute_with(|| {
        SnapshotDeadline::<Test>::kill();
        let first_due = TestSnapshotSchedule::snapshot_due(1);
        assert!(
            first_due.is_some(),
            "genesis MetricSpec activation must have a due block"
        );
        let first_due = first_due.unwrap_or_default();
        assert!(!Welfare::snapshot_overdue(first_due));
        assert_eq!(
            SnapshotDeadline::<Test>::get(),
            Some(SnapshotProgress {
                last_snapshot_epoch: None,
                due_epoch: 1,
            })
        );
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
fn snapshot_deadline_uses_latest_unique_activation_not_largest_version() {
    new_test_ext().execute_with(|| {
        for (version, _) in MetricSpecs::<Test>::iter() {
            MetricSpecs::<Test>::remove(version);
        }
        MetricSpecs::<Test>::insert(9, bounded(specs_activating(9, 5)));
        MetricSpecs::<Test>::insert(2, bounded(specs_activating(2, 7)));
        SnapshotDeadline::<Test>::put(SnapshotProgress {
            last_snapshot_epoch: None,
            due_epoch: 7,
        });

        assert_eq!(Welfare::active_snapshot_spec(7), Some(2));
        assert_ok!(Welfare::record_snapshot(
            RuntimeOrigin::signed(keeper()),
            7,
            2,
        ));
        assert_eq!(
            SnapshotDeadline::<Test>::get().map(|progress| progress.due_epoch),
            Some(8)
        );
    });
}

#[test]
fn tied_latest_activations_cannot_suppress_the_snapshot_detector() {
    new_test_ext().execute_with(|| {
        for (version, _) in MetricSpecs::<Test>::iter() {
            MetricSpecs::<Test>::remove(version);
        }
        MetricSpecs::<Test>::insert(9, bounded(specs_activating(9, 7)));
        MetricSpecs::<Test>::insert(2, bounded(specs_activating(2, 7)));
        SnapshotDeadline::<Test>::put(SnapshotProgress {
            last_snapshot_epoch: None,
            due_epoch: 7,
        });

        assert_eq!(Welfare::active_snapshot_spec(7), None);
        assert_ok!(Welfare::record_snapshot(
            RuntimeOrigin::signed(keeper()),
            7,
            9,
        ));
        assert_eq!(
            SnapshotDeadline::<Test>::get().map(|progress| progress.due_epoch),
            Some(7)
        );
    });
}

#[test]
fn snapshot_deadline_overflow_is_not_spuriously_overdue() {
    new_test_ext().execute_with(|| {
        let due_epoch = u32::MAX / 100 - 1;
        SnapshotDeadline::<Test>::put(SnapshotProgress {
            last_snapshot_epoch: None,
            due_epoch,
        });
        assert!(!Welfare::snapshot_overdue(u32::MAX));
    });
}

#[test]
fn try_state_rejects_snapshot_progress_without_its_prior_snapshot() {
    new_test_ext().execute_with(|| {
        SnapshotDeadline::<Test>::put(SnapshotProgress {
            last_snapshot_epoch: Some(7),
            due_epoch: 8,
        });
        assert!(Welfare::do_try_state().is_err());
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
        Welfare::note_xcm_traffic(2, 0, XcmTrafficKind::Accepted);
        Welfare::note_xcm_traffic(2, u8::MAX, XcmTrafficKind::ProbeTimeout);
        Welfare::note_xcm_traffic(3, 0, XcmTrafficKind::SendFailed);
        // No snapshot/gate owns epoch 1: its traffic-only prefix must still reap.
        Welfare::note_xcm_traffic(1, 7, XcmTrafficKind::Accepted);

        assert_ok!(Welfare::prune(3));
        assert!(!Snapshots::<Test>::contains_key((2, 1)));
        assert!(!GateBreachFlags::<Test>::contains_key(2));
        assert!(!SampledGateDays::<Test>::contains_key(2));
        assert_eq!(XcmTraffic::<Test>::iter_prefix(2).count(), 0);
        assert_eq!(XcmTraffic::<Test>::iter_prefix(1).count(), 0);
        assert!(XcmTraffic::<Test>::contains_key(3, 0));
        assert_eq!(XcmTrafficEpochs::<Test>::get().into_inner(), vec![3]);
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
fn xcm_traffic_prune_is_bounded_and_oldest_first() {
    new_test_ext().execute_with(|| {
        for epoch in [7, 3, 5, 9, 1, 8] {
            Welfare::note_xcm_traffic(epoch, 0, XcmTrafficKind::Accepted);
            Welfare::note_xcm_traffic(epoch, u8::MAX, XcmTrafficKind::ProbeTimeout);
        }

        assert_ok!(Welfare::prune_xcm_traffic(8));

        assert_eq!(XcmTraffic::<Test>::iter_prefix(1).count(), 0);
        assert_eq!(XcmTraffic::<Test>::iter_prefix(3).count(), 0);
        for epoch in [5, 7, 8, 9] {
            assert_eq!(XcmTraffic::<Test>::iter_prefix(epoch).count(), 2);
        }
        assert_eq!(
            XcmTrafficEpochs::<Test>::get().into_inner(),
            vec![7, 5, 9, 8]
        );
    });
}

#[test]
fn xcm_traffic_recorder_saturates_each_counter() {
    new_test_ext().execute_with(|| {
        XcmTraffic::<Test>::insert(
            7,
            3,
            XcmTrafficCounters {
                accepted: u64::MAX,
                failed: u64::MAX,
                probe_timeouts: u64::MAX,
            },
        );

        Welfare::note_xcm_traffic(7, 3, XcmTrafficKind::Accepted);
        Welfare::note_xcm_traffic(7, 3, XcmTrafficKind::SendFailed);
        Welfare::note_xcm_traffic(7, 3, XcmTrafficKind::ProbeTimeout);

        assert_eq!(
            Welfare::xcm_traffic(7, 3),
            XcmTrafficCounters {
                accepted: u64::MAX,
                failed: u64::MAX,
                probe_timeouts: u64::MAX,
            }
        );
    });
}

#[test]
fn xcm_traffic_is_isolated_by_epoch_and_day() {
    new_test_ext().execute_with(|| {
        Welfare::note_xcm_traffic(7, 1, XcmTrafficKind::Accepted);
        Welfare::note_xcm_traffic(7, 2, XcmTrafficKind::SendFailed);
        Welfare::note_xcm_traffic(8, 1, XcmTrafficKind::ProbeTimeout);

        assert_eq!(
            Welfare::xcm_traffic(7, 1),
            XcmTrafficCounters {
                accepted: 1,
                failed: 0,
                probe_timeouts: 0,
            }
        );
        assert_eq!(
            Welfare::xcm_traffic(7, 2),
            XcmTrafficCounters {
                accepted: 0,
                failed: 1,
                probe_timeouts: 0,
            }
        );
        assert_eq!(
            Welfare::xcm_traffic(8, 1),
            XcmTrafficCounters {
                accepted: 0,
                failed: 0,
                probe_timeouts: 1,
            }
        );
        assert_eq!(Welfare::xcm_traffic(8, 2), XcmTrafficCounters::default());
    });
}

#[test]
fn xcm_traffic_epoch_sum_is_field_wise_and_saturating() {
    new_test_ext().execute_with(|| {
        XcmTraffic::<Test>::insert(
            7,
            0,
            XcmTrafficCounters {
                accepted: u64::MAX,
                failed: 1,
                probe_timeouts: 0,
            },
        );
        XcmTraffic::<Test>::insert(
            7,
            u8::MAX,
            XcmTrafficCounters {
                accepted: 1,
                failed: u64::MAX,
                probe_timeouts: u64::MAX,
            },
        );
        XcmTraffic::<Test>::insert(
            8,
            0,
            XcmTrafficCounters {
                accepted: 0,
                failed: 0,
                probe_timeouts: 1,
            },
        );

        assert_eq!(
            Welfare::xcm_traffic_epoch(7),
            XcmTrafficCounters {
                accepted: u64::MAX,
                failed: u64::MAX,
                probe_timeouts: u64::MAX,
            }
        );
    });
}

#[test]
fn xcm_traffic_recorder_is_infallible_across_epoch_and_day_boundaries() {
    new_test_ext().execute_with(|| {
        for epoch in [0, u32::MAX / 2, u32::MAX] {
            for day in u8::MIN..=u8::MAX {
                let kind = match day % 3 {
                    0 => XcmTrafficKind::Accepted,
                    1 => XcmTrafficKind::SendFailed,
                    _ => XcmTrafficKind::ProbeTimeout,
                };
                Welfare::note_xcm_traffic(epoch, day, kind);
            }
            let counters = Welfare::xcm_traffic_epoch(epoch);
            assert_eq!(
                counters.accepted + counters.failed + counters.probe_timeouts,
                256
            );
        }
    });
}

#[test]
fn xcm_traffic_recorder_drops_only_a_new_epoch_when_the_index_is_full() {
    new_test_ext().execute_with(|| {
        for epoch in 0..MAX_XCM_TRAFFIC_EPOCHS_BOUND {
            Welfare::note_xcm_traffic(epoch, 0, XcmTrafficKind::Accepted);
        }
        Welfare::note_xcm_traffic(MAX_XCM_TRAFFIC_EPOCHS_BOUND, 0, XcmTrafficKind::SendFailed);
        Welfare::note_xcm_traffic(0, 0, XcmTrafficKind::ProbeTimeout);

        assert_eq!(
            XcmTrafficEpochs::<Test>::get().len(),
            MAX_XCM_TRAFFIC_EPOCHS_BOUND as usize
        );
        assert!(!XcmTraffic::<Test>::contains_key(
            MAX_XCM_TRAFFIC_EPOCHS_BOUND,
            0
        ));
        assert_eq!(
            Welfare::xcm_traffic(0, 0),
            XcmTrafficCounters {
                accepted: 1,
                failed: 0,
                probe_timeouts: 1,
            }
        );
    });
}

#[test]
fn try_state_accepts_a_bounded_backlog_and_rejects_structural_corruption() {
    new_test_ext().execute_with(|| {
        CurrentEpochValue::set(30);
        // Bounded pruning may legitimately leave an old indexed prefix queued
        // for a later tick; age alone is no longer a try-state violation.
        XcmTraffic::<Test>::insert(
            9,
            0,
            XcmTrafficCounters {
                accepted: 1,
                ..Default::default()
            },
        );
        XcmTrafficEpochs::<Test>::put(BoundedVec::truncate_from(vec![9]));
        assert_ok!(Welfare::do_try_state());

        XcmTraffic::<Test>::remove(9, 0);
        XcmTrafficEpochs::<Test>::kill();
        XcmTraffic::<Test>::insert(
            31,
            0,
            XcmTrafficCounters {
                probe_timeouts: 1,
                ..Default::default()
            },
        );
        XcmTrafficEpochs::<Test>::put(BoundedVec::truncate_from(vec![31]));
        assert!(Welfare::do_try_state().is_err());

        XcmTraffic::<Test>::remove(31, 0);
        XcmTrafficEpochs::<Test>::kill();
        XcmTraffic::<Test>::insert(10, 0, XcmTrafficCounters::default());
        XcmTrafficEpochs::<Test>::put(BoundedVec::truncate_from(vec![10]));
        assert!(Welfare::do_try_state().is_err());

        XcmTraffic::<Test>::remove(10, 0);
        XcmTrafficEpochs::<Test>::kill();
        Welfare::note_xcm_traffic(30, u8::MAX, XcmTrafficKind::SendFailed);
        assert_ok!(Welfare::do_try_state());

        XcmTrafficEpochs::<Test>::kill();
        assert!(Welfare::do_try_state().is_err());

        XcmTraffic::<Test>::remove(30, u8::MAX);
        XcmTrafficEpochs::<Test>::put(BoundedVec::truncate_from(vec![30]));
        assert!(Welfare::do_try_state().is_err());

        XcmTrafficEpochs::<Test>::put(BoundedVec::truncate_from(vec![30, 30]));
        Welfare::note_xcm_traffic(30, 0, XcmTrafficKind::Accepted);
        assert!(Welfare::do_try_state().is_err());
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
        // SQ-79: the gate window must carry at least one observation per
        // measurement epoch before gate books may settle at all, so sample both
        // — this test is about ledger atomicity, not about the window rule.
        for epoch in [11, 12] {
            assert_ok!(Welfare::record_daily_gate(
                RuntimeOrigin::signed(keeper()),
                epoch,
                0,
                1,
            ));
        }
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

// -------------------------------- 03 §2.3/§5.2 epoch-VOID Baseline settlement
//
// 03 §5.2 (owning transition, normative): "Under an epoch VOID, the
// SettleAuthority settles the Baseline vault at `s = 0.5` … The settlement is
// mandatory and unconditional on that path … Implementations MUST treat 'no
// Baseline vault for the epoch' and 'already `Settled`' as no-ops rather than
// failures — a VOID must never fail on this leg (G-1)." 05 §7(5) names welfare
// as the sole SettleAuthority holder that performs it. SQ-92 regression.

#[test]
fn sq92_settle_baseline_void_settles_only_the_named_epoch_at_the_kernel_void_score() {
    // 03 §2.3 transition row `Baseline Open → Settled(s)` ("epoch VOID settles
    // at `s = 0.5`"): the score is the kernel constant, never a literal, and it
    // is the neutral midpoint of the 1e9 score scale (03 §5.2).
    assert_eq!(
        kernel::VOID_BASELINE_SCORE.0.saturating_mul(2),
        kernel::SCORE_SCALE
    );
    new_test_ext().execute_with(|| {
        LedgerCalls::set(Vec::new());

        assert_ok!(Welfare::settle_baseline_void(10));

        // Exactly one settlement, for exactly the voided epoch: the Baseline
        // vault is keyed per epoch, so a VOID of `e` may not touch `e ± 1`.
        assert_eq!(
            LedgerCalls::get(),
            vec![LedgerCall::Baseline(10, kernel::VOID_BASELINE_SCORE)]
        );
    });
}

#[test]
fn sq92_settle_baseline_void_reads_no_welfare_state_and_needs_no_snapshots() {
    // A VOID means no measurement is trusted (05 §7(4)), so the Baseline
    // settlement carries a spec-fixed constant rather than a computed score
    // (05 §7(5): "a terminal transition carrying a spec-fixed constant, not a
    // computation — which is exactly why it survives a VOID"). Pinned by
    // contrast: the scored path for the same epoch cannot run at all here.
    new_test_ext().execute_with(|| {
        assert_noop!(
            Welfare::compute_settlement(10, 1, SettleTarget::Baseline),
            Error::<Test>::MissingComponent
        );
        LedgerCalls::set(Vec::new());

        assert_ok!(Welfare::settle_baseline_void(10));

        assert_eq!(
            LedgerCalls::get(),
            vec![LedgerCall::Baseline(10, kernel::VOID_BASELINE_SCORE)]
        );
    });
}

#[test]
fn sq92_settle_baseline_void_is_a_silent_noop_when_no_baseline_vault_is_open() {
    // G-1 leg of 03 §5.2: both benign cases — the epoch never had a Baseline
    // vault, and the vault is already `Settled` — reach this seam as
    // `baseline_open == false` and MUST be no-ops rather than failures. The two
    // cases are distinguished where the distinction is real (ledger/runtime).
    new_test_ext().execute_with(|| {
        BaselineClosed::set(vec![10]);
        LedgerCalls::set(Vec::new());

        assert_ok!(Welfare::settle_baseline_void(10));

        assert!(LedgerCalls::get().is_empty());
        // The precondition is per epoch: a sibling epoch still settles.
        assert_ok!(Welfare::settle_baseline_void(11));
        assert_eq!(
            LedgerCalls::get(),
            vec![LedgerCall::Baseline(11, kernel::VOID_BASELINE_SCORE)]
        );
    });
}

#[test]
fn sq92_settle_baseline_void_propagates_a_real_ledger_failure() {
    // 03 §5.2 enumerates the no-op cases exhaustively; anything else is a
    // genuine failure and must not be swallowed. G-1 then makes the caller's
    // VOID fail closed rather than record a Void cohort over an `Open`
    // Baseline vault — the exact stranding state the spec forbids.
    new_test_ext().execute_with(|| {
        LedgerFailure::set(Some(LedgerCall::Baseline(10, kernel::VOID_BASELINE_SCORE)));
        LedgerCalls::set(Vec::new());

        assert_noop!(
            Welfare::settle_baseline_void(10),
            sp_runtime::DispatchError::Other("injected ledger failure")
        );

        assert!(LedgerCalls::get().is_empty());
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
        core.register_metric_spec(Registration::Genesis, 1, genesis_specs(1))
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
                    let core_result = core.register_metric_spec(
                        Registration::Live { current_epoch: now },
                        version,
                        specs.clone(),
                    );
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
                    // SQ-79: the gate leg now consults the core's window rule,
                    // so the oracle must too — a zero-sample window makes the
                    // whole proposal settlement fail, scalar leg included. The
                    // window is checked first so a refusal leaves the core's
                    // event log untouched, exactly as the shell's discarded
                    // working state does.
                    let core_result = match core.gate_window_outcomes(cohort) {
                        Ok(gates) => core
                            .compute_settlement(cohort, version)
                            .map(|score| (score, gates)),
                        Err(error) => Err(error),
                    };
                    if let Ok((score, (s_breached, c_breached))) = core_result {
                        expected_ledger_calls.extend([
                            LedgerCall::Scalar(pid, score),
                            LedgerCall::Gate(pid, GateKind::Survival, s_breached),
                            LedgerCall::Gate(pid, GateKind::Security, c_breached),
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

#[test]
fn rolling_window_with_the_runtime_prune_cutoff_never_jams() {
    // Regression for the 2026-07-17 re-review blocker: the runtime seam prunes
    // with cutoff = current − (MAX_SNAPSHOTS_BOUND − 1) — the 05 §3.3 "snapshot
    // e−20 and older" reading — which must always leave one free slot in the
    // 20-capacity window so the next epoch's snapshot records. A cutoff of
    // current − MAX_SNAPSHOTS_BOUND retains a full window and jams recording
    // permanently (settlement deadlock → dead-man; PLAN SQ-200).
    new_test_ext().execute_with(|| {
        // The mock genesis spec activates at epoch 1, so the first recordable
        // snapshot epoch is 1 (clock 2).
        for current in 2..=(3 * MAX_SNAPSHOTS_BOUND) {
            CurrentEpochValue::set(current);
            assert_ok!(Welfare::record_snapshot(
                RuntimeOrigin::signed(keeper()),
                current - 1,
                1,
            ));
            assert_ok!(Welfare::prune(
                current.saturating_sub(MAX_SNAPSHOTS_BOUND - 1)
            ));
        }
        // The window is at steady state: 19 retained + the slot just used.
        assert!(Snapshots::<Test>::iter().count() <= MAX_SNAPSHOTS);
    });
}

// ---------------------------------------------------------------- SQ-79
//
// 05 §4.7 owns the daily gate-breach flags; doc 07 §10 owns the fail-static
// disposition of an unavailable gate input ("if the failed component is a gate
// input, affected cohorts VOID"). A cohort whose whole e+1…e+2 measurement
// window carries **no** recorded daily observation has no gate input at all,
// and the pre-fix code settled its gate books at "no breach" — an
// adopt-favourable claim paid out of absent data (against G-1/R-7). The ruled
// disposition is to refuse: settlement holds at the status quo and the cohort
// takes the existing VoidAuthority path.
//
// *Partial* coverage is deliberately unclassified (05 §4.7 declares no
// expected-day count), so one sampled day per measurement epoch is enough —
// `sq79_one_sampled_day_per_epoch_is_enough` pins that boundary so a later
// completeness rule cannot be introduced by accident.

#[test]
fn sq79_a_wholly_unsampled_gate_window_refuses_settlement_instead_of_reading_no_breach() {
    new_test_ext().execute_with(|| {
        for epoch in [11, 12] {
            assert_ok!(Welfare::record_snapshot(
                RuntimeOrigin::signed(keeper()),
                epoch,
                1,
            ));
        }
        // No `record_daily_gate` for either measurement epoch at all.
        assert!(!GateBreachFlags::<Test>::contains_key(11));
        assert!(!GateBreachFlags::<Test>::contains_key(12));

        LedgerCalls::set(Vec::new());
        assert_noop!(
            Welfare::compute_settlement(
                10,
                1,
                SettleTarget::Proposal {
                    pid: 42,
                    has_gate_books: true,
                },
            ),
            Error::<Test>::GateWindowUnsampled
        );
        // Status quo: nothing settled, no scalar leg leaked out either.
        assert!(LedgerCalls::get().is_empty());
        assert!(!System::events().iter().any(|record| matches!(
            record.event,
            RuntimeEvent::Welfare(Event::SettlementComputed { .. })
        )));
    });
}

#[test]
fn sq79_a_half_sampled_window_is_still_an_unavailable_gate_input() {
    new_test_ext().execute_with(|| {
        for epoch in [11, 12] {
            assert_ok!(Welfare::record_snapshot(
                RuntimeOrigin::signed(keeper()),
                epoch,
                1,
            ));
        }
        // Only e+1 is observed; e+2 is a blank window.
        assert_ok!(Welfare::record_daily_gate(
            RuntimeOrigin::signed(keeper()),
            11,
            0,
            1,
        ));
        assert_noop!(
            Welfare::compute_settlement(
                10,
                1,
                SettleTarget::Proposal {
                    pid: 42,
                    has_gate_books: true,
                },
            ),
            Error::<Test>::GateWindowUnsampled
        );
    });
}

#[test]
fn sq79_one_sampled_day_per_epoch_is_enough() {
    // The ruling stops at zero-sample. Any completeness measure over the days
    // *within* an epoch would need a normative expected-day count, which 05 §4.7
    // does not declare — so a single healthy day per measurement epoch settles.
    new_test_ext().execute_with(|| {
        for epoch in [11, 12] {
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
                LedgerCall::Gate(42, GateKind::Survival, false),
                LedgerCall::Gate(42, GateKind::Security, false),
            ]
        );
    });
}

#[test]
fn sq79_refusal_is_scoped_to_gate_books_only() {
    // A gateless proposal and the Baseline book consume the scalar score, never
    // the §4.7 flags, so an unsampled window must not block them (G-1: refusing
    // more than the missing input would itself be a liveness failure).
    new_test_ext().execute_with(|| {
        for epoch in [11, 12] {
            assert_ok!(Welfare::record_snapshot(
                RuntimeOrigin::signed(keeper()),
                epoch,
                1,
            ));
        }
        LedgerCalls::set(Vec::new());
        assert_ok!(Welfare::compute_settlement(
            10,
            1,
            SettleTarget::Proposal {
                pid: 43,
                has_gate_books: false,
            },
        ));
        assert_ok!(Welfare::compute_settlement(10, 1, SettleTarget::Baseline));
        assert_eq!(
            LedgerCalls::get(),
            vec![
                LedgerCall::Scalar(43, FixedU64(ONE)),
                LedgerCall::Baseline(10, FixedU64(ONE)),
            ]
        );
    });
}

#[test]
fn sq79_view_path_keeps_the_frozen_permissive_default() {
    // 02 §4 `WelfareView.{s_breached, c_breached}` is a frozen display surface:
    // an unsampled current epoch must still render as "no breach so far" rather
    // than erroring. Only the settlement path changed.
    new_test_ext().execute_with(|| {
        let state = Welfare::welfare_state();
        let flags = state.gate_breach(11);
        assert!(!flags.s_breached && !flags.c_breached);
        assert!(!state.gate_window_sampled(11));
    });
}

// ---------------------------------------------------------------- SQ-82
//
// 05 §4.4/§4.6: the `>= current + 2` activation lead protects in-flight cohorts
// (I-16); genesis has none, so a genesis registration activates at epoch 1. The
// pre-fix core inferred "this is genesis" from the ambient `current_epoch == 0`,
// which cannot distinguish the genesis build from an unset/booting clock — so a
// *live* `register_spec` observed against a zero clock inherited the relaxation
// and could activate one epoch early. The context is now explicit.

#[test]
fn sq82_a_live_register_spec_at_a_zero_clock_does_not_inherit_the_genesis_relaxation() {
    new_test_ext().execute_with(|| {
        CurrentEpochValue::set(0);
        // The genesis relaxation would have admitted activation at epoch 1.
        assert_noop!(
            Welfare::register_spec(
                RuntimeOrigin::signed(governance_acc()),
                2,
                bounded(specs_activating(2, 1)),
            ),
            Error::<Test>::BadActivationEpoch
        );
        // The live `current + 2` lead is what actually binds.
        assert_ok!(Welfare::register_spec(
            RuntimeOrigin::signed(governance_acc()),
            2,
            bounded(specs_activating(2, 2)),
        ));
    });
}

#[test]
fn sq82_genesis_registration_still_activates_at_epoch_one() {
    // The relaxation itself is unchanged — 05 §4.6's cold start requires welfare
    // to be computable from epoch 1 — it is now reachable only from the genesis
    // build. The mock genesis registers `genesis_specs` (activation 1).
    new_test_ext().execute_with(|| {
        assert_eq!(Welfare::welfare_state().specs, vec![(1, genesis_specs(1))]);
    });
}

// --------------------------------------------------------------- SQ-201
//
// 05 §3.3 tied the full welfare prune to cohort reap, and `settle_cohort` is its
// only caller. An epoch that never forms a cohort is therefore unreachable by
// cohort-keyed cleanup: after MAX_SNAPSHOTS consecutive cohortless epochs
// `record_snapshot` jams at its hard bound and the §4.8 snapshot-overdue trigger
// fires — a deterministic chain wedge, not idle storage. The epoch-roll prune
// runs on every clock roll and applies the *same* §3.3 cutoff, so it retires
// nothing reap would have retained.

#[test]
fn sq201_cohortless_epochs_wedge_snapshot_recording_without_the_epoch_roll_prune() {
    // The pre-fix failure mode, pinned: with cohort reap never firing, the
    // 20-deep window fills and the 21st epoch cannot record.
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
        // The epoch-roll prune clears the jam with no cohort in sight.
        assert_ok!(Welfare::prune_epoch_roll(3));
        assert_ok!(Welfare::record_snapshot(
            RuntimeOrigin::signed(keeper()),
            MAX_SNAPSHOTS as u32 + 2,
            1,
        ));
    });
}

#[test]
fn sq201_epoch_roll_prune_is_bounded_and_retires_oldest_first() {
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
        // A cutoff far above the window: only EPOCH_ROLL_PRUNE_MAX_EPOCHS go per
        // call, oldest first, so a backlog is spread across ticks (I-20).
        assert_ok!(Welfare::prune_epoch_roll(MAX_SNAPSHOTS as u32 + 2));
        assert_eq!(
            Snapshots::<Test>::iter().count(),
            MAX_SNAPSHOTS - EPOCH_ROLL_PRUNE_MAX_EPOCHS
        );
        for epoch in 2..2 + EPOCH_ROLL_PRUNE_MAX_EPOCHS as u32 {
            assert!(!Snapshots::<Test>::contains_key((epoch, 1)));
            assert!(!GateBreachFlags::<Test>::contains_key(epoch));
            assert!(!SampledGateDays::<Test>::contains_key(epoch));
        }
        assert!(Snapshots::<Test>::contains_key((
            2 + EPOCH_ROLL_PRUNE_MAX_EPOCHS as u32,
            1
        )));
    });
}

#[test]
fn sq201_epoch_roll_prune_never_retires_inside_the_retained_window() {
    // It must be impossible for the roll prune to remove state the reap-driven
    // prune would have kept: both take the same 05 §3.3 cutoff.
    new_test_ext().execute_with(|| {
        for epoch in 2..MAX_SNAPSHOTS as u32 + 2 {
            assert_ok!(Welfare::record_snapshot(
                RuntimeOrigin::signed(keeper()),
                epoch,
                1,
            ));
        }
        let before = Snapshots::<Test>::iter().count();
        assert_ok!(Welfare::prune_epoch_roll(2));
        assert_eq!(Snapshots::<Test>::iter().count(), before);
    });
}

#[test]
fn sq201_epoch_roll_prune_protects_the_snapshot_deadline_binding() {
    // do_try_state binds SnapshotDeadline.last_snapshot_epoch to a live
    // snapshot; maintenance must not be able to break that binding even if a
    // caller supplies an absurd cutoff.
    new_test_ext().execute_with(|| {
        for epoch in 1..4 {
            assert_ok!(Welfare::record_snapshot(
                RuntimeOrigin::signed(keeper()),
                epoch,
                1,
            ));
        }
        let last = SnapshotDeadline::<Test>::get()
            .and_then(|progress| progress.last_snapshot_epoch)
            .expect("snapshot progress advanced");
        assert_ok!(Welfare::prune_epoch_roll(u32::MAX));
        assert!(Snapshots::<Test>::contains_key((last, 1)));
        assert_ok!(Welfare::do_try_state());
    });
}

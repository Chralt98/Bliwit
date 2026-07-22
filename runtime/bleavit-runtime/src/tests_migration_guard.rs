//! B16 — migration stall predicate, MBM lockdown, the `MigrationHalted`
//! diagnostic, and this runtime's first storage migration (SQ-132, SQ-146,
//! SQ-309). Owning spec: 09 §3.1–§3.2. These live in their own file per the
//! standing instruction that `tests.rs` not grow concurrently.

use alloc::vec;

use frame_support::{
    migrations::{FailedMigrationHandler, SteppedMigrations},
    storage::unhashed,
    traits::{GetStorageVersion, OnRuntimeUpgrade, StorageVersion},
    BoundedVec,
};
use futarchy_primitives::kernel;
use parity_scale_codec::MaxEncodedLen;

use crate::{tests, Constitution, ExecutionGuard, Oracle, Runtime, RuntimeEvent, System};

/// SQ-132(d): "stalled" is a pure function of the SDK cursor's own `started_at`,
/// never of the cursor bytes failing to change. The retired byte-equality
/// predicate false-raised on a lawful migration that drains a map for hours
/// while returning byte-identical cursors; this one cannot.
#[test]
fn stall_is_derived_from_started_at_not_cursor_bytes() {
    tests::development_ext().execute_with(|| {
        System::set_block_number(10_000);
        // Bytes are identical across every case below; only `started_at` varies.
        let at = |started_at| pallet_migrations::ActiveCursor {
            index: 3,
            inner_cursor: Some(BoundedVec::truncate_from(vec![7u8, 7, 7])),
            started_at,
        };
        let now = System::block_number();

        // A cursor that has only just started is never stalled, regardless of
        // how long an identical-byte cursor may have existed before it.
        assert!(!crate::configs::active_migration_stall_is_live(&at(now)));

        // Exactly at the budget (now - started_at == MIGRATION_STALL_BLOCKS): the
        // predicate is strict `>`, so this is NOT yet stalled.
        assert!(!crate::configs::active_migration_stall_is_live(&at(
            now - kernel::MIGRATION_STALL_BLOCKS
        )));

        // One block past the budget: stalled.
        assert!(crate::configs::active_migration_stall_is_live(&at(now
            - kernel::MIGRATION_STALL_BLOCKS
            - 1)));
    });
}

/// SQ-309 / SQ-132(d)(ii)/(iii): the MBM lockdown is a *build-time* integrity
/// test, not a convention. Until the inherent-only recovery lane ships, no
/// multi-block migration may be registered at all; when one eventually is, every
/// segment must declare a bounded budget strictly under the stall block and the
/// aggregate must be bounded too. Gated to the production `Migrations` config
/// (benchmarking swaps in `MockedMigrations`).
#[cfg(not(feature = "runtime-benchmarks"))]
#[test]
fn registered_mbms_obey_the_b16_lockdown() {
    type Migrations = <Runtime as pallet_migrations::Config>::Migrations;

    // The prohibition (SQ-309). A stuck cursor forces `OnlyInherents` and there
    // is no on-chain escape yet, so the only safe number of registered MBMs is
    // zero. Production wires `type Migrations = ()`.
    assert_eq!(
        Migrations::len(),
        0,
        "B16 lockdown: no multi-block migration may be registered until the \
         inherent-only recovery lane (SQ-309) ships",
    );

    // Forward-looking, vacuous while `len() == 0` (09 §3.2(d)(ii)/(iii)).
    let mut aggregate: u64 = 0;
    for i in 0..Migrations::len() {
        let max = Migrations::nth_max_steps(i)
            .expect("index < len is in range")
            .expect("every registered MBM must declare max_steps = Some(n) (09 §3.2(d)(ii))");
        assert!(
            max < kernel::MIGRATION_STALL_BLOCKS,
            "each MBM max_steps must be strictly < MIGRATION_STALL_BLOCKS",
        );
        aggregate = aggregate.saturating_add(u64::from(max));
    }
    assert!(
        aggregate < u64::from(kernel::MIGRATION_STALL_BLOCKS),
        "the aggregate MBM run budget must be strictly < MIGRATION_STALL_BLOCKS (09 §3.2(d)(iii))",
    );
}

/// 09 §3.2(4) requires the diagnostic to carry the SDK cursor's exact bytes.
/// `CursorMaxLen` bounds only the migration-owned inner cursor, so the event
/// envelope must also cover the outer enum/index/option/length/block fields.
#[test]
fn migration_halted_cursor_bound_covers_the_full_sdk_cursor_encoding() {
    let sdk_max = <pallet_migrations::CursorOf<Runtime> as MaxEncodedLen>::max_encoded_len();
    assert!(
        sdk_max <= pallet_execution_guard::MAX_MIGRATION_HALT_CURSOR_BOUND as usize,
        "MigrationHalted cursor bound must cover the full encoded SDK cursor: {sdk_max} > {}",
        pallet_execution_guard::MAX_MIGRATION_HALT_CURSOR_BOUND,
    );
}

/// SQ-132 + SQ-146: the first storage migration retires both inert items and
/// advances `pallet-execution-guard`'s storage version 0 -> 1, and its version
/// gate makes a re-run a no-op. It creates no `pallet-migrations` cursor, so it
/// never engages the `OnlyInherents` lockdown.
#[test]
fn first_migration_retires_both_keys_bumps_version_and_is_idempotent() {
    tests::development_ext().execute_with(|| {
        // Model a pre-B16 upgraded chain: version 0, both keys populated.
        StorageVersion::new(0).put::<ExecutionGuard>();
        let bm = crate::migrations::retired::blocked_meters_key();
        let pm = crate::migrations::retired::progress_marker_key();
        // Cross-check each retired key against a *live* sibling that shares its
        // prefix, so a future pallet/instance rename cannot silently point the
        // migration at a non-existent key and orphan the real bytes (SQ-66 class).
        assert_eq!(
            bm[..16],
            pallet_execution_guard::HardGateBreach::<Runtime>::hashed_key()[..16],
            "BlockedMeters must share the live ExecutionGuard pallet prefix",
        );
        assert_eq!(
            pm[..16],
            crate::configs::MigrationHaltSources::hashed_key()[..16],
            "ProgressMarker must share the live BleavitRuntimeMigration instance prefix",
        );
        unhashed::put_raw(&bm, &[1u8, 2, 3]);
        unhashed::put_raw(&pm, &[4u8, 5, 6, 7]);
        assert!(unhashed::exists(&bm) && unhashed::exists(&pm));

        let _weight = <crate::migrations::RetireB16State as OnRuntimeUpgrade>::on_runtime_upgrade();

        assert!(!unhashed::exists(&bm), "BlockedMeters key retired");
        assert!(
            !unhashed::exists(&pm),
            "MigrationProgressMarker key retired"
        );
        assert_eq!(
            ExecutionGuard::on_chain_storage_version(),
            StorageVersion::new(1),
            "execution-guard storage version advanced 0 -> 1",
        );
        assert!(
            !pallet_migrations::Cursor::<Runtime>::exists(),
            "a single-block migration creates no MBM cursor (09 §3.2)",
        );

        // Re-run on the already-migrated chain: version 1 != FROM(0) -> no-op, so
        // re-seeded state is left untouched (the version gate, SQ-66 discipline).
        unhashed::put_raw(&bm, &[9u8]);
        let _ = <crate::migrations::RetireB16State as OnRuntimeUpgrade>::on_runtime_upgrade();
        assert!(
            unhashed::exists(&bm),
            "a version-gated re-run is a no-op and clears nothing",
        );
    });
}

#[test]
fn oracle_v1_migration_retires_legacy_query_and_reconciles_both_health_mirrors() {
    for unhealthy in [false, true] {
        tests::development_ext().execute_with(|| {
            StorageVersion::new(0).put::<Oracle>();
            let legacy = pallet_oracle::ReserveHealthValue {
                consecutive_fails: 17,
                consecutive_passes: 19,
                unhealthy,
                last_query_id: 77,
                last_probe_at: 91,
                pending_since: Some(83),
            };
            pallet_oracle::ReserveHealth::<Runtime>::put(legacy);
            pallet_oracle::ReserveProbeArmed::<Runtime>::put(true);
            pallet_constitution::PhaseFlags::<Runtime>::mutate(|bits| {
                if unhealthy {
                    *bits &= !pallet_constitution::PhaseFlagsValue::RESERVE_HEALTH_FLAG;
                } else {
                    *bits |= pallet_constitution::PhaseFlagsValue::RESERVE_HEALTH_FLAG;
                }
            });
            pallet_futarchy_treasury::State::<Runtime>::mutate(|state| {
                state.reserve_impaired = !unhealthy;
            });

            let _ = <crate::migrations::MigrateOracleReserveProbeV1 as OnRuntimeUpgrade>::on_runtime_upgrade();

            assert_eq!(StorageVersion::get::<Oracle>(), StorageVersion::new(1));
            assert_eq!(
                pallet_oracle::ReserveHealth::<Runtime>::get(),
                pallet_oracle::ReserveHealthValue {
                    last_query_id: 0,
                    last_probe_at: 0,
                    pending_since: None,
                    ..legacy
                }
            );
            assert!(!pallet_oracle::ReserveProbeArmed::<Runtime>::get());
            assert_eq!(
                pallet_constitution::PhaseFlags::<Runtime>::get()
                    & pallet_constitution::PhaseFlagsValue::RESERVE_HEALTH_FLAG
                    != 0,
                unhealthy
            );
            assert_eq!(
                pallet_futarchy_treasury::State::<Runtime>::get().reserve_impaired,
                unhealthy
            );

            let snapshot = (
                pallet_oracle::ReserveHealth::<Runtime>::get(),
                pallet_oracle::ReserveProbeArmed::<Runtime>::get(),
                pallet_constitution::PhaseFlags::<Runtime>::get(),
                pallet_futarchy_treasury::State::<Runtime>::get(),
            );
            let _ = <crate::migrations::MigrateOracleReserveProbeV1 as OnRuntimeUpgrade>::on_runtime_upgrade();
            assert_eq!(
                snapshot,
                (
                    pallet_oracle::ReserveHealth::<Runtime>::get(),
                    pallet_oracle::ReserveProbeArmed::<Runtime>::get(),
                    pallet_constitution::PhaseFlags::<Runtime>::get(),
                    pallet_futarchy_treasury::State::<Runtime>::get(),
                )
            );
        });
    }
}

#[cfg(feature = "try-runtime")]
#[test]
fn oracle_v1_try_runtime_proves_the_full_legacy_transition() {
    tests::development_ext().execute_with(|| {
        StorageVersion::new(0).put::<Oracle>();
        let legacy = pallet_oracle::ReserveHealthValue {
            consecutive_fails: 7,
            consecutive_passes: 11,
            unhealthy: true,
            last_query_id: 99,
            last_probe_at: 123,
            pending_since: Some(117),
        };
        pallet_oracle::ReserveHealth::<Runtime>::put(legacy);
        pallet_oracle::ReserveProbeArmed::<Runtime>::put(true);
        pallet_constitution::PhaseFlags::<Runtime>::mutate(|bits| {
            *bits &= !pallet_constitution::PhaseFlagsValue::RESERVE_HEALTH_FLAG;
        });
        pallet_futarchy_treasury::State::<Runtime>::mutate(|state| {
            state.reserve_impaired = false;
        });

        let state =
            <crate::migrations::MigrateOracleReserveProbeV1 as OnRuntimeUpgrade>::pre_upgrade()
                .expect("oracle pre-upgrade snapshot");
        let _ = <crate::migrations::MigrateOracleReserveProbeV1 as OnRuntimeUpgrade>::on_runtime_upgrade();
        <crate::migrations::MigrateOracleReserveProbeV1 as OnRuntimeUpgrade>::post_upgrade(state)
            .expect("oracle post-upgrade checks");

        assert_eq!(StorageVersion::get::<Oracle>(), StorageVersion::new(1));
        assert_eq!(
            pallet_oracle::ReserveHealth::<Runtime>::get(),
            pallet_oracle::ReserveHealthValue {
                last_query_id: 0,
                last_probe_at: 0,
                pending_since: None,
                ..legacy
            }
        );
        assert!(!pallet_oracle::ReserveProbeArmed::<Runtime>::get());
    });
}

fn unrelated_constitution_params() -> Vec<(
    futarchy_primitives::ParamKey,
    pallet_constitution::ParamRecord,
)> {
    let fee = pallet_constitution::key16(b"ops.probe_fee");
    let rate = pallet_constitution::key16(b"ops.probe_rate");
    pallet_constitution::Params::<Runtime>::iter()
        .filter(|(key, _)| *key != fee && *key != rate)
        .collect()
}

#[test]
fn constitution_v1_migration_inserts_only_missing_exact_probe_rows() {
    tests::development_ext().execute_with(|| {
        StorageVersion::new(0).put::<Constitution>();
        let (fee, rate) = crate::migrations::reserve_probe_param_records()
            .expect("probe rows exist in the registry template");
        pallet_constitution::Params::<Runtime>::remove(fee.key);
        pallet_constitution::Params::<Runtime>::remove(rate.key);
        let unrelated_before = unrelated_constitution_params();

        let _ = <crate::migrations::MigrateConstitutionProbeParamsV1 as OnRuntimeUpgrade>::on_runtime_upgrade();

        assert_eq!(StorageVersion::get::<Constitution>(), StorageVersion::new(1));
        assert_eq!(pallet_constitution::Params::<Runtime>::get(fee.key), Some(fee));
        assert_eq!(pallet_constitution::Params::<Runtime>::get(rate.key), Some(rate));
        assert_eq!(unrelated_constitution_params(), unrelated_before);
    });
}

#[test]
fn constitution_v1_migration_preserves_exact_existing_rows_and_advances_version() {
    tests::development_ext().execute_with(|| {
        StorageVersion::new(0).put::<Constitution>();
        let (fee, rate) = crate::migrations::reserve_probe_param_records()
            .expect("probe rows exist in the registry template");

        let _ = <crate::migrations::MigrateConstitutionProbeParamsV1 as OnRuntimeUpgrade>::on_runtime_upgrade();

        assert_eq!(StorageVersion::get::<Constitution>(), StorageVersion::new(1));
        assert_eq!(pallet_constitution::Params::<Runtime>::get(fee.key), Some(fee));
        assert_eq!(pallet_constitution::Params::<Runtime>::get(rate.key), Some(rate));
    });
}

#[test]
fn constitution_v1_migration_is_atomic_on_one_mismatched_existing_row() {
    tests::development_ext().execute_with(|| {
        StorageVersion::new(0).put::<Constitution>();
        let (fee, mut rate) = crate::migrations::reserve_probe_param_records()
            .expect("probe rows exist in the registry template");
        pallet_constitution::Params::<Runtime>::remove(fee.key);
        rate.last_change_block = 123;
        pallet_constitution::Params::<Runtime>::insert(rate.key, rate);
        let unrelated_before = unrelated_constitution_params();

        let _ = <crate::migrations::MigrateConstitutionProbeParamsV1 as OnRuntimeUpgrade>::on_runtime_upgrade();

        assert_eq!(StorageVersion::get::<Constitution>(), StorageVersion::new(0));
        assert!(pallet_constitution::Params::<Runtime>::get(fee.key).is_none());
        assert_eq!(pallet_constitution::Params::<Runtime>::get(rate.key), Some(rate));
        assert_eq!(unrelated_constitution_params(), unrelated_before);
    });
}

#[cfg(feature = "try-runtime")]
#[test]
fn constitution_v1_try_runtime_rejects_a_mismatched_existing_v0_row() {
    tests::development_ext().execute_with(|| {
        StorageVersion::new(0).put::<Constitution>();
        let (_, mut rate) = crate::migrations::reserve_probe_param_records()
            .expect("probe rows exist in the registry template");
        rate.last_change_block = rate.last_change_block.saturating_add(1);
        pallet_constitution::Params::<Runtime>::insert(rate.key, rate);

        assert!(
            <crate::migrations::MigrateConstitutionProbeParamsV1 as OnRuntimeUpgrade>::pre_upgrade(
            )
            .is_err(),
            "try-runtime must block a release over a mismatched v0 probe row",
        );
        assert_eq!(
            StorageVersion::get::<Constitution>(),
            StorageVersion::new(0)
        );
    });
}

#[cfg(feature = "try-runtime")]
#[test]
fn constitution_v1_try_runtime_noop_preserves_lawfully_amended_values() {
    tests::development_ext().execute_with(|| {
        let (fee, mut rate) = crate::migrations::reserve_probe_param_records()
            .expect("probe rows exist in the registry template");
        rate.value = pallet_constitution::ParamValue::Balance(
            rate.value.as_u128().saturating_add(1),
        );
        pallet_constitution::Params::<Runtime>::insert(rate.key, rate);
        let state = <crate::migrations::MigrateConstitutionProbeParamsV1 as OnRuntimeUpgrade>::pre_upgrade()
            .expect("current-version pre-upgrade is a lawful no-op");
        let _ = <crate::migrations::MigrateConstitutionProbeParamsV1 as OnRuntimeUpgrade>::on_runtime_upgrade();
        <crate::migrations::MigrateConstitutionProbeParamsV1 as OnRuntimeUpgrade>::post_upgrade(state)
            .expect("lawfully amended rows survive the no-op");
        assert_eq!(pallet_constitution::Params::<Runtime>::get(fee.key), Some(fee));
        assert_eq!(pallet_constitution::Params::<Runtime>::get(rate.key), Some(rate));
    });
}

#[cfg(feature = "try-runtime")]
#[test]
fn constitution_v1_try_runtime_inserts_absent_rows() {
    tests::development_ext().execute_with(|| {
        StorageVersion::new(0).put::<Constitution>();
        let (fee, rate) = crate::migrations::reserve_probe_param_records()
            .expect("probe rows exist in the registry template");
        pallet_constitution::Params::<Runtime>::remove(fee.key);
        pallet_constitution::Params::<Runtime>::remove(rate.key);

        let state = <crate::migrations::MigrateConstitutionProbeParamsV1 as OnRuntimeUpgrade>::pre_upgrade()
            .expect("absent v0 rows are migratable");
        let _ = <crate::migrations::MigrateConstitutionProbeParamsV1 as OnRuntimeUpgrade>::on_runtime_upgrade();
        <crate::migrations::MigrateConstitutionProbeParamsV1 as OnRuntimeUpgrade>::post_upgrade(state)
            .expect("inserted v1 rows satisfy post-upgrade checks");
    });
}

#[cfg(feature = "try-runtime")]
#[test]
fn composed_runtime_upgrade_migrates_all_reserve_probe_v0_state_and_passes_try_state() {
    use frame_support::traits::TryState;
    use parity_scale_codec::{Decode, Encode};

    tests::development_ext().execute_with(|| {
        StorageVersion::new(0).put::<ExecutionGuard>();
        let blocked_meters = crate::migrations::retired::blocked_meters_key();
        let progress_marker = crate::migrations::retired::progress_marker_key();
        unhashed::put_raw(&blocked_meters, &[1u8]);
        unhashed::put_raw(&progress_marker, &[2u8]);

        StorageVersion::new(0).put::<Constitution>();
        let (fee, rate) = crate::migrations::reserve_probe_param_records()
            .expect("probe rows exist in the registry template");
        pallet_constitution::Params::<Runtime>::remove(fee.key);
        pallet_constitution::Params::<Runtime>::remove(rate.key);
        pallet_constitution::PhaseFlags::<Runtime>::mutate(|bits| {
            *bits &= !pallet_constitution::PhaseFlagsValue::TREASURY_ARMED;
            *bits &= !pallet_constitution::PhaseFlagsValue::RESERVE_HEALTH_FLAG;
        });

        StorageVersion::new(0).put::<Oracle>();
        let legacy = pallet_oracle::ReserveHealthValue {
            consecutive_fails: 5,
            consecutive_passes: 3,
            unhealthy: true,
            last_query_id: 71,
            last_probe_at: 81,
            pending_since: Some(79),
        };
        pallet_oracle::ReserveHealth::<Runtime>::put(legacy);
        pallet_oracle::ReserveProbeArmed::<Runtime>::put(true);
        pallet_futarchy_treasury::State::<Runtime>::mutate(|state| {
            state.reserve_impaired = false;
        });

        StorageVersion::new(0).put::<crate::FutarchyTreasury>();
        pallet_futarchy_treasury::BootstrapOpsFundingClosed::<Runtime>::put(true);

        // This is the real Executive path: System executes the production
        // SingleBlockMigrations tuple, pallet hooks execute the treasury v1
        // migration, and try-runtime runs every registered pre/post check.
        let input = frame_try_runtime::UpgradeCheckSelect::All.encode();
        let output = crate::apis::api::dispatch("TryRuntime_on_runtime_upgrade", &input)
            .expect("TryRuntime runtime API method");
        let (used, maximum) = <(
            frame_support::weights::Weight,
            frame_support::weights::Weight,
        ) as Decode>::decode(&mut &output[..])
        .expect("TryRuntime result");
        assert!(used.all_lte(maximum));

        assert_eq!(
            StorageVersion::get::<ExecutionGuard>(),
            StorageVersion::new(1)
        );
        assert!(!unhashed::exists(&blocked_meters));
        assert!(!unhashed::exists(&progress_marker));
        assert_eq!(
            StorageVersion::get::<Constitution>(),
            StorageVersion::new(1)
        );
        assert_eq!(
            pallet_constitution::Params::<Runtime>::get(fee.key),
            Some(fee)
        );
        assert_eq!(
            pallet_constitution::Params::<Runtime>::get(rate.key),
            Some(rate)
        );
        assert_eq!(StorageVersion::get::<Oracle>(), StorageVersion::new(1));
        assert_eq!(
            pallet_oracle::ReserveHealth::<Runtime>::get(),
            pallet_oracle::ReserveHealthValue {
                last_query_id: 0,
                last_probe_at: 0,
                pending_since: None,
                ..legacy
            }
        );
        assert!(!pallet_oracle::ReserveProbeArmed::<Runtime>::get());
        assert_ne!(
            pallet_constitution::PhaseFlags::<Runtime>::get()
                & pallet_constitution::PhaseFlagsValue::RESERVE_HEALTH_FLAG,
            0,
        );
        assert!(pallet_futarchy_treasury::State::<Runtime>::get().reserve_impaired);
        assert_eq!(
            StorageVersion::get::<crate::FutarchyTreasury>(),
            StorageVersion::new(1),
        );
        assert!(!pallet_futarchy_treasury::BootstrapOpsFundingClosed::<
            Runtime,
        >::get());
        assert!(
            <crate::AllPalletsWithSystem as TryState<crate::BlockNumber>>::try_state(
                System::block_number(),
                frame_try_runtime::TryStateSelect::All,
            )
            .is_ok()
        );
    });
}

/// The migration's try-runtime hooks prove the before/after transition: pre
/// records both keys present, post asserts both absent. Compiles only under
/// `try-runtime` (the hooks are `#[cfg(feature = \"try-runtime\")]`).
#[cfg(feature = "try-runtime")]
#[test]
fn first_migration_pre_and_post_upgrade_prove_the_transition() {
    use frame_support::traits::UncheckedOnRuntimeUpgrade;

    tests::development_ext().execute_with(|| {
        let bm = crate::migrations::retired::blocked_meters_key();
        let pm = crate::migrations::retired::progress_marker_key();
        unhashed::put_raw(&bm, &[1u8, 2, 3]);
        unhashed::put_raw(&pm, &[4u8]);

        let recorded = crate::migrations::RetireB16StateInner::pre_upgrade()
            .expect("pre_upgrade records present state");
        assert_eq!(
            recorded,
            vec![1u8, 1u8],
            "pre_upgrade sees both keys present"
        );

        let _ = crate::migrations::RetireB16StateInner::on_runtime_upgrade();

        crate::migrations::RetireB16StateInner::post_upgrade(recorded)
            .expect("post_upgrade proves both keys absent");
    });
}

/// 09 §3.2(4): the `MigrationHalted` diagnostic is emitted exactly once, on the
/// first activation of an execution-halt source, carrying the SDK failed step.
/// A second activation while the halt already stands does not re-emit.
#[test]
fn migration_halted_diagnostic_emits_once_on_first_activation() {
    tests::development_ext().execute_with(|| {
        System::set_block_number(1);

        let count_halted = || {
            System::events()
                .into_iter()
                .filter(|record| {
                    matches!(
                        record.event,
                        RuntimeEvent::ExecutionGuard(
                            pallet_execution_guard::Event::MigrationHalted { .. }
                        )
                    )
                })
                .count()
        };
        assert_eq!(count_halted(), 0);

        // First activation of the failure source emits the diagnostic.
        assert_eq!(
            crate::configs::MigrationFailureToGuard::failed(Some(5)),
            frame_support::migrations::FailedMigrationHandling::KeepStuck,
        );

        let emitted = System::events()
            .into_iter()
            .find_map(|record| match record.event {
                RuntimeEvent::ExecutionGuard(pallet_execution_guard::Event::MigrationHalted {
                    cursor,
                    failed_step,
                }) => Some((cursor, failed_step)),
                _ => None,
            });
        let (cursor, failed_step) = emitted.expect("a MigrationHalted diagnostic was emitted");
        assert_eq!(failed_step, Some(5), "carries the SDK-reported failed step");
        assert!(
            cursor.is_empty(),
            "no active cursor in this scenario -> empty diagnostic cursor bytes",
        );
        assert_eq!(count_halted(), 1);

        // A second source activation while already halted does NOT re-emit.
        let _ = crate::configs::MigrationFailureToGuard::failed(Some(6));
        assert_eq!(
            count_halted(),
            1,
            "first-activation-only: no re-emit while the halt still stands",
        );
    });
}

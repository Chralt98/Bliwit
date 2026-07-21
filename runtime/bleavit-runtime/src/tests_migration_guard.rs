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

use crate::{tests, ExecutionGuard, Runtime, RuntimeEvent, System};

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

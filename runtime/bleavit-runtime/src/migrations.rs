//! Storage migrations wired into `frame_system::Config::SingleBlockMigrations`.
//!
//! B16 ships this runtime's **first** storage migration. It retires two inert
//! storage items in one versioned step, gated on `pallet-execution-guard`'s
//! storage version advancing `0 -> 1`:
//!
//! * `pallet-execution-guard`'s `BlockedMeters` (SQ-146 — retired as inert; it
//!   never had a production writer, only tests),
//! * the runtime-level `BleavitRuntimeMigration::ProgressMarker` and its cursor
//!   hash (SQ-132 — the stall predicate now reads the SDK `cursor.started_at`
//!   directly, per 09 §3.2(d)(i), so the marker is gone).
//!
//! On any real chain both keys are already absent: this runtime is pre-genesis,
//! neither key is written by a genesis preset, and neither has a live production
//! writer (the marker only wrote while an MBM cursor existed, and production
//! wires `type Migrations = ()`). The migration still `clear`s both keys — an
//! `O(1)` idempotent no-op on an empty chain, the correct cleanup on any state
//! that ever held them, and it establishes the migration discipline this runtime
//! had not previously exercised. Shipping the `StorageVersion` bump together with
//! its data step is the whole point (SQ-66: a bump without its migration bricked
//! upgraded state).
//!
//! Pattern source: `frame_support::migrations::VersionedMigration` +
//! `frame_support::traits::UncheckedOnRuntimeUpgrade` (frame-support 42.0.0,
//! stable2606), the canonical single-block versioned-migration wrapper.

use crate::Runtime;
#[cfg(any(feature = "phase-four", feature = "recovery"))]
use frame_support::dispatch::DispatchResult;
use frame_support::{
    migrations::VersionedMigration,
    traits::{OnRuntimeUpgrade, UncheckedOnRuntimeUpgrade},
    weights::Weight,
};

#[cfg(feature = "try-runtime")]
use alloc::vec::Vec;
#[cfg(feature = "try-runtime")]
use parity_scale_codec::{Decode, Encode};

/// Raw 32-byte keys of the retired `StorageValue`s, computed from their frozen
/// prefixes so the migration is self-contained and does not depend on the type
/// definitions it removes. `pub(crate)` so the B16 regression test seeds exactly
/// the keys this migration clears.
pub(crate) mod retired {
    use sp_io::hashing::twox_128;

    fn value_key(pallet: &[u8], item: &[u8]) -> [u8; 32] {
        let mut key = [0u8; 32];
        key[..16].copy_from_slice(&twox_128(pallet));
        key[16..].copy_from_slice(&twox_128(item));
        key
    }

    /// `ExecutionGuard::BlockedMeters` — retired `StorageValue` (SQ-146).
    pub(crate) fn blocked_meters_key() -> [u8; 32] {
        value_key(b"ExecutionGuard", b"BlockedMeters")
    }

    /// Runtime-level `BleavitRuntimeMigration::ProgressMarker` — retired stall
    /// progress marker + cursor hash (SQ-132).
    pub(crate) fn progress_marker_key() -> [u8; 32] {
        value_key(b"BleavitRuntimeMigration", b"ProgressMarker")
    }
}

/// Inner (unversioned) migration wrapped by [`RetireB16State`]. Performs no
/// version check itself — that is `VersionedMigration`'s job.
pub struct RetireB16StateInner;

impl UncheckedOnRuntimeUpgrade for RetireB16StateInner {
    fn on_runtime_upgrade() -> Weight {
        sp_io::storage::clear(&retired::blocked_meters_key());
        sp_io::storage::clear(&retired::progress_marker_key());
        // Two existence reads + two clears. No unbounded state, no host
        // storage-root pass (09 §3.2(2) forbids that): fixed, benchmark-free.
        <Runtime as frame_system::Config>::DbWeight::get().reads_writes(2, 2)
    }

    #[cfg(feature = "try-runtime")]
    fn pre_upgrade() -> Result<Vec<u8>, sp_runtime::TryRuntimeError> {
        // Record presence so an operator reading the try-runtime log can see the
        // before-state. On a real chain both are absent; a try-runtime harness
        // may have seeded them (the dedicated runtime test does exactly that).
        let mut present = Vec::with_capacity(2);
        present.push(u8::from(sp_io::storage::exists(
            &retired::blocked_meters_key(),
        )));
        present.push(u8::from(sp_io::storage::exists(
            &retired::progress_marker_key(),
        )));
        Ok(present)
    }

    #[cfg(feature = "try-runtime")]
    fn post_upgrade(_state: Vec<u8>) -> Result<(), sp_runtime::TryRuntimeError> {
        frame_support::ensure!(
            !sp_io::storage::exists(&retired::blocked_meters_key()),
            "B16 migration: ExecutionGuard::BlockedMeters key still present after retirement"
        );
        frame_support::ensure!(
            !sp_io::storage::exists(&retired::progress_marker_key()),
            "B16 migration: BleavitRuntimeMigration::ProgressMarker key still present after retirement"
        );
        Ok(())
    }
}

/// The versioned migration wired into `SingleBlockMigrations`. It runs iff
/// `pallet-execution-guard`'s on-chain storage version is `0`, executes the
/// inner retirement, then advances the on-chain version to `1`. Re-running on an
/// already-migrated chain is a logged no-op (`VersionedMigration` guarantees it),
/// which is why the retirement is safe to leave wired.
pub type RetireB16State = VersionedMigration<
    0,
    1,
    RetireB16StateInner,
    crate::ExecutionGuard,
    <Runtime as frame_system::Config>::DbWeight,
>;

/// v0 reserve-probe state could contain a query id/pending attempt created
/// while production's dispatcher and response sink were both inert. Retire
/// that impossible response identity and old cadence anchor, leave the new
/// readiness latch unarmed, and reconcile both health mirrors to the preserved
/// adverse `unhealthy` value. Direct bounded storage writes make this migration
/// infallible and avoid pretending the dispatch-time fallible sink is an
/// upgrade-safe API.
pub struct MigrateOracleReserveProbeV1Inner;

impl UncheckedOnRuntimeUpgrade for MigrateOracleReserveProbeV1Inner {
    fn on_runtime_upgrade() -> Weight {
        let mut health = pallet_oracle::ReserveHealth::<Runtime>::get();
        let unhealthy = health.unhealthy;
        health.last_query_id = 0;
        health.pending_since = None;
        health.last_probe_at = 0;
        pallet_oracle::ReserveHealth::<Runtime>::put(health);
        pallet_oracle::ReserveProbeArmed::<Runtime>::kill();

        pallet_constitution::PhaseFlags::<Runtime>::mutate(|bits| {
            if unhealthy {
                *bits |= pallet_constitution::PhaseFlagsValue::RESERVE_HEALTH_FLAG;
            } else {
                *bits &= !pallet_constitution::PhaseFlagsValue::RESERVE_HEALTH_FLAG;
            }
        });
        pallet_futarchy_treasury::State::<Runtime>::mutate(|state| {
            state.reserve_impaired = unhealthy;
        });

        // ReserveHealth, PhaseFlags and Treasury State reads; those three
        // writes plus the explicit latch clear. VersionedMigration separately
        // accounts for its StorageVersion read/write.
        <Runtime as frame_system::Config>::DbWeight::get().reads_writes(3, 4)
    }

    #[cfg(feature = "try-runtime")]
    fn pre_upgrade() -> Result<Vec<u8>, sp_runtime::TryRuntimeError> {
        Ok((
            pallet_oracle::ReserveHealth::<Runtime>::get(),
            pallet_constitution::PhaseFlags::<Runtime>::get(),
            pallet_futarchy_treasury::State::<Runtime>::get().reserve_impaired,
        )
            .encode())
    }

    #[cfg(feature = "try-runtime")]
    fn post_upgrade(state: Vec<u8>) -> Result<(), sp_runtime::TryRuntimeError> {
        let (mut expected, _, _): (pallet_oracle::ReserveHealthValue, u32, bool) =
            Decode::decode(&mut &state[..]).map_err(|_| {
                sp_runtime::TryRuntimeError::Other("oracle v1 migration: invalid pre-upgrade state")
            })?;
        expected.last_query_id = 0;
        expected.pending_since = None;
        expected.last_probe_at = 0;
        let unhealthy = expected.unhealthy;
        frame_support::ensure!(
            pallet_oracle::ReserveHealth::<Runtime>::get() == expected,
            "oracle v1 migration: health changed beyond legacy query/cadence retirement"
        );
        frame_support::ensure!(
            !pallet_oracle::ReserveProbeArmed::<Runtime>::get(),
            "oracle v1 migration: legacy state incorrectly implied arming"
        );
        frame_support::ensure!(
            (pallet_constitution::PhaseFlags::<Runtime>::get()
                & pallet_constitution::PhaseFlagsValue::RESERVE_HEALTH_FLAG
                != 0)
                == unhealthy,
            "oracle v1 migration: constitution health mirror diverges"
        );
        frame_support::ensure!(
            pallet_futarchy_treasury::State::<Runtime>::get().reserve_impaired == unhealthy,
            "oracle v1 migration: treasury health mirror diverges"
        );
        Ok(())
    }
}

pub type MigrateOracleReserveProbeV1 = VersionedMigration<
    0,
    1,
    MigrateOracleReserveProbeV1Inner,
    crate::Oracle,
    <Runtime as frame_system::Config>::DbWeight,
>;

pub(crate) fn reserve_probe_param_records() -> Option<(
    pallet_constitution::ParamRecord,
    pallet_constitution::ParamRecord,
)> {
    let fee_key = pallet_constitution::key16(b"ops.probe_fee");
    let rate_key = pallet_constitution::key16(b"ops.probe_rate");
    let mut fee = None;
    let mut rate = None;
    for record in pallet_constitution::genesis_params() {
        if record.key == fee_key {
            fee = Some(record);
        } else if record.key == rate_key {
            rate = Some(record);
        }
    }
    fee.zip(rate)
}

fn valid_probe_record(
    expected_key: futarchy_primitives::ParamKey,
    record: &pallet_constitution::ParamRecord,
) -> bool {
    record.key == expected_key
        && record.value.same_kind(record.min)
        && record.value.same_kind(record.max)
        && record.min.as_u128() <= record.value.as_u128()
        && record.value.as_u128() <= record.max.as_u128()
        && record.cooldown_epochs <= pallet_constitution::META_MAX_COOLDOWN_EPOCHS
        && record.admissible_next_interval().is_ok()
}

/// Newest-main v0 state predates the two reserve-probe pricing records, while
/// `Params` is otherwise genesis-fixed. Insert the exact registry definitions
/// iff absent. A mismatched pre-existing row is fail-closed: do not overwrite
/// it and do not advance the storage version, so the migration remains visibly
/// incomplete and try-runtime rejects the release.
pub struct MigrateConstitutionProbeParamsV1;

impl OnRuntimeUpgrade for MigrateConstitutionProbeParamsV1 {
    fn on_runtime_upgrade() -> Weight {
        use frame_support::traits::{GetStorageVersion, StorageVersion};

        if crate::Constitution::on_chain_storage_version() != StorageVersion::new(0) {
            return <Runtime as frame_system::Config>::DbWeight::get().reads(1);
        }
        let Some((fee, rate)) = reserve_probe_param_records() else {
            return <Runtime as frame_system::Config>::DbWeight::get().reads(1);
        };
        let fee_before = pallet_constitution::Params::<Runtime>::get(fee.key);
        let rate_before = pallet_constitution::Params::<Runtime>::get(rate.key);
        if fee_before.as_ref().is_some_and(|record| record != &fee)
            || rate_before.as_ref().is_some_and(|record| record != &rate)
        {
            return <Runtime as frame_system::Config>::DbWeight::get().reads(3);
        }
        if fee_before.is_none() {
            pallet_constitution::Params::<Runtime>::insert(fee.key, fee);
        }
        if rate_before.is_none() {
            pallet_constitution::Params::<Runtime>::insert(rate.key, rate);
        }
        StorageVersion::new(1).put::<crate::Constitution>();

        // Worst case: version + two row reads + two counted-map counter reads;
        // two row/counter writes and the version write.
        <Runtime as frame_system::Config>::DbWeight::get().reads_writes(7, 5)
    }

    #[cfg(feature = "try-runtime")]
    fn pre_upgrade() -> Result<Vec<u8>, sp_runtime::TryRuntimeError> {
        use frame_support::traits::{GetStorageVersion, StorageVersion};

        let (fee, rate) =
            reserve_probe_param_records().ok_or(sp_runtime::TryRuntimeError::Other(
                "constitution v1 migration: registry definitions are absent",
            ))?;
        let version = crate::Constitution::on_chain_storage_version();
        let fee_before = pallet_constitution::Params::<Runtime>::get(fee.key);
        let rate_before = pallet_constitution::Params::<Runtime>::get(rate.key);
        let needs_migration = version == StorageVersion::new(0);
        if needs_migration {
            frame_support::ensure!(
                fee_before.as_ref().is_none_or(|record| record == &fee)
                    && rate_before.as_ref().is_none_or(|record| record == &rate),
                "constitution v1 migration: mismatched pre-existing probe record"
            );
        } else {
            frame_support::ensure!(
                fee_before.is_some() && rate_before.is_some(),
                "constitution v1 migration: current-version probe row absent"
            );
        }
        Ok((needs_migration, version, fee_before, rate_before, fee, rate).encode())
    }

    #[cfg(feature = "try-runtime")]
    fn post_upgrade(state: Vec<u8>) -> Result<(), sp_runtime::TryRuntimeError> {
        use frame_support::traits::{GetStorageVersion, StorageVersion};

        let (needs_migration, version_before, fee_before, rate_before, fee, rate): (
            bool,
            StorageVersion,
            Option<pallet_constitution::ParamRecord>,
            Option<pallet_constitution::ParamRecord>,
            pallet_constitution::ParamRecord,
            pallet_constitution::ParamRecord,
        ) = Decode::decode(&mut &state[..]).map_err(|_| {
            sp_runtime::TryRuntimeError::Other(
                "constitution v1 migration: invalid pre-upgrade state",
            )
        })?;
        if needs_migration {
            frame_support::ensure!(
                crate::Constitution::on_chain_storage_version() == StorageVersion::new(1),
                "constitution v1 migration: storage version was not advanced"
            );
            frame_support::ensure!(
                pallet_constitution::Params::<Runtime>::get(fee.key) == Some(fee)
                    && pallet_constitution::Params::<Runtime>::get(rate.key) == Some(rate),
                "constitution v1 migration: probe rows differ from registry definitions"
            );
        } else {
            frame_support::ensure!(
                crate::Constitution::on_chain_storage_version() == version_before,
                "constitution v1 migration: no-op changed storage version"
            );
            frame_support::ensure!(
                pallet_constitution::Params::<Runtime>::get(fee.key) == fee_before
                    && pallet_constitution::Params::<Runtime>::get(rate.key) == rate_before,
                "constitution v1 migration: no-op changed lawful live records"
            );
        }
        let fee_after = pallet_constitution::Params::<Runtime>::get(fee.key);
        let rate_after = pallet_constitution::Params::<Runtime>::get(rate.key);
        frame_support::ensure!(
            fee_after
                .as_ref()
                .is_some_and(|record| valid_probe_record(fee.key, record))
                && rate_after
                    .as_ref()
                    .is_some_and(|record| valid_probe_record(rate.key, record)),
            "constitution v1 migration: post-upgrade probe row is absent or invalid"
        );
        Ok(())
    }
}

pub(crate) fn reserve_probe_control_param_records() -> Option<[pallet_constitution::ParamRecord; 5]>
{
    let params = pallet_constitution::genesis_params();
    let find = |name: &[u8]| {
        let key = pallet_constitution::key16(name);
        params.iter().find(|record| record.key == key).copied()
    };
    Some([
        find(b"res.probe_int")?,
        find(b"res.probe_to")?,
        find(b"res.probe_amount")?,
        find(b"res.fail_thr")?,
        find(b"res.recover_thr")?,
    ])
}

fn migrate_probe_control_record(
    live: pallet_constitution::ParamRecord,
    expected: pallet_constitution::ParamRecord,
) -> Option<pallet_constitution::ParamRecord> {
    if live.key != expected.key
        || !live.value.same_kind(expected.value)
        || live.value.as_u128() < expected.min.as_u128()
        || live.value.as_u128() > expected.max.as_u128()
    {
        return None;
    }
    Some(pallet_constitution::ParamRecord {
        min: expected.min,
        max: expected.max,
        max_delta: expected.max_delta,
        cooldown_epochs: expected.cooldown_epochs,
        class: expected.class,
        kernel_bounded: expected.kernel_bounded,
        ..live
    })
}

/// Constitution v1 admitted zero-valued reserve-probe controls and allowed a
/// later registry amendment to restore those zero minima. Validate all five
/// live rows before writing anything, preserve their governed value/history,
/// then install the exact v2 structural metadata. A malformed or zero live row
/// leaves both data and storage version untouched (R-7).
pub struct MigrateConstitutionProbeControlsV2;

impl OnRuntimeUpgrade for MigrateConstitutionProbeControlsV2 {
    fn on_runtime_upgrade() -> Weight {
        use frame_support::traits::{GetStorageVersion, StorageVersion};

        if crate::Constitution::on_chain_storage_version() != StorageVersion::new(1) {
            return <Runtime as frame_system::Config>::DbWeight::get().reads(1);
        }
        let Some(expected) = reserve_probe_control_param_records() else {
            return <Runtime as frame_system::Config>::DbWeight::get().reads(1);
        };
        let mut migrated = expected;
        for (index, definition) in expected.into_iter().enumerate() {
            let Some(live) = pallet_constitution::Params::<Runtime>::get(definition.key) else {
                return <Runtime as frame_system::Config>::DbWeight::get().reads(6);
            };
            let Some(next) = migrate_probe_control_record(live, definition) else {
                return <Runtime as frame_system::Config>::DbWeight::get().reads(6);
            };
            migrated[index] = next;
        }
        for record in migrated {
            pallet_constitution::Params::<Runtime>::insert(record.key, record);
        }
        StorageVersion::new(2).put::<crate::Constitution>();
        // Version + five row reads, then `CountedStorageMap::insert` performs
        // one existence read per replacement; five row writes + version.
        <Runtime as frame_system::Config>::DbWeight::get().reads_writes(11, 6)
    }

    #[cfg(feature = "try-runtime")]
    fn pre_upgrade() -> Result<Vec<u8>, sp_runtime::TryRuntimeError> {
        use frame_support::traits::{GetStorageVersion, StorageVersion};

        let version = crate::Constitution::on_chain_storage_version();
        let expected =
            reserve_probe_control_param_records().ok_or(sp_runtime::TryRuntimeError::Other(
                "constitution v2 migration: registry definitions are absent",
            ))?;
        let mut before = expected;
        for (index, definition) in expected.into_iter().enumerate() {
            let live = pallet_constitution::Params::<Runtime>::get(definition.key).ok_or(
                sp_runtime::TryRuntimeError::Other(
                    "constitution v2 migration: reserve control row is absent",
                ),
            )?;
            if version == StorageVersion::new(1) {
                frame_support::ensure!(
                    migrate_probe_control_record(live, definition).is_some(),
                    "constitution v2 migration: reserve control row is not migratable"
                );
            }
            before[index] = live;
        }
        Ok((version, before, expected).encode())
    }

    #[cfg(feature = "try-runtime")]
    fn post_upgrade(state: Vec<u8>) -> Result<(), sp_runtime::TryRuntimeError> {
        use frame_support::traits::{GetStorageVersion, StorageVersion};

        let (version_before, before, expected): (
            StorageVersion,
            [pallet_constitution::ParamRecord; 5],
            [pallet_constitution::ParamRecord; 5],
        ) = Decode::decode(&mut &state[..]).map_err(|_| {
            sp_runtime::TryRuntimeError::Other(
                "constitution v2 migration: invalid pre-upgrade state",
            )
        })?;
        if version_before == StorageVersion::new(1) {
            frame_support::ensure!(
                crate::Constitution::on_chain_storage_version() == StorageVersion::new(2),
                "constitution v2 migration: storage version was not advanced"
            );
            for (index, definition) in expected.into_iter().enumerate() {
                let live = pallet_constitution::Params::<Runtime>::get(definition.key).ok_or(
                    sp_runtime::TryRuntimeError::Other(
                        "constitution v2 migration: migrated row is absent",
                    ),
                )?;
                let wanted = migrate_probe_control_record(before[index], definition).ok_or(
                    sp_runtime::TryRuntimeError::Other(
                        "constitution v2 migration: pre-upgrade row became invalid",
                    ),
                )?;
                frame_support::ensure!(
                    live == wanted,
                    "constitution v2 migration: value/history or metadata diverged"
                );
            }
        } else {
            frame_support::ensure!(
                crate::Constitution::on_chain_storage_version() == version_before,
                "constitution v2 migration: no-op changed storage version"
            );
            for record in before {
                frame_support::ensure!(
                    pallet_constitution::Params::<Runtime>::get(record.key) == Some(record),
                    "constitution v2 migration: no-op changed a control row"
                );
            }
        }
        Ok(())
    }
}

/// Atomic Constitution migration for the complete reserve-probe parameter
/// family. Predecessor v0 may lack the two pricing rows; v1 already has them.
/// Both paths validate the pricing and five control records before performing
/// any write, then advance directly to v2. This prevents a corrupt v0 control
/// row from leaving a partially advanced v1 state.
pub struct MigrateConstitutionReserveProbeV2;

impl OnRuntimeUpgrade for MigrateConstitutionReserveProbeV2 {
    fn on_runtime_upgrade() -> Weight {
        use frame_support::traits::{GetStorageVersion, StorageVersion};

        let version = crate::Constitution::on_chain_storage_version();
        if version != StorageVersion::new(0) && version != StorageVersion::new(1) {
            return <Runtime as frame_system::Config>::DbWeight::get().reads(1);
        }
        let Some((fee, rate)) = reserve_probe_param_records() else {
            return <Runtime as frame_system::Config>::DbWeight::get().reads(1);
        };
        let Some(controls) = reserve_probe_control_param_records() else {
            return <Runtime as frame_system::Config>::DbWeight::get().reads(1);
        };
        let fee_before = pallet_constitution::Params::<Runtime>::get(fee.key);
        let rate_before = pallet_constitution::Params::<Runtime>::get(rate.key);
        let pricing_valid = if version == StorageVersion::new(0) {
            fee_before.as_ref().is_none_or(|record| record == &fee)
                && rate_before.as_ref().is_none_or(|record| record == &rate)
        } else {
            fee_before
                .as_ref()
                .is_some_and(|record| valid_probe_record(fee.key, record))
                && rate_before
                    .as_ref()
                    .is_some_and(|record| valid_probe_record(rate.key, record))
        };
        if !pricing_valid {
            return <Runtime as frame_system::Config>::DbWeight::get().reads(8);
        }
        let mut migrated = controls;
        for (index, definition) in controls.into_iter().enumerate() {
            let Some(live) = pallet_constitution::Params::<Runtime>::get(definition.key) else {
                return <Runtime as frame_system::Config>::DbWeight::get().reads(8);
            };
            let Some(next) = migrate_probe_control_record(live, definition) else {
                return <Runtime as frame_system::Config>::DbWeight::get().reads(8);
            };
            migrated[index] = next;
        }

        if fee_before.is_none() {
            pallet_constitution::Params::<Runtime>::insert(fee.key, fee);
        }
        if rate_before.is_none() {
            pallet_constitution::Params::<Runtime>::insert(rate.key, rate);
        }
        for record in migrated {
            pallet_constitution::Params::<Runtime>::insert(record.key, record);
        }
        StorageVersion::new(2).put::<crate::Constitution>();
        // Version + seven row reads; up to seven CountedStorageMap existence
        // reads. Worst writes: seven rows, two new-row counters, and version.
        <Runtime as frame_system::Config>::DbWeight::get().reads_writes(15, 10)
    }

    #[cfg(feature = "try-runtime")]
    fn pre_upgrade() -> Result<Vec<u8>, sp_runtime::TryRuntimeError> {
        use frame_support::traits::{GetStorageVersion, StorageVersion};

        let version = crate::Constitution::on_chain_storage_version();
        let (fee, rate) =
            reserve_probe_param_records().ok_or(sp_runtime::TryRuntimeError::Other(
                "constitution v2 migration: pricing definitions are absent",
            ))?;
        let controls =
            reserve_probe_control_param_records().ok_or(sp_runtime::TryRuntimeError::Other(
                "constitution v2 migration: control definitions are absent",
            ))?;
        let fee_before = pallet_constitution::Params::<Runtime>::get(fee.key);
        let rate_before = pallet_constitution::Params::<Runtime>::get(rate.key);
        if version == StorageVersion::new(0) {
            frame_support::ensure!(
                fee_before.as_ref().is_none_or(|record| record == &fee)
                    && rate_before.as_ref().is_none_or(|record| record == &rate),
                "constitution v2 migration: mismatched v0 pricing row"
            );
        } else if version == StorageVersion::new(1) {
            frame_support::ensure!(
                fee_before
                    .as_ref()
                    .is_some_and(|record| valid_probe_record(fee.key, record))
                    && rate_before
                        .as_ref()
                        .is_some_and(|record| valid_probe_record(rate.key, record)),
                "constitution v2 migration: absent or invalid v1 pricing row"
            );
        }
        let mut controls_before = controls;
        for (index, definition) in controls.into_iter().enumerate() {
            let live = pallet_constitution::Params::<Runtime>::get(definition.key).ok_or(
                sp_runtime::TryRuntimeError::Other(
                    "constitution v2 migration: reserve control row is absent",
                ),
            )?;
            if version == StorageVersion::new(0) || version == StorageVersion::new(1) {
                frame_support::ensure!(
                    migrate_probe_control_record(live, definition).is_some(),
                    "constitution v2 migration: reserve control row is not migratable"
                );
            }
            controls_before[index] = live;
        }
        Ok((
            version,
            fee_before,
            rate_before,
            controls_before,
            fee,
            rate,
            controls,
        )
            .encode())
    }

    #[cfg(feature = "try-runtime")]
    fn post_upgrade(state: Vec<u8>) -> Result<(), sp_runtime::TryRuntimeError> {
        use frame_support::traits::{GetStorageVersion, StorageVersion};

        let (version, fee_before, rate_before, controls_before, fee, rate, controls): (
            StorageVersion,
            Option<pallet_constitution::ParamRecord>,
            Option<pallet_constitution::ParamRecord>,
            [pallet_constitution::ParamRecord; 5],
            pallet_constitution::ParamRecord,
            pallet_constitution::ParamRecord,
            [pallet_constitution::ParamRecord; 5],
        ) = Decode::decode(&mut &state[..]).map_err(|_| {
            sp_runtime::TryRuntimeError::Other(
                "constitution v2 migration: invalid composite pre-upgrade state",
            )
        })?;
        if version == StorageVersion::new(0) || version == StorageVersion::new(1) {
            frame_support::ensure!(
                crate::Constitution::on_chain_storage_version() == StorageVersion::new(2),
                "constitution v2 migration: storage version was not advanced"
            );
            frame_support::ensure!(
                pallet_constitution::Params::<Runtime>::get(fee.key)
                    == Some(fee_before.unwrap_or(fee))
                    && pallet_constitution::Params::<Runtime>::get(rate.key)
                        == Some(rate_before.unwrap_or(rate)),
                "constitution v2 migration: pricing rows were not preserved/inserted"
            );
            for (index, definition) in controls.into_iter().enumerate() {
                let wanted = migrate_probe_control_record(controls_before[index], definition)
                    .ok_or(sp_runtime::TryRuntimeError::Other(
                        "constitution v2 migration: pre-upgrade control became invalid",
                    ))?;
                frame_support::ensure!(
                    pallet_constitution::Params::<Runtime>::get(definition.key) == Some(wanted),
                    "constitution v2 migration: control value/history or metadata diverged"
                );
            }
        } else {
            frame_support::ensure!(
                crate::Constitution::on_chain_storage_version() == version,
                "constitution v2 migration: no-op changed storage version"
            );
        }
        Ok(())
    }
}
#[cfg(any(feature = "phase-four", feature = "recovery"))]
fn apply_phase_four_plan(plan: pallet_execution_guard::PhaseFourPlan) -> DispatchResult {
    let origin: crate::RuntimeOrigin = pallet_origins::Origin::FutarchyMeta.into();
    crate::Constitution::set_param(
        origin.clone(),
        pallet_constitution::key16(b"phase3.tvl_cap"),
        pallet_constitution::ParamValue::Balance(plan.tvl_cap),
    )?;
    crate::Constitution::set_param(
        origin,
        pallet_constitution::key16(b"phase3.dep_cap"),
        pallet_constitution::ParamValue::Balance(plan.deposit_cap),
    )
}

#[cfg(any(feature = "phase-four", feature = "recovery"))]
fn transition_phase_four(plan: pallet_execution_guard::PhaseFourPlan) -> DispatchResult {
    apply_phase_four_plan(plan)?;
    sp_io::storage::clear(&sudo_key_storage_key());
    pallet_constitution::PhaseFlags::<Runtime>::put(
        pallet_constitution::PhaseFlagsValue::PARAM_ARMED,
    );
    Ok(())
}

/// Phase-3→4 one-shot transition carried only by the `phase-four` runtime
/// profile. It purges the retired Sudo key at its frozen pallet index and arms
/// PARAM binding atomically with the code image that omits pallet-sudo.
#[cfg(feature = "phase-four")]
pub struct PhaseFourTransition;

#[cfg(any(feature = "phase-four", feature = "recovery"))]
fn sudo_key_storage_key() -> [u8; 32] {
    let mut key = [0u8; 32];
    key[..16].copy_from_slice(&sp_io::hashing::twox_128(b"Sudo"));
    key[16..].copy_from_slice(&sp_io::hashing::twox_128(b"Key"));
    key
}

#[allow(dead_code)]
pub(crate) fn phase_four_transition_weight() -> Weight {
    // `authorize_phase_four` benchmarks the same maximal bridge/queue,
    // preimage, attestation, parameter and release-channel footprint. Its
    // generated artifact is regression-gated; use that conservative envelope
    // for the one-shot mandatory transition too.
    <crate::weights::pallet_execution_guard::WeightInfo<Runtime> as pallet_execution_guard::WeightInfo>::authorize_phase_four()
}

#[allow(dead_code)]
pub(crate) fn terminal_recovery_transition_weight() -> Weight {
    // Terminal recovery consumes the phase bridge plus the committed recovery
    // record. Both components are generated from executable benchmarks and
    // regression-gated; their sum is the conservative mandatory envelope.
    <crate::weights::pallet_execution_guard::WeightInfo<Runtime> as pallet_execution_guard::WeightInfo>::authorize_phase_four()
        .saturating_add(
            <crate::weights::pallet_execution_guard::WeightInfo<Runtime> as pallet_execution_guard::WeightInfo>::commit_recovery_image(),
        )
}

#[cfg(feature = "phase-four")]
impl frame_support::traits::OnRuntimeUpgrade for PhaseFourTransition {
    fn on_runtime_upgrade() -> Weight {
        let bridge = pallet_execution_guard::PhaseFourBridge::<Runtime>::get();
        let plan = match bridge {
            pallet_execution_guard::PhaseFourBridgeState::Scheduled { plan, .. } => Some(plan),
            _ => None,
        };
        let bridge_scheduled = plan.is_some();
        let source_flags = pallet_constitution::PhaseFlagsValue::SHADOW_MODE
            | pallet_constitution::PhaseFlagsValue::SUDO_PRESENT;
        let source_state_exact = crate::configs::PhaseTransitionApplied::get()
            && crate::configs::PhaseTransitionLock::get()
            && pallet_constitution::PhaseFlags::<Runtime>::get() == source_flags
            && sp_io::storage::exists(&sudo_key_storage_key());

        if let Some(plan) = plan.filter(|_| source_state_exact) {
            let transitioned = frame_support::storage::with_storage_layer(|| {
                transition_phase_four(plan)?;
                crate::ExecutionGuard::validation_code_applied()?;
                crate::configs::PhaseTransitionApplied::kill();
                crate::configs::PhaseTransitionLock::kill();
                Ok::<(), sp_runtime::DispatchError>(())
            });
            if transitioned.is_err() {
                crate::configs::note_phase_transition_failure();
            }
        } else if bridge_scheduled
            || crate::configs::PhaseTransitionApplied::get()
            || crate::configs::PhaseTransitionLock::get()
        {
            // The new code is already installed, so status quo means keeping
            // the old key/flags and OnlyInherents lock intact. Operators must
            // recover with the separately committed terminal image.
            crate::configs::note_phase_transition_failure();
        }

        // Fixed upper envelope for the bounded transition: bridge/pending/core
        // records, release channel, one raw Sudo key, flags, and lock markers.
        // B16's benchmark/PoV gate measures this path and pins the envelope.
        phase_four_transition_weight()
    }

    #[cfg(feature = "try-runtime")]
    fn pre_upgrade() -> Result<Vec<u8>, sp_runtime::TryRuntimeError> {
        let transition_pending = matches!(
            pallet_execution_guard::PhaseFourBridge::<Runtime>::get(),
            pallet_execution_guard::PhaseFourBridgeState::Scheduled { .. }
        );
        if transition_pending {
            let source_flags = pallet_constitution::PhaseFlagsValue::SHADOW_MODE
                | pallet_constitution::PhaseFlagsValue::SUDO_PRESENT;
            frame_support::ensure!(
                crate::configs::PhaseTransitionApplied::get()
                    && crate::configs::PhaseTransitionLock::get()
                    && pallet_constitution::PhaseFlags::<Runtime>::get() == source_flags
                    && sp_io::storage::exists(&sudo_key_storage_key()),
                "phase-four migration: source Phase-3 state is not exact"
            );
        }
        Ok(alloc::vec![u8::from(transition_pending)])
    }

    #[cfg(feature = "try-runtime")]
    fn post_upgrade(state: Vec<u8>) -> Result<(), sp_runtime::TryRuntimeError> {
        let transition_pending = state.as_slice() == [1];
        if transition_pending {
            frame_support::ensure!(
                !sp_io::storage::exists(&sudo_key_storage_key()),
                "phase-four migration: Sudo key survived pallet removal"
            );
            frame_support::ensure!(
                pallet_constitution::PhaseFlags::<Runtime>::get()
                    == pallet_constitution::PhaseFlagsValue::PARAM_ARMED,
                "phase-four migration: exact PARAM-only flags not installed"
            );
            frame_support::ensure!(
                matches!(
                    pallet_execution_guard::PhaseFourBridge::<Runtime>::get(),
                    pallet_execution_guard::PhaseFourBridgeState::Consumed
                ) && !crate::configs::PhaseTransitionApplied::get()
                    && !crate::configs::PhaseTransitionLock::get(),
                "phase-four migration: bridge was not consumed atomically"
            );
        } else {
            frame_support::ensure!(
                !crate::configs::PhaseTransitionApplied::get(),
                "phase-four migration: application marker leaked onto a later image"
            );
        }
        Ok(())
    }
}

/// One-shot terminal transition carried only by a `recovery` runtime profile.
/// The old runtime's Cumulus callback proves that the committed recovery code
/// was installed; this new image then performs the bounded repair and consumes
/// every recovery lock atomically before ordinary extrinsics can resume.
#[cfg(feature = "recovery")]
pub struct TerminalRecoveryTransition;

/// Release-specific cutpoint repair for an ordinary failed MBM. The current
/// runtime registers no production MBMs, so there is no truthful generic
/// rollback to perform. A future MBM-bearing primary MUST replace this body in
/// its paired N+2 recovery profile with a bounded, cutpoint-total repair and
/// the corresponding exhaustive tests. Returning an error here is deliberate:
/// it preserves `OnlyInherents` rather than reopening transactions against a
/// half-migrated layout.
#[cfg(feature = "recovery")]
fn repair_retired_mbm(
    _cursor: &pallet_migrations::CursorOf<Runtime>,
) -> Result<(), sp_runtime::DispatchError> {
    Err(sp_runtime::DispatchError::Other(
        "ordinary MBM recovery repair is not configured",
    ))
}

#[cfg(feature = "recovery")]
impl frame_support::traits::OnRuntimeUpgrade for TerminalRecoveryTransition {
    fn on_runtime_upgrade() -> Weight {
        let completed = frame_support::storage::with_storage_layer(|| {
            frame_support::ensure!(
                crate::configs::RecoveryCodeApplied::get()
                    && crate::configs::RecoveryLockdown::get()
                    && !crate::configs::RecoveryAborted::get(),
                sp_runtime::DispatchError::Other("terminal recovery marker missing")
            );
            let scheduled = crate::configs::RecoveryScheduledHash::get().ok_or(
                sp_runtime::DispatchError::Other("terminal recovery schedule missing"),
            )?;
            let recovery = pallet_execution_guard::RecoveryImage::<Runtime>::get().ok_or(
                sp_runtime::DispatchError::Other("terminal recovery commitment missing"),
            )?;
            frame_support::ensure!(
                recovery.hash == scheduled,
                sp_runtime::DispatchError::Other("terminal recovery hash mismatch")
            );
            let (installed_hash, installed_version) = crate::configs::installed_code_identity()
                .ok_or(sp_runtime::DispatchError::Other(
                    "terminal recovery code unreadable",
                ))?;
            frame_support::ensure!(
                installed_hash == recovery.hash
                    && installed_version.spec_version == recovery.target_spec_version,
                sp_runtime::DispatchError::Other("terminal recovery code mismatch")
            );

            if crate::configs::PhaseTransitionLock::get() {
                frame_support::ensure!(
                    !crate::configs::RetiredMigrationCursor::exists(),
                    sp_runtime::DispatchError::Other(
                        "terminal recovery has conflicting phase and MBM causes"
                    )
                );
                let plan = match pallet_execution_guard::PhaseFourBridge::<Runtime>::get() {
                    pallet_execution_guard::PhaseFourBridgeState::Scheduled {
                        pid,
                        code_hash,
                        plan,
                    } if pid == recovery.pid && code_hash == recovery.primary_hash => plan,
                    _ => {
                        return Err(sp_runtime::DispatchError::Other(
                            "terminal recovery phase commitment missing",
                        ))
                    }
                };
                transition_phase_four(plan)?;
            } else {
                let cursor = crate::configs::RetiredMigrationCursor::get().ok_or(
                    sp_runtime::DispatchError::Other("terminal recovery cause missing"),
                )?;
                repair_retired_mbm(&cursor)?;
            }

            crate::ExecutionGuard::recovery_code_applied(installed_hash, installed_version)?;
            crate::configs::complete_terminal_recovery_state();
            Ok::<(), sp_runtime::DispatchError>(())
        });
        if completed.is_err() {
            crate::configs::note_phase_transition_failure();
        }

        // The terminal image has no MBM. Its single bounded transition covers
        // two parameter writes, guard/channel finalization, one preimage
        // unrequest, and the fixed runtime lock set.
        terminal_recovery_transition_weight()
    }

    #[cfg(feature = "try-runtime")]
    fn pre_upgrade() -> Result<Vec<u8>, sp_runtime::TryRuntimeError> {
        frame_support::ensure!(
            crate::configs::RecoveryCodeApplied::get()
                && crate::configs::RecoveryLockdown::get()
                && !crate::configs::RecoveryAborted::get()
                && crate::configs::RecoveryScheduledHash::exists(),
            "terminal recovery migration: source recovery state is not exact"
        );
        Ok(alloc::vec![u8::from(
            crate::configs::PhaseTransitionLock::get(),
        )])
    }

    #[cfg(feature = "try-runtime")]
    fn post_upgrade(state: Vec<u8>) -> Result<(), sp_runtime::TryRuntimeError> {
        frame_support::ensure!(
            !crate::configs::RecoveryCodeApplied::get()
                && !crate::configs::RecoveryLockdown::get()
                && !crate::configs::RecoveryScheduledHash::exists()
                && !crate::configs::RetiredMigrationCursor::exists()
                && !pallet_execution_guard::RecoveryImage::<Runtime>::exists(),
            "terminal recovery migration: recovery state survived completion"
        );
        if state.as_slice() == [1] {
            frame_support::ensure!(
                pallet_constitution::PhaseFlags::<Runtime>::get()
                    == pallet_constitution::PhaseFlagsValue::PARAM_ARMED
                    && !sp_io::storage::exists(&sudo_key_storage_key())
                    && matches!(
                        pallet_execution_guard::PhaseFourBridge::<Runtime>::get(),
                        pallet_execution_guard::PhaseFourBridgeState::Consumed
                    ),
                "terminal recovery migration: Phase-4 repair is incomplete"
            );
        }
        Ok(())
    }
}

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

#[cfg(feature = "try-runtime")]
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

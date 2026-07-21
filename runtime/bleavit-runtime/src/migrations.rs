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
    migrations::VersionedMigration, traits::UncheckedOnRuntimeUpgrade, weights::Weight,
};

#[cfg(feature = "try-runtime")]
use alloc::vec::Vec;

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

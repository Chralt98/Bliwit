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
    migrations::VersionedMigration, traits::UncheckedOnRuntimeUpgrade, weights::Weight,
};

#[cfg(feature = "try-runtime")]
use alloc::{vec, vec::Vec};

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
        let present = vec![
            u8::from(sp_io::storage::exists(&retired::blocked_meters_key())),
            u8::from(sp_io::storage::exists(&retired::progress_marker_key())),
        ];
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
    Weight::from_parts(600_000_000, 512_000)
        .saturating_add(<Runtime as frame_system::Config>::DbWeight::get().reads_writes(40, 30))
}

#[allow(dead_code)]
pub(crate) fn terminal_recovery_transition_weight() -> Weight {
    Weight::from_parts(900_000_000, 768_000)
        .saturating_add(<Runtime as frame_system::Config>::DbWeight::get().reads_writes(64, 48))
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

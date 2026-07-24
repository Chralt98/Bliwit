#![cfg_attr(not(feature = "std"), no_std)]
#![deny(unsafe_code)]

//! Production FRAME shell over `execution-guard-core` (A11, audit scope A).
//!
//! The core owns the queue state machine and terminal bookkeeping. This pallet
//! owns bounded storage, explicit origins, cross-pallet seams and the one
//! runtime-only operation the frame-free model cannot perform: decoding and
//! atomically dispatching the committed `RuntimeCall` batch after the complete
//! 09 §1.2 revalidation list.
//!
//! # Deferred specification questions (D-3; spec intentionally unchanged)
//! - 09 §2.2 describes permissionless application as a direct call plus a
//!   stateless SafetyFilter, while I-10/06 §3.1 permit internal Root for the
//!   single `authorize_upgrade` call and 09 §1.3 says "two allowlisted" calls.
//! - 09 §1.1's `QueuedExecution` sketch omits the implemented
//!   `declared_domains`, `failed_at`, and `ratification_passed` fields.
//! - 09 §1.5 references guard `Rejected`/`UpgradeApplied` events absent from
//!   frozen 02 §6, while local `Enqueued`/`PreimageUnpinned` additions are also
//!   outside that frozen row.
//! - 09 §1.4 lists `utility.batch`/`force_batch` among recursively inspected
//!   wrappers, but their best-effort semantics contradict 09 §1.2(12)'s atomic
//!   dispatch requirement; B1a must reject them and admit only `batch_all`.
//! - 09 §1.2(5) now splits the attestation read at the queue/execute boundary:
//!   queue admission uses the live roster, while execute-time reads the
//!   committed record quorum and v10 revocation surface. Routine rotation is
//!   therefore liveness-safe; cause-aware removal and adverse challenge remain
//!   fail-closed. See 06 §7 and PLAN.md SQ-97/SQ-312.

extern crate alloc;

pub use pallet::*;
pub use weights::WeightInfo;

pub mod weights;

#[cfg(feature = "runtime-benchmarks")]
mod benchmarking;
#[cfg(test)]
mod mock;
#[cfg(test)]
mod tests;

use alloc::vec::Vec;
use frame_support::{
    dispatch::{DispatchResultWithPostInfo, PostDispatchInfo},
    pallet_prelude::{ConstU32, DispatchError, DispatchResult},
    weights::Weight,
    BoundedVec,
};
use futarchy_primitives::{
    kernel, Balance, BlockNumber, EpochId, ProposalClass, ProposalId, ResourceId,
    RuntimeVersionConstraint, H256,
};
use parity_scale_codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use scale_info::TypeInfo;

pub use execution_guard_core::{
    domain_allowed, requires_ratification, AttestationView as CoreAttestationView, CallDomain,
    EpochHandoff as CoreEpochHandoff, Error as CoreError, Event as CoreEvent, ExecutionGuard,
    GuardOrigin, GuardianView as CoreGuardianView, PendingUpgrade, QueuedExecution,
    UpgradeAuthorization, DESCRIPTOR_LEAD_TIME, MAX_CALLS, MAX_DECLARED_DOMAINS,
    MAX_EXECUTION_RECORDS, MAX_PAYLOAD_BYTES, MAX_QUEUE, MAX_RESOURCE_LOCKS, RETRY_WINDOW,
};

pub const MAX_QUEUE_BOUND: u32 = MAX_QUEUE as u32;
pub const MAX_RECORDS_BOUND: u32 = MAX_EXECUTION_RECORDS as u32;
pub const MAX_CALLS_BOUND: u32 = MAX_CALLS as u32;
pub const MAX_DOMAINS_BOUND: u32 = MAX_DECLARED_DOMAINS as u32;
pub const MAX_LOCKS_PER_PROPOSAL_BOUND: u32 = MAX_RESOURCE_LOCKS as u32;
pub const MAX_HELD_RESOURCES_BOUND: u32 = MAX_QUEUE_BOUND * MAX_LOCKS_PER_PROPOSAL_BOUND;
/// Encoded `pallet-migrations::MigrationCursor` envelope: the configured inner
/// cursor bound plus SCALE's enum/index/option/length/block-number overhead.
/// This is a derived type bound, not a protocol parameter; the runtime test pins
/// it against `CursorOf<Runtime>::max_encoded_len()` so the exact-byte diagnostic
/// cannot silently truncate after an SDK type change.
pub const MAX_MIGRATION_HALT_CURSOR_BOUND: u32 =
    futarchy_primitives::bounds::MIGRATION_CURSOR_MAX_LEN + 16;
pub const MAX_RATIFICATIONS_BOUND: u32 =
    futarchy_primitives::bounds::INTAKE_QUEUE + futarchy_primitives::bounds::MAX_LIVE_PROPOSALS;
pub const MAX_QUALIFIED_RECOVERY_IMAGES_BOUND: u32 =
    futarchy_primitives::bounds::INTAKE_QUEUE + futarchy_primitives::bounds::MAX_LIVE_PROPOSALS;

pub type ReDerivedDomains = BoundedVec<CallDomain, ConstU32<MAX_CALLS_BOUND>>;

/// One borrowed top-level call's closed-wrapper analysis. `nested_calls`
/// counts the top-level call and every recursively carried call so the guard
/// can enforce the 16-call budget across the entire payload, not once per
/// top-level item (09 §1.4; I-11/I-20).
pub struct ReDerivedCall {
    pub domains: ReDerivedDomains,
    pub nested_calls: u32,
}

/// A8→A11 terminal callbacks. Ledger resolution remains behind pallet-epoch's
/// ResolveAuthority and is deliberately absent here (05 §6).
pub trait EpochHandoff {
    /// Immutable proposal commitment, available from submission through reap.
    /// Ratification may precede queue admission (06 §2.2), so the guard
    /// cannot derive this binding from `Queue` alone.
    fn payload_hash(pid: ProposalId) -> Option<H256>;
    /// Whether a pre-queue ratification binding belongs to a class that
    /// requires values ratification.  Lightweight test seams may return
    /// `None`; the production runtime returns the exact class predicate so
    /// try-state can reject a forged pending join rather than trusting only a
    /// payload hash.
    fn requires_ratification(_pid: ProposalId) -> Option<bool> {
        None
    }
    /// Immutable proposal context exposed only while a proposal is eligible
    /// for healthy-chain recovery-image qualification.
    fn recovery_qualification_context(pid: ProposalId) -> Option<(H256, RuntimeVersionConstraint)> {
        let _ = pid;
        None
    }
    fn mark_executed(pid: ProposalId) -> DispatchResult;
    fn mark_failed_executed(pid: ProposalId) -> DispatchResult;
    fn retry_exhausted_to_measurement(pid: ProposalId) -> DispatchResult;
    fn reject_or_stale(
        pid: ProposalId,
        reason: futarchy_primitives::RejectReason,
    ) -> DispatchResult;
    /// True only after epoch has left every guard-owned queue/retry state.
    /// Used by guard try-state to detect a leaked terminal queue entry.
    fn is_terminal(pid: ProposalId) -> bool;
}

/// Runtime treasury mirror for queued, statically-sized proposal outflows
/// (08 §1.2). Queue progress is fail-soft, but a failed exact sync must force
/// spendable NAV to zero and leave a try-state-visible mismatch.
pub trait PendingOutflowSync {
    fn sync_pending_outflows() -> DispatchResult;
    fn force_fail_static() -> bool;
    fn pending_outflows_synced() -> bool;
}

impl PendingOutflowSync for () {
    fn sync_pending_outflows() -> DispatchResult {
        Ok(())
    }

    fn force_fail_static() -> bool {
        true
    }

    fn pending_outflows_synced() -> bool {
        true
    }
}

/// Read-only preimage projection. `fetch` must return only the bytes stored under
/// the exact `(hash, expected_len)` key. Implementations must reject an expected
/// length above the kernel payload cap before reading the payload bytes. The
/// pallet still re-hashes and length-checks the result and never trusts the key.
pub trait Preimages {
    fn len(hash: H256) -> Option<u32>;
    fn fetch(hash: H256, expected_len: u32) -> Option<Vec<u8>>;
    fn pin(hash: H256) -> DispatchResult;
    fn unpin(hash: H256) -> DispatchResult;
}

/// Preimage access for a full runtime image. Unlike [`Preimages`], this seam is
/// bounded by `MaxRuntimeCodeBytes`, not the 64 KiB proposal-payload ceiling.
pub trait RecoveryImages {
    fn len(hash: H256) -> Option<u32>;
    fn fetch(hash: H256, expected_len: u32) -> Option<Vec<u8>>;
    fn is_pinned(_hash: H256) -> bool {
        true
    }
    /// Qualification-time, state-backed host envelope. This must include every
    /// artifact-independent schedulability bound available while the chain is
    /// healthy (notably the relay host's current maximum code size).
    fn preflight_qualifies(_code: &[u8]) -> bool {
        true
    }
    fn pin(hash: H256) -> DispatchResult;
    fn unpin(hash: H256) -> DispatchResult;
}

/// Runtime-owned Phase-3 state used by the one-shot sudo-to-guard bridge.
pub trait PhaseState {
    /// True only for the exact Phase-3 bootstrap posture: shadow mode and sudo
    /// present, with no binding proposal-class bit armed.
    fn exact_phase_three() -> bool;
    fn exact_phase_four() -> bool;
    /// Monotone post-bootstrap predicate used by permanent bridge history:
    /// PARAM remains armed, Sudo/shadow never return, and later binding bits
    /// may be added by subsequent phases.
    fn post_sudo_phase() -> bool {
        Self::exact_phase_four()
    }
    /// True when this proposal class is binding in the current phase.
    fn class_execution_enabled(_class: ProposalClass) -> bool {
        true
    }
    fn phase_four_plan_valid(_plan: &PhaseFourPlan) -> bool {
        true
    }
}

/// Bonded attestor projection (I-19). All reads fail closed.
pub trait Attestations {
    fn artifact_hash(attestation_id: u32) -> Option<H256>;
    fn present_unrevoked_unchallenged(attestation_id: u32) -> bool;
    fn has_quorum(pid: ProposalId, artifact_hash: H256) -> bool;
    /// Execute-time quorum over the committed record set. Implementations
    /// that predate contract v11 may conservatively fall back to the live
    /// roster read.
    fn has_record_quorum(pid: ProposalId, artifact_hash: H256) -> bool {
        Self::has_quorum(pid, artifact_hash)
    }
}

/// Guardian/playbook projection used at ordered checks 9 and 10.
pub trait GuardianState {
    fn rerun_held(pid: ProposalId) -> bool;
    fn gate_suspended() -> bool;
    fn ledger_freeze_active() -> bool;
    /// Live constitution dead-man latch. This is intentionally read-only:
    /// copying it into the guard's local storage would make a recovered
    /// incident self-latching on the next aggregate persistence.
    fn dead_man_freeze_active() -> bool;
}

/// Live constitution parameters. Queue timestamps are frozen once, at enqueue;
/// execute never recomputes them from live values.
pub trait Params {
    fn exec_timelock(class: ProposalClass) -> BlockNumber;
    fn exec_grace(class: ProposalClass) -> BlockNumber;
    fn code_spacing() -> BlockNumber;
}

/// Live constitution capability projection for canonical dispatch-time check
/// 6 (09 §1.2(6)). Static class/domain compatibility is necessary but not
/// sufficient: values governance can disable a capability after queueing.
pub trait Capabilities<Call> {
    /// Re-check the live constitution row(s) required by this exact call.
    /// Broad call domains are insufficient for keyed capabilities such as
    /// `SetParam(ParamKey)`, so the runtime adapter receives the decoded call.
    fn call_enabled(class: ProposalClass, call: &Call) -> bool;
}

/// Runtime view of the deferred parachain code-scheduling boundary.
pub trait UpgradeSchedule {
    /// True only after frame-system consumed the matching authorization and
    /// Cumulus durably stored a pending validation function.
    fn scheduling_performed() -> bool;
}

/// Read-only projection of the SDK multi-block-migration cursor. The guard
/// needs only presence: cursor contents remain single-sourced and bounded by
/// `pallet-migrations` (09 §3.2(2)). Runtime implementations must answer with
/// at most one bounded storage read, which `on_initialize` charges.
pub trait MigrationStatusProvider {
    fn cursor_exists() -> bool;
    fn recovery_state() -> MigrationRecoveryState {
        MigrationRecoveryState::default()
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MigrationRecoveryState {
    pub lockdown: bool,
    pub bypass: bool,
    pub retired_cursor: bool,
    pub scheduled_hash: Option<H256>,
    pub aborted: bool,
    pub recovery_code_applied: bool,
    pub phase_transition_lock: bool,
    pub phase_transition_applied: bool,
}

impl MigrationStatusProvider for () {
    fn cursor_exists() -> bool {
        false
    }
}

/// The execution guard is one of the two exhaustive writers of the frozen
/// 168-byte ReleaseChannel record (02 §12).
pub trait ReleaseChannelWriter {
    fn on_upgrade_authorized(
        target_spec_version: u32,
        authorized_at: BlockNumber,
    ) -> DispatchResult;
    fn on_upgrade_applied(target_spec_version: u32) -> DispatchResult;
    /// Relay-Abort status-quo clear. Writer (b) cannot alter the guard-owned
    /// indication, so this clears it unconditionally with the guard state.
    fn on_upgrade_aborted(target_spec_version: u32) -> DispatchResult;
    /// The guard-owned `pending_authorized_at` and `URGENT_UPGRADE` projection
    /// from the live channel, used to enforce I-30 in try-state.
    fn pending_upgrade_indication() -> (BlockNumber, bool);
}

/// B1a's concrete `RuntimeCall` projection. It must walk the closed wrapper set
/// under the MAX_NESTED/MAX_CALLS budget, re-derive every leaf domain and the
/// total recursive call count, reapply
/// pallet-origins' origin-aware SafetyFilter, and construct the class origin.
/// The mock implementation does those operations over a real construct_runtime
/// call. Best-effort wrappers (`utility.batch` and `utility.force_batch`) MUST
/// be rejected: only atomic `utility.batch_all` is compatible with 09
/// §1.2(12). B1a's concrete RuntimeCall classifier must enforce that constraint
/// (D-1; the 09 §1.4 recursion-list tension remains a spec question).
pub trait BatchDispatcher<Call> {
    fn rederive_call(call: &Call) -> Result<ReDerivedCall, DispatchError>;
    /// Origin-aware closed filter. The runtime projection must deny every
    /// origin-elevating wrapper, including `sudo.sudo`, even when the inner
    /// leaf is otherwise within the proposal class.
    fn safety_filter(class: ProposalClass, call: &Call) -> bool;
    /// Recognizes only the exact allowlisted `system.authorize_upgrade(hash)`.
    fn authorize_upgrade_hash(call: &Call) -> Option<H256>;
    fn recovery_image_descriptor(call: &Call) -> Option<RecoveryImageDescriptor>;
    fn phase_four_plan(_class: ProposalClass, _calls: &[Call]) -> Option<PhaseFourPlan> {
        None
    }
    /// True only for the two exact cap-value leaves that a Phase-4 mandate
    /// commits for deferred application. They are validated at authorization
    /// but MUST NOT dispatch until the code-application migration atomically
    /// removes Sudo and arms PARAM.
    fn is_phase_four_cap_commitment(_call: &Call) -> bool {
        false
    }
    fn dispatch_with_class_origin(call: Call, class: ProposalClass) -> DispatchResult;
    /// Post-info-preserving form used for execute refunds. Existing runtime
    /// dispatchers that erase `PostDispatchInfo` remain source-compatible and
    /// deliberately report `actual_weight: None`, causing the guard to fall
    /// back to the call's declared total weight (never an undercharge).
    fn dispatch_with_class_origin_post_info(
        call: Call,
        class: ProposalClass,
    ) -> DispatchResultWithPostInfo {
        Self::dispatch_with_class_origin(call, class)?;
        Ok(PostDispatchInfo::default())
    }
    /// The sole internal-Root dispatch: exactly the committed
    /// `system.authorize_upgrade(hash)` call (I-10).
    fn dispatch_authorize_upgrade(code_hash: H256) -> DispatchResult;
    fn dispatch_authorize_upgrade_post_info(code_hash: H256) -> DispatchResultWithPostInfo {
        Self::dispatch_authorize_upgrade(code_hash)?;
        Ok(PostDispatchInfo::default())
    }
    /// Dispatch permissionless `system.apply_authorized_upgrade(code)` with a
    /// non-Root origin (Signed or None). Root is forbidden on this seam; the
    /// apply/direct+stateless-filter wording tension is retained as D-3.
    fn dispatch_apply_authorized_upgrade(code: Vec<u8>) -> DispatchResult;
    /// Full runtime identity re-derived from the candidate artifact, never
    /// supplied by the caller. `frame_system` reports invalid-version
    /// authorization as a successful dispatch, so the guard must reject a
    /// wrong spec name/version before internal-Root application.
    fn observed_runtime_version(code: &[u8]) -> Option<RuntimeVersionConstraint>;
}

#[derive(
    Clone,
    Copy,
    Debug,
    Decode,
    DecodeWithMemTracking,
    Encode,
    Eq,
    MaxEncodedLen,
    PartialEq,
    TypeInfo,
)]
pub struct RecoveryImageDescriptor {
    pub hash: H256,
    pub len: u32,
    pub target_spec_version: u32,
    pub attestation_id: u32,
}

#[derive(
    Clone,
    Copy,
    Debug,
    Decode,
    DecodeWithMemTracking,
    Encode,
    Eq,
    MaxEncodedLen,
    PartialEq,
    TypeInfo,
)]
pub struct PhaseFourPlan {
    pub tvl_cap: Balance,
    pub deposit_cap: Balance,
}

#[cfg(feature = "runtime-benchmarks")]
pub trait BenchmarkHelper<RuntimeOrigin> {
    fn ratify_origin() -> RuntimeOrigin;
    fn recovery_commit_origin() -> RuntimeOrigin;
    fn phase_four_origin() -> RuntimeOrigin;
    fn prime_ratify(pid: ProposalId, referendum_index: u32);
    fn prime_execute(pid: ProposalId, calls: u32);
    fn prime_recovery_commit(pid: ProposalId) -> RecoveryImageDescriptor;
    fn prime_recovery_qualification(pid: ProposalId, bytes: u32);
    fn prime_phase_four(pid: ProposalId);
    fn prime_recovery_application() -> (H256, RuntimeVersionConstraint);
    fn prime_failed(pid: ProposalId);
    fn prime_pending_upgrade(bytes: u32) -> Vec<u8>;
    fn prime_stale(pid: ProposalId);
    fn prime_keeper_rebate() {}
    fn assert_keeper_rebate_paid(_: futarchy_primitives::keeper::CrankClass) {}
}

#[frame_support::pallet]
pub mod pallet {
    use super::*;
    use crate::PendingOutflowSync;
    use alloc::vec::Vec;
    use core::marker::PhantomData;
    use frame_support::{
        dispatch::{
            DispatchClass, DispatchErrorWithPostInfo, DispatchResultWithPostInfo, GetDispatchInfo,
            Pays, PostDispatchInfo,
        },
        pallet_prelude::*,
        storage::with_storage_layer,
        traits::EnsureOrigin,
    };
    use frame_system::pallet_prelude::*;
    use futarchy_primitives::{
        keeper::{CrankClass, KeeperRebateSink},
        DispatchOutcomeCode, ExecutionRecord, RejectReason, INTEGRATION_CONTRACT_VERSION,
    };
    use parity_scale_codec::{Compact, Decode, DecodeLimit, DecodeWithMemTracking, Encode};
    use sp_runtime::{
        traits::{CheckedSub, Hash as HashT, One},
        SaturatedConversion, TryRuntimeError,
    };

    // Bumped 0 -> 1 by B16 (SQ-132/SQ-146): the first storage migration this
    // runtime has ever shipped retires `BlockedMeters` (inert, no production
    // writer) and the runtime-level `MigrationProgressMarker`/cursor-hash. The
    // versioned migration that clears both keys lives in
    // `runtime::migrations::RetireB16State` and is gated on this version.
    const STORAGE_VERSION: StorageVersion = StorageVersion::new(1);

    #[pallet::pallet]
    #[pallet::storage_version(STORAGE_VERSION)]
    pub struct Pallet<T>(_);

    #[pallet::config]
    pub trait Config:
        frame_system::Config<Hash: From<H256> + Into<H256>, RuntimeEvent: From<Event<Self>>>
    {
        type Epoch: EpochHandoff;
        type EnqueueAuthority: EnsureOrigin<Self::RuntimeOrigin>;
        type Attestations: Attestations;
        type Guardian: GuardianState;
        type Params: Params;
        type Capabilities: Capabilities<Self::RuntimeCall>;
        type UpgradeSchedule: UpgradeSchedule;
        type MigrationStatus: MigrationStatusProvider;
        type Preimages: Preimages;
        type RecoveryImages: RecoveryImages;
        type ReleaseChannel: ReleaseChannelWriter;
        type RatifyOrigin: EnsureOrigin<Self::RuntimeOrigin>;
        type RecoveryCommitOrigin: EnsureOrigin<Self::RuntimeOrigin>;
        type PhaseFourBridgeOrigin: EnsureOrigin<Self::RuntimeOrigin>;
        type PhaseState: PhaseState;
        type Dispatcher: BatchDispatcher<Self::RuntimeCall>;
        /// Fail-soft keeper rebate sink (08 §6). It must never affect a crank.
        type KeeperRebate: KeeperRebateSink<Self::AccountId>;
        /// Fail-soft/loud exact mirror of queue-owned treasury obligations.
        type PendingOutflowSync: crate::PendingOutflowSync;
        /// Runtime-assembly bound for candidate Wasm. This is intentionally
        /// distinct from the 64 KiB proposal-call batch bound.
        #[pallet::constant]
        type MaxRuntimeCodeBytes: Get<u32>;
        type WeightInfo: WeightInfo;
        #[cfg(feature = "runtime-benchmarks")]
        type BenchmarkHelper: BenchmarkHelper<Self::RuntimeOrigin>;
    }

    pub type StoredMeters = BoundedVec<ResourceId, ConstU32<MAX_LOCKS_PER_PROPOSAL_BOUND>>;
    pub type StoredDomains = BoundedVec<CallDomain, ConstU32<MAX_DOMAINS_BOUND>>;
    pub type StoredRecords = BoundedVec<ExecutionRecord, ConstU32<MAX_RECORDS_BOUND>>;
    pub type StoredHeldResources =
        BoundedVec<(ProposalId, ResourceId), ConstU32<MAX_HELD_RESOURCES_BOUND>>;
    pub type StoredUpgradeSpacingHistory =
        BoundedVec<(BlockNumber, BlockNumber), ConstU32<MAX_RECORDS_BOUND>>;
    /// Bound for the exact diagnostic cursor bytes carried by
    /// [`Event::MigrationHalted`] (09 §3.2(4)). It covers the configured inner
    /// cursor plus the SDK cursor's SCALE envelope; a runtime regression pins the
    /// relationship against the SDK type's `MaxEncodedLen`.
    pub type MigrationHaltCursor = BoundedVec<u8, ConstU32<MAX_MIGRATION_HALT_CURSOR_BOUND>>;
    pub type RuntimeBatch<T> =
        BoundedVec<<T as frame_system::Config>::RuntimeCall, ConstU32<MAX_CALLS_BOUND>>;
    pub type RuntimeCode<T> = BoundedVec<u8, <T as Config>::MaxRuntimeCodeBytes>;

    #[derive(
        Clone, Debug, Decode, DecodeWithMemTracking, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo,
    )]
    pub struct RecoveryImageCommitment {
        pub pid: ProposalId,
        pub primary_hash: H256,
        pub hash: H256,
        pub len: u32,
        pub target_spec_version: u32,
        pub attestation_id: u32,
        pub committed_at: BlockNumber,
    }

    #[derive(
        Clone, Debug, Decode, DecodeWithMemTracking, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo,
    )]
    pub struct QualifiedRecoveryImage {
        pub payload_hash: H256,
        pub primary_hash: H256,
        pub version_constraint: RuntimeVersionConstraint,
        pub descriptor: RecoveryImageDescriptor,
    }

    #[derive(
        Clone, Debug, Decode, DecodeWithMemTracking, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo,
    )]
    pub(crate) struct ExecutingUpgradeContext {
        pub(crate) pid: ProposalId,
        pub(crate) primary_hash: H256,
        pub(crate) primary_target_spec_version: u32,
    }

    /// 02 §7.4 Queue value. Field order is the brief's frozen order.
    #[derive(
        Clone, Debug, Decode, DecodeWithMemTracking, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo,
    )]
    pub struct StoredQueuedExecution {
        pub pid: ProposalId,
        pub payload_hash: H256,
        pub payload_len: u32,
        pub class: ProposalClass,
        pub maturity: BlockNumber,
        pub grace_end: BlockNumber,
        pub version_constraint: RuntimeVersionConstraint,
        pub meters_declared: StoredMeters,
        pub ratify_ref: Option<u32>,
        pub ratification_passed: bool,
        pub attestation_id: Option<u32>,
        pub pre_upgrade_checkpoint: Option<(H256, H256)>,
        pub cancelled: bool,
        pub declared_domains: StoredDomains,
        pub failed_at: Option<BlockNumber>,
    }

    impl From<StoredQueuedExecution> for QueuedExecution {
        fn from(value: StoredQueuedExecution) -> Self {
            Self {
                pid: value.pid,
                payload_hash: value.payload_hash,
                payload_len: value.payload_len,
                class: value.class,
                maturity: value.maturity,
                grace_end: value.grace_end,
                version_constraint: value.version_constraint,
                meters_declared: value.meters_declared.into_inner(),
                ratify_ref: value.ratify_ref,
                ratification_passed: value.ratification_passed,
                attestation_id: value.attestation_id,
                // Retained solely for the frozen v6 SCALE shape. The retired
                // authorization-time checkpoint has no behavioral meaning.
                pre_upgrade_checkpoint: None,
                cancelled: value.cancelled,
                declared_domains: value.declared_domains.into_inner(),
                failed_at: value.failed_at,
            }
        }
    }

    impl TryFrom<QueuedExecution> for StoredQueuedExecution {
        type Error = CoreError;
        fn try_from(value: QueuedExecution) -> Result<Self, Self::Error> {
            Ok(Self {
                pid: value.pid,
                payload_hash: value.payload_hash,
                payload_len: value.payload_len,
                class: value.class,
                maturity: value.maturity,
                grace_end: value.grace_end,
                version_constraint: value.version_constraint,
                meters_declared: BoundedVec::try_from(value.meters_declared)
                    .map_err(|_| CoreError::TooManyLocks)?,
                ratify_ref: value.ratify_ref,
                ratification_passed: value.ratification_passed,
                attestation_id: value.attestation_id,
                // Retained solely for the frozen v6 SCALE shape. New writes
                // must keep the compatibility field inert.
                pre_upgrade_checkpoint: None,
                cancelled: value.cancelled,
                declared_domains: BoundedVec::try_from(value.declared_domains)
                    .map_err(|_| CoreError::TooManyDomains)?,
                failed_at: value.failed_at,
            })
        }
    }

    #[derive(
        Clone,
        Copy,
        Debug,
        Decode,
        DecodeWithMemTracking,
        Encode,
        Eq,
        MaxEncodedLen,
        PartialEq,
        TypeInfo,
    )]
    pub struct RatificationRecord {
        pub referendum_index: u32,
        pub payload_hash: H256,
        pub ratified_at: BlockNumber,
    }

    /// Frozen 02 §7.4 names and key/value shapes.
    #[pallet::storage]
    pub type Queue<T: Config> =
        CountedStorageMap<_, Blake2_128Concat, ProposalId, StoredQueuedExecution, OptionQuery>;

    #[pallet::storage]
    pub type Ratifications<T: Config> =
        CountedStorageMap<_, Blake2_128Concat, ProposalId, RatificationRecord, OptionQuery>;

    /// Referendum identity bound by the CODE/META proposer before ratification
    /// passes. This is an internal join: it is mirrored into the queue at
    /// admission, retained across a non-terminal rerun, and consumed only
    /// when the ratification passes or the proposal becomes terminal. It never
    /// appears in the frozen 02 queue view.
    #[pallet::storage]
    pub type PendingRatifications<T: Config> =
        CountedStorageMap<_, Blake2_128Concat, ProposalId, u32, OptionQuery>;

    #[pallet::storage]
    pub type ExecutionRecords<T: Config> = StorageValue<_, StoredRecords, ValueQuery>;

    #[pallet::storage]
    pub type PendingUpgrade<T: Config> =
        StorageValue<_, execution_guard_core::PendingUpgrade, OptionQuery>;

    #[pallet::storage]
    pub type CurrentSpecName<T: Config> = StorageValue<_, RuntimeVersionConstraint, OptionQuery>;

    // Internal bounded mirrors for the core aggregate and I-7/I-17 envelope.
    #[pallet::storage]
    pub type HeldResources<T: Config> = StorageValue<_, StoredHeldResources, ValueQuery>;

    #[pallet::storage]
    pub type HardGateBreach<T: Config> = StorageValue<_, bool, ValueQuery>;

    #[pallet::storage]
    pub type DeadManFreeze<T: Config> = StorageValue<_, bool, ValueQuery>;

    #[pallet::storage]
    pub type MigrationHalt<T: Config> = StorageValue<_, bool, ValueQuery>;

    /// Epoch-scoped guardian suspension. It is effective only while the runtime
    /// projection reports the same current epoch and the hard-gate flag remains
    /// live (06 §5.2; 09 §1.2 check 9).
    #[pallet::storage]
    pub type GateSuspension<T: Config> = StorageValue<_, EpochId, OptionQuery>;

    /// Queue-time-frozen expedited-lane bit; kept outside the frozen Queue value.
    #[pallet::storage]
    pub type Expedited<T: Config> = StorageMap<_, Blake2_128Concat, ProposalId, bool, ValueQuery>;

    #[pallet::storage]
    pub type LastUpgradeAuthorized<T: Config> = StorageValue<_, BlockNumber, OptionQuery>;

    /// Bounded proof trail for the guard-owned I-7/I-17 meter. Each entry is
    /// `(authorized_at, spacing_enforced_for_this_authorization)`; expedited
    /// recovery entries use zero for the normative exemption.
    #[pallet::storage]
    pub type UpgradeSpacingHistory<T: Config> =
        StorageValue<_, StoredUpgradeSpacingHistory, ValueQuery>;

    /// One-block application latch. The relay callback runs after the current
    /// block's initialization, so the next `on_initialize` is the first point
    /// at which the newly installed runtime's MBM cursor can be observed.
    #[pallet::storage]
    pub type PendingAnchorCapture<T: Config> = StorageValue<_, bool, ValueQuery>;

    /// PB-MIGRATION's application-time anchor: the number and committed hash
    /// of the last block before the new image's migrations could step.
    #[pallet::storage]
    pub type PreMigrationAnchor<T: Config> =
        StorageValue<_, (BlockNumberFor<T>, H256), OptionQuery>;

    /// The target whose application was successfully scheduled in Cumulus.
    /// This is deliberately distinct from authorization: relay `Abort` can
    /// consume the Cumulus pending code only after this latch is present.
    #[pallet::storage]
    pub type ScheduledUpgrade<T: Config> = StorageValue<_, H256, OptionQuery>;

    /// Queue-time-frozen `(attestation_id, artifact_hash)` commitment. The
    /// frozen Queue layout has no artifact-hash field; this bounded auxiliary
    /// map prevents a mutable id→artifact projection from changing meaning
    /// after admission (09 §1.1(3)/§1.2(5)).
    #[pallet::storage]
    pub type AttestationBindings<T: Config> =
        StorageMap<_, Blake2_128Concat, ProposalId, (u32, H256), OptionQuery>;

    /// The recovery image committed by the currently authorized CODE/META
    /// mandate. Its full Wasm lives in pallet-preimage and stays requested
    /// until the primary image finishes without an MBM, its MBM completes, or
    /// this image is applied.
    #[pallet::storage]
    pub type RecoveryImage<T: Config> = StorageValue<_, RecoveryImageCommitment, OptionQuery>;

    #[pallet::storage]
    pub type QueuedRecoveryImages<T: Config> =
        StorageMap<_, Blake2_128Concat, ProposalId, RecoveryImageDescriptor, OptionQuery>;

    /// A recovery image qualified while the chain is healthy. Qualification is
    /// a separately weighted, one-image operation so the epoch's ten-item tick
    /// batch never multiplies a full-runtime-Wasm proof. The preimage request
    /// owned by this entry transfers to `QueuedRecoveryImages` at enqueue.
    #[pallet::storage]
    pub type QualifiedRecoveryImages<T: Config> =
        CountedStorageMap<_, Blake2_128Concat, ProposalId, QualifiedRecoveryImage, OptionQuery>;

    /// Recovery-image pins retained across the proposal's non-terminal rerun
    /// cycle. Ownership transfers back to `QueuedRecoveryImages` on re-enqueue
    /// without issuing a second preimage request.
    #[pallet::storage]
    pub type RerunRecoveryPins<T: Config> =
        CountedStorageMap<_, Blake2_128Concat, ProposalId, RecoveryImageDescriptor, OptionQuery>;

    /// Ephemeral context visible only while the guard dispatches one already-
    /// validated upgrade batch. It prevents the public call surface from
    /// creating an unbound recovery commitment.
    #[pallet::storage]
    pub(crate) type ExecutingUpgrade<T: Config> =
        StorageValue<_, ExecutingUpgradeContext, OptionQuery>;

    #[derive(
        Clone,
        Debug,
        Decode,
        DecodeWithMemTracking,
        Default,
        Encode,
        Eq,
        MaxEncodedLen,
        PartialEq,
        TypeInfo,
    )]
    pub enum PhaseFourBridgeState {
        #[default]
        Unused,
        Pending {
            pid: ProposalId,
            code_hash: H256,
            plan: PhaseFourPlan,
        },
        Scheduled {
            pid: ProposalId,
            code_hash: H256,
            plan: PhaseFourPlan,
        },
        Consumed,
    }

    /// One-shot Phase-3→4 bridge state. Relay Abort returns `Pending` to
    /// `Unused`; only observed code application makes it permanently consumed.
    #[pallet::storage]
    pub type PhaseFourBridge<T: Config> = StorageValue<_, PhaseFourBridgeState, ValueQuery>;

    /// Payload pins retained while a queued proposal is in a rerun cycle.
    /// This internal bounded marker transfers the existing pin back into a
    /// later queue entry without an unpinned interval or a double request.
    #[pallet::storage]
    pub type RerunPins<T: Config> =
        CountedStorageMap<_, Blake2_128Concat, ProposalId, H256, OptionQuery>;

    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        // Frozen 02 §6 surface.
        Executed {
            pid: ProposalId,
            record: ExecutionRecord,
        },
        ExecutionFailed {
            pid: ProposalId,
            outcome: DispatchOutcomeCode,
        },
        Ratified {
            pid: ProposalId,
            referendum_index: u32,
        },
        UpgradeAuthorized {
            code_hash: H256,
            authorized_at: BlockNumber,
        },
        // Append-only local observability required by 09 terminal paths.
        Enqueued {
            pid: ProposalId,
            maturity: BlockNumber,
        },
        Rejected {
            pid: ProposalId,
            reason: RejectReason,
        },
        UpgradeApplied {
            code_hash: H256,
            spec_version: u32,
        },
        PreimageUnpinned {
            pid: ProposalId,
            payload_hash: H256,
        },
        UpgradeAborted {
            code_hash: H256,
        },
        /// Defensive alarm: the exact queue mirror failed. `fail_static` says
        /// whether the adapter successfully forced spendable NAV to zero.
        PendingOutflowSyncFailed {
            queued: u32,
            fail_static: bool,
        },
        /// PB-MIGRATION machine-trigger diagnostic (09 §3.2(4)): emitted on the
        /// first activation of a migration halt source (failed step, stall,
        /// applied-code mismatch, or failed abort cleanup). `cursor` carries the
        /// SDK cursor's exact bytes (empty for a source-less halt); `failed_step`
        /// is the SDK-reported step index. This is an operator/monitoring
        /// diagnostic (12 §6.3, RB-UPGRADE),
        /// **outside** the frozen 02 §6 ingest set by that section's (a)-(c) rule
        /// — the same off-contract class as `PendingOutflowSyncFailed`, so it
        /// carries no `INTEGRATION_CONTRACT_VERSION` bump.
        MigrationHalted {
            cursor: MigrationHaltCursor,
            failed_step: Option<u32>,
        },
        // B16 operator diagnostics are appended after every pre-existing
        // off-contract variant so their introduction cannot renumber one.
        RecoveryImageCommitted {
            pid: ProposalId,
            primary_hash: H256,
            recovery_hash: H256,
            target_spec_version: u32,
        },
        RecoveryImageApplied {
            recovery_hash: H256,
            spec_version: u32,
        },
        PhaseFourUpgradeAuthorized {
            pid: ProposalId,
            code_hash: H256,
            justification_hash: H256,
        },
        RecoveryImageQualified {
            pid: ProposalId,
            recovery_hash: H256,
            target_spec_version: u32,
        },
    }

    #[pallet::error]
    pub enum Error<T> {
        QueueFull,
        NotFound,
        Cancelled,
        NotMature,
        GraceExpired,
        BadPreimage,
        StaleQueue,
        NotRatified,
        AttestationMissing,
        CapabilityDenied,
        MetersBlocked,
        ResourceLockMissing,
        GuardianHold,
        GateSuspended,
        FreezeActive,
        PayloadTooLarge,
        TooManyCalls,
        TooManyDomains,
        TooManyLocks,
        BadDomainDeclaration,
        SafetyFilter,
        DispatchFailed,
        BadUpgradePayload,
        PendingUpgradeExists,
        NoPendingUpgrade,
        DescriptorLeadTime,
        UpgradeHashMismatch,
        UpgradeVersionMismatch,
        RecoveryImageMissing,
        RecoveryImageInvalid,
        ShadowMode,
        PhaseFourBridgeUsed,
        JustificationMissing,
        RetryWindowOpen,
        Overflow,
    }

    #[pallet::hooks]
    impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
        fn on_initialize(now: BlockNumberFor<T>) -> Weight {
            let mut writes = 0;
            if PendingAnchorCapture::<T>::get() {
                let mut capture_consumed = false;
                if T::MigrationStatus::cursor_exists() {
                    if let Some(anchor_block) = now.checked_sub(&One::one()) {
                        let parent_hash = frame_system::Pallet::<T>::parent_hash().into();
                        PreMigrationAnchor::<T>::put((anchor_block, parent_hash));
                        writes += 1;
                        capture_consumed = true;
                    }
                } else if Self::release_recovery_image().is_ok() {
                    // The primary image registered no MBMs, so its dormant
                    // recovery request has no remaining purpose.
                    writes += 1;
                    capture_consumed = true;
                } else {
                    // A corrupt/missing request must not silently consume the
                    // only cleanup latch. Keep retrying under an execution
                    // halt until operators repair the pin state.
                    MigrationHalt::<T>::put(true);
                    writes += 1;
                }
                if capture_consumed {
                    // No cursor means the image registered no MBMs. Either
                    // way, a valid application boundary is consumed once.
                    PendingAnchorCapture::<T>::kill();
                    writes += 1;
                }
            }
            if let Some(pending) = PendingUpgrade::<T>::get() {
                if ScheduledUpgrade::<T>::get().is_none()
                    && T::UpgradeSchedule::scheduling_performed()
                {
                    ScheduledUpgrade::<T>::put(pending.hash);
                    writes += 1;
                }
            } else if ScheduledUpgrade::<T>::take().is_some() {
                // A marker without guard ownership can never authorize or
                // recover anything; remove it fail-closed.
                writes += 1;
            }
            // Worst case: the capture latch + migration cursor + System parent
            // hash, PendingUpgrade + ScheduledUpgrade, and the schedule seam's
            // two proofs (Cumulus pending code + system authorization).
            T::DbWeight::get().reads_writes(7, writes)
        }

        fn integrity_test() {
            assert_eq!(
                MAX_QUEUE_BOUND,
                futarchy_primitives::bounds::MAX_LIVE_PROPOSALS
            );
            assert_eq!(MAX_CALLS_BOUND, futarchy_primitives::kernel::MAX_CALLS);
            assert_eq!(MAX_PAYLOAD_BYTES, futarchy_primitives::kernel::MAX_BYTES);
            assert_eq!(
                DESCRIPTOR_LEAD_TIME,
                futarchy_primitives::kernel::DESCRIPTOR_LEAD_TIME_BLOCKS
            );
            assert_eq!(
                MAX_RECORDS_BOUND,
                futarchy_primitives::bounds::MAX_EXECUTION_RECORDS
            );
        }

        #[cfg(feature = "try-runtime")]
        fn try_state(_n: BlockNumberFor<T>) -> Result<(), TryRuntimeError> {
            Self::do_try_state()
        }
    }

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        /// Permissionless 09 §1.2 execution crank.
        #[pallet::call_index(0)]
        #[pallet::weight(Pallet::<T>::execute_precharge())]
        pub fn execute(origin: OriginFor<T>, pid: ProposalId) -> DispatchResultWithPostInfo {
            let checks_only = T::WeightInfo::execute(MAX_CALLS_BOUND);
            let who = ensure_signed(origin)
                .map_err(|error| Self::execute_error_with_weight(error.into(), checks_only))?;
            match with_storage_layer(|| Self::do_execute(pid, false)) {
                Ok(result) => {
                    // B9 keeper rebate: the crank advanced state (a successful
                    // execute always consumes the queue entry). Fail-soft — the
                    // rebate can never affect the crank result (08 §6.3).
                    if !Queue::<T>::contains_key(pid) {
                        T::KeeperRebate::rebate(&who, CrankClass::General);
                    }
                    let actual = Self::execute_actual_weight(result.charge);
                    debug_assert!(actual.all_lte(Self::execute_precharge()));
                    Ok(PostDispatchInfo {
                        actual_weight: Some(actual),
                        pays_fee: Pays::Yes,
                    })
                }
                Err(failure) => {
                    let actual = failure
                        .post_dispatch_charge
                        .map(Self::execute_actual_weight)
                        .unwrap_or(checks_only);
                    Err(Self::execute_error_with_weight(failure.error, actual))
                }
            }
        }

        /// Permissionless second phase of the authorized-upgrade flow.
        #[pallet::call_index(1)]
        #[pallet::weight(T::WeightInfo::apply_authorized_upgrade(code.len() as u32))]
        pub fn apply_authorized_upgrade(
            origin: OriginFor<T>,
            code: RuntimeCode<T>,
        ) -> DispatchResult {
            let _ = ensure_signed(origin)?;
            with_storage_layer(|| Self::do_apply_authorized_upgrade(code.into_inner()))
        }

        /// T22 keeper crank after the bounded T18 retry window.
        #[pallet::call_index(2)]
        #[pallet::weight(T::WeightInfo::expire_failed_execution())]
        pub fn expire_failed_execution(origin: OriginFor<T>, pid: ProposalId) -> DispatchResult {
            let who = ensure_signed(origin)?;
            let result = with_storage_layer(|| Self::do_expire_failed_execution(pid));
            if result.is_ok() && !Queue::<T>::contains_key(pid) {
                T::KeeperRebate::rebate(&who, CrankClass::General);
            }
            result
        }

        /// Sole ratify-track governance call (06 §2.2/§3.2).
        #[pallet::call_index(3)]
        #[pallet::weight(T::WeightInfo::ratify())]
        pub fn ratify(
            origin: OriginFor<T>,
            pid: ProposalId,
            referendum_index: u32,
        ) -> DispatchResult {
            T::RatifyOrigin::ensure_origin(origin)?;
            with_storage_layer(|| Self::do_ratify(pid, referendum_index))
        }

        /// Permissionless T16 cleanup for a deterministically stale,
        /// unratified-at-grace, or revoked-attestation queue entry.
        #[pallet::call_index(4)]
        #[pallet::weight(T::WeightInfo::reject_stale())]
        pub fn reject_stale(origin: OriginFor<T>, pid: ProposalId) -> DispatchResult {
            let who = ensure_signed(origin)?;
            let result = with_storage_layer(|| Self::do_reject_stale(pid));
            if result.is_ok() && !Queue::<T>::contains_key(pid) {
                T::KeeperRebate::rebate(&who, CrankClass::General);
            }
            result
        }

        /// Commit the pre-attested recovery Wasm carried by the same
        /// values-ratified CODE/META payload as its primary authorization.
        /// The call is useful only inside the guard's transient dispatch
        /// context; a bare custom-origin dispatch therefore still fails.
        #[pallet::call_index(5)]
        #[pallet::weight(T::WeightInfo::commit_recovery_image())]
        pub fn commit_recovery_image(
            origin: OriginFor<T>,
            hash: H256,
            len: u32,
            target_spec_version: u32,
            attestation_id: u32,
        ) -> DispatchResult {
            T::RecoveryCommitOrigin::ensure_origin(origin)?;
            with_storage_layer(|| {
                Self::do_commit_recovery_image(hash, len, target_spec_version, attestation_id)
            })
        }

        /// One-shot Phase-3→4 bridge. Bootstrap sudo may select only a passed
        /// shadow CODE/META mandate; all authorization checks and the sole
        /// internal-Root dispatch remain inside the guard (I-10).
        #[pallet::call_index(6)]
        #[pallet::weight(Pallet::<T>::execute_precharge().saturating_add(T::WeightInfo::authorize_phase_four()))]
        pub fn authorize_phase_four(
            origin: OriginFor<T>,
            pid: ProposalId,
            justification_hash: H256,
        ) -> DispatchResultWithPostInfo {
            T::PhaseFourBridgeOrigin::ensure_origin(origin)?;
            ensure!(
                justification_hash != [0_u8; 32],
                Error::<T>::JustificationMissing
            );
            ensure!(T::PhaseState::exact_phase_three(), Error::<T>::ShadowMode);
            ensure!(
                matches!(PhaseFourBridge::<T>::get(), PhaseFourBridgeState::Unused),
                Error::<T>::PhaseFourBridgeUsed
            );
            let queued = Queue::<T>::get(pid).ok_or(Error::<T>::NotFound)?;
            ensure!(
                matches!(queued.class, ProposalClass::Code | ProposalClass::Meta),
                Error::<T>::BadUpgradePayload
            );
            let charge = with_storage_layer(|| -> Result<ExecuteCharge, ExecuteFailure> {
                let result = Self::do_execute(pid, true)?;
                let pending = PendingUpgrade::<T>::get().ok_or(Error::<T>::BadUpgradePayload)?;
                let plan = result
                    .phase_four_plan
                    .ok_or(Error::<T>::BadUpgradePayload)?;
                PhaseFourBridge::<T>::put(PhaseFourBridgeState::Pending {
                    pid,
                    code_hash: pending.hash,
                    plan,
                });
                Self::deposit_event(Event::PhaseFourUpgradeAuthorized {
                    pid,
                    code_hash: pending.hash,
                    justification_hash,
                });
                Ok(result.charge)
            })
            .map_err(|failure| {
                let actual = failure
                    .post_dispatch_charge
                    .map(Self::execute_actual_weight)
                    .unwrap_or_else(|| T::WeightInfo::execute(MAX_CALLS_BOUND));
                Self::execute_error_with_weight(failure.error, actual)
            })?;
            Ok(PostDispatchInfo {
                actual_weight: Some(
                    Self::execute_actual_weight(charge)
                        .saturating_add(T::WeightInfo::authorize_phase_four()),
                ),
                pays_fee: Pays::No,
            })
        }

        /// Permissionless, one-image recovery qualification. This operational
        /// call is the only healthy-chain path that reads the full recovery
        /// Wasm; epoch screening and queue admission consume the immutable
        /// cached descriptor with bounded storage proofs.
        #[pallet::call_index(7)]
        #[pallet::weight((
            T::WeightInfo::qualify_recovery_image(T::MaxRuntimeCodeBytes::get()),
            DispatchClass::Operational,
        ))]
        pub fn qualify_recovery_image(origin: OriginFor<T>, pid: ProposalId) -> DispatchResult {
            let _ = ensure_signed(origin)?;
            with_storage_layer(|| Self::do_qualify_recovery_image(pid))
        }
    }

    #[pallet::extra_constants]
    impl<T: Config> Pallet<T> {
        #[pallet::constant_name(INTEGRATION_CONTRACT_VERSION)]
        fn integration_contract_version() -> u32 {
            INTEGRATION_CONTRACT_VERSION
        }
        #[pallet::constant_name(MaxLiveProposals)]
        fn max_live_proposals() -> u32 {
            MAX_QUEUE_BOUND
        }
        #[pallet::constant_name(MaxExecutionRecords)]
        fn max_execution_records() -> u32 {
            MAX_RECORDS_BOUND
        }
        #[pallet::constant_name(MaxCalls)]
        fn max_calls() -> u32 {
            MAX_CALLS_BOUND
        }
        #[pallet::constant_name(MaxPayloadBytes)]
        fn max_payload_bytes() -> u32 {
            MAX_PAYLOAD_BYTES
        }
        #[pallet::constant_name(DescriptorLeadTime)]
        fn descriptor_lead_time() -> BlockNumber {
            DESCRIPTOR_LEAD_TIME
        }
        #[pallet::constant_name(MaxRuntimeCodeBytes)]
        fn max_runtime_code_bytes() -> u32 {
            T::MaxRuntimeCodeBytes::get()
        }
        #[pallet::constant_name(ExecutionTimelockFloor)]
        fn execution_timelock_floor() -> [u32; 4] {
            kernel::EXECUTION_TIMELOCK_FLOORS_BLOCKS
        }
        #[pallet::constant_name(ExecutionGraceFloor)]
        fn execution_grace_floor() -> u32 {
            kernel::EXECUTION_GRACE_FLOOR_BLOCKS
        }
    }

    #[pallet::genesis_config]
    pub struct GenesisConfig<T: Config> {
        #[serde(skip)]
        pub _config: PhantomData<T>,
        /// PB-MIGRATION drill staging seed (09 §3.2; SQ-274). When `true`,
        /// genesis engages [`MigrationHalt`] so the guardian recovery playbook
        /// (`ActivatePlaybook { Migration, MigrationHalt }`) is dispatchable
        /// against the exact release runtime — with no production
        /// fault-injection surface (R-7). Engaging a freeze at genesis is the
        /// conservative direction (G-1). Every production preset leaves this
        /// `false` (its default), so the shipped chain never boots halted.
        pub migration_halt: bool,
    }

    impl<T: Config> Default for GenesisConfig<T> {
        fn default() -> Self {
            Self {
                _config: PhantomData,
                migration_halt: false,
            }
        }
    }

    #[pallet::genesis_build]
    impl<T: Config> BuildGenesisConfig for GenesisConfig<T> {
        fn build(&self) {
            let version = T::Version::get();
            if let Ok(spec_name) = futarchy_primitives::BoundedVec::<u8, 32>::try_from(
                version.spec_name.as_bytes().to_vec(),
            ) {
                CurrentSpecName::<T>::put(RuntimeVersionConstraint {
                    spec_name,
                    spec_version: version.spec_version,
                });
            }
            if self.migration_halt {
                MigrationHalt::<T>::put(true);
            }
        }
    }

    struct EpochAdapter<T>(PhantomData<T>);

    impl<T: Config> CoreEpochHandoff for EpochAdapter<T> {
        fn mark_executed(&mut self, pid: ProposalId) -> Result<(), CoreError> {
            T::Epoch::mark_executed(pid).map_err(|_| CoreError::DispatchFailed)
        }
        fn mark_failed_executed(&mut self, pid: ProposalId) -> Result<(), CoreError> {
            T::Epoch::mark_failed_executed(pid).map_err(|_| CoreError::DispatchFailed)
        }
        fn retry_exhausted_to_measurement(&mut self, pid: ProposalId) -> Result<(), CoreError> {
            T::Epoch::retry_exhausted_to_measurement(pid).map_err(|_| CoreError::DispatchFailed)
        }
        fn reject_or_stale(
            &mut self,
            pid: ProposalId,
            reason: RejectReason,
        ) -> Result<(), CoreError> {
            T::Epoch::reject_or_stale(pid, reason).map_err(|_| CoreError::DispatchFailed)
        }
    }

    #[derive(Clone, Debug)]
    struct BatchFailure {
        index: u8,
        error: DispatchError,
        consumed_inner: Weight,
    }

    impl From<DispatchError> for BatchFailure {
        fn from(error: DispatchError) -> Self {
            Self {
                index: 0,
                error,
                consumed_inner: Weight::zero(),
            }
        }
    }

    struct BatchDispatch {
        outcome: DispatchOutcomeCode,
        consumed_inner: Weight,
    }

    #[derive(Clone, Copy)]
    struct ExecuteCharge {
        actual_calls: u32,
        consumed_inner: Weight,
    }

    struct ExecuteResult {
        charge: ExecuteCharge,
        phase_four_plan: Option<PhaseFourPlan>,
    }

    struct ExecuteFailure {
        error: DispatchError,
        post_dispatch_charge: Option<ExecuteCharge>,
    }

    impl From<DispatchError> for ExecuteFailure {
        fn from(error: DispatchError) -> Self {
            Self {
                error,
                post_dispatch_charge: None,
            }
        }
    }

    impl<T: Config> From<Error<T>> for ExecuteFailure {
        fn from(error: Error<T>) -> Self {
            DispatchError::from(error).into()
        }
    }

    impl<T: Config> Pallet<T> {
        /// Runtime-internal producer for guardian `suspend_on_gate`.
        pub fn set_gate_suspension(epoch: EpochId) {
            GateSuspension::<T>::put(epoch);
        }

        /// Runtime-internal early-clear hook. Effective suspension is already
        /// lazy (`current epoch && breach`); this removes stale observability.
        pub fn clear_gate_suspension() {
            GateSuspension::<T>::kill();
        }

        /// Freeze a CODE/META referendum identity before queue admission. The
        /// epoch pallet validates the signed proposer and exact ratify-track
        /// preimage before calling this runtime-internal seam. If the proposal
        /// is already queued, the core queue mirror is updated transactionally;
        /// otherwise the bounded pending join survives until `enqueue`.
        pub fn bind_ratification(pid: ProposalId, referendum_index: u32) -> DispatchResult {
            with_storage_layer(|| {
                let payload_hash = T::Epoch::payload_hash(pid).ok_or(Error::<T>::NotFound)?;
                if let Some(bound) = PendingRatifications::<T>::get(pid) {
                    ensure!(bound == referendum_index, Error::<T>::NotRatified);
                }
                if let Some(record) = Ratifications::<T>::get(pid) {
                    ensure!(
                        record.payload_hash == payload_hash
                            && record.referendum_index == referendum_index,
                        Error::<T>::NotRatified
                    );
                    if let Some(queued) = Queue::<T>::get(pid) {
                        ensure!(
                            execution_guard_core::requires_ratification(queued.class)
                                && queued.payload_hash == payload_hash
                                && queued.ratify_ref == Some(referendum_index)
                                && queued.ratification_passed,
                            Error::<T>::NotRatified
                        );
                    }
                    PendingRatifications::<T>::remove(pid);
                    return Ok(());
                }
                if Queue::<T>::contains_key(pid) {
                    let mut state = Self::load()?;
                    state
                        .bind_ratification(pid, referendum_index)
                        .map_err(Self::map_core_error)?;
                    Self::persist(state)?;
                    ensure!(
                        PendingRatifications::<T>::contains_key(pid)
                            || PendingRatifications::<T>::count() < MAX_RATIFICATIONS_BOUND,
                        Error::<T>::QueueFull
                    );
                    PendingRatifications::<T>::insert(pid, referendum_index);
                    return Ok(());
                }
                ensure!(
                    PendingRatifications::<T>::contains_key(pid)
                        || PendingRatifications::<T>::count() < MAX_RATIFICATIONS_BOUND,
                    Error::<T>::QueueFull
                );
                if let Some(bound) = PendingRatifications::<T>::get(pid) {
                    ensure!(bound == referendum_index, Error::<T>::NotRatified);
                } else {
                    PendingRatifications::<T>::insert(pid, referendum_index);
                }
                Ok(())
            })
        }

        /// Internal enqueue endpoint. B1a's A8 adapter constructs `item` from
        /// pallet-epoch's adopted proposal and supplies the epoch-decision
        /// origin; it is not a public extrinsic (I-9).
        pub fn enqueue(
            origin: OriginFor<T>,
            mut item: StoredQueuedExecution,
            expedited: bool,
        ) -> DispatchResult {
            T::EnqueueAuthority::ensure_origin(origin)?;
            with_storage_layer(|| {
                let now = Self::now();
                ensure!(Queue::<T>::count() < MAX_QUEUE_BOUND, Error::<T>::QueueFull);
                ensure!(!Queue::<T>::contains_key(item.pid), Error::<T>::QueueFull);
                ensure!(
                    item.payload_len <= MAX_PAYLOAD_BYTES,
                    Error::<T>::PayloadTooLarge
                );
                ensure!(
                    T::Preimages::len(item.payload_hash) == Some(item.payload_len),
                    Error::<T>::BadPreimage
                );
                let bytes = T::Preimages::fetch(item.payload_hash, item.payload_len)
                    .ok_or(Error::<T>::BadPreimage)?;
                let actual_len =
                    u32::try_from(bytes.len()).map_err(|_| Error::<T>::PayloadTooLarge)?;
                ensure!(actual_len == item.payload_len, Error::<T>::BadPreimage);
                ensure!(
                    Self::hash_bytes(&bytes) == item.payload_hash,
                    Error::<T>::BadPreimage
                );
                ensure!(
                    T::Epoch::payload_hash(item.pid) == Some(item.payload_hash),
                    Error::<T>::BadPreimage
                );
                let current = Self::current_spec()?;
                ensure!(item.version_constraint == current, Error::<T>::StaleQueue);
                let maturity = now
                    .checked_add(T::Params::exec_timelock(item.class))
                    .ok_or(Error::<T>::Overflow)?;
                ensure!(item.maturity == maturity, Error::<T>::NotMature);
                let grace_end = maturity
                    .checked_add(T::Params::exec_grace(item.class))
                    .ok_or(Error::<T>::Overflow)?;
                ensure!(item.grace_end == grace_end, Error::<T>::GraceExpired);
                let calls = Self::decode_batch(&bytes)?;
                let phase_four_admission = T::PhaseState::exact_phase_three()
                    && T::Dispatcher::phase_four_plan(item.class, &calls).is_some();
                ensure!(
                    item.declared_domains.iter().all(|domain| {
                        (execution_guard_core::domain_allowed(item.class, *domain)
                            || (phase_four_admission && *domain == CallDomain::Code))
                            && !matches!(domain, CallDomain::InternalRootApplyUpgrade)
                    }),
                    Error::<T>::CapabilityDenied
                );
                let attestation_binding =
                    if matches!(item.class, ProposalClass::Code | ProposalClass::Meta) {
                        let id = item.attestation_id.ok_or(Error::<T>::AttestationMissing)?;
                        let artifact = T::Attestations::artifact_hash(id)
                            .ok_or(Error::<T>::AttestationMissing)?;
                        let committed_artifact =
                            Self::committed_artifact(&bytes, item.payload_hash)?;
                        ensure!(
                            artifact == committed_artifact
                                && T::Attestations::present_unrevoked_unchallenged(id)
                                && T::Attestations::has_quorum(item.pid, artifact),
                            Error::<T>::AttestationMissing
                        );
                        Some((id, artifact))
                    } else {
                        None
                    };
                let mut primary_hash = None;
                let mut recovery = None;
                for call in &calls {
                    if let Some(hash) = T::Dispatcher::authorize_upgrade_hash(call) {
                        ensure!(
                            primary_hash.replace(hash).is_none(),
                            Error::<T>::BadUpgradePayload
                        );
                    }
                    if let Some(descriptor) = T::Dispatcher::recovery_image_descriptor(call) {
                        ensure!(
                            recovery.replace(descriptor).is_none(),
                            Error::<T>::BadUpgradePayload
                        );
                    }
                }
                match (primary_hash, recovery) {
                    (Some(primary), Some(descriptor)) => {
                        ensure!(descriptor.hash != primary, Error::<T>::RecoveryImageInvalid);
                        ensure!(
                            descriptor.target_spec_version
                                == item.version_constraint.spec_version.saturating_add(2),
                            Error::<T>::UpgradeVersionMismatch
                        );
                        ensure!(
                            descriptor.len > 0
                                && descriptor.len <= T::MaxRuntimeCodeBytes::get()
                                && T::RecoveryImages::len(descriptor.hash) == Some(descriptor.len),
                            Error::<T>::BadPreimage
                        );
                        ensure!(
                            T::Attestations::artifact_hash(descriptor.attestation_id)
                                == Some(descriptor.hash)
                                && T::Attestations::present_unrevoked_unchallenged(
                                    descriptor.attestation_id,
                                )
                                && T::Attestations::has_quorum(item.pid, descriptor.hash),
                            Error::<T>::AttestationMissing
                        );
                        let retained_recovery = RerunRecoveryPins::<T>::get(item.pid);
                        ensure!(
                            retained_recovery.is_none() || retained_recovery == Some(descriptor),
                            Error::<T>::RecoveryImageInvalid
                        );
                        if retained_recovery.is_none() {
                            let qualified = QualifiedRecoveryImages::<T>::take(item.pid)
                                .ok_or(Error::<T>::RecoveryImageInvalid)?;
                            ensure!(
                                qualified.payload_hash == item.payload_hash
                                    && qualified.primary_hash == primary
                                    && qualified.version_constraint == item.version_constraint
                                    && qualified.descriptor == descriptor,
                                Error::<T>::RecoveryImageInvalid
                            );
                        } else {
                            ensure!(
                                !QualifiedRecoveryImages::<T>::contains_key(item.pid),
                                Error::<T>::RecoveryImageInvalid
                            );
                            RerunRecoveryPins::<T>::remove(item.pid);
                        }
                        QueuedRecoveryImages::<T>::insert(item.pid, descriptor);
                    }
                    (None, None) => ensure!(
                        !RerunRecoveryPins::<T>::contains_key(item.pid)
                            && !QualifiedRecoveryImages::<T>::contains_key(item.pid),
                        Error::<T>::RecoveryImageInvalid
                    ),
                    _ => return Err(Error::<T>::RecoveryImageMissing.into()),
                }
                ensure!(
                    !expedited
                        || (matches!(item.class, ProposalClass::Code | ProposalClass::Meta)
                            && (T::Guardian::ledger_freeze_active() || MigrationHalt::<T>::get())),
                    Error::<T>::FreezeActive
                );

                // A CODE/META referendum may be submitted before this
                // proposal reaches the execution queue. Merge its frozen
                // identity into the queue value without treating it as
                // passed; the proposer-side binding is consumed only after
                // the queue mirror has persisted successfully.
                if execution_guard_core::requires_ratification(item.class) {
                    if let Some(bound) = PendingRatifications::<T>::get(item.pid) {
                        ensure!(
                            item.ratify_ref.is_none() || item.ratify_ref == Some(bound),
                            Error::<T>::NotRatified
                        );
                        item.ratify_ref = Some(bound);
                    } else if let Some(record) = Ratifications::<T>::get(item.pid) {
                        ensure!(
                            record.payload_hash == item.payload_hash,
                            Error::<T>::NotRatified
                        );
                        ensure!(
                            item.ratify_ref.is_none()
                                || item.ratify_ref == Some(record.referendum_index),
                            Error::<T>::NotRatified
                        );
                        item.ratify_ref = Some(record.referendum_index);
                    } else {
                        // Queue admission must not manufacture a referendum
                        // identity. Only the proposer-side binding above (or
                        // an already-passed exact record) may populate it.
                        ensure!(item.ratify_ref.is_none(), Error::<T>::NotRatified);
                    }
                } else {
                    ensure!(
                        item.ratify_ref.is_none()
                            && !PendingRatifications::<T>::contains_key(item.pid),
                        Error::<T>::CapabilityDenied
                    );
                }

                item.ratification_passed = execution_guard_core::requires_ratification(item.class)
                    && Ratifications::<T>::get(item.pid).is_some_and(|record| {
                        record.payload_hash == item.payload_hash
                            && item.ratify_ref == Some(record.referendum_index)
                    });
                item.pre_upgrade_checkpoint = None;
                item.cancelled = false;
                item.failed_at = None;
                let pid = item.pid;
                let payload_hash = item.payload_hash;
                let meters = item.meters_declared.clone();
                let mut state = Self::load()?;
                state
                    .enqueue(GuardOrigin::EpochDecision, item.into())
                    .map_err(Self::map_core_error)?;
                for meter in meters {
                    ensure!(
                        !state.held_resources.contains(&(pid, meter)),
                        Error::<T>::ResourceLockMissing
                    );
                    state.held_resources.push((pid, meter));
                }
                let retained_pin = RerunPins::<T>::get(pid);
                ensure!(
                    retained_pin.is_none() || retained_pin == Some(payload_hash),
                    Error::<T>::BadPreimage
                );
                if retained_pin.is_none() {
                    T::Preimages::pin(payload_hash)?;
                } else {
                    RerunPins::<T>::remove(pid);
                }
                Self::persist(state)?;
                Expedited::<T>::insert(pid, expedited);
                if let Some(binding) = attestation_binding {
                    AttestationBindings::<T>::insert(pid, binding);
                }
                Ok(())
            })
        }

        /// Narrow compatibility helper for explicit pre-queue reaping.
        /// Production epoch paths use the universal idempotent
        /// `dequeue_terminal`, which also removes attestation auxiliaries and
        /// retained rerun pins when no Queue entry exists.
        pub fn reap_prequeue_ratification(origin: OriginFor<T>, pid: ProposalId) -> DispatchResult {
            T::EnqueueAuthority::ensure_origin(origin)?;
            ensure!(!Queue::<T>::contains_key(pid), Error::<T>::CapabilityDenied);
            with_storage_layer(|| Self::do_dequeue_terminal(pid))
        }

        /// Idempotent A8→A11 cleanup callback for every terminal path. Epoch is
        /// the sole proposal-state driver; this method removes the guard queue
        /// entry, held resource locks, expedited/attestation/ratification
        /// auxiliaries and any queue/rerun preimage pin. It also works for a
        /// never-queued ratification. A repeated call is a no-op.
        pub fn dequeue_terminal(pid: ProposalId) -> DispatchResult {
            with_storage_layer(|| Self::do_dequeue_terminal(pid))
        }

        /// Rerun-only dequeue. Queue locks/flags are released, while the
        /// ratification, attestation binding and exactly one payload pin live
        /// across the non-terminal cycle.
        pub fn dequeue_for_rerun(pid: ProposalId) -> DispatchResult {
            with_storage_layer(|| Self::do_dequeue_for_rerun(pid))
        }

        /// Cumulus callback for the relay `GoAhead` boundary. Scheduling an
        /// authorized validation function is not application: parachain-system
        /// writes `:code` only after the relay signal, then invokes the
        /// runtime's `OnSystemEvent::on_validation_code_applied` hook. B6 wires
        /// that hook here so pending/release state cannot claim application
        /// before the code is actually installed (09 §2.1(6), §2.3).
        pub fn validation_code_applied() -> DispatchResult {
            with_storage_layer(Self::do_validation_code_applied)
        }

        /// Runtime callback for an explicitly observed relay `Abort`. The
        /// runtime proves the Cumulus pending code vanished without installing
        /// the target before invoking this status-quo transition.
        pub fn validation_code_aborted() -> DispatchResult {
            with_storage_layer(Self::do_validation_code_aborted)
        }

        /// SDK migration-status callback. A completed MBM has no remaining
        /// rollback interval, so its application anchor must not outlive it.
        pub fn migration_completed() {
            PreMigrationAnchor::<T>::kill();
            if Self::release_recovery_image().is_err() {
                MigrationHalt::<T>::put(true);
            }
        }

        /// Validate and materialize the exact pre-authorized recovery image.
        /// The runtime calls this only from the mandatory parachain inherent,
        /// inside the same storage transaction that retires the stuck cursor
        /// and asks Cumulus to schedule the code.
        pub fn prepare_recovery_image() -> Result<(RecoveryImageCommitment, Vec<u8>), DispatchError>
        {
            let recovery = RecoveryImage::<T>::get().ok_or(Error::<T>::RecoveryImageMissing)?;
            let now = Self::now();
            let applicable_at = recovery
                .committed_at
                .checked_add(DESCRIPTOR_LEAD_TIME)
                .ok_or(Error::<T>::Overflow)?;
            ensure!(now >= applicable_at, Error::<T>::DescriptorLeadTime);
            ensure!(
                recovery.len > 0 && recovery.len <= T::MaxRuntimeCodeBytes::get(),
                Error::<T>::PayloadTooLarge
            );
            ensure!(
                T::RecoveryImages::len(recovery.hash) == Some(recovery.len),
                Error::<T>::BadPreimage
            );
            // Eligibility is frozen when the primary and recovery pair is
            // authorized. Once the primary has applied, mutable membership,
            // challenge, or revocation state must not strand a chain already
            // in terminal lockdown with no callable repair surface.
            let code = T::RecoveryImages::fetch(recovery.hash, recovery.len)
                .ok_or(Error::<T>::BadPreimage)?;
            ensure!(
                u32::try_from(code.len()).ok() == Some(recovery.len)
                    && Self::hash_bytes(&code) == recovery.hash,
                Error::<T>::BadPreimage
            );
            let observed = T::Dispatcher::observed_runtime_version(&code)
                .ok_or(Error::<T>::UpgradeVersionMismatch)?;
            let current = Self::current_spec()?;
            ensure!(
                observed.spec_name == current.spec_name
                    && observed.spec_version == recovery.target_spec_version,
                Error::<T>::UpgradeVersionMismatch
            );
            Ok((recovery, code))
        }

        pub fn recovery_scheduled(hash: H256) -> DispatchResult {
            let recovery = RecoveryImage::<T>::get().ok_or(Error::<T>::RecoveryImageMissing)?;
            ensure!(recovery.hash == hash, Error::<T>::UpgradeHashMismatch);
            Ok(())
        }

        /// Record the application boundary of the one-shot Phase-3→4 image.
        /// The state transition is performed only after Cumulus accepts the
        /// exact committed code for relay scheduling.
        pub fn phase_four_scheduled(hash: H256) -> DispatchResult {
            match PhaseFourBridge::<T>::get() {
                PhaseFourBridgeState::Pending {
                    pid,
                    code_hash,
                    plan,
                } if code_hash == hash => {
                    PhaseFourBridge::<T>::put(PhaseFourBridgeState::Scheduled {
                        pid,
                        code_hash,
                        plan,
                    });
                    Ok(())
                }
                _ => Err(Error::<T>::PhaseFourBridgeUsed.into()),
            }
        }

        pub fn recovery_code_applied(
            installed_hash: H256,
            installed_version: RuntimeVersionConstraint,
        ) -> DispatchResult {
            with_storage_layer(|| Self::do_recovery_code_applied(installed_hash, installed_version))
        }

        fn do_recovery_code_applied(
            installed_hash: H256,
            installed_version: RuntimeVersionConstraint,
        ) -> DispatchResult {
            let recovery = RecoveryImage::<T>::get().ok_or(Error::<T>::RecoveryImageMissing)?;
            ensure!(
                !T::MigrationStatus::cursor_exists(),
                Error::<T>::RecoveryImageInvalid
            );
            ensure!(
                installed_hash == recovery.hash,
                Error::<T>::UpgradeHashMismatch
            );
            ensure!(
                Self::current_spec()?.spec_name == installed_version.spec_name
                    && installed_version.spec_version == recovery.target_spec_version,
                Error::<T>::UpgradeVersionMismatch
            );
            match PendingUpgrade::<T>::get() {
                Some(pending) => ensure!(
                    pending.hash == recovery.primary_hash
                        && pending
                            .target_spec_version
                            .checked_add(1)
                            .is_some_and(|target| target == recovery.target_spec_version),
                    Error::<T>::RecoveryImageInvalid
                ),
                None => ensure!(
                    Self::current_spec()?
                        .spec_version
                        .checked_add(1)
                        .is_some_and(|target| target == recovery.target_spec_version),
                    Error::<T>::UpgradeVersionMismatch
                ),
            }
            let mut state = Self::load()?;
            state
                .complete_recovery_application(recovery.hash, recovery.target_spec_version)
                .map_err(Self::map_core_error)?;
            T::ReleaseChannel::on_upgrade_applied(recovery.target_spec_version)?;
            PendingUpgrade::<T>::kill();
            ScheduledUpgrade::<T>::kill();
            if matches!(
                PhaseFourBridge::<T>::get(),
                PhaseFourBridgeState::Scheduled {
                    pid, code_hash, ..
                }
                    if pid == recovery.pid && code_hash == recovery.primary_hash
            ) {
                PhaseFourBridge::<T>::put(PhaseFourBridgeState::Consumed);
            }
            CurrentSpecName::<T>::put(installed_version);
            PreMigrationAnchor::<T>::kill();
            PendingAnchorCapture::<T>::kill();
            Self::release_recovery_image()?;
            Self::persist(state)?;
            Self::deposit_event(Event::RecoveryImageApplied {
                recovery_hash: recovery.hash,
                spec_version: recovery.target_spec_version,
            });
            Ok(())
        }

        /// Deposit the PB-MIGRATION [`Event::MigrationHalted`] diagnostic
        /// (09 §3.2(4)). Called by the runtime halt-source bridge on the first
        /// activation of an execution-halt source, carrying the SDK cursor's
        /// exact bytes and reported failed step. Off the 02 §6 ingest set.
        pub fn note_migration_halted(cursor: MigrationHaltCursor, failed_step: Option<u32>) {
            Self::deposit_event(Event::MigrationHalted {
                cursor,
                failed_step,
            });
        }

        fn release_recovery_image() -> DispatchResult {
            if let Some(recovery) = RecoveryImage::<T>::get() {
                T::RecoveryImages::unpin(recovery.hash)?;
                RecoveryImage::<T>::kill();
            }
            Ok(())
        }

        pub fn queue_reject_reason(pid: ProposalId) -> Option<RejectReason> {
            let queued = Queue::<T>::get(pid)?;
            let now = Self::now();
            if Self::current_spec().is_ok_and(|current| current != queued.version_constraint) {
                return Some(RejectReason::StaleQueue);
            }
            if execution_guard_core::requires_ratification(queued.class)
                && now > queued.grace_end
                && !Self::ratification_valid(&queued)
            {
                return Some(RejectReason::NotRatified);
            }
            if matches!(queued.class, ProposalClass::Code | ProposalClass::Meta) {
                let valid = queued.attestation_id.is_some_and(|id| {
                    AttestationBindings::<T>::get(pid).is_some_and(|(bound_id, artifact)| {
                        id == bound_id
                            && T::Attestations::present_unrevoked_unchallenged(id)
                            && T::Attestations::has_record_quorum(pid, artifact)
                    })
                });
                if !valid {
                    return Some(RejectReason::AttestationMissing);
                }
                if QueuedRecoveryImages::<T>::get(pid).is_some_and(|recovery| {
                    T::Attestations::artifact_hash(recovery.attestation_id) != Some(recovery.hash)
                        || !T::Attestations::present_unrevoked_unchallenged(recovery.attestation_id)
                        || !T::Attestations::has_record_quorum(pid, recovery.hash)
                }) {
                    return Some(RejectReason::AttestationMissing);
                }
            }
            None
        }

        /// Read-only 02 §3 execution-queue projection. Hydration sorts by
        /// proposal id and [`ExecutionGuard::view`] single-homes ratification.
        /// `QueuedExecutionView.meters_clear` is `true` post-SQ-146 retirement of
        /// the inert `BlockedMeters` set; a live rate-meter preview is deferred
        /// (SQ-461). A missing/invalid runtime version is a G-1 empty sentinel
        /// rather than a partially trusted queue.
        pub fn queue_view() -> Vec<futarchy_primitives::QueuedExecutionView> {
            match Self::load() {
                Ok(guard) => guard.view(),
                Err(_) => Vec::new(),
            }
        }

        pub fn retry_exhausted(pid: ProposalId) -> bool {
            Queue::<T>::get(pid)
                .and_then(|queued| queued.failed_at)
                .is_some_and(|failed_at| Self::now() > failed_at.saturating_add(RETRY_WINDOW))
        }

        #[cfg(test)]
        pub(crate) fn seed_core(state: ExecutionGuard) -> DispatchResult {
            Self::persist(state)
        }

        fn do_execute(
            pid: ProposalId,
            allow_phase_four_bridge: bool,
        ) -> Result<ExecuteResult, ExecuteFailure> {
            let now = Self::now();
            let queued = Queue::<T>::get(pid).ok_or(Error::<T>::NotFound)?;
            ensure!(
                allow_phase_four_bridge || T::PhaseState::class_execution_enabled(queued.class),
                Error::<T>::ShadowMode
            );

            // 09 §1.2(1) queue state.
            ensure!(!queued.cancelled, Error::<T>::Cancelled);
            ensure!(queued.maturity <= now, Error::<T>::NotMature);
            match queued.failed_at {
                None => ensure!(now <= queued.grace_end, Error::<T>::GraceExpired),
                Some(failed_at) => ensure!(
                    now <= failed_at.saturating_add(RETRY_WINDOW),
                    Error::<T>::GraceExpired
                ),
            }

            // (2) exact preimage bytes; hash and length are re-derived.
            let noted_len =
                T::Preimages::len(queued.payload_hash).ok_or(Error::<T>::BadPreimage)?;
            ensure!(noted_len == queued.payload_len, Error::<T>::BadPreimage);
            ensure!(noted_len <= MAX_PAYLOAD_BYTES, Error::<T>::PayloadTooLarge);
            let bytes = T::Preimages::fetch(queued.payload_hash, queued.payload_len)
                .ok_or(Error::<T>::BadPreimage)?;
            let encoded_len =
                u32::try_from(bytes.len()).map_err(|_| Error::<T>::PayloadTooLarge)?;
            ensure!(encoded_len == queued.payload_len, Error::<T>::BadPreimage);
            ensure!(
                Self::hash_bytes(&bytes) == queued.payload_hash,
                Error::<T>::BadPreimage
            );

            // (3) frozen runtime version.
            ensure!(
                queued.version_constraint == Self::current_spec()?,
                Error::<T>::StaleQueue
            );

            // (4) ratification record bound to (pid, payload_hash).
            if execution_guard_core::requires_ratification(queued.class) {
                ensure!(Self::ratification_valid(&queued), Error::<T>::NotRatified);
            }

            // (5) the queue-time-validated attestation id remains present,
            // unrevoked, unchallenged and live-quorate. Its frozen artifact
            // binding is later matched to the exact authorize_upgrade hash.
            let attested_artifact =
                if matches!(queued.class, ProposalClass::Code | ProposalClass::Meta) {
                    let id = queued
                        .attestation_id
                        .ok_or(Error::<T>::AttestationMissing)?;
                    let (bound_id, artifact) =
                        AttestationBindings::<T>::get(pid).ok_or(Error::<T>::AttestationMissing)?;
                    ensure!(
                        bound_id == id
                            && T::Attestations::present_unrevoked_unchallenged(id)
                            && T::Attestations::has_record_quorum(pid, artifact),
                        Error::<T>::AttestationMissing
                    );
                    if let Some(recovery) = QueuedRecoveryImages::<T>::get(pid) {
                        ensure!(
                            T::Attestations::artifact_hash(recovery.attestation_id)
                                == Some(recovery.hash)
                                && T::Attestations::present_unrevoked_unchallenged(
                                    recovery.attestation_id,
                                )
                                && T::Attestations::has_record_quorum(pid, recovery.hash),
                            Error::<T>::AttestationMissing
                        );
                    }
                    Some(artifact)
                } else {
                    None
                };

            // (6) static class envelope plus the live constitution capability
            // table. Queue-time admission never freezes a capability on. The
            // exact decoded call is required for keyed/variant capabilities.
            let calls = Self::decode_batch(&bytes)?;
            let phase_four_plan = allow_phase_four_bridge
                .then(|| T::Dispatcher::phase_four_plan(queued.class, &calls))
                .flatten();
            let phase_four_payload = phase_four_plan.is_some();
            ensure!(
                !allow_phase_four_bridge || phase_four_payload,
                Error::<T>::BadUpgradePayload
            );
            ensure!(
                queued.declared_domains.iter().all(|domain| {
                    execution_guard_core::domain_allowed(queued.class, *domain)
                        || (phase_four_payload && *domain == CallDomain::Code)
                }),
                Error::<T>::CapabilityDenied
            );
            for call in &calls {
                let analysis = T::Dispatcher::rederive_call(call)
                    .map_err(|_| Error::<T>::BadDomainDeclaration)?;
                ensure!(
                    !analysis.domains.is_empty(),
                    Error::<T>::BadDomainDeclaration
                );
                ensure!(
                    analysis
                        .domains
                        .iter()
                        .all(|domain| queued.declared_domains.contains(domain)),
                    Error::<T>::BadDomainDeclaration
                );
                ensure!(
                    analysis.domains.iter().all(|domain| {
                        execution_guard_core::domain_allowed(queued.class, *domain)
                            || (phase_four_payload
                                && *domain == CallDomain::Code
                                && T::Dispatcher::recovery_image_descriptor(call).is_some())
                    }) && (phase_four_payload || T::Capabilities::call_enabled(queued.class, call)),
                    Error::<T>::CapabilityDenied
                );
            }

            // (7) rate meters (09 §1.2(7)). The `code.spacing` meter is checked
            // here; treasury/issuance meters are enforced by the dispatched
            // calls themselves. The former generic `BlockedMeters` resource set
            // was retired as inert — it never had a production writer (SQ-146).
            if queued
                .declared_domains
                .contains(&CallDomain::InternalRootAuthorizeUpgrade)
                && !Expedited::<T>::get(pid)
            {
                if let Some(last) = LastUpgradeAuthorized::<T>::get() {
                    let next = last
                        .checked_add(T::Params::code_spacing())
                        .ok_or(Error::<T>::Overflow)?;
                    ensure!(now >= next, Error::<T>::MetersBlocked);
                }
            }

            // (8) resource locks.
            let held = HeldResources::<T>::get();
            ensure!(
                queued
                    .meters_declared
                    .iter()
                    .all(|resource| held.contains(&(pid, *resource))),
                Error::<T>::ResourceLockMissing
            );

            // (9) guardian hold.
            ensure!(!T::Guardian::rerun_held(pid), Error::<T>::GuardianHold);
            ensure!(!T::Guardian::gate_suspended(), Error::<T>::GateSuspended);

            // (10) gate/freezes. Only the queue-time-frozen expedited lane may
            // treat its triggering ledger/migration freeze as satisfied.
            ensure!(
                !HardGateBreach::<T>::get()
                    && !DeadManFreeze::<T>::get()
                    && !T::Guardian::dead_man_freeze_active(),
                Error::<T>::FreezeActive
            );
            let triggering_freeze =
                T::Guardian::ledger_freeze_active() || MigrationHalt::<T>::get();
            ensure!(
                !triggering_freeze || Expedited::<T>::get(pid),
                Error::<T>::FreezeActive
            );

            // (11) bounded decode, actual domains, SafetyFilter and weight.
            ensure!(
                bytes.len() <= MAX_PAYLOAD_BYTES as usize,
                Error::<T>::PayloadTooLarge
            );
            let max_weight = Self::payload_weight_ceiling(T::BlockWeights::get().max_block);
            let mut total_weight = Weight::zero();
            let mut nested_calls = 0u32;
            let mut upgrade_hash = None;
            for call in &calls {
                let analysis = T::Dispatcher::rederive_call(call)
                    .map_err(|_| Error::<T>::BadDomainDeclaration)?;
                nested_calls = nested_calls
                    .checked_add(analysis.nested_calls)
                    .ok_or(Error::<T>::TooManyCalls)?;
                ensure!(nested_calls <= MAX_CALLS_BOUND, Error::<T>::TooManyCalls);
                let domains = analysis.domains;
                ensure!(!domains.is_empty(), Error::<T>::BadDomainDeclaration);
                let internal_root = domains.iter().any(|domain| {
                    matches!(
                        domain,
                        CallDomain::InternalRootAuthorizeUpgrade
                            | CallDomain::InternalRootApplyUpgrade
                    )
                });
                for domain in &domains {
                    ensure!(
                        queued.declared_domains.contains(domain),
                        Error::<T>::BadDomainDeclaration
                    );
                    ensure!(
                        execution_guard_core::domain_allowed(queued.class, *domain)
                            || (phase_four_payload
                                && *domain == CallDomain::Code
                                && T::Dispatcher::recovery_image_descriptor(call).is_some()),
                        Error::<T>::CapabilityDenied
                    );
                }
                ensure!(
                    phase_four_payload || T::Capabilities::call_enabled(queued.class, call),
                    Error::<T>::CapabilityDenied
                );

                if let Some(hash) = T::Dispatcher::authorize_upgrade_hash(call) {
                    ensure!(
                        internal_root
                            && domains.iter().all(|domain| {
                                *domain == CallDomain::InternalRootAuthorizeUpgrade
                            }),
                        Error::<T>::BadUpgradePayload
                    );
                    ensure!(upgrade_hash.is_none(), Error::<T>::BadUpgradePayload);
                    ensure!(
                        matches!(queued.class, ProposalClass::Code | ProposalClass::Meta),
                        Error::<T>::BadUpgradePayload
                    );
                    ensure!(
                        attested_artifact == Some(hash),
                        Error::<T>::AttestationMissing
                    );
                    ensure!(
                        PendingUpgrade::<T>::get().is_none(),
                        Error::<T>::PendingUpgradeExists
                    );
                    upgrade_hash = Some(hash);
                } else {
                    ensure!(!internal_root, Error::<T>::SafetyFilter);
                    ensure!(
                        (phase_four_payload
                            && (T::Dispatcher::recovery_image_descriptor(call).is_some()
                                || T::Dispatcher::is_phase_four_cap_commitment(call)))
                            || T::Dispatcher::safety_filter(queued.class, call),
                        Error::<T>::SafetyFilter
                    );
                    ensure!(
                        call.get_dispatch_info().class == DispatchClass::Normal,
                        Error::<T>::CapabilityDenied
                    );
                }
                total_weight = total_weight.saturating_add(call.get_dispatch_info().total_weight());
            }
            ensure!(
                total_weight.all_lte(max_weight),
                Error::<T>::CapabilityDenied
            );

            // Precompute every fallible authorization field before dispatch.
            // PB-MIGRATION anchoring occurs only after relay application.
            let (upgrade, next_spacing_history) = if let Some(hash) = upgrade_hash {
                let target_spec_version = queued
                    .version_constraint
                    .spec_version
                    .checked_add(1)
                    .ok_or(Error::<T>::Overflow)?;
                let applicable_at = now
                    .checked_add(DESCRIPTOR_LEAD_TIME)
                    .ok_or(Error::<T>::Overflow)?;
                let enforced_spacing = if Expedited::<T>::get(pid) {
                    0
                } else {
                    T::Params::code_spacing()
                };
                let mut history = UpgradeSpacingHistory::<T>::get().into_inner();
                if history.len() == MAX_EXECUTION_RECORDS {
                    history.rotate_left(1);
                    let _ = history.pop();
                }
                history.push((now, enforced_spacing));
                let history = StoredUpgradeSpacingHistory::try_from(history)
                    .map_err(|_| Error::<T>::Overflow)?;
                (
                    Some(UpgradeAuthorization {
                        hash,
                        target_spec_version,
                        applicable_at,
                    }),
                    Some(history),
                )
            } else {
                (None, None)
            };

            if let Some(upgrade) = upgrade {
                ExecutingUpgrade::<T>::put(ExecutingUpgradeContext {
                    pid,
                    primary_hash: upgrade.hash,
                    primary_target_spec_version: upgrade.target_spec_version,
                });
            }

            // Every check is complete. Only now may real dispatch occur.
            let dispatch =
                Self::dispatch_batch(calls.into_inner(), queued.class, phase_four_payload);
            ExecutingUpgrade::<T>::kill();
            let outcome = dispatch.outcome;
            let charge = ExecuteCharge {
                actual_calls: nested_calls,
                consumed_inner: dispatch.consumed_inner,
            };

            // Dispatch has consumed weight even if a later callback/storage
            // operation fails and the outer storage layer rolls state back.
            // Preserve that charge on every post-dispatch error so the refund
            // can never drop to checks-only after real inner execution.
            let post_dispatch: DispatchResult = (|| {
                let mut state = Self::load()?;
                let mut epoch = EpochAdapter::<T>(PhantomData);
                state
                    .complete_prevalidated(
                        GuardOrigin::Signed,
                        &mut epoch,
                        pid,
                        outcome,
                        now,
                        upgrade,
                    )
                    .map_err(Self::map_core_error)?;

                if outcome == DispatchOutcomeCode::Ok {
                    if let Some(upgrade) = upgrade {
                        ensure!(
                            RecoveryImage::<T>::get().is_some_and(|recovery| {
                                recovery.pid == pid && recovery.primary_hash == upgrade.hash
                            }),
                            Error::<T>::RecoveryImageMissing
                        );
                        T::ReleaseChannel::on_upgrade_authorized(upgrade.target_spec_version, now)?;
                        if let Some(history) = next_spacing_history {
                            UpgradeSpacingHistory::<T>::put(history);
                        }
                        LastUpgradeAuthorized::<T>::put(now);
                    }
                    T::Preimages::unpin(queued.payload_hash)?;
                    Self::cleanup_terminal(pid);
                }
                Self::persist(state)
            })();
            post_dispatch.map_err(|error| ExecuteFailure {
                error,
                post_dispatch_charge: Some(charge),
            })?;
            Ok(ExecuteResult {
                charge,
                phase_four_plan,
            })
        }

        fn do_commit_recovery_image(
            hash: H256,
            len: u32,
            target_spec_version: u32,
            attestation_id: u32,
        ) -> DispatchResult {
            let context = ExecutingUpgrade::<T>::get().ok_or(Error::<T>::RecoveryImageInvalid)?;
            ensure!(
                RecoveryImage::<T>::get().is_none(),
                Error::<T>::PendingUpgradeExists
            );
            let descriptor = RecoveryImageDescriptor {
                hash,
                len,
                target_spec_version,
                attestation_id,
            };
            ensure!(
                QueuedRecoveryImages::<T>::get(context.pid) == Some(descriptor),
                Error::<T>::RecoveryImageInvalid
            );
            ensure!(
                hash != context.primary_hash,
                Error::<T>::RecoveryImageInvalid
            );
            ensure!(
                context
                    .primary_target_spec_version
                    .checked_add(1)
                    .is_some_and(|expected| target_spec_version == expected),
                Error::<T>::UpgradeVersionMismatch
            );
            ensure!(
                T::Attestations::artifact_hash(attestation_id) == Some(hash)
                    && T::Attestations::present_unrevoked_unchallenged(attestation_id)
                    && T::Attestations::has_record_quorum(context.pid, hash),
                Error::<T>::AttestationMissing
            );
            QueuedRecoveryImages::<T>::remove(context.pid);
            RecoveryImage::<T>::put(RecoveryImageCommitment {
                pid: context.pid,
                primary_hash: context.primary_hash,
                hash,
                len,
                target_spec_version,
                attestation_id,
                committed_at: Self::now(),
            });
            Self::deposit_event(Event::RecoveryImageCommitted {
                pid: context.pid,
                primary_hash: context.primary_hash,
                recovery_hash: hash,
                target_spec_version,
            });
            Ok(())
        }

        fn dispatch_batch(
            calls: Vec<T::RuntimeCall>,
            class: ProposalClass,
            phase_four_payload: bool,
        ) -> BatchDispatch {
            let result: Result<Weight, BatchFailure> = with_storage_layer(|| {
                let mut consumed_inner = Weight::zero();
                for (index, call) in calls.into_iter().enumerate() {
                    let declared = call.get_dispatch_info().total_weight();
                    if phase_four_payload && T::Dispatcher::is_phase_four_cap_commitment(&call) {
                        // Charge the declared call weight conservatively, but
                        // defer the state mutation until PhaseFourTransition.
                        consumed_inner = consumed_inner.saturating_add(declared);
                        continue;
                    }
                    let dispatch = if let Some(hash) = T::Dispatcher::authorize_upgrade_hash(&call)
                    {
                        T::Dispatcher::dispatch_authorize_upgrade_post_info(hash)
                    } else {
                        T::Dispatcher::dispatch_with_class_origin_post_info(call, class)
                    };
                    match dispatch {
                        Ok(post_info) => {
                            consumed_inner = consumed_inner
                                .saturating_add(post_info.actual_weight.unwrap_or(declared));
                        }
                        Err(error) => {
                            consumed_inner = consumed_inner
                                .saturating_add(error.post_info.actual_weight.unwrap_or(declared));
                            return Err(BatchFailure {
                                index: index.saturated_into::<u8>(),
                                error: error.error,
                                consumed_inner,
                            });
                        }
                    }
                }
                Ok(consumed_inner)
            });
            match result {
                Ok(consumed_inner) => BatchDispatch {
                    outcome: DispatchOutcomeCode::Ok,
                    consumed_inner,
                },
                Err(failure) => BatchDispatch {
                    outcome: DispatchOutcomeCode::Failed {
                        call_index: failure.index,
                        error: Self::dispatch_error_code(&failure.error),
                    },
                    consumed_inner: failure.consumed_inner,
                },
            }
        }

        fn do_apply_authorized_upgrade(code: Vec<u8>) -> DispatchResult {
            let now = Self::now();
            let pending = PendingUpgrade::<T>::get().ok_or(Error::<T>::NoPendingUpgrade)?;
            ensure!(now >= pending.applicable_at, Error::<T>::DescriptorLeadTime);
            let code_hash = Self::hash_bytes(&code);
            ensure!(code_hash == pending.hash, Error::<T>::UpgradeHashMismatch);
            let observed = T::Dispatcher::observed_runtime_version(&code)
                .ok_or(Error::<T>::UpgradeVersionMismatch)?;
            let current = Self::current_spec()?;
            ensure!(
                observed.spec_name == current.spec_name
                    && observed.spec_version == pending.target_spec_version,
                Error::<T>::UpgradeVersionMismatch
            );

            // All stateless checks precede the permissionless system call.
            // Cumulus only schedules the candidate here. Pending/release state
            // is cleared by `validation_code_applied` at relay GoAhead.
            T::Dispatcher::dispatch_apply_authorized_upgrade(code)
        }

        fn do_validation_code_applied() -> DispatchResult {
            let now = Self::now();
            let pending = PendingUpgrade::<T>::get().ok_or(Error::<T>::NoPendingUpgrade)?;
            let mut state = Self::load()?;
            state
                .complete_upgrade_application(
                    GuardOrigin::Signed,
                    pending.hash,
                    pending.target_spec_version,
                    now,
                )
                .map_err(Self::map_core_error)?;
            T::ReleaseChannel::on_upgrade_applied(pending.target_spec_version)?;
            // A valid application is also the successful boundary of any
            // preceding recovery image: retire its old anchor, then let the
            // next on_initialize capture a fresh one only if this image
            // actually registered an MBM cursor.
            PreMigrationAnchor::<T>::kill();
            PendingAnchorCapture::<T>::put(true);
            ScheduledUpgrade::<T>::kill();
            if matches!(
                PhaseFourBridge::<T>::get(),
                PhaseFourBridgeState::Scheduled { code_hash, .. } if code_hash == pending.hash
            ) {
                PhaseFourBridge::<T>::put(PhaseFourBridgeState::Consumed);
            }
            if RecoveryImage::<T>::get().is_some_and(|recovery| recovery.hash == pending.hash) {
                Self::release_recovery_image()?;
            }
            Self::persist(state)
        }

        fn do_validation_code_aborted() -> DispatchResult {
            let pending = PendingUpgrade::<T>::get().ok_or(Error::<T>::NoPendingUpgrade)?;
            ensure!(
                ScheduledUpgrade::<T>::get() == Some(pending.hash),
                Error::<T>::NoPendingUpgrade
            );
            // Writer-(a) owns and unconditionally clears its pending channel
            // indication together with the guard state (I-30, G-1).
            T::ReleaseChannel::on_upgrade_aborted(pending.target_spec_version)?;
            PendingUpgrade::<T>::kill();
            // Abort precedes application. Consume only a defensive stale
            // latch; an older migration anchor remains the recovery audit
            // boundary until completion or a valid recovery image applies.
            PendingAnchorCapture::<T>::kill();
            ScheduledUpgrade::<T>::kill();
            if matches!(
                PhaseFourBridge::<T>::get(),
                PhaseFourBridgeState::Pending { code_hash, .. }
                    | PhaseFourBridgeState::Scheduled { code_hash, .. }
                    if code_hash == pending.hash
            ) {
                PhaseFourBridge::<T>::put(PhaseFourBridgeState::Unused);
            }
            Self::release_recovery_image()?;
            Self::deposit_event(Event::UpgradeAborted {
                code_hash: pending.hash,
            });
            Ok(())
        }

        fn do_expire_failed_execution(pid: ProposalId) -> DispatchResult {
            let queued = Queue::<T>::get(pid).ok_or(Error::<T>::NotFound)?;
            let failed_at = queued.failed_at.ok_or(Error::<T>::NotFound)?;
            ensure!(
                Self::now() > failed_at.saturating_add(RETRY_WINDOW),
                Error::<T>::RetryWindowOpen
            );
            // Epoch drives T22 and calls `dequeue_terminal` in the same
            // storage transaction. The fallback cleanup is idempotent for
            // seam doubles and already-terminal epoch callbacks.
            T::Epoch::retry_exhausted_to_measurement(pid)?;
            Self::dequeue_terminal(pid)
        }

        fn do_ratify(pid: ProposalId, referendum_index: u32) -> DispatchResult {
            ensure!(
                !Ratifications::<T>::contains_key(pid),
                Error::<T>::NotRatified
            );
            // In the production runtime CODE/META proposals must carry the
            // proposer-owned pending join created by `epoch.bind_ratification`.
            // Test seams that do not model proposal classes return `None` and
            // retain the core pallet's historical direct-ratify fixture path.
            if T::Epoch::requires_ratification(pid) == Some(true) {
                ensure!(
                    PendingRatifications::<T>::get(pid) == Some(referendum_index),
                    Error::<T>::NotRatified
                );
            }
            if let Some(bound) = PendingRatifications::<T>::get(pid) {
                ensure!(bound == referendum_index, Error::<T>::NotRatified);
            }
            ensure!(
                Ratifications::<T>::count() < MAX_RATIFICATIONS_BOUND,
                Error::<T>::QueueFull
            );
            let payload_hash = T::Epoch::payload_hash(pid).ok_or(Error::<T>::NotFound)?;
            if let Some(queued) = Queue::<T>::get(pid) {
                ensure!(queued.payload_hash == payload_hash, Error::<T>::BadPreimage);
                let mut state = Self::load()?;
                state
                    .ratify(GuardOrigin::RatifyTrack, pid, referendum_index)
                    .map_err(Self::map_core_error)?;
                Self::persist(state)?;
            } else {
                Self::deposit_event(Event::Ratified {
                    pid,
                    referendum_index,
                });
            }
            Ratifications::<T>::insert(
                pid,
                RatificationRecord {
                    referendum_index,
                    payload_hash,
                    ratified_at: Self::now(),
                },
            );
            PendingRatifications::<T>::remove(pid);
            Ok(())
        }

        fn do_qualify_recovery_image(pid: ProposalId) -> DispatchResult {
            ensure!(
                !Queue::<T>::contains_key(pid),
                Error::<T>::RecoveryImageInvalid
            );
            let (payload_hash, version_constraint) =
                T::Epoch::recovery_qualification_context(pid).ok_or(Error::<T>::NotFound)?;
            let payload_len = T::Preimages::len(payload_hash).ok_or(Error::<T>::BadPreimage)?;
            ensure!(
                payload_len <= MAX_PAYLOAD_BYTES,
                Error::<T>::PayloadTooLarge
            );
            let payload =
                T::Preimages::fetch(payload_hash, payload_len).ok_or(Error::<T>::BadPreimage)?;
            ensure!(
                u32::try_from(payload.len()).ok() == Some(payload_len)
                    && Self::hash_bytes(&payload) == payload_hash,
                Error::<T>::BadPreimage
            );
            let calls = Self::decode_batch(&payload)?;
            let mut primary_hash = None;
            let mut descriptor = None;
            for call in &calls {
                if let Some(hash) = T::Dispatcher::authorize_upgrade_hash(call) {
                    ensure!(
                        primary_hash.replace(hash).is_none(),
                        Error::<T>::BadUpgradePayload
                    );
                }
                if let Some(recovery) = T::Dispatcher::recovery_image_descriptor(call) {
                    ensure!(
                        descriptor.replace(recovery).is_none(),
                        Error::<T>::BadUpgradePayload
                    );
                }
            }
            let primary_hash = primary_hash.ok_or(Error::<T>::RecoveryImageMissing)?;
            let descriptor = descriptor.ok_or(Error::<T>::RecoveryImageMissing)?;
            let current = Self::current_spec()?;
            ensure!(version_constraint == current, Error::<T>::StaleQueue);
            ensure!(
                descriptor.hash != primary_hash
                    && descriptor.len > 0
                    && descriptor.len <= T::MaxRuntimeCodeBytes::get()
                    && current.spec_version.checked_add(2) == Some(descriptor.target_spec_version),
                Error::<T>::RecoveryImageInvalid
            );
            ensure!(
                T::RecoveryImages::len(descriptor.hash) == Some(descriptor.len),
                Error::<T>::BadPreimage
            );
            let recovery_code = T::RecoveryImages::fetch(descriptor.hash, descriptor.len)
                .ok_or(Error::<T>::BadPreimage)?;
            ensure!(
                u32::try_from(recovery_code.len()).ok() == Some(descriptor.len)
                    && Self::hash_bytes(&recovery_code) == descriptor.hash,
                Error::<T>::BadPreimage
            );
            let observed = T::Dispatcher::observed_runtime_version(&recovery_code)
                .ok_or(Error::<T>::UpgradeVersionMismatch)?;
            ensure!(
                observed.spec_name == current.spec_name
                    && observed.spec_version == descriptor.target_spec_version,
                Error::<T>::UpgradeVersionMismatch
            );
            ensure!(
                T::RecoveryImages::preflight_qualifies(&recovery_code),
                Error::<T>::RecoveryImageInvalid
            );
            ensure!(
                T::Attestations::artifact_hash(descriptor.attestation_id) == Some(descriptor.hash)
                    && T::Attestations::present_unrevoked_unchallenged(descriptor.attestation_id)
                    && T::Attestations::has_quorum(pid, descriptor.hash),
                Error::<T>::AttestationMissing
            );
            let qualified = QualifiedRecoveryImage {
                payload_hash,
                primary_hash,
                version_constraint,
                descriptor,
            };
            if let Some(existing) = QualifiedRecoveryImages::<T>::get(pid) {
                ensure!(existing == qualified, Error::<T>::RecoveryImageInvalid);
                return Ok(());
            }
            ensure!(
                QualifiedRecoveryImages::<T>::count() < MAX_QUALIFIED_RECOVERY_IMAGES_BOUND,
                Error::<T>::QueueFull
            );
            T::RecoveryImages::pin(descriptor.hash)?;
            QualifiedRecoveryImages::<T>::insert(pid, qualified);
            Self::deposit_event(Event::RecoveryImageQualified {
                pid,
                recovery_hash: descriptor.hash,
                target_spec_version: descriptor.target_spec_version,
            });
            Ok(())
        }

        fn do_reject_stale(pid: ProposalId) -> DispatchResult {
            ensure!(Queue::<T>::contains_key(pid), Error::<T>::NotFound);
            let reason = Self::queue_reject_reason(pid).ok_or(Error::<T>::StaleQueue)?;
            // Epoch drives T16/T21 and calls back into the idempotent guard
            // cleanup. Do not load→persist stale guard state around that call,
            // or the removed queue entry would be reinserted.
            T::Epoch::reject_or_stale(pid, reason)?;
            Self::dequeue_terminal(pid)?;
            Self::deposit_event(Event::Rejected { pid, reason });
            Ok(())
        }

        fn do_dequeue_terminal(pid: ProposalId) -> DispatchResult {
            ensure!(
                !(QueuedRecoveryImages::<T>::contains_key(pid)
                    && RerunRecoveryPins::<T>::contains_key(pid))
                    && !(QualifiedRecoveryImages::<T>::contains_key(pid)
                        && (QueuedRecoveryImages::<T>::contains_key(pid)
                            || RerunRecoveryPins::<T>::contains_key(pid))),
                Error::<T>::RecoveryImageInvalid
            );
            if let Some(qualified) = QualifiedRecoveryImages::<T>::take(pid) {
                T::RecoveryImages::unpin(qualified.descriptor.hash)?;
            } else if let Some(recovery) = QueuedRecoveryImages::<T>::get(pid) {
                T::RecoveryImages::unpin(recovery.hash)?;
                QueuedRecoveryImages::<T>::remove(pid);
            } else if let Some(recovery) = RerunRecoveryPins::<T>::take(pid) {
                T::RecoveryImages::unpin(recovery.hash)?;
            }
            if let Some(queued) = Queue::<T>::get(pid) {
                let mut state = Self::load()?;
                state.dequeue_terminal(pid);
                T::Preimages::unpin(queued.payload_hash)?;
                Self::persist(state)?;
            } else if let Some(payload_hash) = RerunPins::<T>::take(pid) {
                T::Preimages::unpin(payload_hash)?;
            }
            Self::cleanup_terminal(pid);
            Ok(())
        }

        fn do_dequeue_for_rerun(pid: ProposalId) -> DispatchResult {
            let queued = Queue::<T>::get(pid).ok_or(Error::<T>::NotFound)?;
            ensure!(
                !RerunPins::<T>::contains_key(pid) && RerunPins::<T>::count() < MAX_QUEUE_BOUND,
                Error::<T>::QueueFull
            );
            if QueuedRecoveryImages::<T>::contains_key(pid) {
                ensure!(
                    !RerunRecoveryPins::<T>::contains_key(pid)
                        && RerunRecoveryPins::<T>::count() < MAX_QUEUE_BOUND,
                    Error::<T>::QueueFull
                );
            }
            let mut state = Self::load()?;
            state.dequeue_for_rerun(pid);
            Self::persist(state)?;
            Expedited::<T>::remove(pid);
            RerunPins::<T>::insert(pid, queued.payload_hash);
            if let Some(recovery) = QueuedRecoveryImages::<T>::take(pid) {
                RerunRecoveryPins::<T>::insert(pid, recovery);
            }
            Ok(())
        }

        fn cleanup_terminal(pid: ProposalId) {
            Ratifications::<T>::remove(pid);
            PendingRatifications::<T>::remove(pid);
            Expedited::<T>::remove(pid);
            AttestationBindings::<T>::remove(pid);
            QualifiedRecoveryImages::<T>::remove(pid);
            QueuedRecoveryImages::<T>::remove(pid);
        }

        fn ratification_valid(queued: &StoredQueuedExecution) -> bool {
            queued.ratification_passed
                && Ratifications::<T>::get(queued.pid).is_some_and(|record| {
                    record.payload_hash == queued.payload_hash
                        && queued.ratify_ref == Some(record.referendum_index)
                })
        }

        fn current_spec() -> Result<RuntimeVersionConstraint, DispatchError> {
            CurrentSpecName::<T>::get().ok_or(Error::<T>::StaleQueue.into())
        }

        fn now() -> BlockNumber {
            frame_system::Pallet::<T>::block_number().saturated_into::<BlockNumber>()
        }

        pub(crate) fn payload_weight_ceiling(max_block: Weight) -> Weight {
            max_block
                .saturating_mul(futarchy_primitives::kernel::PROP_MAX_WEIGHT_NUM)
                .saturating_div(futarchy_primitives::kernel::PROP_MAX_WEIGHT_DEN)
        }

        pub(crate) fn execute_precharge() -> Weight {
            T::WeightInfo::execute(MAX_CALLS_BOUND).saturating_add(Self::payload_weight_ceiling(
                T::BlockWeights::get().max_block,
            ))
        }

        fn execute_actual_weight(charge: ExecuteCharge) -> Weight {
            T::WeightInfo::execute(charge.actual_calls).saturating_add(charge.consumed_inner)
        }

        fn execute_error_with_weight(
            error: DispatchError,
            actual_weight: Weight,
        ) -> DispatchErrorWithPostInfo {
            DispatchErrorWithPostInfo {
                post_info: PostDispatchInfo {
                    actual_weight: Some(actual_weight),
                    pays_fee: Pays::Yes,
                },
                error,
            }
        }

        fn hash_bytes(bytes: &[u8]) -> H256 {
            T::Hashing::hash(bytes).into()
        }

        pub fn decode_batch(bytes: &[u8]) -> Result<RuntimeBatch<T>, DispatchError> {
            let mut prefix = bytes;
            let call_count = Compact::<u32>::decode(&mut prefix)
                .map_err(|_| Error::<T>::BadPreimage)?
                .0;
            ensure!(call_count <= MAX_CALLS_BOUND, Error::<T>::TooManyCalls);
            // Depth-limit the recursive decode of the preimage-sourced batch: the
            // element type `RuntimeCall` nests (utility.batch/proxy/multisig/sudo),
            // and the `call_count` guard above bounds only the *outer* count, not
            // nesting depth. Without this, an adversarial hash-committed preimage
            // encoding one deeply-nested call (≤ MAX_BYTES) would recurse in
            // `Decode` until the wasm stack-height trap / native stack abort —
            // a G-1 violation in audit-scope-A code. `decode_all_with_depth_limit`
            // also enforces full-consumption (replacing the trailing-bytes check).
            // Over-deep input fails closed to `BadPreimage`. (15 §4.5 / SQ-225.)
            let mut input = bytes;
            let calls = RuntimeBatch::<T>::decode_all_with_depth_limit(
                futarchy_primitives::kernel::MAX_PAYLOAD_DECODE_DEPTH,
                &mut input,
            )
            .map_err(|_| Error::<T>::BadPreimage)?;
            Ok(calls)
        }

        /// Queue-time artifact binding. Upgrade proposals attest the hash in
        /// the exact allowlisted authorize call; other attested META payloads
        /// bind to the committed batch hash. A second authorize call is never
        /// an admissible commitment.
        fn committed_artifact(bytes: &[u8], payload_hash: H256) -> Result<H256, DispatchError> {
            let calls = Self::decode_batch(bytes)?;
            let mut artifact = None;
            for call in &calls {
                if let Some(hash) = T::Dispatcher::authorize_upgrade_hash(call) {
                    ensure!(artifact.is_none(), Error::<T>::BadUpgradePayload);
                    artifact = Some(hash);
                }
            }
            Ok(artifact.unwrap_or(payload_hash))
        }

        fn dispatch_error_code(error: &DispatchError) -> [u8; 4] {
            let mut code = [0u8; 4];
            for (slot, byte) in code.iter_mut().zip(error.encode().into_iter()) {
                *slot = byte;
            }
            code
        }

        fn load() -> Result<ExecutionGuard, DispatchError> {
            let mut queue = Queue::<T>::iter_values()
                .map(QueuedExecution::from)
                .collect::<Vec<_>>();
            queue.sort_by_key(|queued| queued.pid);
            Ok(ExecutionGuard {
                queue,
                records: ExecutionRecords::<T>::get().into_inner(),
                pending_upgrade: PendingUpgrade::<T>::get(),
                current_spec_name: Self::current_spec()?,
                held_resources: HeldResources::<T>::get().into_inner(),
                gate_suspended: T::Guardian::gate_suspended(),
                hard_gate_breach: HardGateBreach::<T>::get(),
                dead_man_freeze: DeadManFreeze::<T>::get(),
                migration_halt: MigrationHalt::<T>::get(),
                events: Vec::new(),
            })
        }

        fn persist(mut state: ExecutionGuard) -> DispatchResult {
            state.try_state().map_err(Self::map_core_error)?;
            let queue = state
                .queue
                .iter()
                .cloned()
                .map(StoredQueuedExecution::try_from)
                .collect::<Result<Vec<_>, _>>()
                .map_err(Self::map_core_error)?;
            ensure!(queue.len() <= MAX_QUEUE, Error::<T>::QueueFull);
            let records = StoredRecords::try_from(state.records.clone())
                .map_err(|_| Error::<T>::QueueFull)?;
            let held = StoredHeldResources::try_from(state.held_resources.clone())
                .map_err(|_| Error::<T>::TooManyLocks)?;

            let old = Queue::<T>::iter_keys().collect::<Vec<_>>();
            for pid in old {
                Queue::<T>::remove(pid);
            }
            for queued in queue {
                Queue::<T>::insert(queued.pid, queued);
            }
            ExecutionRecords::<T>::put(records);
            match state.pending_upgrade {
                Some(pending) => PendingUpgrade::<T>::put(pending),
                None => PendingUpgrade::<T>::kill(),
            }
            CurrentSpecName::<T>::put(state.current_spec_name);
            HeldResources::<T>::put(held);
            HardGateBreach::<T>::put(state.hard_gate_breach);
            DeadManFreeze::<T>::put(state.dead_man_freeze);
            MigrationHalt::<T>::put(state.migration_halt);
            for event in core::mem::take(&mut state.events) {
                Self::deposit_core_event(event);
            }
            // The queue's hard maximum is 32 while the treasury mirror accepts
            // 64 entries (13 §4), so a healthy adapter cannot overflow. Keep the
            // queue mutation fail-soft, but alarm and poison spendable NAV if
            // decoding, arithmetic, or that structural bound ever drifts.
            if T::PendingOutflowSync::sync_pending_outflows().is_err() {
                Self::deposit_event(Event::PendingOutflowSyncFailed {
                    queued: Queue::<T>::count(),
                    fail_static: T::PendingOutflowSync::force_fail_static(),
                });
            }
            Ok(())
        }

        fn deposit_core_event(event: CoreEvent) {
            let event = match event {
                CoreEvent::Enqueued { pid, maturity } => Event::Enqueued { pid, maturity },
                CoreEvent::Ratified {
                    pid,
                    referendum_index,
                } => Event::Ratified {
                    pid,
                    referendum_index,
                },
                CoreEvent::Executed { pid, record } => Event::Executed { pid, record },
                CoreEvent::ExecutionFailed { pid, outcome } => {
                    Event::ExecutionFailed { pid, outcome }
                }
                CoreEvent::Rejected { pid, reason } => Event::Rejected { pid, reason },
                CoreEvent::UpgradeAuthorized {
                    code_hash,
                    authorized_at,
                    applicable_at: _,
                } => Event::UpgradeAuthorized {
                    code_hash,
                    authorized_at,
                },
                CoreEvent::UpgradeApplied {
                    code_hash,
                    spec_version,
                } => Event::UpgradeApplied {
                    code_hash,
                    spec_version,
                },
                CoreEvent::PreimageUnpinned { pid, payload_hash } => {
                    Event::PreimageUnpinned { pid, payload_hash }
                }
            };
            Self::deposit_event(event);
        }

        pub fn do_try_state() -> Result<(), TryRuntimeError> {
            let state = Self::load()
                .map_err(|_| TryRuntimeError::Other("execution guard current version is absent"))?;
            state
                .try_state()
                .map_err(|_| TryRuntimeError::Other("execution guard core bounds failed"))?;
            let actual_queue_count = Queue::<T>::iter_keys().count();
            let actual_ratification_count = Ratifications::<T>::iter_keys().count();
            let actual_pending_ratification_count = PendingRatifications::<T>::iter_keys().count();
            let actual_rerun_pin_count = RerunPins::<T>::iter_keys().count();
            if Queue::<T>::count() > MAX_QUEUE_BOUND
                || usize::try_from(Queue::<T>::count()).ok() != Some(actual_queue_count)
                || Ratifications::<T>::count() > MAX_RATIFICATIONS_BOUND
                || usize::try_from(Ratifications::<T>::count()).ok()
                    != Some(actual_ratification_count)
                || PendingRatifications::<T>::count() > MAX_RATIFICATIONS_BOUND
                || usize::try_from(PendingRatifications::<T>::count()).ok()
                    != Some(actual_pending_ratification_count)
                || RerunPins::<T>::count() > MAX_QUEUE_BOUND
                || usize::try_from(RerunPins::<T>::count()).ok() != Some(actual_rerun_pin_count)
                || ExecutionRecords::<T>::get().len() > MAX_EXECUTION_RECORDS
            {
                return Err(TryRuntimeError::Other(
                    "execution guard collection bound exceeded",
                ));
            }
            for (pid, queued) in Queue::<T>::iter() {
                if queued.pre_upgrade_checkpoint.is_some() {
                    return Err(TryRuntimeError::Other(
                        "execution guard retired queue checkpoint is not inert",
                    ));
                }
                if T::Epoch::is_terminal(pid) {
                    return Err(TryRuntimeError::Other(
                        "execution guard Queue retains terminal epoch proposal",
                    ));
                }
                if pid != queued.pid {
                    return Err(TryRuntimeError::Other("execution guard Queue key mismatch"));
                }
                if !Expedited::<T>::contains_key(pid) {
                    return Err(TryRuntimeError::Other(
                        "execution guard queue expedited marker is absent",
                    ));
                }
                if queued.maturity > queued.grace_end
                    || queued.failed_at.is_some_and(|failed_at| {
                        failed_at < queued.maturity || failed_at > Self::now()
                    })
                {
                    return Err(TryRuntimeError::Other(
                        "execution guard queue timing shape is invalid",
                    ));
                }
                let requires_ratification =
                    execution_guard_core::requires_ratification(queued.class);
                if !requires_ratification
                    && (queued.ratify_ref.is_some() || queued.ratification_passed)
                {
                    return Err(TryRuntimeError::Other(
                        "execution guard queue ratification shape is invalid",
                    ));
                }
                let ratification = Ratifications::<T>::get(pid);
                if requires_ratification
                    && queued.ratify_ref.is_some()
                    && !queued.ratification_passed
                    && PendingRatifications::<T>::get(pid) != queued.ratify_ref
                {
                    return Err(TryRuntimeError::Other(
                        "execution guard queue ratification identity is unbound",
                    ));
                }
                if queued.ratification_passed
                    != ratification.is_some_and(|record| {
                        record.payload_hash == queued.payload_hash
                            && queued.ratify_ref == Some(record.referendum_index)
                    })
                {
                    return Err(TryRuntimeError::Other(
                        "execution guard ratification binding mismatch",
                    ));
                }
                if queued
                    .meters_declared
                    .iter()
                    .any(|resource| !HeldResources::<T>::get().contains(&(pid, *resource)))
                {
                    return Err(TryRuntimeError::Other(
                        "execution guard queued resource lock is absent",
                    ));
                }
                if matches!(queued.class, ProposalClass::Code | ProposalClass::Meta) {
                    if !queued.attestation_id.is_some_and(|id| {
                        AttestationBindings::<T>::get(pid)
                            .is_some_and(|(bound_id, _)| bound_id == id)
                    }) {
                        return Err(TryRuntimeError::Other(
                            "execution guard attestation binding is absent",
                        ));
                    }
                    let payload = T::Preimages::fetch(queued.payload_hash, queued.payload_len)
                        .ok_or(TryRuntimeError::Other(
                            "execution guard queued upgrade payload is absent",
                        ))?;
                    let calls = Self::decode_batch(&payload).map_err(|_| {
                        TryRuntimeError::Other(
                            "execution guard queued upgrade payload is undecodable",
                        )
                    })?;
                    let mut primary = None;
                    let mut recovery = None;
                    for call in &calls {
                        if let Some(hash) = T::Dispatcher::authorize_upgrade_hash(call) {
                            if primary.replace(hash).is_some() {
                                return Err(TryRuntimeError::Other(
                                    "execution guard queued upgrade has duplicate primary",
                                ));
                            }
                        }
                        if let Some(descriptor) = T::Dispatcher::recovery_image_descriptor(call) {
                            if recovery.replace(descriptor).is_some() {
                                return Err(TryRuntimeError::Other(
                                    "execution guard queued upgrade has duplicate recovery",
                                ));
                            }
                        }
                    }
                    let exact_pair = primary.zip(recovery).is_some_and(|(primary, recovery)| {
                        primary != recovery.hash
                            && queued.version_constraint.spec_version.checked_add(2)
                                == Some(recovery.target_spec_version)
                            && QueuedRecoveryImages::<T>::get(pid) == Some(recovery)
                    });
                    if !exact_pair {
                        return Err(TryRuntimeError::Other(
                            "execution guard queued upgrade recovery binding is invalid",
                        ));
                    }
                } else if AttestationBindings::<T>::contains_key(pid) {
                    return Err(TryRuntimeError::Other(
                        "execution guard non-attested queue has a binding",
                    ));
                } else if QueuedRecoveryImages::<T>::contains_key(pid) {
                    return Err(TryRuntimeError::Other(
                        "execution guard non-upgrade queue has a recovery image",
                    ));
                }
            }
            for (pid, record) in Ratifications::<T>::iter() {
                if PendingRatifications::<T>::contains_key(pid) {
                    return Err(TryRuntimeError::Other(
                        "execution guard ratification is both pending and passed",
                    ));
                }
                let queued_binding = Queue::<T>::get(pid).is_some_and(|queued| {
                    queued.payload_hash == record.payload_hash
                        && queued.ratify_ref == Some(record.referendum_index)
                });
                if !queued_binding && T::Epoch::payload_hash(pid) != Some(record.payload_hash) {
                    return Err(TryRuntimeError::Other(
                        "execution guard Ratifications commitment is absent",
                    ));
                }
            }
            for (pid, bound) in PendingRatifications::<T>::iter() {
                let valid_queue_binding = Queue::<T>::get(pid).is_some_and(|queued| {
                    execution_guard_core::requires_ratification(queued.class)
                        && !queued.ratification_passed
                        && queued.ratify_ref == Some(bound)
                });
                if (!valid_queue_binding && Queue::<T>::contains_key(pid))
                    || Ratifications::<T>::contains_key(pid)
                    || T::Epoch::payload_hash(pid).is_none()
                    || T::Epoch::requires_ratification(pid) == Some(false)
                {
                    return Err(TryRuntimeError::Other(
                        "execution guard orphan pending ratification",
                    ));
                }
            }
            for (pid, _) in AttestationBindings::<T>::iter() {
                if !Queue::<T>::contains_key(pid) && !RerunPins::<T>::contains_key(pid) {
                    return Err(TryRuntimeError::Other(
                        "execution guard orphan attestation binding",
                    ));
                }
            }
            for (pid, payload_hash) in RerunPins::<T>::iter() {
                if Queue::<T>::contains_key(pid)
                    || T::Epoch::payload_hash(pid) != Some(payload_hash)
                {
                    return Err(TryRuntimeError::Other(
                        "execution guard rerun pin is orphaned",
                    ));
                }
            }
            for (pid, _) in Expedited::<T>::iter() {
                if !Queue::<T>::contains_key(pid) {
                    return Err(TryRuntimeError::Other(
                        "execution guard orphan expedited marker",
                    ));
                }
            }
            if ExecutingUpgrade::<T>::exists() {
                return Err(TryRuntimeError::Other(
                    "execution guard transient upgrade context escaped dispatch",
                ));
            }
            let qualified_recovery_count = QualifiedRecoveryImages::<T>::iter_keys().count();
            if QualifiedRecoveryImages::<T>::count() > MAX_QUALIFIED_RECOVERY_IMAGES_BOUND
                || usize::try_from(QualifiedRecoveryImages::<T>::count()).ok()
                    != Some(qualified_recovery_count)
            {
                return Err(TryRuntimeError::Other(
                    "execution guard qualified recovery-image bound exceeded",
                ));
            }
            for (pid, qualified) in QualifiedRecoveryImages::<T>::iter() {
                if Queue::<T>::contains_key(pid)
                    || RerunRecoveryPins::<T>::contains_key(pid)
                    || QueuedRecoveryImages::<T>::contains_key(pid)
                    || T::Epoch::recovery_qualification_context(pid)
                        != Some((qualified.payload_hash, qualified.version_constraint.clone()))
                    || qualified.descriptor.hash == qualified.primary_hash
                    || qualified.descriptor.len == 0
                    || qualified.descriptor.len > T::MaxRuntimeCodeBytes::get()
                    || !T::RecoveryImages::is_pinned(qualified.descriptor.hash)
                    || qualified.version_constraint.spec_version.checked_add(2)
                        != Some(qualified.descriptor.target_spec_version)
                    || T::RecoveryImages::len(qualified.descriptor.hash)
                        != Some(qualified.descriptor.len)
                {
                    return Err(TryRuntimeError::Other(
                        "execution guard qualified recovery image is invalid",
                    ));
                }
            }
            let queued_recovery_count = QueuedRecoveryImages::<T>::iter_keys().count();
            if queued_recovery_count > MAX_QUEUE_BOUND as usize {
                return Err(TryRuntimeError::Other(
                    "execution guard queued recovery-image bound exceeded",
                ));
            }
            for (pid, recovery) in QueuedRecoveryImages::<T>::iter() {
                let Some(queued) = Queue::<T>::get(pid) else {
                    return Err(TryRuntimeError::Other(
                        "execution guard orphan queued recovery image",
                    ));
                };
                if !matches!(queued.class, ProposalClass::Code | ProposalClass::Meta)
                    || recovery.len == 0
                    || recovery.len > T::MaxRuntimeCodeBytes::get()
                    || !T::RecoveryImages::is_pinned(recovery.hash)
                    || T::RecoveryImages::len(recovery.hash) != Some(recovery.len)
                    || queued.version_constraint.spec_version.checked_add(2)
                        != Some(recovery.target_spec_version)
                {
                    return Err(TryRuntimeError::Other(
                        "execution guard queued recovery image is invalid",
                    ));
                }
            }
            let rerun_recovery_count = RerunRecoveryPins::<T>::iter_keys().count();
            if RerunRecoveryPins::<T>::count() > MAX_QUEUE_BOUND
                || usize::try_from(RerunRecoveryPins::<T>::count()).ok()
                    != Some(rerun_recovery_count)
            {
                return Err(TryRuntimeError::Other(
                    "execution guard rerun recovery-image bound exceeded",
                ));
            }
            for (pid, recovery) in RerunRecoveryPins::<T>::iter() {
                if Queue::<T>::contains_key(pid)
                    || QueuedRecoveryImages::<T>::contains_key(pid)
                    || !RerunPins::<T>::contains_key(pid)
                    || recovery.len == 0
                    || recovery.len > T::MaxRuntimeCodeBytes::get()
                    || !T::RecoveryImages::is_pinned(recovery.hash)
                    || T::RecoveryImages::len(recovery.hash) != Some(recovery.len)
                {
                    return Err(TryRuntimeError::Other(
                        "execution guard rerun recovery pin is orphaned",
                    ));
                }
            }
            let held = HeldResources::<T>::get();
            for (index, pair) in held.iter().enumerate() {
                if held.iter().take(index).any(|seen| seen == pair)
                    || !Queue::<T>::get(pair.0)
                        .is_some_and(|queued| queued.meters_declared.contains(&pair.1))
                {
                    return Err(TryRuntimeError::Other(
                        "execution guard duplicate/orphan resource lock",
                    ));
                }
            }
            if PreMigrationAnchor::<T>::get().is_some()
                && !PendingAnchorCapture::<T>::get()
                && !MigrationHalt::<T>::get()
                && !T::MigrationStatus::cursor_exists()
            {
                return Err(TryRuntimeError::Other(
                    "execution guard pre-migration anchor outlived its migration interval",
                ));
            }
            let pending_upgrade = PendingUpgrade::<T>::get().is_some();
            let (pending_authorized_at, urgent_upgrade) =
                T::ReleaseChannel::pending_upgrade_indication();
            if pending_upgrade != (pending_authorized_at != 0) || pending_upgrade != urgent_upgrade
            {
                return Err(TryRuntimeError::Other(
                    "execution guard I-30 release-channel indication mismatch",
                ));
            }
            if !T::PendingOutflowSync::pending_outflows_synced() {
                return Err(TryRuntimeError::Other(
                    "execution guard pending-outflow mirror mismatch",
                ));
            }
            match (PendingUpgrade::<T>::get(), ScheduledUpgrade::<T>::get()) {
                (Some(pending), Some(scheduled)) if pending.hash == scheduled => {}
                (Some(_), None) | (None, None) => {}
                _ => {
                    return Err(TryRuntimeError::Other(
                        "execution guard scheduled-upgrade identity is invalid",
                    ));
                }
            }
            if let Some(recovery) = RecoveryImage::<T>::get() {
                let target_matches = PendingUpgrade::<T>::get().map_or_else(
                    || {
                        Self::current_spec().is_ok_and(|current| {
                            current
                                .spec_version
                                .checked_add(1)
                                .is_some_and(|target| target == recovery.target_spec_version)
                        })
                    },
                    |pending| {
                        pending.hash == recovery.primary_hash
                            && pending
                                .target_spec_version
                                .checked_add(1)
                                .is_some_and(|target| target == recovery.target_spec_version)
                    },
                );
                if recovery.len == 0
                    || recovery.len > T::MaxRuntimeCodeBytes::get()
                    || T::RecoveryImages::len(recovery.hash) != Some(recovery.len)
                    || !T::RecoveryImages::is_pinned(recovery.hash)
                    || recovery.hash == recovery.primary_hash
                    || QueuedRecoveryImages::<T>::contains_key(recovery.pid)
                    || !target_matches
                {
                    return Err(TryRuntimeError::Other(
                        "execution guard committed recovery image is invalid",
                    ));
                }
            }
            let migration = T::MigrationStatus::recovery_state();
            if migration.bypass {
                return Err(TryRuntimeError::Other(
                    "execution guard recovery bypass escaped its storage transaction",
                ));
            }
            if migration.lockdown {
                let Some(recovery) = RecoveryImage::<T>::get() else {
                    return Err(TryRuntimeError::Other(
                        "execution guard recovery lockdown has no commitment",
                    ));
                };
                let cursor_cause = migration.retired_cursor;
                let phase_cause = migration.phase_transition_lock;
                if cursor_cause == phase_cause {
                    return Err(TryRuntimeError::Other(
                        "execution guard recovery lockdown cause is not unique",
                    ));
                }
                let valid = if migration.aborted {
                    migration.scheduled_hash.is_none()
                        && !migration.recovery_code_applied
                        && ((cursor_cause && T::MigrationStatus::cursor_exists())
                            || (phase_cause && !T::MigrationStatus::cursor_exists()))
                } else {
                    migration.scheduled_hash == Some(recovery.hash)
                        && !T::MigrationStatus::cursor_exists()
                };
                if !valid {
                    return Err(TryRuntimeError::Other(
                        "execution guard recovery lockdown shape is invalid",
                    ));
                }
            } else if migration.retired_cursor
                || migration.scheduled_hash.is_some()
                || migration.aborted
                || migration.recovery_code_applied
            {
                return Err(TryRuntimeError::Other(
                    "execution guard orphan runtime recovery state",
                ));
            }
            match PhaseFourBridge::<T>::get() {
                PhaseFourBridgeState::Unused => {
                    if migration.phase_transition_lock || migration.phase_transition_applied {
                        return Err(TryRuntimeError::Other(
                            "execution guard orphan Phase-4 transition lock",
                        ));
                    }
                }
                PhaseFourBridgeState::Pending {
                    pid,
                    code_hash,
                    plan,
                } => {
                    if migration.phase_transition_lock
                        || migration.phase_transition_applied
                        || !T::PhaseState::exact_phase_three()
                        || !T::PhaseState::phase_four_plan_valid(&plan)
                        || PendingUpgrade::<T>::get()
                            .is_none_or(|pending| pending.hash != code_hash)
                        || RecoveryImage::<T>::get().is_none_or(|recovery| recovery.pid != pid)
                    {
                        return Err(TryRuntimeError::Other(
                            "execution guard pending Phase-4 bridge is invalid",
                        ));
                    }
                }
                PhaseFourBridgeState::Scheduled {
                    pid,
                    code_hash,
                    plan,
                } => {
                    if !migration.phase_transition_lock
                        || !T::PhaseState::exact_phase_three()
                        || !T::PhaseState::phase_four_plan_valid(&plan)
                        || PendingUpgrade::<T>::get()
                            .is_none_or(|pending| pending.hash != code_hash)
                        || RecoveryImage::<T>::get().is_none_or(|recovery| recovery.pid != pid)
                    {
                        return Err(TryRuntimeError::Other(
                            "execution guard scheduled Phase-4 bridge is invalid",
                        ));
                    }
                }
                PhaseFourBridgeState::Consumed => {
                    if migration.phase_transition_lock
                        || migration.phase_transition_applied
                        || !T::PhaseState::post_sudo_phase()
                    {
                        return Err(TryRuntimeError::Other(
                            "execution guard consumed Phase-4 bridge is invalid",
                        ));
                    }
                }
            }
            if let Some(pending) = PendingUpgrade::<T>::get() {
                let current = Self::current_spec().map_err(|_| {
                    TryRuntimeError::Other("execution guard current version is absent")
                })?;
                let expected_target = current.spec_version.checked_add(1);
                let expected_applicable = pending.authorized_at.checked_add(DESCRIPTOR_LEAD_TIME);
                if expected_target != Some(pending.target_spec_version)
                    || expected_applicable != Some(pending.applicable_at)
                    || LastUpgradeAuthorized::<T>::get() != Some(pending.authorized_at)
                {
                    return Err(TryRuntimeError::Other(
                        "execution guard pending-upgrade identity is invalid",
                    ));
                }
            }
            let spacing_history = UpgradeSpacingHistory::<T>::get();
            let history_last = spacing_history.last().map(|(at, _)| *at);
            if LastUpgradeAuthorized::<T>::get() != history_last
                || history_last.is_some_and(|last| last > Self::now())
                || T::Params::code_spacing() == 0
            {
                return Err(TryRuntimeError::Other(
                    "execution guard I-7/I-17 code-spacing envelope invalid",
                ));
            }
            for pair in spacing_history.as_slice().windows(2) {
                let [previous, current] = pair else {
                    return Err(TryRuntimeError::Other(
                        "execution guard spacing history window is malformed",
                    ));
                };
                let (previous_at, _) = *previous;
                let (authorized_at, enforced_spacing) = *current;
                let spacing_invalid = match previous_at.checked_add(enforced_spacing) {
                    Some(earliest) => authorized_at < earliest,
                    None => true,
                };
                if enforced_spacing != 0 && spacing_invalid {
                    return Err(TryRuntimeError::Other(
                        "execution guard I-7/I-17 spacing history is non-monotone",
                    ));
                }
            }
            Ok(())
        }

        fn map_core_error(error: CoreError) -> DispatchError {
            match error {
                CoreError::BadOrigin => DispatchError::BadOrigin,
                CoreError::QueueFull => Error::<T>::QueueFull.into(),
                CoreError::NotFound => Error::<T>::NotFound.into(),
                CoreError::Cancelled => Error::<T>::Cancelled.into(),
                CoreError::NotMature => Error::<T>::NotMature.into(),
                CoreError::GraceExpired => Error::<T>::GraceExpired.into(),
                CoreError::BadPreimage => Error::<T>::BadPreimage.into(),
                CoreError::StaleQueue => Error::<T>::StaleQueue.into(),
                CoreError::NotRatified => Error::<T>::NotRatified.into(),
                CoreError::AttestationMissing => Error::<T>::AttestationMissing.into(),
                CoreError::CapabilityDenied => Error::<T>::CapabilityDenied.into(),
                CoreError::MetersBlocked => Error::<T>::MetersBlocked.into(),
                CoreError::ResourceLockMissing => Error::<T>::ResourceLockMissing.into(),
                CoreError::GuardianHold => Error::<T>::GuardianHold.into(),
                CoreError::GateSuspended => Error::<T>::GateSuspended.into(),
                CoreError::FreezeActive => Error::<T>::FreezeActive.into(),
                CoreError::PayloadTooLarge => Error::<T>::PayloadTooLarge.into(),
                CoreError::TooManyCalls => Error::<T>::TooManyCalls.into(),
                CoreError::TooManyDomains => Error::<T>::TooManyDomains.into(),
                CoreError::TooManyLocks => Error::<T>::TooManyLocks.into(),
                CoreError::BadDomainDeclaration => Error::<T>::BadDomainDeclaration.into(),
                CoreError::SafetyFilter => Error::<T>::SafetyFilter.into(),
                CoreError::DispatchFailed => Error::<T>::DispatchFailed.into(),
                CoreError::BadUpgradePayload => Error::<T>::BadUpgradePayload.into(),
                CoreError::PendingUpgradeExists => Error::<T>::PendingUpgradeExists.into(),
                CoreError::NoPendingUpgrade => Error::<T>::NoPendingUpgrade.into(),
                CoreError::DescriptorLeadTime => Error::<T>::DescriptorLeadTime.into(),
                CoreError::UpgradeHashMismatch => Error::<T>::UpgradeHashMismatch.into(),
                CoreError::UpgradeVersionMismatch => Error::<T>::UpgradeVersionMismatch.into(),
                CoreError::RetryWindowOpen => Error::<T>::RetryWindowOpen.into(),
                CoreError::Overflow => Error::<T>::Overflow.into(),
            }
        }
    }
}

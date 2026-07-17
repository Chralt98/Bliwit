#![cfg_attr(not(feature = "std"), no_std)]
#![deny(unsafe_code)]

//! Production FRAME shell over `epoch-core` (A8).
//!
//! The shell owns the frozen 02 §7.1 storage surface, origin checks, bounded
//! mirrors, sibling-pallet seams and atomic persistence. All proposal/epoch/
//! cohort transitions and the ordered decision kernel remain in `epoch-core`.

extern crate alloc;

pub use pallet::*;
pub use weights::WeightInfo;

pub mod weights;

#[cfg(feature = "runtime-benchmarks")]
pub mod benchmarking;
#[cfg(test)]
mod mock;
#[cfg(test)]
mod tests;

use core::marker::PhantomData;
use frame_support::pallet_prelude::{DispatchError, DispatchResult};
use futarchy_primitives::{
    keeper::{CrankClass, KeeperRebateSink},
    Balance, BlockNumber, EpochId, FixedU64, MarketId, MarketSet, MetricSpecVersion, Proposal,
    ProposalClass, ProposalId, RejectReason, RuntimeVersionConstraint, H256,
};

pub use epoch_core::{
    CohortInfo as CoreCohortInfo, CohortStatus, DecisionGuards, DecisionInputs,
    EpochInfo as CoreEpochInfo, EpochParams as CoreEpochParams, EpochState, Error as CoreError,
    Event as CoreEvent, LedgerOps as CoreLedgerOps, Origin as CoreOrigin, SettlementTarget,
    TickInputs, WelfareOps as CoreWelfareOps, MAX_ACTIVE_PER_EPOCH, MAX_INTAKE_QUEUE,
    MAX_LIVE_PROPOSALS, MAX_NON_TERMINAL_COHORTS, MAX_RESOURCES_PER_PROPOSAL, RECENT_COHORTS,
};

pub const MAX_INTAKE_QUEUE_BOUND: u32 = MAX_INTAKE_QUEUE as u32;
pub const MAX_LIVE_PROPOSALS_BOUND: u32 = MAX_LIVE_PROPOSALS as u32;
pub const MAX_COHORT_PROPOSALS_BOUND: u32 = MAX_ACTIVE_PER_EPOCH as u32;
pub const MAX_NON_TERMINAL_COHORTS_BOUND: u32 = MAX_NON_TERMINAL_COHORTS as u32;
pub const RECENT_COHORTS_BOUND: u32 = RECENT_COHORTS as u32;
pub const MAX_RESOURCE_LOCKS_BOUND: u32 =
    MAX_LIVE_PROPOSALS_BOUND * MAX_RESOURCES_PER_PROPOSAL as u32;
pub const TICK_BATCH_BOUND: u32 = futarchy_primitives::kernel::TICK_BATCH;

/// Live epoch/decision tunables sourced from `pallet-constitution::Params`.
pub trait EpochParamsProvider {
    fn get() -> CoreEpochParams;
}

/// Market book role used by the trusted decision-grade read seam.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BookRole {
    Decision,
    Baseline,
    Gate,
}

/// Decision-book reads and Seed/rerun market deployment (A3 → A8).
pub trait MarketAccess<AccountId> {
    fn open_markets(
        proposal: &Proposal<AccountId>,
        rerun: bool,
        requires_gate_markets: bool,
    ) -> Result<MarketSet, DispatchError>;
    fn baseline_market(epoch: EpochId) -> Option<MarketId>;
    fn twap_full(market: MarketId) -> Option<FixedU64>;
    fn twap_trailing(market: MarketId, window: BlockNumber) -> Option<FixedU64>;
    fn spot(market: MarketId) -> Option<FixedU64>;
    /// Returns false for an unavailable or ungraded book.
    fn decision_grade(
        market: MarketId,
        role: BookRole,
        class: ProposalClass,
        params: &CoreEpochParams,
    ) -> bool;
    fn measured_depth(pid: ProposalId) -> Balance;
    fn published_flow_per_day(pid: ProposalId) -> Option<Balance>;
    fn second_insufficiency(pid: ProposalId) -> bool;
    /// Previous epoch's finalized Baseline decision-window TWAP (05 §5.3).
    fn previous_settled_baseline_twap(epoch: EpochId) -> Option<FixedU64>;
}

pub trait OracleAccess {
    fn any_open_dispute_touching(spec: MetricSpecVersion) -> bool;
}

pub trait GuardianAccess {
    fn hold_active(pid: ProposalId) -> bool;
    fn dead_man_engaged() -> bool;
    fn review_window_closed(pid: ProposalId) -> bool;
}

pub trait AttestationAccess {
    fn present_and_quorate(pid: ProposalId, artifact_hash: H256) -> bool;
}

pub trait ConstitutionAccess<AccountId> {
    fn static_checks_pass(proposal: &Proposal<AccountId>) -> bool;
    fn queue_time_check(proposal: &Proposal<AccountId>) -> bool;
    fn in_cap_prize(proposal: &Proposal<AccountId>) -> Option<Balance>;
    fn ledger_frozen() -> bool;
    fn phase_flags() -> u32;
    fn active_metric_spec_version() -> MetricSpecVersion;
    fn treasury_gate_required(proposal: &Proposal<AccountId>) -> bool;
}

pub trait PreimageAccess {
    fn len(hash: H256) -> Option<u32>;
    /// Acquire epoch's qualification-to-queue ownership reference. The
    /// implementation participates in the caller's storage transaction.
    fn request(hash: H256) -> DispatchResult;
    /// Release one reference owned by epoch. Implementations must be
    /// idempotent/fail-safe: a missing underlying request is a no-op.
    fn unrequest(hash: H256);
}

/// A8 → A11 producer seam. Only an adopted decision invokes this endpoint.
pub trait ExecutionGuardAccess {
    fn enqueue(
        pid: ProposalId,
        payload_hash: H256,
        version_constraint: Option<RuntimeVersionConstraint>,
        maturity: BlockNumber,
        grace: BlockNumber,
        requires_ratification: bool,
    ) -> DispatchResult;
    fn queue_reject_reason(pid: ProposalId) -> Option<RejectReason>;
    fn retry_exhausted(pid: ProposalId) -> bool;
    /// Idempotently remove the A11 queue entry and every guard-owned
    /// auxiliary (locks, expedited/attestation/ratification bindings and the
    /// pinned preimage). Epoch calls this after every guard-terminal
    /// transition — T15/T16/T22 (via `tick`), plus the direct T20
    /// (`force_reject_process_hold`) and T24 (`veto_upheld`) guardian paths.
    fn dequeue_terminal(pid: ProposalId) -> DispatchResult;
}

/// Sole settlement hand-off (05 §6). The implementation is pallet-welfare,
/// which alone owns ledger SettleAuthority.
pub trait WelfareSettlement {
    fn compute_settlement(
        cohort_epoch: EpochId,
        spec: MetricSpecVersion,
        target: SettlementTarget,
    ) -> Result<FixedU64, DispatchError>;
}

/// Epoch's ResolveAuthority seam. It intentionally has no settle methods.
pub trait LedgerResolution {
    fn create_vault(pid: ProposalId, spec: MetricSpecVersion) -> DispatchResult;
    fn resolve(pid: ProposalId, branch: futarchy_primitives::Branch) -> DispatchResult;
    fn void(pid: ProposalId) -> DispatchResult;
}

#[cfg(feature = "runtime-benchmarks")]
pub trait BenchmarkHelper<RuntimeOrigin, AccountId> {
    fn prime_submit_epoch(epoch: EpochId);
    fn constitutional_values_origin() -> RuntimeOrigin;
    fn guardian_origin() -> RuntimeOrigin;
    fn execution_guard_origin() -> RuntimeOrigin;
    fn void_authority_origin() -> RuntimeOrigin;
    fn account(seed: u8) -> AccountId;
    fn proposal(
        id: ProposalId,
        who: AccountId,
        now: BlockNumber,
        epoch: EpochId,
    ) -> Proposal<AccountId>;
    fn prime_decision(pid: ProposalId, epoch: EpochId, gates: bool) -> MarketSet;
    /// Saturate the real execution-guard aggregate before a decision enqueues.
    fn prime_guard_enqueue(pid: ProposalId);
    fn prime_settlement(epoch: EpochId);
    fn prime_keeper_rebate() {}
    fn assert_keeper_rebate_paid(_: futarchy_primitives::keeper::CrankClass) {}
}

/// `Get<EpochId>` projection for sibling pallets (treasury/registry/welfare).
pub struct CurrentEpoch<T>(PhantomData<T>);

impl<T: pallet::Config> frame_support::traits::Get<EpochId> for CurrentEpoch<T> {
    fn get() -> EpochId {
        pallet::EpochOf::<T>::get().index
    }
}

#[frame_support::pallet]
pub mod pallet {
    use super::*;
    use alloc::vec::Vec;
    use frame_support::{pallet_prelude::*, traits::EnsureOrigin};
    use frame_system::pallet_prelude::*;
    use futarchy_primitives::{Branch, CohortSummary, DecisionOutcome, EpochPhase, ProposalState};
    use sp_runtime::{SaturatedConversion, TryRuntimeError};

    const STORAGE_VERSION: StorageVersion = StorageVersion::new(0);

    #[pallet::pallet]
    #[pallet::storage_version(STORAGE_VERSION)]
    pub struct Pallet<T>(_);

    #[pallet::config]
    pub trait Config:
        frame_system::Config<
        AccountId: From<[u8; 32]> + Into<[u8; 32]>,
        RuntimeEvent: From<Event<Self>>,
    >
    {
        type Params: EpochParamsProvider;
        type Market: MarketAccess<Self::AccountId>;
        type Oracle: OracleAccess;
        type Guardian: GuardianAccess;
        type Attestation: AttestationAccess;
        type Constitution: ConstitutionAccess<Self::AccountId>;
        type Preimage: PreimageAccess;
        type ExecutionGuard: ExecutionGuardAccess;
        type Welfare: WelfareSettlement;
        type Ledger: LedgerResolution;
        /// Fail-soft keeper rebate sink (08 §6). It must never affect a crank.
        type KeeperRebate: KeeperRebateSink<Self::AccountId>;
        type GuardianOrigin: EnsureOrigin<Self::RuntimeOrigin>;
        type ExecutionGuardOrigin: EnsureOrigin<Self::RuntimeOrigin>;
        type VoidAuthority: EnsureOrigin<Self::RuntimeOrigin>;
        type ConstitutionalValuesOrigin: EnsureOrigin<Self::RuntimeOrigin>;
        type WeightInfo: WeightInfo;
        #[cfg(feature = "runtime-benchmarks")]
        type BenchmarkHelper: BenchmarkHelper<Self::RuntimeOrigin, Self::AccountId>;
    }

    /// Frozen 02 §7.1 `EpochOf` value — exactly three fields.
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
    pub struct EpochInfo {
        pub index: EpochId,
        pub phase: EpochPhase,
        pub phase_start_block: BlockNumber,
    }

    impl Default for EpochInfo {
        fn default() -> Self {
            Self {
                index: 0,
                phase: EpochPhase::Intake,
                phase_start_block: 0,
            }
        }
    }

    /// Internal phase-math fields intentionally kept outside frozen `EpochOf`.
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
    pub struct EpochSchedule {
        pub epoch_start_block: BlockNumber,
        pub length: BlockNumber,
        pub next_length: BlockNumber,
    }

    impl Default for EpochSchedule {
        fn default() -> Self {
            Self {
                epoch_start_block: 0,
                length: CoreEpochParams::DEFAULT.epoch_length,
                next_length: CoreEpochParams::DEFAULT.epoch_length,
            }
        }
    }

    #[derive(
        Clone, Debug, Decode, DecodeWithMemTracking, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo,
    )]
    pub struct CohortInfo {
        pub epoch: EpochId,
        pub proposals: BoundedVec<ProposalId, ConstU32<MAX_COHORT_PROPOSALS_BOUND>>,
        pub status: CohortStatus,
    }

    impl TryFrom<CoreCohortInfo> for CohortInfo {
        type Error = CoreError;

        fn try_from(value: CoreCohortInfo) -> Result<Self, Self::Error> {
            Ok(Self {
                epoch: value.epoch,
                proposals: BoundedVec::try_from(value.proposals)
                    .map_err(|_| CoreError::TooManyCohortProposals)?,
                status: value.status,
            })
        }
    }

    impl From<CohortInfo> for CoreCohortInfo {
        fn from(value: CohortInfo) -> Self {
            Self {
                epoch: value.epoch,
                proposals: value.proposals.into_inner(),
                status: value.status,
            }
        }
    }

    /// Creation-time frozen proposal schedule/spec binding (I-16).
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
    pub struct ProposalSchedule {
        pub epoch: EpochId,
        pub epoch_start_block: BlockNumber,
        pub epoch_length: BlockNumber,
        pub decide_at: BlockNumber,
        pub metric_spec: MetricSpecVersion,
    }

    #[derive(
        Clone,
        Copy,
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
    pub struct DeadManState {
        pub paused_at: Option<BlockNumber>,
        pub recovery_epoch: Option<EpochId>,
    }

    pub type SpecBindings =
        BoundedVec<(ProposalId, MetricSpecVersion), ConstU32<MAX_COHORT_PROPOSALS_BOUND>>;

    #[derive(
        Clone, Debug, Decode, DecodeWithMemTracking, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo,
    )]
    pub struct CohortSchedule {
        pub epoch: EpochId,
        pub creation_epoch_length: BlockNumber,
        pub measurement_until: EpochId,
        pub settlement_epoch: EpochId,
        pub specs: SpecBindings,
    }

    pub type Intake = BoundedVec<ProposalId, ConstU32<MAX_INTAKE_QUEUE_BOUND>>;
    pub type Recent = BoundedVec<CohortSummary, ConstU32<RECENT_COHORTS_BOUND>>;
    pub type Locks = BoundedVec<([u8; 8], ProposalId), ConstU32<MAX_RESOURCE_LOCKS_BOUND>>;
    pub type TickBatch = BoundedVec<ProposalId, ConstU32<TICK_BATCH_BOUND>>;
    pub type Rollovers = BoundedVec<(ProposalId, u8), ConstU32<MAX_INTAKE_QUEUE_BOUND>>;

    /// Frozen 02 §7.1 live proposal map (Screening→settled pipeline only).
    #[pallet::storage]
    pub type Proposals<T: Config> =
        CountedStorageMap<_, Blake2_128Concat, ProposalId, Proposal<T::AccountId>, OptionQuery>;

    #[pallet::storage]
    pub type EpochOf<T: Config> = StorageValue<_, EpochInfo, ValueQuery>;

    #[pallet::storage]
    pub type IntakeQueue<T: Config> = StorageValue<_, Intake, ValueQuery>;

    #[pallet::storage]
    pub type RecentCohortSummaries<T: Config> = StorageValue<_, Recent, ValueQuery>;

    #[pallet::storage]
    pub type Cohorts<T: Config> =
        CountedStorageMap<_, Blake2_128Concat, EpochId, CohortInfo, OptionQuery>;

    // Internal bounded mirrors. They do not alter the 02 §7.1 frontend surface.
    #[pallet::storage]
    pub type IntakeProposals<T: Config> =
        CountedStorageMap<_, Blake2_128Concat, ProposalId, Proposal<T::AccountId>, OptionQuery>;

    #[pallet::storage]
    pub type Schedule<T: Config> = StorageValue<_, EpochSchedule, ValueQuery>;

    #[pallet::storage]
    pub type ResourceLocks<T: Config> = StorageValue<_, Locks, ValueQuery>;

    #[pallet::storage]
    pub type ProposalSchedules<T: Config> =
        StorageMap<_, Blake2_128Concat, ProposalId, ProposalSchedule, OptionQuery>;

    #[pallet::storage]
    pub type CohortSchedules<T: Config> =
        StorageMap<_, Blake2_128Concat, EpochId, CohortSchedule, OptionQuery>;

    #[pallet::storage]
    pub type NextProposalId<T: Config> = StorageValue<_, ProposalId, ValueQuery>;

    #[pallet::storage]
    pub type RolloverCounts<T: Config> = StorageValue<_, Rollovers, ValueQuery>;

    #[pallet::storage]
    pub type DeadMan<T: Config> = StorageValue<_, DeadManState, ValueQuery>;

    #[pallet::storage]
    pub type StaleEpochCutoff<T: Config> = StorageValue<_, ProposalId, OptionQuery>;

    #[pallet::storage]
    pub type BaselineCarry<T: Config> = StorageValue<_, (EpochId, u8), OptionQuery>;

    /// Epoch-owned preimage references between T5 qualification and either a
    /// pre-queue terminal or the atomic T9 handoff to execution-guard. Keeping
    /// the owner keyed by proposal makes request/unrequest idempotent even
    /// when keeper cranks are retried (06 §4; 09 §7.3).
    #[pallet::storage]
    pub type QualificationPins<T: Config> =
        StorageMap<_, Blake2_128Concat, ProposalId, H256, OptionQuery>;

    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        ProposalSubmitted(ProposalId),
        ProposalWithdrawn(ProposalId),
        ScreeningStarted(ProposalId),
        ProposalCancelled {
            pid: ProposalId,
            reason: RejectReason,
        },
        ProposalQualified(ProposalId),
        ProposalDeferred(ProposalId),
        MarketsOpened(ProposalId),
        DecisionExtended(ProposalId),
        ProposalQueued {
            pid: ProposalId,
            payload_hash: H256,
            maturity: BlockNumber,
        },
        ProposalRejected {
            pid: ProposalId,
            reason: RejectReason,
        },
        ProposalDelayed {
            pid: ProposalId,
            justification_hash: H256,
        },
        RerunScheduled(ProposalId),
        RerunOpened(ProposalId),
        MandateExpired(ProposalId),
        MeasurementStarted {
            cohort: EpochId,
        },
        CohortSettled {
            epoch: EpochId,
            s: FixedU64,
        },
        CohortVoided {
            epoch: EpochId,
        },
        BaselineCarried {
            pid: ProposalId,
            epoch: EpochId,
        },
        ProposalForceRejected {
            pid: ProposalId,
            reason: RejectReason,
        },
        IntakeSlashed {
            pid: ProposalId,
            reason: RejectReason,
            amount: Balance,
        },
    }

    #[pallet::error]
    pub enum Error<T> {
        BadPhase,
        IntakeFull,
        TooManyLiveProposals,
        TooManyResources,
        UnknownProposal,
        BadState,
        DuplicateProposal,
        LockConflict,
        TooManyCohorts,
        TooManyCohortProposals,
        BadEpochLength,
        BadParams,
        BadDecisionInput,
        BatchTooLarge,
        ArithmeticOverflow,
        Ledger,
        ExecutionGuard,
        Welfare,
        TryStateViolation,
        BadProposalShape,
    }

    #[pallet::hooks]
    impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
        fn integrity_test() {
            assert_eq!(
                MAX_INTAKE_QUEUE_BOUND,
                futarchy_primitives::bounds::INTAKE_QUEUE,
                "epoch IntakeQueue bound must match 13 §4"
            );
            assert_eq!(
                MAX_LIVE_PROPOSALS_BOUND,
                futarchy_primitives::bounds::MAX_LIVE_PROPOSALS,
                "epoch Proposals bound must match 13 §4"
            );
            assert_eq!(
                TICK_BATCH_BOUND,
                futarchy_primitives::kernel::TICK_BATCH,
                "epoch tick batch must match the kernel cap"
            );
        }

        #[cfg(feature = "try-runtime")]
        fn try_state(_n: BlockNumberFor<T>) -> Result<(), TryRuntimeError> {
            Self::do_try_state()
        }
    }

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        #[pallet::call_index(0)]
        #[pallet::weight(T::WeightInfo::submit())]
        pub fn submit(
            origin: OriginFor<T>,
            mut proposal: Proposal<T::AccountId>,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;
            let now = Self::now();
            let epoch = EpochOf::<T>::get().index;
            proposal.id = NextProposalId::<T>::get();
            ensure!(
                proposal.proposer == who
                    && proposal.epoch == epoch
                    && proposal.submitted_at == now
                    && proposal.state == ProposalState::Submitted
                    && proposal.decide_at == 0
                    && !proposal.rerun
                    && !proposal.extended
                    && !proposal.delayed_once
                    && proposal.payload_len <= futarchy_primitives::kernel::MAX_BYTES
                    && proposal.markets.is_none()
                    && proposal.maturity.is_none()
                    && proposal.grace_end.is_none()
                    && proposal.decision.is_none(),
                Error::<T>::BadProposalShape
            );
            let params = Self::live_params()?;
            Self::mutate(|state, _| state.submit(CoreOrigin::Signed, proposal, &params))
        }

        #[pallet::call_index(1)]
        #[pallet::weight(T::WeightInfo::withdraw())]
        pub fn withdraw(origin: OriginFor<T>, pid: ProposalId) -> DispatchResult {
            let who = ensure_signed(origin)?;
            Self::mutate(|state, _| {
                state.withdraw(CoreOrigin::Signed, pid, &who)?;
                // T2 is normally pre-qualification and therefore unpinned;
                // retain an idempotent cleanup for legacy/corrupt ownership.
                Self::release_qualification_pin(pid);
                Ok(())
            })
        }

        /// Permissionless bounded crank. An empty batch advances only the phase
        /// clock; each item is idempotent when no transition is due.
        #[pallet::call_index(2)]
        #[pallet::weight(T::WeightInfo::tick(pids.len() as u32))]
        pub fn tick(origin: OriginFor<T>, pids: TickBatch) -> DispatchResult {
            let who = ensure_signed(origin)?;
            let params = Self::live_params()?;
            let now = Self::now();
            let mut advanced = false;
            let result = Self::mutate(|state, ledger| {
                state.horizon_k = params.horizon_k;
                state.epoch.next_length = params.epoch_length;
                let clock_before = (
                    state.epoch.index,
                    state.epoch.phase,
                    state.epoch.phase_start_block,
                    state.epoch.epoch_start_block,
                    state.epoch.length,
                );
                Self::sync_clock(state, now)?;
                for pid in pids {
                    let proposal = state.proposal_view(pid)?.clone();
                    let rerun = proposal.state == ProposalState::Rerun;
                    let requires_gate_markets = if rerun {
                        proposal
                            .markets
                            .is_some_and(|markets| markets.gates.is_some())
                    } else {
                        Self::requires_gate_markets_at_seed(&proposal)
                    };
                    // Suppress new market/vault deployment while any safety hold
                    // is active: a genuine stale-epoch (the core's T20 force-reject),
                    // the dead-man pause (05 §4.8) or a ledger freeze (06 §6.3). The
                    // core decides freeze-vs-force-reject from its own authoritative
                    // state, so these are deliberately not fed into `tick` inputs.
                    let safety_hold = T::Guardian::dead_man_engaged()
                        || T::Constitution::ledger_frozen()
                        || state.stale_process_hold(pid);
                    let markets = if !safety_hold
                        && state.recovery_epoch.is_none()
                        && state.epoch.phase == EpochPhase::Seed
                        && matches!(
                            proposal.state,
                            ProposalState::Qualified | ProposalState::Rerun
                        ) {
                        let markets =
                            T::Market::open_markets(&proposal, rerun, requires_gate_markets)
                                .map_err(|_| CoreError::Ledger)?;
                        ensure!(
                            markets.gates.is_some() == requires_gate_markets,
                            CoreError::BadDecisionInput
                        );
                        Some(markets)
                    } else {
                        None
                    };
                    let preimage_ok = proposal.payload_len
                        <= futarchy_primitives::kernel::MAX_BYTES
                        && T::Preimage::len(proposal.payload_hash) == Some(proposal.payload_len);
                    let guard_owned_before = matches!(
                        proposal.state,
                        ProposalState::Queued | ProposalState::FailedExecuted
                    );
                    let events_before = state.events.len();
                    state.tick(
                        CoreOrigin::Keeper,
                        ledger,
                        pid,
                        now,
                        TickInputs {
                            static_checks_pass: preimage_ok
                                && T::Constitution::static_checks_pass(&proposal),
                            active_metric_spec_version: T::Constitution::active_metric_spec_version(
                            ),
                            markets,
                            review_window_closed: T::Guardian::review_window_closed(pid),
                            queue_reject_reason: T::ExecutionGuard::queue_reject_reason(pid),
                            retry_exhausted: T::ExecutionGuard::retry_exhausted(pid),
                        },
                        &params,
                    )?;
                    let state_after = state.proposal_view(pid)?.state;
                    if matches!(
                        proposal.state,
                        ProposalState::Submitted | ProposalState::Screening
                    ) && state_after == ProposalState::Qualified
                    {
                        // T5: request before the qualified state is persisted.
                        // Both writes share this storage layer, so a failed
                        // request rolls the transition back for a keeper retry.
                        Self::pin_at_qualification(pid, proposal.payload_hash)?;
                    }
                    if !Self::epoch_owns_prequeue_pin(state_after) {
                        // Covers pre-queue T20/stale force-rejects. Queued and
                        // failed-executed proposals handed ownership to A11 at
                        // T9 and therefore have no QualificationPins entry.
                        Self::release_qualification_pin(pid);
                    }
                    advanced |= state
                        .events
                        .iter()
                        .skip(events_before)
                        .any(|event| !matches!(event, CoreEvent::NoOp));
                    let guard_owned_after = matches!(
                        state_after,
                        ProposalState::Queued | ProposalState::FailedExecuted
                    );
                    if guard_owned_before && !guard_owned_after {
                        T::ExecutionGuard::dequeue_terminal(pid)
                            .map_err(|_| CoreError::ExecutionGuard)?;
                    }
                    if state.events.iter().any(
                        |event| matches!(event, CoreEvent::RerunOpened(opened) if *opened == pid),
                    ) {
                        let reopened = state.proposal_view(pid)?.decide_at;
                        ProposalSchedules::<T>::try_mutate(pid, |schedule| {
                            let schedule = schedule.as_mut().ok_or(CoreError::TryStateViolation)?;
                            schedule.decide_at = reopened;
                            Ok::<(), CoreError>(())
                        })?;
                    }
                }
                advanced |= clock_before
                    != (
                        state.epoch.index,
                        state.epoch.phase,
                        state.epoch.phase_start_block,
                        state.epoch.epoch_start_block,
                        state.epoch.length,
                    );
                Ok(())
            });
            if result.is_ok() && advanced {
                // B5 recalibrates this weight for the rebate sink's treasury writes.
                T::KeeperRebate::rebate(&who, CrankClass::DecisionCritical);
            }
            result
        }

        #[pallet::call_index(3)]
        #[pallet::weight(T::WeightInfo::decide())]
        pub fn decide(origin: OriginFor<T>, pid: ProposalId) -> DispatchResult {
            let who = ensure_signed(origin)?;
            let params = Self::live_params()?;
            let now = Self::now();
            let mut decision_advanced = false;
            let result = frame_support::storage::with_storage_layer(|| {
                let mut state = Self::load();
                state.dead_man_armed = T::Guardian::dead_man_engaged();
                state.ledger_frozen = T::Constitution::ledger_frozen();
                state.phase_flags = T::Constitution::phase_flags();
                state.horizon_k = params.horizon_k;
                state.epoch.next_length = params.epoch_length;
                Self::sync_clock(&mut state, now).map_err(Self::map_core_error)?;
                let proposal = state
                    .proposal_view(pid)
                    .map_err(Self::map_core_error)?
                    .clone();
                let decision_was_recorded = proposal.decision.is_some();
                let extension_was_recorded = proposal.extended;
                let markets = proposal.markets.ok_or(Error::<T>::BadDecisionInput)?;
                let accept_full = T::Market::twap_full(markets.accept).unwrap_or(FixedU64(0));
                let reject_full = T::Market::twap_full(markets.reject).unwrap_or(FixedU64(0));
                let baseline_full = T::Market::twap_full(markets.baseline).unwrap_or(FixedU64(0));
                let accept_trailing =
                    T::Market::twap_trailing(markets.accept, params.trailing_window)
                        .unwrap_or(FixedU64(0));
                let reject_trailing =
                    T::Market::twap_trailing(markets.reject, params.trailing_window)
                        .unwrap_or(FixedU64(0));
                let baseline_trailing =
                    T::Market::twap_trailing(markets.baseline, params.trailing_window)
                        .unwrap_or(FixedU64(0));
                let accept_spot = T::Market::spot(markets.accept).unwrap_or(FixedU64(0));
                let reject_spot = T::Market::spot(markets.reject).unwrap_or(FixedU64(0));
                let welfare_grade_ok = [
                    (markets.accept, BookRole::Decision),
                    (markets.reject, BookRole::Decision),
                ]
                .iter()
                .all(|(market, role)| {
                    T::Market::decision_grade(*market, *role, proposal.class, &params)
                });
                let baseline_grade_ok = T::Market::baseline_market(proposal.epoch)
                    == Some(markets.baseline)
                    && T::Market::decision_grade(
                        markets.baseline,
                        BookRole::Baseline,
                        proposal.class,
                        &params,
                    );
                let gate_twaps = markets.gates.map(|gates| {
                    let [s_adopt, s_reject, c_adopt, c_reject] = gates;
                    [
                        T::Market::twap_full(s_adopt).unwrap_or(FixedU64(0)),
                        T::Market::twap_full(s_reject).unwrap_or(FixedU64(0)),
                        T::Market::twap_full(c_adopt).unwrap_or(FixedU64(0)),
                        T::Market::twap_full(c_reject).unwrap_or(FixedU64(0)),
                    ]
                });
                let has_gate_markets = markets.gates.is_some();
                let gate_grade_ok = if has_gate_markets {
                    markets.gates.is_some_and(|gates| {
                        gates.iter().all(|market| {
                            T::Market::decision_grade(
                                *market,
                                BookRole::Gate,
                                proposal.class,
                                &params,
                            )
                        })
                    })
                } else {
                    true
                };
                let guards = DecisionGuards {
                    preimage_ok: proposal.payload_len <= futarchy_primitives::kernel::MAX_BYTES
                        && T::Preimage::len(proposal.payload_hash) == Some(proposal.payload_len),
                    resource_locks_held: state.resource_locks_held(pid),
                    process_hold: T::Oracle::any_open_dispute_touching(proposal.metric_spec)
                        || T::Guardian::hold_active(pid)
                        || T::Guardian::dead_man_engaged()
                        || state.stale_process_hold(pid),
                };
                let input = DecisionInputs {
                    accept_full,
                    reject_full,
                    baseline_full,
                    accept_trailing,
                    reject_trailing,
                    baseline_trailing,
                    accept_spot,
                    reject_spot,
                    welfare_grade_ok,
                    baseline_grade_ok,
                    previous_settled_baseline_twap: T::Market::previous_settled_baseline_twap(
                        proposal.epoch,
                    ),
                    welfare_second_insufficient: T::Market::second_insufficiency(pid),
                    gate_grade_ok,
                    gate_twaps,
                    measured_depth: T::Market::measured_depth(pid),
                    published_flow_per_day: T::Market::published_flow_per_day(pid),
                    in_cap_prize: T::Constitution::in_cap_prize(&proposal),
                    attestation_quorate: T::Attestation::present_and_quorate(
                        pid,
                        proposal.payload_hash,
                    ),
                    constitution_queue_ok: T::Constitution::queue_time_check(&proposal),
                };
                let mut ledger = LedgerAdapter::<T>(PhantomData);
                let outcome = state
                    .decide_with(
                        CoreOrigin::Keeper,
                        &mut ledger,
                        pid,
                        now,
                        input,
                        guards,
                        &params,
                    )
                    .map_err(Self::map_core_error)?;
                decision_advanced = state.proposal_view(pid).is_ok_and(|recorded| {
                    (!decision_was_recorded && recorded.decision.is_some())
                        || (!extension_was_recorded && recorded.extended)
                });
                if outcome == DecisionOutcome::Adopt {
                    let queued = state.proposal_view(pid).map_err(Self::map_core_error)?;
                    let maturity = queued.maturity.ok_or(Error::<T>::BadState)?;
                    let grace_end = queued.grace_end.ok_or(Error::<T>::BadState)?;
                    let grace = grace_end
                        .checked_sub(maturity)
                        .ok_or(Error::<T>::ArithmeticOverflow)?;
                    T::ExecutionGuard::enqueue(
                        pid,
                        queued.payload_hash,
                        queued.version_constraint.clone(),
                        maturity,
                        grace,
                        matches!(queued.class, ProposalClass::Code | ProposalClass::Meta),
                    )?;
                }
                if outcome != DecisionOutcome::Extend {
                    // Reject outcomes never queue. For Adopt, A11 acquired its
                    // own reference inside `enqueue`; dropping epoch's reference
                    // here is the atomic qualification→guard ownership handoff.
                    Self::release_qualification_pin(pid);
                }
                Self::persist(state)
            });
            if result.is_ok() && decision_advanced {
                // B5 recalibrates this weight for the rebate sink's treasury writes.
                T::KeeperRebate::rebate(&who, CrankClass::DecisionCritical);
            }
            result
        }

        #[pallet::call_index(4)]
        #[pallet::weight(T::WeightInfo::settle_cohort(*batch))]
        pub fn settle_cohort(origin: OriginFor<T>, epoch: EpochId, batch: u32) -> DispatchResult {
            let who = ensure_signed(origin)?;
            ensure!(
                batch > 0 && batch <= futarchy_primitives::kernel::SETTLE_COHORT_MAX_ITEMS,
                Error::<T>::BatchTooLarge
            );
            let params = Self::live_params()?;
            let now = Self::now();
            let result = frame_support::storage::with_storage_layer(|| {
                let baseline =
                    T::Market::baseline_market(epoch).ok_or(Error::<T>::BadDecisionInput)?;
                let baseline_twap =
                    T::Market::twap_full(baseline).ok_or(Error::<T>::BadDecisionInput)?;
                let mut state = Self::load();
                let cohort_members = state
                    .cohorts
                    .iter()
                    .find(|cohort| cohort.epoch == epoch)
                    .map(|cohort| cohort.proposals.clone())
                    .ok_or(Error::<T>::BadState)?;
                state.horizon_k = params.horizon_k;
                state.epoch.next_length = params.epoch_length;
                Self::sync_clock(&mut state, now).map_err(Self::map_core_error)?;
                let mut welfare = WelfareAdapter::<T>(PhantomData);
                state
                    .settle_cohort(
                        CoreOrigin::Keeper,
                        &mut welfare,
                        epoch,
                        batch,
                        baseline_twap,
                        now,
                    )
                    .map_err(Self::map_core_error)?;
                if !state.cohorts.iter().any(|cohort| cohort.epoch == epoch) {
                    for pid in cohort_members {
                        Self::release_qualification_pin(pid);
                    }
                }
                Self::persist(state)
            });
            if result.is_ok() {
                // B5 recalibrates this weight for the rebate sink's treasury writes.
                T::KeeperRebate::rebate(&who, CrankClass::DecisionCritical);
            }
            result
        }

        /// META/ConstitutionalValues refresh of the next-boundary epoch length.
        #[pallet::call_index(5)]
        #[pallet::weight(T::WeightInfo::set_next_epoch_length())]
        pub fn set_next_epoch_length(origin: OriginFor<T>) -> DispatchResult {
            T::ConstitutionalValuesOrigin::ensure_origin(origin)?;
            let params = Self::live_params()?;
            Self::mutate(|state, _| {
                state.set_next_epoch_length(CoreOrigin::Root, params.epoch_length, &params)
            })
        }

        #[pallet::call_index(6)]
        #[pallet::weight(T::WeightInfo::delay_once())]
        pub fn delay_once(
            origin: OriginFor<T>,
            pid: ProposalId,
            justification_hash: H256,
        ) -> DispatchResult {
            T::GuardianOrigin::ensure_origin(origin)?;
            Self::mutate(|state, _| {
                state.delay_once(CoreOrigin::GuardianHold, pid, justification_hash)
            })
        }

        #[pallet::call_index(7)]
        #[pallet::weight(T::WeightInfo::veto_upheld())]
        pub fn veto_upheld(origin: OriginFor<T>, pid: ProposalId) -> DispatchResult {
            T::GuardianOrigin::ensure_origin(origin)?;
            Self::mutate(|state, ledger| {
                // T24: a `Suspended` proposal was `Queued` before `delay_once`, so A11
                // still owns its queue entry, preimage pin and resource locks. Upholding
                // the veto drives it to the terminal rejected/measuring outcome; release
                // the guard state in lockstep (idempotent — a no-op if nothing is queued).
                state.veto_upheld(CoreOrigin::GuardianHold, ledger, pid)?;
                T::ExecutionGuard::dequeue_terminal(pid).map_err(|_| CoreError::ExecutionGuard)
            })
        }

        #[pallet::call_index(8)]
        #[pallet::weight(T::WeightInfo::mark_executed())]
        pub fn mark_executed(origin: OriginFor<T>, pid: ProposalId) -> DispatchResult {
            T::ExecutionGuardOrigin::ensure_origin(origin)?;
            Self::mutate(|state, ledger| {
                state.mark_executed(CoreOrigin::ExecutionGuard, ledger, pid)
            })
        }

        #[pallet::call_index(9)]
        #[pallet::weight(T::WeightInfo::mark_failed_executed())]
        pub fn mark_failed_executed(origin: OriginFor<T>, pid: ProposalId) -> DispatchResult {
            T::ExecutionGuardOrigin::ensure_origin(origin)?;
            Self::mutate(|state, _| state.mark_failed_executed(CoreOrigin::ExecutionGuard, pid))
        }

        #[pallet::call_index(10)]
        #[pallet::weight(T::WeightInfo::retry_exhausted_to_measurement())]
        pub fn retry_exhausted_to_measurement(
            origin: OriginFor<T>,
            pid: ProposalId,
        ) -> DispatchResult {
            T::ExecutionGuardOrigin::ensure_origin(origin)?;
            Self::mutate(|state, ledger| {
                match state.proposal_view(pid)?.state {
                    ProposalState::FailedExecuted => state.retry_exhausted_to_measurement(
                        CoreOrigin::ExecutionGuard,
                        ledger,
                        pid,
                    )?,
                    ProposalState::Measuring
                    | ProposalState::Settled
                    | ProposalState::Expired
                    | ProposalState::Rejected(_) => {}
                    _ => return Err(CoreError::BadState),
                }
                T::ExecutionGuard::dequeue_terminal(pid).map_err(|_| CoreError::ExecutionGuard)
            })
        }

        #[pallet::call_index(11)]
        #[pallet::weight(T::WeightInfo::expire_or_stale_queue())]
        pub fn expire_or_stale_queue(
            origin: OriginFor<T>,
            pid: ProposalId,
            reason: Option<RejectReason>,
        ) -> DispatchResult {
            T::ExecutionGuardOrigin::ensure_origin(origin)?;
            Self::mutate(|state, ledger| {
                match state.proposal_view(pid)?.state {
                    ProposalState::Queued => state.expire_or_stale_queue(
                        CoreOrigin::ExecutionGuard,
                        ledger,
                        pid,
                        reason,
                    )?,
                    ProposalState::Measuring
                    | ProposalState::Settled
                    | ProposalState::Expired
                    | ProposalState::Rejected(_) => {}
                    _ => return Err(CoreError::BadState),
                }
                T::ExecutionGuard::dequeue_terminal(pid).map_err(|_| CoreError::ExecutionGuard)
            })
        }

        #[pallet::call_index(12)]
        #[pallet::weight(T::WeightInfo::force_reject_process_hold())]
        pub fn force_reject_process_hold(origin: OriginFor<T>, pid: ProposalId) -> DispatchResult {
            T::GuardianOrigin::ensure_origin(origin)?;
            Self::mutate(|state, ledger| {
                // Direct T20 guardian path: when the target is `Queued`/`FailedExecuted`
                // (or a `Suspended` proposal that was queued) A11 still owns the queue
                // entry. Force-rejecting it is terminal, so release the guard state in
                // lockstep (idempotent — a no-op for pre-queue states with no entry).
                state.force_reject_process_hold(CoreOrigin::GuardianHold, ledger, pid)?;
                Self::release_qualification_pin(pid);
                T::ExecutionGuard::dequeue_terminal(pid).map_err(|_| CoreError::ExecutionGuard)
            })
        }

        #[pallet::call_index(13)]
        #[pallet::weight(T::WeightInfo::void_cohort(MAX_COHORT_PROPOSALS_BOUND))]
        pub fn void_cohort(origin: OriginFor<T>, epoch: EpochId) -> DispatchResult {
            T::VoidAuthority::ensure_origin(origin)?;
            Self::mutate(|state, ledger| {
                let members = state
                    .cohorts
                    .iter()
                    .find(|cohort| cohort.epoch == epoch)
                    .map(|cohort| cohort.proposals.clone())
                    .ok_or(CoreError::BadState)?;
                state.void_cohort(CoreOrigin::VoidAuthority, ledger, epoch)?;
                for pid in members {
                    Self::release_qualification_pin(pid);
                }
                Ok(())
            })
        }
    }

    #[pallet::extra_constants]
    impl<T: Config> Pallet<T> {
        #[pallet::constant_name(INTEGRATION_CONTRACT_VERSION)]
        fn integration_contract_version() -> u32 {
            futarchy_primitives::INTEGRATION_CONTRACT_VERSION
        }
        #[pallet::constant_name(MaxLiveProposals)]
        fn max_live_proposals() -> u32 {
            MAX_LIVE_PROPOSALS_BOUND
        }
        #[pallet::constant_name(MaxIntakeQueue)]
        fn max_intake_queue() -> u32 {
            MAX_INTAKE_QUEUE_BOUND
        }
        #[pallet::constant_name(MaxNonTerminalCohorts)]
        fn max_non_terminal_cohorts() -> u32 {
            MAX_NON_TERMINAL_COHORTS_BOUND
        }
        #[pallet::constant_name(RecentCohortSummariesBound)]
        fn recent_cohort_summaries_bound() -> u32 {
            RECENT_COHORTS_BOUND
        }
        #[pallet::constant_name(TickBatch)]
        fn tick_batch() -> u32 {
            TICK_BATCH_BOUND
        }
    }

    #[pallet::genesis_config]
    pub struct GenesisConfig<T: Config> {
        pub index: EpochId,
        pub start_block: BlockNumber,
        #[serde(skip)]
        pub _config: PhantomData<T>,
    }

    impl<T: Config> Default for GenesisConfig<T> {
        fn default() -> Self {
            Self {
                index: 0,
                start_block: 0,
                _config: PhantomData,
            }
        }
    }

    #[pallet::genesis_build]
    impl<T: Config> BuildGenesisConfig for GenesisConfig<T> {
        fn build(&self) {
            let params = T::Params::get();
            assert!(
                params.validate().is_ok(),
                "epoch genesis Params are invalid"
            );
            EpochOf::<T>::put(EpochInfo {
                index: self.index,
                phase: EpochPhase::Intake,
                phase_start_block: self.start_block,
            });
            Schedule::<T>::put(EpochSchedule {
                epoch_start_block: self.start_block,
                length: params.epoch_length,
                next_length: params.epoch_length,
            });
            NextProposalId::<T>::put(1);
        }
    }

    struct CheckedState<T: Config> {
        epoch: EpochInfo,
        schedule: EpochSchedule,
        intake_queue: Intake,
        intake: Vec<(ProposalId, Proposal<T::AccountId>)>,
        live: Vec<(ProposalId, Proposal<T::AccountId>)>,
        cohorts: Vec<(EpochId, CohortInfo)>,
        recent: Recent,
        locks: Locks,
        next_proposal_id: ProposalId,
        rollovers: Rollovers,
        dead_man: DeadManState,
        stale_epoch_cutoff: Option<ProposalId>,
        baseline_carry: Option<(EpochId, u8)>,
    }

    struct LedgerAdapter<T>(PhantomData<T>);

    impl<T: Config> CoreLedgerOps<T::AccountId> for LedgerAdapter<T> {
        fn create_vault(
            &mut self,
            pid: ProposalId,
            spec: MetricSpecVersion,
        ) -> Result<(), CoreError> {
            T::Ledger::create_vault(pid, spec).map_err(|_| CoreError::Ledger)
        }
        fn resolve(&mut self, pid: ProposalId, branch: Branch) -> Result<(), CoreError> {
            T::Ledger::resolve(pid, branch).map_err(|_| CoreError::Ledger)
        }
        fn void(&mut self, pid: ProposalId) -> Result<(), CoreError> {
            T::Ledger::void(pid).map_err(|_| CoreError::Ledger)
        }
    }

    struct WelfareAdapter<T>(PhantomData<T>);

    impl<T: Config> CoreWelfareOps for WelfareAdapter<T> {
        fn compute_settlement(
            &mut self,
            cohort_epoch: EpochId,
            spec: MetricSpecVersion,
            target: SettlementTarget,
        ) -> Result<FixedU64, CoreError> {
            T::Welfare::compute_settlement(cohort_epoch, spec, target)
                .map_err(|_| CoreError::Welfare)
        }
    }

    impl<T: Config> Pallet<T> {
        pub fn current_epoch() -> EpochId {
            EpochOf::<T>::get().index
        }

        pub fn epoch_state() -> EpochState<T::AccountId> {
            Self::load()
        }

        #[cfg(any(test, feature = "runtime-benchmarks"))]
        pub fn seed(state: EpochState<T::AccountId>) -> DispatchResult {
            Self::persist(state)
        }

        fn now() -> BlockNumber {
            frame_system::Pallet::<T>::block_number().saturated_into::<BlockNumber>()
        }

        fn live_params() -> Result<CoreEpochParams, DispatchError> {
            let params = T::Params::get();
            params
                .validate()
                .map_err(|_| DispatchError::from(Error::<T>::BadParams))?;
            Ok(params)
        }

        fn requires_gate_markets_at_seed(proposal: &Proposal<T::AccountId>) -> bool {
            match proposal.class {
                ProposalClass::Code | ProposalClass::Meta => true,
                ProposalClass::Treasury => T::Constitution::treasury_gate_required(proposal),
                ProposalClass::Param | ProposalClass::Constitutional => false,
            }
        }

        fn epoch_owns_prequeue_pin(state: ProposalState) -> bool {
            matches!(
                state,
                ProposalState::Qualified
                    | ProposalState::Trading
                    | ProposalState::Extended
                    | ProposalState::Rerun
            )
        }

        fn pin_at_qualification(pid: ProposalId, hash: H256) -> Result<(), CoreError> {
            if let Some(existing) = QualificationPins::<T>::get(pid) {
                return if existing == hash {
                    Ok(())
                } else {
                    Err(CoreError::TryStateViolation)
                };
            }
            T::Preimage::request(hash).map_err(|_| CoreError::BadDecisionInput)?;
            QualificationPins::<T>::insert(pid, hash);
            Ok(())
        }

        fn release_qualification_pin(pid: ProposalId) {
            if let Some(hash) = QualificationPins::<T>::take(pid) {
                // G-1: cleanup must never turn a terminal transition into an
                // error. Runtime implementations guard the underlying
                // QueryPreimage unrequest, making this an idempotent no-op if
                // external state is already missing.
                T::Preimage::unrequest(hash);
            }
        }

        fn sync_clock(
            state: &mut EpochState<T::AccountId>,
            now: BlockNumber,
        ) -> Result<(), CoreError> {
            let paused_at = state.dead_man_paused_at;
            state.sync_phase(now);
            if let Some(paused_at) = paused_at {
                if !state.dead_man_armed && state.dead_man_paused_at.is_none() {
                    let paused_for = now.saturating_sub(paused_at);
                    for proposal in state.proposals.iter().filter(|proposal| {
                        matches!(
                            proposal.state,
                            ProposalState::Trading | ProposalState::Extended
                        )
                    }) {
                        ProposalSchedules::<T>::try_mutate(proposal.id, |schedule| {
                            if let Some(schedule) = schedule {
                                schedule.decide_at = schedule
                                    .decide_at
                                    .checked_add(paused_for)
                                    .ok_or(CoreError::ArithmeticOverflow)?;
                            }
                            Ok::<(), CoreError>(())
                        })?;
                    }
                }
            }
            Ok(())
        }

        fn mutate(
            op: impl FnOnce(
                &mut EpochState<T::AccountId>,
                &mut LedgerAdapter<T>,
            ) -> Result<(), CoreError>,
        ) -> DispatchResult {
            frame_support::storage::with_storage_layer(|| {
                let mut state = Self::load();
                state.dead_man_armed = T::Guardian::dead_man_engaged();
                state.ledger_frozen = T::Constitution::ledger_frozen();
                state.phase_flags = T::Constitution::phase_flags();
                state.horizon_k = T::Params::get().horizon_k;
                let mut ledger = LedgerAdapter::<T>(PhantomData);
                op(&mut state, &mut ledger).map_err(Self::map_core_error)?;
                Self::persist(state)
            })
        }

        fn load() -> EpochState<T::AccountId> {
            let epoch = EpochOf::<T>::get();
            let schedule = Schedule::<T>::get();
            let mut proposals = IntakeProposals::<T>::iter_values().collect::<Vec<_>>();
            proposals.extend(Proposals::<T>::iter_values());
            proposals.sort_by_key(|p| p.id);
            let mut cohorts = Cohorts::<T>::iter_values()
                .map(CoreCohortInfo::from)
                .collect::<Vec<_>>();
            cohorts.sort_by_key(|c| c.epoch);
            EpochState {
                epoch: CoreEpochInfo {
                    index: epoch.index,
                    phase: epoch.phase,
                    phase_start_block: epoch.phase_start_block,
                    epoch_start_block: schedule.epoch_start_block,
                    length: schedule.length,
                    next_length: schedule.next_length,
                },
                proposals,
                intake_queue: IntakeQueue::<T>::get().into_inner(),
                cohorts,
                recent: RecentCohortSummaries::<T>::get().into_inner(),
                resource_locks: ResourceLocks::<T>::get().into_inner(),
                events: Vec::new(),
                dead_man_armed: T::Guardian::dead_man_engaged(),
                ledger_frozen: T::Constitution::ledger_frozen(),
                phase_flags: T::Constitution::phase_flags(),
                proposal_id_high_water: NextProposalId::<T>::get().saturating_sub(1),
                rollovers: RolloverCounts::<T>::get().into_inner(),
                dead_man_paused_at: DeadMan::<T>::get().paused_at,
                recovery_epoch: DeadMan::<T>::get().recovery_epoch,
                stale_epoch_cutoff: StaleEpochCutoff::<T>::get(),
                baseline_carry: BaselineCarry::<T>::get(),
                horizon_k: T::Params::get().horizon_k,
            }
        }

        fn persist(mut state: EpochState<T::AccountId>) -> DispatchResult {
            state.try_state().map_err(Self::map_core_error)?;
            let checked = Self::checked_state(&state)?;
            Self::update_frozen_schedules(&state)?;

            let old_intake = IntakeProposals::<T>::iter_keys().collect::<Vec<_>>();
            let old_live = Proposals::<T>::iter_keys().collect::<Vec<_>>();
            let old_cohorts = Cohorts::<T>::iter_keys().collect::<Vec<_>>();
            for key in old_intake {
                IntakeProposals::<T>::remove(key);
            }
            for key in old_live {
                Proposals::<T>::remove(key);
            }
            for key in old_cohorts {
                Cohorts::<T>::remove(key);
            }
            for (pid, proposal) in checked.intake {
                IntakeProposals::<T>::insert(pid, proposal);
            }
            for (pid, proposal) in checked.live {
                Proposals::<T>::insert(pid, proposal);
            }
            for (epoch, cohort) in checked.cohorts {
                Cohorts::<T>::insert(epoch, cohort);
            }
            EpochOf::<T>::put(checked.epoch);
            Schedule::<T>::put(checked.schedule);
            IntakeQueue::<T>::put(checked.intake_queue);
            RecentCohortSummaries::<T>::put(checked.recent);
            ResourceLocks::<T>::put(checked.locks);
            NextProposalId::<T>::put(checked.next_proposal_id);
            RolloverCounts::<T>::put(checked.rollovers);
            DeadMan::<T>::put(checked.dead_man);
            match checked.stale_epoch_cutoff {
                Some(cutoff) => StaleEpochCutoff::<T>::put(cutoff),
                None => StaleEpochCutoff::<T>::kill(),
            }
            match checked.baseline_carry {
                Some(carry) => BaselineCarry::<T>::put(carry),
                None => BaselineCarry::<T>::kill(),
            }
            for event in core::mem::take(&mut state.events) {
                Self::deposit_core_event(event);
            }
            Ok(())
        }

        fn checked_state(
            state: &EpochState<T::AccountId>,
        ) -> Result<CheckedState<T>, DispatchError> {
            let mut intake = Vec::new();
            let mut live = Vec::new();
            let terminal_cohort_members = state
                .cohorts
                .iter()
                .filter(|cohort| {
                    matches!(cohort.status, CohortStatus::Settled | CohortStatus::Void)
                })
                .flat_map(|cohort| cohort.proposals.iter().copied())
                .collect::<Vec<_>>();
            for proposal in &state.proposals {
                if matches!(
                    proposal.state,
                    ProposalState::Submitted | ProposalState::Screening
                ) || (proposal.state == ProposalState::Cancelled
                    && proposal.epoch == state.epoch.index)
                {
                    // Preserve current-epoch cancellations internally so a
                    // withdrawal/static-check failure cannot reset the per-account
                    // admission counter. These records never enter the frozen
                    // `Proposals` map or IntakeQueue and are reaped next epoch.
                    intake.push((proposal.id, proposal.clone()));
                } else if !matches!(
                    proposal.state,
                    ProposalState::Cancelled
                        | ProposalState::Settled
                        | ProposalState::Rejected(_)
                        | ProposalState::Expired
                ) || terminal_cohort_members.contains(&proposal.id)
                {
                    live.push((proposal.id, proposal.clone()));
                }
            }
            ensure!(intake.len() <= MAX_INTAKE_QUEUE, Error::<T>::IntakeFull);
            ensure!(
                state
                    .proposals
                    .iter()
                    .filter(|proposal| {
                        !matches!(
                            proposal.state,
                            ProposalState::Submitted
                                | ProposalState::Screening
                                | ProposalState::Cancelled
                                | ProposalState::Settled
                                | ProposalState::Rejected(_)
                                | ProposalState::Expired
                        )
                    })
                    .count()
                    <= MAX_LIVE_PROPOSALS,
                Error::<T>::TooManyLiveProposals
            );
            let cohorts = state
                .cohorts
                .iter()
                .cloned()
                .map(|cohort| {
                    let epoch = cohort.epoch;
                    CohortInfo::try_from(cohort)
                        .map(|cohort| (epoch, cohort))
                        .map_err(Self::map_core_error)
                })
                .collect::<Result<Vec<_>, DispatchError>>()?;
            ensure!(
                state
                    .cohorts
                    .iter()
                    .filter(|cohort| {
                        !matches!(cohort.status, CohortStatus::Settled | CohortStatus::Void)
                    })
                    .count()
                    <= MAX_NON_TERMINAL_COHORTS,
                Error::<T>::TooManyCohorts
            );
            Ok(CheckedState {
                epoch: EpochInfo {
                    index: state.epoch.index,
                    phase: state.epoch.phase,
                    phase_start_block: state.epoch.phase_start_block,
                },
                schedule: EpochSchedule {
                    epoch_start_block: state.epoch.epoch_start_block,
                    length: state.epoch.length,
                    next_length: state.epoch.next_length,
                },
                intake_queue: BoundedVec::try_from(state.intake_queue.clone())
                    .map_err(|_| Error::<T>::IntakeFull)?,
                intake,
                live,
                cohorts,
                recent: BoundedVec::try_from(state.recent.clone())
                    .map_err(|_| Error::<T>::TryStateViolation)?,
                locks: BoundedVec::try_from(state.resource_locks.clone())
                    .map_err(|_| Error::<T>::TooManyResources)?,
                next_proposal_id: state
                    .proposal_id_high_water
                    .checked_add(1)
                    .ok_or(Error::<T>::ArithmeticOverflow)?,
                rollovers: BoundedVec::try_from(state.rollovers.clone())
                    .map_err(|_| Error::<T>::IntakeFull)?,
                dead_man: DeadManState {
                    paused_at: state.dead_man_paused_at,
                    recovery_epoch: state.recovery_epoch,
                },
                stale_epoch_cutoff: state.stale_epoch_cutoff,
                baseline_carry: state.baseline_carry,
            })
        }

        fn update_frozen_schedules(state: &EpochState<T::AccountId>) -> DispatchResult {
            let terminal_cohort_members = state
                .cohorts
                .iter()
                .filter(|cohort| {
                    matches!(cohort.status, CohortStatus::Settled | CohortStatus::Void)
                })
                .flat_map(|cohort| cohort.proposals.iter().copied())
                .collect::<Vec<_>>();
            let live_ids = state
                .proposals
                .iter()
                .filter(|p| {
                    !matches!(
                        p.state,
                        ProposalState::Submitted
                            | ProposalState::Screening
                            | ProposalState::Cancelled
                            | ProposalState::Settled
                            | ProposalState::Rejected(_)
                            | ProposalState::Expired
                    ) || terminal_cohort_members.contains(&p.id)
                })
                .map(|p| p.id)
                .collect::<Vec<_>>();
            for pid in ProposalSchedules::<T>::iter_keys().collect::<Vec<_>>() {
                if !live_ids.contains(&pid) {
                    ProposalSchedules::<T>::remove(pid);
                }
            }
            for proposal in state.proposals.iter().filter(|p| live_ids.contains(&p.id)) {
                if let Some(frozen) = ProposalSchedules::<T>::get(proposal.id) {
                    let extended_decide_at =
                        frozen.decide_at.checked_add(epoch_core::DECISION_EXTENSION);
                    ensure!(
                        frozen.epoch == proposal.epoch
                            && frozen.metric_spec == proposal.metric_spec
                            && (if proposal.rerun {
                                frozen.decide_at == proposal.decide_at
                            } else {
                                frozen.decide_at == proposal.decide_at
                                    || (proposal.extended
                                        && extended_decide_at == Some(proposal.decide_at))
                            }),
                        Error::<T>::TryStateViolation
                    );
                } else {
                    ProposalSchedules::<T>::insert(
                        proposal.id,
                        ProposalSchedule {
                            epoch: proposal.epoch,
                            epoch_start_block: state.epoch.epoch_start_block,
                            epoch_length: state.epoch.length,
                            decide_at: proposal.decide_at,
                            metric_spec: proposal.metric_spec,
                        },
                    );
                }
            }

            let cohort_epochs = state.cohorts.iter().map(|c| c.epoch).collect::<Vec<_>>();
            for epoch in CohortSchedules::<T>::iter_keys().collect::<Vec<_>>() {
                if !cohort_epochs.contains(&epoch) {
                    CohortSchedules::<T>::remove(epoch);
                }
            }
            for cohort in &state.cohorts {
                let mut bindings = Vec::new();
                let mut creation_length = None;
                for pid in &cohort.proposals {
                    let proposal = state.proposal_view(*pid).map_err(Self::map_core_error)?;
                    let schedule =
                        ProposalSchedules::<T>::get(pid).ok_or(Error::<T>::TryStateViolation)?;
                    creation_length.get_or_insert(schedule.epoch_length);
                    bindings.push((*pid, proposal.metric_spec));
                }
                let specs = BoundedVec::try_from(bindings)
                    .map_err(|_| Error::<T>::TooManyCohortProposals)?;
                let previously_frozen = CohortSchedules::<T>::get(cohort.epoch);
                let measurement_until = match cohort.status {
                    CohortStatus::Measuring { until_epoch } => until_epoch,
                    _ => previously_frozen.as_ref().map_or_else(
                        || cohort.epoch.saturating_add(state.horizon_k.into()),
                        |frozen| frozen.measurement_until,
                    ),
                };
                let candidate = CohortSchedule {
                    epoch: cohort.epoch,
                    creation_epoch_length: creation_length.unwrap_or(state.epoch.length),
                    measurement_until,
                    settlement_epoch: measurement_until.saturating_add(1),
                    specs,
                };
                if let Some(frozen) = previously_frozen {
                    ensure!(frozen == candidate, Error::<T>::TryStateViolation);
                } else {
                    CohortSchedules::<T>::insert(cohort.epoch, candidate);
                }
            }
            Ok(())
        }

        fn deposit_core_event(event: CoreEvent) {
            let mapped = match event {
                CoreEvent::ProposalSubmitted(pid) => Some(Event::ProposalSubmitted(pid)),
                CoreEvent::ProposalWithdrawn(pid) => Some(Event::ProposalWithdrawn(pid)),
                CoreEvent::ScreeningStarted(pid) => Some(Event::ScreeningStarted(pid)),
                CoreEvent::ProposalCancelled { pid, reason } => {
                    Some(Event::ProposalCancelled { pid, reason })
                }
                CoreEvent::ProposalQualified(pid) => Some(Event::ProposalQualified(pid)),
                CoreEvent::ProposalDeferred(pid) => Some(Event::ProposalDeferred(pid)),
                CoreEvent::MarketsOpened(pid) => Some(Event::MarketsOpened(pid)),
                CoreEvent::DecisionExtended(pid) => Some(Event::DecisionExtended(pid)),
                CoreEvent::ProposalQueued {
                    pid,
                    payload_hash,
                    maturity,
                } => Some(Event::ProposalQueued {
                    pid,
                    payload_hash,
                    maturity,
                }),
                CoreEvent::ProposalRejected { pid, reason } => {
                    Some(Event::ProposalRejected { pid, reason })
                }
                CoreEvent::ProposalDelayed {
                    pid,
                    justification_hash,
                } => Some(Event::ProposalDelayed {
                    pid,
                    justification_hash,
                }),
                CoreEvent::RerunScheduled(pid) => Some(Event::RerunScheduled(pid)),
                CoreEvent::RerunOpened(pid) => Some(Event::RerunOpened(pid)),
                CoreEvent::MandateExpired(pid) => Some(Event::MandateExpired(pid)),
                CoreEvent::MeasurementStarted { cohort } => {
                    Some(Event::MeasurementStarted { cohort })
                }
                CoreEvent::CohortSettled { epoch, s } => Some(Event::CohortSettled { epoch, s }),
                CoreEvent::CohortVoided { epoch } => Some(Event::CohortVoided { epoch }),
                CoreEvent::BaselineCarried { pid, epoch } => {
                    Some(Event::BaselineCarried { pid, epoch })
                }
                CoreEvent::ProposalForceRejected { pid, reason } => {
                    Some(Event::ProposalForceRejected { pid, reason })
                }
                CoreEvent::IntakeSlashed {
                    pid,
                    reason,
                    amount,
                } => Some(Event::IntakeSlashed {
                    pid,
                    reason,
                    amount,
                }),
                // Owned by pallet-execution-guard in 02 §6; epoch never emits it.
                CoreEvent::ExecutionFailed { .. } | CoreEvent::NoOp => None,
            };
            if let Some(event) = mapped {
                Self::deposit_event(event);
            }
        }

        pub fn do_try_state() -> Result<(), TryRuntimeError> {
            let state = Self::load();
            state
                .try_state()
                .map_err(|_| TryRuntimeError::Other("epoch core try_state failed (I-16/I-21)"))?;
            T::Params::get()
                .validate()
                .map_err(|_| TryRuntimeError::Other("epoch live Params are invalid"))?;
            let live_proposals = Proposals::<T>::iter_values()
                .filter(|proposal| {
                    !matches!(
                        proposal.state,
                        ProposalState::Submitted
                            | ProposalState::Screening
                            | ProposalState::Cancelled
                            | ProposalState::Settled
                            | ProposalState::Rejected(_)
                            | ProposalState::Expired
                    )
                })
                .count();
            let non_terminal_cohorts = Cohorts::<T>::iter_values()
                .filter(|cohort| {
                    !matches!(cohort.status, CohortStatus::Settled | CohortStatus::Void)
                })
                .count();
            if live_proposals > MAX_LIVE_PROPOSALS
                || IntakeProposals::<T>::count() > MAX_INTAKE_QUEUE_BOUND
                || non_terminal_cohorts > MAX_NON_TERMINAL_COHORTS
                || IntakeQueue::<T>::get().len() > MAX_INTAKE_QUEUE
                || RecentCohortSummaries::<T>::get().len() > RECENT_COHORTS
            {
                return Err(TryRuntimeError::Other("epoch FRAME bound exceeded (I-21)"));
            }
            for (pid, proposal) in IntakeProposals::<T>::iter() {
                if pid != proposal.id
                    || !(matches!(
                        proposal.state,
                        ProposalState::Submitted | ProposalState::Screening
                    ) || (proposal.state == ProposalState::Cancelled
                        && proposal.epoch == EpochOf::<T>::get().index))
                {
                    return Err(TryRuntimeError::Other(
                        "epoch intake proposal map key/state mismatch",
                    ));
                }
            }
            for (pid, proposal) in Proposals::<T>::iter() {
                if pid != proposal.id {
                    return Err(TryRuntimeError::Other(
                        "epoch proposal map key does not match value",
                    ));
                }
                let schedule = ProposalSchedules::<T>::get(pid).ok_or(TryRuntimeError::Other(
                    "epoch live proposal lacks frozen schedule",
                ))?;
                let extended_decide_at = schedule
                    .decide_at
                    .checked_add(epoch_core::DECISION_EXTENSION);
                if schedule.epoch != proposal.epoch
                    || schedule.metric_spec != proposal.metric_spec
                    || !(if proposal.rerun {
                        schedule.decide_at == proposal.decide_at
                    } else {
                        schedule.decide_at == proposal.decide_at
                            || (proposal.extended && extended_decide_at == Some(proposal.decide_at))
                    })
                {
                    return Err(TryRuntimeError::Other(
                        "epoch I-16 proposal schedule/spec binding changed",
                    ));
                }
            }
            if ProposalSchedules::<T>::iter_keys().any(|pid| !Proposals::<T>::contains_key(pid)) {
                return Err(TryRuntimeError::Other(
                    "epoch orphan proposal schedule violates I-16",
                ));
            }
            for (epoch, cohort) in Cohorts::<T>::iter() {
                if epoch != cohort.epoch {
                    return Err(TryRuntimeError::Other(
                        "epoch cohort map key does not match value",
                    ));
                }
                let frozen = CohortSchedules::<T>::get(epoch)
                    .ok_or(TryRuntimeError::Other("epoch cohort lacks frozen schedule"))?;
                if frozen.epoch != epoch
                    || frozen.settlement_epoch != frozen.measurement_until.saturating_add(1)
                    || frozen.specs.len() != cohort.proposals.len()
                    || frozen.specs.iter().any(|(pid, spec)| {
                        Proposals::<T>::get(pid)
                            .is_none_or(|proposal| proposal.metric_spec != *spec)
                            || ProposalSchedules::<T>::get(pid).is_none_or(|schedule| {
                                schedule.metric_spec != *spec
                                    || schedule.epoch != epoch
                                    || schedule.epoch_length != frozen.creation_epoch_length
                            })
                    })
                    || matches!(
                        cohort.status,
                        CohortStatus::Measuring { until_epoch }
                            if until_epoch != frozen.measurement_until
                    )
                {
                    return Err(TryRuntimeError::Other(
                        "epoch I-16 cohort schedule/spec binding changed",
                    ));
                }
            }
            if CohortSchedules::<T>::iter_keys().any(|epoch| !Cohorts::<T>::contains_key(epoch)) {
                return Err(TryRuntimeError::Other(
                    "epoch orphan cohort schedule violates I-16",
                ));
            }
            for (pid, hash) in QualificationPins::<T>::iter() {
                let proposal = Proposals::<T>::get(pid).ok_or(TryRuntimeError::Other(
                    "epoch qualification preimage pin is orphaned",
                ))?;
                if proposal.payload_hash != hash || !Self::epoch_owns_prequeue_pin(proposal.state) {
                    return Err(TryRuntimeError::Other(
                        "epoch qualification preimage pin outlived pre-queue ownership",
                    ));
                }
            }
            Ok(())
        }

        pub(crate) fn map_core_error(error: CoreError) -> DispatchError {
            match error {
                CoreError::BadOrigin => DispatchError::BadOrigin,
                CoreError::BadPhase => Error::<T>::BadPhase.into(),
                CoreError::IntakeFull => Error::<T>::IntakeFull.into(),
                CoreError::TooManyLiveProposals => Error::<T>::TooManyLiveProposals.into(),
                CoreError::TooManyResources => Error::<T>::TooManyResources.into(),
                CoreError::UnknownProposal => Error::<T>::UnknownProposal.into(),
                CoreError::BadState => Error::<T>::BadState.into(),
                CoreError::DuplicateProposal => Error::<T>::DuplicateProposal.into(),
                CoreError::LockConflict => Error::<T>::LockConflict.into(),
                CoreError::TooManyCohorts => Error::<T>::TooManyCohorts.into(),
                CoreError::TooManyCohortProposals => Error::<T>::TooManyCohortProposals.into(),
                CoreError::BadEpochLength => Error::<T>::BadEpochLength.into(),
                CoreError::BadParams => Error::<T>::BadParams.into(),
                CoreError::BadDecisionInput => Error::<T>::BadDecisionInput.into(),
                CoreError::BatchTooLarge => Error::<T>::BatchTooLarge.into(),
                CoreError::ArithmeticOverflow => Error::<T>::ArithmeticOverflow.into(),
                CoreError::Ledger => Error::<T>::Ledger.into(),
                CoreError::ExecutionGuard => Error::<T>::ExecutionGuard.into(),
                CoreError::Welfare => Error::<T>::Welfare.into(),
                CoreError::TryStateViolation => Error::<T>::TryStateViolation.into(),
            }
        }
    }
}

// Small public helpers used by the shell without exposing mutable internals.
trait EpochStateView<AccountId> {
    fn proposal_view(&self, pid: ProposalId) -> Result<&Proposal<AccountId>, CoreError>;
    fn resource_locks_held(&self, pid: ProposalId) -> bool;
}

impl<AccountId: Clone + Eq> EpochStateView<AccountId> for EpochState<AccountId> {
    fn proposal_view(&self, pid: ProposalId) -> Result<&Proposal<AccountId>, CoreError> {
        self.proposals
            .iter()
            .find(|proposal| proposal.id == pid)
            .ok_or(CoreError::UnknownProposal)
    }

    fn resource_locks_held(&self, pid: ProposalId) -> bool {
        self.proposals
            .iter()
            .find(|proposal| proposal.id == pid)
            .is_some_and(|proposal| {
                proposal.resources.iter().all(|resource| {
                    self.resource_locks
                        .iter()
                        .any(|(locked, owner)| locked == resource && *owner == pid)
                })
            })
    }
}

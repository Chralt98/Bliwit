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
    attack_cost_hat, decision_converged, effective_baseline_twaps, effective_reject_1e9,
    CohortInfo as CoreCohortInfo, CohortStatus, DecisionGuards, DecisionInputs,
    EpochInfo as CoreEpochInfo, EpochParams as CoreEpochParams, EpochState, Error as CoreError,
    Event as CoreEvent, LedgerOps as CoreLedgerOps, Origin as CoreOrigin, SettlementTarget,
    StaticCheckDisposition, TickInputs, WelfareOps as CoreWelfareOps, MAX_ACTIVE_PER_EPOCH,
    MAX_INTAKE_QUEUE, MAX_LIVE_PROPOSALS, MAX_NON_TERMINAL_COHORTS, MAX_RESOURCES_PER_PROPOSAL,
    RECENT_COHORTS,
};

pub const MAX_INTAKE_QUEUE_BOUND: u32 = MAX_INTAKE_QUEUE as u32;
pub const MAX_LIVE_PROPOSALS_BOUND: u32 = MAX_LIVE_PROPOSALS as u32;
pub const MAX_COHORT_PROPOSALS_BOUND: u32 = MAX_ACTIVE_PER_EPOCH as u32;
pub const MAX_NON_TERMINAL_COHORTS_BOUND: u32 = MAX_NON_TERMINAL_COHORTS as u32;
pub const RECENT_COHORTS_BOUND: u32 = RECENT_COHORTS as u32;
pub const MAX_RESOURCE_LOCKS_BOUND: u32 =
    MAX_LIVE_PROPOSALS_BOUND * MAX_RESOURCES_PER_PROPOSAL as u32;
pub const MAX_PROPOSAL_BONDS_BOUND: u32 = MAX_INTAKE_QUEUE_BOUND + MAX_LIVE_PROPOSALS_BOUND;
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

/// One immutable read of every market/constitution input consumed by
/// `decide()`. The crank and `FutarchyApi::decision_stats` share the private
/// assembly routine that produces this value; `backing_complete` lets the
/// read-only view return `None` where the crank's fail-closed zero sentinel
/// would otherwise hide an unavailable market read.
#[derive(Clone, Debug)]
pub struct DecisionInputSnapshot<AccountId> {
    pub proposal: Proposal<AccountId>,
    pub params: CoreEpochParams,
    pub inputs: DecisionInputs,
    pub backing_complete: bool,
}

/// Decision-book reads and Seed/rerun market deployment (A3 → A8).
pub trait MarketAccess<AccountId> {
    fn open_markets(
        proposal: &Proposal<AccountId>,
        rerun: bool,
        requires_gate_markets: bool,
    ) -> Result<MarketSet, DispatchError>;
    /// Register the one permitted decision extension against the existing
    /// books. If an exact fresh window cannot be exposed, the proposal must
    /// not be persisted as Extended (G-1).
    fn extend_markets(proposal: &Proposal<AccountId>) -> Result<(), DispatchError>;
    /// Reset/reopen all proposal-owned books for an immediate guardian
    /// force-rerun. The shared Baseline is reopened if needed but never reset.
    fn force_rerun_markets(proposal: &Proposal<AccountId>) -> Result<(), DispatchError>;
    /// Seal the proposal books after a final decision. The shared Baseline is
    /// closed by the adapter only when its last proposal has decided.
    fn close_markets(proposal: &Proposal<AccountId>) -> Result<(), DispatchError>;
    /// Seal this proposal's exact frozen decision boundary on every book,
    /// including its shared Baseline window.
    fn seal_decision_window(proposal: &Proposal<AccountId>) -> Result<(), DispatchError>;
    fn baseline_market(epoch: EpochId) -> Option<MarketId>;
    fn twap_full(market: MarketId) -> Option<FixedU64>;
    fn twap_full_at(market: MarketId, end: BlockNumber) -> Option<FixedU64>;
    fn twap_trailing_at(
        market: MarketId,
        end: BlockNumber,
        window: BlockNumber,
    ) -> Option<FixedU64>;
    fn spot_at(market: MarketId, end: BlockNumber) -> Option<FixedU64>;
    /// Returns false for an unavailable or ungraded book.
    fn decision_grade(
        market: MarketId,
        end: BlockNumber,
        role: BookRole,
        class: ProposalClass,
        params: &CoreEpochParams,
    ) -> bool;
    fn measured_depth(pid: ProposalId) -> Option<Balance>;
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
    fn required_bond(proposal: &Proposal<AccountId>) -> Option<Balance>;
    fn static_check(proposal: &Proposal<AccountId>) -> StaticCheckDisposition;
    fn queue_time_check(proposal: &Proposal<AccountId>) -> bool;
    fn in_cap_prize(proposal: &Proposal<AccountId>) -> Option<Balance>;
    fn ledger_frozen() -> bool;
    fn phase_flags() -> u32;
    fn active_metric_spec_version() -> Option<MetricSpecVersion>;
    fn treasury_gate_required(proposal: &Proposal<AccountId>) -> bool;
    /// Canonical CODE/META artifact commitment checked by attestors. `None`
    /// is an ambiguous payload and therefore blocks adoption.
    fn attestation_artifact(proposal: &Proposal<AccountId>) -> Option<H256>;
}

/// Real USDC escrow used for proposal-bond custody.
pub trait ProposalBondCurrency<AccountId> {
    fn hold(who: &AccountId, amount: Balance) -> DispatchResult;
    fn release(who: &AccountId, amount: Balance) -> DispatchResult;
    fn slash_to_insurance(amount: Balance) -> DispatchResult;
    fn escrow_balance() -> Balance;
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
    fn dequeue_for_rerun(pid: ProposalId) -> DispatchResult;
}

/// Sole settlement hand-off (05 §6). The implementation is pallet-welfare,
/// which alone owns ledger SettleAuthority.
pub trait WelfareSettlement {
    fn compute_settlement(
        cohort_epoch: EpochId,
        spec: MetricSpecVersion,
        target: SettlementTarget,
    ) -> Result<FixedU64, DispatchError>;
    /// Retire welfare history after the completed cohort has been reaped.
    /// The implementation derives its bounded rolling-window cutoff from the
    /// supplied live epoch, keeping the retention constant single-homed.
    fn prune(current_epoch: EpochId) -> DispatchResult;
    /// Drain one bounded XCM-traffic retirement batch for the live epoch.
    /// Tick invokes this unconditionally so a pathological historical backlog
    /// is retried until empty even if another crank crossed the epoch boundary.
    fn prune_xcm_traffic(current_epoch: EpochId) -> DispatchResult;
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
        type ProposalBond: ProposalBondCurrency<Self::AccountId>;
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

    /// Bounded completed-epoch timing retained for post-close oracle and
    /// registry windows when `epoch.length` changes at a boundary.
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
    pub struct EpochTiming {
        pub index: EpochId,
        pub start: BlockNumber,
        pub length: BlockNumber,
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
        Clone, Debug, Decode, DecodeWithMemTracking, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo,
    )]
    pub struct ProposalBond<AccountId> {
        pub proposer: AccountId,
        pub held: Balance,
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
    pub type EpochTimings<T: Config> =
        StorageValue<_, BoundedVec<EpochTiming, ConstU32<RECENT_COHORTS_BOUND>>, ValueQuery>;

    /// Internal delayed-proposal→review-deadline join. The guardian effect
    /// producer writes it atomically with `delay_once`; it is removed when T12
    /// or T24 consumes the hold. Cardinality is bounded by `Proposals`.
    #[pallet::storage]
    pub type GuardianReviewDeadlines<T: Config> =
        CountedStorageMap<_, Blake2_128Concat, ProposalId, EpochId, OptionQuery>;

    /// Explicit qualification-era preimage ownership. State alone is not an
    /// ownership proof once a rerun transfers the pin to the execution guard.
    #[pallet::storage]
    pub type QualificationPreimageRequests<T: Config> =
        CountedStorageMap<_, Blake2_128Concat, ProposalId, H256, OptionQuery>;

    /// Internal bounded USDC escrow liabilities, one per admitted proposal.
    #[pallet::storage]
    pub type ProposalBonds<T: Config> =
        CountedStorageMap<_, Blake2_128Concat, ProposalId, ProposalBond<T::AccountId>, OptionQuery>;

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
            let required =
                T::Constitution::required_bond(&proposal).ok_or(Error::<T>::BadProposalShape)?;
            ensure!(proposal.bond >= required, Error::<T>::BadProposalShape);
            ensure!(
                ProposalBonds::<T>::count() < MAX_PROPOSAL_BONDS_BOUND,
                Error::<T>::TooManyLiveProposals
            );
            frame_support::storage::with_storage_layer(|| {
                T::ProposalBond::hold(&who, proposal.bond)?;
                ProposalBonds::<T>::insert(
                    proposal.id,
                    ProposalBond {
                        proposer: who,
                        held: proposal.bond,
                    },
                );
                Self::mutate(|state, _| state.submit(CoreOrigin::Signed, proposal, &params))
            })
        }

        #[pallet::call_index(1)]
        #[pallet::weight(T::WeightInfo::withdraw())]
        pub fn withdraw(origin: OriginFor<T>, pid: ProposalId) -> DispatchResult {
            let who = ensure_signed(origin)?;
            Self::mutate(|state, _| {
                state.withdraw(CoreOrigin::Signed, pid, &who)?;
                Self::release_qualification_preimage(pid);
                T::ExecutionGuard::dequeue_terminal(pid).map_err(|_| CoreError::ExecutionGuard)
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
            let result = frame_support::storage::with_storage_layer(|| {
                Self::mutate(|state, ledger| {
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
                    let mut ordered_pids = pids.into_inner();
                    ordered_pids.sort_by(|pid_a, pid_b| {
                        let proposal_a = state.proposal_view(*pid_a).ok();
                        let proposal_b = state.proposal_view(*pid_b).ok();
                        let qualifies_a = proposal_a.is_some_and(|proposal| {
                            proposal.state == ProposalState::Submitted
                                && proposal.epoch == state.epoch.index
                                && state.epoch.phase == EpochPhase::Qualify
                        });
                        let qualifies_b = proposal_b.is_some_and(|proposal| {
                            proposal.state == ProposalState::Submitted
                                && proposal.epoch == state.epoch.index
                                && state.epoch.phase == EpochPhase::Qualify
                        });
                        match (qualifies_a, qualifies_b) {
                            (true, true) => proposal_b
                                .map(|proposal| proposal.bond)
                                .cmp(&proposal_a.map(|proposal| proposal.bond))
                                .then_with(|| pid_a.cmp(pid_b)),
                            (true, false) => core::cmp::Ordering::Less,
                            (false, true) => core::cmp::Ordering::Greater,
                            (false, false) => pid_a.cmp(pid_b),
                        }
                    });
                    for pid in ordered_pids {
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
                            && T::Preimage::len(proposal.payload_hash)
                                == Some(proposal.payload_len);
                        let events_before = state.events.len();
                        let active_metric_spec = T::Constitution::active_metric_spec_version();
                        state.tick(
                            CoreOrigin::Keeper,
                            ledger,
                            pid,
                            now,
                            TickInputs {
                                static_check: T::Constitution::static_check(&proposal),
                                preimage_ok,
                                active_metric_spec_version: active_metric_spec,
                                markets,
                                review_window_closed: T::Guardian::review_window_closed(pid),
                                queue_reject_reason: T::ExecutionGuard::queue_reject_reason(pid),
                                retry_exhausted: T::ExecutionGuard::retry_exhausted(pid),
                            },
                            &params,
                        )?;
                        let state_after = state.proposal_view(pid)?.state;
                        if proposal.state != ProposalState::Qualified
                            && state_after == ProposalState::Qualified
                        {
                            Self::request_qualification_preimage(pid, proposal.payload_hash)?;
                        } else if Self::is_terminal(state_after) {
                            Self::release_qualification_preimage(pid);
                        }
                        advanced |= state
                            .events
                            .iter()
                            .skip(events_before)
                            .any(|event| !matches!(event, CoreEvent::NoOp));
                        if proposal.state == ProposalState::Suspended
                            && state.proposal_view(pid)?.state != ProposalState::Suspended
                        {
                            GuardianReviewDeadlines::<T>::remove(pid);
                        }
                        if proposal.state == ProposalState::Suspended
                            && state_after == ProposalState::Rerun
                        {
                            T::ExecutionGuard::dequeue_for_rerun(pid)
                                .map_err(|_| CoreError::ExecutionGuard)?;
                        } else if Self::is_terminal(state_after)
                            || (matches!(
                                proposal.state,
                                ProposalState::Queued
                                    | ProposalState::FailedExecuted
                                    | ProposalState::Suspended
                            ) && !matches!(
                                state_after,
                                ProposalState::Queued
                                    | ProposalState::FailedExecuted
                                    | ProposalState::Suspended
                                    | ProposalState::Rerun
                            ))
                        {
                            // Universal idempotent terminal hook. This also reaps
                            // pre-queue ratification/attestation records where no
                            // Queue entry ever existed, and retained Rerun pins on
                            // T20 paths that state-based ownership inference misses.
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
                })?;
                // Stateless retry hook: a clock-crossing trigger can be missed
                // when decide/settle_cohort advances the clock first. Every
                // successful tick therefore drains one bounded welfare batch;
                // the steady-state empty path is only an index read.
                T::Welfare::prune_xcm_traffic(EpochOf::<T>::get().index)
                    .map_err(|_| Error::<T>::Welfare)?;
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
                T::Market::seal_decision_window(&proposal)?;
                let input = Self::assemble_decision_input_snapshot(&state, pid, params)?.inputs;
                let guards = DecisionGuards {
                    preimage_ok: proposal.payload_len <= futarchy_primitives::kernel::MAX_BYTES
                        && T::Preimage::len(proposal.payload_hash) == Some(proposal.payload_len),
                    resource_locks_held: state.resource_locks_held(pid),
                    process_hold: T::Oracle::any_open_dispute_touching(proposal.metric_spec)
                        || T::Guardian::hold_active(pid)
                        || T::Guardian::dead_man_engaged()
                        || state.stale_process_hold(pid),
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
                if outcome == DecisionOutcome::Extend {
                    let extended = state.proposal_view(pid).map_err(Self::map_core_error)?;
                    T::Market::extend_markets(extended)?;
                } else {
                    let decided = state.proposal_view(pid).map_err(Self::map_core_error)?;
                    T::Market::close_markets(decided)?;
                }
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
                    // The guard pinned the same preimage before accepting its
                    // queue write; release epoch's qualification-era request.
                    Self::release_qualification_preimage(pid);
                } else if matches!(outcome, DecisionOutcome::Reject(_)) {
                    Self::release_qualification_preimage(pid);
                    T::ExecutionGuard::dequeue_terminal(pid)?;
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
                        Self::release_qualification_preimage(pid);
                    }
                    // 05 §3.3: cohort reap is a precondition for retiring the
                    // rolling welfare window. Keep the two state changes atomic.
                    T::Welfare::prune(state.epoch.index).map_err(|_| Error::<T>::Welfare)?;
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
            let result = Self::mutate(|state, ledger| {
                // T24: a `Suspended` proposal was `Queued` before `delay_once`, so A11
                // still owns its queue entry, preimage pin and resource locks. Upholding
                // the veto drives it to the terminal rejected/measuring outcome; release
                // the guard state in lockstep (idempotent — a no-op if nothing is queued).
                state.veto_upheld(CoreOrigin::GuardianHold, ledger, pid)?;
                T::ExecutionGuard::dequeue_terminal(pid).map_err(|_| CoreError::ExecutionGuard)
            });
            if result.is_ok() {
                GuardianReviewDeadlines::<T>::remove(pid);
            }
            result
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
                Self::release_qualification_preimage(pid);
                GuardianReviewDeadlines::<T>::remove(pid);
                T::ExecutionGuard::dequeue_terminal(pid).map_err(|_| CoreError::ExecutionGuard)
            })
        }

        #[pallet::call_index(13)]
        #[pallet::weight(T::WeightInfo::void_cohort(MAX_COHORT_PROPOSALS_BOUND))]
        pub fn void_cohort(origin: OriginFor<T>, epoch: EpochId) -> DispatchResult {
            T::VoidAuthority::ensure_origin(origin)?;
            Self::mutate(|state, ledger| {
                let proposals = state.void_affected_proposals(epoch)?;
                state.void_cohort(CoreOrigin::VoidAuthority, ledger, epoch, Self::now())?;
                for pid in proposals {
                    Self::release_qualification_preimage(pid);
                    GuardianReviewDeadlines::<T>::remove(pid);
                    T::ExecutionGuard::dequeue_terminal(pid)
                        .map_err(|_| CoreError::ExecutionGuard)?;
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
        #[pallet::constant_name(PhaseOffsets)]
        fn phase_offsets() -> [(u32, u32); 7] {
            futarchy_primitives::phase_offsets::ORDERED
        }
        #[pallet::constant_name(MaxBooksPerProposal)]
        fn max_books_per_proposal() -> u32 {
            futarchy_primitives::bounds::BOOKS_PER_PROPOSAL
        }
        #[pallet::constant_name(MinEpochLength)]
        fn min_epoch_length() -> u32 {
            futarchy_primitives::kernel::MIN_EPOCH_LENGTH_BLOCKS
        }
        #[pallet::constant_name(DecisionWindowFloor)]
        fn decision_window_floor() -> u32 {
            futarchy_primitives::kernel::DECISION_WINDOW_FLOOR_BLOCKS
        }
        #[pallet::constant_name(DecisionExtension)]
        fn decision_extension() -> u32 {
            futarchy_primitives::kernel::DEC_EXTENSION_BLOCKS
        }
        #[pallet::constant_name(DecisionDeltaFloors)]
        fn decision_delta_floors() -> [FixedU64; 4] {
            futarchy_primitives::kernel::DECISION_DELTA_FLOORS
        }
        #[pallet::constant_name(DecisionSigmaFloors)]
        fn decision_sigma_floors() -> [FixedU64; 4] {
            futarchy_primitives::kernel::DECISION_SIGMA_FLOORS
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

        /// Read the exact 02 §3 decision inputs the crank assembles for `pid`.
        /// This accessor never seals windows or writes storage. Callers that
        /// require a complete public view MUST reject `backing_complete ==
        /// false`; `published_flow_per_day == None` remains complete because
        /// 05 §5.6/08 §5.2 define it as the measured-depth/2 fallback.
        pub fn decision_input_snapshot(
            pid: ProposalId,
        ) -> Option<DecisionInputSnapshot<T::AccountId>> {
            let params = Self::live_params().ok()?;
            let state = Self::load();
            Self::assemble_decision_input_snapshot(&state, pid, params).ok()
        }

        pub fn epoch_timing(index: EpochId) -> Option<EpochTiming> {
            let current = EpochOf::<T>::get();
            let schedule = Schedule::<T>::get();
            if current.index == index {
                return Some(EpochTiming {
                    index,
                    start: schedule.epoch_start_block,
                    length: schedule.length,
                });
            }
            EpochTimings::<T>::get()
                .iter()
                .find(|timing| timing.index == index)
                .copied()
        }

        /// Runtime-internal producer for the guardian's fifth-approval effect.
        /// The guardian pallet has already checked membership, allowance and
        /// status; epoch rechecks the state transition and performs market /
        /// queue changes atomically under its sovereign wiring (06 §5.3).
        pub fn force_rerun_from_guardian(pid: ProposalId) -> DispatchResult {
            frame_support::storage::with_storage_layer(|| {
                let now = Self::now();
                let mut state = Self::load();
                let was_queued = state
                    .proposal_view(pid)
                    .map_err(Self::map_core_error)?
                    .state
                    == ProposalState::Queued;
                state
                    .force_rerun(CoreOrigin::GuardianHold, pid, now)
                    .map_err(Self::map_core_error)?;
                if was_queued {
                    T::ExecutionGuard::dequeue_for_rerun(pid)?;
                }
                let proposal = state
                    .proposal_view(pid)
                    .map_err(Self::map_core_error)?
                    .clone();
                T::Market::force_rerun_markets(&proposal)?;
                ProposalSchedules::<T>::try_mutate(pid, |schedule| -> DispatchResult {
                    let schedule = schedule.as_mut().ok_or(Error::<T>::TryStateViolation)?;
                    schedule.decide_at = proposal.decide_at;
                    Ok(())
                })?;
                Self::persist(state)
            })
        }

        /// Bind the retrospective-review deadline to a delayed proposal. This
        /// durable join survives guardian action-record maintenance and is
        /// consumed by T12/T24.
        pub fn note_guardian_review_deadline(pid: ProposalId, deadline: EpochId) -> DispatchResult {
            let proposal = Proposals::<T>::get(pid).ok_or(Error::<T>::UnknownProposal)?;
            ensure!(
                proposal.state == ProposalState::Suspended && deadline > EpochOf::<T>::get().index,
                Error::<T>::BadState
            );
            ensure!(
                GuardianReviewDeadlines::<T>::contains_key(pid)
                    || GuardianReviewDeadlines::<T>::count() < MAX_LIVE_PROPOSALS_BOUND,
                Error::<T>::TryStateViolation
            );
            GuardianReviewDeadlines::<T>::insert(pid, deadline);
            Ok(())
        }

        #[cfg(any(test, feature = "runtime-benchmarks"))]
        pub fn seed(state: EpochState<T::AccountId>) -> DispatchResult {
            Self::persist(state)
        }

        fn now() -> BlockNumber {
            frame_system::Pallet::<T>::block_number().saturated_into::<BlockNumber>()
        }

        fn request_qualification_preimage(
            pid: ProposalId,
            payload_hash: H256,
        ) -> Result<(), CoreError> {
            if QualificationPreimageRequests::<T>::contains_key(pid) {
                return Err(CoreError::TryStateViolation);
            }
            if QualificationPreimageRequests::<T>::count() >= MAX_LIVE_PROPOSALS_BOUND {
                return Err(CoreError::TooManyLiveProposals);
            }
            T::Preimage::request(payload_hash).map_err(|_| CoreError::BadDecisionInput)?;
            QualificationPreimageRequests::<T>::insert(pid, payload_hash);
            Ok(())
        }

        fn release_qualification_preimage(pid: ProposalId) {
            if let Some(payload_hash) = QualificationPreimageRequests::<T>::take(pid) {
                T::Preimage::unrequest(payload_hash);
            }
        }

        fn is_terminal(state: ProposalState) -> bool {
            matches!(
                state,
                ProposalState::Cancelled
                    | ProposalState::Settled
                    | ProposalState::Rejected(_)
                    | ProposalState::Expired
            )
        }

        fn live_params() -> Result<CoreEpochParams, DispatchError> {
            let params = T::Params::get();
            params
                .validate()
                .map_err(|_| DispatchError::from(Error::<T>::BadParams))?;
            Ok(params)
        }

        /// Single assembly point for the values consumed by `decide_with` and
        /// exposed by `decision_input_snapshot` (02 §3; 05 §5.2-§5.6).
        /// The core's historical zero sentinels remain in `inputs` so a missing
        /// read cannot create an adoption, while `backing_complete` preserves
        /// enough provenance for the runtime API to return `None` instead of
        /// presenting those sentinels as measurements (G-1).
        fn assemble_decision_input_snapshot(
            state: &EpochState<T::AccountId>,
            pid: ProposalId,
            params: CoreEpochParams,
        ) -> Result<DecisionInputSnapshot<T::AccountId>, DispatchError> {
            let proposal = state
                .proposal_view(pid)
                .map_err(Self::map_core_error)?
                .clone();
            let markets = proposal.markets.ok_or(Error::<T>::BadDecisionInput)?;
            let end = proposal.decide_at;

            // 04 §7 / 05 §5.2: exact registered full/trailing windows and
            // close-block spots. The crank keeps the established zero sentinel;
            // the view consults `backing_complete` before exposing any field.
            let accept_full = T::Market::twap_full_at(markets.accept, end);
            let reject_full = T::Market::twap_full_at(markets.reject, end);
            let baseline_full = T::Market::twap_full_at(markets.baseline, end);
            let accept_trailing =
                T::Market::twap_trailing_at(markets.accept, end, params.trailing_window);
            let reject_trailing =
                T::Market::twap_trailing_at(markets.reject, end, params.trailing_window);
            let baseline_trailing =
                T::Market::twap_trailing_at(markets.baseline, end, params.trailing_window);
            let accept_spot = T::Market::spot_at(markets.accept, end);
            let reject_spot = T::Market::spot_at(markets.reject, end);

            let welfare_grade_ok = [
                (markets.accept, BookRole::Decision),
                (markets.reject, BookRole::Decision),
            ]
            .iter()
            .all(|(market, role)| {
                T::Market::decision_grade(*market, end, *role, proposal.class, &params)
            });
            let baseline_grade_ok = T::Market::baseline_market(proposal.epoch)
                == Some(markets.baseline)
                && T::Market::decision_grade(
                    markets.baseline,
                    end,
                    BookRole::Baseline,
                    proposal.class,
                    &params,
                );

            // 05 §5.1: gate order is (S,C) × (adopt,reject), identical to
            // `MarketSet::gates` and the frozen 02 §4 view order.
            let (gate_twaps, gate_backing_complete) = match markets.gates {
                Some(gates) => {
                    let reads = gates.map(|market| T::Market::twap_full_at(market, end));
                    let complete = reads.iter().all(Option::is_some);
                    (
                        Some(reads.map(|value| value.unwrap_or(FixedU64(0)))),
                        complete,
                    )
                }
                None => (None, true),
            };
            let gate_grade_ok = markets.gates.is_none_or(|gates| {
                gates.iter().all(|market| {
                    T::Market::decision_grade(*market, end, BookRole::Gate, proposal.class, &params)
                })
            });

            // 05 §5.6 / 08 §5.2-§5.3: measured decision-pair depth and
            // the same constitution prize proxy feed both security sizing and
            // the Ask-scaled per-book contest floor in the runtime adapter.
            let measured_depth = T::Market::measured_depth(pid);
            let in_cap_prize = T::Constitution::in_cap_prize(&proposal);
            let backing_complete = [
                accept_full,
                reject_full,
                baseline_full,
                accept_trailing,
                reject_trailing,
                baseline_trailing,
                accept_spot,
                reject_spot,
            ]
            .iter()
            .all(Option::is_some)
                && gate_backing_complete
                && measured_depth.is_some()
                && in_cap_prize.is_some();

            Ok(DecisionInputSnapshot {
                proposal: proposal.clone(),
                params,
                inputs: DecisionInputs {
                    accept_full: accept_full.unwrap_or(FixedU64(0)),
                    reject_full: reject_full.unwrap_or(FixedU64(0)),
                    baseline_full: baseline_full.unwrap_or(FixedU64(0)),
                    accept_trailing: accept_trailing.unwrap_or(FixedU64(0)),
                    reject_trailing: reject_trailing.unwrap_or(FixedU64(0)),
                    baseline_trailing: baseline_trailing.unwrap_or(FixedU64(0)),
                    accept_spot: accept_spot.unwrap_or(FixedU64(0)),
                    reject_spot: reject_spot.unwrap_or(FixedU64(0)),
                    welfare_grade_ok,
                    baseline_grade_ok,
                    previous_settled_baseline_twap: T::Market::previous_settled_baseline_twap(
                        proposal.epoch,
                    ),
                    welfare_second_insufficient: T::Market::second_insufficiency(pid),
                    gate_grade_ok,
                    gate_twaps,
                    measured_depth: measured_depth.unwrap_or(0),
                    published_flow_per_day: T::Market::published_flow_per_day(pid),
                    in_cap_prize,
                    attestation_quorate: T::Constitution::attestation_artifact(&proposal)
                        .is_some_and(|artifact| T::Attestation::present_and_quorate(pid, artifact)),
                    constitution_queue_ok: T::Constitution::queue_time_check(&proposal),
                },
                backing_complete,
            })
        }

        fn requires_gate_markets_at_seed(proposal: &Proposal<T::AccountId>) -> bool {
            match proposal.class {
                ProposalClass::Code | ProposalClass::Meta => true,
                ProposalClass::Treasury => T::Constitution::treasury_gate_required(proposal),
                ProposalClass::Param | ProposalClass::Constitutional => false,
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
            Self::reconcile_proposal_bonds(&state)?;
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
            let old_epoch = EpochOf::<T>::get();
            let old_schedule = Schedule::<T>::get();
            if checked.epoch.index != old_epoch.index {
                EpochTimings::<T>::try_mutate(|history| -> DispatchResult {
                    // `epoch-core::sync_phase` catches up arithmetically across
                    // an arbitrary keeper outage. Reconstruct only the bounded
                    // tail retained by this storage (O(RECENT_COHORTS)), rather
                    // than archiving just the first skipped epoch and losing
                    // phase-anchored report/filing windows for intermediates.
                    let first = checked
                        .epoch
                        .index
                        .saturating_sub(RECENT_COHORTS as EpochId)
                        .max(old_epoch.index);
                    for index in first..checked.epoch.index {
                        let (start, length) = if index == old_epoch.index {
                            (old_schedule.epoch_start_block, old_schedule.length)
                        } else {
                            let after_first =
                                index.saturating_sub(old_epoch.index).saturating_sub(1);
                            (
                                old_schedule
                                    .epoch_start_block
                                    .saturating_add(old_schedule.length)
                                    .saturating_add(
                                        old_schedule.next_length.saturating_mul(after_first),
                                    ),
                                old_schedule.next_length,
                            )
                        };
                        history.retain(|timing| timing.index != index);
                        if history.len() == RECENT_COHORTS {
                            history.remove(0);
                        }
                        history
                            .try_push(EpochTiming {
                                index,
                                start,
                                length,
                            })
                            .map_err(|_| Error::<T>::TryStateViolation)?;
                    }
                    Ok(())
                })?;
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

        fn reconcile_proposal_bonds(state: &EpochState<T::AccountId>) -> DispatchResult {
            const EXECUTION_FAILURE_SLASH_DEN: Balance = 2;
            let slashes = state
                .events
                .iter()
                .filter_map(|event| match event {
                    CoreEvent::IntakeSlashed { pid, amount, .. } => Some((*pid, *amount)),
                    _ => None,
                })
                .collect::<Vec<_>>();
            let failed = state
                .events
                .iter()
                .filter_map(|event| match event {
                    CoreEvent::ExecutionFailed { pid, .. } => Some(*pid),
                    _ => None,
                })
                .collect::<Vec<_>>();
            let liabilities = ProposalBonds::<T>::iter().collect::<Vec<_>>();
            for (pid, mut bond) in liabilities {
                if failed.contains(&pid) {
                    let amount = bond
                        .held
                        .checked_div(EXECUTION_FAILURE_SLASH_DEN)
                        .and_then(|base| base.checked_add(bond.held % EXECUTION_FAILURE_SLASH_DEN))
                        .ok_or(Error::<T>::ArithmeticOverflow)?;
                    T::ProposalBond::slash_to_insurance(amount)?;
                    bond.held = bond
                        .held
                        .checked_sub(amount)
                        .ok_or(Error::<T>::ArithmeticOverflow)?;
                    ProposalBonds::<T>::insert(pid, bond);
                    continue;
                }
                let settle = state.proposal_view(pid).map_or(true, |proposal| {
                    matches!(
                        proposal.state,
                        ProposalState::Cancelled
                            | ProposalState::Rejected(_)
                            | ProposalState::Expired
                            | ProposalState::Measuring
                            | ProposalState::Settled
                    )
                });
                if !settle {
                    continue;
                }
                let slash = slashes
                    .iter()
                    .find_map(|(owner, amount)| (*owner == pid).then_some(*amount))
                    .unwrap_or_default()
                    .min(bond.held);
                if slash > 0 {
                    T::ProposalBond::slash_to_insurance(slash)?;
                }
                let refund = bond
                    .held
                    .checked_sub(slash)
                    .ok_or(Error::<T>::ArithmeticOverflow)?;
                if refund > 0 {
                    T::ProposalBond::release(&bond.proposer, refund)?;
                }
                ProposalBonds::<T>::remove(pid);
            }
            Ok(())
        }

        fn checked_state(
            state: &EpochState<T::AccountId>,
        ) -> Result<CheckedState<T>, DispatchError> {
            let mut intake = Vec::new();
            let mut live = Vec::new();
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
                ) {
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
                state.cohorts.len() <= MAX_NON_TERMINAL_COHORTS
                    && state.cohorts.iter().all(|cohort| !matches!(
                        cohort.status,
                        CohortStatus::Settled | CohortStatus::Void
                    )),
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
                    )
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
            let cohorts = Cohorts::<T>::iter_values().collect::<Vec<_>>();
            if live_proposals > MAX_LIVE_PROPOSALS
                || IntakeProposals::<T>::count() > MAX_INTAKE_QUEUE_BOUND
                || cohorts.len() > MAX_NON_TERMINAL_COHORTS
                || cohorts.iter().any(|cohort| {
                    matches!(cohort.status, CohortStatus::Settled | CohortStatus::Void)
                })
                || IntakeQueue::<T>::get().len() > MAX_INTAKE_QUEUE
                || RecentCohortSummaries::<T>::get().len() > RECENT_COHORTS
            {
                return Err(TryRuntimeError::Other("epoch FRAME bound exceeded (I-21)"));
            }
            let current_epoch = EpochOf::<T>::get().index;
            let mut previous_timing = None;
            for timing in EpochTimings::<T>::get() {
                if timing.index >= current_epoch
                    || timing.length < epoch_core::MIN_EPOCH_LENGTH
                    || timing.length % epoch_core::PHASE_DENOM != 0
                    || previous_timing.is_some_and(|previous| previous >= timing.index)
                {
                    return Err(TryRuntimeError::Other(
                        "epoch completed timing history is invalid",
                    ));
                }
                previous_timing = Some(timing.index);
            }
            if GuardianReviewDeadlines::<T>::count() > MAX_LIVE_PROPOSALS_BOUND {
                return Err(TryRuntimeError::Other(
                    "epoch guardian-review deadline bound exceeded",
                ));
            }
            if QualificationPreimageRequests::<T>::count() > MAX_LIVE_PROPOSALS_BOUND {
                return Err(TryRuntimeError::Other(
                    "epoch qualification preimage-request bound exceeded",
                ));
            }
            for (pid, hash) in QualificationPreimageRequests::<T>::iter() {
                if Proposals::<T>::get(pid).is_none_or(|proposal| {
                    proposal.payload_hash != hash
                        || !matches!(
                            proposal.state,
                            ProposalState::Qualified
                                | ProposalState::Trading
                                | ProposalState::Extended
                        )
                }) {
                    return Err(TryRuntimeError::Other(
                        "epoch qualification preimage request is orphaned",
                    ));
                }
            }
            let mut bond_total = 0_u128;
            let mut bond_count = 0_u32;
            for (pid, bond) in ProposalBonds::<T>::iter() {
                bond_count = bond_count.saturating_add(1);
                bond_total = bond_total
                    .checked_add(bond.held)
                    .ok_or(TryRuntimeError::Other(
                        "epoch proposal-bond liability overflow",
                    ))?;
                if bond.held == 0
                    || (!IntakeProposals::<T>::contains_key(pid)
                        && !Proposals::<T>::contains_key(pid))
                {
                    return Err(TryRuntimeError::Other(
                        "epoch proposal-bond liability is orphaned",
                    ));
                }
            }
            if bond_count != ProposalBonds::<T>::count()
                || bond_count > MAX_PROPOSAL_BONDS_BOUND
                || T::ProposalBond::escrow_balance() < bond_total
            {
                return Err(TryRuntimeError::Other(
                    "epoch proposal-bond custody is under-collateralized",
                ));
            }
            for (pid, _) in GuardianReviewDeadlines::<T>::iter() {
                if !Proposals::<T>::get(pid)
                    .is_some_and(|proposal| proposal.state == ProposalState::Suspended)
                {
                    return Err(TryRuntimeError::Other(
                        "epoch guardian-review deadline lacks suspended proposal",
                    ));
                }
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

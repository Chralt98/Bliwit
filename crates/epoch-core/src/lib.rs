#![cfg_attr(not(feature = "std"), no_std)]
#![deny(unsafe_code)]
#![allow(clippy::too_many_arguments)]

extern crate alloc;

use alloc::vec::Vec;
use futarchy_primitives::phase_offsets;
use futarchy_primitives::{
    bounds, Branch, CohortSummary, DecisionOutcome, EpochId, EpochPhase, EpochStatusView, FixedU64,
    MetricSpecVersion, ProposalClass, ProposalId, ProposalState, RejectReason, H256,
};
// Single-homed in `futarchy-primitives` (02 §2; 05 §1.2 frozen layout); re-exported
// so the historical `epoch_core::{MarketSet, Proposal}` path keeps resolving.
pub use futarchy_primitives::{Balance, BlockNumber, MarketSet, Proposal};
use parity_scale_codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use scale_info::TypeInfo;

macro_rules! ensure {
    ($cond:expr, $err:expr $(,)?) => {
        if !$cond {
            return Err($err);
        }
    };
}

pub const DEFAULT_EPOCH_LENGTH: BlockNumber = 302_400;
pub const MIN_EPOCH_LENGTH: BlockNumber = futarchy_primitives::kernel::MIN_EPOCH_LENGTH_BLOCKS;
pub const PHASE_DENOM: BlockNumber = futarchy_primitives::phase_offsets::DENOMINATOR;
pub const MAX_INTAKE_QUEUE: usize = futarchy_primitives::bounds::INTAKE_QUEUE as usize;
pub const MAX_LIVE_PROPOSALS: usize = futarchy_primitives::bounds::MAX_LIVE_PROPOSALS as usize;
pub const MAX_ACTIVE_PER_EPOCH: usize = futarchy_primitives::bounds::MAX_COHORT_PROPOSALS as usize;
pub const MAX_NON_TERMINAL_COHORTS: usize =
    futarchy_primitives::bounds::MAX_NON_TERMINAL_COHORTS as usize;
pub const RECENT_COHORTS: usize = futarchy_primitives::bounds::RECENT_COHORT_SUMMARIES as usize;
pub const MAX_RESOURCES_PER_PROPOSAL: usize =
    futarchy_primitives::bounds::MAX_RESOURCES_PER_PROPOSAL as usize;
pub const DECISION_EXTENSION: BlockNumber = futarchy_primitives::kernel::DEC_EXTENSION_BLOCKS;
pub const STALE_EPOCH_BOUND: BlockNumber = futarchy_primitives::kernel::STALE_EPOCH_BOUND_BLOCKS;
pub const ONE: u64 = 1_000_000_000;
pub const ONE_PP: u64 = futarchy_primitives::kernel::RERUN_HURDLE_BUMP_1E9;

/// Proposal classes whose decision path requires the frozen four-book
/// Survival/Security gate quartet (05 §5.1).
pub const fn requires_gate_markets(class: ProposalClass) -> bool {
    matches!(
        class,
        ProposalClass::Param | ProposalClass::Treasury | ProposalClass::Code | ProposalClass::Meta
    )
}

/// Live 13 §1 epoch/decision parameters captured at one dispatch boundary.
/// The FRAME shell rebuilds this value from `pallet-constitution::Params`; the
/// defaults keep the frame-free oracle behavior deterministic in standalone
/// tests. Kernel bounds remain single-homed in `futarchy-primitives`.
#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub struct EpochParams {
    pub epoch_length: BlockNumber,
    pub epoch_slots: u8,
    pub horizon_k: u8,
    pub decision_window: BlockNumber,
    pub trailing_window: BlockNumber,
    pub delta: [FixedU64; 5],
    pub sigma: [FixedU64; 5],
    pub delta_max: FixedU64,
    pub coverage_pct: u8,
    pub v_min: [Balance; 5],
    pub gate_v_min: [Balance; 5],
    pub gate_p_max: [FixedU64; 2],
    pub gate_eps: [FixedU64; 2],
    pub gate_nb_coverage_pct: u8,
    pub gate_nb_convergence: FixedU64,
    pub timelock: [BlockNumber; 5],
    pub grace: [BlockNumber; 5],
    pub intake_max_per_account: u8,
    pub intake_slash_pct: u8,
}

impl EpochParams {
    pub const DEFAULT: Self = Self {
        epoch_length: DEFAULT_EPOCH_LENGTH,
        epoch_slots: 5,
        horizon_k: 2,
        decision_window: 43_200,
        trailing_window: 14_400,
        delta: [
            FixedU64(37_500_000),
            FixedU64(37_500_000),
            FixedU64(60_000_000),
            FixedU64(90_000_000),
            FixedU64(ONE),
        ],
        sigma: [
            FixedU64(3_000_000),
            FixedU64(5_000_000),
            FixedU64(8_000_000),
            FixedU64(10_000_000),
            FixedU64(0),
        ],
        delta_max: FixedU64(50_000_000),
        coverage_pct: 95,
        v_min: [
            100_000_000_000,
            250_000_000_000,
            600_000_000_000,
            1_200_000_000_000,
            0,
        ],
        gate_v_min: [
            10_000_000_000,
            25_000_000_000,
            60_000_000_000,
            120_000_000_000,
            0,
        ],
        gate_p_max: [FixedU64(50_000_000), FixedU64(50_000_000)],
        gate_eps: [FixedU64(20_000_000), FixedU64(20_000_000)],
        gate_nb_coverage_pct: 98,
        gate_nb_convergence: FixedU64(10_000_000),
        timelock: [28_800, 43_200, 100_800, 201_600, 0],
        grace: [201_600, 201_600, 201_600, 201_600, 0],
        intake_max_per_account: 4,
        intake_slash_pct: 10,
    };

    pub fn validate(&self) -> Result<(), Error> {
        ensure!(
            self.epoch_length >= MIN_EPOCH_LENGTH
                && self.epoch_length % PHASE_DENOM == 0
                && self.decision_window
                    <= self
                        .epoch_length
                        .saturating_mul(phase_offsets::DECIDE_NUM - phase_offsets::TRADE_NUM)
                        / PHASE_DENOM
                && self.trailing_window <= self.decision_window
                && self.epoch_slots > 0
                && usize::from(self.epoch_slots) <= MAX_ACTIVE_PER_EPOCH
                && self.horizon_k > 0
                && usize::from(self.horizon_k) <= MAX_NON_TERMINAL_COHORTS
                && self.intake_max_per_account > 0
                && usize::from(self.intake_max_per_account) <= MAX_INTAKE_QUEUE
                && self.coverage_pct <= 100
                && self.intake_slash_pct <= 100
                && self.gate_nb_coverage_pct <= 100
                && self.delta_max.0 <= ONE
                && self.gate_nb_convergence.0 <= ONE
                && self
                    .gate_p_max
                    .iter()
                    .all(|v| { v.0 <= futarchy_primitives::kernel::GATE_P_MAX_CEILING_1E9 })
                && self.delta.iter().all(|v| v.0 <= ONE)
                && self.sigma.iter().all(|v| v.0 <= ONE),
            Error::BadParams
        );
        ensure!(
            self.v_min
                .iter()
                .zip(self.gate_v_min.iter())
                .take(4)
                .all(|(decision, gate)| {
                    futarchy_primitives::Balance::checked_div(*decision, 20)
                        .is_some_and(|lower| *gate >= lower)
                        && futarchy_primitives::Balance::checked_div(*decision, 2)
                            .is_some_and(|upper| *gate <= upper)
                }),
            Error::BadParams
        );
        Ok(())
    }

    pub fn class_delta(&self, class: ProposalClass) -> u64 {
        let [param, treasury, code, meta, constitutional] = self.delta;
        match class {
            ProposalClass::Param => param.0,
            ProposalClass::Treasury => treasury.0,
            ProposalClass::Code => code.0,
            ProposalClass::Meta => meta.0,
            ProposalClass::Constitutional => constitutional.0,
        }
    }

    pub fn class_sigma(&self, class: ProposalClass) -> u64 {
        let [param, treasury, code, meta, constitutional] = self.sigma;
        match class {
            ProposalClass::Param => param.0,
            ProposalClass::Treasury => treasury.0,
            ProposalClass::Code => code.0,
            ProposalClass::Meta => meta.0,
            ProposalClass::Constitutional => constitutional.0,
        }
    }

    pub fn class_timelock(&self, class: ProposalClass) -> BlockNumber {
        let [param, treasury, code, meta, constitutional] = self.timelock;
        match class {
            ProposalClass::Param => param,
            ProposalClass::Treasury => treasury,
            ProposalClass::Code => code,
            ProposalClass::Meta => meta,
            ProposalClass::Constitutional => constitutional,
        }
    }

    pub fn class_grace(&self, class: ProposalClass) -> BlockNumber {
        let [param, treasury, code, meta, constitutional] = self.grace;
        match class {
            ProposalClass::Param => param,
            ProposalClass::Treasury => treasury,
            ProposalClass::Code => code,
            ProposalClass::Meta => meta,
            ProposalClass::Constitutional => constitutional,
        }
    }
}

impl Default for EpochParams {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Minimal resolve/void/create seam. Production implementations call the
/// conditional-ledger pallet; standalone tests use an in-memory implementation.
pub trait LedgerOps<AccountId> {
    fn create_vault(&mut self, pid: ProposalId, spec: MetricSpecVersion) -> Result<(), Error>;
    fn resolve(&mut self, pid: ProposalId, branch: Branch) -> Result<(), Error>;
    fn void(&mut self, pid: ProposalId) -> Result<(), Error>;
}

/// Welfare is the sole settlement authority (05 §6). This seam deliberately
/// exposes no *scored* ledger settlement method, preventing epoch from
/// double-settling: epoch can request a settlement, never choose its score.
pub trait WelfareOps {
    fn compute_settlement(
        &mut self,
        cohort_epoch: EpochId,
        spec: MetricSpecVersion,
        target: SettlementTarget,
    ) -> Result<FixedU64, Error>;

    /// Settle the epoch's Baseline vault at the fixed neutral VOID score
    /// (`futarchy_primitives::kernel::VOID_BASELINE_SCORE`).
    ///
    /// 03 §2.3/§5 make this mandatory and give it no other owner: the epoch
    /// VOID path is the *only* producer of a Baseline settlement that is not a
    /// welfare computation, and 03 §6.4 justifies `BaselineState` having no
    /// `Voided` variant *precisely because* this settlement always happens.
    /// Without it the vault stays `Open` forever and single-sided Baseline
    /// holders — whose redemption calls require `Settled` — can never redeem.
    ///
    /// No welfare data is read: a VOID means the measurement is unusable, so
    /// this is a settlement-authority passthrough carrying a spec-fixed score,
    /// not a computation. Implementations MUST be a no-op (`Ok`) when the
    /// epoch has no Baseline vault or it is already settled — an epoch VOID
    /// must never fail on the settlement leg (G-1).
    fn settle_baseline_void(&mut self, cohort_epoch: EpochId) -> Result<(), Error>;
}

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub enum SettlementTarget {
    Proposal {
        pid: ProposalId,
        has_gate_books: bool,
    },
    Baseline,
}

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub struct DecisionGuards {
    pub preimage_ok: bool,
    pub resource_locks_held: bool,
    pub process_hold: bool,
}

/// Screening result supplied by the runtime's bounded call classifier.
///
/// T4 names only two full-slash findings: a verified constitution violation
/// or a verified false resource declaration.  Any check the implementation
/// cannot prove is therefore a refundable, status-quo cancellation (G-1), not
/// evidence of proposer fault.
#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub enum StaticCheckDisposition {
    Eligible,
    SlashAll(RejectReason),
    Refund(RejectReason),
}

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub struct TickInputs {
    pub static_check: StaticCheckDisposition,
    pub preimage_ok: bool,
    pub active_metric_spec_version: Option<MetricSpecVersion>,
    pub markets: Option<MarketSet>,
    pub review_window_closed: bool,
    pub queue_reject_reason: Option<RejectReason>,
    pub retry_exhausted: bool,
}

impl Default for TickInputs {
    fn default() -> Self {
        Self {
            static_check: StaticCheckDisposition::Eligible,
            preimage_ok: true,
            active_metric_spec_version: Some(1),
            markets: None,
            review_window_closed: false,
            queue_reject_reason: None,
            retry_exhausted: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub enum Origin {
    Signed,
    Keeper,
    Root,
    GuardianHold,
    /// Pallet-internal authority used only by the guardian review verdict.
    GuardianReview,
    ExecutionGuard,
    VoidAuthority,
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
pub enum CohortStatus {
    Measuring { until_epoch: EpochId },
    AwaitingOracle,
    Settling { cursor: u32 },
    Settled,
    Void,
}

#[derive(Clone, Debug, Decode, Encode, Eq, PartialEq, TypeInfo)]
pub struct CohortInfo {
    pub epoch: EpochId,
    pub proposals: Vec<ProposalId>,
    pub status: CohortStatus,
}

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub struct EpochInfo {
    pub index: EpochId,
    pub phase: EpochPhase,
    pub phase_start_block: BlockNumber,
    pub epoch_start_block: BlockNumber,
    pub length: BlockNumber,
    pub next_length: BlockNumber,
}

/// 05 §5.2/§5.4 step 5: the tri-state welfare-book decision grade. Only the
/// remediable-by-time shortfalls (contest capital below `dec.v_min(class)`,
/// coverage below `dec.coverage`, a first stale event) grade `Insufficient`
/// and may consume the single shared extension budget; every other failure
/// (sanity band, POL floor/undisturbed, a second stale event,
/// non-convergence, or an unreadable book) grades `Invalid` and rejects
/// immediately with `NotDecisionGrade` (status-quo default, G-1).
///
/// The derived `Ord` is severity order (`Ok < Insufficient < Invalid`), so
/// the worst of the Accept/Reject pair is `max`.
#[derive(
    Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, Ord, PartialEq, PartialOrd, TypeInfo,
)]
pub enum WelfareGrade {
    Ok,
    Insufficient,
    Invalid,
}

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub struct DecisionInputs {
    pub accept_full: FixedU64,
    pub reject_full: FixedU64,
    pub baseline_full: FixedU64,
    pub accept_trailing: FixedU64,
    pub reject_trailing: FixedU64,
    pub baseline_trailing: FixedU64,
    pub accept_spot: FixedU64,
    pub reject_spot: FixedU64,
    /// Worst grade of the Accept/Reject welfare pair (05 §5.4 step 5).
    pub welfare_grade: WelfareGrade,
    pub baseline_grade_ok: bool,
    pub previous_settled_baseline_twap: Option<FixedU64>,
    /// 05 §5.4 steps 3-4 are per gate: Survival's validity, then Survival's
    /// veto, then Security's validity, then Security's veto — a Survival
    /// veto must be reported before Security's validity is ever inspected.
    pub survival_grade_ok: bool,
    pub security_grade_ok: bool,
    pub gate_twaps: Option<[FixedU64; 4]>,
    pub measured_depth: Balance,
    pub published_flow_per_day: Option<Balance>,
    pub in_cap_prize: Option<Balance>,
    pub attestation_quorate: bool,
    pub constitution_queue_ok: bool,
}

#[derive(Clone, Debug, Decode, Encode, Eq, PartialEq, TypeInfo)]
pub enum Event {
    ProposalSubmitted(ProposalId),
    ProposalWithdrawn(ProposalId),
    ScreeningStarted(ProposalId),
    ProposalCancelled {
        pid: ProposalId,
        reason: RejectReason,
    },
    ProposalQualified(ProposalId),
    ProposalDeferred(ProposalId),
    SlotsShrunk {
        epoch: EpochId,
        requested: u32,
        funded: u32,
        dropped: Vec<ProposalId>,
    },
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
    ExecutionFailed {
        pid: ProposalId,
        reason: RejectReason,
    },
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
    NoOp,
}

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub enum Error {
    BadOrigin,
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
}

#[derive(Clone, Debug, Decode, Encode, Eq, PartialEq, TypeInfo)]
pub struct EpochState<AccountId> {
    pub epoch: EpochInfo,
    pub proposals: Vec<Proposal<AccountId>>,
    pub intake_queue: Vec<ProposalId>,
    pub cohorts: Vec<CohortInfo>,
    pub recent: Vec<CohortSummary>,
    pub resource_locks: Vec<([u8; 8], ProposalId)>,
    pub events: Vec<Event>,
    pub dead_man_armed: bool,
    /// The detector has observed every active cause healthy, so the latched
    /// flag now guards a proposal-free recovery epoch rather than a pause.
    pub dead_man_recovery_ready: bool,
    pub ledger_frozen: bool,
    pub phase_flags: u32,
    /// Monotone proposal-id high-water mark; reaping never decreases it.
    pub proposal_id_high_water: ProposalId,
    /// Internal T6 rollover counters, bounded by the intake queue.
    pub rollovers: Vec<(ProposalId, u8)>,
    /// First block at which the dead-man pause was observed.
    pub dead_man_paused_at: Option<BlockNumber>,
    /// The one full proposal-free recovery epoch, if active.
    pub recovery_epoch: Option<EpochId>,
    /// High-water snapshot for proposals affected by a stale-clock incident.
    pub stale_epoch_cutoff: Option<ProposalId>,
    /// `(last carried epoch, consecutive carried epochs)` for Baseline fallback.
    pub baseline_carry: Option<(EpochId, u8)>,
    /// Live `epoch.horizon_k`, refreshed from Params by the shell before any
    /// transition that may create a cohort. It is not persisted independently;
    /// the resulting `until_epoch` is creation-time frozen in `CohortInfo`.
    pub horizon_k: u8,
}

impl<AccountId: Clone + Eq> EpochState<AccountId> {
    pub const fn new() -> Self {
        Self {
            epoch: EpochInfo {
                index: 0,
                phase: EpochPhase::Intake,
                phase_start_block: 0,
                epoch_start_block: 0,
                length: DEFAULT_EPOCH_LENGTH,
                next_length: DEFAULT_EPOCH_LENGTH,
            },
            proposals: Vec::new(),
            intake_queue: Vec::new(),
            cohorts: Vec::new(),
            recent: Vec::new(),
            resource_locks: Vec::new(),
            events: Vec::new(),
            dead_man_armed: false,
            dead_man_recovery_ready: false,
            ledger_frozen: false,
            phase_flags: 0,
            proposal_id_high_water: 0,
            rollovers: Vec::new(),
            dead_man_paused_at: None,
            recovery_epoch: None,
            stale_epoch_cutoff: None,
            baseline_carry: None,
            horizon_k: EpochParams::DEFAULT.horizon_k,
        }
    }
    pub fn status_view(&self) -> EpochStatusView {
        EpochStatusView {
            index: self.epoch.index,
            phase: self.epoch.phase,
            phase_start_block: self.epoch.phase_start_block,
            next_boundary: self.next_boundary(),
            dead_man_armed: self.dead_man_armed,
            ledger_frozen: self.ledger_frozen,
            phase_flags: self.phase_flags,
        }
    }
    pub fn set_next_epoch_length(
        &mut self,
        origin: Origin,
        length: BlockNumber,
        params: &EpochParams,
    ) -> Result<(), Error> {
        ensure!(matches!(origin, Origin::Root), Error::BadOrigin);
        ensure!(
            length >= MIN_EPOCH_LENGTH
                && length % PHASE_DENOM == 0
                && params.decision_window
                    <= length.saturating_mul(phase_offsets::DECIDE_NUM - phase_offsets::TRADE_NUM)
                        / PHASE_DENOM,
            Error::BadEpochLength
        );
        self.epoch.next_length = length;
        Ok(())
    }
    pub fn sync_phase(&mut self, now: BlockNumber) {
        if self.epoch.length == 0 || self.epoch.next_length == 0 {
            return;
        }
        // Latch before phase catch-up can erase the persisted overdue boundary.
        // Every clock-sync caller therefore observes T20 stale status, even when
        // `decide` or `settle_cohort` is invoked before `tick`.
        self.latch_stale_epoch(now);
        if self.dead_man_armed && !self.dead_man_recovery_ready {
            if self.recovery_epoch.take().is_some() {
                // A fresh trigger during recovery invalidates the partial
                // healthy interval. Resume only into another full epoch.
                self.dead_man_paused_at = Some(now);
            } else {
                self.dead_man_paused_at.get_or_insert(now);
            }
            return;
        }
        if let Some(paused_at) = self.dead_man_paused_at.take() {
            let paused_for = now.saturating_sub(paused_at);
            for proposal in &mut self.proposals {
                if matches!(
                    proposal.state,
                    ProposalState::Trading | ProposalState::Extended
                ) {
                    proposal.decide_at = proposal.decide_at.saturating_add(paused_for);
                }
                if matches!(
                    proposal.state,
                    ProposalState::Queued
                        | ProposalState::Suspended
                        | ProposalState::FailedExecuted
                ) {
                    proposal.maturity = proposal.maturity.map(|at| at.saturating_add(paused_for));
                    proposal.grace_end = proposal.grace_end.map(|at| at.saturating_add(paused_for));
                }
            }
            // Recovery is a fresh, full, proposal-free logical epoch (05 §4.8).
            self.epoch.index = self.epoch.index.saturating_add(1);
            self.epoch.epoch_start_block = now;
            self.epoch.length = self.epoch.next_length;
            self.epoch.phase = EpochPhase::Intake;
            self.epoch.phase_start_block = now;
            self.recovery_epoch = Some(self.epoch.index);
            return;
        }
        let recovery_epoch = self.recovery_epoch;
        let mut crossed_epoch = false;
        if now
            >= self
                .epoch
                .epoch_start_block
                .saturating_add(self.epoch.length)
        {
            self.epoch.epoch_start_block = self
                .epoch
                .epoch_start_block
                .saturating_add(self.epoch.length);
            self.epoch.index = self.epoch.index.saturating_add(1);
            self.epoch.length = self.epoch.next_length;
            crossed_epoch = true;

            // After the first boundary the already-staged `next_length` applies
            // uniformly. Arithmetic catch-up keeps a long-idle permissionless
            // crank constant-time (I-20) instead of looping once per missed epoch.
            let additional = now.saturating_sub(self.epoch.epoch_start_block) / self.epoch.length;
            self.epoch.epoch_start_block = self
                .epoch
                .epoch_start_block
                .saturating_add(self.epoch.length.saturating_mul(additional));
            self.epoch.index = self.epoch.index.saturating_add(additional);
        }
        if crossed_epoch && recovery_epoch.is_some_and(|epoch| self.epoch.index > epoch) {
            self.recovery_epoch = None;
            // Preserve pre-pause intake without stranding it on an epoch that can
            // never qualify. A late post-recovery crank carries directly to the
            // current epoch rather than replaying missed logical epochs.
            let current_epoch = self.epoch.index;
            for proposal in &mut self.proposals {
                if proposal.state == ProposalState::Submitted
                    && self.intake_queue.contains(&proposal.id)
                {
                    proposal.epoch = current_epoch;
                }
            }
        }
        let phase = phase_at(
            now.saturating_sub(self.epoch.epoch_start_block),
            self.epoch.length,
        );
        if phase != self.epoch.phase || crossed_epoch {
            self.epoch.phase = phase;
            self.epoch.phase_start_block = self.phase_start(phase);
        }
    }
    pub fn submit(
        &mut self,
        origin: Origin,
        proposal: Proposal<AccountId>,
        params: &EpochParams,
    ) -> Result<(), Error> {
        ensure!(matches!(origin, Origin::Signed), Error::BadOrigin);
        params.validate()?;
        self.horizon_k = params.horizon_k;
        ensure!(
            !self.dead_man_armed
                && self.dead_man_paused_at.is_none()
                && self.recovery_epoch.is_none(),
            Error::BadPhase
        );
        ensure!(self.epoch.phase == EpochPhase::Intake, Error::BadPhase);
        ensure!(
            self.intake_queue.len() < MAX_INTAKE_QUEUE,
            Error::IntakeFull
        );
        // Bound the current-epoch admission ledger as well as the live queue.
        // Withdrawn/cancelled entries remain counted until the next epoch so
        // churn cannot create unbounded history or reset account limits.
        ensure!(
            self.proposals
                .iter()
                .filter(|existing| existing.epoch == self.epoch.index)
                .count()
                < MAX_INTAKE_QUEUE,
            Error::IntakeFull
        );
        ensure!(
            self.proposals
                .iter()
                .filter(|p| is_live_state(p.state))
                .count()
                < MAX_LIVE_PROPOSALS,
            Error::TooManyLiveProposals,
        );
        ensure!(
            self.proposals
                .iter()
                .filter(|p| p.epoch == self.epoch.index && p.proposer == proposal.proposer)
                .count()
                < usize::from(params.intake_max_per_account),
            Error::IntakeFull,
        );
        ensure!(
            proposal.resources.len() <= MAX_RESOURCES_PER_PROPOSAL,
            Error::TooManyResources
        );
        ensure!(
            proposal.id > self.proposal_id_high_water,
            Error::DuplicateProposal
        );
        ensure!(proposal.state == ProposalState::Submitted, Error::BadState);
        self.proposal_id_high_water = proposal.id;
        self.intake_queue.push(proposal.id);
        self.events.push(Event::ProposalSubmitted(proposal.id));
        self.proposals.push(proposal);
        Ok(())
    }
    pub fn withdraw(
        &mut self,
        origin: Origin,
        pid: ProposalId,
        who: &AccountId,
    ) -> Result<(), Error> {
        ensure!(matches!(origin, Origin::Signed), Error::BadOrigin);
        ensure!(self.epoch.phase == EpochPhase::Intake, Error::BadState);
        ensure!(
            self.proposal(pid)?.epoch == self.epoch.index,
            Error::BadState
        );
        let p = self.proposal_mut(pid)?;
        ensure!(
            p.state == ProposalState::Submitted && &p.proposer == who,
            Error::BadState
        );
        p.state = ProposalState::Cancelled;
        self.intake_queue.retain(|x| *x != pid);
        self.rollovers.retain(|(proposal, _)| *proposal != pid);
        self.events.push(Event::ProposalWithdrawn(pid));
        Ok(())
    }
    pub fn qualify(
        &mut self,
        origin: Origin,
        pid: ProposalId,
        static_check: StaticCheckDisposition,
        active_metric_spec_version: MetricSpecVersion,
        params: &EpochParams,
    ) -> Result<(), Error> {
        ensure!(matches!(origin, Origin::Keeper), Error::BadOrigin);
        params.validate()?;
        self.horizon_k = params.horizon_k;
        ensure!(self.recovery_epoch.is_none(), Error::BadPhase);
        ensure!(self.epoch.phase == EpochPhase::Qualify, Error::BadPhase);
        ensure!(
            self.proposal(pid)?.epoch == self.epoch.index,
            Error::BadState
        );
        let active = self
            .proposals
            .iter()
            .filter(|p| {
                p.epoch == self.epoch.index
                    && matches!(
                        p.state,
                        ProposalState::Qualified
                            | ProposalState::Trading
                            | ProposalState::Extended
                            | ProposalState::Queued
                            | ProposalState::Suspended
                            | ProposalState::Rerun
                    )
            })
            .count();
        let mut resources = self.proposal(pid)?.resources.clone().into_inner();
        resources.sort_unstable();
        resources.dedup();
        ensure!(
            self.proposal(pid)?.state == ProposalState::Submitted,
            Error::BadState
        );
        self.events.push(Event::ScreeningStarted(pid));
        match static_check {
            StaticCheckDisposition::Eligible => {}
            StaticCheckDisposition::SlashAll(reason) => {
                let bond = self.proposal(pid)?.bond;
                self.cancel(pid, reason, bond)?;
                return Ok(());
            }
            StaticCheckDisposition::Refund(reason) => {
                self.cancel(pid, reason, 0)?;
                return Ok(());
            }
        }
        let remaining = usize::from(params.epoch_slots).saturating_sub(active);
        // On-chain ranking prevents a lower-bond candidate from stealing a slot.
        // `tick` has already required this item to be the globally canonical
        // next candidate; this bounded rank check additionally protects direct
        // core callers and the slot-count transition itself.
        let mut candidates = self
            .proposals
            .iter()
            .filter(|proposal| {
                proposal.epoch == self.epoch.index && proposal.state == ProposalState::Submitted
            })
            .map(|proposal| (proposal.bond, proposal.id))
            .collect::<Vec<_>>();
        candidates.sort_by(|(bond_a, pid_a), (bond_b, pid_b)| {
            bond_b.cmp(bond_a).then_with(|| pid_a.cmp(pid_b))
        });
        if remaining == 0 || !candidates.iter().take(remaining).any(|(_, id)| *id == pid) {
            return self.rollover_or_refund(pid);
        }
        if !resources
            .iter()
            .all(|r| self.resource_locks.iter().all(|(l, _)| l != r))
        {
            return self.rollover_or_refund(pid);
        }
        let decide_at = self.epoch.epoch_start_block.saturating_add(
            self.epoch.length.saturating_mul(phase_offsets::DECIDE_NUM) / PHASE_DENOM,
        );
        for r in resources {
            self.resource_locks.push((r, pid));
        }
        let p = self.proposal_mut(pid)?;
        p.state = ProposalState::Qualified;
        p.metric_spec = active_metric_spec_version;
        p.decide_at = decide_at;
        self.intake_queue.retain(|x| *x != pid);
        self.rollovers.retain(|(proposal, _)| *proposal != pid);
        self.events.push(Event::ProposalQualified(pid));
        Ok(())
    }

    /// Apply the 08 §4.4 POL budget to the fully-qualified slate before any
    /// Seed-phase market is opened. The qualification crank is canonical
    /// descending-bond/ascending-id, but doing this once the whole slate exists
    /// is essential: screening failures must not consume budget and callers may
    /// not choose a cheaper subset by ordering Seed cranks.
    ///
    /// `commitments` carries the exact per-proposal live-Params prediction from
    /// the runtime boundary. An unavailable or overflowing prediction is treated
    /// as unfundable (G-1) and removed before budget ranking. The remaining
    /// removal is from the reverse bond-priority end, so the funded slate is a
    /// prefix of the fundable qualified ranking.
    pub fn shrink_qualified_slots(
        &mut self,
        origin: Origin,
        budget: Balance,
        commitments: &[(ProposalId, Option<Balance>)],
    ) -> Result<Vec<ProposalId>, Error> {
        ensure!(matches!(origin, Origin::Keeper), Error::BadOrigin);
        ensure!(self.epoch.phase == EpochPhase::Seed, Error::BadPhase);

        let mut funded = self
            .proposals
            .iter()
            .filter(|proposal| {
                proposal.epoch == self.epoch.index && proposal.state == ProposalState::Qualified
            })
            .map(|proposal| (proposal.bond, proposal.id))
            .collect::<Vec<_>>();
        funded.sort_by(|(bond_a, pid_a), (bond_b, pid_b)| {
            bond_b.cmp(bond_a).then_with(|| pid_a.cmp(pid_b))
        });
        let requested = u32::try_from(funded.len()).map_err(|_| Error::ArithmeticOverflow)?;
        let mut dropped = Vec::new();

        // An unavailable prediction cannot be funded at any rank. Remove these
        // entries first so a high-bond unavailable proposal cannot force the
        // needless removal of every cheaper, fully priced proposal below it.
        funded.retain(|(_, pid)| {
            let commitment = commitments
                .iter()
                .find_map(|(candidate, amount)| (*candidate == *pid).then_some(*amount))
                .flatten();
            if commitment.is_some() {
                true
            } else {
                dropped.push(*pid);
                false
            }
        });

        loop {
            let total = funded.iter().try_fold(0_u128, |sum, (_, pid)| {
                commitments
                    .iter()
                    .find_map(|(candidate, amount)| (*candidate == *pid).then_some(*amount))
                    .flatten()
                    .and_then(|amount| sum.checked_add(amount))
            });
            if total.is_some_and(|total| total <= budget) {
                break;
            }
            let Some((_, pid)) = funded.pop() else {
                break;
            };
            dropped.push(pid);
        }

        if dropped.is_empty() {
            return Ok(dropped);
        }

        for pid in &dropped {
            ensure!(
                self.proposal(*pid)?.state == ProposalState::Qualified,
                Error::BadState
            );
            self.resource_locks.retain(|(_, owner)| owner != pid);
            let proposal = self.proposal_mut(*pid)?;
            proposal.state = ProposalState::Submitted;
            proposal.decide_at = 0;
            self.rollover_or_refund(*pid)?;
        }
        let funded_count = u32::try_from(funded.len()).map_err(|_| Error::ArithmeticOverflow)?;
        self.events.push(Event::SlotsShrunk {
            epoch: self.epoch.index,
            requested,
            funded: funded_count,
            dropped: dropped.clone(),
        });
        Ok(dropped)
    }
    pub fn open_markets<L: LedgerOps<AccountId>>(
        &mut self,
        origin: Origin,
        ledger: &mut L,
        pid: ProposalId,
        markets: MarketSet,
    ) -> Result<(), Error> {
        ensure!(matches!(origin, Origin::Keeper), Error::BadOrigin);
        ensure!(self.recovery_epoch.is_none(), Error::BadPhase);
        ensure!(self.epoch.phase == EpochPhase::Seed, Error::BadPhase);
        let spec = self.proposal(pid)?.metric_spec;
        let p = self.proposal_mut(pid)?;
        ensure!(
            p.state == ProposalState::Qualified || p.state == ProposalState::Rerun,
            Error::BadState
        );
        ensure!(
            !requires_gate_markets(p.class) || markets.gates.is_some(),
            Error::BadDecisionInput
        );
        if p.state == ProposalState::Qualified {
            ledger.create_vault(pid, spec)?;
        }
        p.markets = Some(markets);
        p.state = ProposalState::Trading;
        self.events.push(Event::MarketsOpened(pid));
        Ok(())
    }
    /// Compatibility entry point for standalone callers. The production shell
    /// uses [`Self::decide_with`] so the step-1/2 guards and live Params are
    /// injected from trusted runtime seams.
    pub fn decide<L: LedgerOps<AccountId>>(
        &mut self,
        origin: Origin,
        ledger: &mut L,
        pid: ProposalId,
        now: BlockNumber,
        input: DecisionInputs,
    ) -> Result<DecisionOutcome, Error> {
        self.decide_with(
            origin,
            ledger,
            pid,
            now,
            input,
            DecisionGuards {
                preimage_ok: true,
                resource_locks_held: true,
                process_hold: false,
            },
            &EpochParams::DEFAULT,
        )
    }

    pub fn decide_with<L: LedgerOps<AccountId>>(
        &mut self,
        origin: Origin,
        ledger: &mut L,
        pid: ProposalId,
        now: BlockNumber,
        input: DecisionInputs,
        guards: DecisionGuards,
        params: &EpochParams,
    ) -> Result<DecisionOutcome, Error> {
        ensure!(matches!(origin, Origin::Keeper), Error::BadOrigin);
        params.validate()?;
        self.horizon_k = params.horizon_k;
        self.epoch.next_length = params.epoch_length;
        self.sync_phase(now);
        ensure!(self.recovery_epoch.is_none(), Error::BadPhase);
        // T20 force-reject (05 §2.1, 06 §6.3). This predicate is deliberately
        // *identical* to `tick`'s sweep condition below, and for the same
        // reason: a ledger freeze and a stale epoch both resolve a live
        // proposal to the status quo with a par void (D-1). Before SQ-98 the
        // freeze arm sat in the decision chain instead and produced
        // `Reject(ProcessHold)` -> `reject_to_measurement` ->
        // `ledger.resolve(pid, Branch::Reject)`, so the *same* freeze gave a
        // proposal a Reject-branch resolution or a par void depending purely
        // on whether `decide` or `tick` observed it first — a race deciding
        // whether Accept-branch holders recover ½ (VOID) or nothing
        // (Reject-branch). Aligning on the par void also avoids handing a
        // frozen system new measurement obligations it cannot discharge:
        // `market.set_frozen(true)` halts `crank_observe`, so a proposal sent
        // into `Measuring` by a freeze would await a score the freeze itself
        // prevents producing. Dead-man stays out of this predicate — 05 §4.8
        // pauses the clock and holds transitions, it never rejects (SQ-90).
        if self.ledger_frozen || self.stale_process_hold(pid) {
            self.force_reject_process_hold(Origin::Keeper, ledger, pid)?;
            return Ok(DecisionOutcome::Reject(RejectReason::ProcessHold));
        }
        if self.force_reject_if_cohort_void(ledger, pid)? {
            return Ok(DecisionOutcome::Reject(RejectReason::ProcessHold));
        }
        ensure!(now >= self.proposal(pid)?.decide_at, Error::BadPhase);
        let out = if !guards.preimage_ok {
            DecisionOutcome::Reject(RejectReason::ConstitutionViolation)
        } else if !guards.resource_locks_held {
            DecisionOutcome::Reject(RejectReason::ResourceConflict)
        } else if guards.process_hold || self.dead_man_armed {
            DecisionOutcome::Reject(RejectReason::ProcessHold)
        } else {
            self.decide_engine(pid, &input, params)?
        };
        match out {
            DecisionOutcome::Extend => {
                let p = self.proposal_mut(pid)?;
                ensure!(!p.extended, Error::BadState);
                p.extended = true;
                p.state = ProposalState::Extended;
                p.decide_at = p
                    .decide_at
                    .checked_add(DECISION_EXTENSION)
                    .ok_or(Error::ArithmeticOverflow)?;
                self.events.push(Event::DecisionExtended(pid));
            }
            DecisionOutcome::Adopt => {
                let maturity = now
                    .checked_add(params.class_timelock(self.proposal(pid)?.class))
                    .ok_or(Error::ArithmeticOverflow)?;
                let payload_hash;
                {
                    let p = self.proposal_mut(pid)?;
                    p.state = ProposalState::Queued;
                    p.maturity = Some(maturity);
                    p.grace_end = Some(
                        maturity
                            .checked_add(params.class_grace(p.class))
                            .ok_or(Error::ArithmeticOverflow)?,
                    );
                    p.decision = Some(out);
                    payload_hash = p.payload_hash;
                }
                self.events.push(Event::ProposalQueued {
                    pid,
                    payload_hash,
                    maturity,
                });
            }
            DecisionOutcome::Reject(r) => {
                self.reject_to_measurement(ledger, pid, r)?;
                if r == RejectReason::NotDecisionGrade || !guards.preimage_ok {
                    let amount = Self::fraction(self.proposal(pid)?.bond, params.intake_slash_pct)?;
                    self.events.push(Event::IntakeSlashed {
                        pid,
                        reason: r,
                        amount,
                    });
                }
            }
        }
        Ok(out)
    }
    pub fn delay_once(&mut self, origin: Origin, pid: ProposalId, h: H256) -> Result<(), Error> {
        ensure!(matches!(origin, Origin::GuardianHold), Error::BadOrigin);
        let p = self.proposal_mut(pid)?;
        ensure!(
            p.state == ProposalState::Queued && !p.delayed_once,
            Error::BadState
        );
        p.delayed_once = true;
        p.state = ProposalState::Suspended;
        self.events.push(Event::ProposalDelayed {
            pid,
            justification_hash: h,
        });
        Ok(())
    }
    /// Guardian force-rerun: immediately restart every proposal book for one
    /// fresh Extended window. Queue timing is voided; positions and the shared
    /// Baseline identity remain intact (06 §5.3).
    pub fn force_rerun(
        &mut self,
        origin: Origin,
        pid: ProposalId,
        now: BlockNumber,
    ) -> Result<(), Error> {
        ensure!(matches!(origin, Origin::GuardianHold), Error::BadOrigin);
        let p = self.proposal_mut(pid)?;
        ensure!(
            matches!(
                p.state,
                ProposalState::Trading | ProposalState::Extended | ProposalState::Queued
            ) && !p.rerun
                && !p.delayed_once,
            Error::BadState
        );
        p.state = ProposalState::Extended;
        p.rerun = true;
        p.extended = true;
        p.maturity = None;
        p.grace_end = None;
        p.decision = None;
        p.decide_at = now
            .checked_add(DECISION_EXTENSION)
            .ok_or(Error::ArithmeticOverflow)?;
        Ok(())
    }
    pub fn schedule_rerun(&mut self, origin: Origin, pid: ProposalId) -> Result<(), Error> {
        ensure!(matches!(origin, Origin::Keeper), Error::BadOrigin);
        let p = self.proposal_mut(pid)?;
        ensure!(p.state == ProposalState::Suspended, Error::BadState);
        p.state = ProposalState::Rerun;
        self.events.push(Event::RerunScheduled(pid));
        Ok(())
    }
    pub fn open_rerun(
        &mut self,
        origin: Origin,
        pid: ProposalId,
        now: BlockNumber,
    ) -> Result<(), Error> {
        ensure!(matches!(origin, Origin::Keeper), Error::BadOrigin);
        ensure!(self.recovery_epoch.is_none(), Error::BadPhase);
        ensure!(self.epoch.phase == EpochPhase::Seed, Error::BadPhase);
        let p = self.proposal_mut(pid)?;
        ensure!(p.state == ProposalState::Rerun, Error::BadState);
        p.state = ProposalState::Extended;
        p.rerun = true;
        p.extended = true;
        p.decide_at = now.saturating_add(DECISION_EXTENSION);
        self.events.push(Event::RerunOpened(pid));
        Ok(())
    }
    pub fn force_reject_process_hold<L: LedgerOps<AccountId>>(
        &mut self,
        origin: Origin,
        ledger: &mut L,
        pid: ProposalId,
    ) -> Result<(), Error> {
        ensure!(
            matches!(origin, Origin::Keeper | Origin::GuardianHold),
            Error::BadOrigin
        );
        if matches!(
            self.proposal(pid)?.state,
            ProposalState::Executed
                | ProposalState::Measuring
                | ProposalState::Settled
                | ProposalState::Cancelled
                | ProposalState::Expired
                | ProposalState::Rejected(_)
        ) {
            self.events.push(Event::NoOp);
            return Ok(());
        }
        if self.proposal(pid)?.markets.is_some() {
            ledger.void(pid)?;
        }
        self.proposal_mut(pid)?.state = ProposalState::Rejected(RejectReason::ProcessHold);
        self.intake_queue.retain(|queued| *queued != pid);
        self.rollovers.retain(|(proposal, _)| *proposal != pid);
        self.resource_locks.retain(|(_, owner)| *owner != pid);
        self.events.push(Event::ProposalForceRejected {
            pid,
            reason: RejectReason::ProcessHold,
        });
        self.maybe_clear_stale_epoch();
        Ok(())
    }

    /// PB-ORACLE-VOID cohort path (06 §6.2; 05 §7).
    pub fn void_affected_proposals(&self, epoch: EpochId) -> Result<Vec<ProposalId>, Error> {
        let mut affected = self
            .cohorts
            .iter()
            .find(|cohort| cohort.epoch == epoch)
            .map(|cohort| cohort.proposals.clone())
            .ok_or(Error::BadState)?;
        for proposal in &self.proposals {
            if proposal.epoch == epoch
                && matches!(
                    proposal.state,
                    ProposalState::Qualified
                        | ProposalState::Trading
                        | ProposalState::Extended
                        | ProposalState::Queued
                        | ProposalState::Suspended
                        | ProposalState::Rerun
                        | ProposalState::FailedExecuted
                )
                && !affected.contains(&proposal.id)
            {
                affected.push(proposal.id);
            }
        }
        ensure!(
            affected.len() <= MAX_ACTIVE_PER_EPOCH,
            Error::TooManyCohortProposals
        );
        Ok(affected)
    }

    pub fn void_cohort<L: LedgerOps<AccountId>, W: WelfareOps>(
        &mut self,
        origin: Origin,
        ledger: &mut L,
        welfare: &mut W,
        epoch: EpochId,
        now: BlockNumber,
    ) -> Result<(), Error> {
        ensure!(matches!(origin, Origin::VoidAuthority), Error::BadOrigin);
        let idx = self
            .cohorts
            .iter()
            .position(|cohort| cohort.epoch == epoch)
            .ok_or(Error::BadState)?;
        ensure!(
            matches!(self.cohorts[idx].status, CohortStatus::Measuring { .. }),
            Error::BadState
        );
        let affected = self.void_affected_proposals(epoch)?;
        for pid in &affected {
            if self.proposal(*pid)?.markets.is_some() {
                ledger.void(*pid)?;
            }
        }
        for pid in &affected {
            let proposal = self.proposal_mut(*pid)?;
            proposal.state = ProposalState::Rejected(RejectReason::ProcessHold);
            proposal.decision = Some(DecisionOutcome::Reject(RejectReason::ProcessHold));
            self.intake_queue.retain(|queued| *queued != *pid);
            self.rollovers.retain(|(proposal, _)| *proposal != *pid);
            self.resource_locks.retain(|(_, owner)| *owner != *pid);
            self.events.push(Event::ProposalForceRejected {
                pid: *pid,
                reason: RejectReason::ProcessHold,
            });
        }
        self.cohorts[idx].status = CohortStatus::Void;
        // 03 §2.3/§5: an epoch VOID settles the epoch's **Baseline** vault at
        // the neutral `s = 0.5` under the SettleAuthority. This is not welfare
        // scoring — no measurement is trusted here — but it is mandatory: the
        // Baseline vault has no `Voided` state (03 §6.4) exactly because this
        // settlement always happens, and `redeem_baseline`/`redeem_baseline_pair`
        // both require `Settled`. Skipping it strands every single-sided
        // Baseline holder of the voided epoch (SQ-92). It is deliberately
        // *outside* the per-proposal `ledger.void` loop above: T20 per-proposal
        // vault voiding is a different VOID and must not settle the Baseline.
        welfare.settle_baseline_void(epoch)?;
        // VOID completes the cohort without welfare *scoring*. Archive the
        // terminal outcome and reap its bounded working set in the same
        // transaction, just as successful settlement does below. The zero
        // score fields are non-semantic when `voided` is set.
        self.push_summary(epoch, FixedU64(0), FixedU64(0), now, true, &affected)?;
        self.events.push(Event::CohortVoided { epoch });
        self.cohorts.remove(idx);
        self.proposals.retain(|p| !affected.contains(&p.id));
        self.resource_locks
            .retain(|(_, pid)| !affected.contains(pid));
        Ok(())
    }
    /// T24 guardian review callback.
    pub fn veto_upheld<L: LedgerOps<AccountId>>(
        &mut self,
        origin: Origin,
        ledger: &mut L,
        pid: ProposalId,
    ) -> Result<(), Error> {
        ensure!(matches!(origin, Origin::GuardianReview), Error::BadOrigin);
        ensure!(
            self.proposal(pid)?.state == ProposalState::Suspended,
            Error::BadState
        );
        self.reject_to_measurement(ledger, pid, RejectReason::VetoUpheldByReview)
    }

    pub fn mark_executed<L: LedgerOps<AccountId>>(
        &mut self,
        origin: Origin,
        ledger: &mut L,
        pid: ProposalId,
    ) -> Result<(), Error> {
        ensure!(matches!(origin, Origin::ExecutionGuard), Error::BadOrigin);
        ensure!(
            matches!(
                self.proposal(pid)?.state,
                ProposalState::Queued | ProposalState::FailedExecuted
            ),
            Error::BadState
        );
        if self.force_reject_if_cohort_void(ledger, pid)? {
            return Ok(());
        }
        self.ensure_can_start_measurement(pid)?;
        ledger.resolve(pid, Branch::Accept)?;
        self.start_measurement(pid)
    }
    /// T18 (05 §2.1): `execute` dispatched but the payload atomically reverted.
    /// The proposal advances `Queued → FailedExecuted`; the ACCEPT branch stays
    /// live (no vault resolve here) so a retry (T23) can still succeed within the
    /// 72 h window, and `DecisionRecord` carries `PayloadReverted`. Bond-slash
    /// accounting (50%, proposer owns executability) is owned by 06/08.
    pub fn mark_failed_executed(&mut self, origin: Origin, pid: ProposalId) -> Result<(), Error> {
        ensure!(matches!(origin, Origin::ExecutionGuard), Error::BadOrigin);
        let p = self.proposal_mut(pid)?;
        ensure!(p.state == ProposalState::Queued, Error::BadState);
        p.state = ProposalState::FailedExecuted;
        self.events.push(Event::ExecutionFailed {
            pid,
            reason: RejectReason::PayloadReverted,
        });
        Ok(())
    }
    /// T22 (05 §2.1): the 72 h retry window opened at T18 is exhausted. The
    /// proposal is measured as **executed-with-failure** — vault `resolve(Accept)`
    /// (the adopted world, including the failure's consequences, is what W
    /// measures) then `FailedExecuted → Measuring`.
    pub fn retry_exhausted_to_measurement<L: LedgerOps<AccountId>>(
        &mut self,
        origin: Origin,
        ledger: &mut L,
        pid: ProposalId,
    ) -> Result<(), Error> {
        ensure!(matches!(origin, Origin::ExecutionGuard), Error::BadOrigin);
        ensure!(
            self.proposal(pid)?.state == ProposalState::FailedExecuted,
            Error::BadState
        );
        if self.force_reject_if_cohort_void(ledger, pid)? {
            return Ok(());
        }
        self.ensure_can_start_measurement(pid)?;
        ledger.resolve(pid, Branch::Accept)?;
        self.start_measurement(pid)
    }
    pub fn expire_or_stale_queue<L: LedgerOps<AccountId>>(
        &mut self,
        origin: Origin,
        ledger: &mut L,
        pid: ProposalId,
        reason: Option<RejectReason>,
    ) -> Result<(), Error> {
        ensure!(
            matches!(origin, Origin::Keeper | Origin::ExecutionGuard),
            Error::BadOrigin
        );
        let p = self.proposal_mut(pid)?;
        ensure!(p.state == ProposalState::Queued, Error::BadState);
        if self.force_reject_if_cohort_void(ledger, pid)? {
            return Ok(());
        }
        match reason {
            // 09 §1.2(3)/(4)/(5): dispatch-time rejections that carry a live vault
            // to the REJECT branch (T16/T21). `AttestationMissing` is the §1.2(5)
            // post-queue revocation case; without it the guard's rejection would
            // strand the proposal `Queued` in the epoch (BadDecisionInput).
            Some(
                r @ (RejectReason::StaleQueue
                | RejectReason::NotRatified
                | RejectReason::AttestationMissing),
            ) => self.reject_to_measurement(ledger, pid, r),
            None => {
                self.ensure_can_start_measurement(pid)?;
                ledger.resolve(pid, Branch::Reject)?;
                self.proposal_mut(pid)?.state = ProposalState::Expired;
                self.events.push(Event::MandateExpired(pid));
                self.start_measurement(pid)
            }
            _ => Err(Error::BadDecisionInput),
        }
    }

    /// One bounded keeper item. The FRAME call supplies at most `TickBatch`
    /// items and invokes this method once per item after synchronizing trusted
    /// seam inputs. Re-invocation in a state with no due transition is benign.
    pub fn tick<L: LedgerOps<AccountId>>(
        &mut self,
        origin: Origin,
        ledger: &mut L,
        pid: ProposalId,
        now: BlockNumber,
        input: TickInputs,
        params: &EpochParams,
    ) -> Result<(), Error> {
        ensure!(matches!(origin, Origin::Keeper), Error::BadOrigin);
        params.validate()?;
        self.horizon_k = params.horizon_k;
        self.epoch.next_length = params.epoch_length;
        self.sync_phase(now);

        let state = self.proposal(pid)?.state;
        let proposal_epoch = self.proposal(pid)?.epoch;
        if self.cohort_epoch_voided(proposal_epoch)
            && matches!(
                state,
                ProposalState::Submitted
                    | ProposalState::Screening
                    | ProposalState::Qualified
                    | ProposalState::Trading
                    | ProposalState::Extended
                    | ProposalState::Queued
                    | ProposalState::Suspended
                    | ProposalState::Rerun
                    | ProposalState::FailedExecuted
            )
        {
            return self.force_reject_process_hold(Origin::Keeper, ledger, pid);
        }
        // T20 force-reject (05 §2.1): a genuine stale-epoch (`StaleEpochBound`), or
        // a ledger freeze (06 §6.3 — a live proposal resolves to status quo; any
        // vault voids at par, D-1). Dead-man is deliberately *excluded*: 05 §4.8
        // freezes the queue and pauses the clock, it never rejects a live proposal.
        if (self.ledger_frozen || self.stale_process_hold(pid))
            && !matches!(
                state,
                ProposalState::Executed
                    | ProposalState::Measuring
                    | ProposalState::Settled
                    | ProposalState::Cancelled
                    | ProposalState::Expired
                    | ProposalState::Rejected(_)
            )
        {
            return self.force_reject_process_hold(Origin::Keeper, ledger, pid);
        }

        // Dead-man pause (05 §4.8) freezes the execution queue and pauses the
        // clock; the post-pause recovery epoch is proposal-free. In each case tick
        // is a no-op — pending transitions are held (status quo, G-1), never
        // decided or force-rejected. A dead-man decision still resolves to status
        // quo independently at `decide()` time (05 §5 step 2).
        //
        // Scope caveat (SQ-286): this hold is reached only when no *other* T20
        // trigger fired above. A ledger freeze or a stale epoch coinciding with
        // a dead-man pause therefore still force-rejects, because those branches
        // precede this one. Which emergency regime dominates is unstated in
        // 05 §4.8 / 06 §6.3 and is deliberately left as shipped here.
        if self.dead_man_armed || self.recovery_epoch.is_some() {
            self.events.push(Event::NoOp);
            return Ok(());
        }

        match state {
            ProposalState::Submitted
                if self.epoch.phase == EpochPhase::Qualify
                    && self.proposal(pid)?.epoch == self.epoch.index =>
            {
                // The bounded intake cannot be screened atomically inside one
                // TickBatch.  Enforce the reviewer's deterministic minimum:
                // every screening outcome is committed in canonical
                // descending-bond / ascending-id order, so caller order and
                // ineligible competitors cannot decide the available slots.
                let next = self
                    .proposals
                    .iter()
                    .filter(|proposal| {
                        proposal.epoch == self.epoch.index
                            && proposal.state == ProposalState::Submitted
                    })
                    .map(|proposal| (proposal.bond, proposal.id))
                    .max_by(|(bond_a, pid_a), (bond_b, pid_b)| {
                        bond_a.cmp(bond_b).then_with(|| pid_b.cmp(pid_a))
                    })
                    .map(|(_, candidate)| candidate);
                ensure!(next == Some(pid), Error::BadState);
                if !input.preimage_ok {
                    self.events.push(Event::ScreeningStarted(pid));
                    let slash = Self::fraction(self.proposal(pid)?.bond, params.intake_slash_pct)?;
                    self.cancel(pid, RejectReason::NotDecisionGrade, slash)
                } else if let Some(spec) = input.active_metric_spec_version {
                    self.qualify(Origin::Keeper, pid, input.static_check, spec, params)
                } else {
                    // Missing system MetricSpec is a fail-closed qualification
                    // failure, not proposer fraud.
                    self.events.push(Event::ScreeningStarted(pid));
                    self.cancel(pid, RejectReason::ProcessHold, 0)
                }
            }
            ProposalState::Qualified if self.epoch.phase == EpochPhase::Seed => {
                let markets = input.markets.ok_or(Error::BadDecisionInput)?;
                self.open_markets(Origin::Keeper, ledger, pid, markets)
            }
            ProposalState::Suspended if input.review_window_closed => {
                self.schedule_rerun(Origin::Keeper, pid)
            }
            ProposalState::Rerun if self.epoch.phase == EpochPhase::Seed => {
                self.open_rerun(Origin::Keeper, pid, now)
            }
            ProposalState::Queued => {
                if let Some(reason) = input.queue_reject_reason {
                    self.expire_or_stale_queue(Origin::Keeper, ledger, pid, Some(reason))
                } else if self.proposal(pid)?.grace_end.is_some_and(|end| now > end) {
                    self.expire_or_stale_queue(Origin::Keeper, ledger, pid, None)
                } else {
                    self.events.push(Event::NoOp);
                    Ok(())
                }
            }
            ProposalState::FailedExecuted if input.retry_exhausted => {
                self.retry_exhausted_to_measurement(Origin::ExecutionGuard, ledger, pid)
            }
            _ => {
                self.events.push(Event::NoOp);
                Ok(())
            }
        }
    }
    pub fn settle_cohort<W: WelfareOps>(
        &mut self,
        origin: Origin,
        welfare: &mut W,
        epoch: EpochId,
        batch: u32,
        baseline_twap: FixedU64,
        now: BlockNumber,
    ) -> Result<FixedU64, Error> {
        ensure!(matches!(origin, Origin::Keeper), Error::BadOrigin);
        ensure!(
            batch > 0 && batch <= futarchy_primitives::kernel::SETTLE_COHORT_MAX_ITEMS,
            Error::BatchTooLarge
        );
        ensure!(self.recovery_epoch.is_none(), Error::BadPhase);
        ensure!(
            self.epoch.phase == EpochPhase::Housekeeping,
            Error::BadPhase
        );
        let idx = self
            .cohorts
            .iter()
            .position(|c| c.epoch == epoch)
            .ok_or(Error::BadState)?;
        let cohort = self.cohorts.get(idx).ok_or(Error::BadState)?;
        let proposals = cohort.proposals.clone();
        ensure!(!proposals.is_empty(), Error::BadState);
        let first = proposals.first().copied().ok_or(Error::BadState)?;
        let spec = self.proposal(first)?.metric_spec;
        ensure!(
            proposals
                .iter()
                .all(|pid| self.proposal(*pid).is_ok_and(|p| p.metric_spec == spec)),
            Error::TryStateViolation
        );
        let cursor = match cohort.status {
            CohortStatus::Measuring { until_epoch } => {
                ensure!(
                    self.epoch.index >= until_epoch.saturating_add(1),
                    Error::BadState
                );
                0usize
            }
            CohortStatus::AwaitingOracle => 0usize,
            CohortStatus::Settling { cursor } => cursor as usize,
            CohortStatus::Settled | CohortStatus::Void => return Err(Error::BadState),
        };
        let total = proposals.len().saturating_add(1); // one Baseline target
        let end = cursor.saturating_add(batch as usize).min(total);
        let mut score = None;
        for item in cursor..end {
            let value = if item < proposals.len() {
                let pid = proposals.get(item).copied().ok_or(Error::BadState)?;
                let has_gate_books = self
                    .proposal(pid)?
                    .markets
                    .is_some_and(|markets| markets.gates.is_some());
                welfare.compute_settlement(
                    epoch,
                    spec,
                    SettlementTarget::Proposal {
                        pid,
                        has_gate_books,
                    },
                )?
            } else {
                welfare.compute_settlement(epoch, spec, SettlementTarget::Baseline)?
            };
            score = Some(value);
        }

        if end < total {
            self.cohorts.get_mut(idx).ok_or(Error::BadState)?.status =
                CohortStatus::Settling { cursor: end as u32 };
            return score.ok_or(Error::Welfare);
        }

        let final_score = score.ok_or(Error::Welfare)?;
        for pid in &proposals {
            let p = self.proposal_mut(*pid)?;
            p.state = ProposalState::Settled;
            p.decision.get_or_insert(DecisionOutcome::Adopt);
        }
        self.push_summary(epoch, final_score, baseline_twap, now, false, &proposals)?;
        self.events.push(Event::CohortSettled {
            epoch,
            s: final_score,
        });
        self.cohorts.remove(idx);
        self.proposals.retain(|p| !proposals.contains(&p.id));
        self.resource_locks
            .retain(|(_, pid)| !proposals.contains(pid));
        Ok(final_score)
    }
    pub fn try_state(&self) -> Result<(), Error> {
        let intake = self
            .proposals
            .iter()
            .filter(|p| matches!(p.state, ProposalState::Submitted | ProposalState::Screening))
            .count();
        let live = self
            .proposals
            .iter()
            .filter(|p| is_live_state(p.state))
            .count();
        ensure!(
            intake <= MAX_INTAKE_QUEUE
                && live <= MAX_LIVE_PROPOSALS
                && self.intake_queue.len() <= MAX_INTAKE_QUEUE
                && self.recent.len() <= RECENT_COHORTS
                && self.epoch.length >= MIN_EPOCH_LENGTH
                && self.epoch.next_length >= MIN_EPOCH_LENGTH
                && self.epoch.length % PHASE_DENOM == 0
                && self.epoch.next_length % PHASE_DENOM == 0
                && self.rollovers.len() <= MAX_INTAKE_QUEUE
                && self
                    .recovery_epoch
                    .is_none_or(|recovery| recovery == self.epoch.index),
            Error::TryStateViolation
        );
        ensure!(
            self.proposals.iter().enumerate().all(|(i, p)| self
                .proposals
                .iter()
                .take(i)
                .all(|seen| seen.id != p.id))
                && self.intake_queue.iter().enumerate().all(|(i, pid)| self
                    .intake_queue
                    .iter()
                    .take(i)
                    .all(|seen| seen != pid))
                && self.intake_queue.iter().all(|pid| self
                    .proposals
                    .iter()
                    .any(|p| p.id == *pid && p.state == ProposalState::Submitted)),
            Error::TryStateViolation
        );
        ensure!(
            self.cohorts.len() <= MAX_NON_TERMINAL_COHORTS
                && self
                    .cohorts
                    .iter()
                    .all(|c| !matches!(c.status, CohortStatus::Settled | CohortStatus::Void)),
            Error::TryStateViolation
        );
        for p in &self.proposals {
            ensure!(
                p.resources.len() <= MAX_RESOURCES_PER_PROPOSAL,
                Error::TryStateViolation
            );
            if matches!(
                p.state,
                ProposalState::Queued
                    | ProposalState::Executed
                    | ProposalState::FailedExecuted
                    | ProposalState::Measuring
                    | ProposalState::Settled
            ) {
                ensure!(p.markets.is_some(), Error::TryStateViolation);
            }
        }
        ensure!(
            self.rollovers
                .iter()
                .enumerate()
                .all(|(index, (pid, count))| {
                    *count == 1
                        && self
                            .rollovers
                            .iter()
                            .take(index)
                            .all(|(seen, _)| seen != pid)
                        && self.proposals.iter().any(|proposal| {
                            proposal.id == *pid && proposal.state == ProposalState::Submitted
                        })
                }),
            Error::TryStateViolation
        );
        for cohort in &self.cohorts {
            ensure!(
                cohort.proposals.len() <= MAX_ACTIVE_PER_EPOCH
                    && cohort.proposals.iter().all(|pid| self
                        .proposals
                        .iter()
                        .any(|p| p.id == *pid && p.epoch == cohort.epoch)),
                Error::TryStateViolation
            );
            if let CohortStatus::Settling { cursor } = cohort.status {
                ensure!(
                    cursor as usize <= cohort.proposals.len().saturating_add(1),
                    Error::TryStateViolation
                );
            }
            if let Some(first) = cohort
                .proposals
                .first()
                .and_then(|pid| self.proposals.iter().find(|p| p.id == *pid))
            {
                ensure!(
                    cohort.proposals.iter().all(|pid| self
                        .proposals
                        .iter()
                        .any(|p| p.id == *pid && p.metric_spec == first.metric_spec)),
                    Error::TryStateViolation
                );
            }
        }
        for (i, (r, pid)) in self.resource_locks.iter().enumerate() {
            let prior = self
                .resource_locks
                .get(..i)
                .ok_or(Error::TryStateViolation)?;
            ensure!(
                prior.iter().all(|(seen, _)| seen != r)
                    && self.proposals.iter().any(|p| p.id == *pid),
                Error::TryStateViolation
            );
        }
        Ok(())
    }

    pub fn decide_engine(
        &mut self,
        pid: ProposalId,
        i: &DecisionInputs,
        params: &EpochParams,
    ) -> Result<DecisionOutcome, Error> {
        let p = self.proposal(pid)?;
        params.validate()?;
        ensure!(
            matches!(p.state, ProposalState::Trading | ProposalState::Extended),
            Error::BadState
        );
        let class = p.class;
        let proposal_epoch = p.epoch;
        let rerun = p.rerun;
        let extended = p.extended;
        let has_gate_markets = p.markets.is_some_and(|markets| markets.gates.is_some());
        if has_gate_markets {
            let Some([s_adopt, s_reject, c_adopt, c_reject]) = i.gate_twaps else {
                return Ok(DecisionOutcome::Reject(RejectReason::NotDecisionGrade));
            };
            let [s_p_max, c_p_max] = params.gate_p_max;
            let [s_eps, c_eps] = params.gate_eps;
            // 05 §5.4 steps 3-4, per gate in order (Survival, then Security):
            // the gate's book validity first, then its veto — a Survival veto
            // is reported before Security's validity is ever inspected. Veto
            // iff adopt TWAP exceeds the absolute ruin cap p_max, or exceeds
            // the reject TWAP by more than the relative margin eps (05 §5.1).
            // Ordered before welfare — no upside overrides a veto (G-4, I-14).
            if !i.survival_grade_ok {
                return Ok(DecisionOutcome::Reject(RejectReason::NotDecisionGrade));
            }
            if s_adopt.0 > s_p_max.0 || s_adopt.0 > s_reject.0.saturating_add(s_eps.0) {
                return Ok(DecisionOutcome::Reject(RejectReason::GateVetoSurvival));
            }
            if !i.security_grade_ok {
                return Ok(DecisionOutcome::Reject(RejectReason::NotDecisionGrade));
            }
            if c_adopt.0 > c_p_max.0 || c_adopt.0 > c_reject.0.saturating_add(c_eps.0) {
                return Ok(DecisionOutcome::Reject(RejectReason::GateVetoSecurity));
            }
        } else if requires_gate_markets(class) {
            return Ok(DecisionOutcome::Reject(RejectReason::NotDecisionGrade));
        }
        // 05 §5.4 step 5: only Insufficient may spend the single shared
        // extension budget; Invalid rejects immediately (a rerun re-enters
        // Extended with `extended` already true, so it can never re-extend).
        match i.welfare_grade {
            WelfareGrade::Ok => {}
            WelfareGrade::Insufficient if !extended => return Ok(DecisionOutcome::Extend),
            WelfareGrade::Insufficient | WelfareGrade::Invalid => {
                return Ok(DecisionOutcome::Reject(RejectReason::NotDecisionGrade));
            }
        }
        if !i.baseline_grade_ok {
            let Some(_carried) = i.previous_settled_baseline_twap else {
                return Ok(DecisionOutcome::Reject(RejectReason::NotDecisionGrade));
            };
            let (consecutive, advances_tracker) = match self.baseline_carry {
                Some((epoch, count)) if proposal_epoch > epoch => (
                    if epoch.saturating_add(1) == proposal_epoch {
                        count.saturating_add(1)
                    } else {
                        1
                    },
                    true,
                ),
                Some((epoch, count)) if proposal_epoch == epoch => (count, false),
                // An older rerun neither rewinds the current streak nor inherits
                // a future epoch's count for its own decision.
                Some(_) => (1, false),
                None => (1, true),
            };
            if advances_tracker {
                self.baseline_carry = Some((proposal_epoch, consecutive));
            }
            self.events.push(Event::BaselineCarried {
                pid,
                epoch: proposal_epoch,
            });
            if has_gate_markets && consecutive >= 2 {
                return Ok(DecisionOutcome::Reject(RejectReason::NotDecisionGrade));
            }
        }
        let Some((baseline_full, baseline_trailing)) = effective_baseline_twaps(i) else {
            return Ok(DecisionOutcome::Reject(RejectReason::NotDecisionGrade));
        };
        let delta = params
            .class_delta(class)
            .saturating_add(if rerun { ONE_PP } else { 0 });
        let r_eff = effective_reject_1e9(i.reject_full, baseline_full, params.class_sigma(class));
        let full = i.accept_full.0 >= r_eff.saturating_add(delta);
        let tail_eff = effective_reject_1e9(
            i.reject_trailing,
            baseline_trailing,
            params.class_sigma(class),
        );
        let tail = i.accept_trailing.0 >= tail_eff.saturating_add(delta);
        let conv = decision_converged(i, params.delta_max);
        if !(full && tail && conv) {
            return Ok(match (full != tail, extended, conv) {
                (true, false, _) => DecisionOutcome::Extend,
                (true, true, _) => DecisionOutcome::Reject(RejectReason::SecondExtensionFailed),
                (false, _, true) => DecisionOutcome::Reject(RejectReason::HurdleNotMet),
                _ => DecisionOutcome::Reject(RejectReason::ConvergenceFailed),
            });
        }
        let attack = attack_cost_hat(
            i.measured_depth,
            i.published_flow_per_day,
            params.decision_window,
        )?;
        let prize = i.in_cap_prize.ok_or(Error::BadDecisionInput)?;
        if prize.saturating_mul(futarchy_primitives::kernel::SECURITY_FACTOR) > attack {
            return Ok(DecisionOutcome::Reject(RejectReason::SecuritySizing));
        }
        if matches!(class, ProposalClass::Code | ProposalClass::Meta) && !i.attestation_quorate {
            return Ok(DecisionOutcome::Reject(RejectReason::AttestationMissing));
        }
        if !i.constitution_queue_ok {
            return Ok(DecisionOutcome::Reject(RejectReason::RateLimited));
        }
        Ok(DecisionOutcome::Adopt)
    }
    fn rollover_or_refund(&mut self, pid: ProposalId) -> Result<(), Error> {
        let rolled = self
            .rollovers
            .iter()
            .find_map(|(proposal, count)| (*proposal == pid).then_some(*count))
            .unwrap_or(0);
        if rolled == 0 {
            ensure!(self.rollovers.len() < MAX_INTAKE_QUEUE, Error::IntakeFull);
            self.rollovers.push((pid, 1));
            let next_epoch = self.epoch.index.saturating_add(1);
            let proposal = self.proposal_mut(pid)?;
            proposal.state = ProposalState::Submitted;
            proposal.epoch = next_epoch;
            if !self.intake_queue.contains(&pid) {
                ensure!(
                    self.intake_queue.len() < MAX_INTAKE_QUEUE,
                    Error::IntakeFull
                );
                self.intake_queue.push(pid);
            }
            self.events.push(Event::ProposalDeferred(pid));
        } else {
            // 05 §2.1 T26: the second deferral is terminal — it cancels with a
            // full refund. Reporting it as `ProposalDeferred` made
            // event-derived history claim the proposal was still live
            // (SQ-166, contract v6).
            self.proposal_mut(pid)?.state = ProposalState::Cancelled;
            self.intake_queue.retain(|queued| *queued != pid);
            self.rollovers.retain(|(proposal, _)| *proposal != pid);
            self.events.push(Event::ProposalCancelled {
                pid,
                reason: RejectReason::RolloverExhausted,
            });
        }
        Ok(())
    }

    fn cancel(
        &mut self,
        pid: ProposalId,
        reason: RejectReason,
        slash: Balance,
    ) -> Result<(), Error> {
        self.proposal_mut(pid)?.state = ProposalState::Cancelled;
        self.intake_queue.retain(|x| *x != pid);
        self.rollovers.retain(|(proposal, _)| *proposal != pid);
        self.events.push(Event::ProposalCancelled { pid, reason });
        if slash > 0 {
            self.events.push(Event::IntakeSlashed {
                pid,
                reason,
                amount: slash,
            });
        }
        Ok(())
    }

    fn fraction(amount: Balance, percent: u8) -> Result<Balance, Error> {
        let percent = Balance::from(percent);
        let whole = amount
            .checked_div(100)
            .and_then(|value| value.checked_mul(percent))
            .ok_or(Error::ArithmeticOverflow)?;
        let remainder = amount % 100;
        let fractional = remainder
            .checked_mul(percent)
            .and_then(|value| value.checked_add(99))
            .and_then(|value| value.checked_div(100))
            .ok_or(Error::ArithmeticOverflow)?;
        whole
            .checked_add(fractional)
            .ok_or(Error::ArithmeticOverflow)
    }

    /// Latch an overdue persisted phase boundary before catch-up erases the
    /// evidence. The high-water snapshot makes later submissions immune while
    /// bounded tick batches drain every proposal affected by the incident.
    pub fn latch_stale_epoch(&mut self, now: BlockNumber) {
        if self.stale_epoch_cutoff.is_none()
            && !self.dead_man_armed
            && self.dead_man_paused_at.is_none()
            && self.recovery_epoch.is_none()
            && now > self.next_boundary().saturating_add(STALE_EPOCH_BOUND)
        {
            self.stale_epoch_cutoff = Some(self.proposal_id_high_water);
        }
        self.maybe_clear_stale_epoch();
    }

    pub fn stale_process_hold(&self, pid: ProposalId) -> bool {
        self.stale_epoch_cutoff.is_some_and(|cutoff| pid <= cutoff)
    }

    fn maybe_clear_stale_epoch(&mut self) {
        if let Some(cutoff) = self.stale_epoch_cutoff {
            let affected_remains = self
                .proposals
                .iter()
                .any(|proposal| proposal.id <= cutoff && is_force_rejectable_state(proposal.state));
            if !affected_remains {
                self.stale_epoch_cutoff = None;
            }
        }
    }
    fn reject_to_measurement<L: LedgerOps<AccountId>>(
        &mut self,
        ledger: &mut L,
        pid: ProposalId,
        r: RejectReason,
    ) -> Result<(), Error> {
        if self.force_reject_if_cohort_void(ledger, pid)? {
            return Ok(());
        }
        let has_markets = self.proposal(pid)?.markets.is_some();
        if has_markets {
            self.ensure_can_start_measurement(pid)?;
            ledger.resolve(pid, Branch::Reject)?;
        }
        self.rollovers.retain(|(proposal, _)| *proposal != pid);
        self.proposal_mut(pid)?.state = ProposalState::Rejected(r);
        self.proposal_mut(pid)?.decision = Some(DecisionOutcome::Reject(r));
        self.events.push(Event::ProposalRejected { pid, reason: r });
        if has_markets {
            self.start_measurement(pid)?;
        } else {
            self.resource_locks.retain(|(_, owner)| *owner != pid);
        }
        Ok(())
    }
    fn ensure_can_start_measurement(&self, pid: ProposalId) -> Result<(), Error> {
        let epoch = self.proposal(pid)?.epoch;
        if let Some(cohort) = self.cohorts.iter().find(|cohort| cohort.epoch == epoch) {
            ensure!(
                !matches!(cohort.status, CohortStatus::Settled | CohortStatus::Void),
                Error::BadState
            );
            ensure!(
                cohort.proposals.contains(&pid) || cohort.proposals.len() < MAX_ACTIVE_PER_EPOCH,
                Error::TooManyCohortProposals
            );
        } else {
            ensure!(
                self.cohorts
                    .iter()
                    .filter(|cohort| {
                        !matches!(cohort.status, CohortStatus::Settled | CohortStatus::Void)
                    })
                    .count()
                    < MAX_NON_TERMINAL_COHORTS,
                Error::TooManyCohorts
            );
        }
        Ok(())
    }
    fn start_measurement(&mut self, pid: ProposalId) -> Result<(), Error> {
        let epoch = self.proposal(pid)?.epoch;
        if let Some(c) = self.cohorts.iter_mut().find(|c| c.epoch == epoch) {
            ensure!(
                !matches!(c.status, CohortStatus::Settled | CohortStatus::Void),
                Error::BadState
            );
            if !c.proposals.contains(&pid) {
                ensure!(
                    c.proposals.len() < MAX_ACTIVE_PER_EPOCH,
                    Error::TooManyCohortProposals
                );
                c.proposals.push(pid);
            }
        } else {
            ensure!(
                self.cohorts
                    .iter()
                    .filter(|c| !matches!(c.status, CohortStatus::Settled | CohortStatus::Void))
                    .count()
                    < MAX_NON_TERMINAL_COHORTS,
                Error::TooManyCohorts
            );
            self.cohorts.push(CohortInfo {
                epoch,
                proposals: alloc::vec![pid],
                status: CohortStatus::Measuring {
                    until_epoch: epoch.saturating_add(self.horizon_k.into()),
                },
            });
        }
        self.rollovers.retain(|(proposal, _)| *proposal != pid);
        self.proposal_mut(pid)?.state = ProposalState::Measuring;
        self.events
            .push(Event::MeasurementStarted { cohort: epoch });
        Ok(())
    }

    /// A VOIDed epoch cannot accept a late proposal into its frozen cohort.
    /// Fail closed through T20 without changing the cohort's I-16 binding.
    fn force_reject_if_cohort_void<L: LedgerOps<AccountId>>(
        &mut self,
        ledger: &mut L,
        pid: ProposalId,
    ) -> Result<bool, Error> {
        let epoch = self.proposal(pid)?.epoch;
        if !self.cohort_epoch_voided(epoch) {
            return Ok(false);
        }
        self.force_reject_process_hold(Origin::Keeper, ledger, pid)?;
        self.proposal_mut(pid)?.decision = Some(DecisionOutcome::Reject(RejectReason::ProcessHold));
        Ok(true)
    }
    fn cohort_epoch_voided(&self, epoch: EpochId) -> bool {
        self.cohorts
            .iter()
            .any(|cohort| cohort.epoch == epoch && cohort.status == CohortStatus::Void)
            || self
                .recent
                .iter()
                .any(|summary| summary.epoch == epoch && summary.voided)
    }
    fn push_summary(
        &mut self,
        epoch: EpochId,
        s: FixedU64,
        baseline_twap: FixedU64,
        now: BlockNumber,
        voided: bool,
        summary_pids: &[ProposalId],
    ) -> Result<(), Error> {
        let mut proposals = futarchy_primitives::BoundedVec::<
            (ProposalId, ProposalClass, DecisionOutcome),
            { bounds::MAX_COHORT_PROPOSALS },
        >::new();
        for pid in summary_pids {
            let p = self.proposal(*pid)?;
            proposals
                .try_push((
                    p.id,
                    p.class,
                    p.decision
                        .unwrap_or(DecisionOutcome::Reject(RejectReason::ProcessHold)),
                ))
                .map_err(|_| Error::TooManyCohortProposals)?;
        }
        if self.recent.len() == RECENT_COHORTS {
            self.recent.remove(0);
        }
        self.recent.push(CohortSummary {
            epoch,
            s_1e9: s,
            baseline_twap_1e9: baseline_twap,
            proposals,
            voided,
            settled_at: now,
        });
        Ok(())
    }
    fn proposal(&self, pid: ProposalId) -> Result<&Proposal<AccountId>, Error> {
        self.proposals
            .iter()
            .find(|p| p.id == pid)
            .ok_or(Error::UnknownProposal)
    }
    fn proposal_mut(&mut self, pid: ProposalId) -> Result<&mut Proposal<AccountId>, Error> {
        self.proposals
            .iter_mut()
            .find(|p| p.id == pid)
            .ok_or(Error::UnknownProposal)
    }
    fn next_boundary(&self) -> BlockNumber {
        self.phase_start(self.epoch.phase)
            .saturating_add(phase_len(self.epoch.phase, self.epoch.length))
    }
    fn phase_start(&self, phase: EpochPhase) -> BlockNumber {
        self.epoch.epoch_start_block.saturating_add(match phase {
            EpochPhase::Intake => 0,
            EpochPhase::Qualify => {
                self.epoch.length.saturating_mul(phase_offsets::QUALIFY_NUM) / PHASE_DENOM
            }
            EpochPhase::Seed => {
                self.epoch.length.saturating_mul(phase_offsets::SEED_NUM) / PHASE_DENOM
            }
            EpochPhase::Trade => {
                self.epoch.length.saturating_mul(phase_offsets::TRADE_NUM) / PHASE_DENOM
            }
            EpochPhase::Decide => {
                self.epoch.length.saturating_mul(phase_offsets::DECIDE_NUM) / PHASE_DENOM
            }
            EpochPhase::Review | EpochPhase::Execute => {
                (self.epoch.length.saturating_mul(phase_offsets::DECIDE_NUM) / PHASE_DENOM)
                    .saturating_add(1)
            }
            EpochPhase::Housekeeping => {
                self.epoch
                    .length
                    .saturating_mul(phase_offsets::HOUSEKEEPING_NUM)
                    / PHASE_DENOM
            }
        })
    }
}
impl<AccountId: Clone + Eq> Default for EpochState<AccountId> {
    fn default() -> Self {
        Self::new()
    }
}

fn phase_at(offset: BlockNumber, l: BlockNumber) -> EpochPhase {
    if offset < l.saturating_mul(phase_offsets::QUALIFY_NUM) / PHASE_DENOM {
        EpochPhase::Intake
    } else if offset < l.saturating_mul(phase_offsets::SEED_NUM) / PHASE_DENOM {
        EpochPhase::Qualify
    } else if offset < l.saturating_mul(phase_offsets::TRADE_NUM) / PHASE_DENOM {
        EpochPhase::Seed
    } else if offset < l.saturating_mul(phase_offsets::DECIDE_NUM) / PHASE_DENOM {
        EpochPhase::Trade
    } else if offset < l.saturating_mul(phase_offsets::HOUSEKEEPING_NUM) / PHASE_DENOM {
        EpochPhase::Decide
    } else {
        EpochPhase::Housekeeping
    }
}
fn phase_len(p: EpochPhase, l: BlockNumber) -> BlockNumber {
    // Phase durations = the gap to the next phase-start offset (13 §3.1).
    match p {
        EpochPhase::Intake => {
            l.saturating_mul(phase_offsets::QUALIFY_NUM - phase_offsets::INTAKE_NUM) / PHASE_DENOM
        }
        EpochPhase::Qualify | EpochPhase::Seed => l / PHASE_DENOM,
        EpochPhase::Trade => {
            l.saturating_mul(phase_offsets::DECIDE_NUM - phase_offsets::TRADE_NUM) / PHASE_DENOM
        }
        EpochPhase::Decide => {
            l.saturating_mul(phase_offsets::HOUSEKEEPING_NUM - phase_offsets::DECIDE_NUM)
                / PHASE_DENOM
        }
        EpochPhase::Review | EpochPhase::Execute => 0,
        EpochPhase::Housekeeping => l / PHASE_DENOM,
    }
}
fn is_live_state(state: ProposalState) -> bool {
    !matches!(
        state,
        ProposalState::Submitted
            | ProposalState::Screening
            | ProposalState::Cancelled
            | ProposalState::Settled
            | ProposalState::Rejected(_)
            | ProposalState::Expired
    )
}
fn is_force_rejectable_state(state: ProposalState) -> bool {
    !matches!(
        state,
        ProposalState::Executed
            | ProposalState::Measuring
            | ProposalState::Settled
            | ProposalState::Cancelled
            | ProposalState::Expired
            | ProposalState::Rejected(_)
    )
}
/// Effective Baseline inputs used by the decision hurdle (05 §5.3). A
/// decision-grade live Baseline wins; otherwise the previous settled Baseline
/// is carried into both full and trailing comparisons.
pub fn effective_baseline_twaps(i: &DecisionInputs) -> Option<(FixedU64, FixedU64)> {
    if i.baseline_grade_ok {
        Some((i.baseline_full, i.baseline_trailing))
    } else {
        i.previous_settled_baseline_twap
            .map(|carried| (carried, carried))
    }
}

/// Exact 05 §5.4 effective reject hurdle. Kept public so read-only runtime
/// projections cannot drift from the decision engine's saturating arithmetic.
pub fn effective_reject_1e9(reject: FixedU64, baseline: FixedU64, sigma_1e9: u64) -> u64 {
    reject.0.max(baseline.0.saturating_sub(sigma_1e9))
}

/// Exact close-spot convergence predicate used by decide() (05 §5.2/§5.4).
pub fn decision_converged(i: &DecisionInputs, delta_max: FixedU64) -> bool {
    i.accept_spot.0.abs_diff(i.accept_full.0) <= delta_max.0
        && i.reject_spot.0.abs_diff(i.reject_full.0) <= delta_max.0
}

/// D-4 measured-depth attack-cost estimate, rounded down throughout (05 §5.6;
/// 08 §5.2). This is public so `FutarchyApi::decision_stats` and decide()
/// execute the same checked arithmetic.
pub fn attack_cost_hat(
    depth: Balance,
    flow: Option<Balance>,
    decision_window: BlockNumber,
) -> Result<Balance, Error> {
    let l_half = depth.checked_div(2).ok_or(Error::ArithmeticOverflow)?;
    let f = flow.map(|x| x.min(l_half)).unwrap_or(l_half);
    let t_dec = (decision_window / futarchy_primitives::kernel::BLOCKS_PER_DAY).max(1);
    f.checked_mul(t_dec.into()).ok_or(Error::ArithmeticOverflow)
}

// Backwards-compatible in-memory adapter for `execution-guard-core`. Production
// `pallet-epoch` never depends on the conditional-ledger pallet; B1a supplies a
// Config seam implementation instead.
impl<AccountId: Clone + Eq> LedgerOps<AccountId>
    for conditional_ledger_core::LedgerState<AccountId>
{
    fn create_vault(&mut self, pid: ProposalId, spec: MetricSpecVersion) -> Result<(), Error> {
        self.create_vault(pid, spec).map_err(|_| Error::Ledger)
    }

    fn resolve(&mut self, pid: ProposalId, branch: Branch) -> Result<(), Error> {
        self.resolve(
            conditional_ledger_core::LedgerOrigin::ResolveAuthority,
            pid,
            branch,
        )
        .map_err(|_| Error::Ledger)
    }

    fn void(&mut self, pid: ProposalId) -> Result<(), Error> {
        self.void(conditional_ledger_core::LedgerOrigin::ResolveAuthority, pid)
            .map_err(|_| Error::Ledger)
    }
}

#[cfg(feature = "runtime-benchmarks")]
pub mod benchmarking {
    use super::*;
    pub fn benchmark_tick() -> Result<(), Error> {
        let mut s = EpochState::<[u8; 32]>::new();
        s.sync_phase(43_200);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use conditional_ledger_core::{Event as LedgerEvent, LedgerState};
    use futarchy_primitives::BoundedVec;
    fn acct(x: u8) -> [u8; 32] {
        [x; 32]
    }
    fn prop(id: ProposalId, state: ProposalState) -> Proposal<[u8; 32]> {
        Proposal {
            id,
            proposer: acct(1),
            class: ProposalClass::Param,
            state,
            epoch: 0,
            submitted_at: 0,
            payload_hash: [id as u8; 32],
            payload_len: 0,
            ask: 0,
            bond: 10,
            resources: BoundedVec::try_from(alloc::vec![[id as u8; 8]]).unwrap(),
            metric_spec: 1,
            decide_at: 0,
            rerun: false,
            extended: false,
            delayed_once: false,
            markets: None,
            maturity: None,
            grace_end: None,
            version_constraint: None,
            decision: None,
        }
    }
    fn pass_input() -> DecisionInputs {
        DecisionInputs {
            accept_full: FixedU64(600_000_000),
            reject_full: FixedU64(500_000_000),
            baseline_full: FixedU64(500_000_000),
            accept_trailing: FixedU64(600_000_000),
            reject_trailing: FixedU64(500_000_000),
            baseline_trailing: FixedU64(500_000_000),
            accept_spot: FixedU64(600_000_000),
            reject_spot: FixedU64(500_000_000),
            welfare_grade: WelfareGrade::Ok,
            baseline_grade_ok: true,
            previous_settled_baseline_twap: None,
            survival_grade_ok: true,
            security_grade_ok: true,
            gate_twaps: Some([FixedU64(0), FixedU64(0), FixedU64(0), FixedU64(0)]),
            measured_depth: 1_000_000,
            published_flow_per_day: None,
            in_cap_prize: Some(100_000),
            attestation_quorate: true,
            constitution_queue_ok: true,
        }
    }
    #[test]
    fn phase_boundaries_scale() {
        let mut s = EpochState::<[u8; 32]>::new();
        s.sync_phase(43_199);
        assert_eq!(s.epoch.phase, EpochPhase::Intake);
        s.sync_phase(43_200);
        assert_eq!(s.epoch.phase, EpochPhase::Qualify);
        s.set_next_epoch_length(Origin::Root, 201_600, &EpochParams::DEFAULT)
            .unwrap();
        s.sync_phase(302_400);
        assert_eq!(s.epoch.length, 201_600);
    }
    #[test]
    fn submit_qualify_decide_adopt() {
        let mut s = EpochState::new();
        let mut ledger = LedgerState::<[u8; 32]>::new();
        s.submit(
            Origin::Signed,
            prop(1, ProposalState::Submitted),
            &EpochParams::DEFAULT,
        )
        .unwrap();
        s.sync_phase(43_200);
        s.qualify(
            Origin::Keeper,
            1,
            StaticCheckDisposition::Eligible,
            1,
            &EpochParams::DEFAULT,
        )
        .unwrap();
        s.epoch.phase = EpochPhase::Trade;
        s.proposal_mut(1).unwrap().state = ProposalState::Trading;
        s.proposal_mut(1).unwrap().markets = Some(MarketSet {
            accept: 1,
            reject: 2,
            gates: Some([3, 4, 5, 6]),
            baseline: 7,
        });
        s.proposal_mut(1).unwrap().decide_at = 10;
        assert_eq!(
            s.decide(Origin::Keeper, &mut ledger, 1, 10, pass_input())
                .unwrap(),
            DecisionOutcome::Adopt
        );
        assert_eq!(s.proposal(1).unwrap().state, ProposalState::Queued);
    }
    #[test]
    fn first_insufficient_extends_second_rejects() {
        let mut s = EpochState::new();
        let mut ledger = LedgerState::<[u8; 32]>::new();
        ledger.create_vault(1, 1).unwrap();
        let mut p = prop(1, ProposalState::Trading);
        p.markets = Some(MarketSet {
            accept: 1,
            reject: 2,
            gates: Some([3, 4, 5, 6]),
            baseline: 7,
        });
        s.proposals.push(p);
        let mut i = pass_input();
        i.welfare_grade = WelfareGrade::Insufficient;
        assert_eq!(
            s.decide(Origin::Keeper, &mut ledger, 1, 0, i).unwrap(),
            DecisionOutcome::Extend
        );
        assert_eq!(
            s.decide(Origin::Keeper, &mut ledger, 1, DECISION_EXTENSION, i)
                .unwrap(),
            DecisionOutcome::Reject(RejectReason::NotDecisionGrade)
        );
        // T21 fired: the deployed vault is resolved to the REJECT branch, not left Open.
        assert!(ledger
            .events
            .iter()
            .any(|e| matches!(e, LedgerEvent::VaultResolved(1, Branch::Reject))));
    }
    #[test]
    fn security_sizing_rejects() {
        let mut s = EpochState::new();
        let mut ledger = LedgerState::<[u8; 32]>::new();
        ledger.create_vault(1, 1).unwrap();
        let mut p = prop(1, ProposalState::Trading);
        p.markets = Some(MarketSet {
            accept: 1,
            reject: 2,
            gates: Some([3, 4, 5, 6]),
            baseline: 7,
        });
        s.proposals.push(p);
        let mut i = pass_input();
        i.in_cap_prize = Some(1_000_000);
        assert_eq!(
            s.decide(Origin::Keeper, &mut ledger, 1, 0, i).unwrap(),
            DecisionOutcome::Reject(RejectReason::SecuritySizing)
        );
    }
    #[test]
    fn security_sizing_uses_decision_window_days_at_floor() {
        let mut s = EpochState::new();
        let mut p = prop(1, ProposalState::Trading);
        p.markets = Some(MarketSet {
            accept: 1,
            reject: 2,
            gates: Some([3, 4, 5, 6]),
            baseline: 7,
        });
        s.proposals.push(p);
        let mut input = pass_input();
        input.measured_depth = 600;
        input.in_cap_prize = Some(101);
        assert_eq!(
            s.decide_engine(1, &input, &EpochParams::DEFAULT),
            Ok(DecisionOutcome::Adopt)
        );
        let mut floor = EpochParams::DEFAULT;
        floor.decision_window = futarchy_primitives::kernel::BLOCKS_PER_DAY;
        assert_eq!(
            s.decide_engine(1, &input, &floor),
            Ok(DecisionOutcome::Reject(RejectReason::SecuritySizing))
        );
    }
    #[test]
    fn shrink_drops_unavailable_commitment_before_lower_bond_fundable_slots() {
        let mut state = EpochState::<[u8; 32]>::new();
        state.epoch.phase = EpochPhase::Seed;
        for (pid, bond) in [(1, 300), (2, 200), (3, 100)] {
            let mut proposal = prop(pid, ProposalState::Qualified);
            proposal.bond = bond;
            state.proposals.push(proposal);
        }

        let dropped = state
            .shrink_qualified_slots(
                Origin::Keeper,
                100,
                &[(1, None), (2, Some(60)), (3, Some(40))],
            )
            .unwrap();

        assert_eq!(dropped, vec![1]);
        assert_eq!(state.proposal(1).unwrap().state, ProposalState::Submitted);
        assert_eq!(state.proposal(1).unwrap().epoch, 1);
        assert_eq!(state.proposal(2).unwrap().state, ProposalState::Qualified);
        assert_eq!(state.proposal(3).unwrap().state, ProposalState::Qualified);
        assert_eq!(
            state.events.last(),
            Some(&Event::SlotsShrunk {
                epoch: 0,
                requested: 3,
                funded: 2,
                dropped: vec![1],
            })
        );
    }
    #[test]
    fn submit_rejects_id_at_or_below_high_water_after_reap() {
        let mut s = EpochState::<[u8; 32]>::new();
        s.proposal_id_high_water = 5;
        assert_eq!(
            s.submit(
                Origin::Signed,
                prop(5, ProposalState::Submitted),
                &EpochParams::DEFAULT,
            ),
            Err(Error::DuplicateProposal)
        );
        s.submit(
            Origin::Signed,
            prop(6, ProposalState::Submitted),
            &EpochParams::DEFAULT,
        )
        .unwrap();
        assert_eq!(s.proposal_id_high_water, 6);
    }
    /// Build a Trading proposal at its decision block over a live vault.
    fn frozen_freeze_fixture() -> (EpochState<[u8; 32]>, LedgerState<[u8; 32]>) {
        let mut s = EpochState::<[u8; 32]>::new();
        let mut ledger = LedgerState::<[u8; 32]>::new();
        ledger.create_vault(1, 1).unwrap();
        let mut p = prop(1, ProposalState::Trading);
        p.markets = Some(MarketSet {
            accept: 1,
            reject: 2,
            gates: None,
            baseline: 7,
        });
        p.decide_at = 0;
        s.proposals.push(p);
        s.ledger_frozen = true;
        (s, ledger)
    }

    #[test]
    fn ledger_freeze_disposition_is_the_same_whether_tick_or_decide_observes_first() {
        // SQ-98 / 06 §6.3: a ledger freeze force-rejects a live proposal to the
        // status quo with a par void (D-1). Before the fix `tick` par-voided the
        // vault while `decide` resolved it onto the Reject branch, so which
        // crank ran first decided whether Accept-branch holders recovered ½ or
        // nothing. Both paths must now land on the identical ledger state.
        let (mut tick_state, mut tick_ledger) = frozen_freeze_fixture();
        tick_state
            .tick(
                Origin::Keeper,
                &mut tick_ledger,
                1,
                0,
                TickInputs::default(),
                &EpochParams::DEFAULT,
            )
            .unwrap();

        let (mut decide_state, mut decide_ledger) = frozen_freeze_fixture();
        assert_eq!(
            decide_state
                .decide(Origin::Keeper, &mut decide_ledger, 1, 0, pass_input())
                .unwrap(),
            DecisionOutcome::Reject(RejectReason::ProcessHold)
        );

        // Same proposal disposition...
        assert_eq!(
            tick_state.proposal(1).unwrap().state,
            ProposalState::Rejected(RejectReason::ProcessHold)
        );
        assert_eq!(
            decide_state.proposal(1).unwrap().state,
            tick_state.proposal(1).unwrap().state
        );
        // ...and, the part that actually moved money, the same vault state:
        // `Voided` (par recovery), never `Resolved(Reject)`.
        assert_eq!(tick_ledger.vaults, decide_ledger.vaults);
        assert_eq!(
            decide_ledger
                .vaults
                .iter()
                .map(|v| v.info.state)
                .collect::<Vec<_>>(),
            vec![futarchy_primitives::VaultState::Voided]
        );
        // A freeze must not push the proposal into measurement: markets are
        // frozen too, so the score it would wait for cannot be produced.
        assert_ne!(
            decide_state.proposal(1).unwrap().state,
            ProposalState::Measuring
        );
    }

    #[test]
    fn t20_emits_only_the_force_rejected_event() {
        let mut s = EpochState::<[u8; 32]>::new();
        let mut ledger = LedgerState::<[u8; 32]>::new();
        ledger.create_vault(1, 1).unwrap();
        let mut p = prop(1, ProposalState::Trading);
        p.markets = Some(MarketSet {
            accept: 1,
            reject: 2,
            gates: Some([3, 4, 5, 6]),
            baseline: 7,
        });
        s.proposals.push(p);
        s.force_reject_process_hold(Origin::Keeper, &mut ledger, 1)
            .unwrap();
        assert_eq!(
            s.events,
            alloc::vec![Event::ProposalForceRejected {
                pid: 1,
                reason: RejectReason::ProcessHold,
            }]
        );
    }
    #[test]
    fn recent_ring_is_bounded() {
        let mut s = EpochState::<[u8; 32]>::new();
        for e in 0..40 {
            s.push_summary(e, FixedU64(1), FixedU64(2), e, false, &[])
                .unwrap();
        }
        assert_eq!(s.recent.len(), RECENT_COHORTS);
        assert_eq!(s.recent[0].epoch, 8);
    }
    #[test]
    fn try_state_catches_duplicate_locks() {
        let mut s = EpochState::new();
        s.proposals.push(prop(1, ProposalState::Submitted));
        s.resource_locks.push(([1; 8], 1));
        s.resource_locks.push(([1; 8], 1));
        assert_eq!(s.try_state(), Err(Error::TryStateViolation));
    }
    #[test]
    fn class_hurdle_params_match_spec() {
        // 13 §1 (V-12 Phase-0 calibrated): dec.delta = 0.0375/0.0375/0.060/0.090
        // and dec.sigma = 0.003/0.005/0.008/0.010 for PARAM/TREASURY/CODE/META.
        // CODE and META must differ (they were conflated at 0.040/0 previously).
        let params = EpochParams::DEFAULT;
        assert_eq!(params.class_delta(ProposalClass::Param), 37_500_000);
        assert_eq!(params.class_delta(ProposalClass::Treasury), 37_500_000);
        assert_eq!(params.class_delta(ProposalClass::Code), 60_000_000);
        assert_eq!(params.class_delta(ProposalClass::Meta), 90_000_000);
        assert_eq!(params.class_sigma(ProposalClass::Param), 3_000_000);
        assert_eq!(params.class_sigma(ProposalClass::Treasury), 5_000_000);
        assert_eq!(params.class_sigma(ProposalClass::Code), 8_000_000);
        assert_eq!(params.class_sigma(ProposalClass::Meta), 10_000_000);
    }
    #[test]
    fn epoch_params_default_pins_every_13_section_1_value() {
        // Pin the A8 genesis/known-good fallback to every 13 §1 epoch/decision
        // default; production behavior still reads the live Params provider.
        assert_eq!(
            EpochParams::DEFAULT,
            EpochParams {
                epoch_length: 302_400,
                epoch_slots: 5,
                horizon_k: 2,
                decision_window: 43_200,
                trailing_window: 14_400,
                delta: [
                    FixedU64(37_500_000),
                    FixedU64(37_500_000),
                    FixedU64(60_000_000),
                    FixedU64(90_000_000),
                    FixedU64(ONE),
                ],
                sigma: [
                    FixedU64(3_000_000),
                    FixedU64(5_000_000),
                    FixedU64(8_000_000),
                    FixedU64(10_000_000),
                    FixedU64(0),
                ],
                delta_max: FixedU64(50_000_000),
                coverage_pct: 95,
                v_min: [
                    100_000_000_000,
                    250_000_000_000,
                    600_000_000_000,
                    1_200_000_000_000,
                    0,
                ],
                gate_v_min: [
                    10_000_000_000,
                    25_000_000_000,
                    60_000_000_000,
                    120_000_000_000,
                    0,
                ],
                gate_p_max: [FixedU64(50_000_000), FixedU64(50_000_000)],
                gate_eps: [FixedU64(20_000_000), FixedU64(20_000_000)],
                gate_nb_coverage_pct: 98,
                gate_nb_convergence: FixedU64(10_000_000),
                timelock: [28_800, 43_200, 100_800, 201_600, 0],
                grace: [201_600, 201_600, 201_600, 201_600, 0],
                intake_max_per_account: 4,
                intake_slash_pct: 10,
            }
        );
    }
    #[test]
    fn param_hurdle_adopts_at_spec_delta() {
        // A PARAM margin of 0.040 clears the V-12-calibrated spec hurdle δ=0.0375
        // (adopt), read from Params rather than any hardcoded value. Pins finding
        // #4 into the engine.
        let mut s = EpochState::new();
        let mut ledger = LedgerState::<[u8; 32]>::new();
        ledger.create_vault(1, 1).unwrap();
        let mut p = prop(1, ProposalState::Trading);
        p.markets = Some(MarketSet {
            accept: 1,
            reject: 2,
            gates: Some([3, 4, 5, 6]),
            baseline: 7,
        });
        s.proposals.push(p);
        let mut i = pass_input();
        i.accept_full = FixedU64(540_000_000);
        i.accept_trailing = FixedU64(540_000_000);
        i.accept_spot = FixedU64(540_000_000);
        assert_eq!(
            s.decide(Origin::Keeper, &mut ledger, 1, 0, i).unwrap(),
            DecisionOutcome::Adopt
        );
    }
    #[test]
    fn gate_veto_uses_configured_cap() {
        // 13 §1 / 05 §5.1: an adopt-gate Survival TWAP of 0.10 sits above the
        // p_max=0.05 ruin cap and must veto, though it fell under the old
        // hardcoded 0.20. reject TWAP equals adopt, so only the absolute cap fires.
        let mut s = EpochState::<[u8; 32]>::new();
        let mut p = prop(1, ProposalState::Trading);
        p.class = ProposalClass::Code; // requires gate markets
        p.markets = Some(MarketSet {
            accept: 1,
            reject: 2,
            gates: Some([1, 2, 3, 4]),
            baseline: 5,
        });
        s.proposals.push(p);
        let mut i = pass_input();
        i.gate_twaps = Some([
            FixedU64(100_000_000), // adopt Survival = 0.10 > p_max 0.05
            FixedU64(100_000_000), // reject Survival = 0.10 (relative test inert)
            FixedU64(0),           // adopt Security
            FixedU64(0),           // reject Security
        ]);
        assert_eq!(
            s.decide_engine(1, &i, &EpochParams::DEFAULT),
            Ok(DecisionOutcome::Reject(RejectReason::GateVetoSurvival))
        );
    }

    #[test]
    fn low_ask_treasury_requires_gate_books_and_both_vetoes_are_reachable() {
        for (gate_twaps, expected) in [
            (
                [
                    FixedU64(100_000_000),
                    FixedU64(100_000_000),
                    FixedU64(0),
                    FixedU64(0),
                ],
                RejectReason::GateVetoSurvival,
            ),
            (
                [
                    FixedU64(0),
                    FixedU64(0),
                    FixedU64(100_000_000),
                    FixedU64(100_000_000),
                ],
                RejectReason::GateVetoSecurity,
            ),
        ] {
            let mut state = EpochState::<[u8; 32]>::new();
            let mut proposal = prop(1, ProposalState::Trading);
            proposal.class = ProposalClass::Treasury;
            proposal.ask = 1;
            proposal.markets = Some(MarketSet {
                accept: 1,
                reject: 2,
                gates: Some([3, 4, 5, 6]),
                baseline: 7,
            });
            state.proposals.push(proposal);
            let mut input = pass_input();
            input.gate_twaps = Some(gate_twaps);

            assert_eq!(
                state.decide_engine(1, &input, &EpochParams::DEFAULT),
                Ok(DecisionOutcome::Reject(expected))
            );
        }

        let mut state = EpochState::<[u8; 32]>::new();
        let mut proposal = prop(1, ProposalState::Trading);
        proposal.class = ProposalClass::Treasury;
        proposal.ask = 1;
        proposal.markets = Some(MarketSet {
            accept: 1,
            reject: 2,
            gates: None,
            baseline: 3,
        });
        state.proposals.push(proposal);
        assert_eq!(
            state.decide_engine(1, &pass_input(), &EpochParams::DEFAULT),
            Ok(DecisionOutcome::Reject(RejectReason::NotDecisionGrade))
        );
    }

    #[test]
    fn param_requires_gate_books_and_both_vetoes_are_reachable() {
        for (gate_twaps, expected) in [
            (
                [
                    FixedU64(100_000_000),
                    FixedU64(100_000_000),
                    FixedU64(0),
                    FixedU64(0),
                ],
                RejectReason::GateVetoSurvival,
            ),
            (
                [
                    FixedU64(0),
                    FixedU64(0),
                    FixedU64(100_000_000),
                    FixedU64(100_000_000),
                ],
                RejectReason::GateVetoSecurity,
            ),
        ] {
            let mut state = EpochState::<[u8; 32]>::new();
            let mut proposal = prop(1, ProposalState::Trading);
            proposal.markets = Some(MarketSet {
                accept: 1,
                reject: 2,
                gates: Some([3, 4, 5, 6]),
                baseline: 7,
            });
            state.proposals.push(proposal);
            let mut input = pass_input();
            input.gate_twaps = Some(gate_twaps);

            assert_eq!(
                state.decide_engine(1, &input, &EpochParams::DEFAULT),
                Ok(DecisionOutcome::Reject(expected))
            );
        }

        let mut state = EpochState::<[u8; 32]>::new();
        let mut proposal = prop(1, ProposalState::Trading);
        proposal.markets = Some(MarketSet {
            accept: 1,
            reject: 2,
            gates: None,
            baseline: 3,
        });
        state.proposals.push(proposal);
        assert_eq!(
            state.decide_engine(1, &pass_input(), &EpochParams::DEFAULT),
            Ok(DecisionOutcome::Reject(RejectReason::NotDecisionGrade))
        );
    }

    #[test]
    fn code_without_physically_deployed_gates_rejects_not_decision_grade() {
        let mut s = EpochState::<[u8; 32]>::new();
        let mut p = prop(1, ProposalState::Trading);
        p.class = ProposalClass::Code;
        p.markets = Some(MarketSet {
            accept: 1,
            reject: 2,
            gates: None,
            baseline: 3,
        });
        s.proposals.push(p);
        assert_eq!(
            s.decide_engine(1, &pass_input(), &EpochParams::DEFAULT),
            Ok(DecisionOutcome::Reject(RejectReason::NotDecisionGrade))
        );
    }

    #[derive(Default)]
    struct RecordingWelfare {
        calls: Vec<(EpochId, MetricSpecVersion, SettlementTarget)>,
        /// Epochs whose Baseline vault was settled through the VOID path.
        void_settlements: Vec<EpochId>,
    }

    impl WelfareOps for RecordingWelfare {
        fn compute_settlement(
            &mut self,
            cohort_epoch: EpochId,
            spec: MetricSpecVersion,
            target: SettlementTarget,
        ) -> Result<FixedU64, Error> {
            self.calls.push((cohort_epoch, spec, target));
            Ok(FixedU64(500_000_000))
        }

        fn settle_baseline_void(&mut self, cohort_epoch: EpochId) -> Result<(), Error> {
            self.void_settlements.push(cohort_epoch);
            Ok(())
        }
    }

    struct RefusingLedger;

    impl LedgerOps<[u8; 32]> for RefusingLedger {
        fn create_vault(
            &mut self,
            _pid: ProposalId,
            _spec: MetricSpecVersion,
        ) -> Result<(), Error> {
            Err(Error::Ledger)
        }

        fn resolve(&mut self, _pid: ProposalId, _branch: Branch) -> Result<(), Error> {
            Err(Error::Ledger)
        }

        fn void(&mut self, _pid: ProposalId) -> Result<(), Error> {
            Err(Error::Ledger)
        }
    }

    #[test]
    fn settle_cohort_delegates_every_target_only_to_welfare() {
        // 05 §6: epoch has no SettleAuthority and cannot double-settle the ledger.
        // Its seam is limited to welfare's proposal/baseline endpoint.
        let mut s = EpochState::<[u8; 32]>::new();
        s.epoch.phase = EpochPhase::Housekeeping;
        s.epoch.index = 3;
        let mut p = prop(1, ProposalState::Measuring);
        p.class = ProposalClass::Code;
        p.markets = Some(MarketSet {
            accept: 1,
            reject: 2,
            gates: Some([1, 2, 3, 4]),
            baseline: 5,
        });
        s.proposals.push(p);
        s.cohorts.push(CohortInfo {
            epoch: 0,
            proposals: alloc::vec![1],
            status: CohortStatus::Measuring { until_epoch: 2 },
        });

        let mut welfare = RecordingWelfare::default();
        s.settle_cohort(
            Origin::Keeper,
            &mut welfare,
            0,
            1,
            FixedU64(500_000_000),
            100,
        )
        .unwrap();
        assert_eq!(s.cohorts[0].status, CohortStatus::Settling { cursor: 1 });
        s.settle_cohort(
            Origin::Keeper,
            &mut welfare,
            0,
            1,
            FixedU64(500_000_000),
            101,
        )
        .unwrap();
        assert_eq!(
            welfare.calls,
            vec![
                (
                    0,
                    1,
                    SettlementTarget::Proposal {
                        pid: 1,
                        has_gate_books: true,
                    },
                ),
                (0, 1, SettlementTarget::Baseline),
            ]
        );
        assert!(s.cohorts.is_empty());
        assert!(s.proposals.is_empty());
        assert_eq!(s.recent.len(), 1);
        // A successfully settled cohort never takes the VOID Baseline path.
        assert!(welfare.void_settlements.is_empty());
    }

    #[test]
    fn void_cohort_settles_the_epoch_baseline_vault_once() {
        // SQ-92 / 03 §2.3/§5: an epoch VOID skips welfare *scoring* but must
        // still settle the epoch's Baseline vault at the neutral score. Without
        // it the vault stays `Open` forever and single-sided holders — whose
        // redemptions require `Settled` — can never redeem.
        let mut s = EpochState::<[u8; 32]>::new();
        let mut ledger = LedgerState::<[u8; 32]>::new();
        let mut p = prop(1, ProposalState::Measuring);
        p.class = ProposalClass::Code;
        p.markets = Some(MarketSet {
            accept: 1,
            reject: 2,
            gates: None,
            baseline: 5,
        });
        ledger.create_vault(1, 1).unwrap();
        s.proposals.push(p);
        s.cohorts.push(CohortInfo {
            epoch: 7,
            proposals: alloc::vec![1],
            status: CohortStatus::Measuring { until_epoch: 2 },
        });

        let mut welfare = RecordingWelfare::default();
        s.void_cohort(Origin::VoidAuthority, &mut ledger, &mut welfare, 7, 100)
            .unwrap();

        // Exactly one Baseline settlement, keyed to the voided epoch.
        assert_eq!(welfare.void_settlements, vec![7]);
        // And no welfare *scoring* — a VOID trusts no measurement.
        assert!(welfare.calls.is_empty());
        assert!(s.cohorts.is_empty());
    }

    #[test]
    fn refusing_ledger_leaves_the_core_at_status_quo() {
        let mut state = EpochState::<[u8; 32]>::new();
        let mut proposal = prop(1, ProposalState::Trading);
        proposal.markets = Some(MarketSet {
            accept: 1,
            reject: 2,
            gates: Some([3, 4, 5, 6]),
            baseline: 7,
        });
        state.proposals.push(proposal);
        let before = state.clone();
        let mut input = pass_input();
        input.constitution_queue_ok = false;
        assert_eq!(
            state.decide(Origin::Keeper, &mut RefusingLedger, 1, 0, input,),
            Err(Error::Ledger)
        );
        assert_eq!(state, before);
    }

    #[test]
    fn injected_params_change_the_hurdle_without_changing_defaults() {
        let mut s = EpochState::<[u8; 32]>::new();
        let mut proposal = prop(1, ProposalState::Trading);
        proposal.markets = Some(MarketSet {
            accept: 1,
            reject: 2,
            gates: Some([3, 4, 5, 6]),
            baseline: 7,
        });
        s.proposals.push(proposal);
        let mut input = pass_input();
        // Margin 0.040 clears the V-12 DEFAULT δ_PARAM=0.0375 (Adopt) but not a
        // stricter injected δ=0.045 (Reject).
        input.accept_full = FixedU64(540_000_000);
        input.accept_trailing = FixedU64(540_000_000);
        input.accept_spot = FixedU64(540_000_000);
        assert_eq!(
            s.decide_engine(1, &input, &EpochParams::DEFAULT),
            Ok(DecisionOutcome::Adopt)
        );
        let mut strict = EpochParams::DEFAULT;
        strict.delta[0] = FixedU64(45_000_000);
        assert_eq!(
            s.decide_engine(1, &input, &strict),
            Ok(DecisionOutcome::Reject(RejectReason::HurdleNotMet))
        );
    }

    #[test]
    fn cohort_summary_preserves_the_twelve_slot_hard_max() {
        let mut state = EpochState::<[u8; 32]>::new();
        let pids: Vec<u64> = (1..=u64::from(bounds::MAX_COHORT_PROPOSALS)).collect();
        for pid in &pids {
            let mut proposal = prop(*pid, ProposalState::Settled);
            proposal.decision = Some(DecisionOutcome::Adopt);
            state.proposals.push(proposal);
        }

        state
            .push_summary(
                0,
                FixedU64(500_000_000),
                FixedU64(500_000_000),
                1,
                false,
                &pids,
            )
            .unwrap();

        assert_eq!(state.recent[0].proposals.len(), MAX_ACTIVE_PER_EPOCH);
        assert_eq!(state.recent[0].proposals.len(), 12);
    }
}

#![cfg_attr(not(feature = "std"), no_std)]
#![deny(unsafe_code)]
#![allow(clippy::too_many_arguments)]

extern crate alloc;

use alloc::vec::Vec;
use futarchy_primitives::{
    Balance, BlockNumber, Branch, CohortSummary, DecisionOutcome, EpochId, EpochPhase,
    EpochStatusView, FixedU64, GateType, MetricSpecVersion, ProposalClass, ProposalId,
    ProposalState, RejectReason, RuntimeVersionConstraint, H256,
};
use pallet_conditional_ledger::{LedgerOrigin, LedgerState};
use pallet_welfare::WelfareState;
use parity_scale_codec::{Decode, Encode, MaxEncodedLen};
use scale_info::TypeInfo;

macro_rules! ensure {
    ($cond:expr, $err:expr $(,)?) => {
        if !$cond {
            return Err($err);
        }
    };
}

pub const DEFAULT_EPOCH_LENGTH: BlockNumber = 302_400;
pub const MIN_EPOCH_LENGTH: BlockNumber = 201_600;
pub const PHASE_DENOM: BlockNumber = 21;
pub const MAX_INTAKE_QUEUE: usize = 64;
pub const MAX_LIVE_PROPOSALS: usize = 32;
pub const MAX_ACTIVE_PER_EPOCH: usize = 5;
pub const MAX_NON_TERMINAL_COHORTS: usize = 4;
pub const RECENT_COHORTS: usize = 32;
pub const MAX_RESOURCES_PER_PROPOSAL: usize = 8;
pub const DECISION_WINDOW: BlockNumber = 43_200;
pub const DECISION_EXTENSION: BlockNumber = 43_200;
pub const TRAILING_WINDOW: BlockNumber = 14_400;
pub const STALE_EPOCH_BOUND: BlockNumber = 100_800;
pub const ONE: u64 = 1_000_000_000;
pub const ONE_PP: u64 = 10_000_000;
/// Convergence bound `dec.delta_max` — default 0.05 (13 §2; 05 §5.2/§5.4 step 8).
/// The spot-vs-window TWAP gap must sit within this for both decision books.
pub const DELTA_MAX_1E9: u64 = 50_000_000;
pub const SF: u128 = 3;
/// Gate-veto absolute ruin cap `gate.p_max` — default 0.05 (13 §2; kernel ceiling
/// 0.10). The ADOPT gate-book TWAP above this vetoes before any welfare upside
/// is weighed (05 §5.1).
pub const GATE_P_MAX_1E9: u64 = 50_000_000;
/// Gate-veto relative margin `gate.eps` — default 0.02 (13 §2; 05 §5.1).
pub const GATE_EPS_1E9: u64 = 20_000_000;

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub enum Origin {
    Signed,
    Keeper,
    Root,
    GuardianHold,
    ExecutionGuard,
}

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
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

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub struct MarketSet {
    pub accept: u64,
    pub reject: u64,
    pub gates: Option<[u64; 4]>,
    pub baseline: u64,
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
    pub welfare_grade_ok: bool,
    pub welfare_second_insufficient: bool,
    pub gate_grade_ok: bool,
    pub gate_twaps: Option<[FixedU64; 4]>,
    pub measured_depth: Balance,
    pub published_flow_per_day: Option<Balance>,
    pub in_cap_prize: Option<Balance>,
    pub attestation_quorate: bool,
    pub constitution_queue_ok: bool,
}

#[derive(Clone, Debug, Decode, Encode, Eq, PartialEq, TypeInfo)]
pub struct Proposal<AccountId> {
    pub id: ProposalId,
    pub proposer: AccountId,
    pub class: ProposalClass,
    pub state: ProposalState,
    pub epoch: EpochId,
    pub submitted_at: BlockNumber,
    pub payload_hash: H256,
    pub ask: Balance,
    pub bond: Balance,
    pub resources: Vec<[u8; 8]>,
    pub metric_spec: MetricSpecVersion,
    pub decide_at: BlockNumber,
    pub rerun: bool,
    pub extended: bool,
    pub delayed_once: bool,
    pub markets: Option<MarketSet>,
    pub maturity: Option<BlockNumber>,
    pub grace_end: Option<BlockNumber>,
    pub version_constraint: Option<RuntimeVersionConstraint>,
    pub decision: Option<DecisionOutcome>,
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
    BadDecisionInput,
    ArithmeticOverflow,
    Ledger,
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
    pub ledger_frozen: bool,
    pub phase_flags: u32,
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
            ledger_frozen: false,
            phase_flags: 0,
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
    ) -> Result<(), Error> {
        ensure!(matches!(origin, Origin::Root), Error::BadOrigin);
        ensure!(
            length >= MIN_EPOCH_LENGTH
                && length % PHASE_DENOM == 0
                && DECISION_WINDOW <= length.saturating_mul(13) / 21,
            Error::BadEpochLength
        );
        self.epoch.next_length = length;
        Ok(())
    }
    pub fn sync_phase(&mut self, now: BlockNumber) {
        while now
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
        }
        let phase = phase_at(
            now.saturating_sub(self.epoch.epoch_start_block),
            self.epoch.length,
        );
        if phase != self.epoch.phase {
            self.epoch.phase = phase;
            self.epoch.phase_start_block = self.phase_start(phase);
        }
    }
    pub fn submit(&mut self, origin: Origin, proposal: Proposal<AccountId>) -> Result<(), Error> {
        ensure!(matches!(origin, Origin::Signed), Error::BadOrigin);
        ensure!(self.epoch.phase == EpochPhase::Intake, Error::BadPhase);
        ensure!(
            self.intake_queue.len() < MAX_INTAKE_QUEUE,
            Error::IntakeFull
        );
        ensure!(
            self.proposals.len() < MAX_LIVE_PROPOSALS,
            Error::TooManyLiveProposals
        );
        ensure!(
            proposal.resources.len() <= MAX_RESOURCES_PER_PROPOSAL,
            Error::TooManyResources
        );
        ensure!(
            self.proposals.iter().all(|p| p.id != proposal.id),
            Error::DuplicateProposal
        );
        ensure!(proposal.state == ProposalState::Submitted, Error::BadState);
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
        let p = self.proposal_mut(pid)?;
        ensure!(
            p.state == ProposalState::Submitted && &p.proposer == who,
            Error::BadState
        );
        p.state = ProposalState::Cancelled;
        self.intake_queue.retain(|x| *x != pid);
        self.events.push(Event::ProposalWithdrawn(pid));
        Ok(())
    }
    pub fn qualify(
        &mut self,
        origin: Origin,
        pid: ProposalId,
        static_checks_pass: bool,
    ) -> Result<(), Error> {
        ensure!(matches!(origin, Origin::Keeper), Error::BadOrigin);
        ensure!(self.epoch.phase == EpochPhase::Qualify, Error::BadPhase);
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
        let resources = self.proposal(pid)?.resources.clone();
        ensure!(
            self.proposal(pid)?.state == ProposalState::Submitted,
            Error::BadState
        );
        self.events.push(Event::ScreeningStarted(pid));
        if !static_checks_pass {
            self.cancel(pid, RejectReason::ConstitutionViolation, true)?;
            return Ok(());
        }
        if active >= MAX_ACTIVE_PER_EPOCH {
            self.proposal_mut(pid)?.state = ProposalState::Submitted;
            self.events.push(Event::ProposalDeferred(pid));
            return Ok(());
        }
        ensure!(
            resources
                .iter()
                .all(|r| self.resource_locks.iter().all(|(l, _)| l != r)),
            Error::LockConflict
        );
        let decide_at = self
            .epoch
            .epoch_start_block
            .saturating_add(self.epoch.length.saturating_mul(18) / 21);
        for r in resources {
            self.resource_locks.push((r, pid));
        }
        let p = self.proposal_mut(pid)?;
        p.state = ProposalState::Qualified;
        p.decide_at = decide_at;
        self.intake_queue.retain(|x| *x != pid);
        self.events.push(Event::ProposalQualified(pid));
        Ok(())
    }
    pub fn open_markets(
        &mut self,
        origin: Origin,
        ledger: &mut LedgerState<AccountId>,
        pid: ProposalId,
        markets: MarketSet,
    ) -> Result<(), Error> {
        ensure!(matches!(origin, Origin::Keeper), Error::BadOrigin);
        ensure!(self.epoch.phase == EpochPhase::Seed, Error::BadPhase);
        let spec = self.proposal(pid)?.metric_spec;
        let p = self.proposal_mut(pid)?;
        ensure!(
            p.state == ProposalState::Qualified || p.state == ProposalState::Rerun,
            Error::BadState
        );
        if p.state == ProposalState::Qualified {
            ledger.create_vault(pid, spec).map_err(|_| Error::Ledger)?;
        }
        p.markets = Some(markets);
        p.state = ProposalState::Trading;
        self.events.push(Event::MarketsOpened(pid));
        Ok(())
    }
    pub fn decide(
        &mut self,
        origin: Origin,
        ledger: &mut LedgerState<AccountId>,
        pid: ProposalId,
        now: BlockNumber,
        input: DecisionInputs,
    ) -> Result<DecisionOutcome, Error> {
        ensure!(matches!(origin, Origin::Keeper), Error::BadOrigin);
        ensure!(now >= self.proposal(pid)?.decide_at, Error::BadPhase);
        let out = self.decide_engine(pid, &input)?;
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
                let maturity = now.saturating_add(timelock(self.proposal(pid)?.class));
                let payload_hash;
                {
                    let p = self.proposal_mut(pid)?;
                    p.state = ProposalState::Queued;
                    p.maturity = Some(maturity);
                    p.grace_end = Some(maturity.saturating_add(grace(p.class)));
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
    pub fn force_reject_process_hold(
        &mut self,
        origin: Origin,
        ledger: &mut LedgerState<AccountId>,
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
            ledger
                .void(LedgerOrigin::ResolveAuthority, pid)
                .map_err(|_| Error::Ledger)?;
        }
        self.proposal_mut(pid)?.state = ProposalState::Rejected(RejectReason::ProcessHold);
        self.events.push(Event::ProposalRejected {
            pid,
            reason: RejectReason::ProcessHold,
        });
        self.events.push(Event::ProposalForceRejected {
            pid,
            reason: RejectReason::ProcessHold,
        });
        Ok(())
    }
    pub fn mark_executed(
        &mut self,
        origin: Origin,
        ledger: &mut LedgerState<AccountId>,
        pid: ProposalId,
    ) -> Result<(), Error> {
        ensure!(matches!(origin, Origin::ExecutionGuard), Error::BadOrigin);
        let p = self.proposal_mut(pid)?;
        ensure!(
            p.state == ProposalState::Queued || p.state == ProposalState::FailedExecuted,
            Error::BadState
        );
        p.state = ProposalState::Executed;
        ledger
            .resolve(LedgerOrigin::ResolveAuthority, pid, Branch::Accept)
            .map_err(|_| Error::Ledger)?;
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
    pub fn retry_exhausted_to_measurement(
        &mut self,
        origin: Origin,
        ledger: &mut LedgerState<AccountId>,
        pid: ProposalId,
    ) -> Result<(), Error> {
        ensure!(matches!(origin, Origin::ExecutionGuard), Error::BadOrigin);
        ensure!(
            self.proposal(pid)?.state == ProposalState::FailedExecuted,
            Error::BadState
        );
        ledger
            .resolve(LedgerOrigin::ResolveAuthority, pid, Branch::Accept)
            .map_err(|_| Error::Ledger)?;
        self.start_measurement(pid)
    }
    pub fn expire_or_stale_queue(
        &mut self,
        origin: Origin,
        ledger: &mut LedgerState<AccountId>,
        pid: ProposalId,
        reason: Option<RejectReason>,
    ) -> Result<(), Error> {
        ensure!(
            matches!(origin, Origin::Keeper | Origin::ExecutionGuard),
            Error::BadOrigin
        );
        let p = self.proposal_mut(pid)?;
        ensure!(p.state == ProposalState::Queued, Error::BadState);
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
                self.proposal_mut(pid)?.state = ProposalState::Expired;
                self.events.push(Event::MandateExpired(pid));
                self.reject_branch_measurement(ledger, pid)
            }
            _ => Err(Error::BadDecisionInput),
        }
    }
    pub fn settle_cohort(
        &mut self,
        origin: Origin,
        welfare: &mut WelfareState,
        ledger: &mut LedgerState<AccountId>,
        epoch: EpochId,
        spec: MetricSpecVersion,
        baseline_twap: FixedU64,
        now: BlockNumber,
    ) -> Result<FixedU64, Error> {
        ensure!(matches!(origin, Origin::Keeper), Error::BadOrigin);
        ensure!(
            self.epoch.phase == EpochPhase::Housekeeping,
            Error::BadPhase
        );
        let idx = self
            .cohorts
            .iter()
            .position(|c| c.epoch == epoch)
            .ok_or(Error::BadState)?;
        let score = welfare
            .compute_settlement(epoch, spec)
            .map_err(|_| Error::Welfare)?;
        // 05 §4.7/§5.1/§6: the gate books settle on the daily breach flags — the
        // Survival gate on the S floor-breach flag and the Security gate on the C
        // floor-breach flag, set iff the flag fired on >= 1 day across the two
        // measurement epochs e+1..e+2. Only C_onchain and S drive these; no attested
        // value can flip a gate outcome.
        let f1 = welfare.gate_breach(epoch.saturating_add(1));
        let f2 = welfare.gate_breach(epoch.saturating_add(2));
        let survival_breached = f1.s_breached || f2.s_breached;
        let security_breached = f1.c_breached || f2.c_breached;
        for pid in self.cohorts[idx].proposals.clone() {
            if let Some(p) = self.proposals.iter_mut().find(|p| p.id == pid) {
                ledger
                    .settle_scalar(LedgerOrigin::SettleAuthority, pid, score)
                    .map_err(|_| Error::Ledger)?;
                // Gate books exist only for gate-bearing proposals (05 §5.1:
                // CODE | META | TREASURY > 1% NAV). Settle those on the breach
                // flags and propagate failures rather than masking them; a
                // gateless proposal has no gate market to settle.
                if requires_gate(p.class, p.ask) {
                    ledger
                        .settle_gate(
                            LedgerOrigin::SettleAuthority,
                            pid,
                            GateType::Survival,
                            survival_breached,
                        )
                        .map_err(|_| Error::Ledger)?;
                    ledger
                        .settle_gate(
                            LedgerOrigin::SettleAuthority,
                            pid,
                            GateType::Security,
                            security_breached,
                        )
                        .map_err(|_| Error::Ledger)?;
                }
                p.state = ProposalState::Settled;
                p.decision.get_or_insert(DecisionOutcome::Adopt);
            }
        }
        ledger
            .settle_baseline(LedgerOrigin::SettleAuthority, epoch, score)
            .ok();
        self.cohorts[idx].status = CohortStatus::Settled;
        self.push_summary(epoch, score, baseline_twap, now)?;
        self.events.push(Event::CohortSettled { epoch, s: score });
        Ok(score)
    }
    pub fn try_state(&self) -> Result<(), Error> {
        ensure!(
            self.proposals.len() <= MAX_LIVE_PROPOSALS
                && self.intake_queue.len() <= MAX_INTAKE_QUEUE
                && self.recent.len() <= RECENT_COHORTS,
            Error::TryStateViolation
        );
        ensure!(
            self.cohorts
                .iter()
                .filter(|c| !matches!(c.status, CohortStatus::Settled | CohortStatus::Void))
                .count()
                <= MAX_NON_TERMINAL_COHORTS,
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
        for (i, (r, pid)) in self.resource_locks.iter().enumerate() {
            ensure!(
                self.resource_locks[..i].iter().all(|(seen, _)| seen != r)
                    && self.proposals.iter().any(|p| p.id == *pid),
                Error::TryStateViolation
            );
        }
        Ok(())
    }

    fn decide_engine(&self, pid: ProposalId, i: &DecisionInputs) -> Result<DecisionOutcome, Error> {
        let p = self.proposal(pid)?;
        ensure!(
            matches!(p.state, ProposalState::Trading | ProposalState::Extended),
            Error::BadState
        );
        if self.dead_man_armed || self.ledger_frozen {
            return Ok(DecisionOutcome::Reject(RejectReason::ProcessHold));
        }
        if requires_gate(p.class, p.ask) {
            ensure!(i.gate_twaps.is_some(), Error::BadDecisionInput);
            if !i.gate_grade_ok {
                return Ok(DecisionOutcome::Reject(RejectReason::NotDecisionGrade));
            }
            let g = i.gate_twaps.unwrap();
            // 05 §5.1: veto iff adopt TWAP exceeds the absolute ruin cap p_max, or
            // exceeds the reject TWAP by more than the relative margin eps, for either
            // gate. Ordered before welfare — no upside overrides a veto (G-4, I-14).
            if g[0].0 > GATE_P_MAX_1E9 || g[0].0 > g[1].0.saturating_add(GATE_EPS_1E9) {
                return Ok(DecisionOutcome::Reject(RejectReason::GateVetoSurvival));
            }
            if g[2].0 > GATE_P_MAX_1E9 || g[2].0 > g[3].0.saturating_add(GATE_EPS_1E9) {
                return Ok(DecisionOutcome::Reject(RejectReason::GateVetoSecurity));
            }
        }
        if !i.welfare_grade_ok {
            return Ok(if !p.extended && !i.welfare_second_insufficient {
                DecisionOutcome::Extend
            } else {
                DecisionOutcome::Reject(RejectReason::NotDecisionGrade)
            });
        }
        let delta = class_delta(p.class).saturating_add(if p.rerun { ONE_PP } else { 0 });
        let r_eff = i
            .reject_full
            .0
            .max(i.baseline_full.0.saturating_sub(class_sigma(p.class)));
        let full = i.accept_full.0 >= r_eff.saturating_add(delta);
        let tail_eff = i
            .reject_trailing
            .0
            .max(i.baseline_trailing.0.saturating_sub(class_sigma(p.class)));
        let tail = i.accept_trailing.0 >= tail_eff.saturating_add(delta);
        let conv = i.accept_spot.0.abs_diff(i.accept_full.0) <= DELTA_MAX_1E9
            && i.reject_spot.0.abs_diff(i.reject_full.0) <= DELTA_MAX_1E9;
        if !(full && tail && conv) {
            return Ok(match (full != tail, p.extended, conv) {
                (true, false, _) => DecisionOutcome::Extend,
                (true, true, _) => DecisionOutcome::Reject(RejectReason::SecondExtensionFailed),
                (false, _, true) => DecisionOutcome::Reject(RejectReason::HurdleNotMet),
                _ => DecisionOutcome::Reject(RejectReason::ConvergenceFailed),
            });
        }
        let attack = attack_cost_hat(i.measured_depth, i.published_flow_per_day)?;
        let prize = i.in_cap_prize.ok_or(Error::BadDecisionInput)?;
        if prize.saturating_mul(SF) > attack {
            return Ok(DecisionOutcome::Reject(RejectReason::SecuritySizing));
        }
        if matches!(p.class, ProposalClass::Code | ProposalClass::Meta) && !i.attestation_quorate {
            return Ok(DecisionOutcome::Reject(RejectReason::AttestationMissing));
        }
        if !i.constitution_queue_ok {
            return Ok(DecisionOutcome::Reject(RejectReason::RateLimited));
        }
        Ok(DecisionOutcome::Adopt)
    }
    fn cancel(&mut self, pid: ProposalId, reason: RejectReason, slash: bool) -> Result<(), Error> {
        let bond = self.proposal(pid)?.bond;
        self.proposal_mut(pid)?.state = ProposalState::Cancelled;
        self.intake_queue.retain(|x| *x != pid);
        self.events.push(Event::ProposalCancelled { pid, reason });
        if slash {
            self.events.push(Event::IntakeSlashed {
                pid,
                reason,
                amount: bond,
            });
        }
        Ok(())
    }
    fn reject_to_measurement(
        &mut self,
        ledger: &mut LedgerState<AccountId>,
        pid: ProposalId,
        r: RejectReason,
    ) -> Result<(), Error> {
        self.proposal_mut(pid)?.state = ProposalState::Rejected(r);
        self.events.push(Event::ProposalRejected { pid, reason: r });
        if self.proposal(pid)?.markets.is_some() {
            self.reject_branch_measurement(ledger, pid)?;
        }
        Ok(())
    }
    fn reject_branch_measurement(
        &mut self,
        ledger: &mut LedgerState<AccountId>,
        pid: ProposalId,
    ) -> Result<(), Error> {
        // T21 (05 §2.1): a rejected/expired proposal whose markets were deployed
        // resolves its vault to the REJECT branch before measurement, so the REJECT
        // leg trades through and settle_cohort can settle a `Resolved` vault. Without
        // this the vault stays `Open` and `settle_scalar` errors, stranding the cohort.
        ledger
            .resolve(LedgerOrigin::ResolveAuthority, pid, Branch::Reject)
            .map_err(|_| Error::Ledger)?;
        self.start_measurement(pid)
    }
    fn start_measurement(&mut self, pid: ProposalId) -> Result<(), Error> {
        let epoch = self.proposal(pid)?.epoch;
        if let Some(c) = self.cohorts.iter_mut().find(|c| c.epoch == epoch) {
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
                    until_epoch: epoch.saturating_add(2),
                },
            });
        }
        self.proposal_mut(pid)?.state = ProposalState::Measuring;
        self.events
            .push(Event::MeasurementStarted { cohort: epoch });
        Ok(())
    }
    fn push_summary(
        &mut self,
        epoch: EpochId,
        s: FixedU64,
        baseline_twap: FixedU64,
        now: BlockNumber,
    ) -> Result<(), Error> {
        let mut proposals = futarchy_primitives::BoundedVec::<
            (ProposalId, ProposalClass, DecisionOutcome),
            5,
        >::new();
        for p in self.proposals.iter().filter(|p| p.epoch == epoch).take(5) {
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
            voided: false,
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
            EpochPhase::Qualify => self.epoch.length * 3 / 21,
            EpochPhase::Seed => self.epoch.length * 4 / 21,
            EpochPhase::Trade => self.epoch.length * 5 / 21,
            EpochPhase::Decide => self.epoch.length * 18 / 21,
            EpochPhase::Review | EpochPhase::Execute => self.epoch.length * 18 / 21 + 1,
            EpochPhase::Housekeeping => self.epoch.length * 20 / 21,
        })
    }
}
impl<AccountId: Clone + Eq> Default for EpochState<AccountId> {
    fn default() -> Self {
        Self::new()
    }
}

fn phase_at(offset: BlockNumber, l: BlockNumber) -> EpochPhase {
    if offset < l * 3 / 21 {
        EpochPhase::Intake
    } else if offset < l * 4 / 21 {
        EpochPhase::Qualify
    } else if offset < l * 5 / 21 {
        EpochPhase::Seed
    } else if offset < l * 18 / 21 {
        EpochPhase::Trade
    } else if offset < l * 20 / 21 {
        EpochPhase::Decide
    } else {
        EpochPhase::Housekeeping
    }
}
fn phase_len(p: EpochPhase, l: BlockNumber) -> BlockNumber {
    match p {
        EpochPhase::Intake => l * 3 / 21,
        EpochPhase::Qualify | EpochPhase::Seed => l / 21,
        EpochPhase::Trade => l * 13 / 21,
        EpochPhase::Decide => l * 2 / 21,
        EpochPhase::Review | EpochPhase::Execute => 0,
        EpochPhase::Housekeeping => l / 21,
    }
}
fn requires_gate(c: ProposalClass, ask: Balance) -> bool {
    matches!(c, ProposalClass::Code | ProposalClass::Meta)
        || (matches!(c, ProposalClass::Treasury) && ask > 0)
}
// `dec.delta` per class — the hurdle floors of the Ask-scaled schedule
// (13 §2; 05 §5.4 step 6). PARAM/TREASURY/CODE/META = 0.015/0.025/0.040/0.060.
// CONSTITUTIONAL is unreachable through this engine (hurdle 1.0).
fn class_delta(c: ProposalClass) -> u64 {
    match c {
        ProposalClass::Param => 15_000_000,
        ProposalClass::Treasury => 25_000_000,
        ProposalClass::Code => 40_000_000,
        ProposalClass::Meta => 60_000_000,
        ProposalClass::Constitutional => ONE,
    }
}
// `dec.sigma` per class — the reject-leg baseline slack (13 §2; 05 §5.3/§5.4).
// PARAM/TREASURY/CODE/META = 0.003/0.005/0.008/0.010.
fn class_sigma(c: ProposalClass) -> u64 {
    match c {
        ProposalClass::Param => 3_000_000,
        ProposalClass::Treasury => 5_000_000,
        ProposalClass::Code => 8_000_000,
        ProposalClass::Meta => 10_000_000,
        ProposalClass::Constitutional => 0,
    }
}
fn timelock(c: ProposalClass) -> BlockNumber {
    match c {
        ProposalClass::Param => 14_400,
        ProposalClass::Treasury => 43_200,
        ProposalClass::Code | ProposalClass::Meta => 86_400,
        ProposalClass::Constitutional => 0,
    }
}
fn grace(c: ProposalClass) -> BlockNumber {
    match c {
        ProposalClass::Param => 43_200,
        ProposalClass::Treasury => 86_400,
        ProposalClass::Code | ProposalClass::Meta => 201_600,
        ProposalClass::Constitutional => 0,
    }
}
fn attack_cost_hat(depth: Balance, flow: Option<Balance>) -> Result<Balance, Error> {
    let l_half = depth.checked_div(2).ok_or(Error::ArithmeticOverflow)?;
    let f = flow.map(|x| x.min(l_half)).unwrap_or(l_half);
    f.checked_mul(3).ok_or(Error::ArithmeticOverflow)
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
    use pallet_conditional_ledger::Event as LedgerEvent;
    use pallet_welfare::{GateBreachFlags, Snapshot};
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
            ask: 0,
            bond: 10,
            resources: alloc::vec![[id as u8; 8]],
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
            welfare_grade_ok: true,
            welfare_second_insufficient: false,
            gate_grade_ok: true,
            gate_twaps: None,
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
        s.set_next_epoch_length(Origin::Root, 201_600).unwrap();
        s.sync_phase(302_400);
        assert_eq!(s.epoch.length, 201_600);
    }
    #[test]
    fn submit_qualify_decide_adopt() {
        let mut s = EpochState::new();
        let mut ledger = LedgerState::<[u8; 32]>::new();
        s.submit(Origin::Signed, prop(1, ProposalState::Submitted))
            .unwrap();
        s.sync_phase(43_200);
        s.qualify(Origin::Keeper, 1, true).unwrap();
        s.epoch.phase = EpochPhase::Trade;
        s.proposal_mut(1).unwrap().state = ProposalState::Trading;
        s.proposal_mut(1).unwrap().markets = Some(MarketSet {
            accept: 1,
            reject: 2,
            gates: None,
            baseline: 3,
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
            gates: None,
            baseline: 3,
        });
        s.proposals.push(p);
        let mut i = pass_input();
        i.welfare_grade_ok = false;
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
            gates: None,
            baseline: 3,
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
    fn recent_ring_is_bounded() {
        let mut s = EpochState::<[u8; 32]>::new();
        for e in 0..40 {
            s.push_summary(e, FixedU64(1), FixedU64(2), e).unwrap();
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
        // 13 §2: dec.delta = 0.015/0.025/0.040/0.060 and dec.sigma =
        // 0.003/0.005/0.008/0.010 for PARAM/TREASURY/CODE/META. CODE and META
        // must differ (they were conflated at 0.040/0 previously).
        assert_eq!(class_delta(ProposalClass::Param), 15_000_000);
        assert_eq!(class_delta(ProposalClass::Treasury), 25_000_000);
        assert_eq!(class_delta(ProposalClass::Code), 40_000_000);
        assert_eq!(class_delta(ProposalClass::Meta), 60_000_000);
        assert_eq!(class_sigma(ProposalClass::Param), 3_000_000);
        assert_eq!(class_sigma(ProposalClass::Treasury), 5_000_000);
        assert_eq!(class_sigma(ProposalClass::Code), 8_000_000);
        assert_eq!(class_sigma(ProposalClass::Meta), 10_000_000);
    }
    #[test]
    fn param_hurdle_adopts_at_spec_delta() {
        // A PARAM margin of 0.017 clears the spec hurdle δ=0.015 (adopt) but would
        // have failed the old hardcoded δ=0.020. Pins finding #4 into the engine.
        let mut s = EpochState::new();
        let mut ledger = LedgerState::<[u8; 32]>::new();
        ledger.create_vault(1, 1).unwrap();
        let mut p = prop(1, ProposalState::Trading);
        p.markets = Some(MarketSet {
            accept: 1,
            reject: 2,
            gates: None,
            baseline: 3,
        });
        s.proposals.push(p);
        let mut i = pass_input();
        i.accept_full = FixedU64(517_000_000);
        i.accept_trailing = FixedU64(517_000_000);
        i.accept_spot = FixedU64(517_000_000);
        assert_eq!(
            s.decide(Origin::Keeper, &mut ledger, 1, 0, i).unwrap(),
            DecisionOutcome::Adopt
        );
    }
    #[test]
    fn gate_veto_uses_configured_cap() {
        // 13 §2 / 05 §5.1: an adopt-gate Survival TWAP of 0.10 sits above the
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
            s.decide_engine(1, &i),
            Ok(DecisionOutcome::Reject(RejectReason::GateVetoSurvival))
        );
    }
    fn wsnapshot(epoch: EpochId, welfare: u64) -> Snapshot {
        Snapshot {
            epoch,
            spec_version: 1,
            s_pillar: FixedU64(welfare),
            c_onchain: FixedU64(welfare),
            c_attested: FixedU64(welfare),
            p_pillar: FixedU64(welfare),
            a_pillar: FixedU64(welfare),
            gate_s: FixedU64(ONE),
            gate_c: FixedU64(ONE),
            welfare: FixedU64(welfare),
            components: Vec::new(),
        }
    }
    #[test]
    fn settle_cohort_reads_gate_breach_flags() {
        // 05 §4.7/§6: gate books settle on the daily breach flags over e+1..e+2,
        // not a hardcoded `false`. A Survival breach recorded at e+1 must settle the
        // Survival gate `true` while Security (unbreached) settles `false`.
        let mut s = EpochState::<[u8; 32]>::new();
        s.epoch.phase = EpochPhase::Housekeeping;
        let mut p = prop(1, ProposalState::Measuring);
        p.class = ProposalClass::Code; // gate-bearing: carries gate books
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

        let mut welfare = WelfareState::new();
        welfare.snapshots.push(wsnapshot(1, 500_000_000));
        welfare.snapshots.push(wsnapshot(2, 500_000_000));
        welfare.gate_flags.push((
            1,
            GateBreachFlags {
                s_breached: true,
                c_breached: false,
                day_bitmap: [0; 2],
            },
        ));

        let mut ledger = LedgerState::<[u8; 32]>::new();
        ledger.create_vault(1, 1).unwrap();
        ledger
            .resolve(LedgerOrigin::ResolveAuthority, 1, Branch::Accept)
            .unwrap();

        s.settle_cohort(
            Origin::Keeper,
            &mut welfare,
            &mut ledger,
            0,
            1,
            FixedU64(500_000_000),
            100,
        )
        .unwrap();
        assert!(ledger
            .events
            .iter()
            .any(|e| matches!(e, LedgerEvent::GateSettled(1, _, GateType::Survival, true))));
        assert!(ledger
            .events
            .iter()
            .any(|e| matches!(e, LedgerEvent::GateSettled(1, _, GateType::Security, false))));
    }
}

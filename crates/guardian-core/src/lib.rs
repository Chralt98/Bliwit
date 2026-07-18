#![cfg_attr(not(feature = "std"), no_std)]
#![deny(unsafe_code)]

extern crate alloc;

use alloc::vec::Vec;
use futarchy_primitives::{AccountId, Balance, BlockNumber, EpochId, ProposalId, H256};
use parity_scale_codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use scale_info::TypeInfo;

pub type ActionId = u32;
pub const GUARDIAN_SEATS: usize = 7;
pub const GUARDIAN_THRESHOLD: u8 = 5;
pub const GUARDIAN_BOND: Balance = 50_000_000_000_000_000;
pub const ACTION_EXPIRY_BLOCKS: BlockNumber = 43_200;
pub const REVIEW_DEADLINE_EPOCHS: EpochId = 2;
pub const REVIEW_SLASH_PERCENT: u8 = 50;
pub const HOLD_MAX_BLOCKS: BlockNumber = futarchy_primitives::kernel::PLAYBOOK_FREEZE_WINDOW_BLOCKS;
// 13 §3.4: `frn.window = dec.extension` (shared K).
pub const FORCE_RERUN_WINDOW_BLOCKS: BlockNumber =
    futarchy_primitives::kernel::DEC_EXTENSION_BLOCKS;
pub const DELAY_ONCE_ALLOWANCE_PER_EPOCH: u8 = 2;
pub const FORCE_RERUN_ALLOWANCE_PER_EPOCH: u8 = 1;
pub const PAUSE_INTAKE_ALLOWANCE_WINDOW_EPOCHS: EpochId = 4;
pub const PAUSE_INTAKE_ALLOWANCE: u8 = 1;
pub const LEDGER_FREEZE_RENEWALS: u8 = 1;

#[derive(
    Clone,
    Copy,
    Debug,
    Decode,
    DecodeWithMemTracking,
    Encode,
    MaxEncodedLen,
    PartialEq,
    Eq,
    TypeInfo,
)]
pub enum GuardianOrigin {
    Signed,
    ConstitutionalValues,
    GuardianHold,
    EmergencyPlaybook,
}

#[derive(
    Clone,
    Copy,
    Debug,
    Decode,
    DecodeWithMemTracking,
    Encode,
    MaxEncodedLen,
    PartialEq,
    Eq,
    TypeInfo,
)]
pub enum ProposalStatus {
    Trading,
    Extended,
    Queued,
    Executed,
    Rerun,
    Other,
}

impl ProposalStatus {
    pub const fn rerunnable(self) -> bool {
        matches!(self, Self::Trading | Self::Extended | Self::Queued)
    }
}

#[derive(
    Clone,
    Copy,
    Debug,
    Decode,
    DecodeWithMemTracking,
    Encode,
    MaxEncodedLen,
    PartialEq,
    Eq,
    TypeInfo,
)]
pub enum PlaybookId {
    Depeg,
    Migration,
    OracleVoid,
    HaltIntake,
    Reserve,
    LedgerFreeze,
}

pub const ALL_PLAYBOOKS: [PlaybookId; 6] = [
    PlaybookId::Depeg,
    PlaybookId::Migration,
    PlaybookId::OracleVoid,
    PlaybookId::HaltIntake,
    PlaybookId::Reserve,
    PlaybookId::LedgerFreeze,
];

#[derive(
    Clone,
    Copy,
    Debug,
    Decode,
    DecodeWithMemTracking,
    Encode,
    MaxEncodedLen,
    PartialEq,
    Eq,
    TypeInfo,
)]
pub enum PlaybookTrigger {
    DepegMedian,
    MigrationHalt,
    OracleDeadlock,
    GateBreach,
    DeadMan,
    VoidInFlight,
    ReserveHealth,
    LedgerDrift,
}

#[derive(
    Clone,
    Copy,
    Debug,
    Decode,
    DecodeWithMemTracking,
    Encode,
    MaxEncodedLen,
    PartialEq,
    Eq,
    TypeInfo,
)]
pub enum GuardianPower {
    PauseIntake {
        until: BlockNumber,
    },
    DelayOnce {
        pid: ProposalId,
    },
    ForceRerun {
        pid: ProposalId,
    },
    ActivatePlaybook {
        id: PlaybookId,
        trigger: PlaybookTrigger,
        expiry: BlockNumber,
        /// PB-ORACLE-VOID cohort target. Every other playbook rejects `Some`.
        target: Option<EpochId>,
    },
    SuspendOnGate,
}

#[derive(
    Clone,
    Copy,
    Debug,
    Decode,
    DecodeWithMemTracking,
    Encode,
    MaxEncodedLen,
    PartialEq,
    Eq,
    TypeInfo,
)]
pub struct ActionTarget {
    pub pid: Option<ProposalId>,
    pub playbook: Option<PlaybookId>,
}

#[derive(
    Clone,
    Copy,
    Debug,
    Decode,
    DecodeWithMemTracking,
    Encode,
    MaxEncodedLen,
    PartialEq,
    Eq,
    TypeInfo,
)]
pub struct PendingAction {
    pub id: ActionId,
    pub proposer: AccountId,
    pub power: GuardianPower,
    pub justification_hash: H256,
    pub created_at: BlockNumber,
    pub expires_at: BlockNumber,
    pub dispatched: bool,
}

#[derive(
    Clone,
    Copy,
    Debug,
    Decode,
    DecodeWithMemTracking,
    Encode,
    MaxEncodedLen,
    PartialEq,
    Eq,
    TypeInfo,
)]
pub struct ReviewRecord {
    pub action_id: ActionId,
    pub deadline_epoch: EpochId,
    pub ratified: bool,
    pub recall_scheduled: bool,
    pub approvers: [AccountId; GUARDIAN_SEATS],
    pub approver_count: u8,
}

#[derive(
    Clone,
    Copy,
    Debug,
    Decode,
    DecodeWithMemTracking,
    Encode,
    MaxEncodedLen,
    PartialEq,
    Eq,
    TypeInfo,
)]
pub struct ActivePlaybook {
    pub id: PlaybookId,
    pub expiry: BlockNumber,
    pub renewals_used: u8,
}

#[derive(Clone, Debug, Decode, Encode, Eq, PartialEq, TypeInfo)]
pub enum Event {
    MembersSet {
        members: [AccountId; GUARDIAN_SEATS],
    },
    ActionProposed {
        action_id: ActionId,
        power: GuardianPower,
    },
    ActionApproved {
        action_id: ActionId,
        who: AccountId,
        approvals: u8,
    },
    GuardianAction {
        action_id: ActionId,
        power: GuardianPower,
        target: ActionTarget,
        justification_hash: H256,
    },
    ForceRerun {
        pid: ProposalId,
        justification_hash: H256,
        window_end: BlockNumber,
    },
    PlaybookActivated {
        id: PlaybookId,
        trigger: PlaybookTrigger,
        expiry: BlockNumber,
    },
    PlaybookRenewed {
        id: PlaybookId,
    },
    PlaybookExpired {
        id: PlaybookId,
    },
    ReviewScheduled {
        action: ActionId,
    },
    ActionRatified {
        action: ActionId,
    },
    ReviewFailed {
        action: ActionId,
        slashed_each: Balance,
    },
}

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub enum Error {
    BadOrigin,
    NotMember,
    DuplicateMember,
    DuplicateApproval,
    ActionNotFound,
    ActionExpired,
    AlreadyDispatched,
    TooManyPending,
    TooManyApprovals,
    TooManyReviews,
    TooManyActivePlaybooks,
    ThresholdNotMet,
    AllowanceExhausted,
    DurationTooLong,
    TriggerInactive,
    BadPlaybookTrigger,
    BadPlaybookTarget,
    AlreadyRerun,
    NotRerunnable,
    ReviewNotFound,
    AlreadyRatified,
    RenewalNotAllowed,
    PlaybookAlreadyActive,
    Overflow,
}

#[derive(
    Clone,
    Copy,
    Debug,
    Decode,
    DecodeWithMemTracking,
    Encode,
    MaxEncodedLen,
    PartialEq,
    Eq,
    TypeInfo,
)]
pub struct TriggerState {
    pub depeg: bool,
    pub migration_halt: bool,
    pub oracle_deadlock: bool,
    pub gate_breach: bool,
    pub dead_man: bool,
    pub void_in_flight: bool,
    pub reserve_health: bool,
    pub ledger_drift: bool,
}

#[derive(Clone, Debug, Decode, Encode, Eq, PartialEq, TypeInfo)]
pub struct Guardian {
    /// Fixed seat slots. A recalled seat is `None` until the values layer
    /// refills the complete council; the approval threshold remains five.
    pub members: [Option<AccountId>; GUARDIAN_SEATS],
    pub member_bonds: [Balance; GUARDIAN_SEATS],
    pub pending: Vec<PendingAction>,
    pub approvals: Vec<(ActionId, AccountId)>,
    pub reviews: Vec<ReviewRecord>,
    pub active_playbooks: Vec<ActivePlaybook>,
    pub rerun_used: Vec<ProposalId>,
    pub delay_used_this_epoch: u8,
    pub force_rerun_used_this_epoch: u8,
    pub pause_used_epoch_window_start: EpochId,
    pub pause_used_in_window: u8,
    pub current_epoch: EpochId,
    pub next_action_id: ActionId,
    pub events: Vec<Event>,
}

impl Guardian {
    pub fn new(members: [AccountId; GUARDIAN_SEATS]) -> Result<Self, Error> {
        validate_members(&members)?;
        Ok(Self {
            members: members.map(Some),
            member_bonds: [GUARDIAN_BOND; GUARDIAN_SEATS],
            pending: Vec::new(),
            approvals: Vec::new(),
            reviews: Vec::new(),
            active_playbooks: Vec::new(),
            rerun_used: Vec::new(),
            delay_used_this_epoch: 0,
            force_rerun_used_this_epoch: 0,
            pause_used_epoch_window_start: 0,
            pause_used_in_window: 0,
            current_epoch: 0,
            next_action_id: 0,
            events: Vec::new(),
        })
    }
    pub fn set_epoch(&mut self, epoch: EpochId) {
        if epoch != self.current_epoch {
            self.current_epoch = epoch;
            self.delay_used_this_epoch = 0;
            self.force_rerun_used_this_epoch = 0;
        }
    }

    pub fn set_members(
        &mut self,
        origin: GuardianOrigin,
        members: [AccountId; GUARDIAN_SEATS],
    ) -> Result<(), Error> {
        ensure!(
            matches!(origin, GuardianOrigin::ConstitutionalValues),
            Error::BadOrigin
        );
        validate_members(&members)?;
        self.members = members.map(Some);
        self.member_bonds = [GUARDIAN_BOND; GUARDIAN_SEATS];
        // Re-election starts the incoming council with a clean workflow: drop
        // every un-dispatched proposed action and its approvals so a recalled
        // member's live 5-of-7 approval can never carry over and help dispatch
        // under the new council (a stale-approval bypass of the threshold —
        // 06 §5.1/§5.4). Accountability state (reviews), emergency state (active
        // playbooks) and the per-proposal rerun ledger deliberately survive.
        self.pending.retain(|action| action.dispatched);
        self.approvals.clear();
        self.events.push(Event::MembersSet { members });
        Ok(())
    }

    /// Apply a values-approved recall to the still-seated approvers of a
    /// failed action. Seats become vacant and the complete live approval
    /// workflow is cleared, matching the conservative `set_members` cleanup.
    /// Bond release timing and fungible custody are owned by the FRAME shell.
    pub fn recall_members(
        &mut self,
        origin: GuardianOrigin,
        approvers: &[AccountId],
    ) -> Result<(), Error> {
        ensure!(
            matches!(origin, GuardianOrigin::ConstitutionalValues),
            Error::BadOrigin
        );
        for (member, bond) in self.members.iter_mut().zip(self.member_bonds.iter_mut()) {
            if member.is_some_and(|who| approvers.contains(&who)) {
                *member = None;
                *bond = 0;
            }
        }
        self.pending.retain(|action| action.dispatched);
        self.approvals.clear();
        Ok(())
    }
    pub fn propose_action(
        &mut self,
        who: AccountId,
        power: GuardianPower,
        justification_hash: H256,
        now: BlockNumber,
    ) -> Result<ActionId, Error> {
        self.ensure_member(who)?;
        ensure!(self.pending.len() < 64, Error::TooManyPending);
        let id = self.next_action_id;
        self.next_action_id = self.next_action_id.checked_add(1).ok_or(Error::Overflow)?;
        self.pending.push(PendingAction {
            id,
            proposer: who,
            power,
            justification_hash,
            created_at: now,
            expires_at: now
                .checked_add(ACTION_EXPIRY_BLOCKS)
                .ok_or(Error::Overflow)?,
            dispatched: false,
        });
        self.approvals.push((id, who));
        self.events.push(Event::ActionProposed {
            action_id: id,
            power,
        });
        Ok(id)
    }
    pub fn approve_action(
        &mut self,
        who: AccountId,
        id: ActionId,
        now: BlockNumber,
        ctx: DispatchContext,
    ) -> Result<bool, Error> {
        self.ensure_member(who)?;
        let idx = self
            .pending
            .iter()
            .position(|a| a.id == id)
            .ok_or(Error::ActionNotFound)?;
        let action = self.pending[idx];
        ensure!(now <= action.expires_at, Error::ActionExpired);
        ensure!(!action.dispatched, Error::AlreadyDispatched);
        ensure!(
            !self.approvals.iter().any(|(a, m)| *a == id && *m == who),
            Error::DuplicateApproval
        );
        self.approvals.push((id, who));
        let approvals = self.approval_count(id);
        let dispatched = if approvals >= GUARDIAN_THRESHOLD {
            if let Err(err) = self.dispatch(idx, now, ctx) {
                self.approvals
                    .retain(|(action_id, member)| !(*action_id == id && *member == who));
                return Err(err);
            }
            true
        } else {
            false
        };
        self.events.push(Event::ActionApproved {
            action_id: id,
            who,
            approvals,
        });
        Ok(dispatched)
    }
    pub fn ratify_action(
        &mut self,
        origin: GuardianOrigin,
        action_id: ActionId,
    ) -> Result<(), Error> {
        ensure!(
            matches!(origin, GuardianOrigin::ConstitutionalValues),
            Error::BadOrigin
        );
        let review = self
            .reviews
            .iter_mut()
            .find(|r| r.action_id == action_id)
            .ok_or(Error::ReviewNotFound)?;
        ensure!(!review.ratified, Error::AlreadyRatified);
        review.ratified = true;
        self.events
            .push(Event::ActionRatified { action: action_id });
        Ok(())
    }
    pub fn mark_review_failed(
        &mut self,
        action_id: ActionId,
        epoch: EpochId,
        actual_slashes: [Balance; GUARDIAN_SEATS],
    ) -> Result<(), Error> {
        let review = self
            .reviews
            .iter_mut()
            .find(|review| review.action_id == action_id)
            .ok_or(Error::ReviewNotFound)?;
        ensure!(
            !review.ratified && !review.recall_scheduled && epoch >= review.deadline_epoch,
            Error::ReviewNotFound
        );
        let approvers = review.approvers;
        let approver_count = review.approver_count;
        review.recall_scheduled = true;
        for (position, approver) in approvers.iter().take(approver_count as usize).enumerate() {
            if let Some(index) = self
                .members
                .iter()
                .position(|member| member.as_ref() == Some(approver))
            {
                self.member_bonds[index] =
                    self.member_bonds[index].saturating_sub(actual_slashes[position]);
            }
        }
        Ok(())
    }

    pub fn enforce_reviews(&mut self, epoch: EpochId) -> Result<(), Error> {
        let due = self
            .reviews
            .iter()
            .filter(|review| {
                !review.ratified && !review.recall_scheduled && epoch >= review.deadline_epoch
            })
            .map(|review| review.action_id)
            .collect::<Vec<_>>();
        let slash = GUARDIAN_BOND.saturating_mul(REVIEW_SLASH_PERCENT as Balance) / 100;
        for action in due {
            self.mark_review_failed(action, epoch, [slash; GUARDIAN_SEATS])?;
            self.events.push(Event::ReviewFailed {
                action,
                slashed_each: slash,
            });
        }
        Ok(())
    }
    pub fn renew_playbook(
        &mut self,
        origin: GuardianOrigin,
        id: PlaybookId,
        now: BlockNumber,
    ) -> Result<(), Error> {
        ensure!(
            matches!(origin, GuardianOrigin::ConstitutionalValues),
            Error::BadOrigin
        );
        ensure!(
            matches!(id, PlaybookId::LedgerFreeze),
            Error::RenewalNotAllowed
        );
        let active = self
            .active_playbooks
            .iter_mut()
            .find(|p| p.id == id)
            .ok_or(Error::RenewalNotAllowed)?;
        ensure!(
            active.renewals_used < LEDGER_FREEZE_RENEWALS,
            Error::RenewalNotAllowed
        );
        active.renewals_used = active.renewals_used.saturating_add(1);
        active.expiry = now.checked_add(HOLD_MAX_BLOCKS).ok_or(Error::Overflow)?;
        self.events.push(Event::PlaybookRenewed { id });
        Ok(())
    }
    pub fn expire_playbooks(&mut self, now: BlockNumber) {
        let mut kept = Vec::new();
        for p in self.active_playbooks.drain(..) {
            if now >= p.expiry {
                self.events.push(Event::PlaybookExpired { id: p.id });
            } else {
                kept.push(p);
            }
        }
        self.active_playbooks = kept;
    }
    /// Reap terminal workflow records so the live-slot caps are **concurrent**,
    /// not lifetime (06 §5.1: proposals expire un-dispatched after 3 days;
    /// §5.4: reviews close on ratification or recall). Dispatched or expired
    /// un-dispatched actions (and their approvals) drop; ratified or
    /// recall-scheduled reviews drop. Without this the 64/128 caps brick the
    /// council after 64 lifetime proposals / 128 lifetime actions (A10
    /// spec-reviewer majors). Driven by the maintenance crank each block.
    pub fn reap_terminal(&mut self, now: BlockNumber) {
        let mut reaped: Vec<ActionId> = Vec::new();
        self.pending.retain(|a| {
            let review_live = self
                .reviews
                .iter()
                .any(|r| r.action_id == a.id && !r.ratified && !r.recall_scheduled);
            let terminal = (a.dispatched && !review_live) || (!a.dispatched && now > a.expires_at);
            if terminal {
                reaped.push(a.id);
            }
            !terminal
        });
        if !reaped.is_empty() {
            self.approvals.retain(|(id, _)| !reaped.contains(id));
        }
        self.reviews.retain(|r| !(r.ratified || r.recall_scheduled));
    }
    pub fn try_state(&self) -> Result<(), Error> {
        validate_seats(&self.members)?;
        ensure!(self.pending.len() <= 64, Error::TooManyPending);
        ensure!(self.reviews.len() <= 128, Error::TooManyReviews);
        ensure!(
            self.active_playbooks.len() <= 6,
            Error::TooManyActivePlaybooks
        );
        // Playbooks are singletons (06 §6.2): no two active records share an id.
        for i in 0..self.active_playbooks.len() {
            for j in (i + 1)..self.active_playbooks.len() {
                ensure!(
                    self.active_playbooks[i].id != self.active_playbooks[j].id,
                    Error::PlaybookAlreadyActive
                );
            }
        }
        for (id, who) in &self.approvals {
            ensure!(
                self.pending.iter().any(|a| a.id == *id),
                Error::ActionNotFound
            );
            self.ensure_member(*who)?;
        }
        Ok(())
    }
    fn dispatch(
        &mut self,
        idx: usize,
        now: BlockNumber,
        ctx: DispatchContext,
    ) -> Result<(), Error> {
        ensure!(self.reviews.len() < 128, Error::TooManyReviews);
        let action = self.pending[idx];
        if let GuardianPower::ActivatePlaybook { id, .. } = action.power {
            // Playbooks are enumerated singletons (06 §6.2): at most one active
            // record per id. If one is already active, LedgerFreeze refuses
            // re-activation (it renews only via a values referendum, §6.3) while
            // every other playbook re-activates in place ("renew by re-activation
            // while triggered") without consuming a new slot — so duplicate
            // non-ledger activations can no longer exhaust the six slots and
            // block PB-LEDGER-FREEZE (A10 Codex adversarial finding).
            if self.active_playbooks.iter().any(|p| p.id == id) {
                ensure!(
                    !matches!(id, PlaybookId::LedgerFreeze),
                    Error::PlaybookAlreadyActive
                );
            } else {
                ensure!(
                    self.active_playbooks.len() < 6,
                    Error::TooManyActivePlaybooks
                );
            }
        }
        self.check_and_consume(action.power, now, ctx)?;
        self.pending[idx].dispatched = true;
        let target = target_of(action.power);
        self.events.push(Event::GuardianAction {
            action_id: action.id,
            power: action.power,
            target,
            justification_hash: action.justification_hash,
        });
        if let GuardianPower::ForceRerun { pid } = action.power {
            self.events.push(Event::ForceRerun {
                pid,
                justification_hash: action.justification_hash,
                window_end: now
                    .checked_add(FORCE_RERUN_WINDOW_BLOCKS)
                    .ok_or(Error::Overflow)?,
            });
        }
        if let GuardianPower::ActivatePlaybook {
            id,
            trigger,
            expiry,
            ..
        } = action.power
        {
            // Upsert by id: re-activation renews the existing record's expiry
            // (06 §6.2) rather than appending a duplicate.
            if let Some(existing) = self.active_playbooks.iter_mut().find(|p| p.id == id) {
                existing.expiry = expiry;
            } else {
                self.active_playbooks.push(ActivePlaybook {
                    id,
                    expiry,
                    renewals_used: 0,
                });
            }
            self.events.push(Event::PlaybookActivated {
                id,
                trigger,
                expiry,
            });
        }
        let (approvers, approver_count) = self.approver_snapshot(action.id);
        self.reviews.push(ReviewRecord {
            action_id: action.id,
            deadline_epoch: self.current_epoch.saturating_add(REVIEW_DEADLINE_EPOCHS),
            ratified: false,
            recall_scheduled: false,
            approvers,
            approver_count,
        });
        self.events
            .push(Event::ReviewScheduled { action: action.id });
        Ok(())
    }
    fn check_and_consume(
        &mut self,
        power: GuardianPower,
        now: BlockNumber,
        ctx: DispatchContext,
    ) -> Result<(), Error> {
        match power {
            GuardianPower::PauseIntake { until } => {
                ensure!(
                    until >= now && until <= now.saturating_add(HOLD_MAX_BLOCKS),
                    Error::DurationTooLong
                );
                if self
                    .current_epoch
                    .saturating_sub(self.pause_used_epoch_window_start)
                    >= PAUSE_INTAKE_ALLOWANCE_WINDOW_EPOCHS
                {
                    self.pause_used_epoch_window_start = self.current_epoch;
                    self.pause_used_in_window = 0;
                }
                ensure!(
                    self.pause_used_in_window < PAUSE_INTAKE_ALLOWANCE,
                    Error::AllowanceExhausted
                );
                self.pause_used_in_window = self.pause_used_in_window.saturating_add(1);
            }
            GuardianPower::DelayOnce { pid } => {
                ensure!(
                    ctx.proposal_status == ProposalStatus::Queued,
                    Error::NotRerunnable
                );
                ensure!(
                    !ctx.in_rerun && !self.rerun_used.contains(&pid),
                    Error::AlreadyRerun
                );
                ensure!(
                    self.delay_used_this_epoch < DELAY_ONCE_ALLOWANCE_PER_EPOCH,
                    Error::AllowanceExhausted
                );
                self.delay_used_this_epoch = self.delay_used_this_epoch.saturating_add(1);
                self.rerun_used.push(pid);
            }
            GuardianPower::ForceRerun { pid } => {
                ensure!(ctx.proposal_status.rerunnable(), Error::NotRerunnable);
                ensure!(
                    !ctx.in_rerun && !self.rerun_used.contains(&pid),
                    Error::AlreadyRerun
                );
                ensure!(
                    self.force_rerun_used_this_epoch < FORCE_RERUN_ALLOWANCE_PER_EPOCH,
                    Error::AllowanceExhausted
                );
                self.force_rerun_used_this_epoch =
                    self.force_rerun_used_this_epoch.saturating_add(1);
                self.rerun_used.push(pid);
            }
            GuardianPower::ActivatePlaybook {
                id,
                trigger,
                expiry,
                target,
            } => {
                ensure!(
                    expiry >= now && expiry <= now.saturating_add(HOLD_MAX_BLOCKS),
                    Error::DurationTooLong
                );
                ensure!(trigger_matches(id, trigger), Error::BadPlaybookTrigger);
                ensure!(
                    matches!((id, target), (PlaybookId::OracleVoid, Some(_)))
                        || (!matches!(id, PlaybookId::OracleVoid) && target.is_none()),
                    Error::BadPlaybookTarget
                );
                ensure!(ctx.triggers.is_active(trigger), Error::TriggerInactive);
            }
            GuardianPower::SuspendOnGate => {
                ensure!(ctx.triggers.gate_breach, Error::TriggerInactive)
            }
        }
        Ok(())
    }
    fn approval_count(&self, id: ActionId) -> u8 {
        self.approvals.iter().filter(|(a, _)| *a == id).count() as u8
    }

    fn approver_snapshot(&self, id: ActionId) -> ([AccountId; GUARDIAN_SEATS], u8) {
        let mut approvers = [[0u8; 32]; GUARDIAN_SEATS];
        let mut count = 0usize;
        for (_, member) in self
            .approvals
            .iter()
            .filter(|(action_id, _)| *action_id == id)
        {
            if count < GUARDIAN_SEATS {
                approvers[count] = *member;
                count += 1;
            }
        }
        (approvers, count as u8)
    }
    fn ensure_member(&self, who: AccountId) -> Result<(), Error> {
        ensure!(self.members.contains(&Some(who)), Error::NotMember);
        Ok(())
    }
}

#[derive(
    Clone,
    Copy,
    Debug,
    Decode,
    DecodeWithMemTracking,
    Encode,
    MaxEncodedLen,
    PartialEq,
    Eq,
    TypeInfo,
)]
pub struct DispatchContext {
    pub proposal_status: ProposalStatus,
    pub in_rerun: bool,
    pub triggers: TriggerState,
}

impl TriggerState {
    pub const fn none() -> Self {
        Self {
            depeg: false,
            migration_halt: false,
            oracle_deadlock: false,
            gate_breach: false,
            dead_man: false,
            void_in_flight: false,
            reserve_health: false,
            ledger_drift: false,
        }
    }
    pub const fn is_active(self, trigger: PlaybookTrigger) -> bool {
        match trigger {
            PlaybookTrigger::DepegMedian => self.depeg,
            PlaybookTrigger::MigrationHalt => self.migration_halt,
            PlaybookTrigger::OracleDeadlock => self.oracle_deadlock,
            PlaybookTrigger::GateBreach => self.gate_breach,
            PlaybookTrigger::DeadMan => self.dead_man,
            PlaybookTrigger::VoidInFlight => self.void_in_flight,
            PlaybookTrigger::ReserveHealth => self.reserve_health,
            PlaybookTrigger::LedgerDrift => self.ledger_drift,
        }
    }
}

fn validate_members(members: &[AccountId; GUARDIAN_SEATS]) -> Result<(), Error> {
    for i in 0..GUARDIAN_SEATS {
        for j in (i + 1)..GUARDIAN_SEATS {
            ensure!(members[i] != members[j], Error::DuplicateMember);
        }
    }
    Ok(())
}

fn validate_seats(members: &[Option<AccountId>; GUARDIAN_SEATS]) -> Result<(), Error> {
    for i in 0..GUARDIAN_SEATS {
        for j in (i + 1)..GUARDIAN_SEATS {
            if members[i].is_some() {
                ensure!(members[i] != members[j], Error::DuplicateMember);
            }
        }
    }
    Ok(())
}
fn target_of(power: GuardianPower) -> ActionTarget {
    match power {
        GuardianPower::DelayOnce { pid } | GuardianPower::ForceRerun { pid } => ActionTarget {
            pid: Some(pid),
            playbook: None,
        },
        GuardianPower::ActivatePlaybook { id, .. } => ActionTarget {
            pid: None,
            playbook: Some(id),
        },
        _ => ActionTarget {
            pid: None,
            playbook: None,
        },
    }
}
fn trigger_matches(id: PlaybookId, trigger: PlaybookTrigger) -> bool {
    matches!(
        (id, trigger),
        (PlaybookId::Depeg, PlaybookTrigger::DepegMedian)
            | (PlaybookId::Migration, PlaybookTrigger::MigrationHalt)
            | (PlaybookId::OracleVoid, PlaybookTrigger::OracleDeadlock)
            | (
                PlaybookId::HaltIntake,
                PlaybookTrigger::GateBreach
                    | PlaybookTrigger::DeadMan
                    | PlaybookTrigger::VoidInFlight
            )
            | (PlaybookId::Reserve, PlaybookTrigger::ReserveHealth)
            | (PlaybookId::LedgerFreeze, PlaybookTrigger::LedgerDrift)
    )
}

#[macro_export]
macro_rules! ensure {
    ($cond:expr, $err:expr $(,)?) => {
        if !$cond {
            return Err($err);
        }
    };
}

#[cfg(feature = "runtime-benchmarks")]
pub mod benchmarking {
    pub fn benchmarks_compile() -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn acct(n: u8) -> AccountId {
        [n; 32]
    }
    fn members() -> [AccountId; GUARDIAN_SEATS] {
        [
            acct(1),
            acct(2),
            acct(3),
            acct(4),
            acct(5),
            acct(6),
            acct(7),
        ]
    }
    fn ctx() -> DispatchContext {
        DispatchContext {
            proposal_status: ProposalStatus::Queued,
            in_rerun: false,
            triggers: TriggerState {
                ledger_drift: true,
                gate_breach: true,
                ..TriggerState::none()
            },
        }
    }
    #[test]
    fn membership_is_values_origin_and_unique() {
        let mut g = Guardian::new(members()).unwrap();
        assert_eq!(
            g.set_members(GuardianOrigin::Signed, members()),
            Err(Error::BadOrigin)
        );
        let mut dup = members();
        dup[1] = dup[0];
        assert_eq!(Guardian::new(dup), Err(Error::DuplicateMember));
    }
    #[test]
    fn five_of_seven_dispatches_and_schedules_review() {
        let mut g = Guardian::new(members()).unwrap();
        let id = g
            .propose_action(acct(1), GuardianPower::ForceRerun { pid: 42 }, [9; 32], 0)
            .unwrap();
        for n in 2..=5 {
            g.approve_action(acct(n), id, 1, ctx()).unwrap();
        }
        assert!(g.pending[0].dispatched);
        assert_eq!(g.reviews.len(), 1);
        assert!(g
            .events
            .iter()
            .any(|e| matches!(e, Event::ForceRerun { pid: 42, .. })));
    }
    #[test]
    fn recalled_vacancies_keep_the_threshold_at_absolute_five() {
        let mut g = Guardian::new(members()).unwrap();
        g.recall_members(
            GuardianOrigin::ConstitutionalValues,
            &[acct(1), acct(2), acct(3)],
        )
        .unwrap();
        assert_eq!(
            g.members.iter().filter(|member| member.is_some()).count(),
            4
        );
        assert_eq!(
            g.propose_action(acct(1), GuardianPower::SuspendOnGate, [0; 32], 0),
            Err(Error::NotMember)
        );
        let id = g
            .propose_action(acct(4), GuardianPower::SuspendOnGate, [0; 32], 0)
            .unwrap();
        for member in [acct(5), acct(6), acct(7)] {
            assert!(!g.approve_action(member, id, 1, ctx()).unwrap());
        }
        assert!(!g.pending[0].dispatched);
        assert_eq!(g.approval_count(id), 4);
    }
    #[test]
    fn force_rerun_requires_pre_execution_and_once() {
        let mut g = Guardian::new(members()).unwrap();
        let id = g
            .propose_action(acct(1), GuardianPower::ForceRerun { pid: 7 }, [0; 32], 0)
            .unwrap();
        let bad = DispatchContext {
            proposal_status: ProposalStatus::Executed,
            ..ctx()
        };
        for n in 2..=4 {
            g.approve_action(acct(n), id, 1, bad).unwrap();
        }
        assert_eq!(
            g.approve_action(acct(5), id, 1, bad),
            Err(Error::NotRerunnable)
        );
        let id2 = g
            .propose_action(acct(1), GuardianPower::ForceRerun { pid: 7 }, [0; 32], 2)
            .unwrap();
        for n in 2..=5 {
            let _ = g.approve_action(acct(n), id2, 3, ctx());
        }
        assert!(g.rerun_used.contains(&7));
    }
    #[test]
    fn delay_once_requires_queued_target_and_allowances_reset_by_epoch() {
        let mut g = Guardian::new(members()).unwrap();
        let id = g
            .propose_action(acct(1), GuardianPower::DelayOnce { pid: 1 }, [0; 32], 0)
            .unwrap();
        let bad = DispatchContext {
            proposal_status: ProposalStatus::Trading,
            ..ctx()
        };
        for n in 2..=4 {
            g.approve_action(acct(n), id, 1, bad).unwrap();
        }
        assert_eq!(
            g.approve_action(acct(5), id, 1, bad),
            Err(Error::NotRerunnable)
        );

        for pid in 2..=3 {
            let id = g
                .propose_action(
                    acct(1),
                    GuardianPower::DelayOnce { pid },
                    [0; 32],
                    pid as u32,
                )
                .unwrap();
            for n in 2..=5 {
                g.approve_action(acct(n), id, pid as u32, ctx()).unwrap();
            }
        }
        let id = g
            .propose_action(acct(1), GuardianPower::DelayOnce { pid: 4 }, [0; 32], 4)
            .unwrap();
        for n in 2..=4 {
            g.approve_action(acct(n), id, 4, ctx()).unwrap();
        }
        assert_eq!(
            g.approve_action(acct(5), id, 4, ctx()),
            Err(Error::AllowanceExhausted)
        );
        g.set_epoch(1);
        g.approve_action(acct(5), id, 5, ctx()).unwrap();
    }

    #[test]
    fn failed_review_capacity_does_not_mutate_dispatch_state() {
        let mut g = Guardian::new(members()).unwrap();
        for action_id in 0..128 {
            g.reviews.push(ReviewRecord {
                action_id,
                deadline_epoch: 0,
                ratified: false,
                recall_scheduled: false,
                approvers: [[0; 32]; GUARDIAN_SEATS],
                approver_count: 0,
            });
        }
        let id = g
            .propose_action(acct(1), GuardianPower::ForceRerun { pid: 55 }, [0; 32], 0)
            .unwrap();
        for n in 2..=4 {
            g.approve_action(acct(n), id, 1, ctx()).unwrap();
        }
        assert_eq!(
            g.approve_action(acct(5), id, 1, ctx()),
            Err(Error::TooManyReviews)
        );
        assert!(!g.pending[0].dispatched);
        assert!(!g.rerun_used.contains(&55));
        assert_eq!(g.force_rerun_used_this_epoch, 0);
    }

    #[test]
    fn ledger_freeze_requires_drift_and_one_renewal() {
        let mut g = Guardian::new(members()).unwrap();
        let id = g
            .propose_action(
                acct(1),
                GuardianPower::ActivatePlaybook {
                    id: PlaybookId::LedgerFreeze,
                    trigger: PlaybookTrigger::LedgerDrift,
                    expiry: 100,
                    target: None,
                },
                [1; 32],
                0,
            )
            .unwrap();
        let no_drift = DispatchContext {
            triggers: TriggerState::none(),
            ..ctx()
        };
        for n in 2..=4 {
            g.approve_action(acct(n), id, 1, no_drift).unwrap();
        }
        assert_eq!(
            g.approve_action(acct(5), id, 1, no_drift),
            Err(Error::TriggerInactive)
        );
        let id2 = g
            .propose_action(
                acct(1),
                GuardianPower::ActivatePlaybook {
                    id: PlaybookId::LedgerFreeze,
                    trigger: PlaybookTrigger::LedgerDrift,
                    expiry: 100,
                    target: None,
                },
                [1; 32],
                2,
            )
            .unwrap();
        for n in 2..=5 {
            g.approve_action(acct(n), id2, 3, ctx()).unwrap();
        }
        assert_eq!(
            g.renew_playbook(GuardianOrigin::Signed, PlaybookId::LedgerFreeze, 10),
            Err(Error::BadOrigin)
        );
        g.renew_playbook(
            GuardianOrigin::ConstitutionalValues,
            PlaybookId::LedgerFreeze,
            10,
        )
        .unwrap();
        assert_eq!(
            g.renew_playbook(
                GuardianOrigin::ConstitutionalValues,
                PlaybookId::LedgerFreeze,
                10
            ),
            Err(Error::RenewalNotAllowed)
        );
    }
    #[test]
    fn duplicate_active_ledger_freeze_is_rejected() {
        let mut g = Guardian::new(members()).unwrap();
        let first = g
            .propose_action(
                acct(1),
                GuardianPower::ActivatePlaybook {
                    id: PlaybookId::LedgerFreeze,
                    trigger: PlaybookTrigger::LedgerDrift,
                    expiry: 100,
                    target: None,
                },
                [1; 32],
                0,
            )
            .unwrap();
        for n in 2..=5 {
            g.approve_action(acct(n), first, 1, ctx()).unwrap();
        }
        let second = g
            .propose_action(
                acct(1),
                GuardianPower::ActivatePlaybook {
                    id: PlaybookId::LedgerFreeze,
                    trigger: PlaybookTrigger::LedgerDrift,
                    expiry: 200,
                    target: None,
                },
                [1; 32],
                2,
            )
            .unwrap();
        for n in 2..=4 {
            g.approve_action(acct(n), second, 3, ctx()).unwrap();
        }
        assert_eq!(
            g.approve_action(acct(5), second, 3, ctx()),
            Err(Error::PlaybookAlreadyActive)
        );
        assert_eq!(g.active_playbooks.len(), 1);
    }

    #[test]
    fn failed_dispatch_rolls_back_fifth_approval() {
        let mut g = Guardian::new(members()).unwrap();
        let id = g
            .propose_action(acct(1), GuardianPower::ForceRerun { pid: 7 }, [0; 32], 0)
            .unwrap();
        let bad = DispatchContext {
            proposal_status: ProposalStatus::Executed,
            ..ctx()
        };
        for n in 2..=4 {
            g.approve_action(acct(n), id, 1, bad).unwrap();
        }
        assert_eq!(
            g.approve_action(acct(5), id, 1, bad),
            Err(Error::NotRerunnable)
        );
        assert_eq!(g.approval_count(id), 4);
        g.approve_action(acct(5), id, 2, ctx()).unwrap();
        assert!(g.pending[0].dispatched);
    }

    #[test]
    fn review_failure_slashes_bonds_and_ratify_origin_checked() {
        let mut g = Guardian::new(members()).unwrap();
        let id = g
            .propose_action(acct(1), GuardianPower::SuspendOnGate, [2; 32], 0)
            .unwrap();
        for n in 2..=5 {
            g.approve_action(acct(n), id, 1, ctx()).unwrap();
        }
        assert_eq!(
            g.ratify_action(GuardianOrigin::Signed, id),
            Err(Error::BadOrigin)
        );
        g.enforce_reviews(3).unwrap();
        assert!(g.member_bonds[..5].iter().all(|b| *b == GUARDIAN_BOND / 2));
        assert!(g.member_bonds[5..].iter().all(|b| *b == GUARDIAN_BOND));
    }

    #[test]
    fn set_members_reelection_clears_pending_and_approvals() {
        let mut g = Guardian::new(members()).unwrap();
        g.propose_action(acct(1), GuardianPower::SuspendOnGate, [0; 32], 0)
            .unwrap();
        g.approve_action(acct(2), 0, 1, ctx()).unwrap();
        assert_eq!(g.pending.len(), 1);
        assert_eq!(g.approvals.len(), 2);
        let mut new_members = members();
        for (i, seat) in new_members.iter_mut().enumerate() {
            *seat = acct((i as u8) + 10);
        }
        g.set_members(GuardianOrigin::ConstitutionalValues, new_members)
            .unwrap();
        // No recalled member's live approval survives to help dispatch.
        assert!(g.pending.is_empty());
        assert!(g.approvals.is_empty());
        assert!(g.try_state().is_ok());
    }

    #[test]
    fn reap_terminal_drops_dispatched_expired_and_terminal_reviews() {
        let mut g = Guardian::new(members()).unwrap();
        // A dispatched action (5-of-7) with its scheduled review.
        g.propose_action(acct(1), GuardianPower::SuspendOnGate, [0; 32], 0)
            .unwrap();
        for n in 2..=5 {
            g.approve_action(acct(n), 0, 1, ctx()).unwrap();
        }
        assert!(g.pending[0].dispatched);
        assert_eq!(g.reviews.len(), 1);
        // A second, un-dispatched action that will expire.
        g.propose_action(acct(1), GuardianPower::SuspendOnGate, [0; 32], 0)
            .unwrap();
        assert_eq!(g.pending.len(), 2);
        // Ratify the first review so it becomes terminal.
        g.ratify_action(GuardianOrigin::ConstitutionalValues, 0)
            .unwrap();
        // Reap past the second action's expiry: dispatched + expired actions
        // (and their approvals) and the ratified review are reclaimed.
        g.reap_terminal(ACTION_EXPIRY_BLOCKS + 1);
        assert!(g.pending.is_empty());
        assert!(g.approvals.is_empty());
        assert!(g.reviews.is_empty());
        assert!(g.try_state().is_ok());
    }
}

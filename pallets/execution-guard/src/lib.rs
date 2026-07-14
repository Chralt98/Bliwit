#![cfg_attr(not(feature = "std"), no_std)]
#![deny(unsafe_code)]

extern crate alloc;

use alloc::vec::Vec;
use futarchy_primitives::{
    BlockNumber, DispatchOutcomeCode, ProposalClass, ProposalId, QueuedExecutionView,
    RatificationStatus, RejectReason, ResourceId, RuntimeVersionConstraint, H256,
};
use pallet_attestor::{AttestationId, AttestorRegistry};
use pallet_conditional_ledger::LedgerState;
use pallet_epoch::{EpochState, Origin as EpochOrigin};
use pallet_guardian::{Guardian, PlaybookId};
use pallet_origins::{Origin as ClassOrigin, RuntimeCall, SafetyFilter};
use parity_scale_codec::{Decode, Encode, MaxEncodedLen};
use scale_info::TypeInfo;

macro_rules! ensure {
    ($cond:expr, $err:expr $(,)?) => {
        if !$cond {
            return Err($err);
        }
    };
}

pub const MAX_QUEUE: usize = 32;
pub const MAX_EXECUTION_RECORDS: usize = 256;
pub const MAX_CALLS: usize = 16;
pub const MAX_PAYLOAD_BYTES: u32 = 65_536;
pub const MAX_DECLARED_DOMAINS: usize = 16;
pub const MAX_RESOURCE_LOCKS: usize = 8;
pub const DESCRIPTOR_LEAD_TIME: BlockNumber = 43_200;
/// T18 retry window (05 §2.1: **72 h**, `= 43,200` blocks per [13]). From the
/// block a payload reverts, `execute` may be retried (T23) until this elapses;
/// afterwards the proposal is measured executed-with-failure (T22).
pub const RETRY_WINDOW: BlockNumber = 43_200;

/// Deterministic content hash of a byte string — the model stand-in for the
/// on-chain preimage content-addressing of 09 §1.2(2)/§7.3 (same idiom as
/// pallet-oracle's `hash_evidence`). Binding the committed `payload_hash` to
/// this derivation is what stops a keeper swapping `payload.calls` under a
/// queued hash.
pub fn content_hash(bytes: &[u8]) -> H256 {
    let mut out = [0u8; 32];
    let len = (bytes.len() as u64).to_le_bytes();
    for (i, b) in len.iter().enumerate() {
        out[24 + i] ^= *b;
    }
    for (i, b) in bytes.iter().enumerate() {
        out[i % 24] ^= b.rotate_left((i / 24 % 8) as u32);
    }
    out
}

/// Content hash of a dispatch batch, re-derived from the SCALE encoding of the
/// exact calls presented at `execute` (09 §1.2(2), I-11). It binds the
/// market-committed identity of each call — its declared domain, size, weight,
/// the call itself, and any upgrade hash / target spec version — so none can be
/// altered while still matching the committed `payload_hash`. The `succeeds` /
/// `error` fields are deliberately excluded: they are the model's stand-in for
/// the *runtime dispatch outcome* (not part of the committed payload), so a
/// retry (T23) may present a now-succeeding dispatch of the same calls.
pub fn hash_payload(calls: &[DispatchCall]) -> H256 {
    let mut buf = Vec::new();
    for c in calls {
        c.domain.encode_to(&mut buf);
        c.encoded_len.encode_to(&mut buf);
        c.declared_weight.encode_to(&mut buf);
        c.call.encode_to(&mut buf);
        c.upgrade_hash.encode_to(&mut buf);
        c.target_spec_version.encode_to(&mut buf);
    }
    content_hash(&buf)
}

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub enum GuardOrigin {
    Signed,
    EpochDecision,
    RatifyTrack,
    System,
}

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub enum CallDomain {
    Public,
    Param,
    Treasury,
    Code,
    Meta,
    InternalRootAuthorizeUpgrade,
    InternalRootApplyUpgrade,
}

impl From<CallDomain> for pallet_origins::CallDomain {
    fn from(value: CallDomain) -> Self {
        match value {
            CallDomain::Public => Self::Public,
            CallDomain::Param => Self::Param,
            CallDomain::Treasury => Self::Treasury,
            CallDomain::Code => Self::Code,
            CallDomain::Meta => Self::Meta,
            CallDomain::InternalRootAuthorizeUpgrade | CallDomain::InternalRootApplyUpgrade => {
                Self::InternalRoot
            }
        }
    }
}

#[derive(Clone, Debug, Decode, Encode, Eq, PartialEq, TypeInfo)]
pub struct DispatchCall {
    pub domain: CallDomain,
    pub encoded_len: u32,
    pub declared_weight: u64,
    pub call: RuntimeCall,
    pub succeeds: bool,
    pub error: [u8; 4],
    pub upgrade_hash: Option<H256>,
    pub target_spec_version: Option<u32>,
}

#[derive(Clone, Debug, Decode, Encode, Eq, PartialEq, TypeInfo)]
pub struct Payload {
    pub hash: H256,
    pub calls: Vec<DispatchCall>,
}

#[derive(Clone, Debug, Decode, Encode, Eq, PartialEq, TypeInfo)]
pub struct QueuedExecution {
    pub pid: ProposalId,
    pub payload_hash: H256,
    pub payload_len: u32,
    pub class: ProposalClass,
    pub maturity: BlockNumber,
    pub grace_end: BlockNumber,
    pub version_constraint: RuntimeVersionConstraint,
    pub meters_declared: Vec<ResourceId>,
    pub ratify_ref: Option<u32>,
    pub ratification_passed: bool,
    pub attestation_id: Option<AttestationId>,
    pub pre_upgrade_checkpoint: Option<(H256, H256)>,
    pub cancelled: bool,
    pub declared_domains: Vec<CallDomain>,
    /// Set at T18 to the block the payload first reverted; `Some(f)` marks the
    /// entry as `FailedExecuted`, opening the `[f, f + RETRY_WINDOW]` retry
    /// window (T23) after which it is measured executed-with-failure (T22).
    pub failed_at: Option<BlockNumber>,
}

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub struct PendingUpgrade {
    pub hash: H256,
    pub authorized_at: BlockNumber,
    pub applicable_at: BlockNumber,
    pub target_spec_version: u32,
}

#[derive(Clone, Debug, Decode, Encode, Eq, PartialEq, TypeInfo)]
pub struct ExecutionRecord {
    pub pid: ProposalId,
    pub payload_hash: H256,
    pub class: ProposalClass,
    pub executed_at: BlockNumber,
    pub result: DispatchOutcomeCode,
}

#[derive(Clone, Debug, Decode, Encode, Eq, PartialEq, TypeInfo)]
pub enum Event {
    Enqueued {
        pid: ProposalId,
        maturity: BlockNumber,
    },
    Ratified {
        pid: ProposalId,
        referendum_index: u32,
    },
    Executed {
        pid: ProposalId,
        record: ExecutionRecord,
    },
    ExecutionFailed {
        pid: ProposalId,
        outcome: DispatchOutcomeCode,
    },
    Rejected {
        pid: ProposalId,
        reason: RejectReason,
    },
    UpgradeAuthorized {
        code_hash: H256,
        authorized_at: BlockNumber,
        applicable_at: BlockNumber,
    },
    UpgradeApplied {
        code_hash: H256,
        spec_version: u32,
    },
    PreimageUnpinned {
        pid: ProposalId,
        payload_hash: H256,
    },
}

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub enum Error {
    BadOrigin,
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
    RetryWindowOpen,
    Overflow,
}

#[derive(Clone, Debug, Decode, Encode, Eq, PartialEq, TypeInfo)]
pub struct ExecutionGuard {
    pub queue: Vec<QueuedExecution>,
    pub records: Vec<ExecutionRecord>,
    pub pending_upgrade: Option<PendingUpgrade>,
    pub current_spec_name: RuntimeVersionConstraint,
    pub held_resources: Vec<(ProposalId, ResourceId)>,
    pub blocked_meters: Vec<ResourceId>,
    pub hard_gate_breach: bool,
    pub dead_man_freeze: bool,
    pub migration_halt: bool,
    pub events: Vec<Event>,
}

impl ExecutionGuard {
    pub fn new(current_spec_name: RuntimeVersionConstraint) -> Self {
        Self {
            queue: Vec::new(),
            records: Vec::new(),
            pending_upgrade: None,
            current_spec_name,
            held_resources: Vec::new(),
            blocked_meters: Vec::new(),
            hard_gate_breach: false,
            dead_man_freeze: false,
            migration_halt: false,
            events: Vec::new(),
        }
    }

    pub fn enqueue(&mut self, origin: GuardOrigin, mut item: QueuedExecution) -> Result<(), Error> {
        ensure!(
            matches!(origin, GuardOrigin::EpochDecision),
            Error::BadOrigin
        );
        // The decision path enqueues fresh entries; the retry clock only opens at
        // T18 inside the guard, never at enqueue.
        item.failed_at = None;
        ensure!(self.queue.len() < MAX_QUEUE, Error::QueueFull);
        ensure!(
            item.meters_declared.len() <= MAX_RESOURCE_LOCKS,
            Error::TooManyLocks
        );
        ensure!(
            item.declared_domains.len() <= MAX_DECLARED_DOMAINS,
            Error::TooManyDomains
        );
        ensure!(
            !self.queue.iter().any(|q| q.pid == item.pid),
            Error::QueueFull
        );
        self.events.push(Event::Enqueued {
            pid: item.pid,
            maturity: item.maturity,
        });
        self.queue.push(item);
        Ok(())
    }

    pub fn ratify(
        &mut self,
        origin: GuardOrigin,
        pid: ProposalId,
        referendum_index: u32,
    ) -> Result<(), Error> {
        ensure!(matches!(origin, GuardOrigin::RatifyTrack), Error::BadOrigin);
        let q = self
            .queue
            .iter_mut()
            .find(|q| q.pid == pid)
            .ok_or(Error::NotFound)?;
        ensure!(q.ratify_ref == Some(referendum_index), Error::NotRatified);
        q.ratification_passed = true;
        self.events.push(Event::Ratified {
            pid,
            referendum_index,
        });
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn execute<AccountId: Clone + Eq>(
        &mut self,
        origin: GuardOrigin,
        epoch: &mut EpochState<AccountId>,
        ledger: &mut LedgerState<AccountId>,
        guardian: &Guardian,
        attestors: &AttestorRegistry,
        pid: ProposalId,
        payload: Payload,
        now: BlockNumber,
        block_hash: H256,
        state_root: H256,
    ) -> Result<(), Error> {
        ensure!(matches!(origin, GuardOrigin::Signed), Error::BadOrigin);
        let idx = self
            .queue
            .iter()
            .position(|q| q.pid == pid)
            .ok_or(Error::NotFound)?;
        let q = self.queue[idx].clone();
        self.check_dispatch_time(&q, guardian, attestors, &payload, now)?;
        let outcome = self.dispatch_batch(&q, &payload, now, block_hash, state_root)?;
        match outcome {
            DispatchOutcomeCode::Ok => {
                // T14 (first success) or T23 (retry within window): `mark_executed`
                // accepts both `Queued` and `FailedExecuted`.
                let record = ExecutionRecord {
                    pid,
                    payload_hash: q.payload_hash,
                    class: q.class,
                    executed_at: now,
                    result: outcome,
                };
                self.push_record(record.clone());
                self.queue.remove(idx);
                epoch
                    .mark_executed(EpochOrigin::ExecutionGuard, ledger, pid)
                    .map_err(|_| Error::DispatchFailed)?;
                self.events.push(Event::PreimageUnpinned {
                    pid,
                    payload_hash: q.payload_hash,
                });
                self.events.push(Event::Executed { pid, record });
                Ok(())
            }
            DispatchOutcomeCode::Failed { .. } => {
                // T18: the payload atomically reverted. Record the failure
                // (`PayloadReverted`), advance the epoch proposal to
                // `FailedExecuted` on the first revert, and keep the queue entry
                // live so a retry (T23) can run within `RETRY_WINDOW`. We return
                // `Ok` so the T18 transition and record persist — returning an
                // error here would roll them back and re-strand the proposal.
                let record = ExecutionRecord {
                    pid,
                    payload_hash: q.payload_hash,
                    class: q.class,
                    executed_at: now,
                    result: outcome,
                };
                self.push_record(record);
                if self.queue[idx].failed_at.is_none() {
                    epoch
                        .mark_failed_executed(EpochOrigin::ExecutionGuard, pid)
                        .map_err(|_| Error::DispatchFailed)?;
                    self.queue[idx].failed_at = Some(now);
                }
                self.events.push(Event::ExecutionFailed { pid, outcome });
                Ok(())
            }
        }
    }

    /// T22 (05 §2.1): once the 72 h retry window opened at T18 is exhausted, a
    /// keeper drives the `FailedExecuted → Measuring` transition (measured as
    /// executed-with-failure) and the queue entry is dropped.
    pub fn expire_failed_execution<AccountId: Clone + Eq>(
        &mut self,
        origin: GuardOrigin,
        epoch: &mut EpochState<AccountId>,
        ledger: &mut LedgerState<AccountId>,
        pid: ProposalId,
        now: BlockNumber,
    ) -> Result<(), Error> {
        ensure!(matches!(origin, GuardOrigin::Signed), Error::BadOrigin);
        let idx = self
            .queue
            .iter()
            .position(|q| q.pid == pid)
            .ok_or(Error::NotFound)?;
        let failed_at = self.queue[idx].failed_at.ok_or(Error::NotFound)?;
        let payload_hash = self.queue[idx].payload_hash;
        ensure!(
            now > failed_at.saturating_add(RETRY_WINDOW),
            Error::RetryWindowOpen
        );
        epoch
            .retry_exhausted_to_measurement(EpochOrigin::ExecutionGuard, ledger, pid)
            .map_err(|_| Error::DispatchFailed)?;
        self.queue.remove(idx);
        self.events
            .push(Event::PreimageUnpinned { pid, payload_hash });
        Ok(())
    }

    /// 09 §2.1(4)/(6), §2.2: permissionless application of an authorized upgrade.
    /// The caller MUST submit the actual artifact bytes `code`; the guard
    /// re-derives the hash from them and matches it against the committed
    /// `PendingUpgrade.hash` — it never trusts a caller-supplied hash (the
    /// authorized hash is public once `UpgradeAuthorized` fires). The recorded
    /// spec version is the *authorized* `target_spec_version`, not a caller
    /// claim, and any divergent `observed_spec_version` is rejected.
    pub fn apply_authorized_upgrade(
        &mut self,
        origin: GuardOrigin,
        code: &[u8],
        observed_spec_version: u32,
        now: BlockNumber,
    ) -> Result<(), Error> {
        ensure!(matches!(origin, GuardOrigin::Signed), Error::BadOrigin);
        let pending = self.pending_upgrade.ok_or(Error::NoPendingUpgrade)?;
        ensure!(now >= pending.applicable_at, Error::DescriptorLeadTime);
        let code_hash = content_hash(code);
        ensure!(pending.hash == code_hash, Error::UpgradeHashMismatch);
        ensure!(
            observed_spec_version == pending.target_spec_version,
            Error::UpgradeVersionMismatch
        );
        self.pending_upgrade = None;
        self.current_spec_name.spec_version = pending.target_spec_version;
        self.events.push(Event::UpgradeApplied {
            code_hash,
            spec_version: pending.target_spec_version,
        });
        Ok(())
    }

    fn check_dispatch_time(
        &self,
        q: &QueuedExecution,
        guardian: &Guardian,
        attestors: &AttestorRegistry,
        payload: &Payload,
        now: BlockNumber,
    ) -> Result<(), Error> {
        ensure!(!q.cancelled, Error::Cancelled);
        ensure!(q.maturity <= now, Error::NotMature);
        // Grace bounds the first attempt (T14); once T18 has opened the retry
        // window a retry (T23) is bounded by `failed_at + RETRY_WINDOW` instead.
        match q.failed_at {
            None => ensure!(now <= q.grace_end, Error::GraceExpired),
            Some(f) => ensure!(now <= f.saturating_add(RETRY_WINDOW), Error::GraceExpired),
        }
        // 09 §1.2(2): bind execution to the committed preimage by re-deriving the
        // hash from the SCALE-encoded calls actually presented, not from the
        // caller-supplied `payload.hash`. A keeper cannot swap `payload.calls`
        // under the queued hash.
        ensure!(
            hash_payload(&payload.calls) == q.payload_hash && q.payload_len <= MAX_PAYLOAD_BYTES,
            Error::BadPreimage
        );
        ensure!(
            q.version_constraint == self.current_spec_name,
            Error::StaleQueue
        );
        if requires_ratification(q.class) {
            ensure!(q.ratification_passed, Error::NotRatified);
        }
        if matches!(q.class, ProposalClass::Code | ProposalClass::Meta) {
            let id = q.attestation_id.ok_or(Error::AttestationMissing)?;
            ensure!(
                attestors.attestations.iter().any(|a| a.id == id),
                Error::AttestationMissing
            );
            ensure!(
                attestors.has_quorum(q.pid, q.payload_hash, now),
                Error::AttestationMissing
            );
        }
        ensure!(
            q.meters_declared
                .iter()
                .all(|m| !self.blocked_meters.contains(m)),
            Error::MetersBlocked
        );
        ensure!(
            q.meters_declared
                .iter()
                .all(|r| self.held_resources.contains(&(q.pid, *r))),
            Error::ResourceLockMissing
        );
        ensure!(!guardian.rerun_used.contains(&q.pid), Error::GuardianHold);
        ensure!(
            !guardian
                .active_playbooks
                .iter()
                .any(|p| matches!(p.id, PlaybookId::LedgerFreeze) && now <= p.expiry),
            Error::FreezeActive
        );
        ensure!(
            !self.hard_gate_breach && !self.dead_man_freeze && !self.migration_halt,
            Error::FreezeActive
        );
        ensure!(payload.calls.len() <= MAX_CALLS, Error::TooManyCalls);
        let total_len: u32 = payload.calls.iter().try_fold(0u32, |acc, c| {
            acc.checked_add(c.encoded_len).ok_or(Error::Overflow)
        })?;
        ensure!(
            total_len == q.payload_len && total_len <= MAX_PAYLOAD_BYTES,
            Error::PayloadTooLarge
        );
        let class_origin =
            ClassOrigin::from_proposal_class(q.class).ok_or(Error::CapabilityDenied)?;
        for c in &payload.calls {
            ensure!(
                q.declared_domains.contains(&c.domain),
                Error::BadDomainDeclaration
            );
            ensure!(domain_allowed(q.class, c.domain), Error::CapabilityDenied);
            SafetyFilter::validate(Some(class_origin), &c.call).map_err(|_| Error::SafetyFilter)?;
        }
        Ok(())
    }

    fn dispatch_batch(
        &mut self,
        q: &QueuedExecution,
        payload: &Payload,
        now: BlockNumber,
        block_hash: H256,
        state_root: H256,
    ) -> Result<DispatchOutcomeCode, Error> {
        for (i, call) in payload.calls.iter().enumerate() {
            if !call.succeeds {
                return Ok(DispatchOutcomeCode::Failed {
                    call_index: i as u8,
                    error: call.error,
                });
            }
        }
        if let Some(upgrade) = payload
            .calls
            .iter()
            .find(|c| matches!(c.domain, CallDomain::InternalRootAuthorizeUpgrade))
        {
            ensure!(
                matches!(q.class, ProposalClass::Code | ProposalClass::Meta),
                Error::BadUpgradePayload
            );
            ensure!(self.pending_upgrade.is_none(), Error::PendingUpgradeExists);
            let hash = upgrade.upgrade_hash.ok_or(Error::BadUpgradePayload)?;
            let target = upgrade
                .target_spec_version
                .ok_or(Error::BadUpgradePayload)?;
            let applicable_at = now
                .checked_add(DESCRIPTOR_LEAD_TIME)
                .ok_or(Error::Overflow)?;
            self.pending_upgrade = Some(PendingUpgrade {
                hash,
                authorized_at: now,
                applicable_at,
                target_spec_version: target,
            });
            if let Some(queued) = self.queue.iter_mut().find(|x| x.pid == q.pid) {
                queued.pre_upgrade_checkpoint = Some((block_hash, state_root));
            }
            self.events.push(Event::UpgradeAuthorized {
                code_hash: hash,
                authorized_at: now,
                applicable_at,
            });
        }
        Ok(DispatchOutcomeCode::Ok)
    }

    fn push_record(&mut self, record: ExecutionRecord) {
        if self.records.len() == MAX_EXECUTION_RECORDS {
            self.records.remove(0);
        }
        self.records.push(record);
    }

    pub fn reject_stale_or_unratified<AccountId: Clone + Eq>(
        &mut self,
        epoch: &mut EpochState<AccountId>,
        ledger: &mut LedgerState<AccountId>,
        pid: ProposalId,
        reason: RejectReason,
    ) -> Result<(), Error> {
        ensure!(
            matches!(
                reason,
                RejectReason::StaleQueue
                    | RejectReason::NotRatified
                    | RejectReason::AttestationMissing
            ),
            Error::StaleQueue
        );
        // Drive the epoch transition first and propagate its result: the epoch
        // now handles all three reasons (T16 → T21). Discarding the error would
        // leave the proposal `Queued` in the epoch while gone from the guard —
        // unable to execute or reach `Rejected` (the reported P2 for
        // `AttestationMissing`, which previously returned `BadDecisionInput`).
        epoch
            .expire_or_stale_queue(EpochOrigin::ExecutionGuard, ledger, pid, Some(reason))
            .map_err(|_| Error::StaleQueue)?;
        if let Some(idx) = self.queue.iter().position(|q| q.pid == pid) {
            self.queue.remove(idx);
        }
        self.events.push(Event::Rejected { pid, reason });
        Ok(())
    }

    pub fn view(&self) -> Vec<QueuedExecutionView> {
        self.queue
            .iter()
            .map(|q| QueuedExecutionView {
                pid: q.pid,
                class: q.class,
                payload_hash: q.payload_hash,
                maturity: q.maturity,
                grace_end: q.grace_end,
                version_constraint: q.version_constraint.clone(),
                cancelled: q.cancelled,
                ratification: if !requires_ratification(q.class) {
                    RatificationStatus::NotRequired
                } else if let Some(r) = q.ratify_ref {
                    if q.ratification_passed {
                        RatificationStatus::Passed { referendum: r }
                    } else {
                        RatificationStatus::Pending { referendum: r }
                    }
                } else {
                    RatificationStatus::Failed { referendum: 0 }
                },
                meters_clear: q
                    .meters_declared
                    .iter()
                    .all(|m| !self.blocked_meters.contains(m)),
            })
            .collect()
    }

    pub fn try_state(&self) -> Result<(), Error> {
        ensure!(self.queue.len() <= MAX_QUEUE, Error::QueueFull);
        ensure!(
            self.records.len() <= MAX_EXECUTION_RECORDS,
            Error::QueueFull
        );
        for q in &self.queue {
            ensure!(q.payload_len <= MAX_PAYLOAD_BYTES, Error::PayloadTooLarge);
            ensure!(
                q.meters_declared.len() <= MAX_RESOURCE_LOCKS,
                Error::TooManyLocks
            );
            ensure!(
                q.declared_domains.len() <= MAX_DECLARED_DOMAINS,
                Error::TooManyDomains
            );
        }
        Ok(())
    }
}

fn requires_ratification(class: ProposalClass) -> bool {
    matches!(
        class,
        ProposalClass::Code | ProposalClass::Meta | ProposalClass::Constitutional
    )
}

fn domain_allowed(class: ProposalClass, domain: CallDomain) -> bool {
    match domain {
        CallDomain::Public => true,
        CallDomain::Param => matches!(class, ProposalClass::Param),
        CallDomain::Treasury => matches!(class, ProposalClass::Treasury),
        CallDomain::Code => matches!(class, ProposalClass::Code),
        CallDomain::Meta => matches!(class, ProposalClass::Meta),
        // 09 §2: the runtime-upgrade path serves **CODE or META** artifacts, so
        // the internal-root authorize/apply domains are admissible for both.
        CallDomain::InternalRootAuthorizeUpgrade | CallDomain::InternalRootApplyUpgrade => {
            matches!(class, ProposalClass::Code | ProposalClass::Meta)
        }
    }
}

#[cfg(feature = "runtime-benchmarks")]
pub mod benchmarking {
    pub fn benchmark_stub() -> u32 {
        11
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use futarchy_primitives::{BoundedVec, ProposalState, RejectReason};
    use pallet_attestor::AttestorRegistry;
    use pallet_conditional_ledger::LedgerState;
    use pallet_epoch::{EpochState, MarketSet, Proposal};
    use pallet_origins::{CallDomain as ODomain, RuntimeCall};

    fn state_of(e: &EpochState<[u8; 32]>, pid: ProposalId) -> ProposalState {
        e.proposals.iter().find(|p| p.id == pid).unwrap().state
    }
    fn guardian7() -> Guardian {
        Guardian::new([
            acct(1),
            acct(2),
            acct(3),
            acct(4),
            acct(5),
            acct(6),
            acct(7),
        ])
        .unwrap()
    }
    /// A `Payload` whose declared hash is the correct content hash of its calls —
    /// the honest, committed preimage.
    fn pl(calls: Vec<DispatchCall>) -> Payload {
        Payload {
            hash: hash_payload(&calls),
            calls,
        }
    }

    fn vc(v: u32) -> RuntimeVersionConstraint {
        RuntimeVersionConstraint {
            spec_name: BoundedVec::try_from(b"bleavit".to_vec()).unwrap(),
            spec_version: v,
        }
    }
    fn h(x: u8) -> H256 {
        [x; 32]
    }
    fn acct(x: u8) -> [u8; 32] {
        [x; 32]
    }
    fn call(domain: CallDomain) -> DispatchCall {
        DispatchCall {
            domain,
            encoded_len: 10,
            declared_weight: 1,
            call: RuntimeCall::Leaf(match domain {
                CallDomain::Public => ODomain::Public,
                CallDomain::Param => ODomain::Param,
                CallDomain::Treasury => ODomain::Treasury,
                CallDomain::Code => ODomain::Code,
                CallDomain::Meta => ODomain::Meta,
                _ => ODomain::Code,
            }),
            succeeds: true,
            error: [0; 4],
            upgrade_hash: None,
            target_spec_version: None,
        }
    }
    /// The same committed call, but whose runtime dispatch reverts — used to
    /// drive T18. Its committed identity (and thus `hash_payload`) is identical
    /// to `call(domain)`; only the model's dispatch-outcome fields differ.
    fn failing_call(domain: CallDomain) -> DispatchCall {
        let mut c = call(domain);
        c.succeeds = false;
        c.error = [1, 2, 3, 4];
        c
    }
    fn queued(class: ProposalClass) -> QueuedExecution {
        QueuedExecution {
            pid: 1,
            payload_hash: h(9),
            payload_len: 10,
            class,
            maturity: 10,
            grace_end: 20,
            version_constraint: vc(1),
            meters_declared: vec![*b"resource"],
            ratify_ref: None,
            ratification_passed: false,
            attestation_id: None,
            pre_upgrade_checkpoint: None,
            cancelled: false,
            declared_domains: vec![
                CallDomain::Param,
                CallDomain::Public,
                CallDomain::Code,
                CallDomain::InternalRootAuthorizeUpgrade,
            ],
            failed_at: None,
        }
    }
    fn epoch_with_queued() -> (EpochState<[u8; 32]>, LedgerState<[u8; 32]>) {
        let mut e = EpochState::new();
        let mut l = LedgerState::new();
        l.create_vault(1, 0).unwrap();
        e.proposals.push(Proposal {
            id: 1,
            proposer: acct(1),
            class: ProposalClass::Param,
            state: ProposalState::Queued,
            epoch: 0,
            submitted_at: 0,
            payload_hash: h(9),
            ask: 1,
            bond: 1,
            resources: Vec::new(),
            metric_spec: 0,
            decide_at: 0,
            rerun: false,
            extended: false,
            delayed_once: false,
            markets: Some(MarketSet {
                accept: 1,
                reject: 2,
                gates: None,
                baseline: 3,
            }),
            maturity: Some(1_000_010),
            grace_end: Some(1_000_020),
            version_constraint: Some(vc(1)),
            decision: None,
        });
        (e, l)
    }

    #[test]
    fn enqueue_origin_and_execute_success_records_and_marks_epoch() {
        let mut g = ExecutionGuard::new(vc(1));
        let calls = vec![call(CallDomain::Param)];
        let mut q = queued(ProposalClass::Param);
        q.maturity = 1_000_010;
        q.grace_end = 1_000_020;
        q.payload_hash = hash_payload(&calls);
        assert_eq!(
            g.enqueue(GuardOrigin::Signed, q.clone()).unwrap_err(),
            Error::BadOrigin
        );
        g.held_resources.push((1, *b"resource"));
        g.enqueue(GuardOrigin::EpochDecision, q).unwrap();
        let (mut e, mut l) = epoch_with_queued();
        let guardian = guardian7();
        let att = AttestorRegistry::new(vec![acct(1), acct(2), acct(3)]).unwrap();
        g.execute(
            GuardOrigin::Signed,
            &mut e,
            &mut l,
            &guardian,
            &att,
            1,
            pl(calls),
            1_000_010,
            h(1),
            h(2),
        )
        .unwrap();
        assert_eq!(g.records.len(), 1);
        assert!(e
            .events
            .iter()
            .any(|ev| matches!(ev, pallet_epoch::Event::MeasurementStarted { .. })));
    }

    #[test]
    fn stale_version_and_meter_contention_block_execution() {
        let mut g = ExecutionGuard::new(vc(2));
        let calls = vec![call(CallDomain::Param)];
        let mut q = queued(ProposalClass::Param);
        q.payload_hash = hash_payload(&calls);
        g.held_resources.push((1, *b"resource"));
        g.enqueue(GuardOrigin::EpochDecision, q).unwrap();
        let guardian = guardian7();
        let att = AttestorRegistry::new(vec![acct(1), acct(2), acct(3)]).unwrap();
        assert_eq!(
            g.check_dispatch_time(&g.queue[0], &guardian, &att, &pl(calls.clone()), 10)
                .unwrap_err(),
            Error::StaleQueue
        );
        g.current_spec_name = vc(1);
        g.blocked_meters.push(*b"resource");
        assert_eq!(
            g.check_dispatch_time(&g.queue[0], &guardian, &att, &pl(calls), 10)
                .unwrap_err(),
            Error::MetersBlocked
        );
    }

    #[test]
    fn capability_and_safety_filter_reject_wrapper_bypass() {
        let mut g = ExecutionGuard::new(vc(1));
        let cap_calls = vec![call(CallDomain::Treasury)];
        let mut q = queued(ProposalClass::Param);
        q.declared_domains = vec![CallDomain::Treasury];
        q.payload_hash = hash_payload(&cap_calls);
        g.held_resources.push((1, *b"resource"));
        g.enqueue(GuardOrigin::EpochDecision, q).unwrap();
        let guardian = guardian7();
        let att = AttestorRegistry::new(vec![acct(1), acct(2), acct(3)]).unwrap();
        assert_eq!(
            g.check_dispatch_time(&g.queue[0], &guardian, &att, &pl(cap_calls), 10)
                .unwrap_err(),
            Error::CapabilityDenied
        );
        let mut c = call(CallDomain::Param);
        c.call = RuntimeCall::Proxy(pallet_origins::BoxedCall::new(RuntimeCall::Leaf(
            ODomain::Param,
        )));
        let sf_calls = vec![c];
        let mut q2 = queued(ProposalClass::Param);
        q2.declared_domains = vec![CallDomain::Param];
        q2.payload_hash = hash_payload(&sf_calls);
        g.queue[0] = q2;
        assert_eq!(
            g.check_dispatch_time(&g.queue[0], &guardian, &att, &pl(sf_calls), 10)
                .unwrap_err(),
            Error::SafetyFilter
        );
    }

    #[test]
    fn upgrade_authorize_and_descriptor_lead_time() {
        let mut g = ExecutionGuard::new(vc(1));
        let code = b"runtime-wasm-v2".to_vec();
        let mut c = call(CallDomain::InternalRootAuthorizeUpgrade);
        c.upgrade_hash = Some(content_hash(&code));
        c.target_spec_version = Some(2);
        let calls = vec![c];
        let ph = hash_payload(&calls);
        let mut q = queued(ProposalClass::Code);
        q.maturity = 43_200;
        q.grace_end = 90_000;
        q.payload_hash = ph;
        q.attestation_id = Some(0);
        q.ratify_ref = Some(42);
        q.ratification_passed = true;
        q.declared_domains = vec![CallDomain::InternalRootAuthorizeUpgrade];
        g.held_resources.push((1, *b"resource"));
        g.enqueue(GuardOrigin::EpochDecision, q).unwrap();
        let mut att = AttestorRegistry::new(vec![acct(1), acct(2), acct(3)]).unwrap();
        att.attest(acct(1), 1, ph, h(8), 0).unwrap();
        att.attest(acct(2), 1, ph, h(8), 0).unwrap();
        let guardian = guardian7();
        g.check_dispatch_time(&g.queue[0], &guardian, &att, &pl(calls.clone()), 43_201)
            .unwrap();
        assert_eq!(
            g.dispatch_batch(&g.queue[0].clone(), &pl(calls), 43_201, h(1), h(2))
                .unwrap(),
            DispatchOutcomeCode::Ok
        );
        assert_eq!(
            g.apply_authorized_upgrade(GuardOrigin::Signed, &code, 2, 50_000)
                .unwrap_err(),
            Error::DescriptorLeadTime
        );
        g.apply_authorized_upgrade(GuardOrigin::Signed, &code, 2, 86_401)
            .unwrap();
        assert!(g.pending_upgrade.is_none());
        assert_eq!(g.current_spec_name.spec_version, 2);
    }

    #[test]
    fn ring_is_bounded_and_try_state_checks_bounds() {
        let mut g = ExecutionGuard::new(vc(1));
        for i in 0..300 {
            g.push_record(ExecutionRecord {
                pid: i,
                payload_hash: h(1),
                class: ProposalClass::Param,
                executed_at: i as u32,
                result: DispatchOutcomeCode::Ok,
            });
        }
        assert_eq!(g.records.len(), MAX_EXECUTION_RECORDS);
        assert_eq!(g.records[0].pid, 44);
        let mut bad = queued(ProposalClass::Param);
        bad.payload_len = MAX_PAYLOAD_BYTES + 1;
        g.queue.push(bad);
        assert_eq!(g.try_state().unwrap_err(), Error::PayloadTooLarge);
    }

    // P1 (09 §1.2(2)): execution is bound to the committed preimage — the hash is
    // re-derived from the presented calls, so a keeper cannot swap `payload.calls`
    // under the queued hash even while supplying `payload.hash == payload_hash`.
    #[test]
    fn preimage_binding_rejects_swapped_calls() {
        let mut g = ExecutionGuard::new(vc(1));
        let committed = vec![call(CallDomain::Param)];
        let ph = hash_payload(&committed);
        let mut q = queued(ProposalClass::Param);
        q.payload_hash = ph;
        g.held_resources.push((1, *b"resource"));
        g.enqueue(GuardOrigin::EpochDecision, q).unwrap();
        let guardian = guardian7();
        let att = AttestorRegistry::new(vec![acct(1), acct(2), acct(3)]).unwrap();
        // Different calls carrying the committed hash as a trusted field: rejected.
        let swapped = Payload {
            hash: ph,
            calls: vec![call(CallDomain::Treasury)],
        };
        assert_eq!(
            g.check_dispatch_time(&g.queue[0], &guardian, &att, &swapped, 10)
                .unwrap_err(),
            Error::BadPreimage
        );
        // The exact committed calls pass the binding.
        g.check_dispatch_time(&g.queue[0], &guardian, &att, &pl(committed), 10)
            .unwrap();
    }

    // P1 (09 §2.1(4)/(6)): applying an upgrade re-derives the hash from the
    // submitted artifact bytes and records the *authorized* target version — a
    // signed caller cannot clear `PendingUpgrade` with the public hash and an
    // arbitrary spec version.
    #[test]
    fn apply_upgrade_binds_artifact_and_authorized_version() {
        let mut g = ExecutionGuard::new(vc(1));
        let code = b"runtime-v2-wasm".to_vec();
        g.pending_upgrade = Some(PendingUpgrade {
            hash: content_hash(&code),
            authorized_at: 0,
            applicable_at: 43_200,
            target_spec_version: 2,
        });
        assert_eq!(
            g.apply_authorized_upgrade(GuardOrigin::Signed, &code, 2, 43_199)
                .unwrap_err(),
            Error::DescriptorLeadTime
        );
        assert_eq!(
            g.apply_authorized_upgrade(GuardOrigin::Signed, b"wrong-artifact", 2, 43_200)
                .unwrap_err(),
            Error::UpgradeHashMismatch
        );
        assert_eq!(
            g.apply_authorized_upgrade(GuardOrigin::Signed, &code, 99, 43_200)
                .unwrap_err(),
            Error::UpgradeVersionMismatch
        );
        g.apply_authorized_upgrade(GuardOrigin::Signed, &code, 2, 43_200)
            .unwrap();
        assert!(g.pending_upgrade.is_none());
        assert_eq!(g.current_spec_name.spec_version, 2);
    }

    // P1 (05 §2.1 T18/T23): a reverted payload advances to FailedExecuted with a
    // recorded failure and stays retryable within the window; a later succeeding
    // dispatch of the same committed calls executes (T23) and starts measurement.
    #[test]
    fn failed_dispatch_advances_to_failed_executed_then_retry_succeeds() {
        let mut g = ExecutionGuard::new(vc(1));
        let committed = vec![call(CallDomain::Param)];
        let ph = hash_payload(&committed);
        let mut q = queued(ProposalClass::Param);
        q.maturity = 1_000_010;
        q.grace_end = 1_000_020;
        q.payload_hash = ph;
        g.held_resources.push((1, *b"resource"));
        g.enqueue(GuardOrigin::EpochDecision, q).unwrap();
        let (mut e, mut l) = epoch_with_queued();
        let guardian = guardian7();
        let att = AttestorRegistry::new(vec![acct(1), acct(2), acct(3)]).unwrap();
        // First attempt: the runtime dispatch reverts.
        g.execute(
            GuardOrigin::Signed,
            &mut e,
            &mut l,
            &guardian,
            &att,
            1,
            pl(vec![failing_call(CallDomain::Param)]),
            1_000_010,
            h(1),
            h(2),
        )
        .unwrap();
        assert_eq!(state_of(&e, 1), ProposalState::FailedExecuted);
        assert_eq!(g.records.len(), 1);
        assert!(g.queue[0].failed_at.is_some());
        assert!(e.events.iter().any(
            |ev| matches!(ev, pallet_epoch::Event::ExecutionFailed { reason, .. }
                if *reason == RejectReason::PayloadReverted)
        ));
        // Retry (T23) within the window with a now-succeeding dispatch.
        g.execute(
            GuardOrigin::Signed,
            &mut e,
            &mut l,
            &guardian,
            &att,
            1,
            pl(committed),
            1_000_015,
            h(1),
            h(2),
        )
        .unwrap();
        assert_eq!(state_of(&e, 1), ProposalState::Measuring);
        assert!(g.queue.is_empty());
    }

    // P1 (05 §2.1 T22): once the retry window is exhausted a keeper drives the
    // FailedExecuted → Measuring transition (executed-with-failure).
    #[test]
    fn failed_execution_retry_window_exhausts_to_measurement() {
        let mut g = ExecutionGuard::new(vc(1));
        let mut q = queued(ProposalClass::Param);
        q.maturity = 1_000_010;
        q.grace_end = 1_000_020;
        q.payload_hash = hash_payload(&[call(CallDomain::Param)]);
        g.held_resources.push((1, *b"resource"));
        g.enqueue(GuardOrigin::EpochDecision, q).unwrap();
        let (mut e, mut l) = epoch_with_queued();
        let guardian = guardian7();
        let att = AttestorRegistry::new(vec![acct(1), acct(2), acct(3)]).unwrap();
        g.execute(
            GuardOrigin::Signed,
            &mut e,
            &mut l,
            &guardian,
            &att,
            1,
            pl(vec![failing_call(CallDomain::Param)]),
            1_000_010,
            h(1),
            h(2),
        )
        .unwrap();
        // Within the window the T22 expiry is refused.
        assert_eq!(
            g.expire_failed_execution(
                GuardOrigin::Signed,
                &mut e,
                &mut l,
                1,
                1_000_010 + RETRY_WINDOW
            )
            .unwrap_err(),
            Error::RetryWindowOpen
        );
        // Past the window it measures as executed-with-failure.
        g.expire_failed_execution(
            GuardOrigin::Signed,
            &mut e,
            &mut l,
            1,
            1_000_010 + RETRY_WINDOW + 1,
        )
        .unwrap();
        assert_eq!(state_of(&e, 1), ProposalState::Measuring);
        assert!(g.queue.is_empty());
    }

    // P2 (09 §1.2(5)): an AttestationMissing rejection propagates to the epoch
    // (T16 → T21) instead of being discarded and stranding the proposal Queued.
    #[test]
    fn attestation_missing_rejection_propagates_to_epoch() {
        let mut g = ExecutionGuard::new(vc(1));
        let mut q = queued(ProposalClass::Code);
        q.payload_hash = hash_payload(&[call(CallDomain::Code)]);
        g.enqueue(GuardOrigin::EpochDecision, q).unwrap();
        let (mut e, mut l) = epoch_with_queued();
        e.proposals[0].class = ProposalClass::Code;
        g.reject_stale_or_unratified(&mut e, &mut l, 1, RejectReason::AttestationMissing)
            .unwrap();
        // T21 fires in the same block: Rejected(AttestationMissing) → Measuring.
        assert_eq!(state_of(&e, 1), ProposalState::Measuring);
        assert!(e.events.iter().any(|ev| matches!(
            ev,
            pallet_epoch::Event::ProposalRejected { reason, .. }
                if *reason == RejectReason::AttestationMissing
        )));
        assert!(g.queue.is_empty());
    }

    // P2 (09 §2): a META-class runtime upgrade passes the capability table and
    // authorizes — the internal-root upgrade domain is admissible for CODE or META.
    #[test]
    fn meta_upgrade_passes_capability_and_authorizes() {
        let mut g = ExecutionGuard::new(vc(1));
        let code = b"meta-runtime-wasm".to_vec();
        let mut c = call(CallDomain::InternalRootAuthorizeUpgrade);
        c.call = RuntimeCall::Leaf(ODomain::Meta);
        c.upgrade_hash = Some(content_hash(&code));
        c.target_spec_version = Some(3);
        let calls = vec![c];
        let ph = hash_payload(&calls);
        let mut q = queued(ProposalClass::Meta);
        q.maturity = 43_200;
        q.grace_end = 90_000;
        q.payload_hash = ph;
        q.attestation_id = Some(0);
        q.ratify_ref = Some(7);
        q.ratification_passed = true;
        q.declared_domains = vec![CallDomain::InternalRootAuthorizeUpgrade];
        g.held_resources.push((1, *b"resource"));
        g.enqueue(GuardOrigin::EpochDecision, q).unwrap();
        let mut att = AttestorRegistry::new(vec![acct(1), acct(2), acct(3)]).unwrap();
        att.attest(acct(1), 1, ph, h(8), 0).unwrap();
        att.attest(acct(2), 1, ph, h(8), 0).unwrap();
        let guardian = guardian7();
        // Previously CapabilityDenied for META; now the full check list passes.
        g.check_dispatch_time(&g.queue[0], &guardian, &att, &pl(calls.clone()), 43_201)
            .unwrap();
        assert_eq!(
            g.dispatch_batch(&g.queue[0].clone(), &pl(calls), 43_201, h(1), h(2))
                .unwrap(),
            DispatchOutcomeCode::Ok
        );
        assert!(g.pending_upgrade.is_some());
    }
}

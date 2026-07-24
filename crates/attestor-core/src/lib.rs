#![cfg_attr(not(feature = "std"), no_std)]
#![deny(unsafe_code)]

extern crate alloc;

use alloc::vec::Vec;
use futarchy_primitives::{AccountId, Balance, BlockNumber, ProposalId, H256};
use parity_scale_codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use scale_info::TypeInfo;

pub type AttestationId = u32;
// Single-homed kernel constants (13 rule 1 / 01 §5.2): import, never re-declare.
pub const MIN_MEMBERS: usize = futarchy_primitives::kernel::ATT_MIN_MEMBERS as usize;
pub const QUORUM: usize = futarchy_primitives::kernel::ATT_QUORUM as usize;
pub const ATTESTOR_BOND: Balance = 25_000_000_000_000_000;
pub const CHALLENGE_WINDOW_BLOCKS: BlockNumber = futarchy_primitives::kernel::ORC_WINDOW_BLOCKS;
pub const CHALLENGE_BOND: Balance = ATTESTOR_BOND / 2;
pub const FALSE_EJECTION_THRESHOLD: u8 = 2;

/// Live constitution values consumed by the attestor state machine.
///
/// The constants above remain the genesis defaults. Runtime callers hydrate
/// this plain frame-free value from
/// `pallet-constitution::Params` before every operation that consumes a
/// tunable (13 reading rule 2).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AttestorParams {
    pub bond: Balance,
    pub challenge_window: BlockNumber,
}

impl AttestorParams {
    pub const DEFAULT: Self = Self {
        bond: ATTESTOR_BOND,
        challenge_window: CHALLENGE_WINDOW_BLOCKS,
    };
}

/// Fifty percent of governed `att.bond`, rounded against the claimant (R-7).
const fn half_ceil(value: Balance) -> Balance {
    (value / 2).saturating_add(value % 2)
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
pub enum AttestorOrigin {
    Signed,
    ConstitutionalValues,
    RatifyTrack,
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
pub enum ChallengeStatus {
    Open {
        challenger: AccountId,
        evidence_hash: H256,
        bond: Balance,
    },
    Upheld,
    Rejected,
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
pub struct AttestorInfo {
    pub account: AccountId,
    pub bond: Balance,
    pub false_count: u8,
    pub active: bool,
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
pub struct Attestation {
    pub id: AttestationId,
    pub pid: ProposalId,
    pub artifact_hash: H256,
    pub statement_hash: H256,
    pub attestor: AccountId,
    pub submitted_at: BlockNumber,
    pub challenge_deadline: BlockNumber,
    pub challenge: Option<ChallengeStatus>,
}

/// Bond basis retained after an attestor leaves the active roster.
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
pub struct AttestorLiability {
    pub account: AccountId,
    pub bond: Balance,
    pub false_count: u8,
    pub ejected: bool,
}

/// Durable cause marker for a record whose signer lost authority. The
/// original `Attestation` shape remains frozen; this auxiliary vector is the
/// contract-v10 addition.
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
pub struct AttestationRevocation {
    pub attestation_id: AttestationId,
    pub pid: ProposalId,
    pub attestor: AccountId,
    pub cause_hash: H256,
}

#[derive(Clone, Debug, Decode, Encode, Eq, PartialEq, TypeInfo)]
pub enum Event {
    MembersSet {
        members: Vec<AccountId>,
    },
    AttestationSubmitted {
        attestation_id: AttestationId,
        pid: ProposalId,
        artifact_hash: H256,
        attestor: AccountId,
    },
    AttestationChallenged {
        attestation_id: AttestationId,
        challenger: AccountId,
        evidence_hash: H256,
    },
    ChallengeResolved {
        attestation_id: AttestationId,
        upheld: bool,
        loser: AccountId,
        slashed: Balance,
    },
    AttestorEjected {
        who: AccountId,
    },
    AttestorRemovedForCause {
        who: AccountId,
        cause_hash: H256,
    },
    AttestationRevoked {
        attestation_id: AttestationId,
        pid: ProposalId,
        attestor: AccountId,
        cause_hash: H256,
    },
}

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub enum Error {
    BadOrigin,
    NotMember,
    DuplicateMember,
    TooFewMembers,
    AttestationNotFound,
    DuplicateAttestation,
    ChallengeWindowClosed,
    ChallengeAlreadyOpen,
    ChallengeBondTooSmall,
    ChallengeStillOpen,
    QuorumMissing,
    /// The referenced attestation exists but has no open challenge to resolve.
    NoOpenChallenge,
    /// try-state only: a member at or past the ejection threshold is still
    /// marked active (06 §7 ejection is terminal while the strike count stands).
    EjectedMemberActive,
    LiabilityExists,
    LiabilityNotFound,
    ProposalNotTerminal,
    ChallengeOpen,
    ReapNotAllowed,
    TooManyLiabilities,
    TooManyRevocations,
    Overflow,
}

#[derive(Clone, Debug, Decode, Encode, Eq, PartialEq, TypeInfo)]
pub struct AttestorRegistry {
    pub members: Vec<AttestorInfo>,
    pub liabilities: Vec<AttestorLiability>,
    pub attestations: Vec<Attestation>,
    pub revocations: Vec<AttestationRevocation>,
    pub next_attestation_id: AttestationId,
    pub events: Vec<Event>,
}

impl AttestorRegistry {
    pub fn new(members: Vec<AccountId>, params: AttestorParams) -> Result<Self, Error> {
        let infos = validate_and_infos(members, params.bond)?;
        Ok(Self {
            members: infos,
            liabilities: Vec::new(),
            attestations: Vec::new(),
            revocations: Vec::new(),
            next_attestation_id: 0,
            events: Vec::new(),
        })
    }
    /// Install the values-elected roster (06 §7).
    ///
    /// Seating is **not** a wholesale rewrite of the member table (SQ-262):
    ///
    /// * `bond` re-binds to the live `att.bond` for every seated member — 13 §1's
    ///   `att.bond` row (SQ-248) makes an amendment reach a member exactly at
    ///   their next seating.
    /// * `false_count` is **carried forward** for a continuing member. Nothing in
    ///   06 §7 resets strikes, and resetting them made one no-op re-election wipe
    ///   the ejection threshold, so the "ejection on the second adjudicated-false
    ///   attestation" discipline was unreachable in practice.
    /// * A departing member with an unsettled record moves to `liabilities`.
    ///   Governance authority ends immediately, while the slash basis remains
    ///   available to challenge resolution and permissionless reap.
    pub fn set_members(
        &mut self,
        origin: AttestorOrigin,
        members: Vec<AccountId>,
        now: BlockNumber,
        params: AttestorParams,
    ) -> Result<(), Error> {
        ensure!(
            matches!(origin, AttestorOrigin::ConstitutionalValues),
            Error::BadOrigin
        );
        let mut infos = validate_and_infos(members.clone(), params.bond)?;
        for info in infos.iter_mut() {
            if let Some(previous) = self
                .members
                .iter()
                .find(|member| member.account == info.account)
            {
                info.false_count = previous.false_count;
                // An attestor already ejected for cause stays ejected: seating
                // MUST NOT launder the ejection the core records at
                // `FALSE_EJECTION_THRESHOLD`.
                info.active = previous.false_count < FALSE_EJECTION_THRESHOLD;
            }
        }
        for info in infos.iter() {
            ensure!(
                !self
                    .liabilities
                    .iter()
                    .any(|liability| liability.account == info.account),
                Error::LiabilityExists
            );
        }
        let previous_members = core::mem::take(&mut self.members);
        for previous in previous_members.iter() {
            let seated = infos.iter().any(|info| info.account == previous.account);
            if seated || !self.has_unsettled_liability(previous.account, now) {
                continue;
            }
            self.liabilities.push(AttestorLiability {
                account: previous.account,
                bond: previous.bond,
                false_count: previous.false_count,
                ejected: !previous.active,
            });
        }
        self.members = infos;
        self.events.push(Event::MembersSet { members });
        Ok(())
    }

    /// Whether `who` still backs a record that can move against their bond: an
    /// open challenge, or an attestation still inside its challenge window. A
    /// settled record (`Upheld`/`Rejected`) carries no further liability, and a
    /// closed window can never be challenged again (`challenge_attestation` is
    /// deadline-barred), so neither retains a basis.
    fn has_unsettled_liability(&self, who: AccountId, now: BlockNumber) -> bool {
        self.attestations
            .iter()
            .filter(|attestation| attestation.attestor == who)
            .any(|attestation| match attestation.challenge {
                Some(ChallengeStatus::Open { .. }) => true,
                None => now <= attestation.challenge_deadline,
                Some(ChallengeStatus::Upheld) | Some(ChallengeStatus::Rejected) => false,
            })
    }

    pub fn bond_basis(&self, who: AccountId) -> Option<Balance> {
        self.members
            .iter()
            .find(|member| member.account == who)
            .map(|member| member.bond)
            .or_else(|| {
                self.liabilities
                    .iter()
                    .find(|liability| liability.account == who)
                    .map(|liability| liability.bond)
            })
    }

    pub fn is_revoked(&self, id: AttestationId) -> bool {
        self.revocations
            .iter()
            .any(|revocation| revocation.attestation_id == id)
    }
    pub fn attest(
        &mut self,
        who: AccountId,
        pid: ProposalId,
        artifact_hash: H256,
        statement_hash: H256,
        now: BlockNumber,
        params: AttestorParams,
    ) -> Result<AttestationId, Error> {
        self.ensure_active_member(who)?;
        ensure!(
            !self
                .attestations
                .iter()
                .any(|a| a.pid == pid && a.artifact_hash == artifact_hash && a.attestor == who),
            Error::DuplicateAttestation
        );
        let id = self.next_attestation_id;
        self.next_attestation_id = self
            .next_attestation_id
            .checked_add(1)
            .ok_or(Error::Overflow)?;
        let att = Attestation {
            id,
            pid,
            artifact_hash,
            statement_hash,
            attestor: who,
            submitted_at: now,
            challenge_deadline: now
                .checked_add(params.challenge_window)
                .ok_or(Error::Overflow)?,
            challenge: None,
        };
        self.attestations.push(att);
        self.events.push(Event::AttestationSubmitted {
            attestation_id: id,
            pid,
            artifact_hash,
            attestor: who,
        });
        Ok(id)
    }
    pub fn challenge_attestation(
        &mut self,
        challenger: AccountId,
        id: AttestationId,
        evidence_hash: H256,
        bond: Balance,
        now: BlockNumber,
    ) -> Result<(), Error> {
        let attestor = self
            .attestations
            .iter()
            .find(|attestation| attestation.id == id)
            .map(|attestation| attestation.attestor)
            .ok_or(Error::AttestationNotFound)?;
        let stored_bond = self.bond_basis(attestor).ok_or(Error::NotMember)?;
        ensure!(bond >= half_ceil(stored_bond), Error::ChallengeBondTooSmall);
        let att = self
            .attestations
            .iter_mut()
            .find(|a| a.id == id)
            .ok_or(Error::AttestationNotFound)?;
        ensure!(now <= att.challenge_deadline, Error::ChallengeWindowClosed);
        ensure!(att.challenge.is_none(), Error::ChallengeAlreadyOpen);
        att.challenge = Some(ChallengeStatus::Open {
            challenger,
            evidence_hash,
            bond,
        });
        self.events.push(Event::AttestationChallenged {
            attestation_id: id,
            challenger,
            evidence_hash,
        });
        Ok(())
    }
    pub fn resolve_challenge(
        &mut self,
        origin: AttestorOrigin,
        id: AttestationId,
        attestation_upheld: bool,
    ) -> Result<(), Error> {
        self.resolve_challenge_with(origin, id, attestation_upheld, |_| false)
    }

    pub fn resolve_challenge_with<F: FnMut(ProposalId) -> bool>(
        &mut self,
        origin: AttestorOrigin,
        id: AttestationId,
        attestation_upheld: bool,
        mut is_executed: F,
    ) -> Result<(), Error> {
        ensure!(
            matches!(origin, AttestorOrigin::RatifyTrack),
            Error::BadOrigin
        );
        let idx = self
            .attestations
            .iter()
            .position(|a| a.id == id)
            .ok_or(Error::AttestationNotFound)?;
        let (challenger, bond) = match self.attestations[idx].challenge {
            Some(ChallengeStatus::Open {
                challenger, bond, ..
            }) => (challenger, bond),
            _ => return Err(Error::NoOpenChallenge),
        };
        let loser = if attestation_upheld {
            challenger
        } else {
            self.attestations[idx].attestor
        };
        let slashed = if attestation_upheld {
            half_ceil(bond)
        } else {
            let stored_bond = self.bond_basis(loser).ok_or(Error::NotMember)?;
            half_ceil(stored_bond)
        };
        if attestation_upheld {
            self.attestations[idx].challenge = Some(ChallengeStatus::Upheld);
        } else {
            self.attestations[idx].challenge = Some(ChallengeStatus::Rejected);
            let attestor = self.attestations[idx].attestor;
            let mut ejected = false;
            if let Some(info) = self.members.iter_mut().find(|m| m.account == attestor) {
                info.bond = info.bond.saturating_sub(slashed);
                info.false_count = info.false_count.saturating_add(1);
                if info.false_count >= FALSE_EJECTION_THRESHOLD {
                    info.active = false;
                    ejected = true;
                }
            } else if let Some(liability) = self
                .liabilities
                .iter_mut()
                .find(|liability| liability.account == attestor)
            {
                liability.bond = liability.bond.saturating_sub(slashed);
                liability.false_count = liability.false_count.saturating_add(1);
                if liability.false_count >= FALSE_EJECTION_THRESHOLD {
                    liability.ejected = true;
                    ejected = true;
                }
            } else {
                return Err(Error::NotMember);
            }
            if ejected {
                self.events.push(Event::AttestorEjected { who: attestor });
                self.revoke_records(attestor, [0; 32], &mut is_executed)?;
            }
        }
        self.events.push(Event::ChallengeResolved {
            attestation_id: id,
            upheld: attestation_upheld,
            loser,
            slashed,
        });
        Ok(())
    }
    pub fn has_quorum(&self, pid: ProposalId, artifact_hash: H256, now: BlockNumber) -> bool {
        // 06 §7: "≥ 3 members required before any CODE/META proposal can
        // qualify". The gate needs a full **active** registry, not just a 2-of-N
        // count — if ejections drop the active membership below MIN_MEMBERS the
        // attestation gate fails outright, even if two active members' windows
        // have closed (A10 Codex PR review, I-19).
        if self.active_member_count() < MIN_MEMBERS {
            return false;
        }
        let mut distinct: Vec<AccountId> = Vec::new();
        for att in self
            .attestations
            .iter()
            .filter(|a| a.pid == pid && a.artifact_hash == artifact_hash)
        {
            if distinct.contains(&att.attestor)
                || !self.is_active_member(att.attestor)
                || self.is_revoked(att.id)
            {
                continue;
            }
            if self.attestation_counts(att, now) {
                distinct.push(att.attestor);
            }
        }
        distinct.len() >= QUORUM
    }

    /// Execute-time quorum over the committed record set. Routine roster
    /// rotation cannot invalidate it; durable revocation and challenge state
    /// still fail closed.
    pub fn has_record_quorum(
        &self,
        pid: ProposalId,
        artifact_hash: H256,
        now: BlockNumber,
    ) -> bool {
        let mut distinct: Vec<AccountId> = Vec::new();
        for att in self
            .attestations
            .iter()
            .filter(|a| a.pid == pid && a.artifact_hash == artifact_hash)
        {
            if distinct.contains(&att.attestor) || self.is_revoked(att.id) {
                continue;
            }
            if self.attestation_counts(att, now) {
                distinct.push(att.attestor);
            }
        }
        distinct.len() >= QUORUM
    }

    pub fn remove_for_cause<F: FnMut(ProposalId) -> bool>(
        &mut self,
        origin: AttestorOrigin,
        who: AccountId,
        cause_hash: H256,
        mut is_executed: F,
    ) -> Result<(), Error> {
        ensure!(
            matches!(origin, AttestorOrigin::ConstitutionalValues),
            Error::BadOrigin
        );
        let index = self
            .members
            .iter()
            .position(|member| member.account == who)
            .ok_or(Error::NotMember)?;
        let member = self.members.remove(index);
        self.liabilities.push(AttestorLiability {
            account: who,
            bond: member.bond,
            false_count: member.false_count,
            ejected: true,
        });
        self.revoke_records(who, cause_hash, &mut is_executed)?;
        self.events
            .push(Event::AttestorRemovedForCause { who, cause_hash });
        Ok(())
    }

    fn revoke_records<F: FnMut(ProposalId) -> bool>(
        &mut self,
        who: AccountId,
        cause_hash: H256,
        is_executed: &mut F,
    ) -> Result<(), Error> {
        let records: Vec<(AttestationId, ProposalId)> = self
            .attestations
            .iter()
            .filter(|attestation| {
                attestation.attestor == who
                    && !is_executed(attestation.pid)
                    && !matches!(attestation.challenge, Some(ChallengeStatus::Rejected))
                    && !self.is_revoked(attestation.id)
            })
            .map(|attestation| (attestation.id, attestation.pid))
            .collect();
        for (attestation_id, pid) in records {
            self.revocations.push(AttestationRevocation {
                attestation_id,
                pid,
                attestor: who,
                cause_hash,
            });
            self.events.push(Event::AttestationRevoked {
                attestation_id,
                pid,
                attestor: who,
                cause_hash,
            });
        }
        Ok(())
    }

    pub fn reap_attestation<F: FnMut(ProposalId) -> bool>(
        &mut self,
        id: AttestationId,
        now: BlockNumber,
        mut is_terminal: F,
    ) -> Result<Option<AttestorLiability>, Error> {
        let index = self
            .attestations
            .iter()
            .position(|attestation| attestation.id == id)
            .ok_or(Error::AttestationNotFound)?;
        let attestation = self.attestations[index];
        ensure!(is_terminal(attestation.pid), Error::ProposalNotTerminal);
        ensure!(
            !matches!(attestation.challenge, Some(ChallengeStatus::Open { .. })),
            Error::ChallengeOpen
        );
        ensure!(
            attestation.challenge.is_some() || now > attestation.challenge_deadline,
            Error::ReapNotAllowed
        );
        self.attestations.remove(index);
        self.revocations
            .retain(|revocation| revocation.attestation_id != id);
        let who = attestation.attestor;
        let still_present = self
            .attestations
            .iter()
            .any(|record| record.attestor == who)
            || self.revocations.iter().any(|record| record.attestor == who);
        if !still_present {
            if let Some(index) = self
                .liabilities
                .iter()
                .position(|liability| liability.account == who)
            {
                return Ok(Some(self.liabilities.remove(index)));
            }
        }
        Ok(None)
    }
    pub fn require_quorum(
        &self,
        pid: ProposalId,
        artifact_hash: H256,
        now: BlockNumber,
    ) -> Result<(), Error> {
        ensure!(
            self.has_quorum(pid, artifact_hash, now),
            Error::QuorumMissing
        );
        Ok(())
    }
    pub fn try_state(&self) -> Result<(), Error> {
        for i in 0..self.members.len() {
            for j in (i + 1)..self.members.len() {
                ensure!(
                    self.members[i].account != self.members[j].account,
                    Error::DuplicateMember
                );
            }
        }
        // Historical attestations from recalled attestors are NOT an invariant
        // violation: `has_quorum` counts only active members, so a lawful recall
        // (`set_members`) legitimately leaves non-member attestations in the
        // ledger. Requiring every stored attestation's attestor to still be
        // seated turned a valid recall-after-attest into a spurious,
        // release-blocking try_state failure (06 §7; A10 spec-reviewer major).
        // Reaping dead records is the B2-deferred map (PLAN SQ-2).
        //
        // Ejection is terminal while the strike count stands (06 §7): reaching
        // `FALSE_EJECTION_THRESHOLD` clears `active` in `resolve_challenge`, and
        // seating preserves that (SQ-262). An ejected-but-active row would let a
        // disqualified signer attest and count toward the CODE/META gate.
        for member in self.members.iter() {
            ensure!(
                member.false_count < FALSE_EJECTION_THRESHOLD || !member.active,
                Error::EjectedMemberActive
            );
        }
        for i in 0..self.liabilities.len() {
            ensure!(
                !self
                    .members
                    .iter()
                    .any(|member| member.account == self.liabilities[i].account),
                Error::LiabilityExists
            );
            for j in (i + 1)..self.liabilities.len() {
                ensure!(
                    self.liabilities[i].account != self.liabilities[j].account,
                    Error::LiabilityExists
                );
            }
        }
        // Every open challenge must still have a slash basis to resolve against
        // (SQ-262): seating retains a liable member as an inactive row, so an
        // `Open` challenge whose attestor has no row at all is unresolvable on
        // its adverse branch and would strand the record permanently.
        for attestation in self.attestations.iter() {
            if matches!(attestation.challenge, Some(ChallengeStatus::Open { .. })) {
                ensure!(
                    self.bond_basis(attestation.attestor).is_some(),
                    Error::NotMember
                );
            }
        }
        for revocation in self.revocations.iter() {
            ensure!(
                self.attestations.iter().any(|attestation| {
                    attestation.id == revocation.attestation_id
                        && attestation.pid == revocation.pid
                        && attestation.attestor == revocation.attestor
                }),
                Error::AttestationNotFound
            );
        }
        Ok(())
    }
    fn attestation_counts(&self, att: &Attestation, now: BlockNumber) -> bool {
        match att.challenge {
            // 06 §7: only attestations "whose windows have closed" count. An
            // upheld challenge proves the attestation valid but does NOT shortcut
            // the 72-hour window — quorum still waits for `challenge_deadline`
            // (conservative reading for the security-critical I-19 gate; A10
            // Codex adversarial finding). Open/Rejected never count.
            None | Some(ChallengeStatus::Upheld) => now > att.challenge_deadline,
            Some(ChallengeStatus::Rejected) | Some(ChallengeStatus::Open { .. }) => false,
        }
    }
    fn ensure_active_member(&self, who: AccountId) -> Result<(), Error> {
        ensure!(self.is_active_member(who), Error::NotMember);
        Ok(())
    }
    fn is_active_member(&self, who: AccountId) -> bool {
        self.members.iter().any(|m| m.account == who && m.active)
    }
    /// Count of currently active (non-ejected) attestors (06 §7 registry floor).
    pub fn active_member_count(&self) -> usize {
        self.members.iter().filter(|m| m.active).count()
    }
}

fn validate_and_infos(
    members: Vec<AccountId>,
    attestor_bond: Balance,
) -> Result<Vec<AttestorInfo>, Error> {
    ensure!(members.len() >= MIN_MEMBERS, Error::TooFewMembers);
    for i in 0..members.len() {
        for j in (i + 1)..members.len() {
            ensure!(members[i] != members[j], Error::DuplicateMember);
        }
    }
    // Fresh rows only. `set_members` is the sole production caller and layers
    // the continuity rules of SQ-262 on top of this: it carries `false_count`
    // forward for continuing members and moves departing liabilities into the
    // independent liability vector. The FRAME shell owns the real hold/slash
    // custody; this core only applies the deterministic state transition.
    Ok(members
        .into_iter()
        .map(|account| AttestorInfo {
            account,
            bond: attestor_bond,
            false_count: 0,
            active: true,
        })
        .collect())
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
    use alloc::vec;
    fn acct(n: u8) -> AccountId {
        [n; 32]
    }
    fn members() -> Vec<AccountId> {
        vec![acct(1), acct(2), acct(3)]
    }
    fn params() -> AttestorParams {
        AttestorParams::DEFAULT
    }
    #[test]
    fn membership_values_origin_and_floor() {
        assert_eq!(
            AttestorRegistry::new(vec![acct(1), acct(2)], params()),
            Err(Error::TooFewMembers)
        );
        assert_eq!(
            AttestorRegistry::new(vec![acct(1), acct(1), acct(2)], params()),
            Err(Error::DuplicateMember)
        );
        let mut r = AttestorRegistry::new(members(), params()).unwrap();
        assert_eq!(
            r.set_members(AttestorOrigin::Signed, members(), 0, params()),
            Err(Error::BadOrigin)
        );
    }
    #[test]
    fn two_distinct_attestors_after_window_form_quorum() {
        let mut r = AttestorRegistry::new(members(), params()).unwrap();
        r.attest(acct(1), 9, [7; 32], [8; 32], 0, params()).unwrap();
        r.attest(acct(2), 9, [7; 32], [8; 32], 0, params()).unwrap();
        assert!(!r.has_quorum(9, [7; 32], CHALLENGE_WINDOW_BLOCKS));
        assert!(r.has_quorum(9, [7; 32], CHALLENGE_WINDOW_BLOCKS + 1));
    }
    #[test]
    fn open_challenge_suppresses_quorum_until_upheld() {
        let mut r = AttestorRegistry::new(members(), params()).unwrap();
        let a = r.attest(acct(1), 9, [7; 32], [8; 32], 0, params()).unwrap();
        r.attest(acct(2), 9, [7; 32], [8; 32], 0, params()).unwrap();
        r.challenge_attestation(acct(9), a, [5; 32], CHALLENGE_BOND, 1)
            .unwrap();
        assert!(!r.has_quorum(9, [7; 32], CHALLENGE_WINDOW_BLOCKS + 1));
        r.resolve_challenge(AttestorOrigin::RatifyTrack, a, true)
            .unwrap();
        assert!(r.has_quorum(9, [7; 32], CHALLENGE_WINDOW_BLOCKS + 1));
    }
    #[test]
    fn false_attestation_slashes_and_ejects_on_second_loss() {
        let mut r = AttestorRegistry::new(members(), params()).unwrap();
        let a = r.attest(acct(1), 1, [1; 32], [2; 32], 0, params()).unwrap();
        r.challenge_attestation(acct(9), a, [3; 32], CHALLENGE_BOND, 1)
            .unwrap();
        r.resolve_challenge(AttestorOrigin::RatifyTrack, a, false)
            .unwrap();
        let b = r.attest(acct(1), 2, [1; 32], [2; 32], 2, params()).unwrap();
        r.challenge_attestation(acct(9), b, [3; 32], CHALLENGE_BOND, 3)
            .unwrap();
        r.resolve_challenge(AttestorOrigin::RatifyTrack, b, false)
            .unwrap();
        assert!(!r.is_active_member(acct(1)));
    }
    #[test]
    fn challenge_paths_check_window_and_bond() {
        let mut r = AttestorRegistry::new(members(), params()).unwrap();
        let a = r.attest(acct(1), 1, [1; 32], [2; 32], 0, params()).unwrap();
        assert_eq!(
            r.challenge_attestation(acct(9), a, [3; 32], CHALLENGE_BOND - 1, 1),
            Err(Error::ChallengeBondTooSmall)
        );
        assert_eq!(
            r.challenge_attestation(
                acct(9),
                a,
                [3; 32],
                CHALLENGE_BOND,
                CHALLENGE_WINDOW_BLOCKS + 1,
            ),
            Err(Error::ChallengeWindowClosed)
        );
    }

    #[test]
    fn upheld_attestation_still_waits_for_its_challenge_window() {
        // 06 §7: an upheld challenge proves the attestation valid but does not
        // shortcut the 72h window — quorum still waits for `challenge_deadline`.
        let mut r = AttestorRegistry::new(members(), params()).unwrap();
        r.attest(acct(2), 9, [7; 32], [8; 32], 0, params()).unwrap(); // id 0, deadline = CWB
        r.attest(acct(1), 9, [7; 32], [8; 32], 100, params())
            .unwrap(); // id 1, deadline = 100+CWB
        r.challenge_attestation(acct(9), 1, [5; 32], CHALLENGE_BOND, 101)
            .unwrap();
        r.resolve_challenge(AttestorOrigin::RatifyTrack, 1, true)
            .unwrap(); // upheld early
                       // acct(2)'s window has closed (counts); acct(1) is upheld but its window
                       // is still open, so it does NOT count — quorum is not yet met.
        assert!(!r.has_quorum(9, [7; 32], CHALLENGE_WINDOW_BLOCKS + 1));
        // Once acct(1)'s window closes too, quorum forms.
        assert!(r.has_quorum(9, [7; 32], 100 + CHALLENGE_WINDOW_BLOCKS + 1));
    }

    #[test]
    fn quorum_fails_when_active_registry_drops_below_minimum() {
        // 06 §7 (I-19): "≥ 3 members required before any CODE/META proposal can
        // qualify". Eject one of three attestors, then have the two remaining
        // active members attest with closed windows — quorum must still FAIL
        // because the active registry is below MIN_MEMBERS (A10 Codex PR review).
        let mut r = AttestorRegistry::new(members(), params()).unwrap();
        // Eject acct(1) via two adjudicated-false attestations.
        let a = r.attest(acct(1), 1, [1; 32], [2; 32], 0, params()).unwrap();
        r.challenge_attestation(acct(9), a, [3; 32], CHALLENGE_BOND, 1)
            .unwrap();
        r.resolve_challenge(AttestorOrigin::RatifyTrack, a, false)
            .unwrap();
        let b = r.attest(acct(1), 2, [1; 32], [2; 32], 2, params()).unwrap();
        r.challenge_attestation(acct(9), b, [3; 32], CHALLENGE_BOND, 3)
            .unwrap();
        r.resolve_challenge(AttestorOrigin::RatifyTrack, b, false)
            .unwrap();
        assert!(!r.is_active_member(acct(1)));
        assert_eq!(r.active_member_count(), 2);
        // The two still-active members each attest a CODE artifact; windows close.
        r.attest(acct(2), 9, [7; 32], [8; 32], 0, params()).unwrap();
        r.attest(acct(3), 9, [7; 32], [8; 32], 0, params()).unwrap();
        // Two counting attestations from distinct active members, but the active
        // registry is below MIN_MEMBERS ⇒ the CODE/META gate has no quorum.
        assert!(!r.has_quorum(9, [7; 32], CHALLENGE_WINDOW_BLOCKS + 1));
    }

    #[test]
    fn try_state_rejects_an_ejected_member_marked_active() {
        let mut r = AttestorRegistry::new(members(), params()).unwrap();
        r.members[0].false_count = FALSE_EJECTION_THRESHOLD;
        r.members[0].active = true;

        assert_eq!(r.try_state(), Err(Error::EjectedMemberActive));
    }

    #[test]
    fn worst_case_liability_scan_stays_within_bound() {
        // Mirrors the `pallet-attestor` `set_members` worst-case benchmark
        // (Finding 1). `set_members` runs one `has_unsettled_liability` scan
        // per departing member over the whole flat ledger, so the cost is
        // O(members × attestations). This pins the top of that envelope — a full
        // 16-member roster, a full 256-record ledger, the sole unsettled record
        // owned by a departing member and placed LAST so its scan traverses the
        // entire ledger (no early `any()` exit) — and asserts the seating still
        // completes, retains exactly the liable member (SQ-262), and stays
        // within the 16-member storage bound the pallet enforces.
        const ROSTER_BOUND: usize = 16; // pallet `MAX_ATTESTORS`
        const LEDGER_BOUND: u32 = 256; // pallet `MAX_ATTESTATIONS`
        const SENTINEL: AccountId = [180; 32]; // never a member

        let previous: Vec<AttestorInfo> = (1..=ROSTER_BOUND as u8)
            .map(|i| AttestorInfo {
                account: acct(i),
                bond: ATTESTOR_BOND,
                false_count: 0,
                active: true,
            })
            .collect();
        let mut attestations = Vec::new();
        for id in 0..LEDGER_BOUND - 1 {
            // Sentinel-owned: matches no departing member, so every member's
            // scan runs to the end of the ledger.
            attestations.push(Attestation {
                id,
                pid: id as ProposalId,
                artifact_hash: [id as u8; 32],
                statement_hash: [7; 32],
                attestor: SENTINEL,
                submitted_at: 0,
                challenge_deadline: CHALLENGE_WINDOW_BLOCKS,
                challenge: None,
            });
        }
        attestations.push(Attestation {
            id: LEDGER_BOUND - 1,
            pid: (LEDGER_BOUND - 1) as ProposalId,
            artifact_hash: [255; 32],
            statement_hash: [7; 32],
            attestor: acct(1),
            submitted_at: 0,
            challenge_deadline: CHALLENGE_WINDOW_BLOCKS,
            challenge: Some(ChallengeStatus::Open {
                challenger: acct(250),
                evidence_hash: [9; 32],
                bond: CHALLENGE_BOND,
            }),
        });
        let mut r = AttestorRegistry {
            members: previous,
            liabilities: Vec::new(),
            attestations,
            revocations: Vec::new(),
            next_attestation_id: LEDGER_BOUND,
            events: Vec::new(),
        };

        // New roster disjoint from [1..=16], so all 16 are departing and rescanned.
        let fresh: Vec<AccountId> = (17u8..=31).map(acct).collect();
        assert_eq!(fresh.len(), ROSTER_BOUND - 1);
        r.set_members(AttestorOrigin::ConstitutionalValues, fresh, 0, params())
            .unwrap();

        // 15 new + exactly one independent liability for the departing member.
        assert_eq!(r.members.len(), ROSTER_BOUND - 1);
        assert!(r
            .liabilities
            .iter()
            .any(|liability| liability.account == acct(1)));
        assert!(!r
            .liabilities
            .iter()
            .any(|liability| liability.account == acct(2)));
        assert!(r.members.len() <= ROSTER_BOUND);
        assert_eq!(r.try_state(), Ok(()));
    }

    #[test]
    fn cause_removal_revokes_unexecuted_records_and_reap_releases_liability() {
        let mut r =
            AttestorRegistry::new(vec![acct(1), acct(2), acct(3), acct(4)], params()).unwrap();
        r.attest(acct(1), 7, [7; 32], [8; 32], 0, params()).unwrap();
        r.remove_for_cause(
            AttestorOrigin::ConstitutionalValues,
            acct(1),
            [55; 32],
            |_| false,
        )
        .unwrap();
        assert!(!r.is_active_member(acct(1)));
        assert!(r.is_revoked(0));
        assert_eq!(r.liabilities.len(), 1);
        assert!(!r.has_record_quorum(7, [7; 32], CHALLENGE_WINDOW_BLOCKS + 1));

        let released = r
            .reap_attestation(0, CHALLENGE_WINDOW_BLOCKS + 1, |_| true)
            .unwrap();
        assert_eq!(released.unwrap().account, acct(1));
        assert!(r.attestations.is_empty());
        assert!(r.liabilities.is_empty());
        assert!(r.revocations.is_empty());
        assert_eq!(r.try_state(), Ok(()));
    }

    #[test]
    fn execute_record_quorum_survives_roster_rotation_without_revocation() {
        let mut r = AttestorRegistry::new(members(), params()).unwrap();
        r.attest(acct(1), 8, [8; 32], [1; 32], 0, params()).unwrap();
        r.attest(acct(2), 8, [8; 32], [2; 32], 0, params()).unwrap();
        r.set_members(
            AttestorOrigin::ConstitutionalValues,
            vec![acct(3), acct(4), acct(5)],
            CHALLENGE_WINDOW_BLOCKS + 1,
            params(),
        )
        .unwrap();
        assert!(!r.has_quorum(8, [8; 32], CHALLENGE_WINDOW_BLOCKS + 1));
        assert!(r.has_record_quorum(8, [8; 32], CHALLENGE_WINDOW_BLOCKS + 1));
    }
}

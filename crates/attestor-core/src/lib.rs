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
    Overflow,
}

#[derive(Clone, Debug, Decode, Encode, Eq, PartialEq, TypeInfo)]
pub struct AttestorRegistry {
    pub members: Vec<AttestorInfo>,
    pub attestations: Vec<Attestation>,
    pub next_attestation_id: AttestationId,
    pub events: Vec<Event>,
}

impl AttestorRegistry {
    pub fn new(members: Vec<AccountId>, params: AttestorParams) -> Result<Self, Error> {
        let infos = validate_and_infos(members, params.bond)?;
        Ok(Self {
            members: infos,
            attestations: Vec::new(),
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
    /// * A departing member that still carries an **unsettled liability** — an
    ///   open challenge, or an attestation whose challenge window has not closed
    ///   — is **retained as an inactive row**. Governance authority ends
    ///   immediately (an inactive member cannot attest and never counts toward
    ///   quorum or the active registry floor), but the slash basis the adverse
    ///   verdict reads survives until the record settles. Dropping the row
    ///   instead left the challenge permanently unresolvable (`NotMember` on the
    ///   adverse branch), which both suppressed the attestation from quorum
    ///   forever and shielded the attestor from the strike a for-cause recall is
    ///   meant to record.
    ///
    /// Retention is bounded by open windows, so retained rows drain on their own;
    /// a seating whose result would exceed the registry's storage bound is
    /// rejected whole by the FRAME shell (G-1).
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
        for previous in self.members.iter() {
            let seated = infos.iter().any(|info| info.account == previous.account);
            if seated || !self.has_unsettled_liability(previous.account, now) {
                continue;
            }
            let mut retained = *previous;
            retained.active = false;
            infos.push(retained);
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
        let stored_bond = self
            .members
            .iter()
            .find(|member| member.account == attestor)
            .map(|member| member.bond)
            .ok_or(Error::NotMember)?;
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
            let stored_bond = self
                .members
                .iter()
                .find(|member| member.account == loser)
                .map(|member| member.bond)
                .ok_or(Error::NotMember)?;
            half_ceil(stored_bond)
        };
        if attestation_upheld {
            self.attestations[idx].challenge = Some(ChallengeStatus::Upheld);
        } else {
            self.attestations[idx].challenge = Some(ChallengeStatus::Rejected);
            let attestor = self.attestations[idx].attestor;
            if let Some(info) = self.members.iter_mut().find(|m| m.account == attestor) {
                info.bond = info.bond.saturating_sub(slashed);
                info.false_count = info.false_count.saturating_add(1);
                if info.false_count >= FALSE_EJECTION_THRESHOLD {
                    info.active = false;
                    self.events.push(Event::AttestorEjected { who: attestor });
                }
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
            if distinct.contains(&att.attestor) || !self.is_active_member(att.attestor) {
                continue;
            }
            if self.attestation_counts(att, now) {
                distinct.push(att.attestor);
            }
        }
        distinct.len() >= QUORUM
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
        ensure!(self.members.len() >= MIN_MEMBERS, Error::TooFewMembers);
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
        // Every open challenge must still have a slash basis to resolve against
        // (SQ-262): seating retains a liable member as an inactive row, so an
        // `Open` challenge whose attestor has no row at all is unresolvable on
        // its adverse branch and would strand the record permanently.
        for attestation in self.attestations.iter() {
            if matches!(attestation.challenge, Some(ChallengeStatus::Open { .. })) {
                ensure!(
                    self.members
                        .iter()
                        .any(|member| member.account == attestation.attestor),
                    Error::NotMember
                );
            }
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
    // forward for continuing members and retains departing members that still
    // carry an unsettled liability. What remains deferred to the real
    // per-account bond system (B-track, PLAN SQ-263/SQ-293) is *custody* —
    // `bond` here is still an arithmetic magnitude, not value held.
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
}

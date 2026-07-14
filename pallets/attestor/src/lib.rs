#![cfg_attr(not(feature = "std"), no_std)]
#![deny(unsafe_code)]

extern crate alloc;

use alloc::vec::Vec;
use futarchy_primitives::{AccountId, Balance, BlockNumber, ProposalId, H256};
use parity_scale_codec::{Decode, Encode, MaxEncodedLen};
use scale_info::TypeInfo;

pub type AttestationId = u32;
pub const MIN_MEMBERS: usize = 3;
pub const QUORUM: usize = 2;
pub const ATTESTOR_BOND: Balance = 25_000_000_000_000_000;
pub const CHALLENGE_WINDOW_BLOCKS: BlockNumber = 43_200;
pub const CHALLENGE_BOND: Balance = ATTESTOR_BOND / 2;
pub const FALSE_EJECTION_THRESHOLD: u8 = 2;

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub enum AttestorOrigin {
    Signed,
    ConstitutionalValues,
    RatifyTrack,
}

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub enum ChallengeStatus {
    Open {
        challenger: AccountId,
        evidence_hash: H256,
        bond: Balance,
    },
    Upheld,
    Rejected,
}

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub struct AttestorInfo {
    pub account: AccountId,
    pub bond: Balance,
    pub false_count: u8,
    pub active: bool,
}

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
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
    pub fn new(members: Vec<AccountId>) -> Result<Self, Error> {
        let infos = validate_and_infos(members)?;
        Ok(Self {
            members: infos,
            attestations: Vec::new(),
            next_attestation_id: 0,
            events: Vec::new(),
        })
    }
    pub fn set_members(
        &mut self,
        origin: AttestorOrigin,
        members: Vec<AccountId>,
    ) -> Result<(), Error> {
        ensure!(
            matches!(origin, AttestorOrigin::ConstitutionalValues),
            Error::BadOrigin
        );
        let infos = validate_and_infos(members.clone())?;
        self.members = infos;
        self.events.push(Event::MembersSet { members });
        Ok(())
    }
    pub fn attest(
        &mut self,
        who: AccountId,
        pid: ProposalId,
        artifact_hash: H256,
        statement_hash: H256,
        now: BlockNumber,
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
                .checked_add(CHALLENGE_WINDOW_BLOCKS)
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
        ensure!(bond >= CHALLENGE_BOND, Error::ChallengeBondTooSmall);
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
            _ => return Err(Error::AttestationNotFound),
        };
        let loser = if attestation_upheld {
            challenger
        } else {
            self.attestations[idx].attestor
        };
        let slashed = if attestation_upheld {
            bond / 2
        } else {
            ATTESTOR_BOND / 2
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
        for att in &self.attestations {
            ensure!(
                self.members.iter().any(|m| m.account == att.attestor),
                Error::NotMember
            );
        }
        Ok(())
    }
    fn attestation_counts(&self, att: &Attestation, now: BlockNumber) -> bool {
        match att.challenge {
            None => now > att.challenge_deadline,
            Some(ChallengeStatus::Upheld) => true,
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
}

fn validate_and_infos(members: Vec<AccountId>) -> Result<Vec<AttestorInfo>, Error> {
    ensure!(members.len() >= MIN_MEMBERS, Error::TooFewMembers);
    for i in 0..members.len() {
        for j in (i + 1)..members.len() {
            ensure!(members[i] != members[j], Error::DuplicateMember);
        }
    }
    Ok(members
        .into_iter()
        .map(|account| AttestorInfo {
            account,
            bond: ATTESTOR_BOND,
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
    #[test]
    fn membership_values_origin_and_floor() {
        assert_eq!(
            AttestorRegistry::new(vec![acct(1), acct(2)]),
            Err(Error::TooFewMembers)
        );
        assert_eq!(
            AttestorRegistry::new(vec![acct(1), acct(1), acct(2)]),
            Err(Error::DuplicateMember)
        );
        let mut r = AttestorRegistry::new(members()).unwrap();
        assert_eq!(
            r.set_members(AttestorOrigin::Signed, members()),
            Err(Error::BadOrigin)
        );
    }
    #[test]
    fn two_distinct_attestors_after_window_form_quorum() {
        let mut r = AttestorRegistry::new(members()).unwrap();
        r.attest(acct(1), 9, [7; 32], [8; 32], 0).unwrap();
        r.attest(acct(2), 9, [7; 32], [8; 32], 0).unwrap();
        assert!(!r.has_quorum(9, [7; 32], CHALLENGE_WINDOW_BLOCKS));
        assert!(r.has_quorum(9, [7; 32], CHALLENGE_WINDOW_BLOCKS + 1));
    }
    #[test]
    fn open_challenge_suppresses_quorum_until_upheld() {
        let mut r = AttestorRegistry::new(members()).unwrap();
        let a = r.attest(acct(1), 9, [7; 32], [8; 32], 0).unwrap();
        r.attest(acct(2), 9, [7; 32], [8; 32], 0).unwrap();
        r.challenge_attestation(acct(9), a, [5; 32], CHALLENGE_BOND, 1)
            .unwrap();
        assert!(!r.has_quorum(9, [7; 32], CHALLENGE_WINDOW_BLOCKS + 1));
        r.resolve_challenge(AttestorOrigin::RatifyTrack, a, true)
            .unwrap();
        assert!(r.has_quorum(9, [7; 32], CHALLENGE_WINDOW_BLOCKS + 1));
    }
    #[test]
    fn false_attestation_slashes_and_ejects_on_second_loss() {
        let mut r = AttestorRegistry::new(members()).unwrap();
        let a = r.attest(acct(1), 1, [1; 32], [2; 32], 0).unwrap();
        r.challenge_attestation(acct(9), a, [3; 32], CHALLENGE_BOND, 1)
            .unwrap();
        r.resolve_challenge(AttestorOrigin::RatifyTrack, a, false)
            .unwrap();
        let b = r.attest(acct(1), 2, [1; 32], [2; 32], 2).unwrap();
        r.challenge_attestation(acct(9), b, [3; 32], CHALLENGE_BOND, 3)
            .unwrap();
        r.resolve_challenge(AttestorOrigin::RatifyTrack, b, false)
            .unwrap();
        assert!(!r.is_active_member(acct(1)));
    }
    #[test]
    fn challenge_paths_check_window_and_bond() {
        let mut r = AttestorRegistry::new(members()).unwrap();
        let a = r.attest(acct(1), 1, [1; 32], [2; 32], 0).unwrap();
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
                CHALLENGE_WINDOW_BLOCKS + 1
            ),
            Err(Error::ChallengeWindowClosed)
        );
    }
}

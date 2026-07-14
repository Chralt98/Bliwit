#![cfg_attr(not(feature = "std"), no_std)]
#![deny(unsafe_code)]

extern crate alloc;

use alloc::vec::Vec;
use futarchy_primitives::{
    AccountId, Balance, BlockNumber, EpochId, FixedU64, MetricSpecVersion, H256,
};
use parity_scale_codec::{Decode, Encode, MaxEncodedLen};
use scale_info::TypeInfo;

pub type FilingId = u32;

pub const MAX_FILINGS_PER_EPOCH: u32 = 64;
pub const MAX_LIVE_EPOCHS: usize = 4;
pub const MAX_AGGREGATES: usize = 4;
pub const REG_CLOSE_BATCH: usize = 20;
pub const REG_WINDOW_BLOCKS: BlockNumber = 43_200;
pub const REG_EXT_WINDOW_BLOCKS: BlockNumber = 28_800;
pub const REPORT_WINDOW_BLOCKS: BlockNumber = 28_800;
pub const REG_BOND_INCIDENT: Balance = 5_000_000_000;
pub const REG_BOND_MILESTONE: Balance = 2_500_000_000;
pub const WT_QUORUM: u8 = 2;
pub const MILESTONE_TARGET_POINTS: u32 = 100;
const ONE: u64 = 1_000_000_000;

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub enum RegistryKind {
    Incident,
    Milestone,
}

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub enum FilingClass {
    S1,
    S2,
    S3,
    Scope(u8),
}

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub enum FilingState {
    Filed {
        window_end: BlockNumber,
        extended: bool,
        acks: u8,
    },
    Challenged {
        round: u8,
        window_end: BlockNumber,
        challenger: AccountId,
        evidence_hash: H256,
    },
    Upheld,
    Rejected,
}

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub struct Filing {
    pub who: AccountId,
    pub class: FilingClass,
    pub points: u16,
    pub evidence_hash: H256,
    pub bond: Balance,
    pub state: FilingState,
    pub spec_version: MetricSpecVersion,
}

#[derive(Clone, Debug, Decode, Encode, Eq, PartialEq, TypeInfo)]
pub enum Event {
    IncidentFiled {
        epoch: EpochId,
        filing_id: FilingId,
        who: AccountId,
        class: FilingClass,
        evidence_hash: H256,
        bond: Balance,
    },
    MilestoneFiled {
        epoch: EpochId,
        filing_id: FilingId,
        who: AccountId,
        class: FilingClass,
        points: u16,
        evidence_hash: H256,
        bond: Balance,
    },
    IncidentChallenged {
        epoch: EpochId,
        filing_id: FilingId,
        challenger: AccountId,
        evidence_hash: H256,
        bond: Balance,
    },
    MilestoneChallenged {
        epoch: EpochId,
        filing_id: FilingId,
        challenger: AccountId,
        evidence_hash: H256,
        bond: Balance,
    },
    IncidentUpheld {
        epoch: EpochId,
        filing_id: FilingId,
    },
    IncidentRejected {
        epoch: EpochId,
        filing_id: FilingId,
    },
    MilestoneAccepted {
        epoch: EpochId,
        filing_id: FilingId,
    },
    MilestoneRejected {
        epoch: EpochId,
        filing_id: FilingId,
    },
    FilingBondSlashed {
        epoch: EpochId,
        filing_id: FilingId,
        loser: AccountId,
        amount: Balance,
        challenger_share: Balance,
        insurance_share: Balance,
    },
    RegistryEpochClosed {
        kind: RegistryKind,
        epoch: EpochId,
        aggregate: FixedU64,
    },
    WindowAcknowledged {
        epoch: EpochId,
        filing_id: FilingId,
        watchtower: AccountId,
    },
    WindowExtended {
        epoch: EpochId,
        filing_id: FilingId,
        new_deadline: BlockNumber,
    },
}

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub enum Error {
    EpochFull,
    TooManyLiveEpochs,
    TooManyAggregates,
    WindowClosed,
    WindowOpen,
    AlreadyChallenged,
    AlreadyFinal,
    SpecVersionMismatch,
    BondBelowMinimum,
    FilingNotFound,
    DuplicateAck,
    BatchTooLarge,
    InvalidClass,
    Overflow,
    NotRegistered,
}

#[derive(Clone, Debug, Decode, Encode, Eq, PartialEq, TypeInfo)]
pub struct FileInput {
    pub who: AccountId,
    pub now: BlockNumber,
    pub epoch: EpochId,
    pub class: FilingClass,
    pub points: u16,
    pub evidence_hash: H256,
    pub spec_version: MetricSpecVersion,
    pub expected_spec: MetricSpecVersion,
    pub filing_window_end: BlockNumber,
}

#[derive(Clone, Debug, Decode, Encode, Eq, PartialEq, TypeInfo)]
pub struct Registry {
    pub kind: RegistryKind,
    pub filings: Vec<((EpochId, FilingId), Filing)>,
    pub filing_count: Vec<(EpochId, u32)>,
    pub aggregates: Vec<(EpochId, FixedU64)>,
    pub events: Vec<Event>,
    ack_records: Vec<(EpochId, FilingId, AccountId)>,
}

impl Registry {
    pub fn new(kind: RegistryKind) -> Self {
        Self {
            kind,
            filings: Vec::new(),
            filing_count: Vec::new(),
            aggregates: Vec::new(),
            events: Vec::new(),
            ack_records: Vec::new(),
        }
    }

    pub fn file(&mut self, input: FileInput) -> Result<FilingId, Error> {
        ensure!(input.now <= input.filing_window_end, Error::WindowClosed);
        ensure!(
            input.spec_version == input.expected_spec,
            Error::SpecVersionMismatch
        );
        // A closed-out epoch's aggregate is terminal (07 §7): late filings
        // must not land behind an already-derived welfare input.
        ensure!(
            self.aggregates.iter().all(|(e, _)| *e != input.epoch),
            Error::AlreadyFinal
        );
        self.validate_class(input.class)?;
        let bond = self.required_bond();
        ensure!(bond > 0, Error::BondBelowMinimum);
        if self.filing_count.iter().all(|(e, _)| *e != input.epoch) {
            ensure!(
                self.filing_count.len() < MAX_LIVE_EPOCHS,
                Error::TooManyLiveEpochs
            );
            self.filing_count.push((input.epoch, 0));
        }
        let count = self
            .filing_count
            .iter_mut()
            .find(|(e, _)| *e == input.epoch)
            .ok_or(Error::Overflow)?;
        ensure!(count.1 < MAX_FILINGS_PER_EPOCH, Error::EpochFull);
        let filing_id = count.1;
        count.1 = count.1.checked_add(1).ok_or(Error::Overflow)?;
        let filing = Filing {
            who: input.who,
            class: input.class,
            points: input.points,
            evidence_hash: input.evidence_hash,
            bond,
            state: FilingState::Filed {
                window_end: input.now.saturating_add(REG_WINDOW_BLOCKS),
                extended: false,
                acks: 0,
            },
            spec_version: input.spec_version,
        };
        self.filings.push(((input.epoch, filing_id), filing));
        match self.kind {
            RegistryKind::Incident => self.events.push(Event::IncidentFiled {
                epoch: input.epoch,
                filing_id,
                who: input.who,
                class: input.class,
                evidence_hash: input.evidence_hash,
                bond,
            }),
            RegistryKind::Milestone => self.events.push(Event::MilestoneFiled {
                epoch: input.epoch,
                filing_id,
                who: input.who,
                class: input.class,
                points: input.points,
                evidence_hash: input.evidence_hash,
                bond,
            }),
        }
        Ok(filing_id)
    }

    /// Watchtower acknowledgment (07 §4 quorum rule, inherited by the registry
    /// per 07 §7). `is_registered_watchtower` is resolved by the runtime
    /// against the oracle pallet's bonded watchtower registry — only bonded,
    /// slashable seats may count toward `WT_QUORUM`.
    pub fn ack_observed(
        &mut self,
        who: AccountId,
        now: BlockNumber,
        is_registered_watchtower: bool,
        epoch: EpochId,
        filing_id: FilingId,
    ) -> Result<(), Error> {
        ensure!(is_registered_watchtower, Error::NotRegistered);
        ensure!(
            !self.ack_records.contains(&(epoch, filing_id, who)),
            Error::DuplicateAck
        );
        let filing = self.filing_mut(epoch, filing_id)?;
        match &mut filing.state {
            FilingState::Filed {
                window_end, acks, ..
            } => {
                // Quorum proves observability during the live challenge
                // window; late acknowledgments must not retro-uphold a filing
                // whose challenge surface already closed.
                ensure!(now <= *window_end, Error::WindowClosed);
                *acks = acks.saturating_add(1);
            }
            FilingState::Challenged { window_end, .. } => {
                ensure!(now <= *window_end, Error::WindowClosed);
            }
            _ => return Err(Error::AlreadyFinal),
        }
        self.ack_records.push((epoch, filing_id, who));
        self.events.push(Event::WindowAcknowledged {
            epoch,
            filing_id,
            watchtower: who,
        });
        Ok(())
    }

    pub fn challenge_filing(
        &mut self,
        who: AccountId,
        now: BlockNumber,
        epoch: EpochId,
        filing_id: FilingId,
        evidence_hash: H256,
    ) -> Result<(), Error> {
        let filing = self.filing_mut(epoch, filing_id)?;
        let window_end = match filing.state {
            FilingState::Filed { window_end, .. } => window_end,
            FilingState::Challenged { .. } => return Err(Error::AlreadyChallenged),
            _ => return Err(Error::AlreadyFinal),
        };
        ensure!(now <= window_end, Error::WindowClosed);
        let bond = filing.bond;
        filing.state = FilingState::Challenged {
            round: 2,
            window_end: now.saturating_add(REG_WINDOW_BLOCKS),
            challenger: who,
            evidence_hash,
        };
        match self.kind {
            RegistryKind::Incident => self.events.push(Event::IncidentChallenged {
                epoch,
                filing_id,
                challenger: who,
                evidence_hash,
                bond,
            }),
            RegistryKind::Milestone => self.events.push(Event::MilestoneChallenged {
                epoch,
                filing_id,
                challenger: who,
                evidence_hash,
                bond,
            }),
        }
        Ok(())
    }

    pub fn resolve_challenge(
        &mut self,
        epoch: EpochId,
        filing_id: FilingId,
        uphold: bool,
    ) -> Result<(), Error> {
        let (who, challenger, bond) = {
            let f = self.filing_mut(epoch, filing_id)?;
            let challenger = match f.state {
                FilingState::Challenged { challenger, .. } => challenger,
                _ => return Err(Error::WindowOpen),
            };
            f.state = if uphold {
                FilingState::Upheld
            } else {
                FilingState::Rejected
            };
            (f.who, challenger, f.bond)
        };
        let loser = if uphold { challenger } else { who };
        self.push_terminal_event(epoch, filing_id, uphold);
        self.push_slash(epoch, filing_id, loser, bond);
        Ok(())
    }

    pub fn crank_close(&mut self, now: BlockNumber, batch: usize) -> Result<(), Error> {
        ensure!(batch <= REG_CLOSE_BATCH, Error::BatchTooLarge);
        let mut processed = 0usize;
        let keys: Vec<(EpochId, FilingId)> = self.filings.iter().map(|(k, _)| *k).collect();
        for (epoch, filing_id) in keys {
            if processed >= batch {
                break;
            }
            let mut terminal: Option<bool> = None;
            let mut extend_to: Option<BlockNumber> = None;
            {
                let f = self.filing_mut(epoch, filing_id)?;
                match &mut f.state {
                    FilingState::Filed {
                        window_end,
                        extended,
                        acks,
                    } => {
                        if now < *window_end {
                            continue;
                        }
                        if *acks >= WT_QUORUM {
                            terminal = Some(true);
                        } else if !*extended {
                            *extended = true;
                            *window_end = now.saturating_add(REG_EXT_WINDOW_BLOCKS);
                            extend_to = Some(*window_end);
                        } else {
                            terminal = Some(false);
                        }
                    }
                    FilingState::Challenged { window_end, .. } => {
                        if now < *window_end {
                            continue;
                        }
                        continue;
                    }
                    _ => continue,
                }
            }
            if let Some(new_deadline) = extend_to {
                self.events.push(Event::WindowExtended {
                    epoch,
                    filing_id,
                    new_deadline,
                });
            }
            if let Some(uphold) = terminal {
                self.filing_mut(epoch, filing_id)?.state = if uphold {
                    FilingState::Upheld
                } else {
                    FilingState::Rejected
                };
                self.push_terminal_event(epoch, filing_id, uphold);
            }
            processed += 1;
        }
        Ok(())
    }

    /// Close-out (07 §7): derives the epoch's aggregate exactly once, only
    /// after the filing window has ended and every filing is terminal. The
    /// `FilingCount` entry is reaped here — it exists to allocate ids and
    /// enforce the per-epoch cap during the window, and holding it past
    /// close-out would wedge the ≤ 4-live-epoch bound permanently.
    pub fn close_epoch(
        &mut self,
        epoch: EpochId,
        now: BlockNumber,
        filing_window_end: BlockNumber,
    ) -> Result<FixedU64, Error> {
        ensure!(now > filing_window_end, Error::WindowOpen);
        ensure!(
            self.aggregates.iter().all(|(e, _)| *e != epoch),
            Error::AlreadyFinal
        );
        ensure!(
            self.filings
                .iter()
                .filter(|((e, _), _)| *e == epoch)
                .all(|(_, f)| matches!(f.state, FilingState::Upheld | FilingState::Rejected)),
            Error::WindowOpen
        );
        let aggregate = match self.kind {
            RegistryKind::Incident => self.incident_aggregate(epoch),
            RegistryKind::Milestone => self.milestone_aggregate(epoch),
        };
        ensure!(
            self.aggregates.len() < MAX_AGGREGATES,
            Error::TooManyAggregates
        );
        self.aggregates.push((epoch, aggregate));
        self.filing_count.retain(|(e, _)| *e != epoch);
        self.events.push(Event::RegistryEpochClosed {
            kind: self.kind,
            epoch,
            aggregate,
        });
        Ok(aggregate)
    }

    /// Reap a closed epoch's records. 07 §7: closed epochs are reaped at
    /// cohort settlement + archive delay — the caller (welfare/epoch wiring)
    /// enforces that timing; the registry only requires that the epoch was
    /// closed out first.
    pub fn reap_epoch(&mut self, epoch: EpochId) -> Result<(), Error> {
        ensure!(
            self.aggregates.iter().any(|(e, _)| *e == epoch),
            Error::WindowOpen
        );
        self.filings.retain(|((e, _), _)| *e != epoch);
        self.ack_records.retain(|(e, _, _)| *e != epoch);
        self.aggregates.retain(|(e, _)| *e != epoch);
        Ok(())
    }

    pub fn try_state(&self) -> Result<(), Error> {
        ensure!(
            self.filing_count.len() <= MAX_LIVE_EPOCHS,
            Error::TooManyLiveEpochs
        );
        ensure!(
            self.aggregates.len() <= MAX_AGGREGATES,
            Error::TooManyAggregates
        );
        for (epoch, count) in &self.filing_count {
            ensure!(*count <= MAX_FILINGS_PER_EPOCH, Error::EpochFull);
            let actual = self.filings.iter().filter(|((e, _), _)| e == epoch).count() as u32;
            ensure!(actual <= *count, Error::Overflow);
        }
        for ((epoch, _), f) in &self.filings {
            ensure!(f.bond >= self.required_bond(), Error::BondBelowMinimum);
            self.validate_class(f.class)?;
            // Every retained filing belongs to exactly one lifecycle stage:
            // a live epoch (counted) or a closed epoch awaiting reap
            // (aggregated) - never both, never neither.
            let live = self.filing_count.iter().any(|(e, _)| e == epoch);
            let closed = self.aggregates.iter().any(|(e, _)| e == epoch);
            ensure!(live != closed, Error::Overflow);
            if closed {
                ensure!(
                    matches!(f.state, FilingState::Upheld | FilingState::Rejected),
                    Error::WindowOpen
                );
            }
        }
        Ok(())
    }

    pub fn aggregate(&self, epoch: EpochId) -> Option<FixedU64> {
        self.aggregates
            .iter()
            .find(|(e, _)| *e == epoch)
            .map(|(_, v)| *v)
    }

    fn required_bond(&self) -> Balance {
        match self.kind {
            RegistryKind::Incident => REG_BOND_INCIDENT,
            RegistryKind::Milestone => REG_BOND_MILESTONE,
        }
    }
    fn validate_class(&self, class: FilingClass) -> Result<(), Error> {
        match (self.kind, class) {
            (RegistryKind::Incident, FilingClass::S1 | FilingClass::S2 | FilingClass::S3) => Ok(()),
            (RegistryKind::Milestone, FilingClass::Scope(_)) => Ok(()),
            _ => Err(Error::InvalidClass),
        }
    }
    fn filing_mut(&mut self, epoch: EpochId, filing_id: FilingId) -> Result<&mut Filing, Error> {
        self.filings
            .iter_mut()
            .find(|((e, id), _)| *e == epoch && *id == filing_id)
            .map(|(_, f)| f)
            .ok_or(Error::FilingNotFound)
    }
    fn push_terminal_event(&mut self, epoch: EpochId, filing_id: FilingId, uphold: bool) {
        match (self.kind, uphold) {
            (RegistryKind::Incident, true) => {
                self.events.push(Event::IncidentUpheld { epoch, filing_id })
            }
            (RegistryKind::Incident, false) => self
                .events
                .push(Event::IncidentRejected { epoch, filing_id }),
            (RegistryKind::Milestone, true) => self
                .events
                .push(Event::MilestoneAccepted { epoch, filing_id }),
            (RegistryKind::Milestone, false) => self
                .events
                .push(Event::MilestoneRejected { epoch, filing_id }),
        }
    }
    fn push_slash(
        &mut self,
        epoch: EpochId,
        filing_id: FilingId,
        loser: AccountId,
        amount: Balance,
    ) {
        self.events.push(Event::FilingBondSlashed {
            epoch,
            filing_id,
            loser,
            amount,
            challenger_share: amount.saturating_mul(40) / 100,
            insurance_share: amount.saturating_mul(60) / 100,
        });
    }
    fn incident_aggregate(&self, epoch: EpochId) -> FixedU64 {
        let sev: u64 = self
            .filings
            .iter()
            .filter(|((e, _), f)| *e == epoch && matches!(f.state, FilingState::Upheld))
            .map(|(_, f)| match f.class {
                FilingClass::S1 => ONE,
                FilingClass::S2 => 400_000_000,
                FilingClass::S3 => 100_000_000,
                FilingClass::Scope(_) => 0,
            })
            .sum();
        FixedU64(ONE.saturating_sub(sev))
    }
    fn milestone_aggregate(&self, epoch: EpochId) -> FixedU64 {
        let points: u32 = self
            .filings
            .iter()
            .filter(|((e, _), f)| *e == epoch && matches!(f.state, FilingState::Upheld))
            .map(|(_, f)| f.points as u32)
            .sum();
        FixedU64(((points as u64).saturating_mul(ONE)) / MILESTONE_TARGET_POINTS as u64)
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::new(RegistryKind::Incident)
    }
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
    pub fn benchmark_stub() {}
}

#[cfg(test)]
mod tests {
    use super::*;
    fn acct(n: u8) -> AccountId {
        [n; 32]
    }
    fn h(n: u8) -> H256 {
        [n; 32]
    }
    fn file_input(
        kind: RegistryKind,
        who: AccountId,
        epoch: EpochId,
        class: FilingClass,
    ) -> FileInput {
        FileInput {
            who,
            now: 1,
            epoch,
            class,
            points: if kind == RegistryKind::Milestone {
                25
            } else {
                0
            },
            evidence_hash: h(9),
            spec_version: 3,
            expected_spec: 3,
            filing_window_end: 10,
        }
    }

    #[test]
    fn incident_filings_are_bounded_and_class_checked() {
        let mut r = Registry::new(RegistryKind::Incident);
        assert_eq!(
            r.file(file_input(
                RegistryKind::Incident,
                acct(1),
                7,
                FilingClass::Scope(1)
            )),
            Err(Error::InvalidClass)
        );
        for i in 0..MAX_FILINGS_PER_EPOCH {
            assert_eq!(
                r.file(file_input(
                    RegistryKind::Incident,
                    acct(i as u8),
                    7,
                    FilingClass::S3
                )),
                Ok(i)
            );
        }
        assert_eq!(
            r.file(file_input(
                RegistryKind::Incident,
                acct(99),
                7,
                FilingClass::S3
            )),
            Err(Error::EpochFull)
        );
        assert_eq!(r.try_state(), Ok(()));
    }

    #[test]
    fn filing_enforces_window_and_spec_version() {
        let mut r = Registry::new(RegistryKind::Milestone);
        let mut input = file_input(RegistryKind::Milestone, acct(1), 1, FilingClass::Scope(2));
        input.now = 11;
        assert_eq!(r.file(input.clone()), Err(Error::WindowClosed));
        input.now = 1;
        input.spec_version = 4;
        assert_eq!(r.file(input), Err(Error::SpecVersionMismatch));
    }

    #[test]
    fn unchallenged_filing_needs_quorum_or_rejects_after_extension() {
        let mut r = Registry::new(RegistryKind::Incident);
        let id = r
            .file(file_input(
                RegistryKind::Incident,
                acct(1),
                5,
                FilingClass::S2,
            ))
            .unwrap();
        r.ack_observed(acct(2), 5, true, 5, id).unwrap();
        r.crank_close(REG_WINDOW_BLOCKS + 2, REG_CLOSE_BATCH)
            .unwrap();
        assert!(matches!(
            r.filings[0].1.state,
            FilingState::Filed { extended: true, .. }
        ));
        r.crank_close(
            REG_WINDOW_BLOCKS + REG_EXT_WINDOW_BLOCKS + 3,
            REG_CLOSE_BATCH,
        )
        .unwrap();
        assert!(matches!(r.filings[0].1.state, FilingState::Rejected));
        assert!(r
            .events
            .iter()
            .any(|e| matches!(e, Event::IncidentRejected { .. })));
    }

    #[test]
    fn quorum_upholds_and_incident_aggregate_is_claimant_adverse() {
        let mut r = Registry::new(RegistryKind::Incident);
        let id = r
            .file(file_input(
                RegistryKind::Incident,
                acct(1),
                5,
                FilingClass::S2,
            ))
            .unwrap();
        r.ack_observed(acct(2), 5, true, 5, id).unwrap();
        r.ack_observed(acct(3), 6, true, 5, id).unwrap();
        r.crank_close(REG_WINDOW_BLOCKS + 2, REG_CLOSE_BATCH)
            .unwrap();
        assert!(matches!(r.filings[0].1.state, FilingState::Upheld));
        assert_eq!(
            r.close_epoch(5, REG_WINDOW_BLOCKS + 3, 10),
            Ok(FixedU64(600_000_000))
        );
    }

    #[test]
    fn challenge_has_one_counter_round_and_slashes_loser() {
        let mut r = Registry::new(RegistryKind::Milestone);
        let id = r
            .file(file_input(
                RegistryKind::Milestone,
                acct(1),
                9,
                FilingClass::Scope(1),
            ))
            .unwrap();
        r.challenge_filing(acct(4), 2, 9, id, h(7)).unwrap();
        assert_eq!(
            r.challenge_filing(acct(5), 3, 9, id, h(8)),
            Err(Error::AlreadyChallenged)
        );
        r.resolve_challenge(9, id, false).unwrap();
        assert!(matches!(r.filings[0].1.state, FilingState::Rejected));
        assert!(r
            .events
            .iter()
            .any(|e| matches!(e, Event::MilestoneRejected { .. })));
        assert!(r.events.iter().any(|e| matches!(e, Event::FilingBondSlashed { amount: REG_BOND_MILESTONE, challenger_share, insurance_share, .. } if *challenger_share == REG_BOND_MILESTONE * 40 / 100 && *insurance_share == REG_BOND_MILESTONE * 60 / 100)));
    }

    #[test]
    fn acks_require_registered_watchtowers_and_a_live_window() {
        // Codex review, PR #20: two arbitrary accounts must not satisfy the
        // bonded watchtower quorum of 07 §4/§7.
        let mut r = Registry::new(RegistryKind::Incident);
        let id = r
            .file(file_input(
                RegistryKind::Incident,
                acct(1),
                5,
                FilingClass::S2,
            ))
            .unwrap();
        assert_eq!(
            r.ack_observed(acct(2), 5, false, 5, id),
            Err(Error::NotRegistered)
        );
        assert_eq!(
            r.ack_observed(acct(3), 5, false, 5, id),
            Err(Error::NotRegistered)
        );
        // Late acknowledgments past the live window are rejected too.
        assert_eq!(
            r.ack_observed(acct(4), REG_WINDOW_BLOCKS + 2, true, 5, id),
            Err(Error::WindowClosed)
        );
        // With no countable acks the filing extends, then rejects.
        r.crank_close(REG_WINDOW_BLOCKS + 2, REG_CLOSE_BATCH)
            .unwrap();
        assert!(matches!(
            r.filings[0].1.state,
            FilingState::Filed { extended: true, .. }
        ));
        r.crank_close(
            REG_WINDOW_BLOCKS + REG_EXT_WINDOW_BLOCKS + 3,
            REG_CLOSE_BATCH,
        )
        .unwrap();
        assert!(matches!(r.filings[0].1.state, FilingState::Rejected));
        r.try_state().unwrap();
    }

    #[test]
    fn close_epoch_waits_for_the_filing_window_and_is_terminal() {
        // Codex review, PR #20: an empty epoch must not close (recording the
        // "no filings => 1" aggregate) while its filing window is still open.
        let mut r = Registry::new(RegistryKind::Incident);
        assert_eq!(r.close_epoch(7, 5, 10), Err(Error::WindowOpen));
        assert_eq!(r.close_epoch(7, 11, 10), Ok(FixedU64(ONE)));
        assert_eq!(r.close_epoch(7, 12, 10), Err(Error::AlreadyFinal));
        // A late filing for the closed epoch is rejected even inside an
        // apparently open window.
        let mut input = file_input(RegistryKind::Incident, acct(1), 7, FilingClass::S3);
        input.now = 5;
        assert_eq!(r.file(input), Err(Error::AlreadyFinal));
        r.try_state().unwrap();
    }

    #[test]
    fn closed_epochs_free_the_live_epoch_cap_and_reap_frees_aggregates() {
        // Codex review, PR #20: after MAX_LIVE_EPOCHS distinct epochs have
        // ever filed, new epochs must still be admissible once the old ones
        // closed out.
        let mut r = Registry::new(RegistryKind::Incident);
        for epoch in 1..=(MAX_LIVE_EPOCHS as EpochId) {
            r.file(file_input(
                RegistryKind::Incident,
                acct(epoch as u8),
                epoch,
                FilingClass::S3,
            ))
            .unwrap();
        }
        assert_eq!(
            r.file(file_input(
                RegistryKind::Incident,
                acct(9),
                99,
                FilingClass::S3
            )),
            Err(Error::TooManyLiveEpochs)
        );
        // Resolve and close epoch 1; its filing_count slot must free up.
        r.challenge_filing(acct(8), 2, 1, 0, h(7)).unwrap();
        r.resolve_challenge(1, 0, true).unwrap();
        r.close_epoch(1, 11, 10).unwrap();
        let id = r
            .file(file_input(
                RegistryKind::Incident,
                acct(9),
                99,
                FilingClass::S3,
            ))
            .unwrap();
        assert_eq!(id, 0);
        r.try_state().unwrap();
        // Reaping (cohort settlement + archive delay, enforced by the caller)
        // releases the aggregate slot and the archived filings.
        assert_eq!(r.reap_epoch(2), Err(Error::WindowOpen));
        r.reap_epoch(1).unwrap();
        assert_eq!(r.aggregate(1), None);
        assert!(r.filings.iter().all(|((e, _), _)| *e != 1));
        r.try_state().unwrap();
    }

    #[test]
    fn close_epoch_requires_terminal_filings_and_computes_milestones() {
        let mut r = Registry::new(RegistryKind::Milestone);
        let id = r
            .file(file_input(
                RegistryKind::Milestone,
                acct(1),
                3,
                FilingClass::Scope(1),
            ))
            .unwrap();
        assert_eq!(r.close_epoch(3, 2, 10), Err(Error::WindowOpen));
        r.ack_observed(acct(2), 5, true, 3, id).unwrap();
        r.ack_observed(acct(3), 6, true, 3, id).unwrap();
        r.crank_close(REG_WINDOW_BLOCKS + 2, REG_CLOSE_BATCH)
            .unwrap();
        assert_eq!(
            r.close_epoch(3, REG_WINDOW_BLOCKS + 3, 10),
            Ok(FixedU64(250_000_000))
        );
        assert_eq!(r.aggregate(3), Some(FixedU64(250_000_000)));
    }
}

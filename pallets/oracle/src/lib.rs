#![cfg_attr(not(feature = "std"), no_std)]
#![deny(unsafe_code)]

extern crate alloc;

use alloc::vec::Vec;
use futarchy_primitives::{
    AccountId, Balance, BlockNumber, EpochId, FixedU64, MetricId, MetricSpecVersion, H256,
};
use pallet_origins::Origin;
use parity_scale_codec::{Decode, Encode, MaxEncodedLen};
use scale_info::TypeInfo;

pub const MAX_REPORTERS: usize = 64;
pub const MAX_WATCHTOWERS: usize = 16;
pub const MAX_ROUNDS: usize = 64;
pub const MAX_COMPONENT_VALUES: usize = 64;
pub const ORC_WINDOW_BLOCKS: BlockNumber = 43_200;
pub const ORC_EXT_WINDOW_BLOCKS: BlockNumber = 28_800;
pub const REPORT_WINDOW_BLOCKS: BlockNumber = 28_800;
pub const RES_PROBE_INTERVAL: BlockNumber = 14_400;
pub const RES_PROBE_TIMEOUT: BlockNumber = 600;
pub const ORC_ROUNDS: u8 = 3;
pub const ORC_BOND_FLOOR: Balance = 10_000_000_000;
pub const ORC_BOND_BPS: u32 = 250;
pub const ORC_REPORTER_STAKE: Balance = 100_000_000_000;
pub const WT_STAKE: Balance = 25_000_000_000;
pub const WT_QUORUM: u8 = 2;
pub const RES_FAIL_THRESHOLD: u8 = 2;
pub const RES_RECOVER_THRESHOLD: u8 = 3;
pub const RES_PROBE_AMOUNT: Balance = 100_000;

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub struct ReporterInfo {
    pub stake: Balance,
    pub registered_at: BlockNumber,
    pub offenses: u8,
}

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub struct WatchtowerInfo {
    pub stake: Balance,
    pub registered_at: BlockNumber,
    pub inactive_epochs: u8,
}

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub enum SettlePath {
    Unchallenged,
    Recomputed,
    Adjudicated,
    Neutral,
}

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub struct SettledComponent {
    pub value: FixedU64,
    pub path: SettlePath,
    pub flagged: bool,
}

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub struct RoundState {
    pub component: MetricId,
    pub epoch: EpochId,
    pub round: u8,
    pub spec_version: MetricSpecVersion,
    pub reporter: AccountId,
    pub value: FixedU64,
    pub evidence_hash: H256,
    pub bond: Balance,
    pub challenge_deadline: BlockNumber,
    pub extended: bool,
    pub challenger: Option<AccountId>,
    pub counter_value: Option<FixedU64>,
    pub acks: u8,
    pub report_hash: H256,
    pub stake_at_risk: Balance,
    pub cumulative_reporter_bond: Balance,
    pub cumulative_challenger_bond: Balance,
}

#[derive(Clone, Copy, Debug, Decode, Default, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub struct ReserveHealth {
    pub consecutive_fails: u8,
    pub consecutive_passes: u8,
    pub unhealthy: bool,
    pub last_query_id: u64,
    pub last_probe_at: BlockNumber,
    pub pending_since: Option<BlockNumber>,
}

#[derive(Clone, Debug, Decode, Encode, Eq, PartialEq, TypeInfo)]
pub enum Event {
    ReporterRegistered {
        who: AccountId,
        stake: Balance,
    },
    Reported {
        component: MetricId,
        epoch: EpochId,
        round: u8,
        reporter: AccountId,
        value: FixedU64,
        evidence_hash: H256,
        bond: Balance,
    },
    Challenged {
        component: MetricId,
        epoch: EpochId,
        round: u8,
        challenger: AccountId,
        counter_value: FixedU64,
        evidence_hash: H256,
        bond: Balance,
    },
    RoundEscalated {
        component: MetricId,
        epoch: EpochId,
        round: u8,
        new_bond: Balance,
    },
    RecomputeProven {
        component: MetricId,
        epoch: EpochId,
        value: FixedU64,
        prover: AccountId,
    },
    AdjudicationRequested {
        component: MetricId,
        epoch: EpochId,
        referendum: u32,
    },
    Adjudicated {
        component: MetricId,
        epoch: EpochId,
        value: FixedU64,
    },
    ComponentSettled {
        component: MetricId,
        epoch: EpochId,
        value: FixedU64,
        path: SettlePath,
    },
    NeutralSettlement {
        component: MetricId,
        epoch: EpochId,
        carried_value: FixedU64,
        flagged_epochs: u8,
    },
    WindowAcknowledged {
        component: MetricId,
        epoch: EpochId,
        round: u8,
        watchtower: AccountId,
    },
    WindowExtended {
        component: MetricId,
        epoch: EpochId,
        round: u8,
        new_deadline: BlockNumber,
    },
    QuorumFailed {
        component: MetricId,
        epoch: EpochId,
        round: u8,
    },
    ReporterSlashed {
        who: AccountId,
        amount: Balance,
        offense: u8,
    },
    ReporterEjected {
        who: AccountId,
    },
    WatchtowerRegistered {
        who: AccountId,
        stake: Balance,
    },
    WatchtowerInactive {
        who: AccountId,
        epoch: EpochId,
    },
    WatchtowerSlashed {
        who: AccountId,
        amount: Balance,
    },
    ReserveProbeSent {
        query_id: u64,
    },
    ReserveProbeResult {
        query_id: u64,
        passed: bool,
    },
    ReserveUnhealthy,
    ReserveRecovered,
}

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub enum Error {
    BadOrigin,
    AlreadyRegistered,
    NotRegistered,
    TooManyReporters,
    TooManyWatchtowers,
    WindowClosed,
    WindowOpen,
    BondBelowMinimum,
    SpecVersionMismatch,
    AlreadyFinal,
    AlreadyChallenged,
    QuorumPending,
    RoundNotFound,
    RoundLimit,
    DuplicateAck,
    ReserveUnhealthy,
    ProbeTooEarly,
    UnknownQuery,
    Overflow,
}

#[derive(Clone, Debug, Decode, Encode, Eq, PartialEq, TypeInfo)]
pub struct ReportInput {
    pub who: AccountId,
    pub now: BlockNumber,
    pub component: MetricId,
    pub epoch: EpochId,
    pub spec_version: MetricSpecVersion,
    pub value: FixedU64,
    pub evidence_hash: H256,
    pub stake_at_risk: Balance,
    pub report_window_end: BlockNumber,
    pub expected_spec: MetricSpecVersion,
}

#[derive(Clone, Debug, Default, Decode, Encode, Eq, PartialEq, TypeInfo)]
pub struct Oracle {
    pub reporters: Vec<(AccountId, ReporterInfo)>,
    pub watchtowers: Vec<(AccountId, WatchtowerInfo)>,
    pub rounds: Vec<RoundState>,
    pub component_values: Vec<((MetricId, EpochId), SettledComponent)>,
    pub reserve_health: ReserveHealth,
    pub events: Vec<Event>,
    ack_records: Vec<(MetricId, EpochId, u8, AccountId, H256)>,
}

impl Oracle {
    pub fn register_reporter(&mut self, who: AccountId, now: BlockNumber) -> Result<(), Error> {
        ensure!(!self.is_reporter(&who), Error::AlreadyRegistered);
        ensure!(
            self.reporters.len() < MAX_REPORTERS,
            Error::TooManyReporters
        );
        self.reporters.push((
            who,
            ReporterInfo {
                stake: ORC_REPORTER_STAKE,
                registered_at: now,
                offenses: 0,
            },
        ));
        self.events.push(Event::ReporterRegistered {
            who,
            stake: ORC_REPORTER_STAKE,
        });
        Ok(())
    }

    pub fn deregister_reporter(&mut self, who: AccountId) -> Result<(), Error> {
        ensure!(
            !self.rounds.iter().any(|r| r.reporter == who),
            Error::WindowOpen
        );
        let pos = self
            .reporters
            .iter()
            .position(|(a, _)| *a == who)
            .ok_or(Error::NotRegistered)?;
        self.reporters.remove(pos);
        Ok(())
    }

    pub fn register_watchtower(&mut self, who: AccountId, now: BlockNumber) -> Result<(), Error> {
        ensure!(!self.is_watchtower(&who), Error::AlreadyRegistered);
        ensure!(
            self.watchtowers.len() < MAX_WATCHTOWERS,
            Error::TooManyWatchtowers
        );
        self.watchtowers.push((
            who,
            WatchtowerInfo {
                stake: WT_STAKE,
                registered_at: now,
                inactive_epochs: 0,
            },
        ));
        self.events.push(Event::WatchtowerRegistered {
            who,
            stake: WT_STAKE,
        });
        Ok(())
    }

    pub fn report(&mut self, input: ReportInput) -> Result<(), Error> {
        ensure!(self.is_reporter(&input.who), Error::NotRegistered);
        ensure!(input.now <= input.report_window_end, Error::WindowClosed);
        ensure!(
            input.spec_version == input.expected_spec,
            Error::SpecVersionMismatch
        );
        ensure!(
            self.find_round(input.component, input.epoch).is_none(),
            Error::AlreadyFinal
        );
        ensure!(self.rounds.len() < MAX_ROUNDS, Error::RoundLimit);
        let bond = round_bond(input.stake_at_risk, 1)?;
        let report_hash = hash_report(
            input.component,
            input.epoch,
            1,
            input.value,
            input.evidence_hash,
        );
        self.rounds.push(RoundState {
            component: input.component,
            epoch: input.epoch,
            round: 1,
            spec_version: input.spec_version,
            reporter: input.who,
            value: input.value,
            evidence_hash: input.evidence_hash,
            bond,
            challenge_deadline: input.now.saturating_add(ORC_WINDOW_BLOCKS),
            extended: false,
            challenger: None,
            counter_value: None,
            acks: 0,
            report_hash,
            stake_at_risk: input.stake_at_risk,
            cumulative_reporter_bond: bond,
            cumulative_challenger_bond: 0,
        });
        self.events.push(Event::Reported {
            component: input.component,
            epoch: input.epoch,
            round: 1,
            reporter: input.who,
            value: input.value,
            evidence_hash: input.evidence_hash,
            bond,
        });
        Ok(())
    }

    pub fn challenge(
        &mut self,
        who: AccountId,
        now: BlockNumber,
        component: MetricId,
        epoch: EpochId,
        counter_value: FixedU64,
        evidence_hash: H256,
    ) -> Result<(), Error> {
        let idx = self
            .find_round(component, epoch)
            .ok_or(Error::RoundNotFound)?;
        let r = &mut self.rounds[idx];
        ensure!(now <= r.challenge_deadline, Error::WindowClosed);
        ensure!(r.challenger.is_none(), Error::AlreadyChallenged);
        let bond = r.bond;
        r.challenger = Some(who);
        r.counter_value = Some(counter_value);
        r.cumulative_challenger_bond = r
            .cumulative_challenger_bond
            .checked_add(bond)
            .ok_or(Error::Overflow)?;
        self.events.push(Event::Challenged {
            component,
            epoch,
            round: r.round,
            challenger: who,
            counter_value,
            evidence_hash,
            bond,
        });
        Ok(())
    }

    pub fn ack_observed(
        &mut self,
        who: AccountId,
        component: MetricId,
        epoch: EpochId,
        round: u8,
        report_hash: H256,
    ) -> Result<(), Error> {
        ensure!(self.is_watchtower(&who), Error::NotRegistered);
        let idx = self
            .find_round(component, epoch)
            .ok_or(Error::RoundNotFound)?;
        let r = &mut self.rounds[idx];
        ensure!(
            r.round == round && r.report_hash == report_hash,
            Error::RoundNotFound
        );
        ensure!(
            !self
                .ack_records
                .contains(&(component, epoch, round, who, report_hash)),
            Error::DuplicateAck
        );
        self.ack_records
            .push((component, epoch, round, who, report_hash));
        r.acks = r.acks.saturating_add(1);
        self.events.push(Event::WindowAcknowledged {
            component,
            epoch,
            round,
            watchtower: who,
        });
        Ok(())
    }

    pub fn crank_round_close(
        &mut self,
        now: BlockNumber,
        batch: usize,
        carried_value: FixedU64,
    ) -> Result<(), Error> {
        let mut processed = 0usize;
        let mut i = 0usize;
        while i < self.rounds.len() && processed < batch {
            if now < self.rounds[i].challenge_deadline {
                i += 1;
                continue;
            }
            let (component, epoch, round) = (
                self.rounds[i].component,
                self.rounds[i].epoch,
                self.rounds[i].round,
            );
            if self.rounds[i].challenger.is_none() {
                if self.rounds[i].acks >= WT_QUORUM {
                    let value = self.rounds[i].value;
                    self.settle_at(i, value, SettlePath::Unchallenged, false)?;
                } else if !self.rounds[i].extended {
                    self.rounds[i].extended = true;
                    self.rounds[i].challenge_deadline = now.saturating_add(ORC_EXT_WINDOW_BLOCKS);
                    self.events.push(Event::WindowExtended {
                        component,
                        epoch,
                        round,
                        new_deadline: self.rounds[i].challenge_deadline,
                    });
                    i += 1;
                } else {
                    self.events.push(Event::QuorumFailed {
                        component,
                        epoch,
                        round,
                    });
                    self.neutral_at(i, carried_value, 1)?;
                }
            } else if self.rounds[i].round < ORC_ROUNDS {
                self.rounds[i].round += 1;
                self.rounds[i].bond =
                    round_bond(self.rounds[i].stake_at_risk, self.rounds[i].round)?;
                self.rounds[i].challenge_deadline = now.saturating_add(ORC_WINDOW_BLOCKS);
                self.rounds[i].acks = 0;
                self.rounds[i].challenger = None;
                self.rounds[i].counter_value = None;
                self.rounds[i].report_hash = hash_report(
                    component,
                    epoch,
                    self.rounds[i].round,
                    self.rounds[i].value,
                    self.rounds[i].evidence_hash,
                );
                self.rounds[i].cumulative_reporter_bond = self.rounds[i]
                    .cumulative_reporter_bond
                    .checked_add(self.rounds[i].bond)
                    .ok_or(Error::Overflow)?;
                self.events.push(Event::RoundEscalated {
                    component,
                    epoch,
                    round: self.rounds[i].round,
                    new_bond: self.rounds[i].bond,
                });
                i += 1;
            } else {
                i += 1;
            }
            processed += 1;
        }
        Ok(())
    }

    pub fn recompute_proof(
        &mut self,
        prover: AccountId,
        component: MetricId,
        epoch: EpochId,
        value: FixedU64,
    ) -> Result<(), Error> {
        let idx = self
            .find_round(component, epoch)
            .ok_or(Error::RoundNotFound)?;
        self.events.push(Event::RecomputeProven {
            component,
            epoch,
            value,
            prover,
        });
        self.settle_at(idx, value, SettlePath::Recomputed, false)
    }

    pub fn request_adjudication(
        &mut self,
        component: MetricId,
        epoch: EpochId,
        referendum: u32,
    ) -> Result<(), Error> {
        let idx = self
            .find_round(component, epoch)
            .ok_or(Error::RoundNotFound)?;
        ensure!(self.rounds[idx].round >= ORC_ROUNDS, Error::WindowOpen);
        ensure!(self.rounds[idx].challenger.is_some(), Error::QuorumPending);
        self.events.push(Event::AdjudicationRequested {
            component,
            epoch,
            referendum,
        });
        Ok(())
    }

    pub fn adjudicate(
        &mut self,
        origin: Origin,
        component: MetricId,
        epoch: EpochId,
        value: FixedU64,
        reporter_wrong: bool,
    ) -> Result<(), Error> {
        ensure!(origin == Origin::OracleResolution, Error::BadOrigin);
        let idx = self
            .find_round(component, epoch)
            .ok_or(Error::RoundNotFound)?;
        if reporter_wrong {
            self.record_reporter_offense(
                self.rounds[idx].reporter,
                self.rounds[idx].cumulative_reporter_bond,
            )?;
        }
        self.events.push(Event::Adjudicated {
            component,
            epoch,
            value,
        });
        self.settle_at(idx, value, SettlePath::Adjudicated, false)
    }

    pub fn crank_reserve_probe(&mut self, now: BlockNumber) -> Result<u64, Error> {
        ensure!(
            now >= self
                .reserve_health
                .last_probe_at
                .saturating_add(RES_PROBE_INTERVAL),
            Error::ProbeTooEarly
        );
        self.reserve_health.last_query_id = self
            .reserve_health
            .last_query_id
            .checked_add(1)
            .ok_or(Error::Overflow)?;
        self.reserve_health.last_probe_at = now;
        self.reserve_health.pending_since = Some(now);
        let query_id = self.reserve_health.last_query_id;
        self.events.push(Event::ReserveProbeSent { query_id });
        Ok(query_id)
    }

    pub fn reserve_probe_result(&mut self, query_id: u64, passed: bool) -> Result<(), Error> {
        ensure!(
            query_id == self.reserve_health.last_query_id,
            Error::UnknownQuery
        );
        self.apply_probe_result(query_id, passed);
        Ok(())
    }

    pub fn crank_probe_timeout(&mut self, now: BlockNumber) -> Result<(), Error> {
        let since = self
            .reserve_health
            .pending_since
            .ok_or(Error::UnknownQuery)?;
        ensure!(
            now >= since.saturating_add(RES_PROBE_TIMEOUT),
            Error::WindowOpen
        );
        let query_id = self.reserve_health.last_query_id;
        self.apply_probe_result(query_id, false);
        Ok(())
    }

    pub fn try_state(&self) -> Result<(), Error> {
        ensure!(
            self.reporters.len() <= MAX_REPORTERS,
            Error::TooManyReporters
        );
        ensure!(
            self.watchtowers.len() <= MAX_WATCHTOWERS,
            Error::TooManyWatchtowers
        );
        ensure!(self.rounds.len() <= MAX_ROUNDS, Error::RoundLimit);
        for r in &self.rounds {
            ensure!((1..=ORC_ROUNDS).contains(&r.round), Error::RoundNotFound);
            ensure!(
                r.bond >= round_bond(r.stake_at_risk, r.round)?,
                Error::BondBelowMinimum
            );
            ensure!(
                r.challenger.is_some() || r.acks < WT_QUORUM || r.round >= 1,
                Error::QuorumPending
            );
        }
        Ok(())
    }

    fn settle_at(
        &mut self,
        idx: usize,
        value: FixedU64,
        path: SettlePath,
        flagged: bool,
    ) -> Result<(), Error> {
        ensure!(
            self.component_values.len() < MAX_COMPONENT_VALUES,
            Error::AlreadyFinal
        );
        let r = self.rounds.remove(idx);
        self.component_values.push((
            (r.component, r.epoch),
            SettledComponent {
                value,
                path,
                flagged,
            },
        ));
        self.events.push(Event::ComponentSettled {
            component: r.component,
            epoch: r.epoch,
            value,
            path,
        });
        Ok(())
    }

    fn neutral_at(
        &mut self,
        idx: usize,
        carried_value: FixedU64,
        flagged_epochs: u8,
    ) -> Result<(), Error> {
        let component = self.rounds[idx].component;
        let epoch = self.rounds[idx].epoch;
        self.events.push(Event::NeutralSettlement {
            component,
            epoch,
            carried_value,
            flagged_epochs,
        });
        self.settle_at(idx, carried_value, SettlePath::Neutral, true)
    }

    fn record_reporter_offense(&mut self, who: AccountId, amount: Balance) -> Result<(), Error> {
        let (_, info) = self
            .reporters
            .iter_mut()
            .find(|(a, _)| *a == who)
            .ok_or(Error::NotRegistered)?;
        info.offenses = info.offenses.saturating_add(1);
        if info.offenses >= 2 {
            self.events.push(Event::ReporterSlashed {
                who,
                amount: amount / 2,
                offense: info.offenses,
            });
        }
        if info.offenses >= 3 {
            self.reporters.retain(|(a, _)| *a != who);
            self.events.push(Event::ReporterEjected { who });
        }
        Ok(())
    }

    fn apply_probe_result(&mut self, query_id: u64, passed: bool) {
        self.reserve_health.pending_since = None;
        if passed {
            self.reserve_health.consecutive_passes =
                self.reserve_health.consecutive_passes.saturating_add(1);
            self.reserve_health.consecutive_fails = 0;
            if self.reserve_health.unhealthy
                && self.reserve_health.consecutive_passes >= RES_RECOVER_THRESHOLD
            {
                self.reserve_health.unhealthy = false;
                self.events.push(Event::ReserveRecovered);
            }
        } else {
            self.reserve_health.consecutive_fails =
                self.reserve_health.consecutive_fails.saturating_add(1);
            self.reserve_health.consecutive_passes = 0;
            if !self.reserve_health.unhealthy
                && self.reserve_health.consecutive_fails >= RES_FAIL_THRESHOLD
            {
                self.reserve_health.unhealthy = true;
                self.events.push(Event::ReserveUnhealthy);
            }
        }
        self.events
            .push(Event::ReserveProbeResult { query_id, passed });
    }

    fn is_reporter(&self, who: &AccountId) -> bool {
        self.reporters.iter().any(|(a, _)| a == who)
    }
    fn is_watchtower(&self, who: &AccountId) -> bool {
        self.watchtowers.iter().any(|(a, _)| a == who)
    }
    fn find_round(&self, component: MetricId, epoch: EpochId) -> Option<usize> {
        self.rounds
            .iter()
            .position(|r| r.component == component && r.epoch == epoch)
    }
}

pub fn round_bond(stake_at_risk: Balance, round: u8) -> Result<Balance, Error> {
    ensure!((1..=ORC_ROUNDS).contains(&round), Error::RoundNotFound);
    let scaled = stake_at_risk
        .checked_mul(ORC_BOND_BPS as Balance)
        .ok_or(Error::Overflow)?
        / 10_000;
    let b1 = core::cmp::max(ORC_BOND_FLOOR, scaled);
    b1.checked_mul(1u128 << (round - 1)).ok_or(Error::Overflow)
}

pub fn can_admit_attested_component(delta_s_max_bps: u32) -> bool {
    let coverage_bps = ((1u32 << ORC_ROUNDS) - 1).saturating_mul(ORC_BOND_BPS);
    coverage_bps >= delta_s_max_bps
}

pub fn hash_report(
    component: MetricId,
    epoch: EpochId,
    round: u8,
    value: FixedU64,
    evidence_hash: H256,
) -> H256 {
    let mut out = evidence_hash;
    let c = component.to_le_bytes();
    let e = epoch.to_le_bytes();
    let v = value.0.to_le_bytes();
    out[0] ^= c[0];
    out[1] ^= c[1];
    out[2] ^= e[0];
    out[3] ^= e[1];
    out[4] ^= e[2];
    out[5] ^= e[3];
    out[6] ^= round;
    for (i, b) in v.iter().enumerate() {
        out[8 + i] ^= *b;
    }
    out
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

    macro_rules! report {
        ($oracle:expr, $who:expr, $now:expr, $component:expr, $epoch:expr, $spec_version:expr, $value:expr, $evidence_hash:expr, $stake_at_risk:expr, $report_window_end:expr, $expected_spec:expr $(,)?) => {
            $oracle.report(ReportInput {
                who: $who,
                now: $now,
                component: $component,
                epoch: $epoch,
                spec_version: $spec_version,
                value: $value,
                evidence_hash: $evidence_hash,
                stake_at_risk: $stake_at_risk,
                report_window_end: $report_window_end,
                expected_spec: $expected_spec,
            })
        };
    }

    #[test]
    fn reporter_and_watchtower_registries_are_bounded() {
        let mut o = Oracle::default();
        assert_eq!(o.register_reporter(acct(1), 1), Ok(()));
        assert_eq!(
            o.register_reporter(acct(1), 1),
            Err(Error::AlreadyRegistered)
        );
        for i in 2..=64 {
            assert_eq!(o.register_reporter(acct(i as u8), 1), Ok(()));
        }
        assert_eq!(
            o.register_reporter(acct(65), 1),
            Err(Error::TooManyReporters)
        );
        for i in 1..=16 {
            assert_eq!(o.register_watchtower(acct(i as u8), 1), Ok(()));
        }
        assert_eq!(
            o.register_watchtower(acct(17), 1),
            Err(Error::TooManyWatchtowers)
        );
    }

    #[test]
    fn value_scaled_bonds_and_admission_rule_match_defaults() {
        assert_eq!(round_bond(400_000_000_000, 1), Ok(10_000_000_000));
        assert_eq!(round_bond(1_200_000_000_000, 1), Ok(30_000_000_000));
        assert_eq!(round_bond(1_200_000_000_000, 3), Ok(120_000_000_000));
        assert!(can_admit_attested_component(1_750));
        assert!(!can_admit_attested_component(1_751));
    }

    #[test]
    fn unchallenged_round_needs_watchtower_quorum_else_extends_then_neutral() {
        let mut o = Oracle::default();
        o.register_reporter(acct(1), 0).unwrap();
        o.register_watchtower(acct(2), 0).unwrap();
        report!(
            o,
            acct(1),
            1,
            7,
            41,
            3,
            FixedU64(62),
            h(9),
            400_000_000_000,
            10,
            3,
        )
        .unwrap();
        o.crank_round_close(ORC_WINDOW_BLOCKS + 2, 1, FixedU64(50))
            .unwrap();
        assert!(matches!(
            o.events.last(),
            Some(Event::WindowExtended { .. })
        ));
        o.crank_round_close(
            ORC_WINDOW_BLOCKS + ORC_EXT_WINDOW_BLOCKS + 3,
            1,
            FixedU64(50),
        )
        .unwrap();
        assert_eq!(o.component_values[0].1.path, SettlePath::Neutral);
        assert!(o.component_values[0].1.flagged);
        assert!(o
            .events
            .iter()
            .any(|e| matches!(e, Event::QuorumFailed { .. })));
    }

    #[test]
    fn quorum_finalizes_and_challenge_supersedes_ack_requirement() {
        let mut o = Oracle::default();
        o.register_reporter(acct(1), 0).unwrap();
        o.register_watchtower(acct(2), 0).unwrap();
        o.register_watchtower(acct(3), 0).unwrap();
        report!(
            o,
            acct(1),
            1,
            7,
            41,
            3,
            FixedU64(62),
            h(9),
            400_000_000_000,
            10,
            3,
        )
        .unwrap();
        let rh = o.rounds[0].report_hash;
        o.ack_observed(acct(2), 7, 41, 1, rh).unwrap();
        o.ack_observed(acct(3), 7, 41, 1, rh).unwrap();
        o.crank_round_close(ORC_WINDOW_BLOCKS + 2, 1, FixedU64(50))
            .unwrap();
        assert_eq!(o.component_values[0].1.path, SettlePath::Unchallenged);

        let mut o = Oracle::default();
        o.register_reporter(acct(1), 0).unwrap();
        report!(
            o,
            acct(1),
            1,
            8,
            42,
            3,
            FixedU64(62),
            h(9),
            400_000_000_000,
            10,
            3,
        )
        .unwrap();
        o.challenge(acct(4), 2, 8, 42, FixedU64(44), h(10)).unwrap();
        o.crank_round_close(ORC_WINDOW_BLOCKS + 2, 1, FixedU64(50))
            .unwrap();
        assert_eq!(o.rounds[0].round, 2);
        assert_eq!(o.rounds[0].bond, 20_000_000_000);
    }

    #[test]
    fn recompute_and_adjudication_close_rounds_with_origin_check_and_offense_discipline() {
        let mut o = Oracle::default();
        o.register_reporter(acct(1), 0).unwrap();
        report!(
            o,
            acct(1),
            1,
            7,
            41,
            3,
            FixedU64(62),
            h(9),
            400_000_000_000,
            10,
            3,
        )
        .unwrap();
        o.challenge(acct(4), 2, 7, 41, FixedU64(44), h(10)).unwrap();
        o.recompute_proof(acct(5), 7, 41, FixedU64(44)).unwrap();
        assert_eq!(o.component_values[0].1.path, SettlePath::Recomputed);

        let mut o = Oracle::default();
        o.register_reporter(acct(1), 0).unwrap();
        for n in 0..3 {
            report!(
                o,
                acct(1),
                1,
                9 + n,
                41,
                3,
                FixedU64(62),
                h(9),
                400_000_000_000,
                10,
                3,
            )
            .unwrap();
            assert_eq!(
                o.adjudicate(Origin::FutarchyParam, 9 + n, 41, FixedU64(44), true),
                Err(Error::BadOrigin)
            );
            o.adjudicate(Origin::OracleResolution, 9 + n, 41, FixedU64(44), true)
                .unwrap();
        }
        assert!(!o.is_reporter(&acct(1)));
        assert!(o
            .events
            .iter()
            .any(|e| matches!(e, Event::ReporterEjected { .. })));
    }

    #[test]
    fn reserve_probe_is_fail_static_and_recovers_after_threshold() {
        let mut o = Oracle::default();
        assert_eq!(o.crank_reserve_probe(RES_PROBE_INTERVAL), Ok(1));
        assert_eq!(
            o.crank_probe_timeout(RES_PROBE_INTERVAL + RES_PROBE_TIMEOUT),
            Ok(())
        );
        assert!(!o.reserve_health.unhealthy);
        assert_eq!(o.crank_reserve_probe(RES_PROBE_INTERVAL * 2), Ok(2));
        o.reserve_probe_result(2, false).unwrap();
        assert!(o.reserve_health.unhealthy);
        for i in 3..=5 {
            o.crank_reserve_probe(RES_PROBE_INTERVAL * i as u32)
                .unwrap();
            o.reserve_probe_result(i, true).unwrap();
        }
        assert!(!o.reserve_health.unhealthy);
        assert!(o
            .events
            .iter()
            .any(|e| matches!(e, Event::ReserveUnhealthy)));
        assert!(o
            .events
            .iter()
            .any(|e| matches!(e, Event::ReserveRecovered)));
    }

    #[test]
    fn report_enforces_registration_window_and_spec_version() {
        let mut o = Oracle::default();
        assert_eq!(
            report!(o, acct(1), 1, 1, 1, 1, FixedU64(1), h(1), 0, 10, 1),
            Err(Error::NotRegistered)
        );
        o.register_reporter(acct(1), 0).unwrap();
        assert_eq!(
            report!(o, acct(1), 11, 1, 1, 2, FixedU64(1), h(1), 0, 10, 1),
            Err(Error::WindowClosed)
        );
        assert_eq!(
            report!(o, acct(1), 1, 1, 1, 2, FixedU64(1), h(1), 0, 10, 1),
            Err(Error::SpecVersionMismatch)
        );
    }
}

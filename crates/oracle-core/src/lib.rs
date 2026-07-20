#![cfg_attr(not(feature = "std"), no_std)]
#![deny(unsafe_code)]

extern crate alloc;

use alloc::vec::Vec;
use futarchy_primitives::{
    AccountId, Balance, BlockNumber, EpochId, FixedU64, MetricId, MetricSpecVersion, H256,
};
use origins_core::Origin;
use parity_scale_codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use scale_info::TypeInfo;

pub const MAX_REPORTERS: usize = 64;
pub const MAX_WATCHTOWERS: usize = futarchy_primitives::kernel::WT_MAX as usize;
/// Live reporting rounds: ≤ 16 components × ≤ 4 concurrently-settling epochs ×
/// ≤ 2 frozen versions overlapping a MetricSpec activation boundary (07 §2(4)).
/// Raised from 64 so a per-version game cannot be the round that overflows the
/// bound (Codex F16 / SQ-59). Within 02 §3's `open_oracle_rounds` cap of 192.
pub const MAX_ROUNDS: usize = 128;
/// Settled values awaiting reaping; sized like [`MAX_ROUNDS`] (per-version).
pub const MAX_COMPONENT_VALUES: usize = 128;
/// Upper bound on live acknowledgment records: at most one live round per game
/// key, each acknowledged by at most every registered watchtower. Pruned on
/// settle/escalate (see [`Oracle::settle_at`]/`crank_round_close`) so this holds
/// by construction; the FRAME shell's `AckRecords` storage bound is this value.
pub const MAX_ACK_RECORDS: usize = MAX_WATCHTOWERS * MAX_ROUNDS;
pub const ORC_WINDOW_BLOCKS: BlockNumber = 43_200;
pub const ORC_EXT_WINDOW_BLOCKS: BlockNumber =
    futarchy_primitives::kernel::WATCHTOWER_EXTENSION_BLOCKS;
pub const REPORT_WINDOW_BLOCKS: BlockNumber = 28_800;
pub const RES_PROBE_INTERVAL: BlockNumber = 14_400;
pub const RES_PROBE_TIMEOUT: BlockNumber = 600;
pub const ORC_ROUNDS: u8 = 3;
pub const ORC_ROUND_CAP_MIN: u8 = futarchy_primitives::kernel::ORC_ROUNDS_MIN;
pub const ORC_ROUND_CAP_MAX: u8 = futarchy_primitives::kernel::ORC_ROUNDS_MAX;
pub const ORC_BOND_FLOOR: Balance = 10_000_000_000;
pub const ORC_BOND_BPS: u32 = 250;
pub const ORC_REPORTER_STAKE: Balance = 100_000_000_000;
pub const WT_STAKE: Balance = 25_000_000_000;
pub const WT_QUORUM: u8 = futarchy_primitives::kernel::WT_QUORUM;
pub const RES_FAIL_THRESHOLD: u8 = 2;
pub const RES_RECOVER_THRESHOLD: u8 = 3;
pub const RES_PROBE_AMOUNT: Balance = 100_000;
/// 13 §5 `orc.max_proof_bytes` (K): 256 KiB per `recompute_proof` submission.
pub const ORC_MAX_PROOF_BYTES: usize = futarchy_primitives::kernel::ORC_MAX_PROOF_BYTES as usize;
/// Component values are FixedU64 (1e9 scale) in [0, 1] before aggregation
/// (05 §4.4 determinism rule 1).
pub const COMPONENT_VALUE_MAX: u64 = 1_000_000_000;

/// Live oracle and reserve-probe tunables sourced from the constitution.
///
/// The frame-free core receives one plain snapshot from its FRAME shell for
/// each operation. [`Self::DEFAULT`] preserves the genesis behavior exactly;
/// production adapters replace individual fields from live
/// `pallet-constitution::Params` reads.
#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub struct OracleParams {
    pub window: BlockNumber,
    pub rounds: u8,
    pub bond_floor: Balance,
    /// Basis points, i.e. 250 = 2.5%.
    pub bond_bps: u32,
    pub reporter_stake: Balance,
    pub watchtower_stake: Balance,
    pub watchtower_quorum: u8,
    pub probe_interval: BlockNumber,
    pub probe_timeout: BlockNumber,
    pub fail_threshold: u8,
    pub recover_threshold: u8,
    pub probe_amount: Balance,
}

impl OracleParams {
    pub const DEFAULT: Self = Self {
        window: ORC_WINDOW_BLOCKS,
        rounds: ORC_ROUNDS,
        bond_floor: ORC_BOND_FLOOR,
        bond_bps: ORC_BOND_BPS,
        reporter_stake: ORC_REPORTER_STAKE,
        watchtower_stake: WT_STAKE,
        watchtower_quorum: WT_QUORUM,
        probe_interval: RES_PROBE_INTERVAL,
        probe_timeout: RES_PROBE_TIMEOUT,
        fail_threshold: RES_FAIL_THRESHOLD,
        recover_threshold: RES_RECOVER_THRESHOLD,
        probe_amount: RES_PROBE_AMOUNT,
    };
}

impl Default for OracleParams {
    fn default() -> Self {
        Self::DEFAULT
    }
}

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

/// Addresses one reporting game: 07 §2(4) runs the game per
/// `(component, epoch, frozen spec version)`.
#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub struct RoundKey {
    pub component: MetricId,
    pub epoch: EpochId,
    pub spec_version: MetricSpecVersion,
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

/// Internal per-game schedule frozen when round one opens. This deliberately
/// lives beside [`RoundState`], whose SCALE layout is frozen by contract v4.
#[derive(Clone, Copy, Debug, Decode, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo)]
pub struct StoredRoundSchedule {
    /// Every later `B_r` doubles from this value, so live amendments cannot
    /// reprice an in-flight dispute (07 §6.1/§13).
    pub round_one_bond: Balance,
    /// Terminal adjudication is gated by this frozen `orc.rounds` value.
    pub round_cap: u8,
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
    NotRecomputable,
    ProofTooLarge,
    EvidenceMismatch,
    BadProof,
    /// A reported/adjudicated value is off the 05 §4.4 `[0, 1]` 1e9 grid.
    ValueOutOfBounds,
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
    /// Parallel internal schedule state, one entry per live round. The FRAME
    /// shell persists this outside the contract-frozen `Rounds` value.
    pub round_schedules: Vec<(RoundKey, StoredRoundSchedule)>,
    pub component_values: Vec<((MetricId, EpochId, MetricSpecVersion), SettledComponent)>,
    pub reserve_health: ReserveHealth,
    pub events: Vec<Event>,
    /// Per-round watchtower acknowledgments, keyed by `report_hash` (07 §13:
    /// "acks are per-round, keyed by `report_hash`"). `pub` so the FRAME shell
    /// (A5) can hydrate/dehydrate the whole aggregate around each call; not part
    /// of the 02 §7.2 FE-read surface.
    pub ack_records: Vec<(MetricId, EpochId, MetricSpecVersion, u8, AccountId, H256)>,
    /// `(component, frozen spec version)` pairs whose `MetricSpec` declares the
    /// value deterministically recomputable from committed evidence (07 §2(4),
    /// §9 - recomputability is a property of the frozen version, not the
    /// MetricId). Populated from spec registration; `recompute_proof` fails
    /// closed for anything else.
    pub recomputable_components: Vec<(MetricId, MetricSpecVersion)>,
    /// Watchtowers that acknowledged ≥ 1 round (or newly registered) in the
    /// current, not-yet-swept epoch — the liveness-discipline activity set
    /// (07 §4). [`Oracle::sweep_watchtower_liveness`] consumes and clears it at
    /// the epoch boundary; the FRAME shell's epoch pallet drives that call (B1a).
    pub watchtower_active: Vec<AccountId>,
}

impl Oracle {
    pub fn register_reporter_with_params(
        &mut self,
        who: AccountId,
        now: BlockNumber,
        params: &OracleParams,
    ) -> Result<(), Error> {
        ensure!(!self.is_reporter(&who), Error::AlreadyRegistered);
        ensure!(
            self.reporters.len() < MAX_REPORTERS,
            Error::TooManyReporters
        );
        self.reporters.push((
            who,
            ReporterInfo {
                stake: params.reporter_stake,
                registered_at: now,
                offenses: 0,
            },
        ));
        self.events.push(Event::ReporterRegistered {
            who,
            stake: params.reporter_stake,
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

    pub fn register_watchtower_with_params(
        &mut self,
        who: AccountId,
        now: BlockNumber,
        params: &OracleParams,
    ) -> Result<(), Error> {
        ensure!(!self.is_watchtower(&who), Error::AlreadyRegistered);
        ensure!(
            self.watchtowers.len() < MAX_WATCHTOWERS,
            Error::TooManyWatchtowers
        );
        self.watchtowers.push((
            who,
            WatchtowerInfo {
                stake: params.watchtower_stake,
                registered_at: now,
                inactive_epochs: 0,
            },
        ));
        self.events.push(Event::WatchtowerRegistered {
            who,
            stake: params.watchtower_stake,
        });
        // A freshly-registered watchtower is active for the epoch it joined, so
        // it is never charged inactivity for that epoch (07 §4).
        self.mark_watchtower_active(who);
        Ok(())
    }

    pub fn report(&mut self, input: ReportInput, params: &OracleParams) -> Result<(), Error> {
        ensure!(self.is_reporter(&input.who), Error::NotRegistered);
        ensure!(input.now <= input.report_window_end, Error::WindowClosed);
        ensure!(
            input.spec_version == input.expected_spec,
            Error::SpecVersionMismatch
        );
        // Component values live on the 05 §4.4 `[0, 1]` 1e9 grid: an
        // out-of-range attestation can never be a valid settled value, so it is
        // rejected at the door rather than allowed to settle unchallenged (I-18;
        // Codex F15).
        ensure!(
            input.value.0 <= COMPONENT_VALUE_MAX,
            Error::ValueOutOfBounds
        );
        ensure!(
            self.find_round(RoundKey {
                component: input.component,
                epoch: input.epoch,
                spec_version: input.spec_version,
            })
            .is_none(),
            Error::AlreadyFinal
        );
        // A settled `(component, epoch, version)` is final (I-18): a fresh
        // report must not reopen its game or shadow the stored value. Across a
        // MetricSpec activation boundary a second live cohort MAY require its
        // own game under a different frozen version (07 §2(4)).
        ensure!(
            !self.component_values.iter().any(|((c, e, v), _)| {
                *c == input.component && *e == input.epoch && *v == input.spec_version
            }),
            Error::AlreadyFinal
        );
        ensure!(self.rounds.len() < MAX_ROUNDS, Error::RoundLimit);
        let bond = round_bond(input.stake_at_risk, 1, params)?;
        // Reject a game whose complete frozen schedule cannot be represented;
        // otherwise a legally-open round could become uncloseable on a later
        // checked doubling (G-1).
        stored_round_bond(bond, params.rounds, params.rounds)?;
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
            challenge_deadline: input.now.saturating_add(params.window),
            extended: false,
            challenger: None,
            counter_value: None,
            acks: 0,
            report_hash,
            stake_at_risk: input.stake_at_risk,
            cumulative_reporter_bond: bond,
            cumulative_challenger_bond: 0,
        });
        self.round_schedules.push((
            RoundKey {
                component: input.component,
                epoch: input.epoch,
                spec_version: input.spec_version,
            },
            StoredRoundSchedule {
                round_one_bond: bond,
                round_cap: params.rounds,
            },
        ));
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
        key: RoundKey,
        counter_value: FixedU64,
        evidence_hash: H256,
    ) -> Result<(), Error> {
        let (component, epoch) = (key.component, key.epoch);
        let idx = self.find_round(key).ok_or(Error::RoundNotFound)?;
        let r = &mut self.rounds[idx];
        // Half-open window `[open, deadline)` — a challenge at the deadline block
        // must not race the close crank that treats that block as mature
        // (Codex F24).
        ensure!(now < r.challenge_deadline, Error::WindowClosed);
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
        now: BlockNumber,
        key: RoundKey,
        round: u8,
        report_hash: H256,
    ) -> Result<(), Error> {
        ensure!(self.is_watchtower(&who), Error::NotRegistered);
        let (component, epoch) = (key.component, key.epoch);
        let idx = self.find_round(key).ok_or(Error::RoundNotFound)?;
        let r = &mut self.rounds[idx];
        ensure!(
            r.round == round && r.report_hash == report_hash,
            Error::RoundNotFound
        );
        // Quorum proves observability during the live challenge window (07 §4).
        // The window is half-open `[open, deadline)`: an acknowledgment at or
        // after the deadline block — the same block the close crank treats as
        // mature — must not retro-finalize (Codex F24 boundary consistency).
        ensure!(now < r.challenge_deadline, Error::WindowClosed);
        // Acks are keyed by the full game triple (07 §2(4)); omitting
        // `spec_version` let one per-version game's ack collide with, or be
        // pruned by, a sibling version (Codex F8).
        ensure!(
            !self.ack_records.contains(&(
                component,
                epoch,
                key.spec_version,
                round,
                who,
                report_hash
            )),
            Error::DuplicateAck
        );
        self.ack_records
            .push((component, epoch, key.spec_version, round, who, report_hash));
        r.acks = r.acks.saturating_add(1);
        self.events.push(Event::WindowAcknowledged {
            component,
            epoch,
            round,
            watchtower: who,
        });
        // The watchtower did its job this epoch — mark it active for liveness
        // discipline (07 §4).
        self.mark_watchtower_active(who);
        Ok(())
    }

    /// Epoch-boundary liveness sweep (07 §4): a watchtower that acknowledged no
    /// round in an epoch that had ≥ 1 open round is marked inactive; two
    /// consecutive inactive epochs slash 10% of `wt.stake` and eject. A
    /// watchtower that was active this epoch has its counter reset. Epochs with
    /// no open round charge nobody (absence of work is not a liveness failure).
    /// The FRAME shell calls this once per epoch rollover with the just-ended
    /// epoch and whether it carried an open round (both known to the epoch
    /// pallet — B1a); the activity set is cleared for the next epoch.
    pub fn sweep_watchtower_liveness(
        &mut self,
        ended_epoch: EpochId,
        had_open_round: bool,
    ) -> Result<(), Error> {
        let mut ejected: Vec<AccountId> = Vec::new();
        for (who, info) in self.watchtowers.iter_mut() {
            if self.watchtower_active.contains(who) {
                info.inactive_epochs = 0;
                continue;
            }
            if !had_open_round {
                // An epoch with no open round is nobody's liveness failure and
                // breaks the "two *consecutive* inactive epochs" streak, so a
                // later miss cannot combine with an earlier one across an exempt
                // epoch to force a slash (07 §4; Codex F5).
                info.inactive_epochs = 0;
                continue;
            }
            info.inactive_epochs = info.inactive_epochs.saturating_add(1);
            self.events.push(Event::WatchtowerInactive {
                who: *who,
                epoch: ended_epoch,
            });
            if info.inactive_epochs >= 2 {
                // 07 §4: two consecutive inactive epochs ⇒ slash 10% of the
                // watchtower stake and eject.
                self.events.push(Event::WatchtowerSlashed {
                    who: *who,
                    amount: ceil_div(info.stake, 10),
                });
                ejected.push(*who);
            }
        }
        for who in &ejected {
            self.watchtowers.retain(|(a, _)| a != who);
        }
        self.watchtower_active.clear();
        Ok(())
    }

    fn mark_watchtower_active(&mut self, who: AccountId) {
        if !self.watchtower_active.contains(&who) {
            self.watchtower_active.push(who);
        }
    }

    /// Close matured rounds up to `batch`. The neutral-settlement path
    /// (07 §10) carries the component's *last valid value* — derived here from
    /// the settled-value history rather than supplied by the caller, so a
    /// keeper can never inject a forged carry value and each neutralized round
    /// carries the value for its own component (07 §4/§10).
    pub fn crank_round_close_with_params(
        &mut self,
        now: BlockNumber,
        batch: usize,
        params: &OracleParams,
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
            let schedule = self.round_schedule(RoundKey {
                component,
                epoch,
                spec_version: self.rounds[i].spec_version,
            })?;
            if self.rounds[i].challenger.is_none() {
                if self.rounds[i].acks >= params.watchtower_quorum {
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
                    let carried = self.last_valid_value(component, epoch);
                    self.neutral_at(i, carried, 1)?;
                }
            } else if self.rounds[i].round < schedule.round_cap {
                let next_round = self.rounds[i].round.checked_add(1).ok_or(Error::Overflow)?;
                let next_bond =
                    stored_round_bond(schedule.round_one_bond, next_round, schedule.round_cap)?;
                self.rounds[i].round = next_round;
                self.rounds[i].bond = next_bond;
                self.rounds[i].challenge_deadline = now.saturating_add(params.window);
                self.rounds[i].acks = 0;
                // A fresh round has a new `report_hash`, so the prior round's
                // acknowledgments can never match it again: drop them so
                // `ack_records` stays bounded by the live-round acks only
                // (G-6/I-20 — the FRAME shell's `AckRecords` bound relies on
                // this pruning). Scoped to the game's own version so a sibling
                // per-version game's acks survive (Codex F8).
                let spec_version = self.rounds[i].spec_version;
                self.ack_records.retain(|(c, e, v, _, _, _)| {
                    !(*c == component && *e == epoch && *v == spec_version)
                });
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

    /// Permissionless mechanical resolution (07 §9): the submitted proof is the
    /// committed content-addressed evidence payload; the settled value is
    /// derived from it, never taken from the caller. Only components whose
    /// frozen spec declares them deterministically recomputable are eligible;
    /// everything else fails closed and resolves by counter-report or
    /// adjudication. (Narrowing: the proof must match the round's reporter
    /// commitment — a reporter whose own committed data contradicts the claimed
    /// value is resolved against, per the 07 §5 worked example.)
    pub fn recompute_proof(
        &mut self,
        prover: AccountId,
        key: RoundKey,
        proof: &[u8],
    ) -> Result<(), Error> {
        let (component, epoch) = (key.component, key.epoch);
        ensure!(proof.len() <= ORC_MAX_PROOF_BYTES, Error::ProofTooLarge);
        ensure!(
            self.recomputable_components
                .contains(&(key.component, key.spec_version)),
            Error::NotRecomputable
        );
        let idx = self.find_round(key).ok_or(Error::RoundNotFound)?;
        ensure!(
            hash_evidence(proof) == self.rounds[idx].evidence_hash,
            Error::EvidenceMismatch
        );
        let value = recompute_value(proof)?;
        if value != self.rounds[idx].value {
            // The committed data disproves the reported value: record the 07 §3
            // offense (stake discipline); the §5.5 bond-stack forfeiture is
            // B-track custody.
            self.record_reporter_offense(self.rounds[idx].reporter)?;
        }
        self.events.push(Event::RecomputeProven {
            component,
            epoch,
            value,
            prover,
        });
        self.settle_at(idx, value, SettlePath::Recomputed, false)
    }

    pub fn request_adjudication(&mut self, key: RoundKey, referendum: u32) -> Result<(), Error> {
        let (component, epoch) = (key.component, key.epoch);
        let idx = self.find_round(key).ok_or(Error::RoundNotFound)?;
        let schedule = self.round_schedule(key)?;
        ensure!(
            self.rounds[idx].round >= schedule.round_cap,
            Error::WindowOpen
        );
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
        key: RoundKey,
        value: FixedU64,
        reporter_wrong: bool,
    ) -> Result<(), Error> {
        ensure!(origin == Origin::OracleResolution, Error::BadOrigin);
        // The adjudicated value must land on the 05 §4.4 grid like any other
        // settled value (Codex F15).
        ensure!(value.0 <= COMPONENT_VALUE_MAX, Error::ValueOutOfBounds);
        let (component, epoch) = (key.component, key.epoch);
        let idx = self.find_round(key).ok_or(Error::RoundNotFound)?;
        let schedule = self.round_schedule(key)?;
        // 07 §5.4: adjudication is the TERMINAL step of the game — the values
        // track resolves a round-`R_max` dispute that carries a live challenge.
        // A fresh or unchallenged round is not adjudicable, so the
        // `OracleResolution` origin cannot bypass the escalation ladder and
        // settle an arbitrary round (Codex F10).
        ensure!(
            self.rounds[idx].round >= schedule.round_cap,
            Error::WindowOpen
        );
        ensure!(self.rounds[idx].challenger.is_some(), Error::QuorumPending);
        if reporter_wrong {
            self.record_reporter_offense(self.rounds[idx].reporter)?;
        }
        self.events.push(Event::Adjudicated {
            component,
            epoch,
            value,
        });
        self.settle_at(idx, value, SettlePath::Adjudicated, false)
    }

    /// Force-neutralize a measurement `epoch` at its `OracleSettleDeadline`
    /// (07 §11 rule 1: any `(component, m)` not challenge-closed by the deadline
    /// settles **neutrally** for every consuming cohort). The epoch pallet drives
    /// this at the schedule-derived deadline (B1a), passing `expected` — the
    /// `(component, frozen version)` pairs live cohorts consume for `epoch`
    /// (§2(4); the epoch/welfare pallet owns that cohort→component map).
    ///
    /// Two obligations, both required for the §11(1) guarantee that welfare finds
    /// a value for **every** expected component at settlement:
    /// 1. **Live rounds** — neutral-settle every still-open round for `epoch`, so
    ///    no `Rounds` entry survives its deadline money-bearing (§13 try-state).
    ///    A later terminal verdict then finds no round (`RoundNotFound`) and can
    ///    only resolve bonds/reputation (I-18) — never overwrite the money.
    /// 2. **No-report components** — an admitted `(component, version)` that got
    ///    no report has no round, so step 1 never touches it; write its neutral
    ///    flagged carry-last `ComponentValues` entry directly (the §10 no-report
    ///    path). Without this, welfare reads an absent component and can stall or
    ///    settle a cohort with a missing neutral value (Codex P1).
    pub fn force_neutralize_expired(
        &mut self,
        epoch: EpochId,
        expected: &[(MetricId, MetricSpecVersion)],
    ) -> Result<(), Error> {
        // (1) `neutral_at`/`settle_at` remove the settled round, so repeatedly
        // take the first remaining round for this epoch; bounded by `MAX_ROUNDS`.
        while let Some(idx) = self.rounds.iter().position(|r| r.epoch == epoch) {
            let component = self.rounds[idx].component;
            let carried = self.last_valid_value(component, epoch);
            self.neutral_at(idx, carried, 1)?;
        }
        // (2) After (1) no round for `epoch` remains, so any expected key still
        // without a `ComponentValues` entry produced no report — neutralize it.
        for &(component, spec_version) in expected {
            let has_value = self
                .component_values
                .iter()
                .any(|((c, e, v), _)| *c == component && *e == epoch && *v == spec_version);
            if !has_value {
                self.neutral_no_report(component, epoch, spec_version)?;
            }
        }
        Ok(())
    }

    pub fn crank_reserve_probe_with_params(
        &mut self,
        now: BlockNumber,
        params: &OracleParams,
    ) -> Result<u64, Error> {
        ensure!(
            now >= self
                .reserve_health
                .last_probe_at
                .saturating_add(params.probe_interval),
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

    pub fn reserve_probe_result_with_params(
        &mut self,
        now: BlockNumber,
        query_id: u64,
        passed: bool,
        params: &OracleParams,
    ) -> Result<(), Error> {
        ensure!(
            query_id == self.reserve_health.last_query_id,
            Error::UnknownQuery
        );
        // Each probe outcome counts exactly once toward the consecutive
        // thresholds: a replayed response for an already consumed query must not
        // move the fail-static state.
        let since = self
            .reserve_health
            .pending_since
            .ok_or(Error::UnknownQuery)?;
        // A response that lands at or after the `res.probe_timeout` deadline is
        // counted as a **fail** regardless of the reported outcome — a late or
        // absent answer is never healthy (07 §8; Codex F2).
        let effective = passed && now < since.saturating_add(params.probe_timeout);
        self.apply_probe_result(query_id, effective, params);
        Ok(())
    }

    pub fn crank_probe_timeout_with_params(
        &mut self,
        now: BlockNumber,
        params: &OracleParams,
    ) -> Result<(), Error> {
        let since = self
            .reserve_health
            .pending_since
            .ok_or(Error::UnknownQuery)?;
        ensure!(
            now >= since.saturating_add(params.probe_timeout),
            Error::WindowOpen
        );
        let query_id = self.reserve_health.last_query_id;
        self.apply_probe_result(query_id, false, params);
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
        ensure!(
            self.round_schedules.len() == self.rounds.len(),
            Error::RoundLimit
        );
        ensure!(
            self.component_values.len() <= MAX_COMPONENT_VALUES,
            Error::AlreadyFinal
        );
        ensure!(self.ack_records.len() <= MAX_ACK_RECORDS, Error::RoundLimit);
        ensure!(
            self.watchtower_active.len() <= MAX_WATCHTOWERS,
            Error::TooManyWatchtowers
        );
        // The liveness activity set only names registered watchtowers (07 §4).
        for who in &self.watchtower_active {
            ensure!(self.is_watchtower(who), Error::NotRegistered);
        }
        for r in &self.rounds {
            let schedule = self.round_schedule(RoundKey {
                component: r.component,
                epoch: r.epoch,
                spec_version: r.spec_version,
            })?;
            ensure!(
                (ORC_ROUND_CAP_MIN..=ORC_ROUND_CAP_MAX).contains(&schedule.round_cap),
                Error::RoundNotFound
            );
            ensure!(
                (1..=schedule.round_cap).contains(&r.round),
                Error::RoundNotFound
            );
            ensure!(
                r.bond == stored_round_bond(schedule.round_one_bond, r.round, schedule.round_cap,)?,
                Error::BondBelowMinimum
            );
            // A live round for an already settled key would let a second
            // settlement shadow the final ComponentValues entry (I-18).
            ensure!(
                !self.component_values.iter().any(|((c, e, v), _)| {
                    *c == r.component && *e == r.epoch && *v == r.spec_version
                }),
                Error::AlreadyFinal
            );
            let recorded_acks = self
                .ack_records
                .iter()
                .filter(|(c, e, v, round, _, hash)| {
                    *c == r.component
                        && *e == r.epoch
                        && *v == r.spec_version
                        && *round == r.round
                        && *hash == r.report_hash
                })
                .count();
            ensure!(usize::from(r.acks) == recorded_acks, Error::QuorumPending);
        }
        for (key, _) in &self.round_schedules {
            ensure!(self.find_round(*key).is_some(), Error::RoundNotFound);
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
        let round = self.rounds.get(idx).ok_or(Error::RoundNotFound)?;
        let key = RoundKey {
            component: round.component,
            epoch: round.epoch,
            spec_version: round.spec_version,
        };
        let schedule_idx = self.round_schedule_index(key).ok_or(Error::RoundNotFound)?;
        let r = self.rounds.remove(idx);
        self.round_schedules.remove(schedule_idx);
        // The game for this `(component, epoch, version)` is terminal: its
        // acknowledgment records are dead weight, so reap them — scoped to this
        // version so a sibling per-version game's acks survive (G-6/I-20;
        // Codex F8).
        self.ack_records.retain(|(c, e, v, _, _, _)| {
            !(*c == r.component && *e == r.epoch && *v == r.spec_version)
        });
        self.component_values.push((
            (r.component, r.epoch, r.spec_version),
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

    /// The value the neutral path carries for `component` when settling
    /// `epoch` (07 §10, "carries its last valid value"): the most recent
    /// settled value for the component in a strictly earlier epoch, or the
    /// neutral 0.5 default (05 §10) when no prior value survives (histories are
    /// reaped at cohort settlement).
    fn last_valid_value(&self, component: MetricId, epoch: EpochId) -> FixedU64 {
        self.component_values
            .iter()
            .filter(|((c, e, _), _)| *c == component && *e < epoch)
            // Deterministic selection independent of storage-hasher order: the
            // greatest earlier epoch, ties broken by the greater spec version
            // (Codex F14). NB: because settled values are reaped at cohort
            // settlement, a fully-reaped history still falls back to neutral
            // 0.5 — carrying the true last value across reaping is Codex F13,
            // tracked as a resume item.
            .max_by_key(|((_, e, v), _)| (*e, *v))
            .map(|(_, settled)| settled.value)
            .unwrap_or(FixedU64(COMPONENT_VALUE_MAX / 2))
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

    /// Neutral-settle an admitted `(component, epoch, spec_version)` that received
    /// **no report** — there is no round to remove, so unlike [`Self::neutral_at`]
    /// this pushes the carry-last flagged `ComponentValues` entry directly (07 §10
    /// no-report path). Only [`Self::force_neutralize_expired`] calls it, and only
    /// for keys with no existing value, so it never shadows a settled entry (I-18).
    fn neutral_no_report(
        &mut self,
        component: MetricId,
        epoch: EpochId,
        spec_version: MetricSpecVersion,
    ) -> Result<(), Error> {
        ensure!(
            self.component_values.len() < MAX_COMPONENT_VALUES,
            Error::AlreadyFinal
        );
        let carried = self.last_valid_value(component, epoch);
        // Defensive symmetry with `settle_at`: a no-report key should carry no
        // acks, but reap any that exist so none outlives its (never-opened) game.
        self.ack_records
            .retain(|(c, e, v, _, _, _)| !(*c == component && *e == epoch && *v == spec_version));
        self.component_values.push((
            (component, epoch, spec_version),
            SettledComponent {
                value: carried,
                path: SettlePath::Neutral,
                flagged: true,
            },
        ));
        self.events.push(Event::NeutralSettlement {
            component,
            epoch,
            carried_value: carried,
            flagged_epochs: 1,
        });
        self.events.push(Event::ComponentSettled {
            component,
            epoch,
            value: carried,
            path: SettlePath::Neutral,
        });
        Ok(())
    }

    fn record_reporter_offense(&mut self, who: AccountId) -> Result<(), Error> {
        // A reporter ejected on a prior game is already maximally punished;
        // recording a further offense against them is a **no-op**, not an error,
        // so a valid recompute/adjudication on their *other* still-live rounds
        // can still settle instead of failing `NotRegistered` (Codex F17). The
        // 3rd-offense ejection removes them from new participation; retained
        // reputation beyond that is B-track custody.
        let Some((_, info)) = self.reporters.iter_mut().find(|(a, _)| *a == who) else {
            return Ok(());
        };
        info.offenses = info.offenses.saturating_add(1);
        let offense = info.offenses;
        let slash_amount = ceil_div(info.stake, 2);
        // 07 §3 stake discipline: 50% of `orc.reporter_stake` on **exactly** the
        // second adjudicated-false report; **ejection** on the third (not a
        // further slash) — Codex F19. The §5.5 round-bond-stack forfeiture and
        // its 40/60 routing are economic custody, wired at B-track (decision #3).
        if offense == 2 {
            self.events.push(Event::ReporterSlashed {
                who,
                amount: slash_amount,
                offense,
            });
        }
        if offense >= 3 {
            self.reporters.retain(|(a, _)| *a != who);
            self.events.push(Event::ReporterEjected { who });
        }
        Ok(())
    }

    fn apply_probe_result(&mut self, query_id: u64, passed: bool, params: &OracleParams) {
        self.reserve_health.pending_since = None;
        if passed {
            self.reserve_health.consecutive_passes =
                self.reserve_health.consecutive_passes.saturating_add(1);
            self.reserve_health.consecutive_fails = 0;
            if self.reserve_health.unhealthy
                && self.reserve_health.consecutive_passes >= params.recover_threshold
            {
                self.reserve_health.unhealthy = false;
                self.events.push(Event::ReserveRecovered);
            }
        } else {
            self.reserve_health.consecutive_fails =
                self.reserve_health.consecutive_fails.saturating_add(1);
            self.reserve_health.consecutive_passes = 0;
            if !self.reserve_health.unhealthy
                && self.reserve_health.consecutive_fails >= params.fail_threshold
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
    fn find_round(&self, key: RoundKey) -> Option<usize> {
        self.rounds.iter().position(|r| {
            r.component == key.component
                && r.epoch == key.epoch
                && r.spec_version == key.spec_version
        })
    }
    fn round_schedule_index(&self, key: RoundKey) -> Option<usize> {
        self.round_schedules
            .iter()
            .position(|(stored, _)| *stored == key)
    }
    fn round_schedule(&self, key: RoundKey) -> Result<StoredRoundSchedule, Error> {
        self.round_schedules
            .iter()
            .find_map(|(stored, schedule)| (*stored == key).then_some(*schedule))
            .ok_or(Error::RoundNotFound)
    }
}

/// Divide a slash base with rounding against the claimant. All callers use a
/// non-zero protocol denominator; the explicit zero branch is fail-closed for
/// any future misuse rather than panicking.
fn ceil_div(value: Balance, divisor: Balance) -> Balance {
    if divisor == 0 {
        return value;
    }
    let quotient = value / divisor;
    if value % divisor == 0 {
        quotient
    } else {
        quotient.saturating_add(1)
    }
}

pub fn round_bond(
    stake_at_risk: Balance,
    round: u8,
    params: &OracleParams,
) -> Result<Balance, Error> {
    ensure!(
        (ORC_ROUND_CAP_MIN..=ORC_ROUND_CAP_MAX).contains(&params.rounds),
        Error::RoundNotFound
    );
    ensure!((1..=params.rounds).contains(&round), Error::RoundNotFound);
    // 07 §6.1 (*Units and rounding*): `orc.bond_bps` is in basis points, so the
    // product carries the `/ 10_000` divisor and that division rounds **up**.
    // Rounding is resolved in the direction of custody (I-4 / I-28): over-custody
    // is dust, under-custody is an unbacked claim, so a bond is never a base unit
    // short. The `max` against the floor is applied after rounding.
    let scaled = stake_at_risk
        .checked_mul(params.bond_bps as Balance)
        .ok_or(Error::Overflow)?
        .div_ceil(10_000);
    let b1 = core::cmp::max(params.bond_floor, scaled);
    let multiplier = 1u128
        .checked_shl(u32::from(round.saturating_sub(1)))
        .ok_or(Error::Overflow)?;
    b1.checked_mul(multiplier).ok_or(Error::Overflow)
}

/// Derive a game's current bond exclusively from its snapshotted round-one
/// bond and round cap (07 §6.1/§13). This is deliberately independent of
/// every live constitution parameter.
pub fn stored_round_bond(
    round_one_bond: Balance,
    round: u8,
    round_cap: u8,
) -> Result<Balance, Error> {
    ensure!(
        (ORC_ROUND_CAP_MIN..=ORC_ROUND_CAP_MAX).contains(&round_cap),
        Error::RoundNotFound
    );
    ensure!((1..=round_cap).contains(&round), Error::RoundNotFound);
    let multiplier = 1u128
        .checked_shl(u32::from(round.saturating_sub(1)))
        .ok_or(Error::Overflow)?;
    round_one_bond
        .checked_mul(multiplier)
        .ok_or(Error::Overflow)
}

pub fn can_admit_attested_component(delta_s_max_bps: u32, params: &OracleParams) -> bool {
    if !(ORC_ROUND_CAP_MIN..=ORC_ROUND_CAP_MAX).contains(&params.rounds) {
        return false;
    }
    let Some(round_multiplier) = 1u32.checked_shl(u32::from(params.rounds)) else {
        return false;
    };
    let coverage_bps = round_multiplier
        .saturating_sub(1)
        .saturating_mul(params.bond_bps);
    coverage_bps >= delta_s_max_bps
}

/// Deterministic content hash for committed evidence payloads (the model
/// stand-in for the content-addressing of 07 §9, in the same idiom as
/// [`hash_report`]).
pub fn hash_evidence(payload: &[u8]) -> H256 {
    let mut out = [0u8; 32];
    let len = (payload.len() as u64).to_le_bytes();
    for (i, b) in len.iter().enumerate() {
        out[24 + i] ^= *b;
    }
    for (i, b) in payload.iter().enumerate() {
        out[i % 24] ^= b.rotate_left((i / 24 % 8) as u32);
    }
    out
}

/// Deterministic recomputation of a component value from its committed
/// evidence payload — the stand-in for evaluating the frozen MetricSpec
/// `formula_ref` (the real engine arrives with the A7 spec registry): the
/// payload's first eight little-endian bytes are the FixedU64 value, which
/// must lie on the [0, 1] 1e9 grid per 05 §4.4 determinism rule 1.
pub fn recompute_value(proof: &[u8]) -> Result<FixedU64, Error> {
    let bytes: [u8; 8] = proof
        .get(..8)
        .and_then(|slice| slice.try_into().ok())
        .ok_or(Error::BadProof)?;
    let raw = u64::from_le_bytes(bytes);
    ensure!(raw <= COMPONENT_VALUE_MAX, Error::BadProof);
    Ok(FixedU64(raw))
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

    /// 07 §6.1 (*Units and rounding*): the base-unit product rounds **up**, so a
    /// bond is never a base unit short of the specified value. Pins the direction
    /// against a silent regression to truncating division (SQ-260 / SQ-289).
    #[test]
    fn round_bond_base_unit_product_rounds_up() {
        let params = OracleParams {
            // Floor at 1 so the `max` never masks the rounding under test.
            bond_floor: 1,
            bond_bps: 1,
            ..OracleParams::DEFAULT
        };
        // 1 bps of 15,000 base units = 1.5 → MUST be 2, not 1.
        assert_eq!(round_bond(15_000, 1, &params), Ok(2));
        // An exact product is unaffected by the rounding direction.
        assert_eq!(round_bond(20_000, 1, &params), Ok(2));
        // The doubling ladder derives from the already-rounded `B_1`.
        assert_eq!(round_bond(15_000, 2, &params), Ok(4));
        // Rounding is applied before the floor `max`, never after it.
        let floored = OracleParams {
            bond_floor: 10,
            ..params
        };
        assert_eq!(round_bond(15_000, 1, &floored), Ok(10));
    }

    /// Keep the pre-existing core tests focused on their original transition
    /// assertions while production callers pass an explicit live snapshot.
    trait DefaultOracleParams {
        fn register_reporter(&mut self, who: AccountId, now: BlockNumber) -> Result<(), Error>;
        fn register_watchtower(&mut self, who: AccountId, now: BlockNumber) -> Result<(), Error>;
        fn crank_round_close(&mut self, now: BlockNumber, batch: usize) -> Result<(), Error>;
        fn crank_reserve_probe(&mut self, now: BlockNumber) -> Result<u64, Error>;
        fn reserve_probe_result(
            &mut self,
            now: BlockNumber,
            query_id: u64,
            passed: bool,
        ) -> Result<(), Error>;
        fn crank_probe_timeout(&mut self, now: BlockNumber) -> Result<(), Error>;
    }

    impl DefaultOracleParams for Oracle {
        fn register_reporter(&mut self, who: AccountId, now: BlockNumber) -> Result<(), Error> {
            Oracle::register_reporter_with_params(self, who, now, &OracleParams::DEFAULT)
        }

        fn register_watchtower(&mut self, who: AccountId, now: BlockNumber) -> Result<(), Error> {
            Oracle::register_watchtower_with_params(self, who, now, &OracleParams::DEFAULT)
        }

        fn crank_round_close(&mut self, now: BlockNumber, batch: usize) -> Result<(), Error> {
            Oracle::crank_round_close_with_params(self, now, batch, &OracleParams::DEFAULT)
        }

        fn crank_reserve_probe(&mut self, now: BlockNumber) -> Result<u64, Error> {
            Oracle::crank_reserve_probe_with_params(self, now, &OracleParams::DEFAULT)
        }

        fn reserve_probe_result(
            &mut self,
            now: BlockNumber,
            query_id: u64,
            passed: bool,
        ) -> Result<(), Error> {
            Oracle::reserve_probe_result_with_params(
                self,
                now,
                query_id,
                passed,
                &OracleParams::DEFAULT,
            )
        }

        fn crank_probe_timeout(&mut self, now: BlockNumber) -> Result<(), Error> {
            Oracle::crank_probe_timeout_with_params(self, now, &OracleParams::DEFAULT)
        }
    }

    fn acct(n: u8) -> AccountId {
        [n; 32]
    }
    fn h(n: u8) -> H256 {
        [n; 32]
    }
    fn key(component: MetricId, epoch: EpochId, spec_version: MetricSpecVersion) -> RoundKey {
        RoundKey {
            component,
            epoch,
            spec_version,
        }
    }

    macro_rules! report {
        ($oracle:expr, $who:expr, $now:expr, $component:expr, $epoch:expr, $spec_version:expr, $value:expr, $evidence_hash:expr, $stake_at_risk:expr, $report_window_end:expr, $expected_spec:expr $(,)?) => {
            $oracle.report(
                ReportInput {
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
                },
                &OracleParams::DEFAULT,
            )
        };
    }

    fn round_deadline(o: &Oracle, k: RoundKey) -> BlockNumber {
        o.rounds
            .iter()
            .find(|r| {
                r.component == k.component && r.epoch == k.epoch && r.spec_version == k.spec_version
            })
            .expect("live round")
            .challenge_deadline
    }

    /// Report round 1, then drive the game to a terminal state (round `R_max`
    /// with a live challenge) so `adjudicate` is admissible (07 §5.4).
    fn to_terminal(o: &mut Oracle, reporter: u8, challenger: u8, k: RoundKey, value: FixedU64) {
        report!(
            o,
            acct(reporter),
            1,
            k.component,
            k.epoch,
            k.spec_version,
            value,
            h(9),
            400_000_000_000,
            100,
            k.spec_version,
        )
        .unwrap();
        // Rounds 1..R_max: challenge (strictly inside the window) then crank at
        // the deadline to escalate.
        for _ in 1..ORC_ROUNDS {
            let d = round_deadline(o, k);
            o.challenge(acct(challenger), d - 1, k, FixedU64(440_000_000), h(10))
                .unwrap();
            o.crank_round_close(d, 1).unwrap();
        }
        // Round R_max carries the terminal challenge.
        let d = round_deadline(o, k);
        o.challenge(acct(challenger), d - 1, k, FixedU64(440_000_000), h(10))
            .unwrap();
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
        assert_eq!(
            round_bond(400_000_000_000, 1, &OracleParams::DEFAULT),
            Ok(10_000_000_000)
        );
        assert_eq!(
            round_bond(1_200_000_000_000, 1, &OracleParams::DEFAULT),
            Ok(30_000_000_000)
        );
        assert_eq!(
            round_bond(1_200_000_000_000, 3, &OracleParams::DEFAULT),
            Ok(120_000_000_000)
        );
        assert!(can_admit_attested_component(1_750, &OracleParams::DEFAULT));
        assert!(!can_admit_attested_component(1_751, &OracleParams::DEFAULT));
        let amended = OracleParams {
            rounds: 2,
            bond_bps: 500,
            ..OracleParams::DEFAULT
        };
        assert!(can_admit_attested_component(1_500, &amended));
        assert!(!can_admit_attested_component(1_501, &amended));
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
        o.crank_round_close(ORC_WINDOW_BLOCKS + 2, 1).unwrap();
        assert!(matches!(
            o.events.last(),
            Some(Event::WindowExtended { .. })
        ));
        o.crank_round_close(ORC_WINDOW_BLOCKS + ORC_EXT_WINDOW_BLOCKS + 3, 1)
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
        o.ack_observed(acct(2), 5, key(7, 41, 3), 1, rh).unwrap();
        o.ack_observed(acct(3), 6, key(7, 41, 3), 1, rh).unwrap();
        o.crank_round_close(ORC_WINDOW_BLOCKS + 2, 1).unwrap();
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
        o.challenge(acct(4), 2, key(8, 42, 3), FixedU64(44), h(10))
            .unwrap();
        o.crank_round_close(ORC_WINDOW_BLOCKS + 2, 1).unwrap();
        assert_eq!(o.rounds[0].round, 2);
        assert_eq!(o.rounds[0].bond, 20_000_000_000);
    }

    #[test]
    fn recompute_and_adjudication_close_rounds_with_origin_check_and_offense_discipline() {
        let mut o = Oracle::default();
        o.recomputable_components.push((7, 3));
        o.register_reporter(acct(1), 0).unwrap();
        // The committed evidence payload recomputes to 0.44-style FixedU64(44),
        // contradicting the reported 62: recompute must settle at 44 and record
        // a reporter offense.
        let mut proof = alloc::vec![0u8; 24];
        proof[..8].copy_from_slice(&44u64.to_le_bytes());
        report!(
            o,
            acct(1),
            1,
            7,
            41,
            3,
            FixedU64(62),
            hash_evidence(&proof),
            400_000_000_000,
            10,
            3,
        )
        .unwrap();
        o.challenge(acct(4), 2, key(7, 41, 3), FixedU64(44), h(10))
            .unwrap();
        o.recompute_proof(acct(5), key(7, 41, 3), &proof).unwrap();
        assert_eq!(o.component_values[0].1.path, SettlePath::Recomputed);
        assert_eq!(o.component_values[0].1.value, FixedU64(44));
        assert_eq!(o.reporters[0].1.offenses, 1);

        let mut o = Oracle::default();
        o.register_reporter(acct(1), 0).unwrap();
        for n in 0..3 {
            // 07 §5.4: adjudication only resolves a terminal (round-R_max,
            // challenged) dispute (Codex F10) — drive each game there first.
            to_terminal(&mut o, 1, 4, key(9 + n, 41, 3), FixedU64(62));
            assert_eq!(
                o.adjudicate(Origin::FutarchyParam, key(9 + n, 41, 3), FixedU64(44), true),
                Err(Error::BadOrigin)
            );
            o.adjudicate(
                Origin::OracleResolution,
                key(9 + n, 41, 3),
                FixedU64(44),
                true,
            )
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
        o.reserve_probe_result(RES_PROBE_INTERVAL * 2, 2, false)
            .unwrap();
        assert!(o.reserve_health.unhealthy);
        for i in 3..=5 {
            o.crank_reserve_probe(RES_PROBE_INTERVAL * i as u32)
                .unwrap();
            o.reserve_probe_result(RES_PROBE_INTERVAL * i as u32, i, true)
                .unwrap();
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
    fn recompute_proof_is_mechanical_and_fail_closed() {
        let mut o = Oracle::default();
        o.recomputable_components.push((7, 3));
        o.register_reporter(acct(1), 0).unwrap();
        let mut proof = alloc::vec![0u8; 24];
        proof[..8].copy_from_slice(&62u64.to_le_bytes());
        report!(
            o,
            acct(1),
            1,
            7,
            41,
            3,
            FixedU64(62),
            hash_evidence(&proof),
            400_000_000_000,
            10,
            3,
        )
        .unwrap();
        // Component not declared recomputable in the frozen spec: fail closed.
        assert_eq!(
            o.recompute_proof(acct(5), key(9, 41, 3), &proof),
            Err(Error::NotRecomputable)
        );
        // Oversized proof.
        assert_eq!(
            o.recompute_proof(
                acct(5),
                key(7, 41, 3),
                &alloc::vec![0u8; ORC_MAX_PROOF_BYTES + 1]
            ),
            Err(Error::ProofTooLarge)
        );
        // Payload that does not match the committed evidence.
        assert_eq!(
            o.recompute_proof(acct(5), key(7, 41, 3), &alloc::vec![1u8; 24]),
            Err(Error::EvidenceMismatch)
        );
        // Committed payload agreeing with the report settles without offense.
        o.recompute_proof(acct(5), key(7, 41, 3), &proof).unwrap();
        assert_eq!(o.component_values[0].1.value, FixedU64(62));
        assert_eq!(o.reporters[0].1.offenses, 0);

        // A committed payload too short to decode, or off the [0,1] 1e9 grid,
        // is a bad proof even when the hash matches.
        let mut o = Oracle::default();
        o.recomputable_components.push((7, 3));
        o.register_reporter(acct(1), 0).unwrap();
        let short = alloc::vec![3u8; 4];
        let mut off_grid = alloc::vec![0u8; 24];
        off_grid[..8].copy_from_slice(&(COMPONENT_VALUE_MAX + 1).to_le_bytes());
        report!(
            o,
            acct(1),
            1,
            7,
            41,
            3,
            FixedU64(62),
            hash_evidence(&short),
            400_000_000_000,
            10,
            3,
        )
        .unwrap();
        assert_eq!(
            o.recompute_proof(acct(5), key(7, 41, 3), &short),
            Err(Error::BadProof)
        );
        assert_eq!(
            o.recompute_proof(acct(5), key(7, 41, 3), &off_grid),
            Err(Error::EvidenceMismatch)
        );
        assert_eq!(recompute_value(&off_grid), Err(Error::BadProof));
    }

    #[test]
    fn late_watchtower_acks_cannot_retro_finalize() {
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
        let deadline = o.rounds[0].challenge_deadline;
        // The window is half-open: an acknowledgment *at* the deadline block (the
        // block the close crank treats as mature) is already rejected (F24).
        assert_eq!(
            o.ack_observed(acct(2), deadline, key(7, 41, 3), 1, rh),
            Err(Error::WindowClosed)
        );
        // The uncranked round then extends rather than finalizing.
        o.crank_round_close(deadline + 2, 1).unwrap();
        assert!(matches!(
            o.events.last(),
            Some(Event::WindowExtended { .. })
        ));
        // Acks strictly inside the live extension window still count toward quorum.
        let extended_deadline = o.rounds[0].challenge_deadline;
        o.ack_observed(acct(2), extended_deadline - 1, key(7, 41, 3), 1, rh)
            .unwrap();
        o.ack_observed(acct(3), extended_deadline - 1, key(7, 41, 3), 1, rh)
            .unwrap();
        o.crank_round_close(extended_deadline + 1, 1).unwrap();
        assert_eq!(o.component_values[0].1.path, SettlePath::Unchallenged);
    }

    #[test]
    fn settled_components_cannot_be_reopened_by_a_new_report() {
        let mut o = Oracle::default();
        o.register_reporter(acct(1), 0).unwrap();
        to_terminal(&mut o, 1, 4, key(7, 41, 3), FixedU64(62));
        o.adjudicate(Origin::OracleResolution, key(7, 41, 3), FixedU64(44), false)
            .unwrap();
        assert_eq!(o.component_values.len(), 1);
        assert_eq!(
            report!(
                o,
                acct(1),
                2,
                7,
                41,
                3,
                FixedU64(99),
                h(9),
                400_000_000_000,
                10,
                3,
            ),
            Err(Error::AlreadyFinal)
        );
        assert_eq!(o.component_values.len(), 1);
        o.try_state().unwrap();
    }

    #[test]
    fn reserve_probe_results_count_once() {
        let mut o = Oracle::default();
        assert_eq!(o.crank_reserve_probe(RES_PROBE_INTERVAL), Ok(1));
        o.reserve_probe_result(RES_PROBE_INTERVAL, 1, false)
            .unwrap();
        // Replaying the consumed query must not add a second consecutive fail.
        assert_eq!(
            o.reserve_probe_result(RES_PROBE_INTERVAL, 1, false),
            Err(Error::UnknownQuery)
        );
        assert!(!o.reserve_health.unhealthy);
        assert_eq!(o.reserve_health.consecutive_fails, 1);
        // A response landing after the timeout already consumed the query is
        // rejected too.
        assert_eq!(o.crank_reserve_probe(RES_PROBE_INTERVAL * 2), Ok(2));
        o.crank_probe_timeout(RES_PROBE_INTERVAL * 2 + RES_PROBE_TIMEOUT)
            .unwrap();
        assert!(o.reserve_health.unhealthy);
        assert_eq!(
            o.reserve_probe_result(RES_PROBE_INTERVAL * 2, 2, true),
            Err(Error::UnknownQuery)
        );
        assert_eq!(o.reserve_health.consecutive_passes, 0);
    }

    #[test]
    fn per_version_games_survive_an_activation_boundary() {
        // Codex review, PR #30 / 07 §2(4): where two live cohorts consume the
        // same (component, epoch) under different frozen spec versions, one
        // game runs per version - settling one must not block or shadow the
        // other, and recomputability is a property of the frozen version.
        let mut o = Oracle::default();
        o.recomputable_components.push((7, 3));
        o.register_reporter(acct(1), 0).unwrap();
        let mut proof = alloc::vec![0u8; 24];
        proof[..8].copy_from_slice(&44u64.to_le_bytes());
        report!(
            o,
            acct(1),
            1,
            7,
            41,
            3,
            FixedU64(44),
            hash_evidence(&proof),
            400_000_000_000,
            10,
            3,
        )
        .unwrap();
        // The version-4 game opens independently while version 3 is live.
        report!(
            o,
            acct(1),
            2,
            7,
            41,
            4,
            FixedU64(50),
            h(9),
            400_000_000_000,
            10,
            4,
        )
        .unwrap();
        assert_eq!(o.rounds.len(), 2);
        // Version 3 is declared recomputable; version 4 is not.
        o.recompute_proof(acct(5), key(7, 41, 3), &proof).unwrap();
        assert_eq!(
            o.recompute_proof(acct(5), key(7, 41, 4), &proof),
            Err(Error::NotRecomputable)
        );
        // The settled version-3 value does not finalize version 4's game...
        assert_eq!(o.component_values.len(), 1);
        assert_eq!(o.rounds.len(), 1);
        // ...which still settles on its own track — escalate v4 to terminal
        // (07 §5.4 / Codex F10) then adjudicate it.
        let k4 = key(7, 41, 4);
        for _ in 1..ORC_ROUNDS {
            let d = round_deadline(&o, k4);
            o.challenge(acct(4), d - 1, k4, FixedU64(50), h(10))
                .unwrap();
            o.crank_round_close(d, 1).unwrap();
        }
        let d = round_deadline(&o, k4);
        o.challenge(acct(4), d - 1, k4, FixedU64(50), h(10))
            .unwrap();
        o.adjudicate(Origin::OracleResolution, k4, FixedU64(50), false)
            .unwrap();
        assert_eq!(o.component_values.len(), 2);
        // A repeat report for a settled version stays final.
        assert_eq!(
            report!(
                o,
                acct(1),
                3,
                7,
                41,
                3,
                FixedU64(60),
                h(9),
                400_000_000_000,
                10,
                3,
            ),
            Err(Error::AlreadyFinal)
        );
        o.try_state().unwrap();
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

    #[test]
    fn watchtower_liveness_grace_then_inactivity_slash_and_reset() {
        // 07 §4 liveness discipline: registration grace, then inactivity
        // accrual, the 2-consecutive slash+eject, the active-epoch reset, and
        // the no-open-round exemption.
        let mut o = Oracle::default();
        o.register_watchtower(acct(2), 0).unwrap();
        o.register_watchtower(acct(3), 0).unwrap();

        // Epoch 1: both were just registered (grace) ⇒ no inactivity.
        o.sweep_watchtower_liveness(1, true).unwrap();
        assert!(o.watchtowers.iter().all(|(_, i)| i.inactive_epochs == 0));
        assert!(o.watchtower_active.is_empty());

        // Epoch 2 had an open round but neither acked ⇒ both inactive once.
        o.sweep_watchtower_liveness(2, true).unwrap();
        assert!(o.watchtowers.iter().all(|(_, i)| i.inactive_epochs == 1));
        assert_eq!(
            o.events
                .iter()
                .filter(|e| matches!(e, Event::WatchtowerInactive { .. }))
                .count(),
            2
        );

        // Watchtower 2 acks in epoch 3 (needs a live round); 3 stays idle.
        o.register_reporter(acct(1), 0).unwrap();
        report!(o, acct(1), 1, 7, 41, 3, FixedU64(62), h(9), 0, 10, 3).unwrap();
        let rh = o.rounds[0].report_hash;
        o.ack_observed(acct(2), 5, key(7, 41, 3), 1, rh).unwrap();
        o.sweep_watchtower_liveness(3, true).unwrap();
        // 2 reset to 0 (active); 3 reaches 2 ⇒ slashed and ejected.
        assert_eq!(o.watchtowers.len(), 1);
        assert_eq!(o.watchtowers[0].0, acct(2));
        assert_eq!(o.watchtowers[0].1.inactive_epochs, 0);
        assert!(o.events.iter().any(
            |e| matches!(e, Event::WatchtowerSlashed { amount, .. } if *amount == WT_STAKE / 10)
        ));

        // An epoch with no open round charges nobody, even when idle.
        o.sweep_watchtower_liveness(4, false).unwrap();
        assert_eq!(o.watchtowers[0].1.inactive_epochs, 0);
        o.try_state().unwrap();
    }

    #[test]
    fn force_neutralize_expired_settles_stale_rounds_and_blocks_late_verdicts() {
        // 07 §11: a round not challenge-closed by its OracleSettleDeadline
        // settles neutrally; a late terminal verdict then finds no round and
        // cannot overwrite the money (I-18; Codex F11/F12).
        let mut o = Oracle::default();
        o.register_reporter(acct(1), 0).unwrap();
        report!(
            o,
            acct(1),
            1,
            7,
            41,
            3,
            FixedU64(620_000_000),
            h(9),
            400_000_000_000,
            100,
            3,
        )
        .unwrap();
        o.challenge(acct(4), 2, key(7, 41, 3), FixedU64(440_000_000), h(10))
            .unwrap();
        o.force_neutralize_expired(41, &[]).unwrap();
        assert_eq!(o.component_values.len(), 1);
        assert_eq!(o.component_values[0].1.path, SettlePath::Neutral);
        assert!(o.component_values[0].1.flagged);
        assert!(o.rounds.is_empty());
        // A late verdict can no longer touch the settled money.
        assert_eq!(
            o.adjudicate(
                Origin::OracleResolution,
                key(7, 41, 3),
                FixedU64(440_000_000),
                true
            ),
            Err(Error::RoundNotFound)
        );
        o.try_state().unwrap();
    }

    #[test]
    fn force_neutralize_expired_neutralizes_no_report_components() {
        // Codex P1 / 07 §11(1): an admitted component that got NO report has no
        // round, so the live-round sweep never touches it — yet welfare must find
        // a value for it at the money deadline. The deadline crank synthesizes the
        // neutral flagged carry-last entry directly (07 §10 no-report path).
        // `expected` = the (component, version) pairs live cohorts consume for the
        // epoch; the epoch/welfare pallet supplies them at B1a.
        let mut o = Oracle::default();
        o.register_reporter(acct(1), 0).unwrap();
        // Component 7 opens a round (settled via the sweep); component 8 is also
        // expected but never reports (settled via the no-report path).
        report!(
            o,
            acct(1),
            1,
            7,
            41,
            3,
            FixedU64(620_000_000),
            h(9),
            400_000_000_000,
            100,
            3,
        )
        .unwrap();
        let expected = [(7, 3), (8, 3)];
        o.force_neutralize_expired(41, &expected).unwrap();
        // Both keys carry a neutral flagged value — 7 from its round, 8 no-report.
        assert_eq!(o.component_values.len(), 2);
        for &(c, v) in &expected {
            let entry = o
                .component_values
                .iter()
                .find(|((cc, ee, vv), _)| *cc == c && *ee == 41 && *vv == v)
                .expect("every expected component has a value by the deadline");
            assert_eq!(entry.1.path, SettlePath::Neutral);
            assert!(entry.1.flagged);
        }
        // The no-report component with no prior value carries neutral 0.5 (05 §10).
        let no_report = o
            .component_values
            .iter()
            .find(|((c, _, _), _)| *c == 8)
            .unwrap();
        assert_eq!(no_report.1.value, FixedU64(COMPONENT_VALUE_MAX / 2));
        assert!(o.rounds.is_empty());
        // Idempotent: a second crank finds both keys valued and adds nothing.
        o.force_neutralize_expired(41, &expected).unwrap();
        assert_eq!(o.component_values.len(), 2);
        o.try_state().unwrap();
    }

    #[test]
    fn round_state_fields_match_contract_v4() {
        use scale_info::TypeDef;
        const ROUND_STATE_FIELDS: [&str; 17] = [
            "component",
            "epoch",
            "round",
            "spec_version",
            "reporter",
            "value",
            "evidence_hash",
            "bond",
            "challenge_deadline",
            "extended",
            "challenger",
            "counter_value",
            "acks",
            "report_hash",
            "stake_at_risk",
            "cumulative_reporter_bond",
            "cumulative_challenger_bond",
        ];
        let type_info = RoundState::type_info();
        let names: Vec<&str> = match &type_info.type_def {
            TypeDef::Composite(c) => c.fields.iter().filter_map(|f| f.name).collect(),
            _ => panic!("RoundState must encode as a SCALE composite type"),
        };
        assert_eq!(names, ROUND_STATE_FIELDS);
    }

    #[test]
    fn offense_against_an_ejected_reporter_is_a_noop_not_an_error() {
        // Codex F17: a reporter ejected on one game can still have another live
        // round settled — a further offense against them is a no-op, so the
        // valid recompute is not stranded by `NotRegistered`.
        let mut o = Oracle::default();
        o.register_reporter(acct(1), 0).unwrap();
        // Committed evidence that disproves the report (recomputes to 0.44).
        let mut proof = alloc::vec![0u8; 24];
        proof[..8].copy_from_slice(&440_000_000u64.to_le_bytes());
        // Game 30 is reported now but recomputed only AFTER the ejection.
        o.recomputable_components.push((30, 3));
        report!(
            o,
            acct(1),
            1,
            30,
            41,
            3,
            FixedU64(620_000_000),
            hash_evidence(&proof),
            400_000_000_000,
            100,
            3,
        )
        .unwrap();
        // Eject acct(1): three recompute-disproofs on other components accrue
        // three offenses (recompute settles each game directly — no crank).
        for n in 0..3 {
            o.recomputable_components.push((31 + n, 3));
            report!(
                o,
                acct(1),
                1,
                31 + n,
                41,
                3,
                FixedU64(620_000_000),
                hash_evidence(&proof),
                400_000_000_000,
                100,
                3,
            )
            .unwrap();
            o.recompute_proof(acct(5), key(31 + n, 41, 3), &proof)
                .unwrap();
        }
        assert!(!o.is_reporter(&acct(1)));
        // The pre-existing game still settles by recompute despite the ejection.
        o.recompute_proof(acct(5), key(30, 41, 3), &proof).unwrap();
        assert!(o.component_values.iter().any(|((c, _, _), _)| *c == 30));
        o.try_state().unwrap();
    }
}

#![cfg_attr(not(feature = "std"), no_std)]
#![deny(unsafe_code)]

//! # `pallet-oracle` — bonded reporting game, watchtowers and reserve probe (A5)
//!
//! Production FRAME shell over the frame-free functional core [`oracle_core`],
//! which remains the differential oracle (Python M3 has no oracle module; the
//! Rust core is the authority) and the auditor-consumable port. Each extrinsic
//! **hydrates** the whole [`Oracle`] aggregate from storage, calls the reviewed
//! core state-transition, then **persists** the mutated aggregate and deposits
//! the events the op produced — so the pallet's observable behavior is the
//! core's behavior by construction (Track-A "shell over core").
//!
//! Spec: `docs/architecture/07` (all — reporter/watchtower registries, the
//! bonded reporting game with value-scaled bonds and 72 h watchtower-acknowledged
//! challenge windows, `recompute_proof`, neutral settlement, the reserve-health
//! probe `R`), `02 §7.2` (frozen storage/event surface), `13 §1/§2` (oracle
//! parameters), `15 §1` (I-18 only challenge-closed values settle money; I-24
//! fail-static XCM boundary).
//!
//! ## Frozen contract surface (02 §7.2 — contract v3, byte-for-byte)
//!
//! - `Reporters: map AccountId → ReporterInfo` (counted; ≥ 3 to admit attested)
//! - `Watchtowers: map AccountId → WatchtowerInfo` (counted; ≤ `wt.max = 16`)
//! - `Rounds`/`ComponentValues`: `StorageNMap` over the **triple key**
//!   `(MetricId, EpochId, MetricSpecVersion)` — 07 §2(4) runs one game per frozen
//!   version, so an activation boundary keeps two games live for one
//!   `(component, epoch)` that the pair key of contract v2 could not hold.
//! - `ReserveHealth` — single value, the deterministic reserve-probe state
//!
//! **Contract v3 (resolved this session — PLAN SQ-58, extends SQ-2).** 02 §7.2
//! froze `Rounds`/`ComponentValues` on the pair `(MetricId, EpochId)`, self-
//! contradictorily (its own bound note said per-version games "append a
//! `RoundState` per frozen version", impossible for a one-value-per-key map).
//! 02 §7.2/§13 were amended to the **triple key** and `INTEGRATION_CONTRACT_VERSION`
//! bumped 2 → 3 under the user's joint backend+frontend sign-off (R-1) — a pre-
//! genesis correction (no runtime deployed). `RoundState` re-embeds the triple for
//! a `try_state` key-integrity check and carries the frozen ack-keying/§5.5 fields;
//! the per-game bond schedule is internal parallel storage. The FE reads the
//! `OracleRoundView` projection (02 §4), not either backing representation.
//! I-18: no per-version settlement may be shadowed by another version's.
//!
//! ## Origins (07 §13; rule 6)
//!
//! Reporter/watchtower/keeper calls are ordinary `Signed` (07 §3/§4/§5: OCWs sign
//! them; `ValidateUnsigned` is implemented for no call). Terminal `adjudicate` is
//! the sole privileged call — [`Config::AdjudicationOrigin`] admits only the
//! `OracleResolution` values track (07 §5.4). The runtime wires that origin over
//! `pallet-origins` (A4/B1a); the mock provides a test resolver.
//!
//! ## Cross-pallet seams deferred to B1a (documented, not hardcoded)
//!
//! - `report` derives its window end, expected spec version and `StakeAtRisk` via
//!   [`Config::Reporting`] (07 §5.1/§2(4)/§6.1) — wired to the epoch clock, the
//!   welfare MetricSpec registry and cohort escrows in B1a; constants in the mock.
//! - The reserve probe advances *state only* here (no `xcm` imports — I-24, rule 7);
//!   the XCM send and the `QueryResponse` handler [`Pallet::reserve_probe_result`]
//!   are wired in B4. The `ReserveUnhealthy`/`ReserveRecovered` mirror to
//!   `constitution.PhaseFlags` bit 7 and the 08 §1.2 treasury NAV haircut now
//!   travel together through the fallible [`ReserveHealthSink`] (SQ-205), applied
//!   on the flag's edge inside this pallet's storage layer so all three writes
//!   commit or none do. **The production runtime still binds `()`**: the probe
//!   feed itself is unwired and its state machine is one-way, so arming the sink
//!   would mean a permanent chain-wide `spendable_nav = 0` — see PLAN SQ-380.
//!   The `PB-RESERVE` split-inflow halt remains guardian wiring.
//! - Economic custody (reserving USDC stakes/bonds, routing the 40/60 slash) is a
//!   B-track runtime concern: the core models bond/stake amounts as `Balance`
//!   fields, matching every other frame-free core (and A1).
//! - Live `pallet-constitution::Params` sourcing of the META/PARAM oracle tunables
//!   (`orc.*`, `wt.*`, `res.*`) enters through [`Config::Params`]; the frame-free
//!   core carries the 13 defaults for fallback and standalone use.

extern crate alloc;

use frame_support::pallet_prelude::DispatchResult;

pub use pallet::*;
pub use weights::WeightInfo;

pub mod weights;

#[cfg(feature = "runtime-benchmarks")]
mod benchmarking;
#[cfg(test)]
mod mock;
#[cfg(test)]
mod tests;

// The functional core is the semantic source of truth; re-export its surface
// named (not glob — the pallet owns its own `Error`/`ReserveHealth` aliases).
pub use oracle_core::{
    round_bond, stored_round_bond, Error as CoreError, Event as CoreEvent, Oracle, OracleParams,
    ReportInput, ReporterInfo, ReserveHealth as ReserveHealthValue, RoundKey, RoundState,
    SettlePath, SettledComponent, StoredRoundSchedule, WatchtowerInfo, MAX_ACK_RECORDS,
    MAX_COMPONENT_VALUES, MAX_REPORTERS, MAX_RESERVE_PROBE_QUERY_ID, MAX_ROUNDS, MAX_WATCHTOWERS,
    ORC_MAX_PROOF_BYTES, ORC_ROUNDS, RES_PROBE_INTERVAL, RES_PROBE_TIMEOUT,
};

use futarchy_primitives::{BlockNumber, EpochId, MetricId, MetricSpecVersion};

/// `MAX_REPORTERS` as a storage bound (07 §13 / 02 §7.2 — counted map).
pub const MAX_REPORTERS_BOUND: u32 = MAX_REPORTERS as u32;
/// `wt.max = 16` (13 §1) as a storage bound.
pub const MAX_WATCHTOWERS_BOUND: u32 = MAX_WATCHTOWERS as u32;
/// Live rounds bound: ≤ 16 components × ≤ 4 settling epochs, per-version (02 §7.2).
pub const MAX_ROUNDS_BOUND: u32 = MAX_ROUNDS as u32;
/// Settled values awaiting reaping at cohort settlement (02 §7.2).
pub const MAX_COMPONENT_VALUES_BOUND: u32 = MAX_COMPONENT_VALUES as u32;
/// Live acknowledgment records; pruned on settle/escalate in the core.
pub const MAX_ACK_RECORDS_BOUND: u32 = MAX_ACK_RECORDS as u32;
/// `orc.max_proof_bytes = 256 KiB` (07 §9) as the `recompute_proof` arg bound.
pub const MAX_PROOF_BYTES_BOUND: u32 = ORC_MAX_PROOF_BYTES as u32;
/// Recomputable `(component, version)` declarations from the MetricSpec registry.
pub const MAX_RECOMPUTABLE_BOUND: u32 = 64;

/// Live oracle and reserve-probe tunables sourced from
/// `pallet-constitution::Params`.
pub trait OracleParamsProvider {
    fn get() -> OracleParams;
}

/// The three cross-pallet inputs `report` derives rather than trusting the caller
/// (07 §5.1 window, §2(4) frozen version, §6.1 `StakeAtRisk`). The runtime wires
/// this over `pallet-epoch` (schedule/escrow) and `pallet-welfare` (MetricSpec
/// registry) at B1a; the mock supplies deterministic values.
pub trait ReportingContext {
    /// Block by which a report for measurement epoch `m` must arrive: close of
    /// `m` + the 2-day report window (07 §5.1).
    fn report_window_end(measurement_epoch: EpochId) -> BlockNumber;

    /// Whether `version` is a frozen MetricSpec version that some live cohort
    /// consuming `(component, epoch)` was created under (07 §2(4), I-16). Returns
    /// a bool rather than a single version so **every** live per-version game
    /// across an activation boundary can be reported — a single-version getter
    /// would reject all but one version and defeat the triple-key design
    /// (Codex F7).
    fn is_expected_spec_version(
        component: MetricId,
        epoch: EpochId,
        version: MetricSpecVersion,
    ) -> bool;

    /// `StakeAtRisk(c, m) = Σ CohortEscrow(k)` over cohorts whose frozen spec
    /// consumes `(component, epoch)` (07 §6.1) — prices the value-scaled bond.
    fn stake_at_risk(component: MetricId, epoch: EpochId) -> futarchy_primitives::Balance;

    /// The `(component, frozen version)` pairs live cohorts consume for
    /// `measurement_epoch` (07 §2(4)) — the set the oracle must guarantee a
    /// (possibly neutral) value for by the §11 money deadline. The
    /// [`Pallet::note_settle_deadline`] crank neutral-settles every member that
    /// produced no report so welfare never reads an absent component
    /// (07 §10 no-report path / §11(1) guarantee). Enumerated by the
    /// epoch/welfare wiring at B1a (welfare owns the cohort→component map); the
    /// mock supplies a deterministic set.
    fn expected_components(
        measurement_epoch: EpochId,
    ) -> alloc::vec::Vec<(MetricId, MetricSpecVersion)>;
}

/// B4/B1a seam (07 §8): fired after `crank_reserve_probe` commits a fresh pending
/// probe. The implementation (`bleavit-xcm`'s `XcmProbeDispatcher`) builds and sends
/// the Asset Hub program. Infallible by design: a failed or missing send simply
/// leaves the probe pending until the fail-static timeout (07 §8, I-24).
pub trait ProbeDispatch {
    /// Whether this runtime binding can actually send the probe program.
    fn live(_: &OracleParams) -> bool {
        true
    }

    fn probe_due(query_id: u64, amount: futarchy_primitives::Balance);
}

impl ProbeDispatch for () {
    fn live(_: &OracleParams) -> bool {
        false
    }

    fn probe_due(_: u64, _: futarchy_primitives::Balance) {}
}

/// XCM-free observation seam fired after an unanswered reserve probe is folded.
///
/// The sink is infallible so XCM-health recording can never affect the
/// fail-static reserve-health transition or the calling keeper crank (07 §8;
/// 09 §6.4).
pub trait ProbeTimeoutSink {
    fn probe_timed_out();
}

impl ProbeTimeoutSink for () {
    fn probe_timed_out() {}
}

/// 07 §8 / 08 §1.2 seam: carries a **transition** of the reserve-health flag `R`
/// to the sibling pallets that own its consequences — the constitution's 02 §7.3
/// bit-7 mirror and the treasury's fail-static NAV haircut.
///
/// Deliberately **fallible**, unlike [`ProbeDispatch`]/[`ProbeTimeoutSink`]. A
/// partial application is the one outcome that must never commit: 08 §1.2 makes
/// `spendable_nav` zero exactly while the flag is set, so a run where the
/// constitution mirror moved and the treasury haircut did not would leave
/// `PhaseFlags` and NAV disagreeing about solvency. Returning `Err` here fails
/// the calling dispatch, and FRAME rolls back the oracle transition together
/// with whatever the sink already wrote — all three writes commit or none do
/// (G-1 status quo, R-7).
///
/// Fired only on an actual edge (`before.unhealthy != after.unhealthy`), so a
/// steady-state crank costs nothing.
pub trait ReserveHealthSink {
    fn reserve_health_changed(unhealthy: bool) -> DispatchResult;
}

impl ReserveHealthSink for () {
    fn reserve_health_changed(_: bool) -> DispatchResult {
        Ok(())
    }
}

/// Constructs a runtime origin resolving to a given authority so benchmarks can
/// drive the privileged `adjudicate` call with its exact 07 §5.4 origin.
#[cfg(feature = "runtime-benchmarks")]
pub trait BenchmarkHelper<RuntimeOrigin> {
    /// A runtime origin that [`Config::AdjudicationOrigin`] accepts.
    fn adjudication_origin() -> RuntimeOrigin;
    /// Install the real cross-pallet reporting window/spec context consumed by
    /// `report`; mock runtimes whose provider is already primed may no-op.
    fn prime_reporting(component: MetricId, epoch: EpochId, version: MetricSpecVersion);
    /// Prime the real reserve-probe budget and sibling route before measuring
    /// the production dispatch path. Pallet-only mocks may no-op.
    fn prime_reserve_probe() {}
    fn prime_keeper_rebate() {}
    fn assert_keeper_rebate_paid(_: futarchy_primitives::keeper::CrankClass) {}
}

#[frame_support::pallet]
pub mod pallet {
    use super::*;
    use alloc::vec::Vec;
    use frame_support::pallet_prelude::*;
    use frame_support::traits::EnsureOrigin;
    use frame_system::pallet_prelude::*;
    use futarchy_primitives::{
        keeper::{CrankClass, KeeperRebateSink},
        Balance, FixedU64, H256,
    };
    use sp_runtime::{traits::SaturatedConversion, TryRuntimeError};

    /// The in-code storage version of this pallet.
    const STORAGE_VERSION: StorageVersion = StorageVersion::new(1);

    #[pallet::pallet]
    #[pallet::storage_version(STORAGE_VERSION)]
    pub struct Pallet<T>(_);

    #[pallet::config]
    pub trait Config:
        frame_system::Config<
        RuntimeEvent: From<Event<Self>>,
        AccountId: From<[u8; 32]> + Into<[u8; 32]>,
    >
    {
        /// The values track admitted to settle a terminal dispute: only
        /// `OracleResolution` (07 §5.4). Wired over `pallet-origins` (A4/B1a).
        type AdjudicationOrigin: EnsureOrigin<Self::RuntimeOrigin>;

        /// The `report`-time cross-pallet reads (07 §5.1/§2(4)/§6.1).
        type Reporting: ReportingContext;

        /// Live constitution-backed oracle and reserve-probe tunables.
        type Params: OracleParamsProvider;

        /// Upper bound on rounds closed per `crank_round_close` call — a
        /// keeper-batch cap that bounds the crank's PoV (07 §13 "bounded
        /// batches"; not a 13 §1 tunable). Never hardcoded in the call body.
        #[pallet::constant]
        type MaxRoundCloseBatch: Get<u32>;

        /// Weight information for extrinsics.
        type WeightInfo: WeightInfo;

        /// Runtime XCM adapter invoked only after a fresh reserve probe commits
        /// (07 §8; B4/B1a). The pallet remains XCM-free by construction.
        type ProbeDispatch: ProbeDispatch;

        /// Infallible local-health observer invoked once per committed timeout
        /// fold (07 §8; 09 §6.4). It never changes dispatch results.
        type ProbeTimeoutSink: ProbeTimeoutSink;

        /// Fallible sibling-pallet sink for reserve-health **transitions**
        /// (07 §8; 08 §1.2). Applied inside the same dispatch as the oracle
        /// transition, so an `Err` rolls the transition back with it.
        type ReserveHealthSink: ReserveHealthSink;

        /// Infallible, fail-soft rebate sink for useful oracle cranks (07 §13,
        /// 08 §6.3). Oracle work is paid from the separate ORACLE budget line.
        type KeeperRebate: KeeperRebateSink<Self::AccountId>;

        /// Origin construction for benchmarking (see [`BenchmarkHelper`]).
        #[cfg(feature = "runtime-benchmarks")]
        type BenchmarkHelper: BenchmarkHelper<Self::RuntimeOrigin>;
    }

    /// 02 §7.2 (frozen): `Reporters: map AccountId → ReporterInfo`. Counted —
    /// ≥ `orc.n_min = 3` full stakes are required before any attested component
    /// admits (07 §3). Bounded by construction (the core rejects the 65th, I-21);
    /// `try_state` re-asserts `≤ MAX_REPORTERS`.
    #[pallet::storage]
    pub type Reporters<T: Config> =
        CountedStorageMap<_, Blake2_128Concat, T::AccountId, ReporterInfo, OptionQuery>;

    /// 02 §7.2 (frozen): `Watchtowers: map AccountId → WatchtowerInfo`. Counted,
    /// ≤ `wt.max = 16` seats (07 §4). Bounded by construction; `try_state` asserts.
    #[pallet::storage]
    pub type Watchtowers<T: Config> =
        CountedStorageMap<_, Blake2_128Concat, T::AccountId, WatchtowerInfo, OptionQuery>;

    /// 02 §7.2 (frozen name; SQ-2 key): the live reporting rounds, keyed by the
    /// `(component, epoch, spec_version)` **triple** so per-version games across a
    /// MetricSpec activation boundary each get their own round (07 §2(4)). Bounded
    /// by construction (≤ `MAX_ROUNDS`, core-enforced); `try_state` asserts.
    #[pallet::storage]
    pub type Rounds<T: Config> = StorageNMap<
        _,
        (
            NMapKey<Blake2_128Concat, MetricId>,
            NMapKey<Blake2_128Concat, EpochId>,
            NMapKey<Blake2_128Concat, MetricSpecVersion>,
        ),
        RoundState,
        OptionQuery,
    >;

    /// Internal (not FE-read): the round-one bond and terminal cap frozen when
    /// each reporting game opens. Kept parallel to [`Rounds`] so the contract-v4
    /// `RoundState` SCALE value remains byte-for-byte unchanged. One entry per
    /// live round; the shared 128-game ceiling and `try_state` correspondence
    /// bound this map.
    #[pallet::storage]
    pub type RoundSchedules<T: Config> = StorageMap<
        _,
        Blake2_128Concat,
        (MetricId, EpochId, MetricSpecVersion),
        StoredRoundSchedule,
        OptionQuery,
    >;

    /// 02 §7.2 (frozen name; SQ-2 key): the settled component values, triple-keyed
    /// like [`Rounds`]. Reaped at cohort settlement; each entry is quorum-,
    /// recompute-, adjudication- or neutral-resolved (07 §13; I-18).
    #[pallet::storage]
    pub type ComponentValues<T: Config> = StorageNMap<
        _,
        (
            NMapKey<Blake2_128Concat, MetricId>,
            NMapKey<Blake2_128Concat, EpochId>,
            NMapKey<Blake2_128Concat, MetricSpecVersion>,
        ),
        SettledComponent,
        OptionQuery,
    >;

    /// 02 §7.2 (frozen): `ReserveHealth` — the deterministic reserve-probe state
    /// (`R`, 07 §8). Single value; the zeroed default is a healthy, never-probed
    /// reserve.
    #[pallet::storage]
    pub type ReserveHealth<T: Config> = StorageValue<_, ReserveHealthValue, ValueQuery>;

    /// Internal monotone latch: the production funding/readiness gate was
    /// satisfied before the first v1 reserve attempt opened. This cannot be
    /// inferred from `last_query_id`: v0 advanced that counter even though its
    /// production dispatcher was the no-op `()` and sent no message.
    #[pallet::storage]
    pub type ReserveProbeArmed<T: Config> = StorageValue<_, bool, ValueQuery>;

    /// Internal (not FE-read): per-round watchtower acknowledgments, keyed by
    /// `report_hash` (07 §13). Bounded to live games' acks by the core's
    /// settle/escalate pruning; `try_state` asserts `≤ MAX_ACK_RECORDS`.
    #[pallet::storage]
    pub type AckRecords<T: Config> = StorageValue<
        _,
        BoundedVec<
            (MetricId, EpochId, MetricSpecVersion, u8, [u8; 32], H256),
            ConstU32<MAX_ACK_RECORDS_BOUND>,
        >,
        ValueQuery,
    >;

    /// Internal (not FE-read): watchtowers active in the current, not-yet-swept
    /// epoch (07 §4 liveness). Bounded to the seat count; cleared each
    /// `note_epoch_boundary` sweep.
    #[pallet::storage]
    pub type WatchtowerActive<T: Config> =
        StorageValue<_, BoundedVec<[u8; 32], ConstU32<MAX_WATCHTOWERS_BOUND>>, ValueQuery>;

    /// Internal (not FE-read): the `(component, frozen version)` pairs the
    /// MetricSpec registry declares deterministically recomputable (07 §2(4)/§9).
    /// `recompute_proof` fails closed for anything absent. Seeded at genesis and
    /// via [`Pallet::note_recomputable`] (welfare `register_spec`, B1a).
    #[pallet::storage]
    pub type Recomputable<T: Config> = StorageValue<
        _,
        BoundedVec<(MetricId, MetricSpecVersion), ConstU32<MAX_RECOMPUTABLE_BOUND>>,
        ValueQuery,
    >;

    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        /// A reporter registered with `orc.reporter_stake` held (07 §3).
        ReporterRegistered { who: T::AccountId, stake: Balance },
        /// A round-1 report was posted with its value-scaled bond (07 §5.1).
        Reported {
            component: MetricId,
            epoch: EpochId,
            round: u8,
            reporter: T::AccountId,
            value: FixedU64,
            evidence_hash: H256,
            bond: Balance,
        },
        /// A challenge was posted, superseding the quorum requirement (07 §5.2).
        Challenged {
            component: MetricId,
            epoch: EpochId,
            round: u8,
            challenger: T::AccountId,
            counter_value: FixedU64,
            evidence_hash: H256,
            bond: Balance,
        },
        /// A challenged round escalated; bonds doubled (07 §5.3/§6.2).
        RoundEscalated {
            component: MetricId,
            epoch: EpochId,
            round: u8,
            new_bond: Balance,
        },
        /// A round was resolved mechanically from committed evidence (07 §9).
        RecomputeProven {
            component: MetricId,
            epoch: EpochId,
            value: FixedU64,
            prover: T::AccountId,
        },
        /// A round-3 dispute was escalated to the `OracleResolution` track (07 §5.4).
        AdjudicationRequested {
            component: MetricId,
            epoch: EpochId,
            referendum: u32,
        },
        /// The values track adjudicated a terminal dispute (07 §5.4).
        Adjudicated {
            component: MetricId,
            epoch: EpochId,
            value: FixedU64,
        },
        /// A component value settled and is final for money (07 §5; I-18).
        ComponentSettled {
            component: MetricId,
            epoch: EpochId,
            value: FixedU64,
            path: SettlePath,
        },
        /// A component took the neutral path, carrying its last valid value (07 §10).
        NeutralSettlement {
            component: MetricId,
            epoch: EpochId,
            carried_value: FixedU64,
            flagged_epochs: u8,
        },
        /// A watchtower acknowledged a round as observable (07 §4).
        WindowAcknowledged {
            component: MetricId,
            epoch: EpochId,
            round: u8,
            watchtower: T::AccountId,
        },
        /// The single 48 h quorum extension fired (07 §4).
        WindowExtended {
            component: MetricId,
            epoch: EpochId,
            round: u8,
            new_deadline: BlockNumber,
        },
        /// No challenge and no quorum after the extension ⇒ neutral (07 §4).
        QuorumFailed {
            component: MetricId,
            epoch: EpochId,
            round: u8,
        },
        /// A reporter's bond stack was slashed on a second offense (07 §3/§5.5).
        ReporterSlashed {
            who: T::AccountId,
            amount: Balance,
            offense: u8,
        },
        /// A reporter was ejected on the third offense (07 §3).
        ReporterEjected { who: T::AccountId },
        /// A watchtower registered with `wt.stake` held (07 §4).
        WatchtowerRegistered { who: T::AccountId, stake: Balance },
        /// A watchtower was marked inactive for an epoch (07 §4).
        WatchtowerInactive { who: T::AccountId, epoch: EpochId },
        /// A watchtower's stake was slashed for liveness failure (07 §4).
        WatchtowerSlashed { who: T::AccountId, amount: Balance },
        /// A reserve-transferability probe was sent (07 §8).
        ReserveProbeSent { query_id: u64 },
        /// A probe outcome was recorded (07 §8).
        ReserveProbeResult { query_id: u64, passed: bool },
        /// The reserve entered the unhealthy fail-static state (07 §8).
        ReserveUnhealthy,
        /// The reserve recovered after `res.recover_threshold` passes (07 §8).
        ReserveRecovered,
    }

    /// 1:1 with [`CoreError`]; `CoreError::BadOrigin` maps to
    /// `DispatchError::BadOrigin` (FRAME convention) and never appears here.
    #[pallet::error]
    pub enum Error<T> {
        /// Caller is already a registered reporter/watchtower (07 §3/§4).
        AlreadyRegistered,
        /// Caller is not a registered reporter/watchtower (07 §3/§4).
        NotRegistered,
        /// Reporter registry is full (`MAX_REPORTERS`).
        TooManyReporters,
        /// Watchtower registry is full (`wt.max = 16`).
        TooManyWatchtowers,
        /// The challenge/report window has closed (07 §5).
        WindowClosed,
        /// The window is still open / round not yet resolvable (07 §5).
        WindowOpen,
        /// Posted bond is below the value-scaled minimum (07 §6).
        BondBelowMinimum,
        /// The report names a version other than the frozen cohort version (07 §2(4)).
        SpecVersionMismatch,
        /// The `(component, epoch, version)` is already settled — final (I-18).
        AlreadyFinal,
        /// This round already carries a challenge (07 §5.2).
        AlreadyChallenged,
        /// A quorum decision is still pending for this round (07 §4).
        QuorumPending,
        /// No round exists for the given key (07 §5).
        RoundNotFound,
        /// Live-round registry is full (`MAX_ROUNDS`).
        RoundLimit,
        /// This watchtower already acknowledged this round (07 §4).
        DuplicateAck,
        /// A reserve-unhealthy condition blocked the action (07 §8).
        ReserveUnhealthy,
        /// The reserve probe interval has not elapsed (07 §8).
        ProbeTooEarly,
        /// The reserve probe has not yet passed its funding/readiness gate.
        ProbeUnavailable,
        /// The `query_id` does not match the outstanding probe (07 §8).
        UnknownQuery,
        /// Arithmetic overflow — rejected, never wrapped (G-1).
        Overflow,
        /// The frozen spec does not declare this component recomputable (07 §9).
        NotRecomputable,
        /// `recompute_proof` payload exceeds `orc.max_proof_bytes` (07 §9).
        ProofTooLarge,
        /// The proof does not match the committed evidence hash (07 §9).
        EvidenceMismatch,
        /// The committed payload does not decode to a valid value (07 §9).
        BadProof,
        /// A reported/adjudicated value is off the 05 §4.4 `[0, 1]` grid.
        ValueOutOfBounds,
        /// Core state validator rejected the aggregate (try-state only).
        TryStateViolation,
    }

    #[pallet::hooks]
    impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
        // 15 §1 try-state: the core machine invariants (I-18 no live round for a
        // settled key; per-round ack consistency; round/bond shape) plus the
        // storage-shape bounds. Reserve probe I-24 is convention-class (no
        // on-chain structure; certified by the probe-timeout drill, 15 §1).
        #[cfg(feature = "try-runtime")]
        fn try_state(_n: BlockNumberFor<T>) -> Result<(), TryRuntimeError> {
            Self::do_try_state()
        }
    }

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        /// `oracle.register_reporter` — permissionless entry, `orc.reporter_stake`
        /// held (07 §3). Signed.
        #[pallet::call_index(0)]
        #[pallet::weight(T::WeightInfo::register_reporter())]
        pub fn register_reporter(origin: OriginFor<T>) -> DispatchResult {
            let who: [u8; 32] = ensure_signed(origin)?.into();
            let now = Self::now();
            let params = T::Params::get();
            Self::mutate_core(|o| o.register_reporter_with_params(who, now, &params))
        }

        /// `oracle.deregister_reporter` — exit once every round the reporter
        /// participated in is closed; stake returned (07 §3). Signed.
        #[pallet::call_index(1)]
        #[pallet::weight(T::WeightInfo::deregister_reporter())]
        pub fn deregister_reporter(origin: OriginFor<T>) -> DispatchResult {
            let who: [u8; 32] = ensure_signed(origin)?.into();
            Self::mutate_core(|o| o.deregister_reporter(who))
        }

        /// `oracle.report` — attest one value for `(component, epoch)` under the
        /// frozen spec version, round-1 bond held (07 §5.1). Signed by a
        /// registered reporter. The window end, expected version and `StakeAtRisk`
        /// are derived via [`Config::Reporting`], never taken from the caller.
        #[pallet::call_index(2)]
        #[pallet::weight(T::WeightInfo::report())]
        pub fn report(
            origin: OriginFor<T>,
            component: MetricId,
            epoch: EpochId,
            spec_version: MetricSpecVersion,
            value: FixedU64,
            evidence_hash: H256,
        ) -> DispatchResult {
            let who: [u8; 32] = ensure_signed(origin)?.into();
            let now = Self::now();
            ensure!(
                T::Reporting::is_expected_spec_version(component, epoch, spec_version),
                Error::<T>::SpecVersionMismatch
            );
            let report_window_end = T::Reporting::report_window_end(epoch);
            let stake_at_risk = T::Reporting::stake_at_risk(component, epoch);
            let params = T::Params::get();
            Self::mutate_core(|o| {
                o.report(
                    ReportInput {
                        who,
                        now,
                        component,
                        epoch,
                        spec_version,
                        value,
                        evidence_hash,
                        stake_at_risk,
                        // Validated above against the live frozen-version set; the
                        // core's equality check is then a no-op (Codex F7).
                        report_window_end,
                        expected_spec: spec_version,
                    },
                    &params,
                )
            })
        }

        /// `oracle.challenge` — post the current-round bond against a report;
        /// proof of observability that supersedes the quorum rule (07 §5.2).
        /// Signed. `spec_version` disambiguates per-version games (07 §2(4)).
        #[pallet::call_index(3)]
        #[pallet::weight(T::WeightInfo::challenge())]
        pub fn challenge(
            origin: OriginFor<T>,
            component: MetricId,
            epoch: EpochId,
            spec_version: MetricSpecVersion,
            counter_value: FixedU64,
            evidence_hash: H256,
        ) -> DispatchResult {
            let who: [u8; 32] = ensure_signed(origin)?.into();
            let now = Self::now();
            let key = RoundKey {
                component,
                epoch,
                spec_version,
            };
            Self::mutate_core(|o| o.challenge(who, now, key, counter_value, evidence_hash))
        }

        /// `oracle.recompute_proof` — permissionless mechanical resolution from
        /// the committed evidence, bounded at `orc.max_proof_bytes` (07 §9).
        /// Signed (keeper, rebated). Fails closed for non-recomputable components.
        #[pallet::call_index(4)]
        #[pallet::weight(T::WeightInfo::recompute_proof(proof.len() as u32))]
        pub fn recompute_proof(
            origin: OriginFor<T>,
            component: MetricId,
            epoch: EpochId,
            spec_version: MetricSpecVersion,
            proof: BoundedVec<u8, ConstU32<MAX_PROOF_BYTES_BOUND>>,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;
            let prover: [u8; 32] = who.clone().into();
            let key = RoundKey {
                component,
                epoch,
                spec_version,
            };
            Self::mutate_core(|o| o.recompute_proof(prover, key, proof.as_slice()))?;
            // B5 recalibrates this weight for the post-commit rebate write/payout.
            T::KeeperRebate::rebate(&who, CrankClass::OracleLine);
            Ok(())
        }

        /// `oracle.register_watchtower` — permissionless-with-stake entry,
        /// `wt.stake` held, ≤ `wt.max = 16` seats (07 §4). Signed.
        #[pallet::call_index(5)]
        #[pallet::weight(T::WeightInfo::register_watchtower())]
        pub fn register_watchtower(origin: OriginFor<T>) -> DispatchResult {
            let who: [u8; 32] = ensure_signed(origin)?.into();
            let now = Self::now();
            let params = T::Params::get();
            Self::mutate_core(|o| o.register_watchtower_with_params(who, now, &params))
        }

        /// `oracle.ack_observed` — a registered watchtower asserts a round was
        /// visible in a finalized block; O(1), keeper-class rebate (07 §4).
        /// Signed. `spec_version` selects the per-version round.
        #[pallet::call_index(6)]
        #[pallet::weight(T::WeightInfo::ack_observed())]
        pub fn ack_observed(
            origin: OriginFor<T>,
            component: MetricId,
            epoch: EpochId,
            spec_version: MetricSpecVersion,
            round: u8,
            report_hash: H256,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;
            let raw_who: [u8; 32] = who.clone().into();
            let now = Self::now();
            let key = RoundKey {
                component,
                epoch,
                spec_version,
            };
            Self::mutate_core(|o| o.ack_observed(raw_who, now, key, round, report_hash))?;
            // B5 recalibrates this weight for the post-commit rebate write/payout.
            T::KeeperRebate::rebate(&who, CrankClass::OracleLine);
            Ok(())
        }

        /// `oracle.crank_round_close(batch)` — permissionless bounded crank that
        /// resolves matured rounds: quorum ⇒ final; no quorum ⇒ one extension then
        /// neutral; challenged ⇒ escalate (07 §4/§5). Signed (keeper, rebated).
        #[pallet::call_index(7)]
        #[pallet::weight(T::WeightInfo::crank_round_close(T::MaxRoundCloseBatch::get()))]
        pub fn crank_round_close(origin: OriginFor<T>, batch: u32) -> DispatchResult {
            let who = ensure_signed(origin)?;
            let now = Self::now();
            let batch = batch.min(T::MaxRoundCloseBatch::get()) as usize;
            let params = T::Params::get();
            let progressed = Self::mutate_core_with_rebate_progress(|o| {
                o.crank_round_close_with_params(now, batch, &params)
            })?;
            if progressed {
                // B5 recalibrates this weight for the post-commit rebate write/payout.
                T::KeeperRebate::rebate(&who, CrankClass::OracleLine);
            }
            Ok(())
        }

        /// `oracle.crank_reserve_probe` — permissionless probe crank: first counts
        /// any timed-out outstanding probe as a fail (fail-static, 07 §8), then
        /// sends the next probe if `res.probe_interval` has elapsed. Signed
        /// (keeper, rebated). The pallet commits state first, then fires the
        /// XCM-free [`ProbeDispatch`] seam; send failure remains fail-static
        /// through the pending probe's timeout (I-24, rule 7).
        #[pallet::call_index(8)]
        #[pallet::weight(T::WeightInfo::crank_reserve_probe())]
        pub fn crank_reserve_probe(origin: OriginFor<T>) -> DispatchResult {
            let who = ensure_signed(origin)?;
            let was_armed = ReserveProbeArmed::<T>::get();
            let params = T::Params::get();
            // Evaluate readiness on both first and recurring paths so the
            // production storage/weight envelope is branch-independent. Only
            // the unarmed path is refused when current readiness is false.
            let dispatch_live = T::ProbeDispatch::live(&params);
            let ready = params.reserve_probe_config_valid() && dispatch_live;
            ensure!(was_armed || ready, Error::<T>::ProbeUnavailable);
            let now = Self::now();
            let (fresh_query_id, folded_timeout, missed) = Self::mutate_reserve_health(|o| {
                let mut folded = false;
                if let Some(since) = o.reserve_health.pending_since {
                    if now >= since.saturating_add(params.probe_timeout) {
                        // The outstanding probe never got a response: absence is
                        // never healthy (07 §8) — count it before sending anew.
                        o.crank_probe_timeout_with_params(now, &params)?;
                        folded = true;
                    }
                }

                // Do not overwrite a still-live outstanding query even if a
                // future parameter amendment makes timeout exceed cadence.
                if o.reserve_health.pending_since.is_some() {
                    return Err(CoreError::ProbeTooEarly);
                }

                let interval = params.probe_interval.max(1);
                // The first readiness-qualified call opens the first attempt
                // immediately, even before one nominal interval has elapsed.
                // That attempt establishes the cadence anchor; pre-arm wall
                // clock time is not an outage and scores no missed slots.
                let elapsed_slots = if was_armed {
                    now.saturating_sub(o.reserve_health.last_probe_at) / interval
                } else {
                    1
                };
                if elapsed_slots > 0 {
                    // Once the disjoint wire namespace is exhausted, opening a
                    // new attempt is impossible forever. Do not let the core's
                    // `Overflow` roll back a timeout or erase completed
                    // no-attempt slots: every elapsed slot is a fail-static
                    // miss, and advancing the cadence anchor keeps subsequent
                    // cranks bounded to the newly elapsed interval.
                    if o.reserve_health.last_query_id >= MAX_RESERVE_PROBE_QUERY_ID {
                        o.note_missed_reserve_probes_with_params(elapsed_slots, &params);
                        o.reserve_health.last_probe_at = now;
                        return Ok((None, folded, elapsed_slots));
                    }
                    // The final elapsed slot is the attempt opened below. Every
                    // earlier slot had no keeper attempt and therefore scores
                    // as a fail (SQ-385 literal fail-static ruling). A timeout
                    // folded above belongs to the previously-opened attempt,
                    // not to any of these missed cadence slots.
                    // Cadence begins only when the readiness-qualified v1 arm
                    // opens its first real attempt. Pre-arm wall-clock slots
                    // were not measurement outages and must not manufacture an
                    // unhealthy state before the probe existed.
                    let missed = if was_armed {
                        elapsed_slots.saturating_sub(1)
                    } else {
                        0
                    };
                    o.note_missed_reserve_probes_with_params(missed, &params);
                    let query_id = o.crank_reserve_probe_with_params(now, &params)?;
                    Ok((Some(query_id), folded, missed))
                } else if folded {
                    // Commit a just-reached timeout even though the next daily
                    // slot has not opened yet (G-1; Codex F3).
                    Ok((None, true, 0))
                } else {
                    Err(CoreError::ProbeTooEarly)
                }
            })?;
            if folded_timeout {
                T::ProbeTimeoutSink::probe_timed_out();
            }
            if let Some(query_id) = fresh_query_id {
                if !was_armed {
                    ReserveProbeArmed::<T>::put(true);
                }
                T::ProbeDispatch::probe_due(query_id, params.probe_amount);
            }
            if fresh_query_id.is_some() || folded_timeout || missed > 0 {
                // B5 recalibrates this weight for the post-commit rebate write/payout.
                T::KeeperRebate::rebate(&who, CrankClass::OracleLine);
            }
            Ok(())
        }

        /// `oracle.adjudicate` — the sole privileged call: the `OracleResolution`
        /// values track settles a terminal dispute and, if the reporter is found
        /// wrong, forfeits its bond stack (07 §5.4/§5.5).
        #[pallet::call_index(9)]
        #[pallet::weight(T::WeightInfo::adjudicate())]
        pub fn adjudicate(
            origin: OriginFor<T>,
            component: MetricId,
            epoch: EpochId,
            spec_version: MetricSpecVersion,
            value: FixedU64,
            reporter_wrong: bool,
        ) -> DispatchResult {
            T::AdjudicationOrigin::ensure_origin(origin)?;
            let key = RoundKey {
                component,
                epoch,
                spec_version,
            };
            Self::mutate_core(|o| {
                o.adjudicate(
                    origins_core::Origin::OracleResolution,
                    key,
                    value,
                    reporter_wrong,
                )
            })
        }
    }

    #[pallet::genesis_config]
    pub struct GenesisConfig<T: Config> {
        /// `(component, frozen version)` pairs the genesis MetricSpec set declares
        /// deterministically recomputable (07 §2(4)/§9). Reporters and watchtowers
        /// start empty (permissionless entry).
        pub recomputable_components: Vec<(MetricId, MetricSpecVersion)>,
        #[serde(skip)]
        pub _config: core::marker::PhantomData<T>,
    }

    impl<T: Config> Default for GenesisConfig<T> {
        fn default() -> Self {
            Self {
                recomputable_components: Vec::new(),
                _config: core::marker::PhantomData,
            }
        }
    }

    #[pallet::genesis_build]
    impl<T: Config> BuildGenesisConfig for GenesisConfig<T> {
        // Genesis-time `assert!` is the FRAME chain-spec convention (runs before
        // any block, so the G-1 dispatch-path rule does not apply).
        fn build(&self) {
            let recomputable: BoundedVec<
                (MetricId, MetricSpecVersion),
                ConstU32<MAX_RECOMPUTABLE_BOUND>,
            > = BoundedVec::truncate_from(self.recomputable_components.clone());
            assert!(
                recomputable.len() == self.recomputable_components.len(),
                "oracle genesis: recomputable set over the storage bound"
            );
            Recomputable::<T>::put(recomputable);
            ReserveHealth::<T>::put(ReserveHealthValue::default());
        }
    }

    impl<T: Config> Pallet<T> {
        // ---- Runtime-internal APIs (not extrinsics; wired by sibling pallets) ----

        /// XCM `QueryResponse` handler for the reserve probe (07 §8, B4). Records
        /// the outcome for the outstanding `query_id`; a response at or after the
        /// `res.probe_timeout` deadline is counted as a fail regardless of the
        /// reported outcome (fail-static — Codex F2), and a stale/unknown id is
        /// dropped (the core rejects it and no state moves). Not an extrinsic —
        /// reachable only from the runtime's XCM query router, which supplies the
        /// current block.
        pub fn reserve_probe_result(query_id: u64, passed: bool) -> DispatchResult {
            let now = Self::now();
            let params = T::Params::get();
            Self::mutate_reserve_health(|o| {
                o.reserve_probe_result_with_params(now, query_id, passed, &params)
            })
        }

        /// Escalate a round-3 dispute onto the `OracleResolution` track, recording
        /// the created `referendum` index (07 §5.4). Not an extrinsic — the
        /// referendum is opened by the governance wiring (06/B-track) which then
        /// calls this. `AdjudicationRequested` is an event, not a call (02 §7.2).
        pub fn request_adjudication(
            component: MetricId,
            epoch: EpochId,
            spec_version: MetricSpecVersion,
            referendum: u32,
        ) -> DispatchResult {
            let key = RoundKey {
                component,
                epoch,
                spec_version,
            };
            Self::mutate_core(|o| o.request_adjudication(key, referendum))
        }

        /// Run the 07 §4 watchtower liveness sweep for a just-ended epoch. Not an
        /// extrinsic — the epoch pallet calls this at each epoch rollover (B1a),
        /// passing the ended epoch and whether it carried an open oracle round
        /// (the schedule + round history live in the epoch pallet). Charges
        /// inactivity, slashes/ejects on the second consecutive miss, and clears
        /// the activity set. `had_open_round = false` charges nobody.
        pub fn note_epoch_boundary(ended_epoch: EpochId, had_open_round: bool) -> DispatchResult {
            Self::mutate_core(|o| o.sweep_watchtower_liveness(ended_epoch, had_open_round))
        }

        /// Force-neutralize `measurement_epoch` at its 07 §11 `OracleSettleDeadline`
        /// (d20). Not an extrinsic — the epoch pallet calls this at the
        /// schedule-derived deadline (B1a). Neutral-settles every still-live round
        /// (so none survives money-bearing, 07 §13; I-18) **and**, for every
        /// expected component that produced no report, writes the neutral
        /// no-report value welfare reads (07 §10/§11(1)) — the expected set comes
        /// from the [`ReportingContext`] provider (welfare owns the cohort→component
        /// map). A late verdict then finds no round and settles bonds only.
        pub fn note_settle_deadline(measurement_epoch: EpochId) -> DispatchResult {
            let expected = T::Reporting::expected_components(measurement_epoch);
            Self::mutate_core(|o| o.force_neutralize_expired(measurement_epoch, &expected))
        }

        /// Declare a `(component, frozen version)` deterministically recomputable
        /// (07 §2(4)/§9). Called by welfare `register_spec` (A7/B1a). Idempotent;
        /// bounded — over-bound declarations are rejected (G-1).
        pub fn note_recomputable(
            component: MetricId,
            version: MetricSpecVersion,
        ) -> DispatchResult {
            Recomputable::<T>::try_mutate(|set| -> DispatchResult {
                if set.iter().any(|(c, v)| *c == component && *v == version) {
                    return Ok(());
                }
                set.try_push((component, version))
                    .map_err(|_| Error::<T>::RoundLimit)?;
                Ok(())
            })
        }

        // ---- Read accessors for sibling pallets / views ----

        /// The settled value for a `(component, epoch, version)`, if final (07 §13;
        /// welfare reads this at settlement — I-18).
        pub fn settled_component(
            component: MetricId,
            epoch: EpochId,
            version: MetricSpecVersion,
        ) -> Option<SettledComponent> {
            ComponentValues::<T>::get((component, epoch, version))
        }

        /// Reap a settled value at cohort settlement (07 §13; A8/B1a wiring).
        /// This is an internal sibling-pallet seam, not a Signed extrinsic, so it
        /// deliberately carries no keeper rebate.
        pub fn reap_component(component: MetricId, epoch: EpochId, version: MetricSpecVersion) {
            ComponentValues::<T>::remove((component, epoch, version));
        }

        /// The reserve-health flag `R` (07 §8): the constitution `PhaseFlags`
        /// bit-7 mirror and the `PB-RESERVE` split-inflow halt read this (B1a).
        pub fn reserve_unhealthy() -> bool {
            ReserveHealth::<T>::get().unhealthy
        }

        /// Whether the one-time reserve-probe readiness gate has completed.
        pub fn reserve_probe_armed() -> bool {
            ReserveProbeArmed::<T>::get()
        }

        // ---- Hydrate / persist (Track-A "load → call core → persist") ----

        /// Rebuild the whole [`Oracle`] aggregate from storage. Reads are bounded
        /// by the storage bounds; granular per-op hydration is a B5 weight
        /// optimization (the correctness contract is the exact core behavior).
        fn load() -> Oracle {
            let reporters = Reporters::<T>::iter()
                .map(|(a, info)| (a.into(), info))
                .collect::<Vec<_>>();
            let watchtowers = Watchtowers::<T>::iter()
                .map(|(a, info)| (a.into(), info))
                .collect::<Vec<_>>();
            let mut rounds = Rounds::<T>::iter_values().collect::<Vec<RoundState>>();
            // Deterministic order so batch cranks are reproducible regardless of
            // storage hasher order.
            rounds.sort_unstable_by_key(|r| (r.component, r.epoch, r.spec_version, r.round));
            let mut round_schedules = RoundSchedules::<T>::iter()
                .map(|((component, epoch, spec_version), schedule)| {
                    (
                        RoundKey {
                            component,
                            epoch,
                            spec_version,
                        },
                        schedule,
                    )
                })
                .collect::<Vec<_>>();
            round_schedules
                .sort_unstable_by_key(|(key, _)| (key.component, key.epoch, key.spec_version));
            let component_values = ComponentValues::<T>::iter().collect::<Vec<_>>();
            Oracle {
                reporters,
                watchtowers,
                rounds,
                round_schedules,
                component_values,
                reserve_health: ReserveHealth::<T>::get(),
                events: Vec::new(),
                ack_records: AckRecords::<T>::get().into_inner(),
                recomputable_components: Recomputable::<T>::get().into_inner(),
                watchtower_active: WatchtowerActive::<T>::get().into_inner(),
            }
        }

        /// Hydrate → run the core op → persist the diff → deposit the op's events.
        /// On `Err` nothing is written (both because we return before `persist`
        /// and because FRAME rolls the dispatch back — G-1 status quo).
        fn mutate_core(op: impl FnOnce(&mut Oracle) -> Result<(), CoreError>) -> DispatchResult {
            Self::mutate_core_with_rebate_progress(op).map(|_| ())
        }

        /// Run a reserve-health-only core operation without hydrating any
        /// reporter, round, acknowledgment or component-value storage. This is
        /// the 07 §13 O(1) QueryResponse path: one authoritative record, the
        /// edge-triggered mirrors, and the operation's bounded events.
        fn mutate_reserve_health<R>(
            op: impl FnOnce(&mut Oracle) -> Result<R, CoreError>,
        ) -> Result<R, DispatchError> {
            frame_support::storage::with_storage_layer(|| {
                let before = ReserveHealth::<T>::get();
                let mut oracle = Oracle {
                    reserve_health: before,
                    ..Oracle::default()
                };
                let result = op(&mut oracle).map_err(Self::map_core_error)?;
                let after = oracle.reserve_health;
                if before != after {
                    ReserveHealth::<T>::put(after);
                }
                if before.unhealthy != after.unhealthy {
                    T::ReserveHealthSink::reserve_health_changed(after.unhealthy)?;
                }
                Self::deposit_core_events(core::mem::take(&mut oracle.events));
                Ok(result)
            })
        }

        /// Hydrate → run → persist and report whether a round advanced through
        /// settlement, escalation, or its latch-once window extension. The
        /// result is returned only after storage/events commit, so no-op/error
        /// calls cannot earn rebates.
        fn mutate_core_with_rebate_progress(
            op: impl FnOnce(&mut Oracle) -> Result<(), CoreError>,
        ) -> Result<bool, DispatchError> {
            // The explicit layer — rather than relying on the caller's dispatch
            // rollback — is what makes the 07 §8 / 08 §1.2 reserve-health sink
            // atomic for *every* entry point, including the runtime-internal
            // `reserve_probe_result` the XCM `QueryResponse` router calls
            // outside any extrinsic (R-7; G-1 status quo on every failure).
            frame_support::storage::with_storage_layer(|| {
                let before = Self::load();
                let mut oracle = before.clone();
                op(&mut oracle).map_err(Self::map_core_error)?;
                let rebate_progress = oracle.events.iter().any(|event| {
                    matches!(
                        event,
                        CoreEvent::RoundEscalated { .. }
                            | CoreEvent::ComponentSettled { .. }
                            | CoreEvent::WindowExtended { .. }
                    )
                });
                Self::persist(&before, &oracle);
                // 08 §1.2 makes `spendable_nav` zero exactly while `R` is set, so
                // the constitution mirror and the treasury haircut must move with
                // the transition or not at all: an `Err` here unwinds the oracle
                // write above together with whatever the sink already wrote.
                if before.reserve_health.unhealthy != oracle.reserve_health.unhealthy {
                    T::ReserveHealthSink::reserve_health_changed(oracle.reserve_health.unhealthy)?;
                }
                Self::deposit_core_events(core::mem::take(&mut oracle.events));
                Ok(rebate_progress)
            })
        }

        /// Write only what changed between `before` and `after` (minimal storage
        /// writes; in-memory diffing over bounded ≤ 64/16 collections).
        fn persist(before: &Oracle, after: &Oracle) {
            if before.reporters != after.reporters {
                for (a, _) in &before.reporters {
                    if !after.reporters.iter().any(|(b, _)| b == a) {
                        Reporters::<T>::remove(T::AccountId::from(*a));
                    }
                }
                for (a, info) in &after.reporters {
                    if before
                        .reporters
                        .iter()
                        .find(|(b, _)| b == a)
                        .map(|(_, i)| i)
                        != Some(info)
                    {
                        Reporters::<T>::insert(T::AccountId::from(*a), info);
                    }
                }
            }
            if before.watchtowers != after.watchtowers {
                for (a, _) in &before.watchtowers {
                    if !after.watchtowers.iter().any(|(b, _)| b == a) {
                        Watchtowers::<T>::remove(T::AccountId::from(*a));
                    }
                }
                for (a, info) in &after.watchtowers {
                    if before
                        .watchtowers
                        .iter()
                        .find(|(b, _)| b == a)
                        .map(|(_, i)| i)
                        != Some(info)
                    {
                        Watchtowers::<T>::insert(T::AccountId::from(*a), info);
                    }
                }
            }
            if before.rounds != after.rounds {
                for r in &before.rounds {
                    if !after.rounds.iter().any(|s| Self::same_round(s, r)) {
                        Rounds::<T>::remove((r.component, r.epoch, r.spec_version));
                    }
                }
                for r in &after.rounds {
                    let existing = before
                        .rounds
                        .iter()
                        .find(|s| Self::same_round(s, r))
                        .copied();
                    if existing != Some(*r) {
                        Rounds::<T>::insert((r.component, r.epoch, r.spec_version), *r);
                    }
                }
            }
            if before.round_schedules != after.round_schedules {
                for (key, _) in &before.round_schedules {
                    if !after
                        .round_schedules
                        .iter()
                        .any(|(stored, _)| stored == key)
                    {
                        RoundSchedules::<T>::remove((key.component, key.epoch, key.spec_version));
                    }
                }
                for (key, schedule) in &after.round_schedules {
                    let existing = before
                        .round_schedules
                        .iter()
                        .find(|(stored, _)| stored == key)
                        .map(|(_, stored)| stored);
                    if existing != Some(schedule) {
                        RoundSchedules::<T>::insert(
                            (key.component, key.epoch, key.spec_version),
                            schedule,
                        );
                    }
                }
            }
            if before.component_values != after.component_values {
                for ((c, e, v), _) in &before.component_values {
                    if !after
                        .component_values
                        .iter()
                        .any(|((c2, e2, v2), _)| c2 == c && e2 == e && v2 == v)
                    {
                        ComponentValues::<T>::remove((*c, *e, *v));
                    }
                }
                for ((c, e, v), settled) in &after.component_values {
                    let existing = before
                        .component_values
                        .iter()
                        .find(|((c2, e2, v2), _)| c2 == c && e2 == e && v2 == v)
                        .map(|(_, s)| s);
                    if existing != Some(settled) {
                        ComponentValues::<T>::insert((*c, *e, *v), settled);
                    }
                }
            }
            if before.reserve_health != after.reserve_health {
                ReserveHealth::<T>::put(after.reserve_health);
            }
            if before.ack_records != after.ack_records {
                // Bounded by the core's settle/escalate pruning; `try_state`
                // asserts. `truncate_from` cannot lose data here.
                AckRecords::<T>::put(BoundedVec::truncate_from(after.ack_records.clone()));
            }
            if before.recomputable_components != after.recomputable_components {
                Recomputable::<T>::put(BoundedVec::truncate_from(
                    after.recomputable_components.clone(),
                ));
            }
            if before.watchtower_active != after.watchtower_active {
                WatchtowerActive::<T>::put(BoundedVec::truncate_from(
                    after.watchtower_active.clone(),
                ));
            }
        }

        fn same_round(a: &RoundState, b: &RoundState) -> bool {
            a.component == b.component && a.epoch == b.epoch && a.spec_version == b.spec_version
        }

        fn now() -> BlockNumber {
            frame_system::Pallet::<T>::block_number().saturated_into::<u32>()
        }

        /// Deposit each event the core op produced, converting the core's raw
        /// `[u8; 32]` accounts into `T::AccountId` (02 §7.2 event set, 1:1).
        fn deposit_core_events(events: Vec<CoreEvent>) {
            for ev in events {
                let mapped = match ev {
                    CoreEvent::ReporterRegistered { who, stake } => Event::ReporterRegistered {
                        who: who.into(),
                        stake,
                    },
                    CoreEvent::Reported {
                        component,
                        epoch,
                        round,
                        reporter,
                        value,
                        evidence_hash,
                        bond,
                    } => Event::Reported {
                        component,
                        epoch,
                        round,
                        reporter: reporter.into(),
                        value,
                        evidence_hash,
                        bond,
                    },
                    CoreEvent::Challenged {
                        component,
                        epoch,
                        round,
                        challenger,
                        counter_value,
                        evidence_hash,
                        bond,
                    } => Event::Challenged {
                        component,
                        epoch,
                        round,
                        challenger: challenger.into(),
                        counter_value,
                        evidence_hash,
                        bond,
                    },
                    CoreEvent::RoundEscalated {
                        component,
                        epoch,
                        round,
                        new_bond,
                    } => Event::RoundEscalated {
                        component,
                        epoch,
                        round,
                        new_bond,
                    },
                    CoreEvent::RecomputeProven {
                        component,
                        epoch,
                        value,
                        prover,
                    } => Event::RecomputeProven {
                        component,
                        epoch,
                        value,
                        prover: prover.into(),
                    },
                    CoreEvent::AdjudicationRequested {
                        component,
                        epoch,
                        referendum,
                    } => Event::AdjudicationRequested {
                        component,
                        epoch,
                        referendum,
                    },
                    CoreEvent::Adjudicated {
                        component,
                        epoch,
                        value,
                    } => Event::Adjudicated {
                        component,
                        epoch,
                        value,
                    },
                    CoreEvent::ComponentSettled {
                        component,
                        epoch,
                        value,
                        path,
                    } => Event::ComponentSettled {
                        component,
                        epoch,
                        value,
                        path,
                    },
                    CoreEvent::NeutralSettlement {
                        component,
                        epoch,
                        carried_value,
                        flagged_epochs,
                    } => Event::NeutralSettlement {
                        component,
                        epoch,
                        carried_value,
                        flagged_epochs,
                    },
                    CoreEvent::WindowAcknowledged {
                        component,
                        epoch,
                        round,
                        watchtower,
                    } => Event::WindowAcknowledged {
                        component,
                        epoch,
                        round,
                        watchtower: watchtower.into(),
                    },
                    CoreEvent::WindowExtended {
                        component,
                        epoch,
                        round,
                        new_deadline,
                    } => Event::WindowExtended {
                        component,
                        epoch,
                        round,
                        new_deadline,
                    },
                    CoreEvent::QuorumFailed {
                        component,
                        epoch,
                        round,
                    } => Event::QuorumFailed {
                        component,
                        epoch,
                        round,
                    },
                    CoreEvent::ReporterSlashed {
                        who,
                        amount,
                        offense,
                    } => Event::ReporterSlashed {
                        who: who.into(),
                        amount,
                        offense,
                    },
                    CoreEvent::ReporterEjected { who } => {
                        Event::ReporterEjected { who: who.into() }
                    }
                    CoreEvent::WatchtowerRegistered { who, stake } => Event::WatchtowerRegistered {
                        who: who.into(),
                        stake,
                    },
                    CoreEvent::WatchtowerInactive { who, epoch } => Event::WatchtowerInactive {
                        who: who.into(),
                        epoch,
                    },
                    CoreEvent::WatchtowerSlashed { who, amount } => Event::WatchtowerSlashed {
                        who: who.into(),
                        amount,
                    },
                    CoreEvent::ReserveProbeSent { query_id } => {
                        Event::ReserveProbeSent { query_id }
                    }
                    CoreEvent::ReserveProbeResult { query_id, passed } => {
                        Event::ReserveProbeResult { query_id, passed }
                    }
                    CoreEvent::ReserveUnhealthy => Event::ReserveUnhealthy,
                    CoreEvent::ReserveRecovered => Event::ReserveRecovered,
                };
                Self::deposit_event(mapped);
            }
        }

        /// Rebuild the core aggregate from storage and run its reviewed validator
        /// plus the FRAME storage-shape bounds (15 §1; I-18).
        pub fn do_try_state() -> Result<(), TryRuntimeError> {
            if StorageVersion::get::<Pallet<T>>() != STORAGE_VERSION {
                return Err(TryRuntimeError::Other(
                    "oracle: on-chain storage version is not v1",
                ));
            }
            let oracle = Self::load();
            // Counter/iteration agreement for the counted maps.
            if oracle.reporters.len() != Reporters::<T>::count() as usize {
                return Err(TryRuntimeError::Other(
                    "Reporters: counter diverges from iterated entries",
                ));
            }
            if oracle.watchtowers.len() != Watchtowers::<T>::count() as usize {
                return Err(TryRuntimeError::Other(
                    "Watchtowers: counter diverges from iterated entries",
                ));
            }
            // Physical NMap key must equal the triple the value embeds — `load()`
            // hydrates from the value, so a key/value divergence (e.g. after a
            // bad migration) would otherwise be invisible (Codex F1).
            for ((c, e, v), round) in Rounds::<T>::iter() {
                if (c, e, v) != (round.component, round.epoch, round.spec_version) {
                    return Err(TryRuntimeError::Other(
                        "Rounds: physical key diverges from embedded (component, epoch, version)",
                    ));
                }
                let schedule = RoundSchedules::<T>::get((c, e, v)).ok_or(
                    TryRuntimeError::Other("Rounds: live game is missing its frozen schedule"),
                )?;
                if !(futarchy_primitives::kernel::ORC_ROUNDS_MIN
                    ..=futarchy_primitives::kernel::ORC_ROUNDS_MAX)
                    .contains(&schedule.round_cap)
                    || !(1..=schedule.round_cap).contains(&round.round)
                    || stored_round_bond(schedule.round_one_bond, round.round, schedule.round_cap)
                        != Ok(round.bond)
                {
                    return Err(TryRuntimeError::Other(
                        "RoundSchedules: frozen schedule is outside the kernel envelope",
                    ));
                }
            }
            for (key, _) in RoundSchedules::<T>::iter() {
                if !Rounds::<T>::contains_key(key) {
                    return Err(TryRuntimeError::Other(
                        "RoundSchedules: frozen schedule has no live round",
                    ));
                }
            }
            if ReserveProbeArmed::<T>::get() && oracle.reserve_health.last_query_id == 0 {
                return Err(TryRuntimeError::Other(
                    "ReserveProbeArmed requires an opened v1 probe id",
                ));
            }
            oracle
                .try_state()
                .map_err(|_| TryRuntimeError::Other("oracle core try_state failed (07 §13; I-18)"))
        }

        pub(crate) fn map_core_error(err: CoreError) -> DispatchError {
            match err {
                CoreError::BadOrigin => DispatchError::BadOrigin,
                CoreError::AlreadyRegistered => Error::<T>::AlreadyRegistered.into(),
                CoreError::NotRegistered => Error::<T>::NotRegistered.into(),
                CoreError::TooManyReporters => Error::<T>::TooManyReporters.into(),
                CoreError::TooManyWatchtowers => Error::<T>::TooManyWatchtowers.into(),
                CoreError::WindowClosed => Error::<T>::WindowClosed.into(),
                CoreError::WindowOpen => Error::<T>::WindowOpen.into(),
                CoreError::BondBelowMinimum => Error::<T>::BondBelowMinimum.into(),
                CoreError::SpecVersionMismatch => Error::<T>::SpecVersionMismatch.into(),
                CoreError::AlreadyFinal => Error::<T>::AlreadyFinal.into(),
                CoreError::AlreadyChallenged => Error::<T>::AlreadyChallenged.into(),
                CoreError::QuorumPending => Error::<T>::QuorumPending.into(),
                CoreError::RoundNotFound => Error::<T>::RoundNotFound.into(),
                CoreError::RoundLimit => Error::<T>::RoundLimit.into(),
                CoreError::DuplicateAck => Error::<T>::DuplicateAck.into(),
                CoreError::ReserveUnhealthy => Error::<T>::ReserveUnhealthy.into(),
                CoreError::ProbeTooEarly => Error::<T>::ProbeTooEarly.into(),
                CoreError::UnknownQuery => Error::<T>::UnknownQuery.into(),
                CoreError::Overflow => Error::<T>::Overflow.into(),
                CoreError::NotRecomputable => Error::<T>::NotRecomputable.into(),
                CoreError::ProofTooLarge => Error::<T>::ProofTooLarge.into(),
                CoreError::EvidenceMismatch => Error::<T>::EvidenceMismatch.into(),
                CoreError::BadProof => Error::<T>::BadProof.into(),
                CoreError::ValueOutOfBounds => Error::<T>::ValueOutOfBounds.into(),
            }
        }
    }
}

# 02 — Integration Contract (Chain ↔ Frontend, FROZEN)

**Status: normative component specification. Supersedes the corresponding sections of BACKEND_PLAN.md/FRONTEND_PLAN.md** (FE §30 in full — all patch items P-1…P-11 are ACCEPTED and applied here; FE §13; BE §7 SCALE surface, BE §25/§26 contract rows).

**Boundary.** This document owns everything the chain and the canonical frontend must agree on byte-for-byte: shared SCALE types, the `FutarchyApi` runtime API and its view types, the frozen event schema, the storage items and names the frontend reads directly, chain identity constants, the constants-binding rules, the WSS bootnode chain-spec requirement, the backend-published test-artifact feed, and the `ReleaseChannel` raw storage key. It does **not** own the *semantics* behind these surfaces (ledger rules → [03](03-conditional-ledger.md), market mechanics → [04](04-markets-and-pricing.md), decision engine → [05](05-welfare-and-decision-engine.md), oracle game → [07](07-oracle-and-disputes.md), upgrade path → [09](09-execution-upgrades-and-rollout.md)) — but where a name or layout appears both here and there, **this document's spelling is canonical**. Normative language per RFC 2119.

**Ownership and freeze (D-2, resolves F-4).** This contract is **jointly owned by the backend and frontend teams**. It is frozen at **contract version 2**. Any change — additive or otherwise — REQUIRES sign-off from both teams and a version bump (§13). The contingency for contract breach is the D-6 layer-1 fallback (chain-served ring + TWAP checkpoints), never a third-party service.

---

## 1. Contract surface at a glance

| # | Surface | Section | Origin |
|---|---|---|---|
| 1 | Shared SCALE primitives (`futarchy-primitives`) | §2 | BE §7, repaired per D-1/D-3/B-2/B-3/B-med |
| 2 | `FutarchyApi` — 11 runtime-API methods + view types | §3–§4 | FE §30 P-5/P-7, completed |
| 3 | Frozen event schema (ledger, market, epoch, oracle, guardian, execution, system) | §5–§6 | FE §15.3 + X-11 fixes |
| 4 | Storage items the frontend reads (incl. `RecentCohortSummaries`, `BaselineMarketOf`, `PhaseFlags`, oracle items) | §7 | X-1c, X-10, X-11b/c |
| 5 | Chain identity constants | §8 | D-17 |
| 6 | Constants & parameter binding (no FE hardcodes) | §9 | X-11e/h |
| 7 | WSS bootnode chain-spec requirement | §10 | D-6, X-4 |
| 8 | Backend-published test artifacts per release | §11 | X-15 |
| 9 | `ReleaseChannel` fixed-layout raw storage key | §12 | D-14 |
| 10 | Change control | §13 | D-2 |

Disposition of FE §30: P-1…P-4, P-6, P-8…P-11 amend the topology, repository, invariant, testing and rollout content now owned by [01](01-system-overview.md), [09](09-execution-upgrades-and-rollout.md), [12](12-release-and-operations.md) and [15](15-invariants-and-testing.md) and are applied there; P-5 and P-7 are the runtime surface and are applied **and completed** here (the P-5 gaps: the `pallet-epoch` storage-list addition for `RecentCohortSummaries`, and the canonical type name `DecisionOutcome` replacing the FE draft's `DecisionOutcomeCode`).

---

## 2. Shared SCALE primitives (`futarchy-primitives`)

All types live in the `no_std` crate `futarchy-primitives`, SCALE-encoded, versioned via a `#[codec(index)]`-stable discipline: **enum variants and struct fields are append-only after genesis**; removals require a new type + storage migration + contract version bump. All collections `BoundedVec`/`BoundedBTreeMap`. Numeric conventions: balances `u128` (USDC 6 decimals, VIT 12 decimals); prices/scores `FixedU64` semantics (1e9 scale) at every API and event boundary; internal LMSR math in 64.64.

The pre-genesis repairs below (relative to the superseded BE §7) are FINAL as of contract v1; `ProposalClass::Emergency` is deleted *before* genesis, so append-only discipline is not violated.

```rust
pub type ProposalId = u64;                  // monotone, never reused
pub type EpochId    = u32;                  // == EpochIndex
pub type CohortId   = EpochId;              // cohort ≡ its origin epoch
pub type MarketId   = u64;                  // monotone
pub type MetricId   = u16;
pub type MetricSpecVersion = u16;
pub type ResourceId = [u8; 8];
pub type ParamKey   = [u8; 16];

pub enum Branch { Accept, Reject }
pub enum ScalarSide { Long, Short }
pub enum GateType { Survival, Security }

/// Enlarged per B-2 (gate instruments) — semantics in 03.
pub enum PositionKind {
    BranchUsdc,
    Long,
    Short,
    GateYes(GateType),
    GateNo(GateType),
}

/// Enlarged per B-3 (Baseline ledger home) — semantics in 03.
pub enum PositionId {
    Proposal { proposal: ProposalId, branch: Branch, kind: PositionKind },
    Baseline { epoch: EpochId, side: ScalarSide },
}

/// `Voided` added per D-1; `ScalarSettled` carries the winning branch (B-low)
/// and the settlement score `s` that redemption needs ([03](03-conditional-ledger.md) §2.3).
pub enum VaultState { Open, Resolved(Branch), ScalarSettled { winner: Branch, s: FixedU64 }, Voided }

/// Five classes — `Emergency` deleted (D-7).
pub enum ProposalClass { Param, Treasury, Code, Meta, Constitutional }

pub enum ProposalState {
    Submitted, Screening, Qualified, Trading, Extended,
    Queued, Suspended, Rerun,
    Rejected(RejectReason), Executed, FailedExecuted,
    Measuring, Settled, Cancelled, Expired,
}

/// Three variants appended per B-med (producers wired in 05/06/09).
pub enum RejectReason {
    NotDecisionGrade, GateVetoSurvival, GateVetoSecurity, HurdleNotMet,
    ConvergenceFailed, SecondExtensionFailed, ProcessHold, ConstitutionViolation,
    ResourceConflict, RateLimited, VetoUpheldByReview, StaleQueue, PayloadReverted,
    NotRatified,            // D-5: values ratification absent/failed at execute
    SecuritySizing,         // D-4: InCapPrize > AttackCost̂ / 3
    AttestationMissing,     // D-18: bonded kernel attestation quorum absent
}

pub enum EpochPhase { Intake, Qualify, Seed, Trade, Decide, Review, Execute, Housekeeping }

/// CANONICAL decision-outcome type (X-11g). The FE draft name
/// `DecisionOutcomeCode` is RENAMED to this; there is no other outcome type.
pub enum DecisionOutcome { Adopt, Reject(RejectReason), Extend }

/// Dispatch result recorded per execution (X-11g; previously referenced, never defined).
pub enum DispatchOutcomeCode {
    Ok,
    /// index of the failing call within the batch + SCALE-truncated DispatchError.
    Failed { call_index: u8, error: [u8; 4] },
}

/// Ratification state surfaced on proposal views (D-5, D-11).
pub enum RatificationStatus {
    NotRequired,
    Pending { referendum: u32 },
    Passed { referendum: u32 },
    Failed { referendum: u32 },
}

/// Trade direction for quotes and `Traded` events.
pub enum TradeSide { BuyLong, BuyShort, SellLong, SellShort }
```

`Proposal` gains the fields the decision engine reads (`ask: Balance`, `decide_at: BlockNumber` — B-med, semantics in [05](05-welfare-and-decision-engine.md)); `ExecutionRecord.result` is typed `DispatchOutcomeCode` as above. The full `Proposal`/`ExecutionRecord` structs and their ≤ 512 B / bound arguments are owned by [05](05-welfare-and-decision-engine.md)/[09](09-execution-upgrades-and-rollout.md); their SCALE layouts are part of this contract by inclusion in `futarchy-primitives`.

The crate re-exports `INTEGRATION_CONTRACT_VERSION: u32 = 2`, exposed as a `pallet-constitution` runtime constant (metadata-readable, §9).

---

## 3. `FutarchyApi` runtime API (11 methods, normative)

Declared in the `runtime-api/` crate; **the runtime MUST implement all 11 methods**. All view types are plain SCALE structs in `futarchy-primitives` (§4) under the append-only discipline, so the TypeScript side decodes them with generated descriptors. All methods are read-only, executed by callers via `chainHead_call` through the light client — no dispatch weight; implementations MUST be O(bounded-collection) with the bounds shown (every backing map is bounded).

```rust
sp_api::decl_runtime_apis! {
    pub trait FutarchyApi {
        /// Epoch clock: index, phase, boundaries, dead-man, freeze and phase flags.
        fn epoch_status() -> EpochStatusView;                                        // ≤ 128 B
        /// All live proposals with market ids, states, decide_at, maturity, ratification.
        fn proposal_summaries() -> BoundedVec<ProposalSummaryView, ConstU32<32>>;    // ≤ 32 × 256 B
        /// Exact quote incl. fee for a hypothetical trade at current book state (USDC-denominated, D-3 wrapper semantics).
        fn quote(market: MarketId, side: TradeSide, amount: Balance) -> QuoteView;   // ≤ 96 B
        /// Decision statistics exactly as decide() would read them now (incl. D-4 sizing).
        fn decision_stats(pid: ProposalId) -> Option<DecisionStatsView>;             // ≤ 512 B
        /// All positions of an account across proposal, gate and Baseline instruments.
        fn account_positions(who: AccountId) -> BoundedVec<PositionView, ConstU32<64>>;
        /// Execution queue incl. maturity/grace/version/ratification state.
        fn execution_queue() -> BoundedVec<QueuedExecutionView, ConstU32<32>>;
        /// Current welfare pillars, gates, breach + reserve flags, active MetricSpec.
        fn welfare_current() -> WelfareView;                                         // ≤ 1 KiB
        /// Typed constitution params (value + bounds + governance metadata) for ≤ 64 keys.
        fn params(keys: BoundedVec<ParamKey, ConstU32<64>>) -> BoundedVec<ParamView, ConstU32<64>>;
        /// Treasury NAV components (matches the treasury definition in 08), incl. haircut flag.
        fn nav() -> NavView;                                                         // ≤ 256 B
        /// Ring of the last 32 cohort settlements (mirrors RecentCohortSummaries, §7.1).
        fn recent_cohorts() -> BoundedVec<CohortSummaryView, ConstU32<32>>;
        /// Oracle rounds currently open.
        fn open_oracle_rounds() -> BoundedVec<OracleRoundView, ConstU32<192>>;       // 16 × 4 × 3
    }
}
```

**Verification posture.** Runtime calls execute as `chainHead`-scoped calls: smoldot runs the runtime locally against proof-backed storage for the chosen finalized block **[VERIFY smoldot/PAPI route typed runtime-calls through `chainHead_call` pinned to a finalized hash — FE-P2; until verified, every `FutarchyApi` result used on the transaction path MUST be cross-checked against direct storage reads]**. Runtime APIs are an optimization, never a trust root: every value is also recomputable client-side from the storage items of §7.

---

## 4. View types (normative, `futarchy-primitives`)

```rust
pub struct EpochStatusView {
    pub index: EpochId,
    pub phase: EpochPhase,
    pub phase_start_block: BlockNumber,
    pub next_boundary: BlockNumber,
    pub dead_man_armed: bool,          // dead-man switch engaged (09)
    pub ledger_frozen: bool,           // PB-LEDGER-FREEZE active (D-9)
    pub phase_flags: u32,              // verbatim copy of pallet-constitution::PhaseFlags (§7.3)
}

pub struct ProposalSummaryView {
    pub id: ProposalId,
    pub class: ProposalClass,
    pub state: ProposalState,
    pub proposer: AccountId,
    pub epoch: EpochId,
    pub payload_hash: H256,
    pub ask: Balance,                                   // 0 for non-treasury asks
    pub decision_market: Option<(MarketId, MarketId)>,  // (accept, reject)
    pub gate_markets: Option<[MarketId; 4]>,            // (S,C) × (adopt, reject)
    pub decide_at: BlockNumber,
    pub maturity: Option<BlockNumber>,                  // set once Queued
    pub ratification: RatificationStatus,               // D-5/D-11
}

pub struct QuoteView {
    pub cost: Balance,            // USDC the wrapper charges (buy) / pays (sell), excl. fee
    pub fee: Balance,             // USDC fee at the current mkt.fee
    pub p_after_1e9: FixedU64,    // post-trade instantaneous price
    pub max_trade: Balance,       // current per-trade max for this book
    pub within_domain: bool,      // |q_L − q_S|/b ≤ 48 after the trade
}

pub struct DecisionStatsView {
    pub pid: ProposalId,
    pub twap_accept_1e9: FixedU64,
    pub twap_reject_1e9: FixedU64,
    pub twap_baseline_1e9: FixedU64,
    pub r_eff_1e9: FixedU64,                    // max(reject, baseline − σ)
    pub trailing_accept_1e9: FixedU64,
    pub trailing_reject_1e9: FixedU64,
    pub coverage_pct: u8,
    pub traded_volume: Balance,
    pub v_min_required: Balance,                // Ask-scaled (D-4 secondary)
    pub converged: bool,
    pub gate_twaps_1e9: Option<[FixedU64; 4]>,  // (S,C) × (adopt, reject)
    pub attack_cost_hat: Balance,               // D-4 primary: measured-depth estimate
    pub in_cap_prize: Balance,                  // must satisfy ≤ attack_cost_hat / 3
}

pub struct PositionView {
    pub position: PositionId,       // proposal, gate or Baseline instrument
    pub balance: Balance,
    pub vault_state: VaultState,    // of the owning vault (Baseline: its epoch vault)
}

pub struct QueuedExecutionView {
    pub pid: ProposalId,
    pub class: ProposalClass,
    pub payload_hash: H256,
    pub maturity: BlockNumber,
    pub grace_end: BlockNumber,
    pub version_constraint: RuntimeVersionConstraint,
    pub cancelled: bool,
    pub ratification: RatificationStatus,
    pub meters_clear: bool,         // rate meters would admit execution now
}

pub struct WelfareView {
    pub epoch: EpochId,
    pub spec_version: MetricSpecVersion,
    pub s_pillar_1e9: FixedU64,
    pub c_onchain_1e9: FixedU64,    // deterministic sub-pillar (D-18)
    pub c_attested_1e9: FixedU64,   // attested sub-pillar (settlement-time only)
    pub p_pillar_1e9: FixedU64,
    pub a_pillar_1e9: FixedU64,
    pub gate_s_1e9: FixedU64,
    pub gate_c_1e9: FixedU64,
    pub w_current_1e9: FixedU64,
    pub s_breached: bool,
    pub c_breached: bool,
    pub reserve_flag: bool,         // reserve-health trigger R (B-med USDC-freeze)
}

pub struct ParamView {
    pub key: ParamKey,
    pub value: u128,                // SCALE-encoded per-key scalar, unit per 13
    pub min: u128,
    pub max: u128,
    pub max_delta: u128,
    pub cooldown_blocks: u32,
    pub last_change: BlockNumber,
    pub class: ProposalClass,
}

pub struct NavView {
    pub total: Balance,             // NAV per the definition in 08
    pub main: Balance,
    pub pol: Balance,
    pub insurance: Balance,
    pub keeper: Balance,
    pub oracle: Balance,
    pub rewards: Balance,
    pub stream_remainders: Balance, // undisbursed outbound streams
    pub obligations: Balance,
    pub haircut_flag: bool,         // NAV haircut surfaced when reserve_flag set
}

/// Stored form == view form (§7.1). FE draft's `DecisionOutcomeCode` is renamed.
pub struct CohortSummary {
    pub epoch: EpochId,
    pub s_1e9: FixedU64,
    pub baseline_twap_1e9: FixedU64,
    pub proposals: BoundedVec<(ProposalId, ProposalClass, DecisionOutcome), ConstU32<5>>,
    pub voided: bool,
    pub settled_at: BlockNumber,
}
pub type CohortSummaryView = CohortSummary;

pub struct OracleRoundView {
    pub component: MetricId,
    pub epoch: EpochId,
    pub round: u8,                        // 1..=3
    pub reporter: AccountId,
    pub value_1e9: FixedU64,
    pub evidence_hash: H256,
    pub bond: Balance,                    // value-scaled per D-18
    pub challenge_deadline: BlockNumber,
    pub acked_by_watchtowers: u8,         // quorum progress (D-18)
    pub escalated: bool,
}
```

---

## 5. pallet-market events (X-1b)

`pallet-market`'s call table gains an explicit **Events** column; the price-history pillar of the frontend is fed exclusively by these events. Emission points are normative.

| Event | Fields | Emitted when |
|---|---|---|
| `Traded` | `{ market: MarketId, who: AccountId, side: TradeSide, amount: Balance, cost: Balance, p_after: FixedU64 }` | Every successful `buy`/`sell` fill (wrapper semantics D-3); `side` is the 4-variant `TradeSide` (§2) and `amount`/`cost` are **unsigned magnitudes** — direction is carried entirely by `side`; `cost` is USDC incl. maker payment, excl. fee; **`p_after` = the post-trade instantaneous `p_L`** (1e9; `p_S = 1 − p_L` is derived; gate books map YES ↦ LONG) |
| `Observed` | `{ market: MarketId, o_t: FixedU64 }` | Every accepted TWAP observation (on-trade and cranked) on the 10-block observation grid *(normative interval: [13](13-parameters.md))* |
| `MarketCreated` | `{ market: MarketId, kind: MarketKind, pid: Option<ProposalId>, epoch: EpochId, b: Balance }` | Book deployment at Seed (`MarketKind ∈ { DecisionAccept, DecisionReject, GateS_Adopt, GateS_Reject, GateC_Adopt, GateC_Reject, Baseline }`) |
| `MarketClosed` | `{ market: MarketId }` | Book frozen at decision close / branch resolution (books do NOT reopen — D-8) |
| `MarketReaped` | `{ market: MarketId }` | Post-settlement cleanup |

The minimal FE ingest set is `Traded` + `Observed`; the lifecycle events bound chart ranges without storage diffing.

---

## 6. Frozen event schema (all pallets the frontend ingests)

Canonical names below are FINAL; the frontend `CRITICAL_SURFACE` list and local-index ingest filter MUST use exactly these. **X-11d fix:** the FE draft's four misnamed epoch events are corrected — `Withdrawn` → **`ProposalWithdrawn`**, `Cancelled` → **`ProposalCancelled`**, `Qualified` → **`ProposalQualified`**, `Deferred` → **`ProposalDeferred`**.

| Pallet | Events (canonical) |
|---|---|
| `pallet-conditional-ledger` | `Split`, `Merged`, `ScalarSplit`, `ScalarMerged`, `PositionTransferred`, `VaultResolved { pid, branch }`, `Redeemed`, `ScalarSettlementSet { pid, branch, s }` (carries winning branch — B-low), `ScalarRedeemed`, `ScalarPairRedeemed { pid, amount }` (B-5), `GateSettled { pid, branch, gate, outcome }` (B-2), **`VaultVoided { pid }`** (D-1, X-11f), **`VoidRedeemed { pid, kind, amount, payout }`** (D-1), `VaultReaped` |
| `pallet-market` | §5 table |
| `pallet-epoch` | `ProposalSubmitted`, `ProposalWithdrawn`, `ScreeningStarted`, `ProposalCancelled { reason }`, `ProposalQualified`, `ProposalDeferred`, `MarketsOpened`, `DecisionExtended`, `ProposalQueued { payload_hash, maturity }`, `ProposalRejected { reason }`, `ProposalDelayed { justification_hash }`, `RerunScheduled`, `RerunOpened`, `MandateExpired`, `MeasurementStarted { cohort }`, `CohortSettled { epoch, s }`, **`ProposalForceRejected { pid, reason }`** — emitted by transition T20 (emergency/VOID force-reject), which previously emitted nothing and silently corrupted every event-derived archive (X-11f), `IntakeSlashed { pid, reason, amount }` (accompanies every partial intake-bond slash — [06](06-governance-and-guardians.md) §4) |
| `pallet-execution-guard` | `Executed { pid, record }`, `ExecutionFailed { pid, outcome: DispatchOutcomeCode }`, `Ratified { pid, referendum_index }` (written by `execution_guard.ratify(proposal_id, referendum_index)`, the sole `ratify`-track governance call — [06](06-governance-and-guardians.md) §2.2), `UpgradeAuthorized { code_hash, authorized_at }` (system-event mirror carrying `authorized_at` for the `DescriptorLeadTime` check, D-14) |
| `pallet-oracle` | §7.2 table |
| `pallet-registry` | `IncidentFiled`, `MilestoneFiled`, `IncidentChallenged`, `MilestoneChallenged`, `IncidentUpheld`, `IncidentRejected`, `MilestoneAccepted`, `MilestoneRejected`, `FilingBondSlashed`, `RegistryEpochClosed` (field detail in [07](07-oracle-and-disputes.md); names frozen here) |
| `pallet-guardian` | `GuardianAction { action_id, power, target, justification_hash }`, `ForceRerun { pid, justification_hash, window_end }`, `PlaybookActivated { id, trigger, expiry }`, `PlaybookRenewed { id }`, `PlaybookExpired { id }`, `ReviewScheduled { action, referendum }` |
| `frame-system` / upgrade path | `CodeUpdated`, `UpgradeAuthorized` (native), ingested for descriptor switching |

Every terminal proposal/vault/cohort state transition MUST emit exactly one event (the T20 fix closes the last silent transition), so event-derived history is complete by construction — the load-bearing property behind the D-2/D-6 history model.

---

## 7. Storage items the frontend reads directly

Key-hasher choices follow the source pallets (maps `Blake2_128Concat` unless stated). This section freezes **names, key types and value types**; bound arguments live with the owning pallets.

### 7.1 `pallet-epoch` (X-1c)

| Item | Type | Notes |
|---|---|---|
| `Proposals` | `map ProposalId → Proposal` | ≤ `MaxLiveProposals = 32` *(normative value: [13](13-parameters.md))* |
| `EpochOf` | `EpochInfo { index, phase, phase_start_block }` | — |
| `IntakeQueue` | `BoundedVec<ProposalId, 64>` | Pre-qualification scope only (D-10) |
| **`RecentCohortSummaries`** | **`BoundedVec<CohortSummary, ConstU32<32>>`** | **Ring of the last 32 cohorts** (≈ 5.8 KiB), pushed at `settle_cohort` completion, FIFO-evicted; one push per ~21 days amortized into the existing settle crank — negligible weight, no new hook. This is the P-5 storage-list addition the FE draft omitted from §5.2.3, applied: a fresh browser renders ~22 months of settlement history with zero infrastructure dependency (D-6 layer 1) |
| `Cohorts` | `map EpochId → CohortInfo` | ≤ 4 non-terminal |

### 7.2 `pallet-oracle` (X-11c — canonical names; [07](07-oracle-and-disputes.md) uses these)

Storage:

| Item | Type | Bound |
|---|---|---|
| `Reporters` | `map AccountId → ReporterInfo { stake: Balance, registered_at: BlockNumber, offenses: u8 }` | counted; ≥ 3 required before attested components admit |
| `Rounds` | `map (MetricId, EpochId) → RoundState { round: u8, spec_version: MetricSpecVersion, reporter: AccountId, value: FixedU64, evidence_hash: H256, bond: Balance, challenge_deadline: BlockNumber, extended: bool, challenger: Option<AccountId>, counter_value: Option<FixedU64>, acks: u8 }` | ≤ 16 components × ≤ 4 settling epochs (per-version games across an activation boundary append a `RoundState` per frozen version — [07](07-oracle-and-disputes.md) §2) |
| `ComponentValues` | `map (MetricId, EpochId) → SettledComponent { value: FixedU64, path: SettlePath, flagged: bool }` | reaped at cohort settlement; `SettlePath ∈ { Unchallenged, Recomputed, Adjudicated, Neutral }` |
| `Watchtowers` | `map AccountId → WatchtowerInfo { stake: Balance, registered_at: BlockNumber, inactive_epochs: u8 }` | counted, ≤ `wt.max = 16` seats; bonded acknowledgment quorum (D-18; registry semantics in [07](07-oracle-and-disputes.md) §4) |
| `ReserveHealth` | `{ consecutive_fails: u8, consecutive_passes: u8, unhealthy: bool, last_query_id }` | single value; the deterministic reserve-probe state (`R`, [07](07-oracle-and-disputes.md) §8) |

Events:

| Event | Fields |
|---|---|
| `ReporterRegistered` | `{ who, stake }` |
| `Reported` | `{ component, epoch, round, reporter, value, evidence_hash, bond }` |
| `Challenged` | `{ component, epoch, round, challenger, counter_value, evidence_hash, bond }` |
| `RoundEscalated` | `{ component, epoch, round, new_bond }` |
| `RecomputeProven` | `{ component, epoch, value, prover }` |
| `AdjudicationRequested` | `{ component, epoch, referendum }` |
| `Adjudicated` | `{ component, epoch, value }` |
| `ComponentSettled` | `{ component, epoch, value, path }` |
| `NeutralSettlement` | `{ component, epoch, carried_value, flagged_epochs }` |
| `WindowAcknowledged` | `{ component, epoch, round, watchtower }` |
| `WindowExtended` | `{ component, epoch, round, new_deadline }` |
| `QuorumFailed` | `{ component, epoch, round }` — no challenge and no watchtower quorum after the single extension ⇒ neutral path ([07](07-oracle-and-disputes.md) §4) |
| `ReporterSlashed` | `{ who, amount, offense }` / `ReporterEjected { who }` |
| `WatchtowerRegistered` | `{ who, stake }` |
| `WatchtowerInactive` | `{ who, epoch }` |
| `WatchtowerSlashed` | `{ who, amount }` |
| `ReserveProbeSent` | `{ query_id }` |
| `ReserveProbeResult` | `{ query_id, passed: bool }` |
| `ReserveUnhealthy` | `{ }` / `ReserveRecovered { }` — reserve-health state transitions (`R`, [07](07-oracle-and-disputes.md) §8) |

### 7.3 `pallet-constitution`

| Item | Type | Notes |
|---|---|---|
| `Params` | `map ParamKey → ParamRecord` | read via `params()`; §9 binding rules |
| **`PhaseFlags`** | `u32` bitset | **The key the frontend binds trading enablement (and the Phases 0–3 "bootstrap governance — sudo active" banner, D-13) to.** Bit assignments: 0 = shadow mode, 1 = PARAM armed, 2 = TREASURY armed, 3 = CODE/META armed, 4 = sudo present, 5 = ledger frozen (PB-LEDGER-FREEZE), 6 = dead-man engaged, 7 = reserve-health flag; bits 8–31 reserved (append-only) |
| **`ReleaseChannel`** | fixed-layout raw value | §12 — NOT ordinary SCALE-metadata-dependent storage |

### 7.4 `pallet-market` (X-10) and other reads

| Item | Type | Notes |
|---|---|---|
| **`BaselineMarketOf`** | **`map EpochId → MarketId`** (in **`pallet-market`** — the pallet home per [04 §8.3](04-markets-and-pricing.md)) | **X-10 fix**: the declared backing storage for `baseline_market(epoch)`. Written at Baseline book creation (Seed of each epoch); retained for all epochs still present in the `RecentCohortSummaries` ring plus live epochs; pruned in lockstep with ring eviction (≤ 36 entries) |
| `Markets` | `map MarketId → MarketState` | ≤ `MaxLiveMarkets = 196` *(normative value: [13](13-parameters.md))* |

`pallet-conditional-ledger::{Vaults, BaselineVaults, Positions, PositionTotals}` — note the **key order of `Positions` is `(PositionId, AccountId)`** (per-vault drainable, B-med); a per-account storage prefix scan is therefore NOT available, and the frontend MUST use `account_positions()` (the runtime API iterates the bounded live-vault set) or the per-account key index maintained by the ledger ([03](03-conditional-ledger.md)). `pallet-execution-guard::{Queue, Ratifications, ExecutionRecords}` (a `RatificationRecord` is written by the frozen governance call `execution_guard.ratify(proposal_id, referendum_index)`, binding `(pid, payload_hash)` — [06 §2.2](06-governance-and-guardians.md)); `pallet-welfare::{Snapshots, MetricSpecs, GateBreachFlags}`; `pallet-guardian` membership/allowances; `System.Account`, `ForeignAssets.Account(USDC_LOCATION, who)` (NOT `Assets.Account(1337, who)` — X-11a; the USDC identifier is the XCM Location of §8).

---

## 8. Chain identity constants (D-17; X-11a/b)

Pinned in the frontend's `ChainIdentity` at build time and asserted at boot. These values are frozen; changing any is a contract version bump.

| Constant | Value |
|---|---|
| ss58 prefix | **7777** (ss58-registry submission REQUIRED before Phase 2) |
| paraId | Assigned at onboarding; **all test fixtures use 4242** |
| USDC asset | `pallet-assets` instance **`ForeignAssets`**, keyed by XCM `Location { parents: 1, interior: X3(Parachain(1000), PalletInstance(50), GeneralIndex(1337)) }` **[VERIFY asset index 1337 at implementation]** |
| USDC decimals | 6 (preserved from Asset Hub); `min_balance = 10^4` (1 cent) |
| VIT decimals | 12 |
| VIT existential deposit | **0.01 VIT** (= 10^10 plancks) |
| Phase flag storage | `pallet-constitution::PhaseFlags` (§7.3) — the trading-enablement key |
| Contract version | `INTEGRATION_CONTRACT_VERSION = 2` (runtime constant) |

---

## 9. Constants and parameter binding (X-11e/h — no FE hardcodes)

Two representations exist, and the frontend MUST bind to them and never hardcode:

1. **Kernel constants (class K)** have *no storage representation*; they are exposed as **pallet constants in the runtime metadata** (the constants API) and are readable without any storage query. They change only via Wasm upgrade, which the frontend already tracks through descriptors.
2. **Tunables** live in `pallet-constitution::Params` and are read via `params()` (or the raw `Params` map); their kernel floors/ceilings are ALSO metadata constants.

Enumeration of every value the frontend's precondition tables re-check (defaults/bounds are quoted for readability; *normative values: [13](13-parameters.md)*):

| Value | Representation (FE binding target) | Used by FE precondition row |
|---|---|---|
| `MinSplit` kernel floor (0.01 USDC) + live `ledger.min_split` | metadata constant (floor) + `params()` (live) | `ledger.split/merge` |
| Per-trade min / max (`mkt.min_trade = 1`, `mkt.max_trade = b/4`) | metadata constants (K) | `market.buy/sell` |
| `MinTransfer` | metadata constant (K) | `ledger.transfer` |
| `MaxPositionsPerAccount = 64` (protocol accounts exempt) | metadata constant | `ledger.transfer` (recipient bound), position views |
| Positions entry deposit (0.1 USDC) | metadata constant | `ledger.split`, `transfer` fee headroom |
| `IntakeQueue = 64` bound; intake rate limit ≤ 4 entries/epoch/account | metadata constants | `epoch.submit` |
| `MaxLiveProposals = 32` | metadata constant | discovery bounds |
| `prop.bond` per class | `params()` | `epoch.submit` |
| `mkt.fee` (30 bps default) | `params()` | quote display, `buy/sell` cost recompute |
| `mkt.obs_interval` (10 blocks) | `params()` | crank staleness check |
| `dec.window`, `dec.trailing`, `dec.extension`, `dec.delta`, `dec.sigma`, `dec.coverage`, `dec.v_min` | `params()` (kernel floors as metadata constants) | decision previews, `decide` crank |
| `gate.p_max`, `gate.eps` | `params()` (0.10 K ceiling as constant) | gate-market screens |
| `exec.timelock` per class, `exec.grace` | `params()` (K floors as constants) | `execution_guard.execute` |
| `orc.bond_floor`/`orc.rounds`/`orc.window`, `orc.bond_bps` value scaling, `orc.reporter_stake` | `params()` | `oracle.report/challenge` |
| `trs.cap_proposal`/`cap_30d`/`cap_180d`, `trs.stream_threshold` | `params()` | treasury proposal screens |
| `fee.vit_usdc_rate` | `params()` | fee-currency selector (D-12) |
| `epoch.length`, `epoch.slots`, phase-offset fractions | `params()` + metadata constants | countdowns, phase headers |
| `DescriptorLeadTime = 43,200` blocks | metadata constant | upgrade banners, execute precondition |
| `RecentCohortSummaries` ring size = 32; books/proposal ≤ 6; `MaxLiveMarkets = 196` | metadata constants | history windows, chart bounds |

Catch-all rule: **any** [13](13-parameters.md) key a frontend workflow evaluates MUST be sourced from `params()`/metadata at a pinned finalized block. Shipping a numeric copy of any of these values in the frontend bundle is a release-gate failure (frontend CI asserts no literal matches against the constants list).

---

## 10. WSS bootnode chain-spec requirement (D-6, X-4)

The production (and Paseo) chain-spec artifacts in `deploy/` MUST list **≥ 8 browser-reachable WSS multiaddrs across ≥ 4 independent operators, with ≥ 2 endpoints on port 443** (corporate/mobile networks block non-443 WSS). These endpoints are the canonical frontend's guaranteed dial set — the fallback for the open **[VERIFY browser-WSS peer behavior under smoldot 3.x]**. Operators hold the protocol-funded 30-day served-state commitment ([12](12-release-and-operations.md)); the requirement is a rollout phase gate ([01 §7](01-system-overview.md), [09](09-execution-upgrades-and-rollout.md)). Chain-spec updates that would drop the set below any of the three thresholds MUST NOT be released.

---

## 11. Backend-published test artifacts per release (X-15)

The frontend's compatibility controls are release-gated on backend-published inputs. **Every tagged runtime release** MUST publish, as CI artifacts attached to the release in the `futarchy-chain` repository and mirrored content-addressed alongside the frontend release channel:

| Artifact | Contents | Consumed by |
|---|---|---|
| Runtime Wasm + metadata | Reproducibly-built `runtime.wasm`, SCALE metadata blob, metadata hash, spec_name/spec_version | Descriptor regeneration + drift CI (FE §12.1-equivalent, [12](12-release-and-operations.md)) |
| Chopsticks environment | Forked-state config + fixture state snapshots for: upgrade transition, StaleQueue, VOID epoch, manufactured precondition failures | Nightly Chopsticks suites |
| Zombienet environment | Relay+para topology files + genesis overrides matching the release | e2e suites driven through the FE data layer |
| chainHead fixtures | Deterministic JSON-RPC transcripts for every screen store's read set | Mock-runtime PR suites |

**Gating rule:** publishing these artifacts is backend WBS row **E15** (mirroring FE-R1); E15 is release-gating for the backend exactly as FE-12 is for the frontend. A runtime release without the full artifact set MUST NOT ship, and runtime changes that break `FutarchyApi` compatibility gates MUST NOT merge without a coordinated frontend release (FE §30 P-10, applied). WBS ownership detail: [15](15-invariants-and-testing.md).

---

## 12. `ReleaseChannel` fixed-layout raw storage key (D-14)

Purpose: pinned-release (stranded) frontends must learn "a newer canonical release exists" **without current metadata** — precisely when they are `ReadOnlyIncompatible`. This replaces the superseded `system.remark` release pointer, which stranded apps could not decode.

- **Raw key (frozen forever):** `twox128("Constitution") ++ twox128("ReleaseChannel")`.
- **Value layout (frozen forever — fixed-width, no length prefixes, readable with a raw `state_getStorage`/chainHead storage proof and no metadata):**

| Offset | Width | Field |
|---|---|---|
| 0 | 1 | `schema: u8` — always `1`; any other value ⇒ layout extended append-only, prefix still valid |
| 1 | 32 | `version: [u8; 32]` — UTF-8 semver, zero-padded (current canonical release) |
| 33 | 43 | `manifest_txid: [u8; 43]` — Arweave base64url TXID, zero-padded |
| 76 | 32 | `release_json_hash: [u8; 32]` — SHA-256 |
| 108 | 4 | `updated_at: u32` LE — block number of last update |
| 112 | 4 | `spec_version: u32` LE — current runtime spec_version |
| 116 | 4 | `pending_authorized_at: u32` LE — block of a pending `UpgradeAuthorized`; 0 if none (D-14 lead-time display, [09](09-execution-upgrades-and-rollout.md)) |
| 120 | 32 | `min_supported_version: [u8; 32]` — UTF-8 semver, zero-padded; oldest release that may sign without a blocking warning ([12 §3.2](12-release-and-operations.md)) |
| 152 | 4 | `keyring_generation: u32` LE — monotone keyring generation ([12 §2.1](12-release-and-operations.md)) |
| 156 | 8 | `revoked_key_bits: u64` LE — bitmask over key indices within the generation's published keyring ([12 §2.3](12-release-and-operations.md)) |
| 164 | 4 | `flags: u32` LE — bit 0 `SECURITY`, bit 1 `EXPEDITED`, bit 2 `URGENT_UPGRADE`; bits 3–31 reserved zero |

Total **168 bytes** (v1.0 baseline — the pre-freeze 78- and 92-byte drafts in earlier drafts of [09](09-execution-upgrades-and-rollout.md)/[12](12-release-and-operations.md) are superseded by this merged layout; no schema bump, this is the first frozen version). Writers, exhaustively: (a) the **execution guard** at `UpgradeAuthorized` (sets `spec_version` target, `pending_authorized_at`, `URGENT_UPGRADE`) and at applied-upgrade detection (clears the pending fields); (b) the **`ConstitutionalValues` origin** via `constitution.set_release_channel` on each canonical repoint, `min_supported_version` bump or key revocation ([12](12-release-and-operations.md)). No other origin can write it. The layout MUST NEVER change except by appending fields beyond offset 168 with a schema bump; readers parse by offset, never by SCALE metadata. A compromised writer can only cause a false "update available" banner pointing at a TXID users independently verify, or warning/signing friction in old releases ([14](14-threat-model.md)).

---

## 13. Change control (D-2; resolves F-4, X-11h)

1. **Joint ownership.** This document is owned by the backend and frontend teams jointly. Every change REQUIRES explicit sign-off from a named owner on each side, recorded in the document history.
2. **Versioning.** `INTEGRATION_CONTRACT_VERSION` (§8) is stamped in `futarchy-primitives`, exposed via the constants API, and echoed in the frontend `release.json`. Any change to §2–§12 bumps it. The `FutarchyApi` itself carries an `sp_api` version; additive methods bump the API version *and* the contract version.
3. **Append-only.** SCALE types, event fields and view types may only gain trailing fields/variants post-genesis. Renames and removals require a new type, a migration, and a major contract bump with a coordinated FE release inside the `DescriptorLeadTime` window (D-14).
4. **No hardcodes (X-11h).** The frontend binds to the constants API and `params()` for every chain-tunable value (§9); frontend CI enforces the no-literal rule. The 64-position bound and every other formerly hardcoded §21-tunable are chain-read.
5. **Release coupling.** Backend E15 and frontend FE-R1 are the two ends of this contract; neither side's release gates pass while the other's contract surface is red (§11).
6. **Contingency.** If a contract regression ships anyway, the frontend degrades to the D-6 layer-1 surface (chain-served `RecentCohortSummaries` + 8-checkpoint TWAP series + direct storage reads) — reduced depth, full correctness; it never falls back to a trusted third-party service.

---

## Resolves

| Finding | Resolution in this document |
|---|---|
| X-1a | §3–§4: the complete 11-method `FutarchyApi` with every view type fully defined in `futarchy-primitives`; light-client-callable; P-5/P-7 applied and completed |
| X-1b | §5: `Traded { market, side, amount, cost, p_after }` and `Observed { market, o_t }` with an explicit Events table for `pallet-market` |
| X-1c | §7.1: `RecentCohortSummaries` ring (last **32** cohorts) added to `pallet-epoch` storage — the §5.2.3 storage-list edit P-5 missed — with push point, eviction and weight argument |
| X-10 | §7.4: `BaselineMarketOf: map EpochId → MarketId` declared (in `pallet-market`, per [04 §8.3](04-markets-and-pricing.md)) as the backing storage for Baseline-market discovery, with write point and pruning rule |
| X-11a | §7.4/§8: USDC is `ForeignAssets` keyed by the pinned XCM Location; `ChainIdentity` pins the USDC identifier; `Assets.Account(1337, …)` reads are wrong by contract |
| X-11b | §8: ss58 7777, paraId (fixtures 4242), VIT ED 0.01, `PhaseFlags` storage location all specified |
| X-11c | §7.2: oracle pallet storage-item names and full event set defined canonically; [07](07-oracle-and-disputes.md) uses these names |
| X-11d | §6: four FE §15.3 epoch event names corrected to `ProposalWithdrawn`/`ProposalCancelled`/`ProposalQualified`/`ProposalDeferred`; full canonical set frozen |
| X-11f | §6: T20 now emits `ProposalForceRejected { pid, reason }`; the ledger emits `VaultVoided`/`VoidRedeemed` — no silent terminal transitions remain |
| X-11g | §2: canonical `DecisionOutcome` enum defined (FE's `DecisionOutcomeCode` renamed away); `DispatchOutcomeCode` defined for `ExecutionRecord` |
| X-11h | §9/§13: every FE-re-checked constant enumerated with its chain-side representation; FE binds to constants API/`params()`; no-hardcode rule CI-enforced |
| X-15 | §11: per-release runtime wasm + metadata, Chopsticks/Zombienet environments and chainHead fixtures published as release-gating backend artifacts (WBS E15 mirrors FE-R1) |
| F-4 | Header + §13: contract frozen, jointly owned, version-stamped, change-controlled; contingency is the D-6 layer-1 fallback, not a third-party dependency |

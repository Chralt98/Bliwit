# 02 — Integration Contract (Chain ↔ Frontend, FROZEN)

**Status: normative component specification. Supersedes the corresponding sections of BACKEND_PLAN.md/FRONTEND_PLAN.md** (FE §30 in full — all patch items P-1…P-11 are ACCEPTED and applied here; FE §13; BE §7 SCALE surface, BE §25/§26 contract rows).

**Boundary.** This document owns everything the chain and the canonical frontend must agree on byte-for-byte: shared SCALE types, the `FutarchyApi` runtime API and its view types, the frozen event schema, the storage items and names the frontend reads directly, chain identity constants, the constants-binding rules, the WSS bootnode chain-spec requirement, the backend-published test-artifact feed, and the `ReleaseChannel` raw storage key. It does **not** own the *semantics* behind these surfaces (ledger rules → [03](03-conditional-ledger.md), market mechanics → [04](04-markets-and-pricing.md), decision engine → [05](05-welfare-and-decision-engine.md), oracle game → [07](07-oracle-and-disputes.md), upgrade path → [09](09-execution-upgrades-and-rollout.md)) — but where a name or layout appears both here and there, **this document's spelling is canonical**. Normative language per RFC 2119.

**Ownership and freeze (D-2, resolves F-4).** This contract is **jointly owned by the backend and frontend teams**. It is frozen at **contract version 11**. Any change — additive or otherwise — REQUIRES sign-off from both teams and a version bump (§13). The contingency for contract breach is the D-6 layer-1 fallback (chain-served ring + TWAP checkpoints), never a third-party service.

---

## 1. Contract surface at a glance

| # | Surface | Section | Origin |
|---|---|---|---|
| 1 | Shared SCALE primitives (`futarchy-primitives`) | §2 | BE §7, repaired per D-1/D-3/B-2/B-3/B-med |
| 2 | `FutarchyApi` — 11 runtime-API methods + view types | §3–§4 | FE §30 P-5/P-7, completed |
| 3 | Frozen event schema (ledger, market, epoch, oracle, registry, guardian, attestor, execution, system) | §5–§6 | FE §15.3 + X-11 fixes |
| 4 | Storage items the frontend reads (incl. `RecentCohortSummaries`, `BaselineMarketOf`, `PhaseFlags`, oracle and attestor items) | §7 | X-1c, X-10, X-11b/c |
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

/// `Voided` added per D-1; proposal vaults use `ScalarSettled`, which carries
/// the winning branch (B-low) and settlement score `s` that redemption needs.
/// Baseline epoch vaults have no branch and use `BaselineSettled`.
pub enum VaultState {
    Open,
    Resolved(Branch),
    ScalarSettled { winner: Branch, s: FixedU64 },
    Voided,
    BaselineSettled { s: FixedU64 },
}

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
    RolloverExhausted,      // 05 §2.1 T26: second deferral cancels with full refund (v6)
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
    NoPassedRecord,
    Passed { referendum: u32 },
}

/// Trade direction for quotes and `Traded` events.
pub enum TradeSide { BuyLong, BuyShort, SellLong, SellShort }
```

The `MetricId` assignment registry is owned by [05](05-welfare-and-decision-engine.md) §4.3 and mirrored in [13](13-parameters.md) §3.4; this contract freezes the `u16` type and its keyed surfaces without creating a third assignment table.

`Proposal` gains the fields the decision engine reads (`ask: Balance`, `decide_at: BlockNumber` — B-med, semantics in [05](05-welfare-and-decision-engine.md)); `ExecutionRecord.result` is typed `DispatchOutcomeCode` as above. The full `Proposal`/`ExecutionRecord` structs and their ≤ 512 B / bound arguments are owned by [05](05-welfare-and-decision-engine.md)/[09](09-execution-upgrades-and-rollout.md); their SCALE layouts are part of this contract by inclusion in `futarchy-primitives`.

Proposal positions MUST project a settled proposal vault as `ScalarSettled { winner, s }`; Baseline positions MUST project a settled epoch vault as `BaselineSettled { s }` and MUST NOT fabricate a proposal branch. `RatificationStatus::NoPassedRecord` means only that the execution guard has no passing ratification record. It is deliberately agnostic between no referendum, an in-flight referendum and a failed referendum; the frontend MUST derive that lifecycle from `pallet-referenda` ([06](06-governance-and-guardians.md) §2.2). `Pending` and `Failed` are removed because the guard cannot truthfully produce them in the deployed design. This `RatificationStatus` restructure is a pre-genesis contract-v6 repair; no deployed SCALE value requires migration.

The crate re-exports `INTEGRATION_CONTRACT_VERSION: u32 = 11`, exposed as a `pallet-constitution` runtime constant (metadata-readable, §9).

---

## 3. `FutarchyApi` runtime API (11 methods, normative)

Declared in the `runtime-api/` crate; **the runtime MUST implement all 11 methods**. All view types are plain SCALE structs in `futarchy-primitives` (§4) under the append-only discipline, so the TypeScript side decodes them with generated descriptors. API collections use the shipped const-generic `futarchy_primitives::BoundedVec<T, N>` composite wrapper (a `Vec<T>` field under that exact type path), not FRAME's `BoundedVec<T, ConstU32<N>>`; the generated metadata path and composite form are part of the freeze. All methods are read-only, executed by callers via `chainHead_call` through the light client — no dispatch weight; implementations MUST be O(bounded-collection) with the bounds shown (every backing map is bounded).

```rust
sp_api::decl_runtime_apis! {
    pub trait FutarchyApi {
        /// Epoch clock: index, phase, boundaries, dead-man, freeze and phase flags.
        fn epoch_status() -> EpochStatusView;                                        // ≤ 128 B
        /// All live proposals with market ids, states, decide_at, maturity, ratification.
        fn proposal_summaries() -> futarchy_primitives::BoundedVec<ProposalSummaryView, 32>; // ≤ 32 × 256 B
        /// Exact quote incl. fee for a hypothetical trade at current book state (USDC-denominated, D-3 wrapper semantics).
        fn quote(market: MarketId, side: TradeSide, amount: Balance) -> QuoteView;   // ≤ 96 B
        /// Finalized decision statistics from sealed registered windows (incl. D-4 sizing).
        fn decision_stats(pid: ProposalId) -> Option<DecisionStatsView>;             // ≤ 512 B
        /// All positions of an account across proposal, gate and Baseline instruments.
        fn account_positions(who: AccountId) -> futarchy_primitives::BoundedVec<PositionView, 64>;
        /// Execution queue incl. maturity/grace/version/ratification state.
        fn execution_queue() -> futarchy_primitives::BoundedVec<QueuedExecutionView, 32>;
        /// Current welfare pillars, gates, breach + reserve flags, active MetricSpec.
        fn welfare_current() -> WelfareView;                                         // ≤ 1 KiB
        /// Typed constitution params (value + bounds + governance metadata) for ≤ 64 keys.
        fn params(keys: futarchy_primitives::BoundedVec<ParamKey, 64>) -> futarchy_primitives::BoundedVec<ParamView, 64>;
        /// Treasury NAV components (matches the treasury definition in 08), incl. haircut flag.
        fn nav() -> NavView;                                                         // ≤ 256 B
        /// Ring of the last 32 cohort settlements (mirrors RecentCohortSummaries, §7.1).
        fn recent_cohorts() -> futarchy_primitives::BoundedVec<CohortSummaryView, 32>;
        /// Oracle rounds currently open.
        fn open_oracle_rounds() -> futarchy_primitives::BoundedVec<OracleRoundView, 192>; // ≤128 live (16×4×2 per-version); cap 192
    }
}
```

`decision_stats(pid)` MUST return `None` until the proposal's registered decision windows have been sealed and every input required by the decision path is evaluable. It is a finalized decision-statistics view, not an in-Trade projected TWAP.

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
    pub evaluable: bool,
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
    pub vault_state: VaultState,    // proposal: ScalarSettled; Baseline epoch vault: BaselineSettled
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
    pub active_spec_available: bool,
}

pub struct ParamView {
    pub key: ParamKey,
    pub value: u128,                // raw inner scalar of the stored ParamValue
    pub min: u128,
    pub max: u128,
    pub max_delta: u128,
    pub cooldown_blocks: u32,
    pub last_change: BlockNumber,
    pub class: ProposalClass,
    pub min_next: u128,
    pub max_next: u128,
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
    pub haircut_flag: bool,         // 08 §1.2 reserve_impaired; true while reserve health is degraded
    pub spendable_nav: Balance,     // 08 §1.2 spendable NAV; 0 while haircut_flag is true
    pub meter_utilization_bps: u32, // 08 §1.3 rolling-meter utilization, in basis points
    pub class_floors: [Balance; 4], // 08 §4.1; Param, Treasury, Code, Meta declaration order
}

// Every market-bearing proposal carries gate markets. PARAM uses the ≈4,620,989
// USDC class floor and Treasury the 7,393,600-USDC class floor implemented by
// 08 §4.1; the existing four-slot array and optional four-market IDs are unchanged.

/// Stored form == view form (§7.1). FE draft's `DecisionOutcomeCode` is renamed.
pub struct CohortSummary {
    pub epoch: EpochId,
    pub s_1e9: FixedU64,
    pub baseline_twap_1e9: FixedU64,
    pub proposals: futarchy_primitives::BoundedVec<(ProposalId, ProposalClass, DecisionOutcome), 12>,
    pub voided: bool,
    pub settled_at: BlockNumber,
}
pub type CohortSummaryView = CohortSummary;

// The 12-entry proposal bound is the hard maximum of the `epoch.slots` key
// (13 §1), not its launch default. A governance raise can therefore never
// truncate the D-6 chain-served fallback surface.

pub struct OracleRoundView {
    pub component: MetricId,
    pub epoch: EpochId,
    pub spec_version: MetricSpecVersion,  // per-version game key (contract v3, 07 §2(4))
    pub round: u8,                        // 1..=3
    pub reporter: AccountId,
    pub value_1e9: FixedU64,
    pub evidence_hash: H256,
    pub bond: Balance,                    // value-scaled per D-18
    pub challenge_deadline: BlockNumber,
    pub acked_by_watchtowers: u8,         // quorum progress (D-18)
    pub escalated: bool,                   // round > 1; a prior round advanced the game
}
```

`QuoteView.evaluable` MUST be `true` for every successfully computed quote, including a quote whose post-trade state has `within_domain == false`. It MUST be `false` for a missing, closed, trade-inadmissible, inventory-invalid or overflowing book; in that state every price-like field is non-renderable and non-actionable. `within_domain` retains exactly one meaning: the post-trade `|q_L − q_S| / b ≤ 48` predicate.

`WelfareView.spec_version` is meaningful only when `active_spec_available == true`. A selected `MetricSpecVersion` of zero is legal and MUST still set `active_spec_available`; a false flag means that no unique active spec is available under [05](05-welfare-and-decision-engine.md) §4.6.

`ParamView.value`, `.min` and `.max` carry the raw inner scalar of the stored `ParamValue`: `Fixed` uses the 1e9 grid, `Perbill` uses parts per billion, `Percent` uses integer percent, and integer/balance variants use their native scalar. These fields MUST NOT be interpreted as the human/display unit in [13](13-parameters.md). The canonical `Perbill` → basis-points projection divides by 100,000 and floors; fee recomputation MUST use the frozen `Market::Fee` basis-points constant and cross-check it against the raw `mkt.fee` parameter. `max_delta` remains the conservative symmetric projection `min(upward allowance, downward allowance)` and is intentionally lossy for factor rules. `min_next` and `max_next` are the exact inclusive next-value interval after intersecting the record bounds with its max-Δ rule; no delta rule yields `[min, max]`, and a factor rule rounds the lower end as `ceil(value / factor)` and saturates the upper end as `value × factor`.

`OracleRoundView.escalated` is `round > 1`: it is true iff at least one prior round advanced the game. It MUST be false for a round-1 report even while that round has a live challenger, and MUST NOT be interpreted as “currently challenged”; any future view of that distinct fact requires a separately named field.

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

This section freezes the **names and field shapes of the events the canonical frontend ingests**. It is exhaustive for that ingest set: the frontend `CRITICAL_SURFACE` list and local-index ingest filter MUST use exactly these names. It is not an exhaustive declaration of every event a pallet may emit.

An event MUST appear here if any of the following holds: (a) the canonical frontend ingests it for a workflow in [11](11-frontend-workflows.md); (b) another architecture document **requires the canonical frontend to read or surface it**; or (c) it is the sole on-chain record of a terminal proposal, vault or cohort state transition. An event meeting none of these conditions MAY be emitted as a pallet-local operational diagnostic; it is not contract surface, and adding, changing or removing one does not bump the contract version.

Criterion (b) is deliberately narrower than "another document specifies its fields". A document may fix an event's name and field semantics *without* making it contract surface — [08](08-treasury-and-economics.md) §1.4 does exactly that for `KeeperBudgetLow`/`KeeperBudgetExhausted`, which stay treasury-owned and amendable there without an integration-contract change. What binds an event here is a frontend read obligation, not the existence of a normative field list.

**Off-contract diagnostics are defined by rule, not by enumeration.** A pallet event that fails (a)–(c) is outside this schema whether or not it is named here; the per-pallet rows below are the complete ingest set, and everything else a pallet emits is outside it by construction. Pallets with no row in this section (`pallet-welfare`, `pallet-futarchy-treasury` beyond the row below, `pallet-constitution`) have no event in the ingest set. Shipped examples of the excluded class, given only to make the boundary concrete: `pallet-execution-guard::PendingOutflowSyncFailed`, `pallet-epoch::IntakePauseSet`/`IntakePauseCleared`, and the ledger and market freeze/pause diagnostics.

Canonical names below are FINAL. **X-11d fix:** the FE draft's four misnamed epoch events are corrected — `Withdrawn` → **`ProposalWithdrawn`**, `Cancelled` → **`ProposalCancelled`**, `Qualified` → **`ProposalQualified`**, `Deferred` → **`ProposalDeferred`**.

| Pallet | Events (canonical) |
|---|---|
| `pallet-conditional-ledger` | `Split`, `Merged`, `ScalarSplit`, `ScalarMerged`, **`GateSplit { pid: ProposalId, branch: Branch, gate: GateType, amount: Balance }`**, **`GateMerged { pid: ProposalId, branch: Branch, gate: GateType, amount: Balance }`**, `PositionTransferred`, **`BaselineSplit { epoch: EpochId, amount: Balance }`**, **`BaselineMerged { epoch: EpochId, amount: Balance }`**, `VaultResolved { pid, branch }`, **`VaultVoided { pid }`** (D-1, X-11f), `ScalarSettlementSet { pid, branch, s }` (carries winning branch — B-low), `GateSettled { pid, branch, gate, outcome }` (B-2), **`BaselineSettled { epoch: EpochId, s: FixedU64 }`**, `Redeemed`, `ScalarRedeemed`, `ScalarPairRedeemed { pid, amount }` (B-5), **`GateRedeemed { pid: ProposalId, gate: GateType, amount: Balance }`**, **`VoidRedeemed { pid, kind, amount, payout }`** (D-1), **`BaselineRedeemed { epoch: EpochId, side: ScalarSide, payout: Balance }`**, `VaultReaped`, **`BaselineVaultReaped { epoch: EpochId, residue: Balance }`** |
| `pallet-market` | §5 table |
| `pallet-epoch` | `ProposalSubmitted`, `ProposalWithdrawn`, `ScreeningStarted`, `ProposalCancelled { pid, reason }`, `ProposalQualified`, `ProposalDeferred`, **`SlotsShrunk { epoch: EpochId, requested: u32, funded: u32, dropped: Vec<ProposalId> }`**, `MarketsOpened`, `DecisionExtended`, `ProposalQueued { pid, payload_hash, maturity }`, `ProposalRejected { pid, reason }`, `ProposalDelayed { pid, justification_hash }`, `RerunScheduled`, `RerunOpened`, `MandateExpired`, `MeasurementStarted { cohort }`, `CohortSettled { epoch, s }`, **`CohortVoided { epoch: EpochId }`**, **`BaselineCarried { pid: ProposalId, epoch: EpochId }`**, **`ProposalForceRejected { pid, reason }`** — emitted by transition T20 (emergency/VOID force-reject), which previously emitted nothing and silently corrupted every event-derived archive (X-11f), `IntakeSlashed { pid, reason, amount }` (accompanies every partial intake-bond slash — [06](06-governance-and-guardians.md) §4). In `SlotsShrunk`, `requested` and `funded` are proposal-slot counts, not USDC amounts; USDC determines which qualified entries fit, while these fields report the pre- and post-shrink entry counts. |
| `pallet-execution-guard` | `Executed { pid, record }`, `ExecutionFailed { pid, outcome: DispatchOutcomeCode }`, `Ratified { pid, referendum_index }` (written by `execution_guard.ratify(proposal_id, referendum_index)`, the sole `ratify`-track governance call — [06](06-governance-and-guardians.md) §2.2), `UpgradeAuthorized { code_hash: H256, authorized_at: BlockNumber }` (system-event mirror carrying `authorized_at` for the `DescriptorLeadTime` check, D-14), **`Enqueued { pid: ProposalId, maturity: BlockNumber }`**, **`Rejected { pid: ProposalId, reason: RejectReason }`**, **`UpgradeApplied { code_hash: H256, spec_version: u32 }`**, **`PreimageUnpinned { pid: ProposalId, payload_hash: H256 }`**, **`UpgradeAborted { code_hash: H256 }`**. `UpgradeAuthorized` remains the two-field public event; `applicable_at` is derived as `authorized_at + DescriptorLeadTime` ([09](09-execution-upgrades-and-rollout.md) §2.1). |
| `pallet-oracle` | §7.2 table |
| `pallet-registry` | `IncidentFiled`, `MilestoneFiled`, `IncidentChallenged`, `MilestoneChallenged`, `IncidentUpheld`, `IncidentRejected`, `MilestoneAccepted`, `MilestoneRejected`, `FilingBondSlashed`, `RegistryEpochClosed` (field detail in [07](07-oracle-and-disputes.md); names frozen here), **`WindowAcknowledged { epoch: EpochId, filing_id: FilingId, watchtower: AccountId }`**, **`WindowExtended { epoch: EpochId, filing_id: FilingId, new_deadline: BlockNumber }`**. `FilingId = u32`; these pallet-registry events are distinct from the identically named pallet-oracle events in §7.2, which carry `component`/`round`. |
| `pallet-guardian` | `GuardianAction { action_id, power, target, justification_hash }`, `ForceRerun { pid, justification_hash, window_end }`, `PlaybookActivated { id, trigger, expiry }`, `PlaybookRenewed { id }`, `PlaybookExpired { id }`, `ReviewScheduled { action, referendum }`, **`MembersSet { members: [AccountId; 7] }`**, **`ActionProposed { action_id: ActionId, power: GuardianPower }`**, **`ActionApproved { action_id: ActionId, who: AccountId, approvals: u8 }`**, **`ActionRatified { action: ActionId }`**, **`ReviewFailed { action: ActionId, slashed_each: Balance }`**, **`RecallScheduled { action: ActionId, referendum: u32 }`**, **`RecallEnacted { action: ActionId, removed: BoundedVec<AccountId, ConstU32<7>> }`**, **`PlaybookRegistrationSet { id: PlaybookId, enabled: bool }`** |
| `pallet-attestor` | **`MembersSet { members: Vec<AccountId> }`**, **`AttestationSubmitted { attestation_id: AttestationId, pid: ProposalId, artifact_hash: H256, attestor: AccountId }`**, **`AttestationChallenged { attestation_id: AttestationId, challenger: AccountId, evidence_hash: H256 }`**, **`ChallengeResolved { attestation_id: AttestationId, upheld: bool, loser: AccountId, slashed: Balance }`**, **`AttestorEjected { who: AccountId }`**, **`AttestorRemovedForCause { who: AccountId, cause_hash: H256 }`**, **`AttestationRevoked { attestation_id: AttestationId, pid: ProposalId, attestor: AccountId, cause_hash: H256 }`** |
| `pallet-futarchy-treasury` | **`NavHaircutFlagged { epoch: EpochId, flag: bool }`** — emitted on every reserve-health flag transition; in the ingest set under criterion (b) because [08](08-treasury-and-economics.md) §1.2(4) requires the frontend to surface the flag on every NAV render. The line's other events (`Spent`, `StreamOpened`/`Claimed`/`Cancelled`, `BudgetLineFunded`, `VitIssued`, `KeeperBudgetLow`, `KeeperBudgetExhausted`, `NavFloorUnmet`, the coretime events) are **not** in the ingest set and stay treasury-owned per [08](08-treasury-and-economics.md) §1.4 |
| `frame-system` / upgrade path | `CodeUpdated`, `UpgradeAuthorized` (native), ingested for descriptor switching |

`pallet-execution-guard::PendingOutflowSyncFailed { queued: u32, fail_static: bool }` is deliberately outside the ingest set: it is an operations alarm with no canonical frontend workflow. It carries no monitoring binding in [12](12-release-and-operations.md) §6.3 today; adding one is an operations concern that does not touch this contract.

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
| **`RecentCohortSummaries`** | **`BoundedVec<CohortSummary, ConstU32<32>>`** | **Ring of the last 32 cohorts** (≈ 4.9 KiB at the current 158-byte `CohortSummary`; exact arithmetic in [13](13-parameters.md) §5), pushed at `settle_cohort` completion, FIFO-evicted; one push per ~21 days amortized into the existing settle crank — negligible weight, no new hook. This is the P-5 storage-list addition the FE draft omitted from §5.2.3, applied: a fresh browser renders ~22 months of settlement history with zero infrastructure dependency (D-6 layer 1) |
| `Cohorts` | `map EpochId → CohortInfo` | ≤ 4 non-terminal |

### 7.2 `pallet-oracle` (X-11c — canonical names; [07](07-oracle-and-disputes.md) uses these)

Storage:

| Item | Type | Bound |
|---|---|---|
| `Reporters` | `map AccountId → ReporterInfo { stake: Balance, registered_at: BlockNumber, offenses: u8 }` | counted; ≥ 3 required before attested components admit |
| `Rounds` | `map (MetricId, EpochId, MetricSpecVersion) → RoundState { component: MetricId, epoch: EpochId, round: u8, spec_version: MetricSpecVersion, reporter: AccountId, value: FixedU64, evidence_hash: H256, bond: Balance, challenge_deadline: BlockNumber, extended: bool, challenger: Option<AccountId>, counter_value: Option<FixedU64>, acks: u8, report_hash: H256, stake_at_risk: Balance, cumulative_reporter_bond: Balance, cumulative_challenger_bond: Balance }` | ≤ **128** = 16 components × ≤ 4 settling epochs × ≤ 2 concurrent frozen versions — one live game per `(component, epoch, spec_version)`. The **triple key** (contract v3) is normative: 07 §2(4) runs an independent game per frozen version, so an activation boundary keeps two games live for one `(component, epoch)`; the pair key of contract v2 could not hold them (it maps one value per key). The value re-embeds `component`/`epoch`/`spec_version` for a `try_state` key-integrity check. `report_hash`/`stake_at_risk`/`cumulative_*_bond` back per-round ack keying, bond-schedule freezing and §5.5 slashing; the FE reads the `OracleRoundView` projection (§4), not this struct |
| `ComponentValues` | `map (MetricId, EpochId, MetricSpecVersion) → SettledComponent { value: FixedU64, path: SettlePath, flagged: bool }` | reaped at cohort settlement; **triple key** (contract v3) — per-version games settle their own cohorts, so one `(component, epoch)` can carry a settled value per frozen version. `SettlePath ∈ { Unchallenged, Recomputed, Adjudicated, ChallengerDefault, Neutral }`; `ChallengerDefault` is the §5.3 deadline path when the reporter does not post a signed `counter_report` |
| `Watchtowers` | `map AccountId → WatchtowerInfo { stake: Balance, registered_at: BlockNumber, inactive_epochs: u8 }` | counted, ≤ `wt.max = 16` seats; bonded acknowledgment quorum (D-18; registry semantics in [07](07-oracle-and-disputes.md) §4) |
| `ReserveHealth` | `{ consecutive_fails: u8, consecutive_passes: u8, unhealthy: bool, last_query_id: u64, last_probe_at: BlockNumber, pending_since: Option<BlockNumber> }` | single value; the deterministic reserve-probe state (`R`, [07](07-oracle-and-disputes.md) §8). `last_probe_at`/`pending_since` (contract v3) time the probe for the fail-static timeout |

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
| `Params` | `map ParamKey → ParamRecord` | read via `params()`; §9 binding rules. `ParamRecord` carries both `last_changed_epoch` (the cooldown clock, [13](13-parameters.md) rule 7) and `last_change_block` (contract v4) — §4's `ParamView.last_change` is a block number, and epochs do not determine it |
| **`PhaseFlags`** | `u32` bitset | **The key the frontend binds trading enablement (and the Phases 0–3 "bootstrap governance — sudo active" banner, D-13) to.** Bit assignments: 0 = shadow mode, 1 = PARAM armed, 2 = TREASURY armed, 3 = CODE/META armed, 4 = sudo present, 5 = ledger frozen (PB-LEDGER-FREEZE), 6 = dead-man engaged, 7 = reserve-health flag; bits 8–31 reserved (append-only) |
| **`ReleaseChannel`** | fixed-layout raw value | §12 — NOT ordinary SCALE-metadata-dependent storage |

### 7.4 `pallet-market` (X-10) and other reads

| Item | Type | Notes |
|---|---|---|
| **`BaselineMarketOf`** | **`map EpochId → MarketId`** (in **`pallet-market`** — the pallet home per [04 §8.3](04-markets-and-pricing.md)) | **X-10 fix, contract-v8 retained-book rule**: the declared backing storage for `baseline_market(epoch)`. Written at Baseline book creation; retained for exactly as long as the referenced `MarketBook` exists, including after its terminal latch and for a strictly-past orphan epoch whose Baseline vault is still `Open`; removed atomically only when that book is successfully reaped. Its structural bound is therefore `MaxStoredMarkets = 2,240`, not the active/POL count or `RecentCohortSummaries` ring. A present mapping MUST resolve to a present `BookKind::Baseline` owned by the same epoch; `try-state` enforces the inverse |
| `Markets` | `map MarketId → MarketBook<AccountId>` | ≤ `MaxStoredMarkets = 2,240` total retained rows and ≤ `MaxLiveMarkets = 196` rows without a durable terminal latch *(normative values: [13](13-parameters.md))*; first terminal observation removes the book's checkpoint/decision-window auxiliaries and releases the active/POL slot, while the readable book remains until reap. Once the latch plus archive delay permit reap, it atomically discards only the book/fee accounts' fixed-universe inventory, removes their protocol-account registrations, the book and any `BaselineMarketOf` inverse; it neither waits for nor removes the owning ledger vault, marker or claimant rows. `MarketState` is the frame-free core's whole-state aggregate and is not the stored value |
| `pallet-inflow-caps::CumulativeDeposits` | `map AccountId → u128` | Per-account cumulative XCM USDC inflow meter for the Phase-3 deposit-cap precheck ([09](09-execution-upgrades-and-rollout.md) §5.2) |

`pallet-conditional-ledger::{Vaults, BaselineVaults, Positions, PositionTotals}` — note the **key order of `Positions` is `(PositionId, AccountId)`** (per-vault drainable, B-med); a per-account storage prefix scan is therefore NOT available, and the frontend MUST use `account_positions()` (the runtime API iterates the bounded live-vault set) or the per-account key index maintained by the ledger ([03](03-conditional-ledger.md)). `pallet-execution-guard::{Queue, Ratifications, ExecutionRecords}` (a `RatificationRecord` is written by the frozen governance call `execution_guard.ratify(proposal_id, referendum_index)`, binding `(pid, payload_hash)` — [06 §2.2](06-governance-and-guardians.md)); `pallet-welfare::{Snapshots, MetricSpecs, GateBreachFlags}`; `pallet-guardian` membership/allowances; `System.Account`, `ForeignAssets.Account(USDC_LOCATION, who)` (NOT `Assets.Account(1337, who)` — X-11a; the USDC identifier is the XCM Location of §8).

### 7.5 `pallet-attestor`

The shipped pallet uses value storage rather than keyed maps. The item names and the full SCALE value shapes below are frozen byte-for-byte; “—” means the storage item has no map key.

| Item | Key type | Value type | Notes |
|---|---|---|---|
| `Members` | — | `BoundedVec<AttestorInfo, ConstU32<16>>`, where `AttestorInfo { account: AccountId, bond: Balance, false_count: u8, active: bool }` | Elected bonded member set; value-query default is empty |
| `Attestations` | — | `BoundedVec<Attestation, ConstU32<256>>`, where `Attestation { id: AttestationId, pid: ProposalId, artifact_hash: H256, statement_hash: H256, attestor: AccountId, submitted_at: BlockNumber, challenge_deadline: BlockNumber, challenge: Option<ChallengeStatus> }` and `ChallengeStatus ∈ { Open { challenger: AccountId, evidence_hash: H256, bond: Balance }, Upheld, Rejected }` | Flat shipped attestation ledger; value-query default is empty |
| `Liabilities` | — | `BoundedVec<AttestorLiability, ConstU32<16>>`, where `AttestorLiability { account: AccountId, bond: Balance, false_count: u8, ejected: bool }` | Bond basis for departed/ejected attestors until every record is reaped; value-query default is empty |
| `Revocations` | — | `BoundedVec<AttestationRevocation, ConstU32<256>>`, where `AttestationRevocation { attestation_id: AttestationId, pid: ProposalId, attestor: AccountId, cause_hash: H256 }` | Durable cause markers; the original attestation shape is unchanged |
| `NextAttestationId` | — | `AttestationId = u32` | Monotone cursor; value-query default is 0 |

Storage items not listed in this section are not contract surface, and their raw decodability through portable metadata is not guaranteed. In particular, every treasury consumer MUST bind `nav()` rather than raw `pallet-futarchy-treasury::State`.

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
| Contract version | `INTEGRATION_CONTRACT_VERSION = 11` (runtime constant) |

---

## 9. Constants and parameter binding (X-11e/h — no FE hardcodes)

Two representations exist, and the frontend MUST bind to them and never hardcode:

1. **Kernel constants (class K)** have *no storage representation*; they are exposed as **pallet constants in the runtime metadata** (the constants API) and are readable without any storage query. They change only via Wasm upgrade, which the frontend already tracks through descriptors.
2. **Tunables** live in `pallet-constitution::Params` and are read via `params()` (or the raw `Params` map). Only hard envelopes of keys in [13](13-parameters.md) reading rule 7's exhaustive kernel-bounded set are also metadata constants; META-amendable registry bounds remain in `Params` and bind through `ParamView.min` / `ParamView.max`.

Enumeration of every value the frontend's precondition tables re-check (defaults/bounds are quoted for readability; *normative values: [13](13-parameters.md)*):

| Value | Representation (FE binding target) | Used by FE precondition row |
|---|---|---|
| Live `ledger.min_split` (K floor 0.01 USDC) | `params()` (authoritative live record); the already-wired `MinSplit` metadata constant mirrors that live value | `ledger.split/merge` |
| Per-trade min / max (`mkt.min_trade = 1`, `mkt.max_trade = b/4`) | metadata constants (K) | `market.buy/sell` |
| `MinTransfer` | metadata constant (K) | `ledger.transfer` |
| `MaxPositionsPerAccount = 64` (protocol accounts exempt) | metadata constant | `ledger.transfer` (recipient bound), position views |
| Positions entry deposit (0.1 USDC) | metadata constant | `ledger.split`, `transfer` fee headroom |
| `IntakeQueue = 64` bound | metadata constant | `epoch.submit` queue-cap check |
| `intake.max_per_account` live rate limit (launch default 4; META-amendable bounds [2, 8]) | `params()` — bind `ParamView.value`, `.min`, and `.max`; no metadata constant | `epoch.submit` account-rate check |
| `MaxLiveProposals = 32` | metadata constant | discovery bounds |
| `prop.bond` per class | `params()` | `epoch.submit` |
| `mkt.fee` (30 bps default) | `Market::Fee` for the basis-points projection used by quote recomputation, cross-checked against the raw `Perbill` inner scalar from `params()` (§4) | quote display, `buy/sell` cost recompute |
| `mkt.obs_interval` (10 blocks) | `params()` | crank staleness check |
| `dec.window`, `dec.trailing`, `dec.delta`, `dec.sigma`, `dec.coverage`, `dec.v_min`; `dec.extension` (K) | `params()` for tunable values/bounds. Metadata constants exist only for the rule-7 kernel-bounded `dec.window` / `dec.delta` / `dec.sigma` floors and kernel-only `dec.extension`. `dec.trailing` / `dec.coverage` / `dec.v_min` bind through `ParamView.min` / `.max`: respectively 3,600 / 28,800 blocks, 90 / 99 percent, and ×0.1 / ×10 of the per-class schedule; only the effective-v-min `2·InCapPrize` term is K. | finalized decision statistics, `decide` crank |
| `gate.p_max`, `gate.eps` | `params()` (0.10 K ceiling as constant) | gate-market screens |
| `exec.timelock` per class, `exec.grace` | `params()` (K floors as constants) | `execution_guard.execute` |
| `orc.bond_floor`/`orc.rounds`/`orc.window`, `orc.bond_bps` value scaling, `orc.reporter_stake` | `params()` | `oracle.report/challenge` |
| `trs.cap_proposal`/`cap_30d`/`cap_180d`, `trs.stream_threshold` | `params()` | treasury proposal screens |
| Phase-3 deposit caps `phase3.tvl_cap` / `phase3.dep_cap` | `params()` using the canonical keys in [13](13-parameters.md), combined with §7.4 `CumulativeDeposits` | global/per-account deposit-cap precheck |
| `fee.vit_usdc_rate` | `params()` | fee-currency selector (D-12) |
| `epoch.length`, `epoch.slots` | `params()` (live values and record bounds) + applicable metadata floor/bound constants | countdowns, phase headers |
| Phase-offset fractions | `PhaseOffsets` metadata constant only (kernel-fixed `futarchy-primitives::phase_offsets`; never `Params`) | countdowns, phase headers |
| `DescriptorLeadTime = 43,200` blocks | metadata constant | upgrade banners, execute precondition |
| `RecentCohortSummaries` ring size = 32; books/proposal ≤ 6; `MaxLiveMarkets = 196`; `MaxStoredMarkets = 2,240` | metadata constants | active history/chart bounds; retained direct-read/discovery bound |

### Frozen metadata-constant names (SQ-138)

The tuple/array orders in this table are part of the freeze. Every per-class array is ordered **Param, Treasury, Code, Meta**. `FixedU64` values use the contract's 1e9 grid; `Balance` values use USDC base units where the row is USDC-denominated.

| Pallet | Constant name | Type | Value source |
|---|---|---|---|
| Constitution | `INTEGRATION_CONTRACT_VERSION` | `u32` | `futarchy_primitives::INTEGRATION_CONTRACT_VERSION` (= 11) |
| Constitution | `MaxParams` | `u32` | `constitution_core::MAX_PARAMS` (= 128) |
| Constitution | `MaxCapabilities` | `u32` | `constitution_core::MAX_CAPABILITIES` (= 64) |
| Constitution | `MaxMeters` | `u32` | `constitution_core::MAX_METERS` (= 16) |
| ConditionalLedger | `MinSplit` | `Balance` | live `Params[ledger.min_split]`, backstopped by `kernel::MIN_SPLIT_USDC` |
| ConditionalLedger | `PositionDeposit` | `Balance` | live `Params[ledger.pos_dep]` (launch 0.1 USDC) |
| ConditionalLedger | `MaxPositionsPerAccount` | `u32` | `bounds::MAX_ACCOUNT_POSITIONS` (= 64) |
| ConditionalLedger | `ArchiveDelay` | `BlockNumber` (`u32`) | live `Params[ledger.archive]` |
| ConditionalLedger | `ReapBatch` | `u32` | `kernel::REAP_BATCH` (= 100) |
| ConditionalLedger | `MinTransfer` | `Balance` | `kernel::MIN_TRANSFER_USDC` (= 10,000 base units) |
| Market | `Fee` | `u128` | live `Params[mkt.fee]`, projected in basis points by flooring raw `Perbill / 100,000` (launch 30) |
| Market | `ObsInterval` | `u64` | live `Params[mkt.obs_interval]`, promoted from `u32` (launch 10 blocks) |
| Market | `Kappa1e9` | `u64` | live `Params[mkt.kappa]` on the 1e9 grid (launch 5,000,000) |
| Market | `ArchiveDelay` | `BlockNumber` (`u32`) | live `Params[ledger.archive]` |
| Market | `MinTrade` | `Balance` | `kernel::MIN_TRADE_USDC` (= 1,000,000 base units) |
| Market | `MaxTradeRatio` | `(u32, u32)` | kernel ratio `(1, 4)` (`b/4`) |
| Market | `MaxLiveMarkets` | `u32` | `bounds::MAX_LIVE_MARKETS` (= 196) |
| Market | `MaxStoredMarkets` | `u32` | `bounds::MAX_STORED_MARKETS` (= 2,240) |
| Market | `GatePMaxCeiling` | `FixedU64` | `kernel::GATE_P_MAX_CEILING_1E9` (= 100,000,000; 0.10) |
| Market | `GateEpsFloor` | `FixedU64` | [13 §1](13-parameters.md) `gate.eps` K floor (= 5,000,000; 0.005) |
| Oracle | `MaxRoundCloseBatch` | `u32` | `kernel::TICK_BATCH` (= 10) |
| Registry (each instance) | `Kind` | `RegistryKind` | runtime instance `Config::Kind` (`Incident` or `Milestone`) |
| Registry (each instance) | `ArchiveDelay` | `BlockNumber` (`u32`) | live `Params[ledger.archive]` |
| Registry (each instance) | `MaxFilingsPerEpoch` | `u32` | `kernel::REG_MAX_FILINGS_EPOCH` (= 64) |
| Registry (each instance) | `MaxEvidenceLen` | `u32` | fixed `H256` evidence-hash width (= 32 bytes) |
| ExecutionGuard | `INTEGRATION_CONTRACT_VERSION` | `u32` | `futarchy_primitives::INTEGRATION_CONTRACT_VERSION` (= 11) |
| ExecutionGuard | `MaxLiveProposals` | `u32` | `bounds::MAX_LIVE_PROPOSALS` (= 32) |
| ExecutionGuard | `MaxExecutionRecords` | `u32` | `bounds::MAX_EXECUTION_RECORDS` (= 256) |
| ExecutionGuard | `MaxCalls` | `u32` | `kernel::MAX_CALLS` (= 16) |
| ExecutionGuard | `MaxPayloadBytes` | `u32` | `kernel::MAX_BYTES` (= 65,536) |
| ExecutionGuard | `DescriptorLeadTime` | `BlockNumber` (`u32`) | `kernel::DESCRIPTOR_LEAD_TIME_BLOCKS` (= 43,200) |
| ExecutionGuard | `MaxRuntimeCodeBytes` | `u32` | runtime `Config::MaxRuntimeCodeBytes` (`pallet_preimage::MAX_SIZE`) |
| ExecutionGuard | `ExecutionTimelockFloor` | `[u32; 4]` | [13 §1](13-parameters.md) `exec.lock.*` K hard minima, `[14,400; 4]` blocks |
| ExecutionGuard | `ExecutionGraceFloor` | `u32` | [13 §1](13-parameters.md) `exec.grace` K hard minimum (= 100,800 blocks) |
| Epoch | `INTEGRATION_CONTRACT_VERSION` | `u32` | `futarchy_primitives::INTEGRATION_CONTRACT_VERSION` (= 11) |
| Epoch | `MaxLiveProposals` | `u32` | `bounds::MAX_LIVE_PROPOSALS` (= 32) |
| Epoch | `MaxIntakeQueue` | `u32` | `bounds::INTAKE_QUEUE` (= 64) |
| Epoch | `MaxNonTerminalCohorts` | `u32` | `bounds::MAX_NON_TERMINAL_COHORTS` (= 4) |
| Epoch | `RecentCohortSummariesBound` | `u32` | `bounds::RECENT_COHORT_SUMMARIES` (= 32) |
| Epoch | `TickBatch` | `u32` | `kernel::TICK_BATCH` (= 10) |
| Epoch | `PhaseOffsets` | `[(u32, u32); 7]` | `futarchy_primitives::phase_offsets`: `[(0,21), (3,21), (4,21), (5,21), (15,21), (18,21), (20,21)]` for Intake/Qualify/Seed/Trade/DecideWindow/Decide/Housekeeping |
| Epoch | `MaxBooksPerProposal` | `u32` | `bounds::BOOKS_PER_PROPOSAL` (= 6) |
| Epoch | `MinEpochLength` | `u32` | `kernel::MIN_EPOCH_LENGTH_BLOCKS` (= 201,600) |
| Epoch | `DecisionWindowFloor` | `u32` | [13 §1](13-parameters.md) `dec.window` K hard minimum (= 14,400 blocks) |
| Epoch | `DecisionExtension` | `u32` | `kernel::DEC_EXTENSION_BLOCKS` (= 43,200) |
| Epoch | `DecisionDeltaFloors` | `[FixedU64; 4]` | [13 §1](13-parameters.md) `dec.delta.*` K hard minima (= `[5,000,000; 4]`) |
| Epoch | `DecisionSigmaFloors` | `[FixedU64; 4]` | [13 §1](13-parameters.md) `dec.sigma.*` K hard minima (= `[0; 4]`) |
| Welfare | `INTEGRATION_CONTRACT_VERSION` | `u32` | `futarchy_primitives::INTEGRATION_CONTRACT_VERSION` (= 11) |
| Welfare | `MaxMetricSpecs` | `u32` | `welfare_core::MAX_METRIC_SPECS` (= 16) |
| Welfare | `MaxSnapshots` | `u32` | `welfare_core::MAX_SNAPSHOTS` (= 20) |
| Welfare | `MaxGateFlags` | `u32` | `welfare_core::MAX_GATE_FLAGS` (= 20) |
| Welfare | `MaxDailyGateSamples` | `u8` | `welfare_core::MAX_DAILY_GATE_SAMPLES` (= 64) |
| FutarchyTreasury | `INTEGRATION_CONTRACT_VERSION` | `u32` | `futarchy_primitives::INTEGRATION_CONTRACT_VERSION` (= 11) |
| FutarchyTreasury | `MaxStreams` | `u32` | `futarchy_treasury_core::MAX_STREAMS` (= 128) |
| FutarchyTreasury | `MaxBudgetLines` | `u32` | `futarchy_treasury_core::MAX_BUDGET_LINES` (= 32) |
| FutarchyTreasury | `MaxPolCommitments` | `u32` | `futarchy_treasury_core::MAX_POL_COMMITMENTS` (= 196) |
| Guardian | `GuardianSeats` | `u32` | `guardian_core::GUARDIAN_SEATS` (= 7) |
| Guardian | `GuardianThreshold` | `u8` | `guardian_core::GUARDIAN_THRESHOLD` (= 5) |
| Guardian | `GuardianBond` | `Balance` | `guardian_core::GUARDIAN_BOND` (= 50,000 VIT) |
| Guardian | `PlaybookFreezeWindowBlocks` | `BlockNumber` (`u32`) | `kernel::PLAYBOOK_FREEZE_WINDOW_BLOCKS` (= 201,600) |
| Attestor | `AttMinMembers` | `u32` | `kernel::ATT_MIN_MEMBERS` (= 3) |
| Attestor | `AttQuorum` | `u32` | `kernel::ATT_QUORUM` (= 2) |
| Attestor | `ChallengeWindowBlocks` | `BlockNumber` (`u32`) | `attestor_core::CHALLENGE_WINDOW_BLOCKS` (= 43,200) |

The `PalletId` configuration constants exposed by ConditionalLedger, Market and Registry are intentionally absent: they are internal custody identifiers and no frontend workflow binds them. No placeholder names from external release tooling are normative; this table is the canonical name freeze.

Catch-all rule: **any** [13](13-parameters.md) key a frontend workflow evaluates MUST be sourced from `params()`/metadata at a pinned finalized block. Shipping a numeric copy of any of these values in the frontend bundle is a release-gate failure (frontend CI asserts no literal matches against the constants list).

---

## 10. WSS bootnode chain-spec requirement (D-6, X-4)

The production (and Paseo) chain-spec artifacts in `deploy/` MUST list **≥ 8 browser-reachable WSS multiaddrs across ≥ 4 independent operators, with ≥ 2 endpoints on port 443** (corporate/mobile networks block non-443 WSS). These endpoints are the canonical frontend's guaranteed dial set — the fallback for the open **[VERIFY browser-WSS peer behavior under smoldot 3.x]**. Operators hold the protocol-funded 30-day served-state commitment ([12](12-release-and-operations.md)); the requirement is a rollout phase gate ([01 §7](01-system-overview.md), [09](09-execution-upgrades-and-rollout.md)). Chain-spec updates that would drop the set below any of the three thresholds MUST NOT be released.

---

## 11. Backend-published test artifacts per release (X-15)

The frontend's compatibility controls are release-gated on backend-published inputs. **Every tagged runtime release** MUST publish, as CI artifacts attached to the release in the `futarchy-chain` repository and mirrored content-addressed alongside the frontend release channel:

| Artifact | Contents | Consumed by |
|---|---|---|
| Runtime Wasm + metadata | Reproducibly-built primary `runtime.wasm`, SCALE metadata blob, metadata hash and spec_name/spec_version; for every primary, the same fields for its separately built, exact-next-version terminal-recovery runtime | Descriptor regeneration + drift CI for **both live-capable versions** (FE §12.1-equivalent, [12](12-release-and-operations.md)); a recovery image is ineligible until its descriptor is live |
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

Total **168 bytes** (v1.0 baseline — the pre-freeze 78- and 92-byte drafts in earlier drafts of [09](09-execution-upgrades-and-rollout.md)/[12](12-release-and-operations.md) are superseded by this merged layout; no schema bump, this is the first frozen version). Offset 112 `spec_version` is always the currently installed runtime `spec_version`, never the target of a pending upgrade; a stranded reader uses it to determine compatibility with the runtime that is actually installed.

Post-genesis writers and field ownership are exhaustive:

1. Writer (a), the **execution guard**, exclusively owns offsets 112–119 (`spec_version`, `pending_authorized_at`) and flags bit 2 (`URGENT_UPGRADE`). At `UpgradeAuthorized` it MUST set `pending_authorized_at` and bit 2, stamp `updated_at`, and MUST NOT change `spec_version`. At applied-upgrade detection it MUST set `spec_version` to the newly installed version, clear `pending_authorized_at` and bit 2, and stamp `updated_at`. At relay abort it MUST clear `pending_authorized_at` and bit 2, leave `spec_version` unchanged because the old runtime remains installed, and stamp `updated_at`. Both terminal clears are unconditional on the channel's prior contents.
2. Writer (b), the **`ConstitutionalValues` origin** via `constitution.set_release_channel`, owns offsets 1–107 and 120–163 plus flags bits 0–1 on each canonical repoint, `min_supported_version` bump or key revocation ([12](12-release-and-operations.md)). It MUST merge its input with the stored record and preserve offsets 112–119 and flags bit 2 byte-for-byte. Offset 108 `updated_at` is shared write metadata and each writer MUST stamp it. Offset 0 remains the fixed schema byte and flags bits 3–31 remain reserved zero.

No other origin can write the record. The layout MUST NEVER change except by appending fields beyond offset 168 with a schema bump; readers parse by offset, never by SCALE metadata. **I-30:** `ExecutionGuard::PendingUpgrade.is_some()` iff channel `pending_authorized_at != 0` iff flags bit 2 is set; runtime `try_state` MUST enforce both equivalences. With writer ownership enforced, a compromised writer (b) can still cause a false "update available" banner pointing at a TXID users independently verify, or warning/signing friction in old releases, but it cannot suppress or fabricate the execution guard's pending-upgrade indication ([14](14-threat-model.md)).

---

## 13. Change control (D-2; resolves F-4, X-11h)

1. **Joint ownership.** This document is owned by the backend and frontend teams jointly. Every change REQUIRES explicit sign-off from a named owner on each side, recorded in the document history.
2. **Versioning.** `INTEGRATION_CONTRACT_VERSION` (§8) is stamped in `futarchy-primitives`, exposed via the constants API, and echoed in the frontend `release.json`. Any change to §2–§12 bumps it. The `FutarchyApi` itself carries an `sp_api` version; additive methods bump the API version *and* the contract version.
3. **Append-only.** SCALE types, event fields and view types may only gain trailing fields/variants post-genesis. Renames and removals require a new type, a migration, and a major contract bump with a coordinated FE release inside the `DescriptorLeadTime` window (D-14).
4. **No hardcodes (X-11h).** The frontend binds to the constants API and `params()` for every chain-tunable value (§9); frontend CI enforces the no-literal rule. The 64-position bound and every other formerly hardcoded §21-tunable are chain-read.
5. **Release coupling.** Backend E15 and frontend FE-R1 are the two ends of this contract; neither side's release gates pass while the other's contract surface is red (§11).
6. **Contingency.** If a contract regression ships anyway, the frontend degrades to the D-6 layer-1 surface (chain-served `RecentCohortSummaries` + 8-checkpoint TWAP series + direct storage reads) — reduced depth, full correctness; it never falls back to a trusted third-party service.
7. **Independent counters.** `INTEGRATION_CONTRACT_VERSION` and the SDK's `RuntimeVersion.transaction_version` are independent counters. The contract version is exposed through `futarchy-primitives`, the constants API and `release.json`; `transaction_version` denotes compatibility of existing dispatchables as embedded in signed-transaction validity. An additive contract bump MUST NOT change `transaction_version`.

**Version history.**

- **v11 (2026-07-23) — signed oracle escalation custody (A12).** Section 7.2 adds the explicit `ChallengerDefault` `SettlePath` variant for the §5.3 deadline outcome. The oracle's signed `counter_report` advances a challenged round only with reporter consent; challenger identity and cumulative bond liabilities persist across rounds, and d20 neutralization retains the bounded round record until bond/reputation resolution. This is a pre-genesis additive revision; no migration is required. Joint backend+frontend sign-off: **the user (owner for both sides under R-1), 2026-07-23, through the standing autonomous-resolution delegation.**
- **v10 (2026-07-23) — cause-aware attestor departure and durable revocation (B19).** Section 6 appends the two attestor lifecycle events; §7.5 adds bounded `Liabilities` and `Revocations` auxiliary values while retaining the original `Attestation` shape; §8 exposes `remove_for_cause` (ConstitutionalValues), permissionless `reap_attestation`, the new storage projections and custody events. Queue-time admission continues to require the live ≥3 roster; after a record is committed, execution uses record quorum and durable revocation state. `set_members` holds the live `att.bond` from every newly seated account and carries unsettled bases into `Liabilities`; challenge bonds are held and slash proceeds route to INSURANCE. This is a pre-genesis additive revision; no migration is required. Joint backend+frontend sign-off: **the user (owner for both sides under R-1), 2026-07-23, through the standing autonomous-resolution delegation.**
- **v9 (2026-07-23) — paired-recovery descriptor coverage (B16).** Section 11 extends the runtime-Wasm/metadata artifact row to the exact paired terminal-recovery runtime because that image becomes the live next `spec_version` after recovery. Backend release assembly MUST publish both metadata blobs and frontend descriptor generation/drift CI MUST cover both before the primary is eligible; recovery is no longer an operator-only metadata input. Recovery-only guard calls, internal storage and diagnostics remain outside the canonical frontend ingest set. No SCALE shape, runtime API, storage key, transaction validity or SDK transaction-version change is introduced. Joint backend+frontend sign-off: **the user (owner for both sides under R-1), 2026-07-23, through the explicit B16 repair request and standing autonomous-resolution delegation.**
- **v8 (2026-07-22) — active/retained market-capacity split and bounded reap isolation (SQ-483).** Section 7.4 separates the live/POL obligation envelope (`MaxLiveMarkets = 196`) from total readable book retention (`MaxStoredMarkets = 2,240`), and §9 adds the latter as an additive metadata constant. The retained bound is derived from the independent one-epoch creation maximum, the 14-day epoch floor, the one-year K ceiling on `ledger.archive`, and one boundary batch; terminal observation releases the live/POL slot and deletes auxiliary history while leaving the book readable until archive reap. Market reap is independent of the unbounded claimant-position backlog: at the delay boundary it atomically discards only its book/fee accounts across a fixed 28-cell proposal or four-cell Baseline universe before unregistering them, while ledger claimant cleanup may run before or after. To make that fixed inventory exhaustive, the existing `ledger.transfer` call now rejects every protocol-account recipient; only the `MarketAuthority` internal path may move inventory into protocol custody. Canonical book/fee addresses occupy a domain-separated `AccountId32` namespace classified as protocol custody before creation and after reap; `MarketProtocolAccounts` is ownership/refcount state only, and creation rejects a non-canonical pair before mutation. This is a pre-genesis validity correction: no deployed signed transaction was valid under the prior rule, so `transaction_version` remains at its initial value. The pallet error metadata gains only trailing `ProtocolDestination` / `UnreservedProtocolAccount` variants; no frozen event, API method, view type or storage-key shape changes. `ActiveMarketCount` is a new internal pre-genesis counter and no deployed state exists to migrate. Joint backend+frontend sign-off: **the user (owner for both sides under R-1), 2026-07-22, under the standing autonomous-resolution delegation.**
- **v7 (2026-07-22) — Baseline discovery retention and orphan-settlement liveness (SQ-66/SQ-320).** Section 7.4 replaces the stale ring-coupled `BaselineMarketOf` retention rule with the runtime's market-lifetime rule: the mapping remains present while its referenced Baseline book exists and is removed atomically only with successful book reap. This keeps strictly-past orphan epochs discoverable until their permissionless neutral settlement can write the market-side terminal latch, and it freezes the honest shared structural bound (`MaxLiveMarkets = 196`) instead of the unenforced ≤36 claim. No SCALE type, event, API method, storage key or transaction validity changes; this is a pre-genesis semantic correction to a frozen direct-read item, so no storage migration is required and `transaction_version` remains unchanged. Joint backend+frontend sign-off: **the user (owner for both sides under R-1), 2026-07-22, under the standing autonomous-resolution delegation.**
- **v6 (2026-07-21) — batch C, the contract-reconciliation revision.** Section 6 defines the exhaustive canonical-frontend ingest set (without claiming to exhaust pallet-local events) and adds the shipped epoch, execution-guard, guardian, registry and attestor event shapes. Section 2 appends branch-free `VaultState::BaselineSettled` and restructures `RatificationStatus` to `NotRequired` / `NoPassedRecord` / `Passed`; §§3–4 freeze the const-generic `futarchy_primitives::BoundedVec` API path, append the exact `ParamView` interval, `QuoteView.evaluable` and `WelfareView.active_spec_available`, and define `ParamView` raw units, `OracleRoundView.escalated` and finalized `decision_stats()` availability. Section 7 corrects `Markets` to `MarketBook<AccountId>`, adds `CumulativeDeposits`, and excludes unlisted raw storage from the contract. Section 9 adds the Phase-3 deposit-cap binding, removes the implementation-status column and completes the freeze with the 16 Welfare/FutarchyTreasury/Guardian/Attestor constants. Section 12 assigns exclusive guard ownership of the pending-upgrade bytes, requires merge-preserving `set_release_channel`, freezes current-not-target `spec_version` semantics and adds I-30; §13 declares the contract and transaction-version counters independent. **Pre-genesis revision** — no runtime is deployed, so no migration is required and §13's post-genesis append-only/migration clause (point 3) does not apply; the `RatificationStatus` restructure is taken under that allowance, as v3 and v4 took theirs. Joint backend+frontend sign-off: **the user (owner for both sides under R-1), 2026-07-21, user-delegated batch.**
- **v5 (2026-07-19) — universal market-bearing-class gate markets.** Section 4's class-floor semantics change so every market-bearing proposal carries the existing four-book `(S,C)×(adopt,reject)` gate set: Treasury loses its former ask/NAV threshold and uses **7,393,600 USDC**; PARAM loses its former static-classification exception and its existing `NavView.class_floors` slot rises to **≈4,620,989 USDC**. The frozen `MarketSet.gates: Option<[MarketId; 4]>`, `ProposalSummaryView.gate_markets`, `DecisionStatsView.gate_twaps_1e9`, `NavView.class_floors: [Balance; 4]`, and `MaxLiveMarkets = 196` shapes are unchanged. The PARAM extension is folded into the same pre-genesis v5 revision; no v6 is warranted because no separately deployed contract or SCALE shape intervened. **Pre-genesis revision** — no runtime is deployed, so no migration is required and §13's post-genesis append-only/migration clause (point 3) does not apply. Joint backend+frontend sign-off: the user (owner for both sides under R-1), 2026-07-19.
- **v4 (2026-07-17) — B2 02-amendment batch.** One pre-genesis revision carries the queued SQ-2 residuals — SQ-23's intake representation erratum, SQ-24's phase-fraction representation split, and SQ-26's attestor storage freeze in §7.5 — together with SQ-37's §6 conditional-ledger event completion, SQ-43's 12-entry `CohortSummary.proposals` hard-max bound, SQ-55's three trailing `NavView` fields, SQ-125's phase-fraction metadata-exposure mandate, and SQ-138's frozen metadata-constant names, restricted to genuine kernel values per [13](13-parameters.md) reading rule 7; META-amendable registry bounds bind through `params()`. The batch also lands the storage change the B2 implementation needs to make §4's `ParamView.last_change: BlockNumber` faithful: §7.3's `ParamRecord` gains `last_change_block`, because the record previously stored only `last_changed_epoch` and a block number cannot be reconstructed from an epoch index (historical epoch boundaries are not retained). **Pre-genesis revision** — no runtime is deployed, so §13's post-genesis append-only/migration clause (point 3) does not apply. Joint backend+frontend sign-off: the user (owner for both sides under R-1), 2026-07-17, user-delegated batch.
- **v3 (2026-07-15) — oracle per-version reconciliation (A5).** 07 §2(4) runs one reporting game per `(component, epoch, frozen spec_version)`, so §7.2 `Rounds`/`ComponentValues` take the **triple key** `(MetricId, EpochId, MetricSpecVersion)`. Contract v2's pair key was self-contradictory — its own bound note said per-version games "append a `RoundState` per frozen version," which a one-value-per-key map cannot do; the triple resolves it. `RoundState` additionally carries the ack-keying/bond-freezing/§5.5-slashing fields the protocol requires, `ReserveHealth` its probe-timing fields, and `OracleRoundView` (§4) gains `spec_version`. **Pre-genesis revision** — no runtime is deployed, so §13's post-genesis append-only/migration clause (point 3) does not apply; the change is a straight restructure. Joint backend+frontend sign-off: the user (owner for both sides under R-1), 2026-07-15.
- **v2** — the frozen baseline established at D-2 (all FE §30 patch items applied).

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

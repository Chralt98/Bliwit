# Data surface, canonical naming & UI-visible parameter values

> **DERIVED, NON-NORMATIVE.** Refreshed 2026-07-21 from the frozen spec —
> doc 02 (integration contract, frozen, v6) and doc 13 (the single home of parameter values) —
> for upload to Claude Design. Where this file and the spec disagree, the spec wins. All names
> below are CANONICAL: use these exact spellings in UI copy, labels and mock data. Values
> marked [VERIFY] are unresolved in the spec — never invent them.

Block-time basis for human-time conversions: **6 s/block, 14,400 blocks/day** (13, preamble).

## SECTION A — Data surface & naming (doc 02)

### A1. Chain identity a UI shows (02 §8; 13 §3.5)

| Item | Value |
|---|---|
| Address format | SS58 prefix **7777**, checksummed, with identicons |
| paraId | assigned at onboarding; test fixtures use 4242 |
| **USDC** (trading/treasury asset) | bridged USDC; **6 decimals**; `min_balance = 10^4` base units ("1 cent"); lives in `ForeignAssets` keyed by an XCM Location ([VERIFY asset index 1337]) |
| **VIT** (native governance token) | **12 decimals**; total supply 10^9; existential deposit 0.01 VIT |
| Prices / scores | fixed-point, **1e9 scale** at every API/event boundary; quote clamp [0.001, 0.999]; `p_S = 1 − p_L`; gate books map YES ↦ LONG |
| Time | all deadlines are block numbers (`decide_at`, `maturity`, `grace_end`, `challenge_deadline`, `next_boundary`) — the UI computes countdowns from them |
| Contract version | `INTEGRATION_CONTRACT_VERSION = 6`, a runtime constant, echoed in `release.json` |

### A2. What the UI can read and display (02 §3–§4, §7)

Eleven read-only `FutarchyApi` runtime-API methods, callable via the light client; every value
is also recomputable client-side from storage ("an optimization, never a trust root").

**Epoch clock (global header)** — `epoch_status()` → `EpochStatusView`: epoch `index`, current
`phase` — one of **`Intake, Qualify, Seed, Trade, Decide, Review, Execute, Housekeeping`** —
`phase_start_block`, `next_boundary` (countdown target), `dead_man_armed`, `ledger_frozen`,
`phase_flags`. `PhaseFlags` bits: 0 shadow mode · 1 PARAM armed · 2 TREASURY armed · 3
CODE/META armed · 4 **sudo present** (drives the bootstrap banner) · 5 ledger frozen
(PB-LEDGER-FREEZE) · 6 dead-man engaged · 7 reserve-health flag.

**Proposals** — `proposal_summaries()` → up to 32 `ProposalSummaryView`: `id`, `class`,
`state`, `proposer`, `epoch`, `payload_hash`, `ask` (USDC; 0 for non-treasury),
`decision_market` (accept, reject pair), `gate_markets` ([4]: (S,C) × (adopt, reject)),
`decide_at`, `maturity`, `ratification`.
Classes: **`Param, Treasury, Code, Meta, Constitutional`** (no Emergency).
Proposal states: **`Submitted, Screening, Qualified, Trading, Extended, Queued, Suspended,
Rerun, Rejected(RejectReason), Executed, FailedExecuted, Measuring, Settled, Cancelled,
Expired`**.
Ratification badge: `NotRequired` / `NoPassedRecord` / `Passed { referendum }`.
`NoPassedRecord` says only that the execution guard has no passing record; derive never
submitted / ongoing / failed referendum lifecycle from `pallet-referenda`, not this badge.

**Markets & trading** — `quote(market, side, amount)` → `QuoteView`: `cost` (USDC, excl. fee),
`fee`, `p_after_1e9` (post-trade price), `max_trade`, `within_domain`, `evaluable` — an exact
pre-trade preview panel. `evaluable` is true for every successfully computed quote, even when
`within_domain` is false; when `evaluable` is false, every price-like field is non-renderable
and nonactionable. Trade directions: `BuyLong, BuyShort, SellLong, SellShort`. Market kinds:
**`DecisionAccept, DecisionReject, GateS_Adopt, GateS_Reject, GateC_Adopt, GateC_Reject,
Baseline`**. `BaselineMarketOf(epoch)` finds the Baseline book. Price history is fed
exclusively by events: `Traded { market, who, side, amount, cost, p_after }` and
`Observed { market, o_t }` (TWAP observation every 10 blocks); `MarketCreated`/`MarketClosed`
(books freeze at decision close and never reopen)/`MarketReaped` bound chart ranges.

**Finalized decision dashboard (per proposal)** — once the registered decision windows are
sealed and every decision-path input is evaluable, `decision_stats(pid)` returns
`DecisionStatsView`:
`twap_accept_1e9`, `twap_reject_1e9`, `twap_baseline_1e9`, `r_eff_1e9`
(= max(reject, baseline − σ)), `trailing_accept_1e9`, `trailing_reject_1e9`, `coverage_pct`,
`traded_volume`, `v_min_required`, `converged`, `gate_twaps_1e9`, `attack_cost_hat`,
`in_cap_prize` (must be ≤ attack_cost_hat / 3). Before sealing it returns `None`: this is a
finalized decision snapshot, not a live "will it pass?" view, and must not drive projected
uplift or projected PASS/REJECT UI during Trade/Extended.

**Portfolio / ledger** — `account_positions(who)` → up to 64 `PositionView`: `position`,
`balance`, `vault_state`. `PositionId` = `Proposal { proposal, branch, kind }` or
`Baseline { epoch, side }`; `PositionKind` = `BranchUsdc, Long, Short, GateYes(GateType),
GateNo(GateType)`; `Branch` = `Accept | Reject`; `GateType` = `Survival | Security`.
Vault states (drive redeemability badges): **`Open`, `Resolved(Branch)`,
`ScalarSettled { winner, s }`, `Voided`, `BaselineSettled { s }`**. Proposal positions use
`ScalarSettled`; Baseline positions use branch-free `BaselineSettled` and never fabricate a
winning branch. Balances: `System.Account` (VIT) and
`ForeignAssets.Account(USDC_LOCATION, who)` (USDC).

**Execution queue** — `execution_queue()` → up to 32 `QueuedExecutionView`: `pid`, `class`,
`payload_hash`, `maturity`, `grace_end`, `version_constraint`, `cancelled`, `ratification`,
`meters_clear`.

**Welfare / gates** — `welfare_current()` → `WelfareView`: `epoch`, `spec_version`, pillar
values `s_pillar_1e9`, `c_onchain_1e9`, `c_attested_1e9`, `p_pillar_1e9`, `a_pillar_1e9`,
gate values `gate_s_1e9`, `gate_c_1e9`, composite `w_current_1e9`, flags `s_breached`,
`c_breached`, `reserve_flag`, `active_spec_available`. `spec_version` is meaningful only
when `active_spec_available` is true; version zero remains legal when that flag is true.

**Treasury** — `nav()` → `NavView`: `total`, `main`, `pol`, `insurance`, `keeper`, `oracle`,
`rewards`, `stream_remainders`, `obligations`, `haircut_flag` (exactly 08 §1.2
`reserve_impaired`), `spendable_nav`, `meter_utilization_bps`, `class_floors` (Param,
Treasury, Code, Meta order).

**Settlement history** — `recent_cohorts()` → ring of the last **32** `CohortSummary`:
`epoch`, `s_1e9`, `baseline_twap_1e9`, `proposals` (≤ 12 of (id, class, `DecisionOutcome`)),
`voided`, `settled_at`. "A fresh browser renders ~22 months of settlement history with zero
infrastructure dependency." `DecisionOutcome` = `Adopt | Reject(RejectReason) | Extend`.

**Oracle** — `open_oracle_rounds()` → up to 192 `OracleRoundView`: `component`, `epoch`,
`spec_version`, `round` (1..=3), `reporter`, `value_1e9`, `evidence_hash`, `bond`, `challenge_deadline`,
`acked_by_watchtowers`, `escalated`. Component settlement paths: `Unchallenged, Recomputed,
Adjudicated, Neutral`.

**Governance parameters** — `params(keys)` → per key: `value`, `min`, `max`, `max_delta`,
`cooldown_blocks`, `last_change`, `class`, `min_next`, `max_next` — enough to render a full
"constitution browser" with editable range and rate-limit context per parameter.
`min_next`/`max_next` are the exact inclusive next-value interval after intersecting the
record bounds with the max-delta rule; `value`/`min`/`max` remain raw stored scalars.

**Release/upgrade channel** — `ReleaseChannel` fixed-layout raw storage value (readable even
without metadata): current canonical frontend `version` (semver), `manifest_txid` (Arweave),
`release_json_hash`, `updated_at`, `spec_version`, `pending_authorized_at` (pending upgrade
lead-time display), `min_supported_version` (older releases sign only past a blocking
warning), `keyring_generation`, `revoked_key_bits`, `flags` (SECURITY / EXPEDITED /
URGENT_UPGRADE) — the "update available" banner source.

### A3. Canonical naming (02 §5–§7 — UI copy traces to these exact spellings)

**Extrinsics:** `market.buy` / `market.sell` · `ledger.split` / `ledger.merge` /
`ledger.split_scalar` / `ledger.merge_scalar` / `ledger.merge_gate` / `ledger.transfer` /
`ledger.redeem` / `ledger.redeem_scalar` / `ledger.redeem_scalar_pair` / `ledger.redeem_void` ·
`epoch.submit` / `epoch.withdraw` · `oracle.register_reporter` / `oracle.report` /
`oracle.challenge` / `oracle.recompute_proof` · `registry.file_incident` /
`registry.file_milestone` / `registry.challenge` · `execution_guard.execute` /
`execution_guard.ratify(proposal_id, referendum_index)` · `futarchy_treasury.claim_stream` ·
`guardian.approve_action` etc. · `welfare.snapshot(epoch)` ·
`system.apply_authorized_upgrade` · `conviction_voting.vote/delegate/undelegate/remove_vote/
unlock` · `referenda.submit/place_decision_deposit/refund_*` · cranks: `epoch.tick`,
`market.crank_observe`, `market.reap`, `epoch.settle_cohort`, `decide`.

**Events (activity feed / notification vocabulary):**
- ledger: `Split`, `Merged`, `ScalarSplit`, `ScalarMerged`, `PositionTransferred`,
  `VaultResolved { pid, branch }`, `Redeemed`, `ScalarSettlementSet { pid, branch, s }`,
  `ScalarRedeemed`, `ScalarPairRedeemed`, `GateSettled { pid, branch, gate, outcome }`,
  `VaultVoided { pid }`, `VoidRedeemed { pid, kind, amount, payout }`, `VaultReaped`
- market: `Traded`, `Observed`, `MarketCreated`, `MarketClosed`, `MarketReaped`
- epoch: `ProposalSubmitted`, `ProposalWithdrawn`, `ScreeningStarted`,
  `ProposalCancelled { reason }`, `ProposalQualified`, `ProposalDeferred`, `MarketsOpened`,
  `DecisionExtended`, `ProposalQueued { payload_hash, maturity }`,
  `ProposalRejected { reason }`, `ProposalDelayed { justification_hash }`, `RerunScheduled`,
  `RerunOpened`, `MandateExpired`, `MeasurementStarted { cohort }`, `CohortSettled { epoch, s }`,
  `ProposalForceRejected`, `IntakeSlashed { pid, reason, amount }`
- execution guard: `Executed { pid, record }`, `ExecutionFailed { pid, outcome }`,
  `Ratified { pid, referendum_index }`, `UpgradeAuthorized { code_hash, authorized_at }`
- oracle: `ReporterRegistered`, `Reported`, `Challenged`, `RoundEscalated`, `RecomputeProven`,
  `AdjudicationRequested`, `Adjudicated`, `ComponentSettled`, `NeutralSettlement`,
  `WindowAcknowledged`, `WindowExtended`, `QuorumFailed`, `ReporterSlashed`, `ReporterEjected`,
  `WatchtowerRegistered`, `WatchtowerInactive`, `WatchtowerSlashed`, `ReserveProbeSent`,
  `ReserveProbeResult`, `ReserveUnhealthy`, `ReserveRecovered`
- registry: `IncidentFiled`, `MilestoneFiled`, `IncidentChallenged`, `MilestoneChallenged`,
  `IncidentUpheld`, `IncidentRejected`, `MilestoneAccepted`, `MilestoneRejected`,
  `FilingBondSlashed`, `RegistryEpochClosed`
- guardian: `GuardianAction { action_id, power, target, justification_hash }`,
  `ForceRerun { pid, justification_hash, window_end }`,
  `PlaybookActivated { id, trigger, expiry }`, `PlaybookRenewed`, `PlaybookExpired`,
  `ReviewScheduled { action, referendum }`
- system/upgrade: `CodeUpdated`, `UpgradeAuthorized`

Guarantee the feed design can rely on: every terminal proposal/vault/cohort state transition
emits exactly one event — event-derived history is complete by construction.

### A4. Error / rejection taxonomy users can hit

**`RejectReason`** (rendered wherever a proposal is `Rejected(...)`): `NotDecisionGrade`,
`GateVetoSurvival`, `GateVetoSecurity`, `HurdleNotMet`, `ConvergenceFailed`,
`SecondExtensionFailed`, `ProcessHold`, `ConstitutionViolation`, `ResourceConflict`,
`RateLimited`, `VetoUpheldByReview`, `StaleQueue`, `PayloadReverted`, `NotRatified`,
`SecuritySizing`, `AttestationMissing`, `RolloverExhausted`.

**`DispatchOutcomeCode`** on execution records: `Ok` or `Failed { call_index, error }`.
**`IntakeFull`**: intake queue overflow (> 64) refuses submission. **Trade domain**: a trade
pushing the book beyond its price domain is rejected — an evaluable quote with
`QuoteView.within_domain == false` lets the UI disable it beforehand; an unevaluable quote
disables action without rendering price-like fields. **Signing-version warning**: releases older than
`min_supported_version` sign only past a blocking warning. Client-side error codes:
`FE-BOOT-001..004`, `FE-CHAIN-001..005`, `FE-COMPAT-001..002`, `FE-TX-001..007`,
`FE-IDX-001..002`, `FE-REL-001..004`, `FE-PROV-001..004`, `FE-UPG-001` — fixed user copy +
expert detail + documented recovery per code; no free-text errors.

### A5. Mandated frontend behaviors (02 §3, §5–§13)

1. **No hardcoded chain values, ever** — every tunable is read live from the chain
   (constants API or `params()`); shipping a numeric copy is a release-gate failure.
2. Light-client-first reads; runtime-API results cross-checked against storage on tx paths.
3. Chain identity pinned at build, asserted at boot; genesis mismatch is terminal.
4. Charts are event-fed (`Traded` + `Observed`); lifecycle events bound ranges.
5. ≥ 8 browser-reachable WSS bootnodes are the guaranteed dial set.
6. `DescriptorLeadTime` (72 h) drives upgrade banners.
7. On contract regression: fall back to the chain-served layer-1 surface — "reduced depth,
   full correctness; never a trusted third-party service".
8. Fee-currency selector (VIT or USDC) reads `fee.vit_usdc_rate` live.
9. Trading enablement + sudo banner bind to `PhaseFlags`; dead-man and ledger-freeze states
   come from `EpochStatusView`.

## SECTION B — UI-visible parameter values (doc 13)

Defaults are simulation hypotheses unless marked frozen/K (kernel); the real UI reads them
live. For mock data these are the correct realistic values.

### B1. Time: epoch clock & phase schedule (13 §1, §3.1)

| Parameter | Value | UI surface |
|---|---|---|
| `epoch.length` | 302,400 blocks = **21 days** | epoch progress bar; all phase math |
| `epoch.slots` (N_active) | **5** proposals qualified per epoch | intake/qualification screens |
| `epoch.horizon_k` | 2 epochs measurement horizon | proposal timeline |
| Intake | days 0–3 | phase header + countdown |
| Qualify | day 3–4 | phase header |
| Seed | day 4–5 (books deployed) | "markets open at …" |
| Trade | **days 5–18 (13 days)** | market pages, countdowns |
| Decide window | days 15–18 (final 72 h TWAP accrual; trailing = final 24 h) | "decision window live" state |
| Decide | day 18 | proposal countdown target |
| Review | day 18 + per-class timelock | queue ETA |
| Execute | per-proposal maturity within grace | execution queue |
| Housekeeping | days 20–21 (settlement/cleanup) | phase header |
| Capital duration | settles at epoch e+3 ≈ **63–66 days** | position detail |

### B2. Markets, trading, LMSR liquidity (13 §1–§3.2)

| Parameter | Value | UI surface |
|---|---|---|
| `mkt.fee` | 30 bps | fee line in trade ticket |
| `MinTrade` / `MaxTrade` | 1 USDC / b/4 per trade | amount validation, `max_trade` |
| `mkt.obs_interval` | 10 blocks (= 1 min) | chart granularity |
| `mkt.kappa` κ | 0.005 per interval (TWAP slew cap) | "TWAP capped" chart annotation |
| Price domain | quotes clamp to [0.001, 0.999] | price axes never show 0/1 |
| `pol.b` decision books | floors 10k / 25k / 60k / 100k USDC per class (PARAM/TREASURY/CODE/META) | market depth indicator |
| `pol.b_gate` / `pol.b_baseline` | 7,500 / 25,000 USDC | gate & Baseline book pages |
| Maker worst-case loss | b·ln 2 per book | market-info tooltip |
| Sanity band | [0.02, 0.98] on welfare books (gate books exempt) | chart band shading |
| Staleness | price gaps > 50 blocks in decision window ⇒ one 3-day extension, then reject | "market stale" warning |
| `TwapCheckpoints` | 8 per market (chain-served chart fallback) | degraded-mode charts |
| Bounds | ≤ 6 books/proposal · ≤ 4 Baseline books · ≤ 196 live markets · ≤ 32 live proposals | list sizing |

### B3. Decision thresholds (13 §1) — per class PARAM / TREASURY / CODE / META

| Parameter | Value | UI surface |
|---|---|---|
| `dec.window` / `dec.trailing` | 72 h / 24 h | decision-window banner, trailing overlay |
| `dec.extension` | 3 days, at most once | `DecisionExtended` notice + new countdown |
| `dec.delta` δ | 0.0375 / 0.0375 / 0.060 / 0.090 required ACCEPT-over-REJECT margin (V-12 Phase-0 calibrated) | "needs +δ margin" gauge |
| `dec.sigma` σ | 0.003 / 0.005 / 0.008 / 0.010 baseline noise floor | decision-stats panel |
| `dec.delta_max` | 0.05 spot-vs-TWAP convergence bound | "converged" check |
| `dec.coverage` | 95% required observation coverage | coverage meter |
| `dec.v_min` | 100k / 250k / 600k / 1.2M USDC min volume (effective = max(floor, 2·InCapPrize)) | volume progress bar |
| `gate.p_max` | 0.05 gate veto threshold | gate-market red line |
| `welfare.thetaS` lo/hi | 0.90 / 0.98 survival gate | welfare gauges |
| `welfare.thetaC` lo/hi | 0.85 / 0.95 security gate | welfare gauges |
| `welfare.wP` / `wA` | 0.60 / 0.40 pillar weights | welfare composition chart |

### B4. Proposals: bonds, intake, rewards, limits (13 §1–§4)

| Parameter | Value | UI surface |
|---|---|---|
| `prop.bond` | 1k / 5k+0.5%·Ask / 25k / 50k USDC per class | "bond required" in submit form |
| `intake.max_per_account` | 4 per epoch | "3 of 4 submissions used" |
| `intake.slash_fraction` | 10% of bond (to INSURANCE) on non-decision-grade / missing preimage | submission warning; `IntakeSlashed` toast |
| `IntakeQueue` | 64; overflow ⇒ `IntakeFull` | queue page "full" state |
| `trs.proposer_reward` | PARAM 500 USDC; TREASURY/CODE min(0.05%·Ask, 25k); META 25k | "potential reward" |
| Payload limits | ≤ 16 calls / 64 KiB / 25% block weight; nesting ≤ 4 | payload builder validation |

### B5. Execution & upgrades (13 §1, §2)

| Parameter | Value | UI surface |
|---|---|---|
| `exec.timelock` | 2 / 3 / 7 / 14 days per class | queue countdown to maturity |
| `exec.grace` | 14 days | "executable until" |
| `code.spacing` | 30 days between CODE executions | upgrade calendar |
| `DescriptorLeadTime` | 43,200 blocks = 72 h | upgrade banner countdown |
| `StaleEpochBound` | 7 days ⇒ force-reject in-flight | explains `StaleQueue` |
| Dead-man switch | no finalized block ~8 h or snapshot > 4 d overdue ⇒ freeze | `dead_man_armed` banner |
| PB-LEDGER-FREEZE | ≤ 14 days, one renewal | `ledger_frozen` banner |
| Attestation | 25k VIT bond; ≥ 3 attestors; 2-of-N quorum; 72 h challenge window | "2 of 3 attested" progress |
| `grd.bond` / `grd.review_deadline` | 50k VIT / 2 epochs | guardian roster, review countdown |

### B6. Treasury & economics (13 §1, §3.4–§3.5)

| Parameter | Value | UI surface |
|---|---|---|
| `trs.cap_proposal` | 5% of NAV max single ask | ask validation |
| `trs.cap_30d` / `trs.cap_180d` | 10% / 30% of NAV | treasury spend meters (gauges) |
| `trs.stream_threshold` | 1% of NAV — larger payouts stream | "paid as stream" badge |
| `iss.inflation_cap` | 2%/yr, down-only | tokenomics page |
| `fee.vit_usdc_rate` | 1.0 × ref (placeholder 0.05 USDC/VIT [VERIFY]; bounds 0.1×–10×) | fee-currency selector |
| USDC treasury target | ≥ 25M USDC before Phase 5 | treasury progress |
| `keeper.budget_epoch` / `keeper.rebate` | 12,000 USDC/epoch / ≈ 3× fee cost | keeper dashboard |
| `phase3.tvl_cap` / `phase3.deposit_cap` | 2,000,000 / 20,000 USDC (sim-gated [VERIFY]) | deposit caps banner + form limit |

### B7. Oracle, disputes, registry, reserve (13 §1–§3.4)

| Parameter | Value | UI surface |
|---|---|---|
| `orc.window` | 72 h challenge window per round | `challenge_deadline` countdown |
| `orc.rounds` | 3 max | "Round 2 of 3" stepper |
| `orc.bond_floor` / `orc.bond_bps` | 10k USDC / 250 bps; B₁ = max(floor, ceil(bps × stake-at-risk / 10,000)); doubles per round; B₁ and R_max freeze per game at open | bond calculator |
| `orc.reporter_stake` | 100,000 USDC | reporter registration |
| `orc.n_min` | 3 reporters required | "awaiting reporters (2/3)" |
| `wt.quorum` / `wt.stake` / `wt.max` | 2 acks / 25,000 USDC / ≤ 16 seats | watchtower quorum progress |
| Watchtower extension | one +48 h per (component, epoch) | `WindowExtended` notice |
| Oracle settle deadline | epoch(m+1) Housekeeping (d20); unclosed ⇒ **neutral settlement** | "settles neutrally at d20" warning |
| Adjudication track | 60% approval / 10% support / 7-day, pre-cohort conviction snapshot | oracle-dispute referendum page |
| Slashing split | 40% challenger / 60% INSURANCE; reporter −50% on 2nd false report, ejected on 3rd | challenge-reward preview; `offenses` badge |
| `reg.bond_incident` / `reg.bond_milestone` | 5,000 / 2,500 USDC | registry filing forms |
| Reserve probe | daily, 1 h timeout, 0.10 USDC; unhealthy after 2 failures, recovered after 3 passes | reserve-health tile |

### B8. Ledger, positions, redemptions (13 §1–§4)

| Parameter | Value | UI surface |
|---|---|---|
| `ledger.min_split` / `MinTransfer` | 0.01 USDC | split/transfer validation |
| `ledger.position_deposit` | 0.1 USDC per position entry | deposit line in confirmations |
| `MaxPositionsPerAccount` | 64 | "62/64 positions" meter |
| VOID payouts | **cross-branch** Accept+Reject pairs merge at par (100%) — same-branch LONG+SHORT merges pay no USDC and yield one branch-USDC worth `floor(a/2)`; `redeem_void`: unpaired branch-USDC `floor(a/2)`, unpaired LONG/SHORT/gate `floor(a/4)` | VOID redemption dialog |
| Scalar redemption | LONG `floor(a·s)`; unpaired SHORT `floor(a·(1−s))`; pairs via `redeem_scalar_pair` = exactly `a` | redeem payout preview |
| Rounding | charges round up, payouts round down (against the claimant) | payout fine print |
| `RecentCohortSummaries` | ring of 32 (~22 months) | history page range |
| `ExecutionRecords` | ring of 256 | execution history range |

**Unset-by-spec ([VERIFY], sim-/ops-gated — never invent values):** `sec.prize.*`,
`sec.flow_cap`, `collator.bond_req_vit`, `ops.*` budget lines, `pol.b_baseline` calibration,
`fee.vit_usdc_rate_ref` at TGE, `phase3.*` caps before Phase-3 arming.

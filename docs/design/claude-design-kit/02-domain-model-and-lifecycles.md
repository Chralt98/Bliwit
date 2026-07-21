# Domain model & lifecycles — what every screen's objects are

> **DERIVED, NON-NORMATIVE.** Distilled 2026-07-12 (commit `9f250be`) from the frozen spec —
> docs 03 (conditional ledger), 04 (markets & pricing), 05 (welfare & decision engine),
> 06 (governance & guardians), 07 (oracle & disputes), 08 (treasury & economics),
> 09 (execution & upgrades) — for upload to Claude Design. Where this file and the spec
> disagree, the spec wins. All names are canonical spellings; never rename them.

## 1. The money system (03)

- **USDC** is the collateral: all trading, bonds and payouts. **VIT** is for values voting and
  operator bonds.
- `ledger.split` turns `a` USDC into `a` **AcceptUsdc** + `a` **RejectUsdc** (**branch-USDC**):
  conditional USDC that exists "in the ACCEPT world" / "in the REJECT world". A complete pair
  (1 AcceptUsdc + 1 RejectUsdc) always merges back to 1 USDC at par.
- Within a branch, branch-USDC splits further into **LONG/SHORT** scalar pairs on the
  settlement score `s` (LONG pays `s`, SHORT pays `1−s`), and for gated classes into
  **GateYes/GateNo** pairs per gate (`Survival`, `Security`). Per epoch there are also
  Baseline legs **B-LONG/B-SHORT** on the epoch score.
- Each proposal has a **vault** (escrow + instrument home) with a state machine; each account
  holds ≤ 64 position entries, 0.1 USDC refundable deposit per entry, minimum split/transfer
  0.01 USDC.

**Vault states drive which buttons are enabled:**

| State | Meaning | Allowed user ops |
|---|---|---|
| `Open` | minting + trading live | all split/merge families, transfer |
| `Resolved(winner)` | winner branch recorded; losing claims frozen (not burned); no unpaired redemption yet | merge (pairs → par), transfer |
| `ScalarSettled { winner, s }` | terminal; carries winner + settlement score | redemption calls only |
| `Voided` | terminal annulment | `merge` of an Accept+Reject pair at par, `merge_scalar`/gate-merge as value-neutral consolidation (no USDC paid), transfer, `redeem_void` |
| `BaselineSettled { s }` | branch-free position-view projection of a settled Baseline vault | `redeem_baseline`, `redeem_baseline_pair` |

`BaselineSettled` is never a proposal-vault storage state and never carries or displays a
winning branch. Baseline storage has its own `BaselineState`; the shared spelling exists only
so `PositionView.vault_state` can project proposal and Baseline positions through one type.

**Redemption payout matrix** (all payouts floor against the redeemer):

| Vault state | Holding | Call | Payout per unit |
|---|---|---|---|
| `ScalarSettled{w,s}` | winning branch-USDC | `redeem` | 1 |
| `ScalarSettled` | LONG / SHORT of winner | `redeem_scalar` | `floor(a·s)` / `floor(a·(1−s))` |
| `ScalarSettled` | LONG+SHORT pair | `redeem_scalar_pair` | exactly `a` |
| any non-settled | Accept+Reject pair | `merge` | 1 USDC per pair (par) — the **only** 100 % path under VOID |
| any non-settled | same-branch LONG+SHORT or YES+NO set | `merge_scalar` / gate-merge | **no USDC**; mints 1 same-branch branch-USDC, worth `floor(a/2)` under VOID until paired across branches |
| `Voided` | unpaired branch-USDC | `redeem_void` | `floor(a/2)` |
| `Voided` | unpaired LONG/SHORT/gate leg | `redeem_void` | `floor(a/4)` |

Unredeemed claims sweep to INSURANCE after 1 year (`VaultReaped`).

## 2. The epoch machine (05)

21-day default epoch, phases as fractions of length (see kit file 05 for the day schedule):
`Intake` (d0–3, submissions) → `Qualify` (d3–4, screening + slotting, ≤ 5 slots) → `Seed`
(d4–5, vaults + markets created, POL seeded) → `Trade` (d5–18) → `Decide` (d18) → `Review`
(class timelock) → `Execute` (maturity → grace_end) → `Housekeeping` (d20–21, settlement of
cohort e−3, reaping). Several cohorts are always in flight:

```
epoch:      e        e+1      e+2      e+3
cohort e:   trade →  measure  measure  settle   (capital committed ≈ 63–66 days)
```

**Proposal lifecycle** (states the UI badges): `Submitted → Screening → Qualified → Trading
→ [Extended] → Queued → Executed → Measuring → Settled`, with branches to `Cancelled`
(withdrawal/screening fail), `Rejected(RejectReason)` (decide or execute-time fail — then
usually straight into `Measuring` of the REJECT branch: **rejection is the most common,
perfectly healthy path**), `Suspended → Rerun` (guardian delay + re-decision at 2× liquidity
and +1 pp hurdle), `FailedExecuted` (payload reverted; 50% bond slash), `Expired` (grace
missed). Bonds: full refund on withdrawal and every decision-grade outcome; 10% slash on
non-decision-grade/preimage-missing; 100% on constitution violation. Proposer rewards on
execution (PARAM 500 USDC; TREASURY/CODE min(0.05%·Ask, 25k); META 25k).

**Dead-man switch**: if no block finalizes for ~8 h or a welfare snapshot is > 4 days
overdue, the execution queue freezes, decision windows extend day-for-day, the epoch clock
pauses — the UI must tolerate a paused clock.

## 3. Markets & trading (04)

Per proposal: a **decision pair** (ACCEPT-scalar + REJECT-scalar book); every market-bearing
class (PARAM, TREASURY, CODE, META) adds **4 gate books** ((Survival, Security) × (adopt,
reject)); each epoch has one unconditional **Baseline** book. CONSTITUTIONAL proposals have
no markets (referendum path). Typical epoch ≈ 31 live books; max 196.

- **LMSR pricing**: `p_L ∈ [0.001, 0.999]`, `p_S = 1 − p_L`. Decision book price = market's
  estimate of the welfare score conditional on that branch. Gate book price = breach
  probability (healthy gates trade near 0, e.g. 0.011). Baseline = unconditional welfare
  estimate.
- **`buy(market, side, amount, max_cost)`** pays USDC (cost + 30 bps fee, slippage bound
  mandatory) and delivers the target position **plus a "mirror credit"** — `cost` of
  mirror-branch branch-USDC. The portfolio must show this credit: it's what makes a
  losing-branch buyer whole at par (they lose only fees). `sell` mirrors this (auto-merge;
  any unmatched remainder stays as branch-USDC).
- **TWAP**: observations every 10 blocks from the previous block's quote, slew-capped
  (κ = 0.005/interval) — recorded series are smooth by construction. The decision uses the
  final-72 h TWAP + final-24 h trailing TWAP, never spot alone. A price gap > 50 blocks in
  the decision window = "stale event": first extends the decision 3 days (once), second
  rejects.
- Books **freeze at decision close and never reopen** (no post-resolution trading in v1).
- Realistic worked example for mock data (04 §12): ACCEPT-LONG spot 0.560, REJECT-LONG
  0.520; window TWAPs 0.5585 / 0.5210; Baseline TWAP 0.5230; `r_eff` 0.5210; uplift 0.0375 ≥
  δ 0.025 ✓; trailing 0.5570 / 0.5222; convergence 0.0015 ✓; gate TWAPs S 0.011/0.009,
  C 0.017/0.015 ✓ ⇒ **Adopt**.

## 4. The decision rule — what the finalized decision panel shows (05 §5)

`decision_stats(pid)` is available only after the registered windows are sealed and every
decision input is evaluable. It is a finalized snapshot, never a live trading preview; while
it is unavailable, the UI shows no projected uplift or projected PASS/REJECT state.

Kernel-ordered, gates first ("upside is never weighed against ruin"):
1. State/timing valid; no process hold (open oracle dispute, guardian hold, dead-man).
2. **Gate veto**: any gate book with `p̂_adopt > p_max` (0.05) or `p̂_adopt > p̂_reject + ε`
   (0.02) vetoes — regardless of welfare upside.
3. **Decision-grade** per book: coverage ≥ 95% of scheduled observations; no staleness;
   POL floor undisturbed; contest volume ≥ `dec.v_min(class)`; TWAP inside the sanity band
   [0.02, 0.98] (welfare books).
4. **Hurdle**: ACCEPT TWAP ≥ `r_eff` + δ(class), where `r_eff = max(REJECT TWAP,
   Baseline TWAP − σ)`; same on trailing TWAPs; spot-vs-TWAP convergence ≤ 0.05.
5. **Security sizing**: `3 × InCapPrize ≤ AttackCost̂` (else `SecuritySizing` reject).
6. CODE/META: attestation quorum (2-of-3) + values ratification `Passed` (checked at
   execute-time).
Outcome: `Adopt` / `Extend` (once) / `Reject(reason)` — all 17 `RejectReason`s (kit file 05
§A4) need human-readable renderings.

## 5. Welfare metric — the protocol-health dashboard (05 §4)

`W = g(S; 0.90, 0.98) · g(C; 0.85, 0.95) · P^0.60 · A^0.40`, all in [0,1]:
- **S** (survival/liveness): min of block production, relay finality, collator concentration.
- **C** (security-continuity): on-chain part (XCM health, reserve health, economic security,
  weight headroom, runtime integrity, collator adequacy) × attested part (incident score —
  one S1 incident zeroes it).
- **P** (usage): fees burned/paid, qualified users, settled value. **A** (progress): shipped
  audited upgrades, runtime performance, integrations.
- `g` = smoothstep gates with floors/ceilings; **daily gate-breach flags** (`s_breached`,
  `c_breached` + per-day bitmap — display-ready) settle the gate books.
- Settlement score `s = GeoMean(W_{e+1}, W_{e+2})` — one number per cohort; both branches
  settle against it. No token price appears anywhere in W.

## 6. Values layer — six referenda tracks (06 §2)

Standard conviction voting (1×–6×, locks up to 32 weeks). Tracks (deposit / prepare /
decision / confirm / approval / support / enactment):
`metric` (10k VIT, 2d/14d/2d, 60%→50% / 10%→2%, 14 d) · `constitution` (25k, 2d/21d/3d,
67% / 15%→5%, 28 d) · `entrenched` (50k, 7d/28d/7d, 80% / 20%→10%, 4 epochs) · `guardian`
(5k, 1d/7d/1d, 55% / 5%, 2 d) · `ratify` (1k, 1d/7d/1d, 50% / 5%, immediate — the only
deadline is at `execute()`) · `oracle` (5k, 0/7d/1d, 60% / 10%→3%, immediate).
The `oracle` track tally uses a **pre-cohort conviction snapshot**: VIT locked after the
disputed cohort began has zero weight — the UI shows each voter's snapshot-eligible power.

**Ratification** (CODE/META): a `ratify` referendum can be submitted any time after the
artifact hash is committed, runs concurrently with trading/timelock, and is checked only at
`execute()` dispatch. Missing ⇒ proposal stays Queued (retry until `grace_end`), then
`Rejected(NotRatified)` with bond refund.

## 7. Guardians, playbooks, attestors (06 §5–§7)

- **Guardians**: 7 elected accounts, 50k VIT bond each, **5-of-7** approvals dispatch
  atomically; pending actions expire after 3 days. Powers with allowances: `pause_intake`
  (≤ 14 d, 1 per 4 epochs), `delay_once` (one queued proposal, once ever), `force_rerun`
  (1/epoch), `activate_playbook` (only while its on-chain trigger is verifiably active),
  `suspend_on_gate`. Every action auto-schedules a retrospective `ratify` review — failure
  slashes 50% of every approver's bond and schedules recall. Guardians can never move funds,
  enact proposals, or change outcomes.
- **Playbooks** (the only emergency mechanism): `PB-DEPEG` (freeze new-market creation),
  `PB-MIGRATION` (migration recovery), `PB-ORACLE-VOID` (void the cohort), `PB-HALT-INTAKE`,
  `PB-RESERVE` (halt split inflows only; merge/redeem/exit stay open), `PB-LEDGER-FREEZE`
  (all ledger + market calls error `Frozen`; ≤ 14 d + one renewal). Active playbooks show
  trigger + expiry countdown.
- **Attestors**: ≥ 3 bonded members (25k VIT); CODE/META upgrades need a 2-of-N signed
  attestation (reproducible build + kernel invariants preserved), challengeable for 72 h.
  UI: "2 of 3 attested" progress on upgrade proposals.

## 8. Oracle game & registries (07)

Per attested metric component and measurement epoch: report window (2 d) → up to 3 challenge
rounds (72 h each; bonds double per round: B₁, 2B₁, 4B₁; B₁ = max(10k USDC, 2.5% ×
stake-at-risk)) → terminal adjudication on the `oracle` referendum track (7 d). Unchallenged
rounds need a **2-watchtower acknowledgment quorum**; missing quorum grants one 48 h
extension then settles **neutrally** (carry last value, flagged). The **money deadline** is
d20 of the next epoch (`OracleSettleDeadline`): whatever isn't closed settles neutrally;
later verdicts move bonds and reputations only, never settled money. A failed **gate input**
voids the affected cohorts. Roles: **reporters** (100k USDC stake, ≥ 3 required),
**watchtowers** (25k USDC, ≤ 16 seats), challengers (anyone, bonded). `recompute_proof`
resolves deterministic disputes mechanically. Evidence is content-addressed
(`evidence_hash`) — the UI re-hashes fetched evidence before rendering.

**Registries** (bonded claims about off-chain facts, consumed at settlement): IncidentRegistry
(severity S1 = 1.0 / S2 = 0.4 / S3 = 0.1; bond 5k USDC) and MilestoneRegistry (bond 2.5k USDC);
72 h challenge windows, watchtower quorum, per-epoch aggregates.

## 9. Treasury (08)

- **NAV** = liquid USDC + stream remainders owed in − obligations; VIT holdings and in-flight
  XCM marked 0 (conservative). `nav()` exposes `total`, `spendable_nav`, `haircut_flag`
  (exactly `reserve_impaired`), `meter_utilization_bps` + `class_floors` in
  Param/Treasury/Code/Meta order (distance-to-floor rendering; arming below floor
  fails loudly with `NavFloorUnmet`).
- Sub-accounts / budget lines: `MAIN`, `POL` (≤ 0.75% NAV/epoch), `POL_BASELINE`, `KEEPER`
  (12k USDC/epoch), `ORACLE`, `REWARDS`, `INSURANCE`, `ops.*` (bootnodes, RPC/archive,
  keepers, evidence hosting, monitoring, Arweave, collators, coretime).
- **Streams**: linear vesting grants, recipient-claimable (`claim_stream`); mandatory for
  grants > 1% NAV; cancellable by a later TREASURY decision.
- **Meters** (gauges in the UI): per-proposal ≤ 5% NAV; 30-day ≤ 10%; 180-day ≤ 30%; VIT
  issuance ≤ 2%/yr. `SlotsShrunk` (slate reduced to fit the POL budget) must appear on the
  epoch dashboard.
- **Reserve health**: daily XCM probe of USDC transferability; 2 consecutive fails ⇒
  `ReserveUnhealthy`: split inflows halt (PB-RESERVE), NAV shows haircut,
  `spendable_nav = 0`; 3 passes ⇒ `ReserveRecovered`.

## 10. Execution & upgrades (09)

- **Queue**: per passed proposal `{ pid, payload_hash, class, maturity, grace_end,
  version_constraint, ratification, cancelled, meters_clear }`. Anyone may call
  `execute(pid)` inside [maturity, grace_end]; it re-validates a **13-item checklist**
  (window, preimage hash, runtime version, ratification, attestation, capabilities, rate
  meters, resource locks, guardian suspension, gate flags, dead-man/freeze, batch bounds,
  descriptor lead time) — the UI renders all of them as named expected/actual rows.
- **Upgrades**: `Executed` ⇒ `UpgradeAuthorized { hash, authorized_at }` → 72 h
  `DescriptorLeadTime` countdown → anyone submits `system.apply_authorized_upgrade(code)`
  (the FE's upgrade crank fetches the Wasm from the Arweave release artifact and
  hash-verifies before submission). Migration failure ⇒ PB-MIGRATION halt → retry (≤ 2) or
  rollback via the expedited CODE lane (72 h gate market + 3-day fast ratify).
- **`ReleaseChannel`** (raw key readable by stranded apps): canonical release semver,
  manifest TXID, `min_supported_version`, `pending_authorized_at`, SECURITY / EXPEDITED /
  URGENT_UPGRADE flags — drives the "newer release exists" banner.

## 11. Merged glossary (canonical; supplement to kit file 05's naming tables)

**branch-USDC / AcceptUsdc / RejectUsdc** conditional USDC per branch · **complete pair**
Accept+Reject pair (par) · **complete set** LONG+SHORT of one branch · **mirror credit**
the mirror-branch branch-USDC a buyer keeps · **decision pair** the two scalar books per
proposal · **gate books** the four (S,C)×(adopt,reject) veto books · **Baseline** the
epoch's unconditional welfare book · **book** one LMSR market maker with subsidy `b` ·
**settlement score `s`** GeoMean of the two measured epochs' welfare · **decision-grade**
validity checks before prices may decide · **`r_eff`** effective reject floor ·
**hurdle δ / uplift** required/achieved ACCEPT margin · **POL** protocol-owned liquidity
seeding · **headroom** `b·ln 2` worst-case maker subsidy · **VOID / `Voided`** annulment
state (pairs par, unpaired ½/¼) · **rerun** guardian-triggered re-decision (2× POL, +1 pp) ·
**cohort** an epoch's settling proposal set · **Housekeeping** end-of-epoch settlement
window · **keeper / crank / rebate** permissionless maintenance caller / call / refund ·
**INSURANCE** slash-and-residue sink · **watchtower ack** observability co-signature ·
**StakeAtRisk** escrow a report can move · **neutral settlement** carry-last-value-flagged
oracle fallback · **`OracleSettleDeadline`** d20 money deadline · **ProcessHold**
decide-time hold from an open dispute · **NAV / spendable NAV / haircut** treasury solvency
measures · **stream** linear claimable grant · **`InCapPrize` / `AttackCost̂`** flip prize
vs. manipulation-cost estimate · **dead-man** liveness failsafe · **`PhaseFlags`** rollout
phase bitset (sudo banner, trading enablement) · **`DescriptorLeadTime`** 72 h
authorize→apply floor · **expedited CODE lane** freeze-gated fast repair path ·
**preimage pinning** payload bytes locked from qualification · **shrink-to-fit /
`SlotsShrunk`** slate reduction when POL budget is short.

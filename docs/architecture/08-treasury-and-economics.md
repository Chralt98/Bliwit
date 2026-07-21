# 08 — Treasury and Economics

**Status: normative component specification. Supersedes the corresponding sections of BACKEND_PLAN.md/FRONTEND_PLAN.md** (BE §17, §21 economics rows, §27.1–27.4 economics, ADR-14 sizing rule; FE §17.4 fee mechanics).

**Boundary.** This document owns: `pallet-futarchy-treasury` (accounts, NAV, outflow controls, streams, budget lines), genesis economics (VIT supply/allocation/vesting/issuance, initial USDC funding), the minimum-viable-NAV phase gates, the economic-security sizing regime (AttackCost̂ estimator, decide-time cap, Ask-scaled liquidity), transaction-fee economics (`fee.vit_usdc_rate`), keeper economics, intake/bond economics, and the POL seeding flow's economics. It references: ledger mechanics ([03](03-conditional-ledger.md)), book mechanics and `headroom` ([04](04-markets-and-pricing.md)), the decision engine that hosts the `SecuritySizing` step ([05](05-welfare-and-decision-engine.md)), intake/bond lifecycle ([06](06-governance-and-guardians.md)), the reserve-health trigger ([07](07-oracle-and-disputes.md)), rollout phase gates ([09](09-execution-upgrades-and-rollout.md)), and operations funding execution ([12](12-release-and-operations.md)). All parameter values quoted here are normative in [13](13-parameters.md); the arithmetic below is normative in *this* document.

Normative language: RFC 2119. USDC amounts in whole units (6 decimals); `ln 2 = 0.693147…`; all worked arithmetic is shown and MUST be reproduced by the Phase-0 reference model.

---

## 1. `pallet-futarchy-treasury` (carried forward, amended)

### 1.1 Accounts and budget lines

Derived sub-accounts: `MAIN`, `POL`, `INSURANCE`, `KEEPER`, `ORACLE`, `REWARDS`, and (new, D-16) `OPS` — whose budget lines are the lowercase `ops.*` keys used throughout this set (naming normalized with [12](12-release-and-operations.md) §6.1; values consolidated in [13](13-parameters.md)). Outflow calls accept only the `FutarchyTreasury` origin (from the execution guard) and MUST name a budget line; per-line budgets are constitution-keyed.

| Line (account) | Purpose | Per-epoch default *(normative value: [13](13-parameters.md))* |
|---|---|---|
| `POL` | Market subsidy commitments (§8) | ≤ `pol.budget_epoch` = 0.75% NAV (kernel ceiling 1.5%) |
| `POL_BASELINE` | Standing Baseline book subsidy (§4.3) | `pol.b_baseline`·ln 2 ≈ 17,329 USDC/epoch, outside `pol.budget_epoch` |
| `KEEPER` | Metered crank rebates (§6) | `keeper.budget_epoch` = 12,000 USDC |
| `ORACLE` | Reporter fees + escalation float | per-epoch line |
| `REWARDS` | Proposer rewards: PARAM 500; TREASURY/CODE min(0.05%·Ask, 25k); META 25k USDC — paid on `Executed` | — |
| `ops.bootnodes` / `ops.rpc_archive` | ≥8 WSS bootnodes; ≥4 public RPC + ≥2 archive nodes **and the 30-day served-state operator commitment** (D-6, D-16) | funded lines, named operators ([12](12-release-and-operations.md)) |
| `ops.keepers` | Keeper operations **beyond** the metered budget (D-16; §6.3) | funded line |
| `ops.oracle_evidence` | Oracle evidence hosting ([07](07-oracle-and-disputes.md)) | funded line |
| `ops.watchtowers` | ≥ 2 bonded registered watchtowers for the challenge-window quorum ([07](07-oracle-and-disputes.md), [12](12-release-and-operations.md) §6.1) | funded line |
| `ops.monitoring` | §28-equivalent monitoring | funded line |
| `ops.arweave` | Arweave/ArNS permabuy + release hosting (D-16) | funded line |
| `ops.collators` | Collator compensation, `collator.comp_epoch` = 2,000 USDC/collator/epoch (§2.4) | 5 collators ⇒ 10,000 USDC/epoch at launch |
| `ops.coretime` | Coretime renewal budget; the enumerated renewal call is **exempt from the dead-man freeze** (D-9, mechanics in [09](09-execution-upgrades-and-rollout.md)); USDC-denominated — a renewal debits the full DOT outflow's USDC value at `ops.coretime_dot_rate`, rounded up ([09](09-execution-upgrades-and-rollout.md) §4) | funded line |
| `INSURANCE` | Slash proceeds, swept dust, fee share | inflow-only except by TREASURY decision |

**Where per-line budgets bind (normative).** `fund_budget_line` is an allocation act, not a spend: it moves credit from `MAIN` to a line under a passed TREASURY decision, and is deliberately unmetered beyond `MAIN` solvency and the custody sync of §1.4. Each line's per-epoch budget is enforced by its **consuming mechanism**, never at funding time — the keeper meter for `KEEPER` (§6.3), epoch shrink-to-fit against `pol.budget_epoch` · spendable NAV for `POL` (§4.4, whose T13 rerun top-ups are a stated exception to that budget). A line whose consumer is not yet implemented is bounded only by its funded balance and the §1.3 rolling meters; lines carrying a `[VERIFY]`-gated budget in [13](13-parameters.md) §1 are allocation-only until Phase-2/3 ops sizing.

Fee routing (unchanged): 30 bps market fee → 50% `INSURANCE` / 50% POL offset.

### 1.2 NAV (with haircuts and the reserve-health flag)

```
NAV = liquid USDC at par
    + undisbursed stream remainders owed *to* the treasury (cancellation reversions)
    − outstanding obligations (open stream remainders owed *from* the treasury,
      queued in-cap proposal outflows, POL commitments of live books §8.2)
VIT holdings: marked 0. In-flight XCM: marked 0 until arrival (conservative).
```

**Reserve-health haircut (new; B-med “USDC freeze”).** [07](07-oracle-and-disputes.md) defines the deterministic reserve-health sub-metric `R` in `C_onchain`. While the `R` flag is set:

1. the published NAV view carries `reserve_impaired = true` (never silently full-backing);
2. **spendable NAV for all new commitments is 0**: no new POL seeding, no new outflows, no new stream openings, and every minimum-viable-NAV gate of §4 evaluates as failing (fail-static). Existing books and existing stream claims are unaffected. One explicit carve-out (D-9, [09](09-execution-upgrades-and-rollout.md) §4): `execute_coretime_renewal`'s `ops.coretime` line debit remains dispatchable under the flag — renewal is maintenance during exactly the degradations the flag accompanies, and NAV carries no DOT term;
3. playbook `PB-RESERVE` becomes admissible: halts `split` inflows ([03](03-conditional-ledger.md));
4. event `NavHaircutFlagged { epoch, flag }` is emitted on every flag transition. The FE MUST surface the flag on every NAV render ([10](10-frontend-architecture.md), [11](11-frontend-workflows.md)).

`nav()` is the committed [02 §4](02-integration-contract.md) `NavView`: `total` is the NAV above; `spendable_nav` is zero under the reserve-health flag; `meter_utilization_bps` is the rolling-meter utilization; `haircut_flag` is exactly `reserve_impaired`; and `class_floors` carries the §4.1 arming floors in Param/Treasury/Code/Meta order. The remaining fields decompose the treasury accounts, stream remainders and obligations. FE-15 renders the complete view ([11](11-frontend-workflows.md)).

**INSURANCE is outside NAV (normative).** The INSURANCE account is **not** a summand of `NavView.total`: it is not liquid USDC available to the treasury's commitments, and excluding it understates NAV — the fail-static direction for every cap, meter, gate and floor that divides by it. `NavView.insurance` reports the INSURANCE custody balance for transparency only; the account fields are a **partial** view of the treasury's spendable custody — the nine `ops.*` lines carry no `NavView` field of their own — and MUST NOT be presented as an additive decomposition of `total` (a constraint on the FE-15 rendering of [11](11-frontend-workflows.md) §11.8.3). This settles the accounting question only — §1.1's "inflow-only *except by TREASURY decision*" describes a spending path that must exist for the account to be anything other than a sink.

### 1.3 Outflow controls, streams, meters (carried forward)

- Per-proposal outflow ≤ `trs.cap_proposal` = 5% **spendable** NAV (kernel ceiling 10%); rolling ≤ 10%/30 d and ≤ 30%/180 d of the same base (monotone meters, I-7).
- **Enforcement layering (normative).** `trs.cap_proposal` binds at three layers, and the per-proposal guarantee is the *aggregate* one. The decision engine re-derives a proposal's committed outflow from its own call batch and MUST refuse to qualify or to queue any proposal whose **aggregate** derived ask exceeds `trs.cap_proposal` · spendable NAV; a proposal whose outflow is not statically derivable from its batch MUST fail closed at qualification. The treasury re-checks the cap **per outflow call** at execution, and the rolling meters above bound the chain-wide total. The per-call check is a backstop, not the guarantee — so any class that acquires the treasury-spend capability row of [06](06-governance-and-guardians.md) §3.2, **or** whose `InCapPrize` becomes independently valued (§5.2), MUST be brought under the same decide-time aggregate check. A class granted `TreasurySpend` could otherwise carry many individually-under-cap `spend` calls and never meet the aggregate test.
- Streams mandatory for grants > `trs.stream_threshold` = 1% NAV; linear, recipient-claimable, cancellable by a later TREASURY decision; cancellation reverts the undisbursed remainder to `MAIN`.
- Meter contention: execution waits queued and retries within grace.
- `recover_foreign` (assets sent to pallet accounts outside protocol flows): TREASURY-class only, never admin.

### 1.4 Calls (delta over BE §5.2.7)

`spend(line, dest, amount)`, `open_stream(line, recipient, total, start, duration)`, `cancel_stream(id)`, `claim_stream(id)` (Signed recipient), `fund_budget_line(line, amount)`, `recover_foreign(asset, dest, amount)` — all as before, line-scoped. **Custody sync (added 2026-07-17, SQ-123):** for lines backed by a dedicated real-USDC custody pot (`KEEPER` and `ORACLE`, the §6.3 rebate pots derived as `bl/trsry` sub-accounts), `fund_budget_line` is custody-synced — the call atomically transfers `amount` USDC from the `MAIN` custody account to the line's pot and fails as a whole (internal credit rolled back) if `MAIN` holds insufficient real USDC, so funding can never make the internal line ledger claim more than its pot holds; the §6.3 try-state drift alarm (line ≤ pot, per payout line) remains as the backstop against every other drift source. Lines without pots keep custody in `MAIN` (their outflow custody wiring is the A9 fungibles follow-up) (every outflow call names a budget line per §1.1; `open_stream` funds the stream from `line` and reverts its remainder there on cancellation; `recover_foreign`'s `amount` allows partial sweeps). New: `issue_vit(amount, line)` (§2.3, `FutarchyTreasury` origin, issuance-metered); vesting-schedule storage (§2.2). The `execute_coretime_renewal(period_index)` call (permissionless Signed keeper, dead-man-freeze exempt per D-9) is specified in [09](09-execution-upgrades-and-rollout.md) §4; its companions (added 2026-07-18, SQ-245/SQ-246 ruling) are `note_coretime_quote(period_index, price)` (Signed, accepted only from the stored coretime quote authority; freeze-exempt), `prune_coretime_quote(period_index)` (Signed: permissionless once the quote's `ops.coretime_quote_ttl` freshness window has expired, quote-authority-anytime otherwise; freeze-exempt), and `set_coretime_authority(quote_authority, renewal_account)` (`FutarchyTreasury` origin) — semantics in [09](09-execution-upgrades-and-rollout.md) §4. Events: `StreamOpened/Claimed/Cancelled`, `BudgetLineFunded`, `VitIssued`, `NavHaircutFlagged`, `KeeperBudgetLow { remaining: Balance }` (`remaining` is the metered budget left when the event fires — normally at the §6.3 80% crossing, and the whole remaining budget when Low is force-emitted immediately ahead of `KeeperBudgetExhausted`; both keeper events are diagnostic, drive the RB-KEEPER alarm, and are read by no on-chain decision — neither appears in [02](02-integration-contract.md) §6, so both stay treasury-owned and amendable here without an integration-contract change), `KeeperBudgetExhausted { epoch, spent }` (§6.3), `SlotsShrunk` (§4.4, emitted by the decision engine, [05](05-welfare-and-decision-engine.md)), `NavFloorUnmet` (§4.4), plus `Spent`, `ForeignRecovered`, `CoretimeRenewalCalled`, `CoretimeQuoteNoted { period_index, price }`, `CoretimeQuotePruned { period_index }` and `CoretimeAuthoritySet` ([09](09-execution-upgrades-and-rollout.md) §4).

---

## 2. Genesis economics (B-14, D-15)

### 2.1 VIT supply and allocation

Total supply **1,000,000,000 VIT, 12 decimals**, fixed at genesis; existential deposit 0.01 VIT *(identity constants: [02](02-integration-contract.md), values frozen in [13](13-parameters.md))*.

| Allocation | Share | Amount | Vesting / control |
|---|---|---|---|
| Treasury reserve | 30% | 300,000,000 | Held in `MAIN`; **marked 0 in NAV**; disbursable only via TREASURY-class decisions post-Phase-5 |
| Community distribution | 25% | 250,000,000 | Linear vest over 24 months from Phase-4 arming *(schedule simulation-gated)* |
| Founding team | 20% | 200,000,000 | **4-year linear vest, 1-year cliff**, from TGE |
| Ecosystem / ops fund | 15% | 150,000,000 | Feeds the `ops.*` lines; per-epoch budgets; Phase ≤ 4 administered by the ops multisig under the D-13 exposure caps, Phase ≥ 5 TREASURY-class |
| Phase 3–4 incentive programs | 10% | 100,000,000 | Trading/keeper/reporter bootstrap incentives; backstops the reporter loans of §2.5 |

Vesting is enforced on-chain from genesis via SDK **`pallet-vesting`** (stable2606, `=49.0.0`) linear lock schedules on VIT — genesis-configured balances locks, **one schedule per beneficiary with `begin = cliff end`**: nothing is spendable before the cliff — locked VIT cannot even pay transaction fees (beneficiaries pay fees in USDC per §9 until vested) — then the full locked amount unlocks linearly to the end of the vesting horizon. For the founding team that is zero until TGE + 1 year, then linear to TGE + 4 years (the integer per-block unlock floors, so a sub-VIT remainder MAY clear one block after the 4-year mark — rounding is always against the claimant, never ahead of schedule) — everywhere ≤ the idealized `t/4` catch-up-at-cliff curve, i.e. this reading can only unlock *slower* than the alternative, never faster (the conservative direction; a two-schedule catch-up composition is rejected because `pallet-vesting`'s genesis lock is replace-not-accumulate, which would leave the cliff tranche spendable at genesis). Schedules are denominated in para-blocks at the nominal 6 s block time; slower-than-nominal block production delays unlocks and can never accelerate them. *(Resolved at B3: SDK `pallet-vesting` adopted over an in-pallet schedule store — battle-tested lock accounting, genesis-native schedules, permissionless `vest()`; `pallet-futarchy-treasury` keeps only its §1.3 USDC grant streams.)* The community-distribution and incentive allocations, whose schedules cannot start at genesis (Phase-4 arming is not a genesis-known block), are held at genesis in protocol-derived treasury sub-accounts; the community 24-month schedule is created at Phase-4 arming.

### 2.2 Why 30% reserve is consistent with “VIT marked 0 in NAV”

The reserve exists for values-layer continuity (guardian bonds, conviction depth, future issuance-free grants), not solvency; NAV — the solvency and sizing base — remains USDC-only. No VIT price ever enters NAV, W, or any sizing formula (D-18 reflexivity rule, [05](05-welfare-and-decision-engine.md)).

### 2.3 Issuance mechanism (`iss.inflation_cap`)

- Default issuance schedule: **zero**. VIT is minted only by `issue_vit(amount, line)`, dispatched by a TREASURY-class decision, credited only to `REWARDS` or `ops.*` lines (never to arbitrary accounts).
- Rolling 365-day issuance meter (I-7, monotone): Σ minted ≤ `iss.inflation_cap` × supply-at-window-start; `iss.inflation_cap` = **2%/yr, amendable down only** (kernel bound).
- Every mint emits `VitIssued { amount, line, meter_after }`.

### 2.4 Collator compensation

`collator.comp_epoch` = **2,000 USDC per collator per epoch** (PARAM-adjustable, *normative value: [13](13-parameters.md)*), paid from `ops.collators` at epoch Housekeeping to the session's registered collators pro-rata to authored-block share. Launch load: 5 invulnerables ⇒ 10,000 USDC/epoch ≈ 174,000 USDC/yr — 0.7% of the 25M initial treasury per year; sustainable without issuance.

### 2.5 Initial USDC treasury and funding sequence

- HRMP to Asset Hub opens Phase 2 (Paseo) / Phase 3 (Polkadot); initial USDC transferred in before Phase-4 arming (BE §27.4 carried forward).
- **Funding target: ≥ 25,000,000 USDC before Phase-5 (TREASURY) arming** (D-15). Adequacy arithmetic against the §4 floors:
  - Phase-4 (binding PARAM): full 5-slot PARAM epoch needs NAV ≥ 5 × 34,657 / 0.75% ≈ **23,104,906 USDC** — the Phase-4 **slate-capacity target**, distinct from the ≥ 25,000,000 USDC funding target above. The §4.2 arming gate itself is the per-class 1 × PARAM floor of §4.1; this larger figure is what makes a *full* five-slot slate viable inside one epoch's budget.
  - At 25M NAV the per-epoch POL budget is 0.75% × 25M = **187,500 USDC**, which fits a full five-PARAM slate (**5 × 34,657 ≈ 173,287 USDC**) or a mixed 1 CODE + 2 PARAM slate at the same commitment, both ≤ 187,500 ✔ (the Baseline book is funded outside this budget, §4.3).
  - 25M > 13.87M = the one-CODE floor, so Phase-6 arming is reachable without further funding **if** NAV has not decayed; the §4 gate re-checks at arming time regardless.

**Reporter-stake bootstrap (B-15-adjacent sequencing, D-15).** Phase-3 arming requires ≥ 3 registered reporters with full `orc.reporter_stake` = 100,000 USDC stakes. The treasury MAY extend **recallable USDC loans** (per-reporter ≤ 75,000 USDC, line backstopped by the 10% incentive allocation) held directly as reporter stake, never withdrawable by the reporter. The reporter MUST post ≥ 25% (≥ 25,000 USDC) of own capital, and **slashing consumes the reporter's own tranche first** — a loan with no reporter skin would deter nothing. Loans are recallable by TREASURY decision or automatically on reporter exit/ejection. Bootstrap line sizing: 3–5 reporters × 75,000 = 225,000–375,000 USDC.

Welfare cold start (`PriorBounds`, epochs 1–12 winsorization) is specified in [05](05-welfare-and-decision-engine.md) (D-15).

---

## 3. POL commitments per proposal (recomputed per D-10)

Book inventory per class under the reconciled bound of ≤ 6 books/proposal (2 decision + 4 gate; *bounds normative in [13](13-parameters.md)*). The **POL commitment** charged to `pol.budget_epoch` is the worst-case subsidy loss, Σ over the proposal's books of `b·ln 2` (per-book worst-case maker loss, I-12) — deliberately conservative: only the realized branch's books can actually lose, but the budget meter charges both branches.

| Class | Books | Commitment formula | Commitment (USDC) |
|---|---|---|---|
| PARAM | 2 decision + 4 gate | (2 × 10,000 + 4 × 7,500) × ln 2 = 50,000 × ln 2 | **34,657** |
| TREASURY (all ask sizes) | 2 decision + 4 gate | (2 × 25,000 + 4 × 7,500) × ln 2 = 80,000 × ln 2 | **55,452** |
| CODE | 2 decision + 4 gate | (2 × 60,000 + 4 × 7,500) × ln 2 = 150,000 × ln 2 | **103,972** |
| META | 2 decision + 4 gate | (2 × 100,000 + 4 × 7,500) × ln 2 = 230,000 × ln 2 | **159,424** |
| Baseline (per epoch) | 1 | 25,000 × ln 2 (`pol.b_baseline`, §4.3) | **17,329** |

Cash seeding at book creation additionally carries the per-book `headroom` margin ([04](04-markets-and-pricing.md) sizes it); seeding mechanics — one `split`, per-branch `split_scalar` into complete sets — are the per-branch walk of §8 and [03](03-conditional-ledger.md). Committed POL withdraws at settlement; realized cost is the live-branch divergence loss only (≤ half the commitment; the §11.10-equivalent worked example is ≈ 180 USDC for a TREASURY book walked 0.5 → 0.56 — corrected value, frozen in [13](13-parameters.md)).

---

## 4. Minimum-viable NAV and the loud phase gate (B-18, D-15)

### 4.1 Per-class NAV floors

With `pol.budget_epoch` = 0.75% NAV, seeding one proposal of class K requires NAV ≥ commitment(K)/0.0075:

| Gate | Requirement | Floor (USDC) |
|---|---|---|
| 1 × PARAM | (50,000 × ln 2) / 0.0075 | **~4,620,989** |
| 1 × TREASURY (all ask sizes) | 55,452 / 0.0075 | **7,393,600** |
| 1 × CODE | 103,972 / 0.0075 | **13,862,944** (~13.9M — the D-15 “one CODE ⇒ ≥ ~14M”) |
| 1 × META | 159,424 / 0.0075 | **21,256,533** |
| Full 5-slot PARAM epoch | (250,000 × ln 2) / 0.0075 | **~23,104,906** |
| 5 concurrent META (worst slate) | 797,119 / 0.0075 | **106,282,533** (~106M) |

**Rounding and tolerance (normative).** The four class floors are **frozen constants**, not quantities an implementation re-derives at read time: the treasury MUST return exactly the values above. They were computed from the §3 commitments at `pol.budget_epoch` = 0.75%, but they do not share one rounding convention — TREASURY and META divide the whole-USDC-rounded commitment, CODE divides the exact commitment, and PARAM sits ~7.8 USDC above its own exact quotient. What is normative is the **direction** and the literals themselves: every class floor is at or above the exact requirement `commitment(K)/0.0075` — residuals PARAM +7.80, TREASURY +30.07, CODE +0.39, META +19.46 USDC — so the §4.2 gate is never more permissive than the arithmetic demands. Conformance testing MUST reproduce the frozen literals **exactly**; a differential harness that re-derives them MUST case-fit the per-class convention above before comparing, and MAY then allow a ±10 USDC display tolerance. A single re-derivation compared against all four rows satisfies **no** convention — the TREASURY and META residuals alone exceed that tolerance — and MUST NOT be used. The two multi-slot rows are **capacity** figures, not §4.2 arming gates: arming is per class and evaluates the four single-class floors above, while a full slate's viability is governed by §4.4 shrink-to-fit against the live per-epoch budget. Those two rows floor at different stages — 5×PARAM floors the quotient, 5×META the commitment — and both sit marginally *below* their exact requirements (−0.02 and −34.69 USDC); they gate nothing, so the at-or-above direction does not apply to them. The treasury additionally returns the META floor for the market-less `Constitutional` class, defensively — outside this table's closed list ([05](05-welfare-and-decision-engine.md) §5.6). §2.5 uses the five-slot PARAM figure as the Phase-4 *slate-capacity* target for exactly that reason — it is the NAV at which a full PARAM slate fits, not the NAV at which the class arms.

**Freeze and re-derivation (normative).** Because the floors are frozen, they do not track the keys they were derived from. A decision that **lowers `pol.budget_epoch` or raises any `pol.b`** invalidates this table in the *unsafe* direction: the true floor rises above the frozen literal, and §4.2 would then arm a class below its real minimum-viable NAV. Because the floors are compile-time constants, **no governance artifact can move them** — re-deriving on paper does not change what the runtime enforces. Such a decision is therefore safe only when paired with a **CODE** proposal that updates the literals; the `RederiveBudgets` obligation that `pol.budget_epoch`, the four `pol.b` class keys and `pol.b_gate` carry in [13](13-parameters.md) §5 is a screening-time flag for that pairing, not a mechanism that re-derives anything.

### 4.2 The gate rule (normative, loud)

**Arming a proposal class (at a rollout phase gate, [09](09-execution-upgrades-and-rollout.md)) REQUIRES published `spendable NAV` ≥ the class floor of §4.1.** The check is explicit, machine-evaluated, and **loud**: an arming attempt below floor fails with `NavFloorUnmet { class, nav, floor }` (event + extrinsic error), and the `nav()` view exposes `floor(class)` values so the FE can render distance-to-floor continuously. Below ~13.9M NAV the chain **cannot pass its own runtime upgrades** — this fact is now surfaced, never silent. Under the §1.2 reserve-health flag, `spendable NAV = 0` and every gate fails (fail-static).

### 4.3 The Baseline book is funded outside `pol.budget_epoch`

`pol.b_baseline` = **25,000 USDC** (default; **simulation-gated — [VERIFY via Phase-0/3 calibration]**), commitment 17,329 USDC/epoch from the dedicated `POL_BASELINE` line, ≤ 4 concurrently live Baseline books (one per live epoch) ⇒ ≤ 69,315 USDC standing. Rationale: (a) the Baseline TWAP is the reject-leg floor input to **every** decision ([05](05-welfare-and-decision-engine.md)) and must exist even in an epoch with zero qualified proposals, so it MUST NOT compete with proposal subsidies under shrink-to-fit; (b) its manipulation resistance must be at least mid-class, hence the TREASURY-tier `b`. This keeps the Baseline commitment outside the §4.1 proposal-class floor arithmetic. Ledger home and settlement path: [03](03-conditional-ledger.md)/[04](04-markets-and-pricing.md) (B-3).

### 4.4 Slots shrink to fit — with an event

Shrink-to-fit stays: if the epoch's qualified slate's total commitment exceeds the POL budget, slots are dropped in reverse bond-priority order until it fits. Every shrink emits `SlotsShrunk { epoch, requested, funded, dropped: Vec<ProposalId> }` and the affected proposals reject as deferred (bond treatment per [06](06-governance-and-guardians.md)); the FE MUST surface shrink events on the epoch dashboard ([11](11-frontend-workflows.md)). Silent zeroing of upgrade capacity is thereby eliminated: capacity loss is always an event plus a rendered NAV-floor distance.

**Reruns are not budget-charged (normative).** A T13 rerun's additional POL ([05](05-welfare-and-decision-engine.md) §2.1, T13: books reopen at 2× POL) is **not** charged against the current epoch's `pol.budget_epoch`. Shrink-to-fit is evaluated once, over the epoch's qualified slate at Seed entry, and a rerun is by construction not part of any slate qualification. No second meter is required, because the exposure is structurally bounded rather than budget-bounded: `delayed_once` and `rerun` are one-way flags, so each proposal admits at most one rerun and therefore at most one POL doubling ([05](05-welfare-and-decision-engine.md) §2.1, *Rerun finality*), capping an epoch's rerun exposure at 2× its qualified commitment. The resulting commitment still enters treasury NAV netting as a POL obligation (§1.2, §8.2), and the rerun seed fails status-quo (G-1) if `POL` cannot fund it.

---

## 5. Economic-security sizing (B-8, D-4)

### 5.1 What was wrong (the review's arithmetic, restated)

ADR-14's rule `AttackCost ≥ 3·MEV` had no mechanism: `pol.b`, `dec.v_min`, δ were flat per class. With the A-2 flow model (arbitrage flow F ≈ L/2 per day against sustained mispricing), a TREASURY decision pair at defaults has depth L = 2 × 25,000 × ln 2 = **34,657 USDC**, F ≈ **17,329 USDC/day**, so holding a decision-flipping displacement through the 72 h decision window bleeds at most ≈ 3 × 17,329 = **51,986 USDC**. Against the maximum in-cap prize `trs.cap_proposal` × NAV: at the BE §30.2 example NAV (200,000/2.1% = 9,523,810 ⇒ prize 476,190) the required cost 3 × 476,190 = 1,428,571 exceeds 51,986 by **27.5×**; at 100M NAV (prize 5M, required 15M) by **288.6×**. That 27–290× shortfall is what this section closes.

### 5.2 Primary mechanism: the decide-time cap

New decision-engine step (inserted after the convergence check, before the meters check, in [05](05-welfare-and-decision-engine.md); reason code `RejectReason::SecuritySizing`):

```
AttackCost̂(p) = F̂(p) · T_dec                         // USDC
  T_dec  = dec.window / 14,400 blocks-per-day           // = 3 days at default
  F̂(p)   = min( L̂(p)/2 ,  F̂_pub )  per day             // conservative minimum
  L̂(p)   = time-averaged effective POL depth of p's decision pair (2·b·ln 2 as seeded, from I-12 telemetry)
          + min( min(ContestCapital_acc(window), ContestCapital_rej(window))
                 ([04](04-markets-and-pricing.md) §7a: time-weighted marked net open interest;
                  the shallower book is binding — the same per-book measure graded against
                  dec.v_min in step 5; SQ-231 amendment: gross traded notional is manipulable
                  by the attacker's own flow and no longer feeds the certificate),
                 sec.flow_cap · (b_acc + b_rej) )       // the C_hold wash ceiling, now gate-bearing
  F̂_pub  = the published measured arbitrage-flow parameter (A-2 obligation,
            measured Phases 3–4); until published, F̂ = L̂/2.

REQUIRE  InCapPrize(p) ≤ AttackCost̂(p) / 3   else Reject(SecuritySizing)
```

`InCapPrize(p)` — the maximum extractable value of a wrongly flipped decision, per class:

| Class | InCapPrize |
|---|---|
| PARAM | certified capability-envelope value of the parameter delta ([05](05-welfare-and-decision-engine.md)) |
| TREASURY | `ask` (already ≤ `trs.cap_proposal`·NAV by the outflow cap) |
| CODE / META | max(`ask`, envelope), conservatively floored at `trs.cap_proposal`·NAV for runtime-upgrade payloads — an upgrade is assumed able to reach the full per-proposal outflow cap |

Every TREASURY proposal also undergoes the four gate-book veto checks of [05](05-welfare-and-decision-engine.md) §5, regardless of whether its ask is above or below `trs.stream_threshold`; that threshold continues to govern payout streaming only (§1.3), not gate eligibility.

NAV in this computation is `spendable NAV` (§1.2): under the reserve-health flag it is 0 and — consistently — no new adoption passes sizing. All inputs are decide-time on-chain measurements; the cap therefore **scales with the value at stake by construction**, which the flat defaults never did.

The inner pair reduction is normatively **MIN**, never SUM: an attacker can flip through the cheaper, shallower decision book, so counting the deeper book would overstate security. §5.4(b) is the arithmetic lock: both books must individually clear `dec.v_min = 400,000`, yet the worked `L̂` adds one 400,000 term (`34,657 + 400,000 = 434,657`), not 800,000.

### 5.3 Secondary mechanism: Ask-scaled liquidity (floors = current defaults)

Piecewise-linear per class in `P = InCapPrize(p)`, with `P_ref(class) = AttackCost̂_default(class)/3` (the largest prize default depth supports; see §5.4):

```
dec.v_min(class, P) = max( v_min_floor(class), 2·P )
pol.b(class, P)     = b_floor(class) · max(1, P / P_ref(class))
δ(class, P)         = min( δ_floor(class) · max(1, P / P_ref(class)) , 0.10 )   // hard kernel cap
```

Floors are the current defaults (*normative values: [13](13-parameters.md)*); the `pol.b` and δ slopes are **simulation-gated [VERIFY in Phase-0 calibration]** — the kernel guarantee below rests on the `v_min` term alone, so slope tuning cannot weaken it.

**Why `v_min = 2·P` closes the rule identically.** If the proposal is decision-grade, measured **contest capital** ([04](04-markets-and-pricing.md) §7a) ≥ `dec.v_min` ≥ 2P, and the `sec.flow_cap` ceiling does not bind at exactly-grade organic depth (next paragraph), so:

```
AttackCost̂ = 1.5 · L̂ ≥ 1.5 · (2·b·ln 2 + 2P) = 3P + 3·b·ln 2  >  3P   ∎
```

i.e., every decision-grade, sizing-passing adoption satisfies `AttackCost̂ ≥ 3·InCapPrize` with a margin of `3·b·ln 2` that itself grows under the `pol.b` scaling. Proposals that cannot attract **held** depth 2× their prize are rejected `SecuritySizing` — status-quo default, exactly the intended failure mode. Since the SQ-231 amendment the 2P term is capital genuinely at risk through the window: supplying it as an attacker means holding net exposure the displacement-and-hold theory (§5.5, `C_hold`) already prices, so the certificate can no longer be self-funded by churn.

**Ceiling non-bindingness (kernel-checked at the consuming engine).** The gate ceiling `sec.flow_cap · (b_acc + b_rej)` must not reject honest exactly-grade proposals: under the **normative `pol.b` seeding of this section** (`b = b_floor · max(1, P/P_ref)`), the binding ratio is `2P / (b_acc + b_rej) = P/b` — for `P ≤ P_ref` it is at most `P_ref/b_floor`, and for `P > P_ref` the scaling holds it constant at exactly `P_ref/b_floor` ≤ 6.7 across the §5.4 defaults table (PARAM/TREASURY/CODE 5.7, META 6.7). Any `sec.flow_cap ≥ 7` therefore leaves the identity intact; **7 is the row's hard minimum** (*normative bound: [13](13-parameters.md)*), and the Phase-0-calibrated value (sim-gated) sits above it. A book seeded at floor `b` while `v_min` carries the `2P` scaling (the §5.4(b) illustration as printed) is **not a configuration the normative seeding produces** — there the ratio can reach 8, which is why the illustration below also records its scaled-seeding form.

### 5.4 Worked recomputation at defaults (normative)

**(a) Maximum in-cap prize, CODE at its NAV floor.** NAV = 13,862,944 (§4.1) ⇒ P = 5% × NAV = **693,147 USDC**.
- Scaled `dec.v_min` = max(600,000, 2 × 693,147) = **1,386,294 USDC**.
- POL depth = 2 × 60,000 × ln 2 = **83,178 USDC**.
- L̂ (at exactly-grade volume) = 83,178 + 1,386,294 = **1,469,472** ⇒ AttackCost̂ = 1.5 × 1,469,472 = **2,204,208 USDC**.
- Requirement 3P = 2,079,441 ≤ 2,204,208 ✔ — holds with margin 124,767 USDC (= 1.5 × 83,178 = 6.0%). Cap check: P = 693,147 ≤ AttackCost̂/3 = 734,736 ✔.

**(b) The §30.2-equivalent TREASURY example.** Ask 200,000 at NAV 9,523,810:
- `dec.v_min` = max(250,000, 400,000) = **400,000**; at floor depth (a conservative illustration that under-states §5.3's own `pol.b` scaling) L̂ = 34,657 + 400,000 = 434,657; AttackCost̂ = **651,986**.
- 3P = 600,000 ≤ 651,986 ✔ (margin 8.7%). Under the old flat defaults this identical proposal had AttackCost ≈ 51,986 vs required 600,000 — an 11.5× shortfall, now closed.
- **Normative-seeding form (SQ-231 consistency note):** §5.3 scales `b = 25,000 · 200,000/142,329 ≈ 35,130` here, so L̂ = 48,700 + 400,000 = 448,700 and AttackCost̂ = 673,050 (margin 12.2%); the `sec.flow_cap` ceiling at its ×7 minimum is 7 · 70,260 = 491,820 ≥ 400,000 — not binding, per §5.3's non-bindingness bound. The floor-depth arithmetic above is kept as the conservative lower bound the identity already clears.

**(c) PARAM at flat defaults (scaling not binding).** L̂ = 2 × 10,000 × ln 2 + 100,000 = 113,863; AttackCost̂ = 170,794; max passable envelope value = **56,931 USDC**. A PARAM delta whose certified envelope exceeds this must either attract more organic volume or fail sizing. The four PARAM gate books are separate veto inputs and their POL depth is deliberately excluded from `L̂`, exactly as for every other gated class.

**Defaults table `P_ref(class)`** (derived, frozen in [13](13-parameters.md) as derived values): PARAM 56,931 (= 1.5·(13,863 + 100,000)/3); TREASURY 142,329 (= 1.5·(34,657 + 250,000)/3); CODE 341,589 (= 1.5·(83,178 + 600,000)/3); META 669,315 (= 1.5·(138,629 + 1,200,000)/3). Every class term is the **decision-pair** seeded depth `2·b·ln 2` at the [13](13-parameters.md) `pol.b` floors (10k/25k/60k/100k) plus the `dec.v_min` floor — gate-book depth is deliberately excluded from L̂ (§5.2 measures the decision pair only). The superseded PARAM decision-depth cells (27,726 / 63,863) were a doubling slip: the decision pair still uses `2·b·ln 2`; §3's larger total PARAM commitment now adds four distinct `pol.b_gate` books and does not alter this security-depth term.

### 5.5 Honesty clause

`AttackCost̂` is an *upper bound* estimate of the manipulation bleed (F̂·T bounds absorbed adverse flow, not realized loss per unit). The SF = 3 divisor, the conservative `min(·, F̂_pub)`, the requirement to hold displacement through full **and** trailing windows with convergence ([05](05-welfare-and-decision-engine.md)), the `v_min` identity of §5.3, and — since the SQ-231 amendment — the manipulation-resistant contest-capital input with its `sec.flow_cap` ceiling are the compensating margins. Because the gate is an upper bound, the engine also emits the finer *lower-bound* diagnostic **`ManipFloor̂ = C_disp + C_hold`** per decision ([05 §5.6](05-welfare-and-decision-engine.md)); it never gates in v1, but its published series is part of the same calibration obligation as F̂ — if `ManipFloor̂` persistently reads below `3·InCapPrize` for adopted proposals, δ and/or the `dec.v_min`/`pol.b` slopes MUST be tightened before caps rise. A-2 remains an **empirical** assumption: F̂ MUST be measured in Phases 3–4 and published before caps rise; deep-pocketed off-system attackers remain the residual (TM-18, [14](14-threat-model.md)). The Phase-0 exit simulation ([15](15-invariants-and-testing.md) §4.9) validates the `ManipFloor̂`↔`AttackCost̂` envelope at that irreducible line: it scores a causal wrong-PASS flip as a failure only when the *realized* attacker cost is **below the prize** (profitable capture); a flip whose realized cost stays ≥ the prize but below `3·InCapPrize` (e.g. thin-market/gate-suppression griefing) is the TM-18 residual the SF = 3 margin guards against, recorded as a diagnostic.

---

## 6. Keeper economics (B-med, recomputed)

### 6.1 Crank volume (derivation)

Concurrently trading books = 5 slots × 6 + 1 Baseline = **31** (forecast trading is cut, D-8, so measuring cohorts' books are closed; *bounds: [13](13-parameters.md)*). Observation grid = every 10 blocks:

- **Decision-critical** (72 h decision window, 43,200 blocks): 43,200/10 = 4,320 obs/book × 31 = **133,920**, plus decide/tick/settle/snapshot cranks (order 10²) ⇒ **≥ 134k decision-critical cranks/epoch**.
- **Full trading window** (d5–d18, 187,200 blocks): 18,720 obs/book × 31 = **580,320 ≈ 580k cranks/epoch**.

(On-trade updates advance the grid for free, so these are worst-case zero-organic-trade figures.)

### 6.2 Budget sizing

`keeper.budget_epoch` = **12,000 USDC** (raised from 3,000; *normative value: [13](13-parameters.md)*). Derivation: assumed crank fee ≈ 0.03 USDC **[VERIFY against benchmarked weights + `fee.vit_usdc_rate` at launch]**, `keeper.rebate` ≈ 3× fee ⇒ 133,920 × 0.09 ≈ **12,053 USDC** — the budget covers the full decision-critical load at the 3× profitability multiple (keeper gross margin ≈ 8,000 USDC/epoch over fees paid). The old 3,000 budget covered <25% of decision-critical volume — rational keepers would have stopped mid-window and every decision would have rejected `NotDecisionGrade`.

### 6.3 Meter structure and exhaustion behavior

- Two tranches: **≥ 80% reserved for decision-critical cranks** (decision-window observations, `decide`, `settle_cohort`, `tick`, `snapshot`); ≤ 20% general (out-of-window observations, reaping).
- **The decision-critical list is closed (normative).** Exactly the five crank families named above draw on the ≥ 80% reservation. Every other sanctioned permissionless keeper crank draws on the ≤ 20% general tranche — including `execution_guard.execute`, `expire_failed_execution` and `reject_stale`, `welfare.record_daily_gate`, `futarchy_treasury.execute_coretime_renewal` and `prune_coretime_quote`, and all reap/dust sweeps. Cranks funded from the `ORACLE` line ([07](07-oracle-and-disputes.md)) sit **outside this meter entirely** and consume neither tranche. Privileged cleanup entry points that are not permissionless keeper surfaces are unrebated. The general tranche is a **partial subsidy by construction** — full-window observation demand exceeds it by roughly an order of magnitude (see the `ops.keepers` bullet below) — so no general-tranche crank may be assumed rebated; `ops.keepers` is its continuity path.
- 80% consumption ⇒ `KeeperBudgetLow`; 100% ⇒ `KeeperBudgetExhausted { epoch, spent }` + RB-KEEPER ops alarm. Cranks remain permissionless and idempotent after exhaustion — rebates stop, nothing else changes.
- **Exhaustion is effective, and it latches (normative).** A literal 100% trigger would almost never fire, since the budget is rarely an exact multiple of `keeper.rebate`. `KeeperBudgetExhausted { epoch, spent }` therefore fires on the **first rebate attempt that does not fit the remaining budget, or on one that exactly exhausts it**, reporting the amount actually spent — 0 in the degenerate case where a single rebate exceeds the whole budget, and the full budget on the exact-fit path, where that final rebate is still paid. `KeeperBudgetLow` always precedes it, and both latch once per epoch. Exhaustion is a **payment latch, not merely an alarm**: once it fires, no further metered rebate is paid for the remainder of that epoch, even if a subsequent `keeper.rebate` or `keeper.budget` amendment would create headroom. Rebates resume at the next epoch boundary. The latch is not a liveness trap — the rebate is an infallible post-effect that never alters its crank, so every crank stays permissionless, idempotent and dispatchable at ordinary transaction fee throughout.
- Beyond-meter continuity is a funded ops line (`ops.keepers`, §1.1): the ≥ 2 committed keeper operators of the node-roles table run through exhaustion. Full-window coverage at 3× rebate would cost ≈ 580,320 × 0.09 ≈ 52,229 USDC/epoch — deliberately **not** metered: out-of-window observation gaps only degrade chart density, never decisions (staleness counts only inside the decision window, [04](04-markets-and-pricing.md)).

**A-1 restated:** at least one rational, funded keeper exists; if none does, the chain adopts nothing — safe but stagnant. The 12,000 USDC meter plus the ops line is what makes A-1 *economically reasonable* rather than aspirational; it still cannot make it a code-enforced guarantee.

---

## 7. Intake and bond economics (B-13 economic side, slot monopolization)

New rules (lifecycle owned by [06](06-governance-and-guardians.md); economics here):

1. **10% bond slash — routed to the INSURANCE account, not burned** (USDC is bridged Asset-Hub USDC; burning it would strand backing reserve on Asset Hub — [06](06-governance-and-guardians.md) §4) — on preimage-missing cancellation and on every non-decision-grade outcome; bonds refund in full only on a decision-grade outcome (adopt or reject — rejection is information).
2. **`request_preimage` pinning at qualification** (hygiene half of B-13, in [06](06-governance-and-guardians.md)).
3. **≤ 4 intake entries per account per epoch** (`intake.max_per_account`).

**Cost of the griefing strategies (before → after).** Before: full slot capture + intake denial locked ≈ 109k USDC of *fully refundable* bonds ≈ **$314/epoch** of time-value (5%/yr × 21 d); pure intake denial ≈ $92/epoch.

After, per epoch, at USDC ≈ $1:

| Strategy | Locked | Slashed/epoch (to INSURANCE) | Notes |
|---|---|---|---|
| Intake denial (64 × PARAM bond 1,000, preimage-missing) | 64,000 | **6,400** (10%) | needs ≥ 16 funded accounts (64 ÷ 4-per-account limit) — ~70× the old $92 |
| Slot capture (5 × ≥ TREASURY/CODE bonds ≥ 25,000, ride to non-decision-grade) | ≥ 125,000 | **≥ 12,500** | bond-priority means matching honest class bonds, not minima |
| Combined monopolization | ≈ 189,000 | **≈ 18,900** | ≈ 60× the old all-in $314 |
| “Refund path”: make the junk decision-grade instead | — | ≥ ≈ 18,000 in fees alone | must self-supply `dec.v_min` **contest capital** ([04](04-markets-and-pricing.md) §7a) — *held* net exposure carried through the window, not churn: since the SQ-231 amendment a round trip nets out of `noi_t` by construction, so the 3M is capital the attacker must hold, not turnover it may recycle (5 × CODE = 3M held × 2 × 30 bps entry/exit) — plus the `C_hold` adverse-selection bleed on that held position, plus divergence loss to the POL books, plus 63–66-day scalar capital duration and market risk |

Monopolization is no longer pocket change: every path costs **five figures per epoch, unrecoverable by the attacker** (and INSURANCE-accretive), versus ~$300 of time-value before. Threat row: [14](14-threat-model.md).

---

## 8. POL seeding flow (per-branch, cap-exempt)

Consistent with the [03](03-conditional-ledger.md) per-branch walk (B-4 fix):

1. `POL` account calls `split(pid, c)` — escrow += c; per-branch supplies `supply(AcceptUsdc) += c`, `supply(RejectUsdc) += c` (dual mint; the mirror is free by construction, so seeding is decision-neutral).
2. Per branch b: `split_scalar(pid, b, c_b)` converts branch-USDC into complete LONG_b+SHORT_b sets held by the book account; gate books receive their per-branch YES/NO complete sets analogously (`GateYes/GateNo` kinds, [03](03-conditional-ledger.md)).
3. Each book's inventory = `b·ln 2 + headroom` of complete sets ([04](04-markets-and-pricing.md) sizes `headroom`); the per-branch identity `escrowed == supply(bUSDC_b) + Q_b` holds at every step — no counter underflows on this flow (the B-4 defect).
4. Book revenue is immediately re-split into complete sets (D-3 revenue recycling), so book solvency is structural, not budgetary.
5. At settlement, POL withdraws; realized subsidy = live-branch divergence loss; `POL` offset receives its 50% fee share.

**Protocol-account exemptions:** the `POL`/book/treasury sub-accounts are exempt from `MaxPositionsPerAccount` = 64 and from the 0.1 USDC per-entry Positions deposit (*bounds: [13](13-parameters.md)*) — a decision pair + 4 gate books across two branches materially exceeds a user cap, and the deposit would be the treasury paying itself. Exemption is by account-list membership in `pallet-constitution`, not by any admin toggle.

---

## 9. Transaction fees (X-14, D-12)

- `pallet-transaction-payment` computes the fee in VIT; `pallet-asset-tx-payment` charges USDC-electing users `fee_usdc = ceil(fee_gov × fee.vit_usdc_rate)`, minimum 1 base unit.
- **`fee.vit_usdc_rate`** (USDC per VIT) is a typed constitution key: bounds **[0.1×, 10×] of the genesis reference** `fee.vit_usdc_rate_ref` (a kernel constant fixed at genesis from the launch reference price — **[VERIFY at TGE pricing; placeholder reference 0.05 USDC/VIT]**), PARAM-adjustable, max Δ ×2, cooldown 1 epoch (*normative row: [13](13-parameters.md)*).
- **USDC-only users are always viable, end-to-end**: the inbound reserve transfer's execution on this chain is paid via the XCM `WeightTrader` selling execution for USDC or DOT; every subsequent local extrinsic — including the outbound `reserve_transfer` exit — is payable in USDC via the rate above. No VIT balance is ever a precondition for any user workflow. The FE fee-currency selector binds to this key ([11](11-frontend-workflows.md)); the guided funding flow is [11](11-frontend-workflows.md)'s D-12 surface.
- Rate-staleness failure mode: if the rate drifts outside honesty (VIT repricing faster than PARAM cadence), the bounded [0.1×, 10×] envelope caps the damage to a 10× fee mispricing in either direction — annoying, never disabling; guardian playbooks are not needed for fee drift.

---

## 10. Resolves

| Finding | Resolution in this document |
|---|---|
| B-8 (with [05](05-welfare-and-decision-engine.md)) | §5: decide-time `InCapPrize ≤ AttackCost̂/3` cap from measured depth + Ask-scaled `v_min`/`pol.b`/δ with the `v_min = 2P` identity; worked arithmetic shows the 27–290× shortfall closed at defaults |
| B-13 economic side (with [06](06-governance-and-guardians.md)) | §7: 10% slashes (to INSURANCE) + per-account rate limit priced out — griefing now costs five figures/epoch forfeited vs ~$314 time-value |
| B-14 / D-15 | §2: VIT 1B/12-dec allocation + vesting + zero-default 2%-capped issuance; ≥ 25M USDC target with adequacy arithmetic; collator comp 2,000; reporter bootstrap loans (recallable, skin-first slashing) |
| B-18 / D-15 | §3–§4: recomputed commitments (34,657 / 55,452 / 103,972 / 159,424 for PARAM/TREASURY/CODE/META; 17,329 Baseline), per-class NAV floors, loud `NavFloorUnmet` arming gate, `SlotsShrunk` event + FE surface, Baseline funded off-budget |
| X-14 / D-12 | §9: `fee.vit_usdc_rate` key, bounds, USDC-only viability incl. the on-ramp |
| B-med keeper budget | §6: ≥ 134k/580k crank recomputation, 12,000 USDC budget derivation, tranches, exhaustion alarms, A-1 restated |
| B-med USDC freeze (with [07](07-oracle-and-disputes.md), [10](10-frontend-architecture.md)) | §1.2: reserve-health haircut flag in `nav()`, spendable-NAV = 0 fail-static, PB-RESERVE hook, FE surfacing |
| X-13 partial / D-16 (with [12](12-release-and-operations.md)) | §1.1: named, funded `ops.*` budget lines incl. 30-day operator window, beyond-meter keeper subsidy, ArNS permabuy, coretime line (dead-man exempt per D-9) |

# 13 — Parameters, Bounds and Constants (single reconciled table)

**Status: normative component specification. Supersedes the corresponding sections of BACKEND_PLAN.md/FRONTEND_PLAN.md** (BE §21 in full; every bound, constant or default restated anywhere else in either source).

**Boundary.** This document is the **single source of truth for every tunable parameter, kernel constant, storage bound and frozen protocol constant**. Other component documents reference these values and MUST NOT restate different ones (quoting for readability is permitted when marked *(normative value: §13)*). Chain-identity constants and the integration surface are jointly owned with [02](02-integration-contract.md) (D-17); where both documents state a value they are byte-identical by construction of the D-2 freeze.

Normative language: RFC 2119. Blocks at 6 s (14,400/day); USDC 6 decimals, VIT 12 decimals; `Fixed` = `FixedU64` (1e9 scale) at API boundaries.

**Reading rules (X-11e/X-11h — no hardcoding, anywhere):**

1. **K** = kernel constant: compile-time in `futarchy-primitives`, no storage representation, changeable only by a Wasm upgrade that the attestation regime surfaces ([09](09-execution-upgrades-and-rollout.md)).
2. Everything not marked K lives in `pallet-constitution::Params` as a typed record `{ value, min, max, max_delta_per_decision, cooldown, class }`; min/max/max-delta/cooldown/class are genesis-fixed for kernel-bounded keys and META-amendable within compile-time meta-bounds otherwise.
3. **Every value in this document — K constants included — is machine-readable by clients** via the runtime **constants API** (metadata) or the named storage item, per the [02](02-integration-contract.md) contract. The frontend MUST read, never hardcode (this closes X-11h's backend side: there is no value the FE re-checks that the chain does not expose).
4. Every default is a **simulation hypothesis** unless marked frozen; Phase 0–3 calibration obligations are tagged *sim-gated*.
5. `ProposalClass::Emergency` is deleted (D-7): no row in this document carries an EMERGENCY value, and none may be added without reopening D-7.

---

## 1. Constitution keys (typed, bounded, rate-limited)

Per-class value lists are ordered **PARAM / TREASURY / CODE / META** unless stated. "vf" = values floor applies.

| Key | Type | Unit | Default | Hard min | Hard max | Max Δ/decision | Cooldown | Class | Doc |
|---|---|---|---|---|---|---|---|---|---|
| `epoch.length` | u32 | blocks | 302,400 (21 d) | 201,600 (14 d floor) | 604,800 | 10% | 2 epochs | META | [05](05-welfare-and-decision-engine.md) |
| `epoch.slots` (N_active) | u8 | — | 5 | 1 | 12 | 2 | 1 | META | [05](05-welfare-and-decision-engine.md) (§3 cross-check) |
| `epoch.horizon_k` | u8 | epochs | 2 | 1 | 4 | 1 | 4 | META+values | [05](05-welfare-and-decision-engine.md) |
| `mkt.obs_interval` | u32 | blocks | 10 | 5 | 50 | 5 | 1 | PARAM | [04](04-markets-and-pricing.md) (§3 cross-check) |
| `mkt.kappa` κ | Fixed | /interval | 0.005 | 0.001 | 0.02 | 0.002 | 2 | META | [04](04-markets-and-pricing.md) |
| `mkt.fee` | Perbill | bps | 30 | 5 | 100 | 10 | 1 | PARAM | [04](04-markets-and-pricing.md) |
| `dec.window` | u32 | blocks | 43,200 (72 h) | 14,400 | 86,400 | 20% | 2 | META (vf) | [05](05-welfare-and-decision-engine.md) |
| `dec.trailing` | u32 | blocks | 14,400 (24 h) | 3,600 | 28,800 | — | 2 | META | [05](05-welfare-and-decision-engine.md) |
| `dec.delta` δ per class | Fixed | s-units | 0.015 / 0.025 / 0.040 / 0.060 — **floors of the Ask-scaled schedule, [08](08-treasury-and-economics.md) §5.3** | 0.005 | 0.10 (K cap incl. scaling) | 0.005 | 2 | META (vf) | [05](05-welfare-and-decision-engine.md), [08](08-treasury-and-economics.md) |
| `dec.sigma` σ per class | Fixed | s-units | 0.003 / 0.005 / 0.008 / 0.010 | 0 | δ/2 | — | 2 | META | [05](05-welfare-and-decision-engine.md) |
| `dec.delta_max` (convergence) | Fixed | — | 0.05 | 0.02 | 0.10 | — | 2 | META | [05](05-welfare-and-decision-engine.md) |
| `dec.coverage` | Percent | % | 95 | 90 | 99 | — | 2 | META (vf) | [05](05-welfare-and-decision-engine.md) |
| `dec.v_min` per class | Balance | USDC | 100k / 250k / 600k / 1.2M — **floors; effective value = max(floor, 2·InCapPrize), [08](08-treasury-and-economics.md) §5.3** | ×0.1 | ×10 (floors only; the 2·P term is K) | ×2 | 2 | META (vf) | [05](05-welfare-and-decision-engine.md), [08](08-treasury-and-economics.md) |
| `gate.p_max` (S, C) | Fixed | prob | 0.05 | — | **0.10 K ceiling** | 0.01 | 4 | META+values | [05](05-welfare-and-decision-engine.md) |
| `gate.v_min` (gate-book contest floor, per book) | Balance | USDC | **0.1 × `dec.v_min`(class)** | ×0.05 | ×0.5 | ×2 | 2 | META | [05](05-welfare-and-decision-engine.md) §5.2 |
| `gate.nb_coverage` (GB-NB near-boundary coverage) | Percent | % | 98 | 95 | 100 | — | 2 | META | [05](05-welfare-and-decision-engine.md) §5.2 |
| `gate.nb_conv` (GB-NB spot-vs-TWAP bound) | Fixed | — | 0.01 | 0.005 | 0.02 | — | 2 | META | [05](05-welfare-and-decision-engine.md) §5.2 |
| `gate.eps` ε | Fixed | prob | 0.02 | 0.005 | p_max/2 | — | 2 | META | [05](05-welfare-and-decision-engine.md) |
| `welfare.thetaS` lo/hi | Fixed | — | 0.90 / 0.98 | **lo floor K-entrenched** | — | 0.01 | 4 | CONST (loosen: entrenched) | [05](05-welfare-and-decision-engine.md) |
| `welfare.thetaC` lo/hi | Fixed | — | 0.85 / 0.95 | lo floor K | — | 0.01 | 4 | CONST | [05](05-welfare-and-decision-engine.md) |
| `welfare.wP/wA` | Fixed | — | 0.60 / 0.40 | 0.30 | 0.70 | 0.05 | 4 | CONST | [05](05-welfare-and-decision-engine.md) |
| `prop.bond` per class | Balance | USDC | 1k / 5k + 0.5%·Ask / 25k / 50k | ×0.1 | ×10 | ×2 | 2 | META | [06](06-governance-and-guardians.md) |
| `intake.max_per_account` | u8 | entries/epoch | **4** (frozen, Part 3) | 2 | 8 | 2 | 2 | META | [06](06-governance-and-guardians.md), [08](08-treasury-and-economics.md) §7 |
| `intake.slash_fraction` (slashed to INSURANCE, never burned) | Percent | % of bond | **10** (frozen, Part 3) | 5 (K floor) | 25 | 5 pp | 2 | META | [06](06-governance-and-guardians.md), [08](08-treasury-and-economics.md) §7 |
| `pol.b` decision, per branch, per class | Balance | USDC | 10k / 25k / 60k / 100k — **floors of the Ask-scaled schedule** | budget-capped | — | 25% | 1 | TREASURY | [04](04-markets-and-pricing.md), [08](08-treasury-and-economics.md) |
| `pol.b_gate` | Balance | USDC | 7,500 | — | — | 25% | 1 | TREASURY | [04](04-markets-and-pricing.md) |
| `pol.b_baseline` | Balance | USDC | **25,000** *(sim-gated — [VERIFY via Phase-0/3 calibration])*; funded from `POL_BASELINE`, **outside** `pol.budget_epoch` | 10,000 | 100,000 | 25% | 1 | TREASURY | [04](04-markets-and-pricing.md), [08](08-treasury-and-economics.md) §4.3 |
| `pol.budget_epoch` | Percent | NAV | 0.75% | — | **1.5% K** | — | 2 | META | [08](08-treasury-and-economics.md) |
| `exec.timelock` per class | u32 | blocks | 2 d / 3 d / 7 d / 14 d | **24 h K floor** | 30 d | ×2 | 2 | META | [09](09-execution-upgrades-and-rollout.md) |
| `exec.grace` | u32 | blocks | 14 d | **7 d K floor** | 30 d | — | 2 | META | [09](09-execution-upgrades-and-rollout.md) |
| `trs.cap_proposal` | Percent | NAV | 5% | — | **10% K** | 1 pp | 2 | META | [08](08-treasury-and-economics.md) |
| `trs.cap_30d` / `trs.cap_180d` | Percent | NAV | 10% / 30% | — | 15% / 40% K | — | 2 | META | [08](08-treasury-and-economics.md) |
| `trs.stream_threshold` | Percent | NAV | 1% | 0.5% | 5% | — | 2 | META | [08](08-treasury-and-economics.md) |
| `trs.proposer_reward` | Balance | USDC | PARAM 500; TREASURY/CODE min(0.05%·Ask, 25k); META 25k | ×0.1 | ×10 | ×2 | 2 | META | [08](08-treasury-and-economics.md) |
| `iss.inflation_cap` | Percent | /yr | 2% | — | amendable **down only** (K) | — | — | CONST | [08](08-treasury-and-economics.md) §2.3 |
| `fee.vit_usdc_rate` | Fixed | USDC/VIT | 1.0 × `fee.vit_usdc_rate_ref` (ref is K, set at genesis from launch price — **[VERIFY at TGE; placeholder ref 0.05 USDC/VIT]**) | **0.1 × ref (K)** | **10 × ref (K)** | ×2 | 1 | PARAM | [08](08-treasury-and-economics.md) §9 |
| `code.spacing` | u32 | blocks | 30 d | **14 d K floor** | — | — | 2 | META | [09](09-execution-upgrades-and-rollout.md) |
| `orc.bond_floor` (round-1 floor) | Balance | USDC | 10k | 2.5k | 100k | — | 2 | META | [07](07-oracle-and-disputes.md) §6 |
| `orc.bond_bps` (value scaling: `B_1 = max(orc.bond_floor, orc.bond_bps × StakeAtRisk)`; `B_r = B_1·2^(r−1)` — [07](07-oracle-and-disputes.md) §6.1) | Perbill | bps | **250** | **150** (hard min — keeps the §6.3 coverage rule ≥ 10.5%) | 1,000 | ×2 | 2 | META | [07](07-oracle-and-disputes.md) §6 |
| `orc.rounds` R_max | u8 | — | 3 | 2 | 4 | — | 2 | META | [07](07-oracle-and-disputes.md) |
| `orc.window` (challenge) | u32 | blocks | **43,200 (72 h — frozen, D-18)** | 43,200 (72 h kernel floor — never lowered) | 72,000 (120 h) | — | 2 | META | [07](07-oracle-and-disputes.md) §5 |
| `orc.reporter_stake` | Balance | USDC | 100k | 25k | 500k | ×2 | 2 | META | [07](07-oracle-and-disputes.md) |
| `dis.merit_min` (ProcessHold dispute-bond threshold) | Balance | USDC | **= `B_1(c, m)` (value-scaled per [07](07-oracle-and-disputes.md) §6.1; default equality)** | floor: `orc.bond_floor` | — | ×2 | 2 | META | [07](07-oracle-and-disputes.md) §12 |
| `wt.quorum` (watchtower ack) | u8 | of N registered | 2 | 2 (kernel floor) | 5 | 1 | 2 | META | [07](07-oracle-and-disputes.md) §4 |
| `wt.stake` (watchtower bond) | Balance | USDC | 25,000 | 10k | 100k | ×2 | 2 | META | [07](07-oracle-and-disputes.md) §4 |
| `orc.n_min` (reporters before attested admission / Phase-3 arming) | u8 | — | 3 | 3 (K floor) | 16 | 1 | 2 | META | [07](07-oracle-and-disputes.md) §3, [05](05-welfare-and-decision-engine.md) §4.3.1 |
| `reg.bond_incident` / `reg.bond_milestone` | Balance | USDC | 5,000 / 2,500 | ×0.5 | ×10 | ×2 | 2 | META | [07](07-oracle-and-disputes.md) §7 |
| `res.probe_interval` / `res.probe_timeout` | u32 | blocks | 14,400 / 600 | — | — | — | 1 | PARAM | [07](07-oracle-and-disputes.md) §8 |
| `res.probe_amount` | Balance | USDC | 0.10 (10 USDC-cents) | — | — | — | 1 | PARAM | [07](07-oracle-and-disputes.md) §8 |
| `res.fail_threshold` / `res.recover_threshold` | u8 | consecutive probes | 2 / 3 | — | — | — | 2 | META | [07](07-oracle-and-disputes.md) §8 |
| `grd.bond` / allowances | — | — | 50k VIT / [06](06-governance-and-guardians.md) table | — | scope K | — | — | entrenched | [06](06-governance-and-guardians.md) |
| `keeper.rebate` | Balance | USDC | ≈3× fee cost per sanctioned crank **[VERIFY fee basis at benchmark time]** | 1× | 10× | — | 1 | PARAM | [08](08-treasury-and-economics.md) §6 |
| `keeper.budget_epoch` | Balance | USDC | **12,000** (raised per keeper-medium; derivation [08](08-treasury-and-economics.md) §6.2) | 6,000 (floor covers decision-critical load — see §3 note) | 60,000 | ×2 | 1 | PARAM | [08](08-treasury-and-economics.md) |
| `collator.comp_epoch` | Balance | USDC/collator | **2,000** (frozen default, D-15) | 500 | 10,000 | ×2 | 1 | PARAM | [08](08-treasury-and-economics.md) §2.4 |
| `collator.n_min` (K component of `C_onchain`) | u8 | — | 4 | 3 | 12 | 1 | 2 | META | [05](05-welfare-and-decision-engine.md) §4.3 |
| `collator.bond_req_vit` (E-coverage requirement, per collator) | Balance | VIT | **[VERIFY — set with the collator program before Phase-3 arming; sim-gated]** | — | — | ×2 | 2 | META | [05](05-welfare-and-decision-engine.md) §4.3.1 |
| `collator.n_target` (E-coverage denominator) | u8 | — | 5 at launch, phase-scheduled upward **[VERIFY schedule at phase gates]** | 4 | 12 | 1 | 2 | META | [05](05-welfare-and-decision-engine.md) §4.3.1 |
| `sec.prize.param` / `sec.prize.code` / `sec.prize.meta` (InCapPrize capability-envelope proxies; an undefined proxy ⇒ the proposal MUST NOT pass sizing) | Balance | USDC | **[VERIFY — derived from the class capability envelopes in Phase-0 calibration; sim-gated]** (CODE/META effective prize additionally floored at `trs.cap_proposal`·NAV for upgrade payloads, [08](08-treasury-and-economics.md) §5.2) | — | — | ×2 | 2 | META | [05](05-welfare-and-decision-engine.md) §5.6, [08](08-treasury-and-economics.md) §5 |
| `sec.flow_cap` (wash-trade ceiling on measured contest flow in `C_hold` of the `ManipFloor̂` diagnostic — diagnostic-only in v1, never gates) | Fixed | × of `(b_acc + b_rej)` | **[VERIFY — Phase-0 calibration; sim-gated]** | — | — | ×2 | 2 | META | [05](05-welfare-and-decision-engine.md) §5.6, [14](14-threat-model.md) |
| `ops.*` budget lines (`ops.bootnodes`, `ops.rpc_archive`, `ops.collators`, `ops.keepers`, `ops.oracle_evidence`, `ops.monitoring`, `ops.arweave`, `ops.watchtowers`, `ops.coretime`) | Balance | USDC/epoch | **[VERIFY — sized in Phase-2/3 ops planning; ops-gated]** (`ops.collators` = `collator.comp_epoch` × collator count; `ops.keepers` per [08](08-treasury-and-economics.md) §6.3) | — | — | — | 1 | TREASURY | [08](08-treasury-and-economics.md) §1.1, [12](12-release-and-operations.md) §6.1 |
| `grd.review_deadline` (guardian retro-ratification deadline) | u32 | epochs | 2 | 1 | 4 | 1 | 2 | META | [06](06-governance-and-guardians.md) §5.4 |
| `att.bond` (attestor bond) | Balance | VIT | 25,000 | ×0.5 | ×10 | ×2 | 2 | entrenched | [06](06-governance-and-guardians.md) §7 |
| `att.challenge_window` | u32 | blocks | 43,200 (72 h) | 43,200 | 72,000 | — | 2 | META | [06](06-governance-and-guardians.md) §7, [09](09-execution-upgrades-and-rollout.md) §2.4 |
| `ledger.min_split` (= MinTransfer) | Balance | USDC | 0.01 (10⁴ base units) | 0.01 (K floor) | 1 | — | 2 | META | [03](03-conditional-ledger.md) |
| `ledger.archive_delay` | u32 | blocks | 1 yr | 90 d | — | — | 2 | META | [03](03-conditional-ledger.md) |
| `ledger.position_deposit` | Balance | USDC/entry | **0.1** (frozen, Part 3; raised from 0.01) | 0.1 (K floor) | 1 | — | 2 | META | [03](03-conditional-ledger.md) |
| `phase3.tvl_cap` (global real-USDC exposure, D-13) | Balance | USDC | 2,000,000 *(sim-gated **[VERIFY before Phase-3 arming]**)* | — | raised only by phase-gate META + values ratification | — | — | META+values | [09](09-execution-upgrades-and-rollout.md) |
| `phase3.deposit_cap` (per account, D-13) | Balance | USDC | 20,000 *(sim-gated **[VERIFY]**)* | — | as above | — | — | META+values | [09](09-execution-upgrades-and-rollout.md) |

Safety rationale (row-wise, carried forward): kernel floors/ceilings exist so no captured decision sequence can walk a defense to zero — δ cannot reach 0, windows cannot reach one block, κ cannot open flash manipulation, timelocks/grace cannot vanish, p_max cannot exceed 0.10, guardian scope cannot grow, the intake slash fraction cannot reach 0, and the Ask-scaling `2·InCapPrize` term of `dec.v_min` is kernel, not a key.

---

## 2. Kernel constants (K — compile-time, constants-API-exposed)

| Constant | Value | Doc |
|---|---|---|
| `MinTrade` / `MaxTrade` | 1 USDC / b/4 per extrinsic (single-trade impact ≤ 0.25 logit) | [04](04-markets-and-pricing.md) |
| `dec.extension` | 43,200 blocks (3 d), at most once per proposal | [05](05-welfare-and-decision-engine.md) |
| `prop.max_calls` / `max_bytes` / `max_weight` | 16 / 64 KiB / 25% of block limit | [09](09-execution-upgrades-and-rollout.md) |
| `MAX_NESTED` (SafetyFilter recursion; wrapper set closed incl. `proxy_announced`, `as_multi_threshold_1`) | 4 levels, ≤ 16 calls | [06](06-governance-and-guardians.md), [09](09-execution-upgrades-and-rollout.md) |
| LMSR domain bound | `\|q_L − q_S\|/b ≤ 48`; quoting clamp [0.001, 0.999] | [04](04-markets-and-pricing.md) |
| LMSR error bounds | exp2/log2 ≤ 2 ulp (64.64); composed cost ≤ 8 ulp; per-trade cost error ≤ 8·2⁻⁶⁴·b | [04](04-markets-and-pricing.md) |
| Rounding discipline | charges round up, payouts round down (maker-adverse/escrow-favoring); ledger divisions round against the claimant | [03](03-conditional-ledger.md), [04](04-markets-and-pricing.md) |
| **VOID payout rules** | complete pairs recover par via `merge`/`merge_scalar`/`merge_gate` (100%); `redeem_void`: unpaired branch-USDC pays `floor(a/2)`; unpaired LONG/SHORT **and unpaired gate legs (`GateYes`/`GateNo`)** pay `floor(a/4)` (the consistent gate extension of the D-1 rule, [03](03-conditional-ledger.md) §5.3); residue swept per dust rule | [03](03-conditional-ledger.md), [15](15-invariants-and-testing.md) I-26 |
| **Scalar redemption rules** | LONG `floor(a·s)`; **unpaired SHORT `floor(a·(1−s))`**; paired via atomic `redeem_scalar_pair` = exactly `a` | [03](03-conditional-ledger.md) |
| `DescriptorLeadTime` | 43,200 blocks (72 h) between `UpgradeAuthorized` and permissionless application | [09](09-execution-upgrades-and-rollout.md) |
| PB-LEDGER-FREEZE | ≤ 14 days, one renewal only (values referendum); admissible only under the I-4 drift flag | [06](06-governance-and-guardians.md), [09](09-execution-upgrades-and-rollout.md) |
| Expedited CODE lane | 72 h gate market + 3-day fast-track values ratification; admissible only while PB-LEDGER-FREEZE active | [06](06-governance-and-guardians.md), [09](09-execution-upgrades-and-rollout.md) |
| Watchtower window extension (`orc.ext_window`) | one +48 h (28,800-block) extension per `(component, epoch)` lifecycle if `wt.quorum` acknowledgments absent | [07](07-oracle-and-disputes.md) |
| `OracleSettleDeadline(m)` | start of epoch(m+1) Housekeeping (**d20** at the default length): any `(component, m)` not challenge-closed settles **neutrally** for every consuming cohort; late verdicts settle bonds only (I-18) | [07](07-oracle-and-disputes.md) §11, [05](05-welfare-and-decision-engine.md) §7 |
| `orc.max_proof_bytes` | 256 KiB per `recompute_proof` | [07](07-oracle-and-disputes.md) §9 |
| `reg.max_filings_epoch` / `wt.max` / attestor registry floors (`att.min_members` = 3, `att.quorum` = 2) | 64 filings/epoch/instance; ≤ 16 watchtower seats; ≥ 3 attestors, 2-of-N quorum | [07](07-oracle-and-disputes.md) §4/§7, [06](06-governance-and-guardians.md) §7 |
| Kernel attestation | bonded attestor registry (values-elected, ≥ 3); 2-of-3 signed attestations + challenge window | [06](06-governance-and-guardians.md), [09](09-execution-upgrades-and-rollout.md) |
| Dead-man switch | no finalized parachain block for 4,800 relay blocks (~8 h) or snapshot > 4 d overdue ⇒ queue freeze, clock pause; coretime-renewal call **exempt** (D-9) | [05](05-welfare-and-decision-engine.md), [09](09-execution-upgrades-and-rollout.md) |
| `StaleEpochBound` | 7 days ⇒ force-reject all in-flight on next tick | [05](05-welfare-and-decision-engine.md) |
| Crank batch bounds | `TickBatch` = 10; `ReapBatch` = 100; `settle_cohort` ≤ 100 items/call | [05](05-welfare-and-decision-engine.md), [03](03-conditional-ledger.md) |
| Entrenched floors | θS⁻/θC⁻ K-entrenched; guardian scope K; annulment requirement K; VOID rule K | [05](05-welfare-and-decision-engine.md), [06](06-governance-and-guardians.md) |
| Keeper-budget floor note | the 6,000 USDC hard min of `keeper.budget_epoch` is a kernel floor: below it the metered budget cannot cover decision-critical cranks at the 1× rebate bound and A-1 fails silently | [08](08-treasury-and-economics.md) §6 |

---

## 3. Frozen protocol constants (Part 3 of the [decision record](00-decision-record.md))

### 3.1 Epoch schedule — offsets as **fractions of `epoch.length`** (d-labels at the 21-day default)

Phase offsets are stored as rational fractions of `epoch.length` (B-med fix), so the schedule survives `epoch.length` changes. **Changes take effect next epoch; in-flight cohorts keep their creation-time schedule.** Day labels below are the 302,400-block default.

| Phase | Fraction of epoch | Blocks (default) | Label |
|---|---|---|---|
| Intake | [0, 3/21) | 0 – 43,200 | d0–d3 |
| Qualify | [3/21, 4/21) | 43,200 – 57,600 | d3–d4 |
| Seed | [4/21, 5/21) | 57,600 – 72,000 | d4–d5 |
| **Trade** | [5/21, 18/21) | 72,000 – 259,200 | **d5–d18 (13 days — corrected label; was “d4–d18”)** |
| Decide window (accrual) | [15/21, 18/21) | 216,000 – 259,200 | d15–d18 (final 72 h; trailing = final 24 h) |
| Decide | 18/21 | 259,200 | d18 |
| Review (timelock) | d18 + `exec.timelock(class)` | — | per class |
| Execute | per-proposal maturity, within `exec.grace` | — | — |
| Housekeeping | [20/21, 1) | 288,000 – 302,400 | d20–d21 |

Related frozen values: measurement horizon k = 2 epochs; settlement at e+3; ≤ 4 non-terminal cohorts; **maturity worked example B+288,000** (corrected from B+287,000); capital duration example ~63–66 days.

### 3.2 Markets and TWAP

- **TWAP slew cap κ applies per 10-block observation interval** (`mkt.obs_interval`) — the ADR-11 “per 60-block” wording is corrected; the widening rule over k missed intervals is `(1±κ)^k`.
- Observations read the **previous block's** stored quote; staleness counts gaps > 50 blocks **inside the decision window only** (first ⇒ one 3-day extension, second ⇒ reject).
- Gate books are **exempt from the [0.02, 0.98] sanity band** (near-boundary validity rule instead); the band applies to welfare (decision + Baseline) books only; `V_min` is per-book ([05](05-welfare-and-decision-engine.md)).
- Maker worst-case loss `b·ln 2` per book; **worked maker-loss example ≈ 180 USDC** (TREASURY b = 25,000 walked 0.5 → 0.56; corrected from ≈ 1,507).

### 3.3 LMSR authoritative test vectors (b = 10,000 USDC, 64.64; regenerated from the reference model in CI — B-6)

| # | Action | Frozen value |
|---|---|---|
| V1 | cost of buying 1,000 LONG from q=(0,0) | `10000·ln((e^{0.1}+1)/2)` = **512.494795136…** USDC (**corrected** from 512.925464970) |
| V2 | price after V1 | 0.524979187479… |
| V3 | displace 0.5 → 0.6 | Δ = 4,054.65108108… LONG; cost = 2,231.43551314… USDC |
| V4 | worst-case loss | 6,931.47180560… USDC |
| V5 | V1 round trip net of 2 × 30 bps fees | net = **−3.074969…** USDC (**corrected** from −3.077552) |
| V6 | domain edge | a buy pushing `q_L − q_S > 48·b` MUST be rejected (`PriceBoundExceeded`); the state at exactly `48·b` is in-domain per [04](04-markets-and-pricing.md) §4 |

On-chain results MUST match within the §2 error bound plus one base unit of rounding.

### 3.4 Oracle, disputes, governance

- Challenge window **72 h (43,200 blocks)** with watchtower quorum (`wt.quorum` = 2-of-N registered, bonded; one +48 h extension); components not challenge-closed by `OracleSettleDeadline` (d20) settle neutrally ([07](07-oracle-and-disputes.md) §11).
- Oracle latency budget: report 2 d + rounds + 7-day terminal track — reconciled table in [07](07-oracle-and-disputes.md).
- **Oracle adjudication track: 60% approval / 10% support / 7-day decision**, tally on a **pre-cohort conviction snapshot** (VIT locked before the subject cohort's creation) — raised from 50%/3%/5-day (B-19/D-18).
- **Expedited `ratify` schedule** (admissible only while PB-LEDGER-FREEZE is active, for the expedited-CODE-lane referendum only): prepare 0 / decision 3 d / confirm 12 h ([06](06-governance-and-guardians.md) §2.1). `frn.window` (force-rerun Extended window) = `dec.extension` = 43,200 blocks (shared K, [06](06-governance-and-guardians.md) §9).
- Slashing split 40% challenger / 60% INSURANCE; reporter-stake slash 50% on second adjudicated-false report, ejection on third.
- Intake: ≤ 4 entries/epoch/account; 10% bond slash **to INSURANCE** (never burned — USDC is bridged USDC) on non-decision-grade and preimage-missing outcomes (§1 keys; economics [08](08-treasury-and-economics.md) §7).
- Keeper metered budget 12,000 USDC/epoch; collator compensation 2,000 USDC/collator/epoch ([08](08-treasury-and-economics.md)).

### 3.5 Chain identity and supply (owned by [02](02-integration-contract.md); values frozen, D-17)

| Constant | Value |
|---|---|
| ss58 prefix | 7777 (registry submission before Phase 2) |
| paraId | assigned at onboarding; test fixtures 4242 |
| USDC | `ForeignAssets`, Location `{parents: 1, X3(Parachain(1000), PalletInstance(50), GeneralIndex(1337))}` **[VERIFY asset index 1337]**; 6 decimals; `min_balance` 10⁴ |
| VIT | native; **total supply 10⁹, 12 decimals**; existential deposit 0.01 VIT |
| USDC treasury target | ≥ 25M USDC before Phase-5 arming; per-class min-viable NAV floors per [08](08-treasury-and-economics.md) §4 |
| Phase flag | `pallet-constitution::PhaseFlags` (bitset) — the FE trading-enablement key |
| `ReleaseChannel` | fixed-layout raw storage key in `pallet-constitution`, SCALE layout frozen forever (D-14) |
| Bootnodes | ≥ 8 browser-reachable WSS across ≥ 4 operators, ≥ 2 on :443; operator served-state window 30 days | 

---

## 4. Reconciled storage bounds (D-10 — one table, all budgets derive from it)

| Bound | Value | Scope (the reconciliation) | Doc |
|---|---|---|---|
| `IntakeQueue` | **64** | **pre-qualification only** (Submitted, awaiting Screening); overflow ⇒ `IntakeFull`, bond refused | [06](06-governance-and-guardians.md) |
| `MaxLiveProposals` | **32** | **Screening → Settled** — the “(all states)” qualifier is deleted; the two bounds have disjoint scopes and are both kept | [05](05-welfare-and-decision-engine.md) |
| Books per proposal | **≤ 6** (2 decision + 4 gate; the old “7 + margin” is deleted) | per proposal | [04](04-markets-and-pricing.md) |
| Baseline books | ≤ 4 live (one per live epoch) | per chain | [04](04-markets-and-pricing.md) |
| `MaxLiveMarkets` | **196 = 32·6 + 4** (replaces both “≈225 = 32·7+1” and “≤121”) | per chain | [04](04-markets-and-pricing.md) |
| Concurrently *trading* books | 31 = 5·6 + 1 (forecast trading cut, D-8) | per epoch | [04](04-markets-and-pricing.md) |
| `RecentCohortSummaries` ring | **32** cohorts (chain-served history, layer 1 of D-6) | `pallet-epoch` | [02](02-integration-contract.md) |
| `TwapCheckpoints` | 8 per market | `pallet-market` | [04](04-markets-and-pricing.md) |
| `MaxPositionsPerAccount` | **64**, counter-enforced; **protocol accounts (POL/books/treasury subs) exempt** from cap and deposit | `pallet-conditional-ledger` | [03](03-conditional-ledger.md) |
| Positions deposit | **0.1 USDC/entry** (`ledger.position_deposit`) | — | [03](03-conditional-ledger.md) |
| Positions key order | **`(PositionId, AccountId)`** — per-vault prefix-drainable for reaping (was `(AccountId, PositionId)`) | — | [03](03-conditional-ledger.md) |
| `MaxSettlingCohorts` | 4 non-terminal (2 measuring + 1 awaiting oracle + 1 settling) | `pallet-epoch` | [05](05-welfare-and-decision-engine.md) |
| Resource locks | ≤ 32 proposals × 8 domains | `pallet-epoch` | [05](05-welfare-and-decision-engine.md) |
| Oracle games | ≤ 16 components × 4 settling epochs × 3 rounds | `pallet-oracle` | [07](07-oracle-and-disputes.md) |
| `MetricSpecs` | ≤ 16 versions | `pallet-welfare` | [05](05-welfare-and-decision-engine.md) |
| Snapshots | ≤ 20 epochs (H + challenge + 12) | `pallet-welfare` | [05](05-welfare-and-decision-engine.md) |
| `ExecutionRecords` | ring 256 (canonical history is event-derived within the committed window, D-2/D-6 — “pruned to indexer” language deleted) | [09](09-execution-upgrades-and-rollout.md), [02](02-integration-contract.md) |
| Registry filings (Incident/Milestone) | bounded per `pallet-registry` spec | [07](07-oracle-and-disputes.md) |

**Why 64 and 32 are jointly satisfiable:** intake admits ≤ 64 candidates per epoch *before* Screening; qualification passes ≤ `epoch.slots` = 5 per epoch into the live pipeline. Live occupancy = 5 trading + ≤ 20 in measurement/settlement (5 × 4 cohort stages) + extended/suspended/rerun/queued stragglers ≤ 7 of margin ⇒ 32 suffices with headroom; 64 merely prices the pre-qualification waiting room (bonds + slash, [08](08-treasury-and-economics.md) §7).

---

## 5. Derived-value cross-checks (recomputed from 32/196 — normative derivations)

1. **Market state:** 196 books × sizeof(`MarketState`) ≈ 196 × ~512 B ≈ **98 KiB** map ceiling (was 225 × ~512 B ≈ 115 KiB) **[VERIFY struct size at benchmark time]**. Per-extrinsic touch bound: `decide(pid)` reads ≤ 6 proposal books + 1 Baseline + O(10) params; `settle_cohort` ≤ 100 (market, total) items/call — PoV per call bounded regardless of map ceiling.
2. **Vaults:** ≤ 32 live + 4 cohorts × 5 settling = **≤ 52** × ~256 B ≈ 13 KiB **[VERIFY]**; per-branch supply fields (B-4) add two `Balance` words/vault.
3. **Positions:** globally priced by the 0.1 USDC deposit (dusting an account to its 64-cap now costs 6.4 USDC/victim-account, cf. threat row in [14](14-threat-model.md)); per-vault reap drains the `(PositionId, *)` prefix in `ReapBatch` = 100 chunks.
4. **Keeper crank load** (feeds `keeper.budget_epoch`): 31 trading books × (43,200/10) = **133,920 decision-critical** observations/epoch; × (187,200/10) = **580,320 full-window** — derivation and budget fit in [08](08-treasury-and-economics.md) §6. This derivation binds: it assumed `epoch.slots` = 5, books ≤ 6, `mkt.obs_interval` = 10 and `dec.window` = 43,200.
5. **POL/NAV floors** (feed the [09](09-execution-upgrades-and-rollout.md) phase gates): commitments 13,863 / 34,657 / 55,452 / 103,972 / 159,424 (+ 17,329 Baseline); floors 1.85M / 4.62M / 7.39M / 13.86M / 21.26M; five-META worst slate ≈ 106.3M — arithmetic normative in [08](08-treasury-and-economics.md) §3–4.
6. **Parameter-coupling rule (normative):** a decision changing `epoch.slots`, `mkt.obs_interval`, `dec.window`, or `epoch.length` MUST carry re-derivations of items 1–5 in its committed artifact; the classifier tags these keys with a `RederiveBudgets` obligation checked at screening. Books-per-proposal (6) and `MaxLiveProposals` (32) are not keys — changing them is a CODE change that reopens this document.
7. **Chain-served history budget:** `RecentCohortSummaries` 32 × ~256 B + 8 TWAP checkpoints × 196 markets ≈ **70 KiB** of always-served light-client state **[VERIFY sizes]** — within the D-6 layer-1 budget stated in [02](02-integration-contract.md).

---

## 6. Resolves

| Finding | Resolution in this document |
|---|---|
| B-med IntakeQueue vs MaxLive / D-10 | §4: 64 = pre-qualification, 32 = Screening→Settled, disjoint scopes; joint-satisfiability argument; `MaxLiveMarkets` 196 = 32·6+4 replaces 225/121/“6-vs-7” — every budget re-derived in §5 |
| X-11h (backend side; with X-11e) | Reading rules: every FE-rechecked value (position bound, per-trade min/max, `MinSplit`, all §1 keys, all K constants) is constants-API- or storage-readable per [02](02-integration-contract.md); the FE hardcodes nothing |
| B-6 (owned by [04](04-markets-and-pricing.md)) | §3.3: V1 = 512.494795136, V5 = −3.074969 frozen; CI regeneration required |
| B-low drift batch | §3.1 Trade = d5–d18 with fractional offsets; §3.1 maturity B+288,000; §3.2 κ per 10-block interval (ADR-11 corrected), maker-loss ≈ 180 USDC; §3.4 latency reconciliation pointer |
| B-med epoch.length | §3.1: offsets as fractions, next-epoch effectivity, creation-time schedule for in-flight cohorts, 14-day floor |
| B-med Positions map (owned by [03](03-conditional-ledger.md)) | §4: key order `(PositionId, AccountId)`, 0.1 USDC deposit, 64-cap + protocol exemption recorded as the single normative values |
| B-med keeper budget / D-15 / D-16 values | §1: `keeper.budget_epoch` 12,000 (floor 6,000 kernel-justified), `collator.comp_epoch` 2,000, `intake.*`, `fee.vit_usdc_rate`, `pol.b_baseline`, `orc.bond_bps`, `orc.window` 72 h, watchtower/attestor constants — all keys named with owning docs |
| D-7 | Reading rule 5: no EMERGENCY parameter rows exist |
| D-17 / X-11a/b (with [02](02-integration-contract.md)) | §3.5: identity constants frozen once, jointly owned |

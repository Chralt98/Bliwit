# 13 — Parameters, Bounds and Constants (single reconciled table)

**Status: normative component specification. Supersedes the corresponding sections of BACKEND_PLAN.md/FRONTEND_PLAN.md** (BE §21 in full; every bound, constant or default restated anywhere else in either source).

**Boundary.** This document is the **single source of truth for every tunable parameter, kernel constant, storage bound and frozen protocol constant**. Other component documents reference these values and MUST NOT restate different ones (quoting for readability is permitted when marked *(normative value: §13)*). Chain-identity constants and the integration surface are jointly owned with [02](02-integration-contract.md) (D-17); where both documents state a value they are byte-identical by construction of the D-2 freeze.

Normative language: RFC 2119. Blocks at 6 s (14,400/day); USDC 6 decimals, VIT 12 decimals; `Fixed` = `FixedU64` (1e9 scale) at API boundaries.

**Reading rules (X-11e/X-11h — no hardcoding, anywhere):**

1. **K** = kernel constant: compile-time in `futarchy-primitives`, no storage representation, changeable only by a Wasm upgrade that the attestation regime surfaces ([09](09-execution-upgrades-and-rollout.md)). **Boundary of the primitives home:** every K-marked numeric value in this document — the LMSR domain bound and error-bound constants included, which `futarchy-fixed` imports and asserts rather than re-declares — lives in `futarchy-primitives::kernel`; sibling crates MUST import it, never re-declare it. Two carve-outs: per-pallet **storage-bound arguments** (§4) live with their owning pallets ([02 §7](02-integration-contract.md)), and pallet-internal implementation constants with no row in this document need no primitives home. Contract-surface **types** — every type named in 02 §§2–7, event-field enums such as `MarketKind` included — likewise live in `futarchy-primitives` (02 §2).
2. Everything not marked K lives in `pallet-constitution::Params` as a typed record `{ value, min, max, max_delta_per_decision, cooldown, last_change, class, kernel_bounded }`; min/max/max-delta/cooldown/class are genesis-fixed for kernel-bounded keys and META-amendable within compile-time meta-bounds otherwise.
3. **Every value in this document — K constants included — is machine-readable by clients** via the runtime **constants API** (metadata) or the named storage item, per the [02](02-integration-contract.md) contract. The frontend MUST read, never hardcode (this closes X-11h's backend side: there is no value the FE re-checks that the chain does not expose).
4. Every default is a **simulation hypothesis** unless marked frozen; Phase 0–3 calibration obligations are tagged *sim-gated*.
5. `ProposalClass::Emergency` is deleted (D-7): no row in this document carries an EMERGENCY value, and none may be added without reopening D-7.
6. **`ParamKey` encoding (canonical).** The on-chain `ParamKey` ([02](02-integration-contract.md) §2, `[u8; 16]`) is the row's dotted name, UTF-8-encoded and zero-padded to 16 bytes. Names longer than 16 bytes carry an explicit canonical key, written `key:` in their row — truncation is forbidden (the encoder rejects over-long names). **Per-class rows** (`dec.delta`, `dec.sigma`, `dec.v_min`, `prop.bond`, `pol.b`, `exec.timelock` → base `exec.lock`, `trs.proposer_reward` → base `trs.reward`) materialize as four keys with the class suffixes `.param` / `.trs` / `.code` / `.meta`.
7. **Record classes and the view projection.** `ParamRecord.class` uses the six-class set {PARAM, TREASURY, META, CONST, entrenched, META+values}; `ParamView.class: ProposalClass` ([02](02-integration-contract.md) §4) projects CONST and entrenched onto `Constitutional` and META+values onto `Meta`. Rows whose min or max is a kernel floor/ceiling are marked `kernel-bounded`: per rule 2 their **entire** governance-metadata tuple (min/max/max-Δ/cooldown/class) is genesis-fixed and `amend_registry` refuses them outright; all other rows are META-amendable within the compile-time meta-bounds (the amendment must keep `min ≤ value ≤ max`, preserve the value kind, and keep `cooldown ≤ 8` epochs). The kernel-bounded set is, normatively and exhaustively (per-row K/floor/ceiling markers plus the §1 safety rationale; conservative where a bound is cross-key-derived or marked "never lowered"/"down only"): `att.window`, `code.spacing`, `dec.delta.code`, `dec.delta.meta`, `dec.delta.param`, `dec.delta.trs`, `dec.sigma.code`, `dec.sigma.meta`, `dec.sigma.param`, `dec.sigma.trs`, `dec.window`, `epoch.length`, `exec.grace`, `exec.lock.code`, `exec.lock.meta`, `exec.lock.param`, `exec.lock.trs`, `gate.eps`, `gate.p_max`, `intake.slash_pct`, `iss.inflation`, `keeper.budget`, `ledger.min_split`, `ledger.pos_dep`, `mkt.kappa`, `orc.bond_bps`, `orc.n_min`, `orc.window`, `pol.budget_epoch`, `trs.cap_180d`, `trs.cap_30d`, `trs.cap_proposal`, `welfare.thC_lo`, `welfare.thS_lo`, `wt.quorum`. Cross-key couplings that a static record cannot express (`dec.sigma ≤ δ/2`, `gate.eps ≤ p_max/2`, `welfare.wP + wA = 1`, `gate.v_min = 0.1 × dec.v_min(class)`) bind at the consuming engine; the seeded static bounds are conservative over-approximations.

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
| `welfare.thetaS` lo/hi (keys: `welfare.thS_lo` / `welfare.thS_hi`) | Fixed | — | 0.90 / 0.98 | **lo floor K-entrenched** | — | 0.01 | 4 | CONST (loosen: entrenched) | [05](05-welfare-and-decision-engine.md) |
| `welfare.thetaC` lo/hi (keys: `welfare.thC_lo` / `welfare.thC_hi`) | Fixed | — | 0.85 / 0.95 | lo floor K | — | 0.01 | 4 | CONST | [05](05-welfare-and-decision-engine.md) |
| `welfare.wP/wA` (keys: `welfare.wP` / `welfare.wA`) | Fixed | — | 0.60 / 0.40 | 0.30 | 0.70 | 0.05 | 4 | CONST | [05](05-welfare-and-decision-engine.md) |
| `prop.bond` per class | Balance | USDC | 1k / 5k + 0.5%·Ask / 25k / 50k | ×0.1 | ×10 | ×2 | 2 | META | [06](06-governance-and-guardians.md) |
| `intake.max_per_account` (key: `intake.max_acct`) | u8 | entries/epoch | **4** (frozen, Part 3) | 2 | 8 | 2 | 2 | META | [06](06-governance-and-guardians.md), [08](08-treasury-and-economics.md) §7 |
| `intake.slash_fraction` (key: `intake.slash_pct`; slashed to INSURANCE, never burned) | Percent | % of bond | **10** (frozen, Part 3) | 5 (K floor) | 25 | 5 pp | 2 | META | [06](06-governance-and-guardians.md), [08](08-treasury-and-economics.md) §7 |
| `pol.b` decision, per branch, per class | Balance | USDC | 10k / 25k / 60k / 100k — **floors of the Ask-scaled schedule** | budget-capped | — | 25% | 1 | TREASURY | [04](04-markets-and-pricing.md), [08](08-treasury-and-economics.md) |
| `pol.b_gate` | Balance | USDC | 7,500 | — | — | 25% | 1 | TREASURY | [04](04-markets-and-pricing.md) |
| `pol.b_baseline` | Balance | USDC | **25,000** *(sim-gated — [VERIFY via Phase-0/3 calibration])*; funded from `POL_BASELINE`, **outside** `pol.budget_epoch` | 10,000 | 100,000 | 25% | 1 | TREASURY | [04](04-markets-and-pricing.md), [08](08-treasury-and-economics.md) §4.3 |
| `pol.budget_epoch` | Perbill | NAV | 0.75% | — | **1.5% K** | — | 2 | META | [08](08-treasury-and-economics.md) |
| `exec.timelock` per class | u32 | blocks | 2 d / 3 d / 7 d / 14 d | **24 h K floor** | 30 d | ×2 | 2 | META | [09](09-execution-upgrades-and-rollout.md) |
| `exec.grace` | u32 | blocks | 14 d | **7 d K floor** | 30 d | — | 2 | META | [09](09-execution-upgrades-and-rollout.md) |
| `trs.cap_proposal` | Percent | NAV | 5% | — | **10% K** | 1 pp | 2 | META | [08](08-treasury-and-economics.md) |
| `trs.cap_30d` / `trs.cap_180d` | Percent | NAV | 10% / 30% | — | 15% / 40% K | — | 2 | META | [08](08-treasury-and-economics.md) |
| `trs.stream_threshold` (key: `trs.stream_thr`) | Perbill | NAV | 1% | 0.5% | 5% | — | 2 | META | [08](08-treasury-and-economics.md) |
| `trs.proposer_reward` | Balance | USDC | PARAM 500; TREASURY/CODE min(0.05%·Ask, 25k); META 25k | ×0.1 | ×10 | ×2 | 2 | META | [08](08-treasury-and-economics.md) |
| `iss.inflation_cap` (key: `iss.inflation`) | Percent | /yr | 2% | — | amendable **down only** (K) | — | — | CONST | [08](08-treasury-and-economics.md) §2.3 |
| `fee.vit_usdc_rate` (key: `fee.vit_usdc`) | Fixed | USDC/VIT | 1.0 × `fee.vit_usdc_rate_ref` (ref is K, set at genesis from launch price — **[VERIFY at TGE; placeholder ref 0.05 USDC/VIT]**) | **0.1 × ref (K)** | **10 × ref (K)** | ×2 | 1 | PARAM | [08](08-treasury-and-economics.md) §9 |
| `code.spacing` | u32 | blocks | 30 d | **14 d K floor** | — | — | 2 | META | [09](09-execution-upgrades-and-rollout.md) |
| `orc.bond_floor` (round-1 floor) | Balance | USDC | 10k | 2.5k | 100k | — | 2 | META | [07](07-oracle-and-disputes.md) §6 |
| `orc.bond_bps` (value scaling: `B_1 = max(orc.bond_floor, orc.bond_bps × StakeAtRisk)`; `B_r = B_1·2^(r−1)` — [07](07-oracle-and-disputes.md) §6.1) | Perbill | bps | **250** | **150** (hard min — keeps the §6.3 coverage rule ≥ 10.5%) | 1,000 | ×2 | 2 | META | [07](07-oracle-and-disputes.md) §6 |
| `orc.rounds` R_max | u8 | — | 3 | 2 | 4 | — | 2 | META | [07](07-oracle-and-disputes.md) |
| `orc.window` (challenge) | u32 | blocks | **43,200 (72 h — frozen, D-18)** | 43,200 (72 h kernel floor — never lowered) | 72,000 (120 h) | — | 2 | META | [07](07-oracle-and-disputes.md) §5 |
| `orc.reporter_stake` (key: `orc.rep_stake`) | Balance | USDC | 100k | 25k | 500k | ×2 | 2 | META | [07](07-oracle-and-disputes.md) |
| `dis.merit_min` (ProcessHold dispute-bond threshold) | Balance | USDC | **= `B_1(c, m)` (value-scaled per [07](07-oracle-and-disputes.md) §6.1; default equality)** | floor: `orc.bond_floor` | — | ×2 | 2 | META | [07](07-oracle-and-disputes.md) §12 |
| `wt.quorum` (watchtower ack) | u8 | of N registered | 2 | 2 (kernel floor) | 5 | 1 | 2 | META | [07](07-oracle-and-disputes.md) §4 |
| `wt.stake` (watchtower bond) | Balance | USDC | 25,000 | 10k | 100k | ×2 | 2 | META | [07](07-oracle-and-disputes.md) §4 |
| `orc.n_min` (reporters before attested admission / Phase-3 arming) | u8 | — | 3 | 3 (K floor) | 16 | 1 | 2 | META | [07](07-oracle-and-disputes.md) §3, [05](05-welfare-and-decision-engine.md) §4.3.1 |
| `reg.bond_incident` / `reg.bond_milestone` (keys: `reg.bond_inc` / `reg.bond_mile`) | Balance | USDC | 5,000 / 2,500 | ×0.5 | ×10 | ×2 | 2 | META | [07](07-oracle-and-disputes.md) §7 |
| `res.probe_interval` / `res.probe_timeout` (keys: `res.probe_int` / `res.probe_to`) | u32 | blocks | 14,400 / 600 | — | — | — | 1 | PARAM | [07](07-oracle-and-disputes.md) §8 |
| `res.probe_amount` | Balance | USDC | 0.10 (10 USDC-cents) | — | — | — | 1 | PARAM | [07](07-oracle-and-disputes.md) §8 |
| `res.fail_threshold` / `res.recover_threshold` (keys: `res.fail_thr` / `res.recover_thr`) | u8 | consecutive probes | 2 / 3 | — | — | — | 2 | META | [07](07-oracle-and-disputes.md) §8 |
| `grd.bond` / allowances | — | — | 50k VIT / [06](06-governance-and-guardians.md) table | — | scope K | — | — | entrenched | [06](06-governance-and-guardians.md) |
| `keeper.rebate` | Balance | USDC | ≈3× fee cost per sanctioned crank **[VERIFY fee basis at benchmark time]** | 1× | 10× | — | 1 | PARAM | [08](08-treasury-and-economics.md) §6 |
| `keeper.budget_epoch` (key: `keeper.budget`) | Balance | USDC | **12,000** (raised per keeper-medium; derivation [08](08-treasury-and-economics.md) §6.2) | 6,000 (floor covers decision-critical load — see §3 note) | 60,000 | ×2 | 1 | PARAM | [08](08-treasury-and-economics.md) |
| `collator.comp_epoch` (key: `collator.comp`) | Balance | USDC/collator | **2,000** (frozen default, D-15) | 500 | 10,000 | ×2 | 1 | PARAM | [08](08-treasury-and-economics.md) §2.4 |
| `collator.n_min` (K component of `C_onchain`) | u8 | — | 4 | 3 | 12 | 1 | 2 | META | [05](05-welfare-and-decision-engine.md) §4.3 |
| `collator.bond_req_vit` (key: `collator.bond`; E-coverage requirement, per collator) | Balance | VIT | **[VERIFY — set with the collator program before Phase-3 arming; sim-gated]** | — | — | ×2 | 2 | META | [05](05-welfare-and-decision-engine.md) §4.3.1 |
| `collator.n_target` (key: `collator.n_tgt`; E-coverage denominator) | u8 | — | 5 at launch, phase-scheduled upward **[VERIFY schedule at phase gates]** | 4 | 12 | 1 | 2 | META | [05](05-welfare-and-decision-engine.md) §4.3.1 |
| `sec.prize.param` / `sec.prize.code` / `sec.prize.meta` (InCapPrize capability-envelope proxies; an undefined proxy ⇒ the proposal MUST NOT pass sizing) | Balance | USDC | **[VERIFY — derived from the class capability envelopes in Phase-0 calibration; sim-gated]** (CODE/META effective prize additionally floored at `trs.cap_proposal`·NAV for upgrade payloads, [08](08-treasury-and-economics.md) §5.2) | — | — | ×2 | 2 | META | [05](05-welfare-and-decision-engine.md) §5.6, [08](08-treasury-and-economics.md) §5 |
| `sec.flow_cap` (wash-trade ceiling on measured contest flow in `C_hold` of the `ManipFloor̂` diagnostic — diagnostic-only in v1, never gates) | Fixed | × of `(b_acc + b_rej)` | **[VERIFY — Phase-0 calibration; sim-gated]** | — | — | ×2 | 2 | META | [05](05-welfare-and-decision-engine.md) §5.6, [14](14-threat-model.md) |
| `ops.*` budget lines (`ops.bootnodes`, `ops.rpc_archive`, `ops.collators`, `ops.keepers`, `ops.oracle_evidence` (key: `ops.oracle_ev`), `ops.monitoring`, `ops.arweave`, `ops.watchtowers`, `ops.coretime`) | Balance | USDC/epoch | **[VERIFY — sized in Phase-2/3 ops planning; ops-gated]** (`ops.collators` = `collator.comp_epoch` × collator count; `ops.keepers` per [08](08-treasury-and-economics.md) §6.3) | — | — | — | 1 | TREASURY | [08](08-treasury-and-economics.md) §1.1, [12](12-release-and-operations.md) §6.1 |
| `grd.review_deadline` (key: `grd.review_dl`; guardian retro-ratification deadline) | u32 | epochs | 2 | 1 | 4 | 1 | 2 | META | [06](06-governance-and-guardians.md) §5.4 |
| `att.bond` (attestor bond) | Balance | VIT | 25,000 | ×0.5 | ×10 | ×2 | 2 | entrenched | [06](06-governance-and-guardians.md) §7 |
| `att.challenge_window` (key: `att.window`) | u32 | blocks | 43,200 (72 h) | 43,200 | 72,000 | — | 2 | META | [06](06-governance-and-guardians.md) §7, [09](09-execution-upgrades-and-rollout.md) §2.4 |
| `ledger.min_split` (= MinTransfer) | Balance | USDC | 0.01 (10⁴ base units) | 0.01 (K floor) | 1 | — | 2 | META | [03](03-conditional-ledger.md) |
| `ledger.archive_delay` (key: `ledger.archive`) | u32 | blocks | 1 yr | 90 d | — | — | 2 | META | [03](03-conditional-ledger.md) |
| `ledger.position_deposit` (key: `ledger.pos_dep`) | Balance | USDC/entry | **0.1** (frozen, Part 3; raised from 0.01) | 0.1 (K floor) | 1 | — | 2 | META | [03](03-conditional-ledger.md) |
| `phase3.tvl_cap` (global real-USDC exposure, D-13) | Balance | USDC | 2,000,000 *(sim-gated **[VERIFY before Phase-3 arming]**)* | — | raised only by phase-gate META + values ratification | — | — | META+values | [09](09-execution-upgrades-and-rollout.md) |
| `phase3.deposit_cap` (key: `phase3.dep_cap`; per account, D-13) | Balance | USDC | 20,000 *(sim-gated **[VERIFY]**)* | — | as above | — | — | META+values | [09](09-execution-upgrades-and-rollout.md) |

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
| `MIGRATION_STALL_BLOCKS` | Active cursor stalled **> 900 blocks** raises the PB-MIGRATION halt | [09](09-execution-upgrades-and-rollout.md) §3.2 |
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
| Entrenched floors | θS⁻/θC⁻ K-entrenched at **0.90 / 0.85** — deliberately equal to the [05](05-welfare-and-decision-engine.md) launch defaults: the welfare-gate knees are tighten-only from genesis and can never be amended below launch values; guardian scope K; annulment requirement K; VOID rule K | [05](05-welfare-and-decision-engine.md), [06](06-governance-and-guardians.md) |
| Keeper-budget floor note | the 6,000 USDC hard min of `keeper.budget_epoch` is a kernel floor: below it the metered budget cannot cover decision-critical cranks at the 1× rebate bound and A-1 fails silently | [08](08-treasury-and-economics.md) §6 |

---

## 3. Frozen protocol constants (Part 3 of the [decision record](00-decision-record.md))

### 3.1 Epoch schedule — offsets as **fractions of `epoch.length`** (d-labels at the 21-day default)

Phase offsets are stored as rational fractions of `epoch.length` (B-med fix), so the schedule survives `epoch.length` changes. **Representation: the fraction pairs (numerator, denominator = 21) are kernel constants (K) in `futarchy-primitives`, exposed to clients as pallet metadata constants — not `Params` storage.** ([02 §9](02-integration-contract.md)'s constant-binding table currently lumps the fractions with the genuinely-`Params` keys `epoch.length`/`epoch.slots` under one "`params()` + metadata constants" row; splitting the fractions onto the metadata-constant side is queued for the B2 contract amendment alongside SQ-2/SQ-23, so 02 bumps once. Until then a frontend binds the fractions via metadata constants, never `params()`.) **Changes take effect next epoch; in-flight cohorts keep their creation-time schedule.** Day labels below are the 302,400-block default.

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
| USDC | `ForeignAssets`, Location `{parents: 1, X3(Parachain(1000), PalletInstance(50), GeneralIndex(1337))}` (asset index 1337 verified Circle-native sufficient USDC, 2026-07-16, PLAN V-17); 6 decimals; `min_balance` 10⁴ |
| Counterparty system chains | Asset Hub = `Parachain(1000)` (USDC reserve, [09](09-execution-upgrades-and-rollout.md) §6.1); **Coretime chain = `Parachain(1005)`** (renewal-funding target, [09](09-execution-upgrades-and-rollout.md) §4; verified 2026-07-16, PLAN V-18); relay = parent. Single-homed as numbers in `futarchy-primitives::chain_identity` |
| VIT | native; **total supply 10⁹, 12 decimals**; existential deposit 0.01 VIT |
| VIT vesting | SDK `pallet-vesting`; min vested transfer **1 VIT**; genesis-allocation schedules per [08](08-treasury-and-economics.md) §2.1 |
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
| Oracle games (live rounds) | ≤ 16 components × 4 settling epochs × 2 concurrent frozen versions = **128** — one live round per `(component, epoch, version)` game (the 3-round escalation ladder is sequential *within* a game, not concurrent storage; ≤ 2 versions overlap only across a MetricSpec activation boundary — [07](07-oracle-and-disputes.md) §2(4)). Within 02 §3's `open_oracle_rounds` view cap of 192. `MAX_ROUNDS = 128` (SQ-59) | `pallet-oracle` | [07](07-oracle-and-disputes.md) |
| `MetricSpecs` | ≤ 16 versions | `pallet-welfare` | [05](05-welfare-and-decision-engine.md) |
| Snapshots | ≤ 20 epochs (H + challenge + 12) | `pallet-welfare` | [05](05-welfare-and-decision-engine.md) |
| `ExecutionRecords` | ring 256 (canonical history is event-derived within the committed window, D-2/D-6 — “pruned to indexer” language deleted) | [09](09-execution-upgrades-and-rollout.md), [02](02-integration-contract.md) |
| `MIGRATION_SERVICE_WEIGHT_PERCENT` | **50%** of maximum block weight | `pallet-migrations` per-block service budget | [09](09-execution-upgrades-and-rollout.md) §2.1/§3.2 |
| `MIGRATION_CURSOR_MAX_LEN` | **65,536 bytes** encoded | `pallet-migrations` active cursor | [09](09-execution-upgrades-and-rollout.md) §2.1/§3.2 |
| `MIGRATION_IDENTIFIER_MAX_LEN` | **256 bytes** encoded | `pallet-migrations` migration identifier | [09](09-execution-upgrades-and-rollout.md) §2.1/§3.2 |
| Registry filings (Incident/Milestone) | bounded per `pallet-registry` spec | [07](07-oracle-and-disputes.md) |
| `Params` registry | **128** keys (genesis-fixed set; ≥ the ~87 currently-concrete §1 rows plus headroom for `[VERIFY]`-gated rows as they resolve; the `params()` runtime API keeps its own 64-keys-per-call bound, [02](02-integration-contract.md) §3) | `pallet-constitution` |
| `Capabilities` table | 64 rows | `pallet-constitution` |
| `Meters` | 16 (generic bounded-meter primitive; empty at genesis — envelope meters live with their owning pallets, [15](15-invariants-and-testing.md) I-17) | `pallet-constitution` |
| Vesting schedules per account | **8** (`MAX_VESTING_SCHEDULES`; the genesis allocation uses exactly one per founding-team beneficiary) | `pallet-vesting` | [08](08-treasury-and-economics.md) §2.1 |
| Treasury `Streams` | **128** open vesting streams (recipient-claimable grants > `trs.stream_threshold`, §1.3); ≥ `epoch.slots` new grants/epoch over multi-epoch vesting horizons, with headroom | `pallet-futarchy-treasury` ([08](08-treasury-and-economics.md) §1.3) |
| Treasury budget lines | **32** — ≥ the enumerated `POL`/`POL_BASELINE`/`KEEPER`/`ORACLE`/`REWARDS`/`ops.*` lines (§1.1) with headroom; upsert-keyed, so occupancy ≤ the line enumeration | `pallet-futarchy-treasury` ([08](08-treasury-and-economics.md) §1.1) |
| Treasury pending outflows | **64** — queued in-cap proposal outflows awaiting meter grace (§1.3); matched to the `IntakeQueue` pre-qualification ceiling | `pallet-futarchy-treasury` ([08](08-treasury-and-economics.md) §1.3) |
| Treasury POL commitments | **196** = `MaxLiveMarkets` — one live-book subsidy obligation per market that NAV nets against (§1.2/§8.2) | `pallet-futarchy-treasury` ([08](08-treasury-and-economics.md) §8) |
| Treasury coretime obligations | **8** each: ≤ 8 funded-period idempotency keys **and** ≤ 8 open renewal quotes (two separately-bounded collections, D-9); ~8 renewal periods of retained history | `pallet-futarchy-treasury` ([09](09-execution-upgrades-and-rollout.md) §4) |

**Why 64 and 32 are jointly satisfiable:** intake admits ≤ 64 candidates per epoch *before* Screening; qualification passes ≤ `epoch.slots` = 5 per epoch into the live pipeline. Live occupancy = 5 trading + ≤ 20 in measurement/settlement (5 × 4 cohort stages) + extended/suspended/rerun/queued stragglers ≤ 7 of margin ⇒ 32 suffices with headroom; 64 merely prices the pre-qualification waiting room (bonds + slash, [08](08-treasury-and-economics.md) §7).

---

## 5. Derived-value cross-checks (recomputed from 32/196 — normative derivations)

1. **Market state:** 196 books × sizeof(`MarketState`) = 196 × **189 B** (measured `MarketBook` `MaxEncodedLen`, B5 benchmark run 2026-07-17; the pre-measurement ~512 B model stays the pinned growth ceiling) ≈ **36.2 KiB** map ceiling, within the ≤ 98 KiB budget (was 225 × ~512 B ≈ 115 KiB). Per-extrinsic touch bound: `decide(pid)` reads ≤ 6 proposal books + 1 Baseline + O(10) params; `settle_cohort` ≤ 100 (market, total) items/call — PoV per call bounded regardless of map ceiling (measured estimates: `decide` 183,055 B, `settle_cohort(5)` 359,385 B, pinned as regression ceilings in the runtime's `pov_budgets` suite).
2. **Vaults:** ≤ 32 live + 4 cohorts × 5 settling = **≤ 52** × **160 B** (measured `VaultInfo` `MaxEncodedLen`, B5 2026-07-17; ~256 B stays the pinned ceiling) ≈ **8.1 KiB**, within the ≤ 13 KiB budget; per-branch supply fields (B-4) add two `Balance` words/vault.
3. **Positions:** globally priced by the 0.1 USDC deposit (dusting an account to its 64-cap now costs 6.4 USDC/victim-account, cf. threat row in [14](14-threat-model.md)); per-vault reap drains the `(PositionId, *)` prefix in `ReapBatch` = 100 chunks.
4. **Keeper crank load** (feeds `keeper.budget_epoch`): 31 trading books × (43,200/10) = **133,920 decision-critical** observations/epoch; × (187,200/10) = **580,320 full-window** — derivation and budget fit in [08](08-treasury-and-economics.md) §6. This derivation binds: it assumed `epoch.slots` = 5, books ≤ 6, `mkt.obs_interval` = 10 and `dec.window` = 43,200.
5. **POL/NAV floors** (feed the [09](09-execution-upgrades-and-rollout.md) phase gates): commitments 13,863 / 34,657 / 55,452 / 103,972 / 159,424 (+ 17,329 Baseline); floors 1.85M / 4.62M / 7.39M / 13.86M / 21.26M; five-META worst slate ≈ 106.3M — arithmetic normative in [08](08-treasury-and-economics.md) §3–4.
6. **Parameter-coupling rule (normative):** a decision changing `epoch.slots`, `mkt.obs_interval`, `dec.window`, or `epoch.length` MUST carry re-derivations of items 1–5 in its committed artifact; the classifier tags these keys with a `RederiveBudgets` obligation checked at screening. Books-per-proposal (6) and `MaxLiveProposals` (32) are not keys — changing them is a CODE change that reopens this document.
7. **Chain-served history budget:** `RecentCohortSummaries` 32 × **81 B** (measured `CohortSummary` `MaxEncodedLen`, B5 2026-07-17) + 8 TWAP checkpoints × 196 markets at the [04 §7](04-markets-and-pricing.md) spec shape (8 × (4 B `u32` block + 32 B two-limb cumulative) + 1 length byte = 289 B/market; the checkpoint series is not yet implemented — A3-deferred — so its spec-derived maximum is budgeted) = **59,236 B ≈ 57.8 KiB** of always-served light-client state, within the ≤ 70 KiB D-6 layer-1 budget stated in [02](02-integration-contract.md).

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

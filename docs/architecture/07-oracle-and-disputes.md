# 07 — Oracle and Dispute System

**Status: normative component specification. Supersedes the corresponding sections of BACKEND_PLAN.md/FRONTEND_PLAN.md** (primarily BE §15, the oracle rows of §5.2.5, §12.3's attested-input column, and the oracle rows of §21/§22). Normative language: RFC 2119. Implements [D-18 (oracle side)](./00-decision-record.md) and the disposition rows for B-9 (attested side), ProcessHold, oracle bonds, challenge censorship, Incident/MilestoneRegistry, USDC reserve health, and the §15.2-latency low finding.

**Boundary.** This document owns: the adapter hierarchy; the C-pillar determinism split as it constrains the oracle (what is attested, when, and what attested data may never touch); the reporter and watchtower registries; the bonded optimistic reporting game with value-scaled bonds and 72-hour watchtower-acknowledged challenge windows; `pallet-registry` (IncidentRegistry + MilestoneRegistry); the deterministic reserve-health probe `R` and playbook `PB-RESERVE` (trigger side); the scoped `ProcessHold` predicate; the evidence and `recompute_proof` flow; and the reconciled dispute-latency budget. It references, and does not restate: pillar composition, daily flags and `W` ([05](./05-welfare-and-decision-engine.md)); the `OracleResolution` adjudication track and guardian playbook registry ([06](./06-governance-and-guardians.md)); `Voided` ledger semantics ([03](./03-conditional-ledger.md)); oracle/registry economics, budget lines and bootstrap sequencing ([08](./08-treasury-and-economics.md)); frontend surfaces ([10](./10-frontend-architecture.md), [11](./11-frontend-workflows.md)); evidence hosting and ops funding ([12](./12-release-and-operations.md)); residual collusion threats ([14](./14-threat-model.md)); the frozen name/type contract ([02](./02-integration-contract.md)); the master parameter table ([13](./13-parameters.md)).

---

## 1. Adapter hierarchy and the determinism boundary

Priority order (unchanged from BE §15.1, SGF §7.1): (1) **on-chain deterministic** — no oracle at all; (2) **relay-derived** — read from validation data, deterministic; (3) **deterministic cross-chain query** — the reserve-health probe of §8, an XCM query with a fail-static timeout, new in this revision; (4) **bonded optimistic attestation** — everything else. Only class-4 components enter the dispute system of §5. A MetricSpec MUST place every component in the lowest-numbered class that can produce it; `register_spec` MUST reject a class-4 declaration for a quantity derivable in classes 1–3.

Class 3 is deliberately narrow: exactly one component (`R`, §8) uses it in v1, its output is monotone fail-static (a missing or failed response can only *lower* the component), and no other XCM-derived input may be admitted without an entrenched-track amendment. This is the reconciliation of the reserve probe with invariant I-24, which [15](./15-invariants-and-testing.md) restates as: *no XCM outcome may move any decision or settlement input toward adoption or toward higher settlement; absence of a response is always the pessimistic case*.

## 2. The C-pillar split: `C_onchain` and `C_attested` (B-9, D-18)

ADR-10's claim that gate-breach facts are deterministic with "no oracle discretion" was false as previously specified: C mixed same-block counters with attested components that carry challenge windows and up to multi-week dispute latency. The split below makes the claim true by construction. Pillar composition, weights and the daily-flag computation are owned by [05](./05-welfare-and-decision-engine.md); this section is normative for what the oracle may and may not feed.

| Sub-pillar | Components (v1) | Source class | Consumed by |
|---|---|---|---|
| `C_onchain` | Economic security `E` (dimensionless coverage ratios per D-18/B-10 — no attested price, see [05](./05-welfare-and-decision-engine.md)); weight headroom `H`; reserve health `R` (§8) | classes 1–3 | **Daily gate-breach flags `C_daily`**, gate-market settlement, and settlement-time `W` |
| `C_attested` | Incident score `I` (from IncidentRegistry, §7); external-price components if ever admitted (≥ Phase 6, none in v1 — the §17.1 depeg feed remains monitoring-only and is never a settlement input) | class 4 | **Settlement-time `W` only** |

Normative consequences:

1. Daily `C_daily` flags MUST be computed from `C_onchain` alone, same-block, from data available at the flag block. The oracle never gates daily flags; gate markets (which settle on the daily flags — [04](./04-markets-and-pricing.md)) therefore settle without any oracle input, restoring ADR-10.
2. `C_attested` components enter exactly one place: the settlement-time recomputation of `W_{e+1}, W_{e+2}` used to produce `s` for a cohort. A conforming implementation MUST NOT read any class-4 value on any daily, decide-time, or gate path.
3. **What reporters attest, and when.** For each admitted class-4 component `c` and each measurement epoch `m`, one value `v(c, m) ∈` the component's sanity bounds is attested, in the report window opening at the close of epoch `m` (§5 step 1). For `I` the "reporter" is the registry aggregation of §7 (individual filings are the bonded objects; the epoch aggregate is derived deterministically from challenge-closed filings — no separate report round). For any future external-price component, the reporter attests the value computed per the frozen formula (`formula_ref`) from content-addressed raw data.
4. **MetricSpec version freezing.** Cohorts settle on their creation-time MetricSpec version (I-16). A report MUST name the spec version it attests under; a report naming a version other than the frozen version of every cohort consuming `(c, m)` is invalid at dispatch (`Error::SpecVersionMismatch`). Where two live cohorts consume the same `(c, m)` under *different* frozen versions (possible across an activation boundary), one report per version is required and each settles only its own cohorts; the game of §5 runs per `(c, m, version)`.
5. Admission control: an attested component MUST NOT be admitted to a MetricSpec unless its documented maximum single-epoch settlement impact satisfies the bond-coverage rule of §6.3, and ≥ 3 registered reporters plus ≥ 2 registered watchtowers exist (bootstrap sequencing and stake-loan funding per [08](./08-treasury-and-economics.md)).

## 3. Reporter registry

Permissionless entry with `orc.reporter_stake` *(normative value: [13](./13-parameters.md); default 100,000 USDC)* held; exit returns the stake after all rounds the reporter participated in are closed. ≥ 3 registered reporters with full stakes are REQUIRED before any attested component may be admitted to a MetricSpec and before Phase-3 arming ([08](./08-treasury-and-economics.md) funds recallable stake-bootstrapping loans from the incentive allocation). Stake discipline: 50% slash on a second adjudicated-false report; ejection on the third; slashes route 40% to the honest counterparty of the terminating round, 60% to INSURANCE (GFP §9.5).

OCWs on reporter-operated nodes MAY compute values and submit the signed extrinsics automatically; consensus verifies only signatures, bonds and windows. No unsigned oracle transactions are accepted (`ValidateUnsigned` is not implemented for any call in this document).

## 4. Watchtower registry (challenge-censorship repair, D-18)

The prior rule "unchallenged ⇒ final" made silence load-bearing: colluding collators could censor challenges for one 48 h window and finalize a false report (the review's challenge-censorship medium; TM-4's "delay, never wrong" was a mischaracterization). Finalization-by-silence now additionally requires positive, bonded evidence that the report was *observable*:

- **Registry.** `register_watchtower(entity_ref)` — Signed, `wt.stake` *(default 25,000 USDC)* held; bounded `wt.max = 16` seats; watchtowers MUST be independent registered entities under the same entity rule that pins the collator-concentration metric (no two seats per entity; entity registry per [05](./05-welfare-and-decision-engine.md)). Membership is permissionless-with-stake; the values layer MAY recall a watchtower via the `guardian` track ([06](./06-governance-and-guardians.md)).
- **Acknowledgment.** `ack_observed(component, epoch, round, report_hash)` — Signed by a registered watchtower, O(1), keeper-class fee rebate. It asserts exactly: "this report/counter-report was visible in a finalized block and the challenge surface is reachable." It asserts nothing about the value's truth.
- **Quorum rule.** An *unchallenged* round finalizes at window close **only if ≥ `wt.quorum = 2` distinct watchtowers have acknowledged it** (`wt.quorum` is a kernel floor; raising is META-amendable). Otherwise the window extends **once** by `orc.ext_window = 48 h (28,800 blocks)` for that `(component, epoch)` lifecycle — one extension total across all its rounds, never per round. Where a MetricSpec activation boundary makes two per-version games run for one `(component, epoch)` (§2(4)), the single-extension budget is **per frozen-version game** — each version's game gets at most one extension across its own rounds, since the versioned games are independent tracks (they settle only their own cohorts). If at the end of the extension there is still neither a challenge nor a quorum, the value is treated as unobservable: the component takes the **neutral-settlement path** of §10 (never finalizes forward), the reporter's bond is refunded in full (absence of quorum is not the reporter's fault), and a `QuorumFailed` event is emitted.
- A **challenge supersedes the quorum requirement** for that round: a posted challenge is itself proof that the report was observable, and the game proceeds on the escalation clock regardless of acknowledgments.
- **Liveness discipline.** A watchtower that acknowledges no round in an epoch with ≥ 1 open round is marked inactive (event `WatchtowerInactive`); two consecutive inactive epochs slash 10% of `wt.stake` and eject. Rebates make honest participation approximately costless; the stake makes registration-then-abandonment (to starve the quorum) costly.

Corrected TM-4 characterization (row owned by [14](./14-threat-model.md)): collator censorship now yields **delay, and a wrong settlement only under watchtower + collator collusion** — ≥ 2 watchtowers and the censoring collator set must jointly defect. That residual is a threat-model row, not a protocol claim.

## 5. The reporting game (per `(component, epoch)`, class-4 only)

1. **Report** — within 2 days of the measurement epoch's end: `report(component, epoch, spec_version, value, evidence_hash)`, Signed by a registered reporter, round-1 bond `B_1` (§6) held. Evidence MUST be retrievable: content-addressed raw data + recomputation instructions per the frozen MetricSpec; unretrievable evidence is treated as absent (GFP §9.1). No report by window close ⇒ neutral settlement (§10).
2. **Challenge window** — **72 h (43,200 blocks)** *(frozen shared constant; kernel floor — `orc.window` MAY be raised via META to ≤ 120 h, never lowered)*: anyone MAY `challenge(component, epoch, counter_value, evidence_hash)` posting the current-round bond. Window close resolves per the §4 quorum rule: quorum + no challenge ⇒ value final; no quorum + no challenge ⇒ one 48 h extension, then neutral; challenge ⇒ escalate.
3. **Escalation** — bonds double per round (§6), `R_max = 3` rounds, each with its own 72 h window. Where the frozen spec permits deterministic recomputation from the committed raw data, any keeper MAY submit `recompute_proof(round_id, proof)` resolving the round mechanically at any point (§9); otherwise rounds resolve by counter-report + counter-challenge. **Both sides post round `r`'s bond by a consenting signed call** — the reporter via `counter_report(component, epoch, spec_version, value, evidence_hash)`, the challenger via `challenge` at the new bond. Escalation is therefore opt-in on both sides, and a party that does not post within the round's window loses by default: **if the reporter does not `counter_report` before the round's deadline the round resolves in the challenger's favour** — their counter-value settles, subject to §11's money deadline — and the reporter forfeits the stack already posted, per §5.5. Symmetrically, a round drawing no fresh challenge closes to the reporter under §4's quorum rule. No bond may be debited from a party by another party's call: in particular **no keeper crank may inflate a stack the bonded party has not funded**. A crank observes deadlines; it never posts collateral on someone's behalf.
4. **Terminal adjudication** — a round-3 dispute escalates to the `OracleResolution` values track: **60% approval / 10% support / 7-day decision** with a **pre-cohort conviction snapshot** (VIT locked before the subject cohort's creation; capital that entered later does not vote) — track parameters and snapshot mechanics owned by [06](./06-governance-and-guardians.md). The oracle reporting game's only terminal-adjudication call is `oracle.adjudicate(component, epoch, spec_version, value, reporter_wrong)` (SQ-236, 2026-07-21): `(component, epoch, spec_version)` is the round key frozen in [02](./02-integration-contract.md) §7.2 — there is no separate `round_id` — `value` is the settled component value, and `reporter_wrong` records whether §3's reporter-stake discipline applies (a *finding of fault*, not itself a slash: the first adjudicated-false report causes neither slash nor ejection). Trust assumption unchanged: the backstop is stake-weighted (A-3) and exists to make earlier-round lying unprofitable, not for routine use (FGP §6).
5. **Slashing** — the adjudicated-wrong side forfeits its full round-bond stack: 40% to the honest counterparty, 60% to INSURANCE. Reporter-stake discipline per §3. Bond resolution follows the verdict **whenever it lands**, including after the money deadline of §11 — a late verdict settles bonds and reputations but never re-opens settled money (I-18).
6. **Latency cap and money deadline** — per §11: components not challenge-closed by `OracleSettleDeadline` settle neutrally; the schedule budget is met by construction, not by hope.
7. **Neutral settlement** — §10. No path settles "forward" on contested data.

Worked example (BE §30.6, restated under this spec): reporter posts integrations value 0.62 for epoch 41 on a cohort stack with `StakeAtRisk = 400k` USDC ⇒ `B_1 = max(10k, 2.5% × 400k) = 10k`; challenger posts 0.44 with usage-bar evidence (10k); round 2 (20k) counter-assert; round 3 (40k) opens; a keeper's `recompute_proof` resolves mechanically at 0.44. The reporter forfeits the 70k stack (40/60), second offense recorded; settlement uses 0.44; total delay 9 days — inside the §11 budget.

## 6. Bonds: value-scaled (oracle-bonds medium, D-18)

Flat bonds made high-value cohorts cheap to attack: on a ~1.2M-USDC META cohort, shifting `s` by 0.10 on a subjective attested component netted ~+50k USDC even after forfeiting the full flat 70k stack. Bonds now scale with value-at-stake.

### 6.1 Definitions

```
StakeAtRisk(c, m)   = Σ CohortEscrow(k)  over every cohort k whose frozen MetricSpec
                      consumes component c for measurement epoch m
CohortEscrow(k)     = Σ_pid escrowed(pid) over k's vaults, read at the block Snapshot(m)
                      finalizes (deterministic, on-chain; frozen for the lifecycle)
B_1(c, m)           = max(orc.bond_floor, ceil(orc.bond_bps × StakeAtRisk(c, m) / 10,000))
B_r(c, m)           = B_1(c, m) · 2^(r−1),   r = 1…R_max
```

Note the Σ over cohorts: with k = 2, epochs `m` are consumed by two overlapping cohorts (cohort e and e+1 both measure e+2), so the value a false `v(c, m)` can move is the *sum* of their escrows, and the bond prices that sum.

**Units and rounding (normative).** `orc.bond_bps` is denominated in basis points, so the product carries the explicit `/ 10,000` divisor shown above. That division rounds **up**, as does any parameter-representation conversion that produces `orc.bond_bps` from a finer-grained on-chain encoding. Rounding is resolved in the direction of custody, on the same principle as I-4 and I-28 ([15](./15-invariants-and-testing.md) §1): over-custody is a dust/reconciliation matter, whereas under-custody is an unbacked claim. Rounding a bond down is the under-custody direction; rounding up costs at most one base unit. The `max(·)` against `orc.bond_floor` is applied after rounding. The §6.3 admission rule takes `orc.bond_bps` directly and is evaluated in basis-point space, so it is unaffected by this base-unit rounding either way.

**Per-game freezing of `B_1` and `R_max` (normative).** Both bind **once**, when round 1 of a `(component, epoch, spec_version)` game is created, and are stored with the game. Every subsequent escalation derives `B_r` from the stored `B_1` by the doubling rule above and tests terminality against the stored `R_max`; no escalation re-reads `orc.bond_floor`, `orc.bond_bps` or `orc.rounds`. A META amendment to any of those three therefore prices only games opened after it takes effect. The freeze is required, not merely convenient: a live read would let a lawful amendment retroactively under-collateralize a component that was admitted under the §6.3 coverage rule at the older parameters, and would make the §13 bond identity unsatisfiable on states the protocol can lawfully reach. An implementation MUST additionally refuse to open a game whose complete frozen ladder (through `B_1 · 2^(R_max−1)`) is not representable in `Balance`, so that a lawfully opened round can never become uncloseable. This extends §13's freezing language — which scopes "no mid-game repricing" to *escrow* movement — to parameter amendment as well.

### 6.2 Escalation table (defaults; master table [13](./13-parameters.md))

| Round | Bond (each side) | Cumulative forfeit if adjudicated wrong |
|---|---|---|
| 1 (report / first challenge) | `B_1 = max(10,000 USDC, ceil(250 × StakeAtRisk / 10,000))` — i.e. 2.5% of `StakeAtRisk`, rounded up (§6.1) | `B_1` |
| 2 | `2·B_1` | `3·B_1` |
| 3 | `4·B_1` | `7·B_1` |
| Terminal | no new bond; verdict distributes the stack | `7·B_1 = 17.5% × StakeAtRisk` at the default bps |

`orc.bond_floor` default 10,000 USDC (hard min 2,500, hard max 100,000); `orc.bond_bps` default **250 bps**, hard min **150 bps** (see §6.3), hard max 1,000 bps; both META-amendable within bounds, cooldown 2 epochs. Honest-challenger revenue also scales: winning any round pays 40% of the loser's stack, ≥ `0.4·B_1 = 1% of StakeAtRisk` — challenge incentives grow with exactly the value that needs defending.

### 6.3 Bond-coverage rule and the META worked example

**Admission rule (normative, machine-checked at `register_spec`):** an attested component with documented maximum single-epoch settlement impact `Δs_max` (a mandatory MetricSpec field per [05](./05-welfare-and-decision-engine.md) §12.4-equivalent) MAY be admitted only if

```
(2^R_max − 1) · orc.bond_bps  ≥  Δs_max          // default: 7 × 2.5% = 17.5%
```

so that a reporter who must survive every round (or win at terminal, against the pre-cohort-snapshot electorate) risks more than the maximum value a lie can move. The 150 bps hard min keeps the left side ≥ 10.5% even at the parameter floor.

**The review's scenario, recomputed.** META cohort, `StakeAtRisk = 1,200,000` USDC; attacker shifts `s` by 0.10 via a subjective attested component; gross gain bounded by `0.10 × 1,200,000 = 120,000` USDC (attained only if the attacker holds *every* winning scalar unit).

| Regime | `B_1` | Stack at risk | Best-case attacker net |
|---|---|---|---|
| Old (flat) | 10,000 | 70,000 | **+50,000** (profitable) |
| This spec | `max(10,000; 2.5% × 1.2M) = 30,000` | `7 × 30,000 = 210,000` | `120,000 − 210,000 =` **−90,000** |

At any realistic position share (< 100% of the winning side) the loss deepens; a *successful* attack additionally requires winning the terminal referendum against pre-cohort conviction locks, which the bond math no longer subsidizes.

## 7. `pallet-registry` — IncidentRegistry and MilestoneRegistry

The bonded filing/challenge/slashing subsystem that feeds `C_attested` (incidents) and the A pillar (milestones) previously had no owning pallet, no bounds and no budget. It is one pallet, two instances via `RegistryKind ∈ {Incident, Milestone}`. Outputs are consumed **only at settlement time** (§2); registry sub-games can hold *settlement*, never *decisions* (§12).

**Purpose / trust boundary.** Turns permissionless bonded claims about off-chain facts ("an S2 incident occurred in epoch m", "milestone M shipped, 3 points") into challenge-closed on-chain records. It holds only filing bonds; it cannot touch escrow, markets or W directly — `pallet-welfare` reads its *closed* records at snapshot time.

```rust
pub trait Config: frame_system::Config {
    type RuntimeEvent: /* … */;
    type Collateral: fungibles::Mutate<AccountId>;        // USDC bonds
    type Kind: Get<RegistryKind>;                          // instance discriminant
    type MaxFilingsPerEpoch: Get<u32>;                     // 64
    type MaxEvidenceLen: Get<u32>;                         // 32-byte content hash only
    type WeightInfo: WeightInfo;
}
```

**Storage** (all bounded, SCALE-stable):

| Item | Type | Max-size argument |
|---|---|---|
| `Filings: double_map (EpochId, FilingId) → Filing` | `{ who, class: FilingClass, points: u16, evidence_hash: H256, bond: Balance, state: Filed{window_end, extended, acks} \| Challenged{round, window_end, challenger, evidence_hash} \| Upheld \| Rejected, spec_version }` | ≤ `MaxFilingsPerEpoch(=64)` per epoch × ≤ 4 non-settled epochs live; closed epochs reaped a fixed archive delay after close, not at cohort settlement — see the consumption-model note below |
| `FilingCount: map EpochId → u32` | overflow ⇒ `Error::EpochFull` (hard bound, never silent growth) | ≤ 4 keys live |
| `Aggregates: map EpochId → FixedU64` | derived once per epoch at close-out (the `I` input or milestone-points input to welfare) | ≤ 4 keys live |

**Why `Filed`/`Challenged` carry those fields (SQ-69, 2026-07-21).** The earlier sketch gave `Filed{window_end}` and `Challenged{round, window_end}`, which cannot express rules this section already states: `extended` records that the one 48 h quorum-failure extension of §4 has been consumed (without it the extension is repeatable), `acks` carries progress toward the `wt.quorum` watchtower acknowledgments that unchallenged closure requires, and `challenger`/`evidence_hash` bind the counter-party and their content-addressed evidence for the 40/60 bond split of §5.5. The state variants above are the normative shape.

`FilingClass` for the Incident instance = severity `{S1 = 1.0, S2 = 0.4, S3 = 0.1}` (values normative in [05](./05-welfare-and-decision-engine.md)); for the Milestone instance = the enumerated scope classes of the frozen MetricSpec (scope inflation stays challengeable).

**Calls:**

| Call | Origin | Bond / preconditions | Effect | Events | Weight |
|---|---|---|---|---|---|
| `file(epoch, class, points, evidence_hash, spec_version)` | Signed | `reg.bond_incident` (default 5,000 USDC) or `reg.bond_milestone` (2,500 USDC) held; epoch within its filing window (open through the epoch + its report window); count < 64; evidence content-addressed | creates `Filed` with a **72 h challenge window** *(the frozen kernel constant — see the fixed-window note below; supersedes the previous 4-day milestone window)*, watchtower quorum rule of §4 applies to unchallenged closure at the kernel floor `wt.quorum = 2` | `IncidentFiled` / `MilestoneFiled` | O(1) |
| `challenge_filing(epoch, filing_id, evidence_hash)` | Signed | matching bond held; window open | `Challenged`; one counter-round (registry games do not escalate — round 2 closes by `recompute_proof` where the spec permits, else by the filing party's terminal escalation into §5 step 4 as a `(component, epoch)` dispute) | `IncidentChallenged` / `MilestoneChallenged` | O(1) |
| `ack_observed(epoch, filing_id)` | Signed (registered watchtower) | filing in `Filed`; window open; not already challenged; caller has not already acknowledged this filing; fewer than `wt.quorum` acknowledgments recorded | records one acknowledgment toward the §4 quorum that unchallenged closure requires; asserts observability only, never the filing's truth | — (no registry event; quorum state is read from `Filings`. `WindowAcknowledged` is frozen in [02](./02-integration-contract.md) §7.2 for the **oracle**, with shape `{ component, epoch, round, watchtower }`; whether 02 §6 gains a registry-shaped counterpart is an open contract question, not settled here) | O(1), keeper-class fee rebate on the oracle budget line |
| `crank_close(epoch, batch)` | Signed (keeper, rebated) | window elapsed | closes ≤ 20 filings/call: unchallenged + quorum ⇒ `Upheld`; quorum failure ⇒ per §4 (one 48 h extension, then the filing is `Rejected`-as-unobservable with bond refunded); challenged ⇒ resolve per round outcome; loser's bond splits 40/60 per §5.5 | `IncidentUpheld`/`IncidentRejected`/`MilestoneAccepted`/`MilestoneRejected`, `FilingBondSlashed` | bounded batch, rebated on the oracle budget line |
| `close_epoch(epoch)` | Signed (keeper) | all filings terminal | computes and **stores** `Aggregates[epoch]` (Incident: `max(0, 1 − Σ severity)` over Upheld filings, "no filings ⇒ 1"; Milestone: `min(1, points ÷ target)`) for welfare to read at snapshot — see the consumption-model note below | `RegistryEpochClosed` | O(filings ≤ 64) |
| `reap_epoch(epoch)` | Signed (keeper, rebated) | epoch closed; the archive delay of the consumption-model note has elapsed (`Error::ReapNotDue` otherwise) | removes the epoch's filings, filing count, acknowledgments and aggregate; one-shot — a second call finds no close record and errors | — | bounded batch, rebated on the **general** keeper tranche ([08](./08-treasury-and-economics.md) §6.3) |

**Fixed windows and quorum (normative).** The registry's challenge window and watchtower quorum are the **kernel floors** — 72 h (43,200 blocks) and `wt.quorum = 2` — as fixed constants. Unlike the §5 oracle game, which reads the live `orc.window` and `wt.quorum` and therefore tracks a META raise (`orc.window` to ≤ 120 h; `wt.quorum` upward), the registry tracks neither: after such an amendment the oracle's windows move and both registry instances remain at 72 h / 2. The divergence is deliberate. Registry filings are flat-bonded claims about off-chain facts that hold *settlement* only, never decisions (§12), and their closure must clear the §11 money deadline; lengthening their window buys no dispute quality while pushing closure toward that deadline, whereas the oracle's value-scaled game is precisely where additional deliberation time is worth buying. Operator-facing tooling SHOULD surface the asymmetry, because "raise `orc.window`" otherwise reads as a system-wide change and is not one.

**Watchtower acknowledgment and its rebate (normative).** §4 states the acknowledgment call against the oracle's signature `ack_observed(component, epoch, round, report_hash)`; the registry's analogue takes `(epoch, filing_id)` and is the call the `file` row's "watchtower quorum rule of §4 applies" clause invokes. It carries the same **keeper-class fee rebate**, paid from the **oracle budget line** rather than from the metered keeper tranches — watchtower acknowledgment is §4 machinery the registry borrows wholesale, so it is funded where §4's own acknowledgments are (see the crank-funding note below). The two rebate exposures differ by construction and MUST NOT be equalized: the registry stops paying once `wt.quorum` acknowledgments are recorded for a filing and rejects further ones, since quorum is all that unchallenged closure needs, whereas an oracle round accepts and rebates one acknowledgment from each registered watchtower up to `wt.max`. Both deduplicate per acknowledger, so no watchtower is paid twice for the same object, and both degrade fail-soft: when the oracle line cannot cover a rebate, the acknowledgment still succeeds and no rebate is paid.

**Crank funding lines (normative).** This section's rebated cranks are funded from two different places, and the split is deliberate. `ack_observed` and `crank_close` are rebated from the **oracle budget line** ([08](./08-treasury-and-economics.md)): both are dispute machinery — the first is §4's acknowledgment borrowed wholesale, the second is the registry's analogue of `crank_round_close`, which §13 already funds from that line. `reap_epoch` is **not**: it is archival cleanup with no dispute content, and [08](./08-treasury-and-economics.md) §6.3 assigns reaping to the metered **general** keeper tranche, where every other reaping-shaped crank in the system sits. The distinction is a liveness one. The general tranche is capped at 20% of the keeper budget, so exhausting it can never starve a decision-critical crank — and when it is exhausted only the rebate stops, while reaping stays permissionless. The oracle line, by contrast, funds reporter and escalation incentives and carries no per-tranche reservation; charging routine storage housekeeping to it would couple dispute liveness to archival work. `close_epoch` carries no rebate at all — that omission from its origin cell is deliberate, not an oversight.

**Consumption model: welfare pulls, the registry does not push (normative).** `close_epoch` computes the epoch aggregate and **stores** it; `pallet-welfare` **reads** the stored value at snapshot time (§2), as the trust-boundary paragraph above already states. The registry's welfare seam MUST remain a no-op sink and MUST NOT be bound to a live write path: a push would invert the dependency, let a registry crank move a welfare input outside the snapshot, and place the relative ordering of two independently keeper-cranked calls on the settlement path. Closed records MUST therefore remain readable until every cohort that can consume them has settled. Because reaping is time-gated rather than consumption-gated, that gate is normative rather than incidental: an epoch's records MUST NOT be reaped before the §11 money deadline for that epoch has passed and every cohort consuming it has settled, and the archive delay implementing this MUST carry a floor no shorter than the §11 worst case (d21 from the close of the measurement epoch). An implementation that derives the delay from a parameter owned by another component MUST pin that floor independently, so retuning the other component cannot silently shorten it.

**Milestone normalization (normative).** The `target` in `min(1, points ÷ target)` is a field of the **frozen MetricSpec** for the milestone component, not a [13](./13-parameters.md) parameter: it is per-component and per-version, and a cohort settles on its creation-time spec version (I-16), so a global tunable could retroactively renormalize milestones a live cohort is already measuring. The clamp is normative — [05](./05-welfare-and-decision-engine.md) requires every component value to lie in [0, 1] before aggregation, and over-shipping against a target MUST NOT let one A-pillar component exceed 1 and dilute the others. A MetricSpec whose milestone component carries no positive `target` is **not admissible**: `register_spec` MUST reject it, and until the MetricSpec surface carries the field no milestone component may be admitted. A zero or absent target MUST NOT be silently normalized to an aggregate of 0 — that is a fail-*adverse* value masquerading as a measurement, not the fail-closed rejection this rule requires.

**Hooks:** none (I-20). **Errors:** `EpochFull`, `WindowClosed`, `WindowOpen`, `AlreadyChallenged`, `SpecVersionMismatch`, `BondBelowMinimum`, `NotRegistered`, `DuplicateAck`, `AlreadyQuorum`, `ReapNotDue`, `MilestoneTargetUnset` (the Milestone instance's frozen-MetricSpec `target` is zero or absent — `file` and `close_epoch` both refuse per the milestone-normalization note above, so no aggregate is ever derived from an undefined divisor). **Suppression economics:** wrongly *rejecting* a true incident filing costs the challenger the bond (40% to the filer) — permissionless bonded filing plus slash-for-wrong-rejection keeps suppression priced, as the §12.3 gaming-vector column requires. **Audit concerns:** filing-window/report-window fencepost alignment; the terminal-escalation handoff into §5 (a registry dispute that escalates MUST carry §6 value-scaled bonds from that point, sized by the `StakeAtRisk` of the cohorts consuming `I` or the milestone component for that epoch).

Event names above are frozen in [02](./02-integration-contract.md) §6 (pallet-registry row) and match it exactly; this section MUST NOT drift from it.

## 8. Reserve health `R` and `PB-RESERVE` (USDC-freeze medium)

A frozen Asset Hub USDC channel or a frozen sovereign-account balance previously fired nothing: PB-DEPEG watches *price*, which does not move when transfers freeze, so NAV and the FE kept reporting full backing. `R` is a deterministic class-3 sub-metric in `C_onchain`.

**Probe.** Once per epoch day (`res.probe_interval = 14,400 blocks`), the keeper-cranked `crank_reserve_probe()` sends one XCM program to Asset Hub exercising **transferability** of the chain's sovereign USDC: withdraw `res.probe_amount` (default 10 USDC-cents) from the sovereign account, re-deposit to the same account, and report the outcome via a paid `ReportError`/`QueryResponse` leg with a fresh `query_id` (admissibility verified against the live `asset-hub-polkadot` barrier, 2026-07-16, PLAN V-6: the paid `WithdrawAsset + BuyExecution + SetAppendix(SetFeesMode jit + ReportError) + DepositAsset` sequence passes `AllowTopLevelPaidExecutionFrom<Everything>`, which does not inspect appendix contents; the appendix `SetFeesMode { jit_withdraw: true }` pays Asset Hub's non-zero response-delivery fee from the sovereign account so a successful probe does not time out; **no `Transact` in either direction** — the response returns as a `QueryResponse`). Probe fees are paid from sovereign DOT/USDC under the ops budget line ([12](./12-release-and-operations.md), [08](./08-treasury-and-economics.md)).

**Scoring (fail-static, normative):**

- Success response within `res.probe_timeout = 600 blocks (1 h)` ⇒ probe pass.
- Error response, or timeout, or no probe sent (keeper outage) ⇒ probe **fail**. Absence is never healthy: unlike the XCM-traffic metric `X` ("no traffic ⇒ 1"), `R` has no benefit-of-the-doubt branch.
- `R_daily = 1` if the day's probe passed, else `0`; `res.fail_threshold = 2` consecutive failed probes ⇒ `ReserveUnhealthy` state.

**Consequences of `ReserveUnhealthy`:**

1. The daily **C breach flag is set** (via `R = 0` in `C_onchain`), and — because probe traffic rides the USDC channel — sustained unresponsiveness also degrades `X`, a second `C_onchain` component ([05](./05-welfare-and-decision-engine.md) §4.3): the C gate fails toward status quo, deterministically, with no oracle in the loop.
2. **`PB-RESERVE` is armed** (guardian playbook, registered in [06](./06-governance-and-guardians.md)'s playbook registry): activation halts **split inflows only** (`ledger.split` rejects with `Error::ReserveUnhealthy`; `merge`, `redeem*`, trading and withdrawals of already-escrowed value are unaffected — the halt stops new exposure, never exit), and sets the **treasury NAV-haircut flag** (NAV reporting and the mark-down rule are economics, owned by [08](./08-treasury-and-economics.md); the FE surfaces the flag and the degraded-backing banner per [10](./10-frontend-architecture.md)).
3. Recovery: `res.recover_threshold = 3` consecutive passed probes clears the state (`ReserveRecovered`), lifts the split halt automatically, and schedules the mandatory retrospective ratification of the playbook activation per [06](./06-governance-and-guardians.md).

I-24 reconciliation per §1: the probe is the sole XCM-derived settlement input and is monotone fail-static. Residual: a relay or Asset Hub outage honestly sets the flags — that is the intended behavior (the reserve *is* unreachable), same philosophy as `F`.

## 9. Evidence and `recompute_proof`

Evidence is content-addressed raw data + recomputation instructions sufficient for a third party to reproduce the value under the frozen `formula_ref`. Retrievability is a validity condition (§5.1); hosting is a funded ops line with a named owner ([12](./12-release-and-operations.md)); once the Bulletin Chain path is live **[VERIFY: Bulletin Chain mainnet availability and authorization path]**, large artifacts MAY publish there by CID with only the hash on-chain — expiry of evidence storage MUST NOT affect any settled decision. `recompute_proof(round_id, proof)` is permissionless (any keeper, rebated), bounded at `orc.max_proof_bytes = 256 KiB`, and resolves a round mechanically wherever the frozen spec declares the component deterministically recomputable from committed data; the FE surfaces submission and evidence display in the operator area ([11](./11-frontend-workflows.md), FE-15).

## 10. Neutral settlement and VOID

Deadline breach, no-report, quorum failure (§4), or a §11 money-deadline miss ⇒ the component **carries its last valid value with the epoch flagged**; two consecutive flagged epochs ⇒ affected not-yet-settled cohorts recompute `W` without the component, weights renormalized (EFP §3 rule). The two-consecutive-flag tracking is a **welfare** settlement-time operation (welfare owns the `W` recompute and derives consecutiveness from the flagged `ComponentValues` history); the oracle's `NeutralSettlement.flagged_epochs` field is a per-event indicator that this epoch's settlement is flagged, not a running cross-epoch count — SQ-61. If the failed component is a **gate input**, affected cohorts VOID. VOID semantics are owned by [03](./03-conditional-ledger.md)'s `Voided` state: scalar sets settle at neutral `s = 0.5`, complete pairs recover par via `merge`, unpaired branch-USDC pays `floor(a/2)`, unpaired LONG/SHORT pays `floor(a/4)`; decisions already made stand; queued executions depending on the voided epoch's gates cancel (I-15). Under the §2 split no gate input is attested, so the oracle-driven VOID trigger arises only via `R`-adjacent determinism failures or PB-ORACLE-VOID ([06](./06-governance-and-guardians.md)) — the fail-static backstop is retained even though the daily gate path no longer touches the oracle.

## 11. Latency budget, reconciled (§15.2-latency low)

The old §15.2 table (2 d + 3×2 d + 5 d + 2 d ≈ 15 d) matched neither the 72 h windows nor the hardened track. Reconciled worst case, anchored at `t0` = close of measurement epoch `m` (= epoch `m+1`, day 0; for a cohort `e`, the binding case is `m = e+2`, i.e. days count within epoch `e+3`):

| Stage | Window | Worst-case close (days after `t0`) |
|---|---|---|
| Report window | 2 d | d2 |
| Round 1 (72 h) + the single 48 h quorum extension | 3 d + 2 d | d7 |
| Round 2 (72 h) | 3 d | d10 |
| Round 3 (72 h) | 3 d | d13 |
| Terminal: `OracleResolution` 7 d decision + 1 d confirm, immediate enactment ([06](./06-governance-and-guardians.md)) | 8 d | d21 |

Rules making the budget hold by construction rather than by arithmetic luck:

1. **Money deadline.** `OracleSettleDeadline(m) = start of epoch(m+1) Housekeeping (d20)`. Any `(component, m)` not challenge-closed by the deadline settles **neutrally** (§10) for every consuming cohort. The **oracle** owns force-neutralization: the epoch pallet drives `pallet-oracle`'s `force_neutralize_expired(m, expected)` crank at the schedule-derived deadline (the schedule lives in the epoch clock), passing `expected` — the `(component, frozen version)` pairs live cohorts consume for `m` (the epoch/welfare pallet owns that cohort→component map, §2(4)). The crank has **two obligations**: (i) neutral-settle every still-live round for `m` so none survives money-bearing (the §13 try-state invariant); and (ii) for every *expected* component that produced **no report** — which therefore has no round for (i) to touch — write the neutral flagged carry-last `ComponentValues` entry the §10 no-report path requires, so no admitted component is left absent at the deadline (SQ-63). Force-neutralization settles the **money** — it writes the neutral `ComponentValues` entry — but it MUST NOT destroy the round's bond record: the `Rounds` entry is **retained, now non-money-bearing**, until a terminal verdict resolves its stack per §5.5. The §13 try-state invariant is worded for exactly this ("no `Rounds` entry survives its epoch's `OracleSettleDeadline` **in a money-bearing state**"), and a round whose `(component, epoch, spec_version)` already carries a settled `ComponentValues` entry is non-money-bearing by construction — I-18 guarantees that settled value is the neutral one and that no later verdict can overwrite it. This retention is what makes §11(2)'s "the verdict resolves **bonds/reputation only**" and §11(4)'s griefing price implementable at all: removing the round at d20 would discard the very stack those rules dispose of, and would silently refund an attacker who rode a dispute to terminal precisely to force neutral settlement. Retention is bounded by the track's own schedule (7 d decision + 1 d confirm), after which the stack resolves and the entry is reaped. This is SQ-60's division: welfare reads the (now-neutral) `ComponentValues` at settlement; the oracle guarantees the entry exists by the deadline **for every expected component, reported or not**. Cohort settlement therefore always proceeds in its scheduled Housekeeping; `settle_cohort`'s cursor MAY run into the next epoch's opening cranks, which is safe for I-21 because the next cohort cannot enter `Settling` before its own d20, ≥ 19 days later.
2. **Verdict-vs-deadline.** A terminal verdict landing by d20 settles money normally (the common case: an undisputed or early-escalated path closes by ≈ d13–d17). The maximally delayed path (report at d2, extension consumed, all three rounds, full track) lands at d21 — past the deadline — so its money settles neutrally and the verdict resolves **bonds only** (§5.5). This is I-18 verbatim: only challenge-closed values settle money; contested ⇒ neutral/VOID.
3. The single-extension rule (§4) is what keeps the sum at 21 d; per-round extensions would add 4 d and are prohibited.
4. An attacker who rides a dispute to terminal purely to force neutral settlement pays the §6 stack for a status-quo outcome (carry-last + flag, or VOID for gate inputs) — a griefing price, priced in [14](./14-threat-model.md)'s dispute-griefing row.

## 12. `ProcessHold`, scoped (ProcessHold medium)

The decision engine's step-2 predicate ([05](./05-welfare-and-decision-engine.md) §14.1-equivalent) is now:

```rust
fn any_open_dispute_touching(spec: MetricSpecVersion, now: BlockNumber) -> bool {
    Rounds::iter().any(|d|
        d.is_oracle_round()                                   // §5 rounds only — registry
                                                              // sub-games NEVER hold decisions
        && spec.components().contains(&d.component)           // consumed by the proposal's
                                                              // FROZEN MetricSpec version
        && d.posted_bond >= dis_merit_min(d.component, d.epoch))
}
```

- **Merit floor.** `dis.merit_min(c, m) = B_1(c, m)` — the value-scaled round-1 bond of §6.1 (a distinct parameter key so the values layer can raise it independently; default equality). Every §5 oracle challenge qualifies by construction; the flat, smaller registry-filing bonds of §7 never do. Censoring one decide window therefore costs a forfeit-at-risk of ≥ 2.5% of the touched `StakeAtRisk`, not one flat bond.
- **Scope.** "Consumed" means: the component id is in the proposal's *creation-time-frozen* spec version's component set (I-16). Disputes on components outside that set — or on any registry filing whose challenge round has not escalated into a §5 round — do not hold the decision. Rationale: a merit dispute on a consumed component contests the very quantity the proposal's scalar books will settle on (directly for `m ∈` the cohort's measurement window; indirectly through the frozen normalization history); everything else is settlement's problem, handled below.
- **Registry sub-games hold settlement, never decisions.** `settle_cohort` treats a component as not challenge-closed while any consumed filing's window or challenge round is open; the cohort waits in `AwaitingOracle` until closure or the §11 money deadline (then: neutral). `decide()` never reads registry state.
- **Extended proposals and the epoch boundary (explicit).** An `Extended` proposal decides up to 3 days into the next epoch's calendar — exactly when the previous measurement epoch's report window (d0–d2) and round-1 windows (through ≈ d7) are open. The predicate is evaluated **at `decide()` dispatch time against rounds open at that block**; the extension does not grandfather a clean state, and a merit dispute opened during the extension window MAY hold the extended decide. This is the intended direction of failure: `Reject(ProcessHold)` is status-quo, the bond is refundable and the proposal resubmittable (per the reason-code table in [05](./05-welfare-and-decision-engine.md)), while the censor has posted a value-scaled bond it forfeits if the dispute is adjudicated frivolous. Keeper-lag races change nothing: a late `decide` evaluates the same recorded accumulators and the same dispatch-time predicate. The residual (repeated bonded censorship across epochs) is a [14](./14-threat-model.md) row with the §6 economics attached.
- A hold never produces a noisy PASS, and `DeadMan::engaged()` / guardian holds remain independent conjuncts unchanged from the source.

## 13. `pallet-oracle` implementation deltas

Format per BE §5.2. Storage names and value shapes are frozen in [02](./02-integration-contract.md) §7.2; this pallet implements exactly those items (all bounded): `Reporters: map AccountId → ReporterInfo` (≤ 64); `Watchtowers: map AccountId → WatchtowerInfo` (≤ `wt.max = 16`); `Rounds: map (MetricId, EpochId, MetricSpecVersion) → RoundState { component, epoch, round, spec_version, reporter, value, evidence_hash, bond, challenge_deadline, extended, challenger, counter_value, acks, report_hash, stake_at_risk, cumulative_reporter_bond, cumulative_challenger_bond }` — ≤ **128** = 16 components × ≤ 4 concurrently-settling epochs × ≤ 2 concurrent frozen versions (one live game per `(component, epoch, spec_version)` triple; per-version games across an activation boundary per §2(4) — contract v3, 02 §7.2/§13); `ComponentValues: map (MetricId, EpochId, MetricSpecVersion) → SettledComponent { value, path, flagged }` reaped at cohort settlement; `ReserveHealth: { consecutive_fails: u8, consecutive_passes: u8, unhealthy: bool, last_query_id: u64, last_probe_at: BlockNumber, pending_since: Option<BlockNumber> }` (single value). Calls: `register_reporter`, `deregister_reporter`, `report`, `challenge`, `counter_report`, `recompute_proof`, `register_watchtower`, `ack_observed`, `crank_round_close(batch)`, `crank_reserve_probe`, `adjudicate(component, epoch, spec_version, value, reporter_wrong)` (`OracleResolution` origin only). Hooks: none except the `QueryResponse` handler for the reserve probe (O(1), keyed by stored `query_id`; unknown query ids are dropped unpaid). Errors: `NotRegistered`, `WindowClosed`, `BondBelowMinimum`, `SpecVersionMismatch`, `AlreadyFinal`, `QuorumPending`, `ReserveUnhealthy`. Weight drivers: O(1) per call; round-close and probe cranks are bounded batches, keeper-rebated within the oracle budget line ([08](./08-treasury-and-economics.md)).

**Events (canonical names frozen in [02](./02-integration-contract.md) §7.2; this list matches it exactly):** `ReporterRegistered`, `ReporterSlashed`, `ReporterEjected`, `Reported`, `Challenged`, `RoundEscalated`, `RecomputeProven`, `AdjudicationRequested`, `Adjudicated`, `ComponentSettled`, `NeutralSettlement`, `WindowAcknowledged`, `WindowExtended`, `QuorumFailed`, `WatchtowerRegistered`, `WatchtowerInactive`, `WatchtowerSlashed`, `ReserveProbeSent`, `ReserveProbeResult`, `ReserveUnhealthy`, `ReserveRecovered`.

**Invariants (machine-checked in `try-state`):** I-18 (only challenge-closed values settle money); every `ComponentValues` entry is either quorum-acknowledged, challenge-resolved, adjudicated, or neutral-flagged; `Σ held bonds == Σ open-round bonds × sides`; no `Rounds` entry survives its epoch's `OracleSettleDeadline` in a money-bearing state. **Audit concerns:** window/extension fencepost arithmetic at epoch boundaries; ack replay across rounds (acks are per-round, keyed by `report_hash`); bond-schedule freezing at snapshot finalization (no mid-game repricing when escrow changes, and none when `orc.bond_floor`/`orc.bond_bps`/`orc.rounds` are amended — the per-game freeze of §6.1); the registry→oracle terminal-escalation handoff (§7).

## 14. Parameters introduced or changed here

Single source of truth is [13](./13-parameters.md); this table enumerates the keys this document defines so 13 can consolidate them. All defaults are simulation hypotheses unless marked K.

| Key | Default | Bounds / class |
|---|---|---|
| `orc.window` | **72 h (43,200 blocks)** — frozen shared constant | kernel floor 72 h; META ≤ 120 h |
| `orc.ext_window` | 48 h (28,800 blocks), once per lifecycle | K |
| `orc.bond_floor` | 10,000 USDC | 2,500 – 100,000; META |
| `orc.bond_bps` | 250 bps of `StakeAtRisk` | **hard min 150 bps** (§6.3); max 1,000; META, cooldown 2 |
| `orc.rounds` `R_max` | 3 | 2 – 4; META |
| `orc.reporter_stake` | 100,000 USDC | 25k – 500k; META |
| `orc.max_proof_bytes` | 256 KiB | K |
| `wt.quorum` | 2 | kernel floor 2; META upward |
| `wt.max` | 16 | K |
| `wt.stake` | 25,000 USDC | 10k – 100k; META |
| `dis.merit_min` | `= B_1(c, m)` (value-scaled) | floor: `orc.bond_floor`; META |
| `reg.bond_incident` / `reg.bond_milestone` | 5,000 / 2,500 USDC | ×0.5 – ×10; META |
| `reg.max_filings_epoch` | 64 | K |
| `res.probe_interval` / `res.probe_timeout` | 14,400 / 600 blocks | PARAM |
| `res.probe_amount` | 10 USDC-cents | PARAM |
| `res.fail_threshold` / `res.recover_threshold` | 2 / 3 consecutive | META |
| `OracleSettleDeadline(m)` | start of epoch(m+1) Housekeeping (**d20** at the default length) — the §11 money deadline | K (schedule-derived; consolidated in [13](./13-parameters.md) §2) |

The previous `orc.bond0 / rounds / window` row of BE §21 is superseded by the rows above.

---

## Resolves

| Finding | Resolution in this document |
|---|---|
| B-9 (attested side; D-18 gate split) | §2: `C_attested` (incidents, external prices) enters settlement-time `W` only; daily gate flags and gate-market settlement consume `C_onchain` exclusively — the oracle never gates daily flags; reporters' subject matter, timing and MetricSpec-version freezing specified (§2.3–2.4). On-chain pillar composition owned by 05. |
| ProcessHold (medium) | §12: `any_open_dispute_touching` scoped to §5 oracle rounds on components consumed by the proposal's frozen MetricSpec with posted bond ≥ `dis.merit_min` (= the value-scaled round-1 bond); registry sub-games hold settlement, never decisions; extended-proposal/epoch-boundary evaluation made explicit (dispatch-time predicate, no grandfathering, refundable status-quo reject); griefing priced via §6 and rowed in 14. |
| Oracle bonds (medium; D-18) | §6: `bond = max(flat_floor, ceil(bps × StakeAtRisk / 10,000))` with doubling rounds expressed in those terms; §6.3 bond-coverage admission rule; the ~1.2M-USDC META cohort attack recomputed from +50k profitable to ≤ −90k. |
| Challenge censorship (medium; D-18) | §4–§5: 72 h windows (frozen constant) + bonded-watchtower quorum (≥ 2-of-N ack) required for finalization-by-silence, else one 48 h extension, then neutral settlement — "unchallenged ⇒ final" no longer settles money under censorship; TM-4 corrected to "delay, and wrong only under watchtower + collator collusion" (residual rowed in 14). |
| Incident/MilestoneRegistry (medium) | §7: `pallet-registry` fully specified — bonded filings, 72 h challenge windows with quorum, slashing (40/60), bounded storage with max-size arguments, weights, frozen event names (02); outputs feed `C_attested`/A at settlement only. |
| USDC reserve health (medium) | §8: deterministic class-3 `R` sub-metric in `C_onchain` (Asset Hub transferability probe via XCM query + fail-static timeout); frozen/unresponsive reserve sets the daily C breach flag (via `R`, and under sustained unresponsiveness via `X` as well — both `C_onchain` components, [05](./05-welfare-and-decision-engine.md) §4.3) and arms `PB-RESERVE` (split-inflow halt + NAV-haircut flag; economics in 08, FE surface in 10); I-24 restated fail-static (§1, 15). |
| §15.2 latency (low) | §11: latency table reconciled with the 72 h windows, single 48 h extension and the hardened 60%/10%/7-day track (06); worst case 21 d bounded by the d20 money deadline — contested-at-deadline values settle neutrally, verdicts settle bonds whenever they land (I-18). |

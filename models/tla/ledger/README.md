# Ledger TLA⁺ model

This directory is the bounded exhaustive proposal-vault model required by
[03 §11](../../../docs/architecture/03-conditional-ledger.md) and
[15 §4.1](../../../docs/architecture/15-invariants-and-testing.md). `Ledger.tla`
checks the complete modeled proposal-vault operation alphabet over every legal and
illegal dispatch interleaving. `Next` invokes every public operation from every
modeled vault state and over every relevant actor/argument; the operation itself
chooses success or an explicit real-name rejection. Amounts are integer USDC base
units and payouts use integer division, including a dedicated quarter-value scope
with amounts 4 and 5.

## Checked properties

| Model predicate | Obligation |
|---|---|
| `TypeOK` | Every state component remains inside its declared finite domain. |
| `LedgerIdentities` | The proposal-vault part of I-1/L-1: live per-branch identities in `Open`/`Resolved`, tracker agreement in every state, and the state-dependent terminal valuation forms of 03 §6.5. |
| `ConservationBound` | The per-proposal-vault 03 §6.5 bound `MaxRemainingPayout <= escrow`, lifetime `cumulativePaidOut <= escrowIn`, and exact modeled flow `escrowIn = escrow + cumulativePaidOut`. This is not the cross-vault I-4 custody statement. |
| `ResolveSafety` / `AuthoritySafety` | I-3's exactly-once, `Open`-only, `ResolveAuthority` structure plus authority/source-state checks for `void` and settlement (03 §2.3, §5.2). The post-decision half is assumed at the ledger interface and discharged by the proposal model's I-9/I-15 checks: resolve is emitted only on T17/T21/T22 (05 §2.1). |
| `VoidConservation` | I-26/D-1: entry escrow equals remaining escrow plus all Voided payouts; merges pay par, set merges/transfers pay zero, and unpaired claims use `floor(a/2)` or `floor(a/4)` (03 §6.4). |
| `VoidedCallSurface` / `SweepTiming` | Amended I-27: recovery calls are merge, scalar/gate merge, transfer, and `redeem_void`; `sweep_dust` additionally succeeds after the abstract `RedemptionArchiveDelay` event. Every other public call, plus a premature sweep, errors (03 §5.4; 15 §1 I-27). |
| `NoReopening` | D-8: no return to `Open`, and neither terminal state can be left (03 §2.3). |
| `OutflowMonotonicity` | In `Resolved`, only `merge` can reduce escrow; all ordinary redemption attempts fail (03 §2.3, §6.4). |
| `PayoutExactness` | I-5: par branch/pair/gate redemptions and claimant-adverse LONG/SHORT flooring (03 §5.3, §6.3). |
| `RejectedEconomicStutter` | Action property proving every explicit rejected public operation leaves all economic state unchanged while retaining the real attempted operation name. |

Rejected calls preserve `lastOp` as the actual attempted call (`Split`, `Redeem`,
and so on), `lastActor`, the relevant kind, and amount. Therefore I-27 and the
`Resolved` redemption lockout are not truths obtained from omitted transitions or
duplicated `Next` guards. The checked configs retain a state-count floor, while four
witness configs must produce TLC invariant violations proving reachability of a live
nonzero Voided gate-leg redemption, a nonzero scalar quarter payout, rejected
`Redeem` in `Resolved`, and rejected `Split` in `Voided`. `Done` permits legitimate
stuttering without requiring `-deadlock`.

## Config architecture — the fingerprint-view partition

The `last*` audit variables are pure edge labels (no guard reads them), but they
multiply TLC's fingerprint space ~50–70×. `FingerprintView` excludes them from
state identity, which makes the large conservation scopes tractable — and makes
invariants OVER the labels unsound to check there: a `Reject` edge changes only
labels, so its successor deduplicates against its own source and is never
re-evaluated. The gate therefore partitions:

| Configs | VIEW | Invariants checked | Observed distinct states (8 workers) |
|---|---|---|---|
| `Small.cfg` / `Full.cfg` / `Quarter.cfg` | `FingerprintView` | Pure-state only: `TypeOK`, `LedgerIdentities`, `ConservationBound`, `ResolveCountSafety`, `VoidAccounting`, `NoReopeningState`, plus the per-edge action property `RejectedEconomicStutter` (action properties are view-independent) | 26,944 (3 s) / 2,861,160 (9 m 23 s) / 155,183 (11 s) |
| `Audit.cfg` / `AuditQuarter.cfg` | none | The FULL list including every label invariant: `ResolveSafety`, `AuthoritySafety`, `SweepTiming`, `VoidConservation`, `VoidedCallSurface`, `NoReopening`, `OutflowMonotonicity`, `PayoutExactness` — checked on every edge, rejects included, at the Small and Quarter scopes respectively | 1,882,843 (3 m 25 s) / 10,063,640 (9 m 25 s) |
| `Witness*.cfg` | none | One negated reachability condition each; TLC MUST violate it (runner `WITNESS_CONFIGS` contract) | seconds each |

The manifest floor (`MIN_DISTINCT_STATES=13000`) is ~50% of the smallest observed
main-config count (Small's 26,944).

## Action correspondence

| Model action | Normative source | Core cross-check |
|---|---|---|
| `DecisionSignal` | Assumed I-3 post-decision interface input; 05 §2.1 T17/T21/T22 and 15 §1 I-9/I-15 | No ledger function; the proposal model checks the upstream resolve-emission contract. |
| `ArchiveDelayElapsed` | 03 §5.4; amended 15 §1 I-27 | Clock/archive input compressed to a monotone terminal-state Boolean. |
| `Split` / `Merge` | 03 §5.1, §6 equations | `LedgerState::split` / `merge` |
| `SplitScalar` / `MergeScalar` | 03 §5.1, §6, §6.3 | `split_scalar` / `merge_scalar` |
| `SplitGate` / `MergeGate` | 03 §5.1, §6, §6.4 | `split_gate` / `merge_gate` |
| `Transfer` | 03 §5.1 | `transfer` |
| `Resolve` | 03 §2.3, §5.2; 15 §1 I-3 | `resolve` |
| `Void` | 03 §2.3, §5.2, §6.4, including Open/PB-ORACLE-VOID and Resolved/dispute/T20 timing | `void` |
| `SettleScalar` / `SettleGate` | 03 §2.3, §5.2 | `settle_scalar` / `settle_gate` |
| `Redeem` | 03 §5.3, §6 | `redeem` |
| `RedeemScalar` / `RedeemScalarPair` | 03 §5.3, §6.3 | `redeem_scalar` / `redeem_scalar_pair` |
| `RedeemGate` | 03 §5.3, §6 | `redeem_gate` |
| `RedeemVoid` | 03 §5.3, §6.4; 15 §1 I-26 | `redeem_void` |
| `SweepDust` | 03 §5.4, §6.5, §7 R-5; amended 15 §1 I-27 | pallet `sweep_dust` housekeeping path in either terminal state after archive delay |

The Baseline family is intentionally excluded. It is an independent two-leg scalar
vault with no branch resolution or Voided state; including it multiplies the state
space without strengthening the proposal-vault I-3/I-26/I-27/D-8 obligations. Its
pair/floor algebra is represented by `RedeemScalar`/`RedeemScalarPair`, but that does
**not** prove the Baseline I-1 identity, Baseline key isolation, or aggregation with
proposal vaults. Those obligations, the I-4 cross-vault sum, and sovereign-account
custody remain owned by try-state, PT suites, and the ledger differential.

## Constants and scopes

| Constant | `Small.cfg` | `Full.cfg` | `Quarter.cfg` | Meaning |
|---|---:|---:|---:|---|
| `Holders` | 2 | 2 | 1 | The two-holder scopes cover transfer, fragmentation, assembly and ordering; the quarter scope reduces this dimension for tractability. |
| `ResolveAuthority` | 1 distinct | 1 distinct | 1 distinct | Only origin allowed to resolve or void; all modeled accounts still attempt those calls (03 §5.2). |
| `SettleAuthority` | 1 distinct | 1 distinct | 1 distinct | Only origin allowed to settle scalar/gate values; all modeled accounts still attempt those calls (03 §5.2). |
| `MaxEscrow` | 1 | 2 | 5 | Finite lifetime collateral-in bound, in abstract base units. |
| `MaxAmount` | 1 | 2 | 5 | Upper bound for audited call amounts. |
| `Amounts` | `{1}` | `{1,2}` | `{4,5}` | Enumerated public-call amounts; the third scope makes nonzero `div 4` payouts reachable and distinguishes 4 from 5 flooring. |
| `ScoreScale` | 2 | 3 | 2 | Fixed-point denominator used by integer flooring. |
| `ScoreGrid` | `{0,1,2}` | `{0,1,3}` | `{0,1,2}` | Both endpoints and a remainder-producing interior score. |

`Branches`, both `Gates`, all seven per-branch instrument kinds, and the full public
action alphabet remain fixed in all configs. `Audit.cfg` reuses the Small scope and
`AuditQuarter.cfg` the Quarter scope (without the fingerprint view — see the config
architecture above). Holder symmetry changes only
representation. The manifest's state floor is calibrated from fresh observed counts;
see the verification record below.

## Abstractions and exclusions

- One proposal vault is modeled. The checked identity is only the proposal-vault
  portion of I-1 and the checked solvency claim is only `V(state) <= E` from 03
  §6.5. Baseline identity, vault IDs, cross-vault summation, and sovereign custody
  (I-4) are excluded; try-state, PT suites, and the differential own them.
- I-3's post-decision half is an assumed interface contract. This model checks only
  exactly-once/authority/`Open` structure; the proposal model checks I-9/I-15 and
  permits resolve emission only through T17/T21/T22 (05 §2.1; 15 §1).
- `usdcSupply`, `scalarMass` (`Q_b`), and `gateMass` (`G_{b,g}`) are live aggregate
  supplies. In terminal states the mass is the live pairable minimum after asymmetric
  burns. This is the valuation-relevant abstraction of the stored counters; exact
  per-instrument totals are independently recomputed from per-account holdings.
- Amounts and escrow are bounded small naturals. The operations are linear in amount;
  03 §6.5 supplies the induction to unbounded legal balances. Overflow behavior is
  represented by refusing `split` above `MaxEscrow`; Rust checked-arithmetic error
  atomicity is covered by explicit rejected transitions.
- Minimum split/transfer, existential deposits, account-position caps, position
  storage counts, fees, and ED routing are omitted. They gate or accompany a call but
  do not alter the 03 §6 accounting equations; failed calls stutter here. Transfer
  remainder sweeping is likewise irrelevant to aggregate conservation.
- Gate class policy is omitted because the ledger permits both gate families for
  every proposal vault (03 §5.1). Both gate outcomes and unset-before-settlement
  interleavings remain explicit.
- `ArchiveDelayElapsed` compresses the full `RedemptionArchiveDelay` clock to one
  monotone interface event. `SweepDust` is then one atomic cleanup in either terminal
  state (`ScalarSettled` or `Voided`): it removes bounded position entries and sends
  remaining escrow to INSURANCE. This follows amended 15 §1 I-27 and 03 §5.4.
  `ReapBatch`, Merkle claims, deposit refunds, and cursor progress remain storage and
  liveness concerns outside these solvency invariants.
- Fees and direct pallet-account transfers are outside the ledger call semantics;
  both are ordinary fully collateralized positions when they reach this boundary
  (03 §7 R-3/R-6).

## Running

From the repository root:

```bash
tools/verify/run-model-checks.sh
```

Direct smoke invocation:

```bash
cd models/tla/ledger
java -cp ../../../tools/verify/bin/tla2tools.jar tlc2.TLC \
  -workers auto -config Small.cfg Ledger.tla
```

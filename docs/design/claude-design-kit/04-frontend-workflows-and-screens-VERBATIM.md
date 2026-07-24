> **DERIVED COPY for design-tool context — DO NOT EDIT.**
> Verbatim copy of `docs/architecture/11-frontend-workflows.md` (the frozen source of truth),
> regenerated 2026-07-23 from integration contract v11 (A12 signed oracle escalation custody; B19 attestor lifecycle; B18 guardian-review and ratification-binding
> workflow), on top of the previous SQ-483 refresh at
> commit `a893790`, for upload to Claude Design. If this copy and the source ever differ, the
> source wins. Regenerate by re-copying the source file.

# 11 — Frontend Workflows and Screens

**Status: normative component specification. Supersedes the corresponding sections of BACKEND_PLAN.md/FRONTEND_PLAN.md** (FE §17 wallet/transaction architecture, FE §19 UX/degradation matrix, the workflow-facing rows of FE §30, and the FE §26 work breakdown; it consumes but does not restate FE §18, which is owned by [10-frontend-architecture.md](10-frontend-architecture.md)).

**Boundary.** This document owns: the user-facing screen inventory, the transaction-construction and signing-safety rules, every per-call precondition table, the VOID redemption UX, the governance surface (FE-14), the operator surface (FE-15), the USDC funding flow, the sudo-era banner, the UX degradation matrix (E-rows), and the frontend work breakdown. It references: the frozen chain↔frontend contract in [02-integration-contract.md](02-integration-contract.md) (all storage/event/call/constant names used here are the canonical ones frozen there), the ledger semantics in [03-conditional-ledger.md](03-conditional-ledger.md), governance tracks and ratification in [06-governance-and-guardians.md](06-governance-and-guardians.md), the oracle/registry games in [07-oracle-and-disputes.md](07-oracle-and-disputes.md), fee and treasury economics in [08-treasury-and-economics.md](08-treasury-and-economics.md), the execution guard's dispatch-time checks in [09-execution-upgrades-and-rollout.md](09-execution-upgrades-and-rollout.md), the data layer and boot/compat machines in [10-frontend-architecture.md](10-frontend-architecture.md), and release/operations in [12-release-and-operations.md](12-release-and-operations.md). Constants quoted here are for readability only; normative values live in [13-parameters.md](13-parameters.md) and [02](02-integration-contract.md).

RFC 2119 language throughout.

---

## 11.1 What is carried forward unchanged

The design review verified the following as correct; this document carries them forward deliberately, not by omission:

1. **The pre-sign refresh design (INV-FE-2).** Every submit path re-evaluates its full precondition set against a single freshly pinned finalized block B′ immediately before the wallet is invoked, structurally mirroring the backend's own dispatch-time revalidation (`execute()`'s checks in [09](09-execution-upgrades-and-rollout.md); `decide()`'s ordered checks in [05-welfare-and-decision-engine.md](05-welfare-and-decision-engine.md)). The frontend never predicts an outcome the runtime would not re-derive; the runtime is always the final arbiter (§11.4).
2. **The E-row discipline.** Every user-visible degradation state is specified as a row with six mandatory facets: **V** visible state · **L** loading behavior · **A** verified data available · **U** unavailable convenience data · **F** failure message · **R** recovery (§11.12).
3. **Signing-flow safety.** `Finalized` is the only success state; the confirm screen is decoded from the exact payload bytes (`prep.scaleHex`), never from form state; addresses render with identicon + checksummed SS58 (prefix 7777, *(normative value: [02](02-integration-contract.md))*); metadata-hash signing is the second display channel; dispatch outcomes are decoded from finalized events only (§11.3).

---

## 11.2 Screen inventory

The canonical frontend serves **every** protocol workflow — including the values layer and operator workflows (D-11) and USDC funding (D-12). "All state light-client readable" is a hard requirement for every row: each screen's reads appear in its precondition/query table below, and none requires an archive node, indexer, or RPC (INV-FE-4 stands unamended).

| # | Screen / workflow | Area | Primary reads | Extrinsics | Spec |
|---|---|---|---|---|---|
| S1 | Epoch & phase header, sudo banner | global | `Epoch.EpochOf`, `Constitution.PhaseFlags` | — | §11.10, [10](10-frontend-architecture.md) |
| S2 | Proposal list / detail (+ ratification status) | core | `Epoch.Proposals`, `proposal_summaries()`, finalized-only `decision_stats()`, `Preimage.PreimageFor`, `Referenda.ReferendumInfoFor` | — | §11.7.4 |
| S3 | Market trading (decision, gate, Baseline books) | core | `Market.Markets`, `BaselineMarketOf`, `quote()` + client LMSR cross-check | `market.buy/sell` | §11.5 |
| S4 | Positions / transfer / redeem (incl. **Voided**) | core | `Positions` prefix / `account_positions()`, `Vaults(pid)`, `BaselineVaults(epoch)` | `ledger.split/merge/split_scalar/merge_scalar/transfer/redeem/redeem_scalar/redeem_scalar_pair/redeem_void/redeem_baseline/redeem_baseline_pair` | §11.5, §11.6 |
| S5 | Submit proposal | core | `epoch_status()`, `IntakeQueue`, class bond params, preimage flow | `epoch.submit/withdraw`, `preimage.note_preimage`, `preimage.request_preimage` | §11.5 |
| S6 | Execution queue + execute | core | `ExecutionGuard.Queue`, `execution_queue()` | `execution_guard.execute` | §11.5 |
| S7 | Welfare & constitution dashboard | core | `welfare_current()`, `params()`, `Welfare.Snapshots`, `MetricSpecs`, `GateBreachFlags` | — | [05](05-welfare-and-decision-engine.md) |
| S8 | Recent settlements | core | `recent_cohorts()`, `ExecutionRecords` ring | — | [02](02-integration-contract.md) |
| S9 | Referenda list / detail (six tracks) | **FE-14** | §11.7.2 | `referenda.submit`, `place_decision_deposit`, refunds | §11.7 |
| S10 | Vote / delegate / undelegate / unlock | **FE-14** | §11.7.2 | `conviction_voting.*` | §11.7 |
| S11 | OracleResolution ballot | **FE-14** | §11.7.5 | `conviction_voting.vote` | §11.7.5 |
| S12 | Funding: deposit (Asset Hub → chain) | **funding** | AH light-client reads §11.9 | AH `pallet_xcm` reserve transfer | §11.9 |
| S13 | Funding: withdraw (chain → Asset Hub) | **funding** | local balances, XCM health flag | local `pallet_xcm.reserve_transfer` | §11.9 |
| S14 | Reporter console (register, report, recompute proofs, evidence) | **FE-15** | §11.8.1 | `oracle.register_reporter/report/challenge/recompute_proof` | §11.8.1 |
| S15 | Guardian console (5-of-7 signing, allowances, retro-ratification) | **FE-15** | §11.8.2 | `guardian.*` per [06](06-governance-and-guardians.md) | §11.8.2 |
| S16 | Treasury: stream claims + `nav()` view (haircut flag) | **FE-15** | `nav()`, stream storage per [02](02-integration-contract.md) | `futarchy_treasury.claim_stream` | §11.8.3 |
| S17 | Upgrade crank (`apply_authorized_upgrade`) | **FE-15** | authorized-hash storage, release artifact | `system.apply_authorized_upgrade` | §11.8.4 |
| S18 | Welfare snapshot crank | **FE-15** | snapshot staleness | `welfare.snapshot(epoch)` | §11.8.5 |
| S19 | Incident / Milestone registry: file + challenge | **FE-15** | `pallet-registry` storage per [07](07-oracle-and-disputes.md) | `registry.file_incident/file_milestone/challenge` | §11.8.6 |
| S20 | Balances & funding status | core | `System.Account`, `ForeignAssets.Account(USDC_LOCATION, who)` | — | [02](02-integration-contract.md) |

USDC balance reads use the `ForeignAssets` instance keyed by the pinned XCM `Location` (D-17, frozen in [02](02-integration-contract.md), incl. the `[VERIFY asset index 1337]` that lives there) — never `Assets.Account`.

**Finalized decision statistics are not a trading preview.** `decision_stats(pid)` is available only after the registered decision windows are sealed and every decision-path input is evaluable ([02](02-integration-contract.md) §3). S2 MAY render it only as finalized decision statistics; while a proposal remains in Trade/Extended and the view returns `None`, S2 and S3 MUST render no projected uplift, projected PASS/REJECT, or other in-Trade preview derived from it.

**Epoch shrink visibility.** The epoch dashboard MUST ingest `SlotsShrunk { epoch, requested, funded, dropped }` and render `requested` and `funded` as proposal-slot counts plus the dropped proposal IDs — never as USDC amounts ([08](08-treasury-and-economics.md) §4.4). A shrink is a visible capacity event, not an absent/zeroed slot.

**Position-state projection.** Proposal positions render `VaultState::ScalarSettled { winner, s }`; `PositionId::Baseline` renders the branch-free `VaultState::BaselineSettled { s }`. The positions and redemption screens MUST NOT fabricate or display a winning branch for a Baseline vault; they route that state to `redeem_baseline` / `redeem_baseline_pair` ([03](03-conditional-ledger.md) §5.3).

**Forecast trading is cut (D-8).** Books close at branch resolution and never reopen. No screen, route, precondition row, or copy in this document refers to post-resolution ("forecast") trading; the residue in FE §17.6/§14.1 is removed. S3 trades only while the owning proposal is in `Trading`/`Extended` (Baseline books: while the epoch trading window is open — see §11.5 row P-2).

FE-15 lives under an explicit **"Advanced"** navigation area: same trust rules, same precondition discipline, denser information, no simplified summaries. It is part of the canonical release, not a separate app.

---

## 11.3 Transaction construction and signing safety

Carried forward from FE §17.1–§17.3, §17.5, §17.7–§17.8 with the fee repair of D-12:

- **Signers.** Injected PJS-compatible extensions via `polkadot-api/pjs-signer` **[VERIFY exact export names on PAPI 2.1.x — FE-P1]**; raw-payload flow (QR/hex + metadata-hash mode) for air-gapped and hardware signers **[VERIFY Ledger Generic App + CheckMetadataHash flow for a custom chain — FE-P6]**. Multisig via `Multisig.as_multi` with approval state read from `Multisig.Multisigs`; proxies supported as call wrappers under the same precondition system.
- **Lifecycle.** Draft → Prepared(at B) → Refreshing → {Blocked | AwaitingSignature(at B′)} → Broadcast → InBestBlock → **Finalized** (only success state) | Dropped | Retracted. Post-finalization the app decodes the extrinsic's events to distinguish inclusion from call success and renders dispatch errors with human text (e.g. `market.buy failed: MaxCostExceeded — you paid nothing`).
- **Mortality/nonce.** Era 64 blocks from B′ (256 for raw-external); nonce from finalized `System.Account(who).nonce` at B′ plus tracked in-flight increments; phase-boundary proximity warning when a relevant boundary is < 25 blocks away.
- **Fee currency selector (D-12, X-14 resolved).** Fees are payable in VIT or USDC via `pallet-asset-tx-payment`; the conversion binds to the constitution key **`fee.vit_usdc_rate`** (typed, bounded [0.1×, 10×] around its reference, PARAM-adjustable — *(normative value: [08](08-treasury-and-economics.md)/[13](13-parameters.md))*). The selector reads this key live (a `Constitution.Params` storage read, light-client verified), shows the estimate in both currencies, and recomputes on selection. USDC-only accounts are always viable: every precondition table below computes fee headroom in the *selected* fee asset. The rate key and its bounds MUST be displayed in expert mode.
- **Anti-substitution.** The confirm screen derives its human summary by decoding `prep.scaleHex` — the exact bytes to be signed — never from form state; the wallet's metadata-hash decode is the independent second channel.
- **Dry-run.** No general dry-run through the light client; the precondition system statically checks every failure mode the runtime would (per-call tables, §11.5–§11.9); expert mode allows dry-run via the quarantined RPC fallback labelled "unverified simulation", never gating success.

---

## 11.4 Pre-sign refresh (INV-FE-2)

```ts
export async function refreshAndGate<T>(prep: TxPreparation<T>): Promise<Gate> {
  const at = await client.getFinalizedBlock();                 // B' — single pin
  const rt = await api.runtimeVersionAt(at.hash);              // compat gate (doc 10)
  if (rt.spec_version !== prep.builtFor.specVersion) return blocked('FE-TX-007', rt);
  const results = await Promise.all(prep.preconditions.map(p => p.evaluateAt(at.hash)));
  const failed = results.filter(r => !r.ok);
  return failed.length ? blocked('FE-TX-004', failed /* diff view */)
                       : proceed({ at, results });
}
```

Rules (normative):

1. Every submit path passes through `refreshAndGate` — structurally (the tx machine has no bypass edge), not by convention.
2. Each precondition row is an **exact read at B′**: a storage key, a runtime-API call, or a **constants-API (metadata) read**. Values that the backend defines as kernel constants with *no storage representation* — per-trade min/max, `MinSplit`, `MinTransfer`, `MaxPositionsPerAccount`, and every §21-class tunable's kernel floor — MUST be read from the runtime **constants API** exposed per D-2/[02](02-integration-contract.md), never from storage and never hardcoded (X-11e, X-11h). Constants are re-read whenever the compat layer observes a new `spec_version` (they can only change via Wasm change).
3. Expected/actual values render in the confirm screen; expert mode shows raw keys and SCALE values (INV-FE-14).
4. Provider/local-index data never satisfies any precondition (INV-FE-3); every row reads chain state.
5. A precondition failure shows a diff (expected vs. actual at B′) and returns to Draft with form state preserved.

---

## 11.5 Precondition tables — core protocol calls

Each row = the exact re-reads at B′. `[C]` marks a constants-API read; everything else is a storage or runtime-API read at B′.

| # | Tx | Preconditions re-read at B′ |
|---|---|---|
| P-1 | `market.buy/sell` (decision & gate books) | owning proposal state ∈ {`Trading`, `Extended`} — **only**; market phase Open; book `q_L, q_S, b` (recompute cost via client LMSR using the frozen `Market::Fee` bps metadata constant; cross-check it against raw `params(mkt.fee)` by floored `Perbill / 100,000`; recheck `max_cost`/`min_proceeds` still satisfiable); `quote()` vs. client recompute agree within the fixed-point bounds (else `FE-CHAIN-005`, trading blocked); user USDC balance (buy) / position balance (sell); per-trade min/max `[C]`; `Constitution.PhaseFlags` trading-enabled bit set; no PB-LEDGER-FREEZE active ([06](06-governance-and-guardians.md)) |
| P-2 | `market.buy/sell` (**Baseline book**) | `BaselineMarketOf(epoch)` exists (D-2/X-10) and its contract-v8-coextensive `Markets[id]` Baseline book exists; either absent blocks trading (SQ-304, [04](04-markets-and-pricing.md) §8.3); epoch trading window open — Trade phase d5–d18 *(normative value: [13](13-parameters.md))* **or** any epoch-e decision pair still in `Extended` (the Baseline book stays open through the last epoch-e decision incl. per-pair extensions, [04](04-markets-and-pricing.md) §8.4); `BaselineVaults(epoch)` open ([03](03-conditional-ledger.md)); book state + slippage recheck as P-1; per-trade min/max `[C]`; PhaseFlags trading-enabled; no PB-LEDGER-FREEZE |
| P-3 | `ledger.split` / `split_scalar` | vault `Open`; USDC balance ≥ amount + fee headroom (in selected fee asset); `MinSplit` `[C]`; caller position count < `MaxPositionsPerAccount` `[C]` for each newly created position key; no PB-LEDGER-FREEZE |
| P-4 | `ledger.merge` / `merge_scalar` | vault ∈ {`Open`, `Resolved`, **`Voided`**} (merge is available in every non-`ScalarSettled` state — the D-1 par path, [03](03-conditional-ledger.md) §5.1); user holds ≥ amount of the complete pair being merged (both sides re-read); payout = amount USDC at par, displayed |
| P-5 | `ledger.redeem` (branch-USDC) | vault `ScalarSettled{winner, s}` **only** — `Resolved` admits no unpaired redemption (outflow monotonicity, [03](03-conditional-ledger.md) §2.3; `merge` is the par path there, row P-4); user holds winning-branch USDC ≥ amount; payout 1:1 displayed. *(Not applicable under `Voided` — see §11.6; the old "winning-position balance" requirement is deleted for VOID.)* |
| P-6 | `ledger.redeem_scalar` / `redeem_baseline` | proposal vault `ScalarSettled { winner, s }`, or Baseline position view `BaselineSettled { s }`; settlement `s` present; user LONG/SHORT balance ≥ amount; expected payout recomputed and displayed: LONG `floor(a·s)`, unpaired SHORT `floor(a·(1−s))` *(normative values: [13](13-parameters.md))*; only the proposal-vault path renders/uses `winner` |
| P-7 | `ledger.redeem_scalar_pair` / `redeem_baseline_pair` | proposal vault `ScalarSettled`, or Baseline position view `BaselineSettled`; user holds ≥ a of **both** LONG and SHORT (within the proposal's winning branch when applicable); payout exactly `a`, displayed ([03](03-conditional-ledger.md), B-5) |
| P-8 | `ledger.redeem_void` | see §11.6 table |
| P-9 | `ledger.transfer` | vault ∈ {`Open`, `Resolved`, `Voided`}; recipient is not a protocol account; recipient position count < `MaxPositionsPerAccount` `[C]`; `MinTransfer` `[C]`; per-entry deposit 0.1 USDC headroom *(normative value: [13](13-parameters.md))* |
| P-10 | `epoch.submit` | `Epoch.EpochOf.phase == Intake`; `IntakeQueue` len < 64 *(normative value: [13](13-parameters.md))*; caller's intake entries this epoch < 4 (rate limit, [06](06-governance-and-guardians.md)); class bond balance; preimage noted with matching hash + len **and pinned via `preimage.request_preimage`** ([06](06-governance-and-guardians.md), B-13); resource-domain validity vs. constitution tables; **warning surfaced**: preimage-missing cancellation slashes 10% of the bond, non-decision-grade outcomes slash 10% (to INSURANCE) — the old "full refund" copy is removed |
| P-11 | `epoch.withdraw` | proposal in `Submitted`, caller is proposer, before Qualify |
| P-12 | `execution_guard.execute` | **complete dispatch-time list — see below** |
| P-13 | `oracle.report` | round open; report window not elapsed; caller in reporter registry with full `ReporterStake` held; round bond balance — bond = `max(flat_floor, bps × cohort_escrow)` recomputed and displayed ([07](07-oracle-and-disputes.md)); evidence hash provided |
| P-14 | `oracle.challenge` | round open; challenge window (72 h *(normative value: [13](13-parameters.md))*, incl. any watchtower-quorum extension) not elapsed; matching escalation bond balance (doubles per round, value-scaled floor) |
| P-15 | crank calls (`epoch.tick`, `market.crank_observe`, `market.reap`, `epoch.settle_cohort`, …) | corresponding staleness precondition true at B′, else "no-op — nothing to crank" (never sign a guaranteed no-op without an explicit expert override) |

### `execution_guard.execute` — the complete precondition row (X-11i resolved)

The frontend re-checks **every** check the backend performs at dispatch time ([09](09-execution-upgrades-and-rollout.md)); the FE row previously omitted ratification, meters, resource locks and gate flags. All of the following are read at B′:

| Check | Read at B′ |
|---|---|
| 1. Queued, not cancelled | `ExecutionGuard.Queue(pid)` fields |
| 2. Window | `maturity ≤ now ≤ grace_end` vs. finalized height |
| 3. Preimage | `Preimage.PreimageFor(payload_hash, len)` present; client re-hashes and compares to the trading-time commitment |
| 4. Runtime version | `RuntimeVersionConstraint` == live `spec_name`/`spec_version` |
| 5. **Ratification (CODE/META and ratify-required classes)** | linked `ratify`-track referendum is `Approved` — the single **execute-time deadline** of D-5 ([06](06-governance-and-guardians.md)); missing/unpassed ⇒ `execute` **errors `NotRatified` without changing proposal state** (it stays `Queued` and stays retryable until `grace_end`); only after `grace_end` does the T16 crank record `RejectReason::NotRatified` (SQ-163). The FE blocks pre-sign either way, but MUST NOT present a pre-grace failure as terminal — the ratification panel of §11.7.4 stays actionable |
| 6. **Attestation presence (CODE/META)** | the committed attestation records still exist, unrevoked, with no challenge resolved against them — a **record** check, not a live-registry check; the ≥ 2-of-3 quorum over the bonded registry is a *queue-time* precondition already discharged (mirrors 09 §1.2(5) item-for-item per X-11i; re-sited 2026-07-20, SQ-97) ([06](06-governance-and-guardians.md)/[09](09-execution-upgrades-and-rollout.md)) |
| 7. **Capability rules** | call domains of the decoded batch ⊆ declared domains; each domain's `CapabilityRule` admits the class origin (`Constitution` capability table) |
| 8. **Rate meters** | treasury meters (per-proposal ≤ 5% NAV; 30-day ≤ 10%; 180-day ≤ 30% *(normative values: [13](13-parameters.md))*) and issuance meters have headroom for `meters_declared` |
| 9. **Resource locks** | `Epoch.ResourceLocks` still held by `pid` for every declared domain |
| 10. **Guardian suspension** | no `delay_once` suspension, no active `suspend_on_gate` freeze |
| 11. **Gate flags** | no active hard-gate daily breach flag in `Welfare.GateBreachFlags` |
| 12. **Dead-man / freeze** | dead-man switch not engaged; PB-LEDGER-FREEZE not active |
| 13. Batch bounds | decoded batch ≤ 16 calls, ≤ 64 KiB, declared weight ≤ 25% block limit *(normative values: [13](13-parameters.md))*; SafetyFilter closure over nested wrappers incl. `proxy_announced`, `as_multi_threshold_1` (static check on the preimage) |
| 14. **Descriptor lead time (CODE/META)** | `now ≥ authorized_at + DescriptorLeadTime` (43,200 blocks = 72 h *(normative value: [13](13-parameters.md))*) per D-14/[09](09-execution-upgrades-and-rollout.md) |

The FE renders each of the 14 checks as a named row with expected/actual; any failure blocks with the same reason code the runtime would return. This table and the backend's list in [09](09-execution-upgrades-and-rollout.md) MUST stay in lockstep; the contract tests in [15-invariants-and-testing.md](15-invariants-and-testing.md) diff them.

---

**Reaped Baseline books (normative; SQ-304; contract v11).** Successful market reap removes the Baseline book and its `BaselineMarketOf(e)` entry atomically. When cohort history identifies epoch `e` but that mapping is absent, the UI MUST label the book **reaped/archived**, MUST NOT render a missing or fail-closed zero quote as a market price, and MUST disable every trade action on it; cohort history continues to render from `RecentCohortSummaries`. A present mapping with an absent or mismatched book is corrupt chain state and triggers the compatibility hard block. Redemption of already-held Baseline positions is unaffected — it reads the vault, not the book.

## 11.6 VOID redemption workflow (X-6, D-1)

The redeem screen (S4) handles the `VaultState::Voided` state end-to-end. On `VaultVoided` (event frozen in [02](02-integration-contract.md)) the position card for that vault switches to the VOID layout:

1. **Primary action — `merge`, and only `merge`, is the 100 % path.** It is offered as "Merge pairs → 100 % recovery" **only** when the user holds an **Accept+Reject branch-USDC** pair, which is the sole holding that redeems at par under VOID. The screen computes the maximal cross-branch pairable amount across their positions and pre-fills it. A user who holds no cross-branch pair MUST NOT see a 100 % headline (SQ-171).

1a. **Consolidation step — `merge_scalar` / `merge_gate` are NOT recovery actions.** A same-branch LONG+SHORT or YES+NO set pays **no USDC**: merging it climbs the set to one *same-branch* branch-USDC. That output is worth par only once paired with the opposite branch, and `floor(a/2)` otherwise. The UI MUST present these calls as value-neutral consolidation, label the resulting branch-USDC with its actual VOID value, and route the user to whichever of step 1 or step 2 their post-merge holdings actually reach. Presenting `merge_scalar` under a 100 %-recovery heading is exactly the overstatement this section forbids.
2. **Secondary action — `redeem_void(pid, kind, amount)`** for genuinely unpaired holdings, with the rates shown **honestly and prominently**: unpaired branch-USDC pays `floor(a/2)`; unpaired LONG or SHORT pays `floor(a/4)` *(normative values: [13](13-parameters.md))*. Copy (normative intent, exact wording localizable): *"This vault was voided. Cross-branch complete pairs (Accept **and** Reject) recover 100% by merging; a same-branch LONG+SHORT set alone does not — merging it yields one branch-USDC, worth 0.5 under VOID. An unpaired single-branch position redeems at 0.5 per branch-USDC (0.25 per LONG/SHORT) — the value of a voided binary claim."* No copy may describe the 0.5/0.25 rates as a penalty or confiscation — they are the neutral price of a voided binary claim. **Nor may any copy promise "par" or "full principal" to a wrapper buyer unless the displayed holdings are complete across both branches (SQ-171).** The screen MUST show the **pair-first D-1 neutral recovery of the user's actual package** and explain that any difference from the original debit is the premium or discount of their realized average execution price against the neutral prior, plus fees. Cross-branch complete-set holders do recover par; a buyer recovers `cost` only if their *realized average execution price* was 0.5, which a finite buy opening at that quote does not achieve.
3. **Mixed holdings**: the screen decomposes the user's balances into (max **cross-branch** pairs, at par) + (same-branch sets, consolidated by step 1a) + (residual unpaired amounts, at the step-2 rates) and offers the actions in one flow. The headline figure MUST be the **total recovery those actual holdings reach**, never the par value of the pairs alone.
4. Rounding is against the redeemer; residues follow the dust rule ([03](03-conditional-ledger.md)); the displayed payout is the exact floor computation.

Precondition rows:

| Tx (under `Voided`) | Preconditions re-read at B′ |
|---|---|
| `ledger.merge` / `merge_scalar` | vault state == `Voided` (or `Open`/`Resolved`); user holds ≥ amount of **both** sides of the pair; payout = amount USDC (par), displayed |
| `ledger.redeem_void(pid, kind, amount)` | vault state == `Voided`; user balance of `kind` ≥ amount; expected payout recomputed and displayed (`floor(a/2)` branch-USDC; `floor(a/4)` LONG/SHORT); **no winning-position-balance requirement** — that requirement applies only to P-5/P-6 under `Resolved`/`ScalarSettled` and is explicitly absent here |

There is no "winning branch" under VOID; any UI element that gates redemption on a winning position MUST NOT render for a `Voided` vault. See E-row E16 (§11.12).

---

## 11.7 FE-14 — Governance surface (X-2, D-11)

The values layer is served by the canonical frontend. All state involved is bounded and light-client readable; the storage enumeration below is the CRITICAL_SURFACE addition for the compat probes of [10](10-frontend-architecture.md).

### 11.7.1 Screens

| Screen | Contents |
|---|---|
| Referenda list | All referenda across the six tracks (`metric`, `constitution`, `entrenched`, `guardian`, `ratify`, `oracle` — [06](06-governance-and-guardians.md)); filter by track/status; per-row: track, status (Preparing/Deciding/Confirming/Approved/Rejected/TimedOut/Cancelled), approval & support curves vs. current tally, decision-period countdown |
| Referendum detail | Full call decode from preimage (undecodable ⇒ structured-unknown per [10](10-frontend-architecture.md), never guessed); track parameters; tally with conviction breakdown; enactment schedule; user's own vote if any |
| Vote | AYE/NAY/abstain + split; conviction 1×–6× with the resulting lock duration (up to 32 weeks *(normative value: [13](13-parameters.md))*) displayed **before** signing |
| Delegation | per-class delegate/undelegate with conviction; current delegations listed |
| Unlock | per-class expired locks, computed unlock blocks, one-click `unlock` |
| Ratification panel (on proposal detail, S2) | §11.7.4 |
| OracleResolution ballot | §11.7.5 |

### 11.7.2 Queries (all light-client readable; storage items enumerated)

| Read | Item |
|---|---|
| Referendum enumeration | `Referenda.ReferendumCount` + `Referenda.ReferendumInfoFor(index)` (live set bounded; terminal entries carry final tally); `Referenda.TrackQueue(track)`, `Referenda.DecidingCount(track)` |
| Track constants | `[C]` referenda track table (constants API — deposits, curves, periods) |
| Referendum calls | `Preimage.StatusFor` / `Preimage.PreimageFor(hash, len)` — client re-hashes bytes |
| User votes & delegations | `ConvictionVoting.VotingFor(who, class)` |
| User locks | `ConvictionVoting.ClassLocksFor(who)` + lock expiry derivation |
| Enactment | `Scheduler.Agenda` (display only — the FE never infers execution from schedule presence) |
| Ratification linkage | `ExecutionGuard.Queue(pid)` commitment + scan of `ratify`-track referenda whose call is `ratify(pid, …)` (bounded: live referenda only); after a referendum is submitted, the proposer binds its exact index with `epoch.bind_ratification` before queue admission or while queued |
| Oracle ballots | `open_oracle_rounds()` + oracle round storage ([02](02-integration-contract.md) names) |

### 11.7.3 Extrinsics and precondition rows

| # | Tx | Preconditions re-read at B′ |
|---|---|---|
| G-1 | `conviction_voting.vote(poll_index, vote)` | referendum status `Ongoing`; vote balance ≤ free VIT; conviction lock duration displayed; **oracle track: snapshot rule of §11.7.5 evaluated and surfaced** |
| G-2 | `conviction_voting.delegate(class, to, conviction, balance)` | no direct votes recorded in `VotingFor(who, class)` (else offer `remove_vote` first); balance ≤ free VIT; target address reviewed per §11.3 anti-substitution |
| G-3 | `conviction_voting.undelegate(class)` | currently delegating in class |
| G-4 | `conviction_voting.remove_vote(class, index)` | vote exists; referendum ended or removal allowed |
| G-5 | `conviction_voting.unlock(class, target)` | computed unlock block ≤ now (else blocked with the exact remaining lock time) |
| G-6 | `referenda.submit(track_origin, proposal, enactment)` | track deposit balance `[C]`; call admissible for the track's `Contains` filter (statically checked against the frozen admissible-call sets of [06](06-governance-and-guardians.md)); preimage noted; for the `ratify` track: the referenced proposal's artifact commitment exists (submittable any time after commitment at proposal submission, D-5) |
| G-7 | `referenda.place_decision_deposit(index)` | referendum in `Preparing`; deposit balance `[C]` |
| G-8 | `referenda.refund_submission_deposit` / `refund_decision_deposit` | referendum terminal; refund available |
| G-9 | `epoch.bind_ratification(proposal_id, referendum_index)` | caller is the proposal's proposer; proposal is non-terminal CODE/META; referendum is live on the `ratify` track with the scoped `ConstitutionalValues` origin; its exact preimage decodes to `execution_guard.ratify(proposal_id, referendum_index)`; the artifact commitment is unchanged; repeated binding of the same index is idempotent |

For G-6 on the `ratify` track: the `ratify(proposal_id, referendum_index)` call signature is frozen in [02](02-integration-contract.md)/[06](06-governance-and-guardians.md); the FE pre-computes the prospective `referendum_index` from `ReferendumCount` and warns that an interleaving submission changes the index (rebuild-and-resubmit flow, same as a nonce race).

After G-6 creates a ratification referendum, the proposal's proposer MUST submit G-9 with the exact index; an unbound referendum cannot pass through the guard's `ratify` call. Before queue admission, the pending binding is an internal join outside the contract surface, so the panel MUST keep G-9 actionable until the transaction is finalized and the queued `ratify_ref` becomes readable. A proposal with no submitted referendum may still queue without a `ratify_ref`; once the queue entry carries `ratify_ref`, the FE shows that identity as frozen even while the referendum is still ongoing.

### 11.7.4 Ratification status on proposal detail

For every proposal whose class requires values ratification (CODE/META per [06](06-governance-and-guardians.md)), the proposal detail screen (S2) MUST render a ratification panel:

- linked `ratify`-track referendum (or "none submitted yet — anyone may submit; after submission the proposer binds its exact index" with one-click G-6/G-9 flows; the artifact hash was committed at proposal submission);
- its live status and tally;
- the **execute-time deadline** (D-5, [06](06-governance-and-guardians.md)): *"must be Approved by the moment `execute()` is dispatched; execution window: maturity → grace_end"* with both block numbers and countdowns;
- if the referendum cannot mathematically pass before `grace_end` (decision + confirm periods vs. remaining window), an explicit warning: *"ratification can no longer complete inside the execution window — this proposal will reject with `NotRatified`."*

The guard status is not the referendum lifecycle source. `RatificationStatus::NoPassedRecord` means only that the guard has no passed-ratification record; it deliberately does not distinguish never submitted, submitted-but-unbound, submitted and ongoing, or submitted and failed. The panel MUST derive those lifecycle states from `pallet-referenda` (`ReferendumInfoFor`, calls and tallies above). `RatificationStatus::Passed { referendum }` supplies the guard's passed-record linkage after approval; it does not replace the referendum read.

### 11.7.5 OracleResolution ballot and the pre-cohort snapshot rule

Terminal (round-3) oracle disputes escalate to the hardened `oracle` track: 60% approval / 10% support / 7-day *(normative values: [13](13-parameters.md); D-18)*, only admissible call `oracle.adjudicate(component, epoch, spec_version, value, reporter_wrong)`. The tally uses a **pre-cohort conviction snapshot**: only VIT conviction-locked **before the subject cohort's creation block** counts; capital that entered after the cohort began is excluded ([06](06-governance-and-guardians.md)).

The ballot screen MUST:

1. show the dispute lineage: component, epoch, all round reports/challenges with bonds, and evidence links (§11.8.1 evidence rules);
2. display the snapshot block (cohort creation) and the **user's effective voting power at that snapshot** — reading the user's conviction-lock history from `ConvictionVoting.VotingFor`/`ClassLocksFor` as of the snapshot block **[VERIFY the snapshot mechanism's exact readable storage representation — whether the runtime stores a snapshot map or re-derives from lock timestamps; frozen in [02](02-integration-contract.md); the FE binds to whichever representation the contract freezes]**;
3. pre-sign, warn (and show effective power = 0) when the user's locks post-date the snapshot — the vote would be signable but weightless;
4. never present the ballot as a routine vote: copy states it is the stake-weighted backstop that makes earlier-round lying unprofitable ([07](07-oracle-and-disputes.md)).

### 11.7.6 Required-UX statements (E-row discipline; full rows in §11.12)

- Governance state renders with the same `Verified<T>` provenance badges as market state; a tally is never shown from provider data.
- A referendum in `Confirming` MUST display the confirm-period abort semantics (support dropping below the curve restarts confirmation).
- Vote/delegate confirm screens MUST display the lock consequence (amount, conviction, unlock date) above the fold.

---

## 11.8 FE-15 — Operator surface (X-12, D-11)

The "Advanced" area. Every workflow below is light-client readable and follows §11.4 discipline.

### 11.8.1 Reporter console

| Tx | Preconditions re-read at B′ |
|---|---|
| `oracle.register_reporter()` | free USDC ≥ `ReporterStake` (100,000 USDC *(normative value: [13](13-parameters.md))*); not already registered; stake-hold consequence displayed |
| `oracle.report` / `oracle.challenge` | rows P-13/P-14 (§11.5) |
| `oracle.recompute_proof(component, epoch, spec_version, proof)` | round open and the consumed MetricSpec component permits deterministic recomputation ([07](07-oracle-and-disputes.md)); the FE recomputes the proof result locally from the committed raw data before submission and blocks on mismatch — never submit a proof the client's own recomputation contradicts |

**Dispute-evidence display.** Evidence is content-addressed (`evidence_hash`): the console fetches from the operator-funded evidence hosting ([12](12-release-and-operations.md), D-16) and any user-supplied gateway, **re-hashes the received bytes and compares to `evidence_hash` before rendering**; mismatch or unavailability renders as "evidence unretrievable — treated as absent by the protocol" ([07](07-oracle-and-disputes.md)), never as silent omission. Evidence bytes are rendered as text/structured data only, never HTML.

### 11.8.2 Guardian console (5-of-7 signing flow)

The system's most privileged actors get a specified signing tool. Approval aggregation lives inside `pallet-guardian` (not `pallet-multisig`); the console reads guardian storage (membership ≤ 7, per-power allowances, pending actions — item names frozen in [02](02-integration-contract.md)) and provides:

| Element | Behavior |
|---|---|
| Pending actions list | each pending guardian action: power, target, `justification_hash` (+ resolved justification document via the evidence rules of §11.8.1), current approvals m-of-7, expiry |
| Approve flow | `guardian.approve_action(action_id)` (call name per [06](06-governance-and-guardians.md) §5.1); preconditions: caller is a member; action pending and unexpired; caller has not already approved; the approval renders the **exact enumerated call batch** being approved (playbooks are preimage-committed enumerated batches — decoded and displayed, never summarized away) |
| Propose flow | power-specific forms for `pause_intake`, `delay_once`, `force_rerun`, `activate_playbook`, `suspend_on_gate`; preconditions: allowance remaining for the power (allowance meters displayed); playbook activation additionally requires its on-chain trigger condition (gate breach flag / depeg / dead-man / VOID / I-4 drift flag for PB-LEDGER-FREEZE) to be **verifiably active at B′** — the trigger read is part of the precondition row |
| Ratification tracker | every executed action's auto-scheduled `ratify`-track retrospective review, with the 50%-bond-slash + recall consequence of an unratified action stated |

### 11.8.3 Treasury: stream claims and `nav()`

| Tx | Preconditions re-read at B′ |
|---|---|
| `futarchy_treasury.claim_stream(stream_id)` | stream exists and not cancelled; caller is recipient; claimable amount (linear vesting, computed client-side from stream fields at B′) > 0 and displayed |

**`nav()` view (rendered, at last).** The treasury screen renders every [02 §4](02-integration-contract.md) `NavView` field: the account decomposition, stream remainders, obligations, `total`, `spendable_nav`, `meter_utilization_bps`, and the Param/Treasury/Code/Meta `class_floors` (including continuous distance-to-floor). In-flight XCM and VIT holdings remain marked 0 with copy explaining the conservative rule. **Reserve-haircut flag**: when the reserve-health trigger R is set ([07](07-oracle-and-disputes.md)/[08](08-treasury-and-economics.md) — e.g. a frozen USDC sovereign account), `haircut_flag` is exactly 08 §1.2's `reserve_impaired` and `spendable_nav` is 0; the FE MUST replace the headline NAV with the haircut presentation and a persistent banner *"reserve health degraded — NAV shown with haircut; split inflows halted (PB-RESERVE)"*. The FE never renders full backing while the flag is set.

### 11.8.4 Upgrade crank — `system.apply_authorized_upgrade`

Permissionless and load-bearing for liveness; the Advanced area makes it executable from the canonical frontend:

1. Read the authorized upgrade state: the authorized code hash from `parachain-system` storage, and `PendingUpgrade { hash, authorized_at, applicable_at }` from the execution guard — `authorized_at` is recorded when `execute()` dispatches `authorize_upgrade`, and the SafetyFilter denies `apply_authorized_upgrade` until `applicable_at = authorized_at + DescriptorLeadTime` ([09](09-execution-upgrades-and-rollout.md) §2.2; [02](02-integration-contract.md) names).
2. Fetch the matching Wasm from the **Arweave release artifact** published by the backend release train ([02](02-integration-contract.md) test/release artifact feed; [12](12-release-and-operations.md)) — via the same multi-gateway, hash-verified retrieval as app assets.
3. **Verify the artifact hash against the authorized hash BEFORE submission** — client-side, streaming BLAKE2b-256 over the downloaded bytes; a mismatch hard-blocks with `FE-UPG-001` and never reaches the wallet.
4. Precondition row: authorized hash present; artifact hash == authorized hash (step 3); `now ≥ authorized_at + DescriptorLeadTime` (D-14); fee headroom for a multi-MB extrinsic (displayed — it is large).
5. Submit `system.apply_authorized_upgrade(code)`.

**Memory/streaming honesty.** Hashing streams in bounded chunks, but *submission* requires the full Wasm (typically 1.5–5 MB compressed) in memory as a call argument, and the signed extrinsic transits the light client's transaction pool path. This is architecturally heavier than any other FE extrinsic. **[VERIFY smoldot + PAPI behavior for multi-MB extrinsic submission — pool/gossip size limits, peer banning on oversized transactions, and mobile memory headroom; prototype FE-P10 (extends FE-P4). Fallback if it fails: the Advanced screen performs steps 1–3 (fetch + verify) and hands the verified blob + prebuilt call data to the expert RPC path or an operator CLI (`tools/` in [12](12-release-and-operations.md)) — verification stays in-browser even when submission cannot.]** The screen states which path is active.

### 11.8.5 Welfare snapshot crank

`welfare.snapshot(epoch)`: precondition — epoch boundary passed and snapshot for `epoch` not yet taken (staleness read at B′); otherwise "no-op — nothing to crank" (row P-15). The Advanced dashboard shows snapshot staleness prominently (an overdue snapshot > 4 days engages the dead-man rule — [05](05-welfare-and-decision-engine.md)).

### 11.8.6 Incident / Milestone registry ([07](07-oracle-and-disputes.md))

| Tx | Preconditions re-read at B′ |
|---|---|
| `registry.file_incident(...)` / `registry.file_milestone(...)` | filing bond balance (value-scaled per [07](07-oracle-and-disputes.md)); registry bounds not exceeded; evidence hash provided (evidence rules of §11.8.1 apply to display) |
| `registry.challenge(filing_id)` | filing within its 72 h challenge window (incl. watchtower-quorum extension state, displayed); challenge bond balance |

Registry state (filings, challenge windows, watchtower acknowledgments, slash outcomes) renders in the Advanced area with countdowns; registry sub-games hold settlement, never decisions — the copy states this ([07](07-oracle-and-disputes.md)). The registry-window history and countdown adjustments MUST source `WindowAcknowledged { epoch, filing_id, watchtower }` and `WindowExtended { epoch, filing_id, new_deadline }` from the **registry** event stream frozen in [02 §6](02-integration-contract.md), alongside the filing storage read. They are distinct from the oracle events of the same names, whose fields carry `component`/`round`; the ingest filter MUST bind the pallet as well as the variant name.

---

## 11.9 Funding flow (X-8, D-12)

USDC funding is **in scope** for the canonical frontend and ships in the same release train (WBS row FE-16, §11.13).

### 11.9.1 Deposit — Asset Hub → futarchy chain

A guided flow using a **second light-client connection to Asset Hub** (same smoldot instance, additional chain — memory budgeted in [10](10-frontend-architecture.md)) and a **pinned Asset Hub descriptor set** produced by the same descriptor pipeline and release-gated identically ([10](10-frontend-architecture.md)/[12](12-release-and-operations.md)). The deposit transaction is constructed and signed against Asset Hub through the same signer abstraction, precondition discipline and confirm-screen rules as local transactions.

Construction: an AH-side `pallet_xcm` reserve transfer (`limited_reserve_transfer_assets` or the pinned descriptor set's canonical equivalent **[VERIFY exact AH extrinsic + params against the pinned AH runtime at implementation — descriptor pipeline]**) of AH-USDC (asset id per D-17, **[VERIFY asset index 1337]** owned by [02](02-integration-contract.md)) to the user's account on the futarchy chain (paraId per [02](02-integration-contract.md)).

Precondition row (reads on the **AH connection** at its own finalized B′, plus local reads):

| Check | Read |
|---|---|
| AH connection synced & descriptors compatible | AH compat gate ([10](10-frontend-architecture.md)) |
| AH USDC balance ≥ amount + AH-side fees | AH `Assets.Account(1337, who)` **[VERIFY id]** |
| AH existential/fee viability | AH account remains above its existential/sufficiency requirements after the transfer: USDC is a *sufficient* asset, but AH fee payment and the account's surviving state (DOT ED vs. sufficient-asset-only account) are re-checked and displayed **[VERIFY AH fee payment in USDC via asset conversion vs. DOT-only for this call shape — descriptor pipeline]** |
| Amount ≥ USDC `min_balance` | 10⁴ units (1 cent *(normative value: [13](13-parameters.md))*) — below it the deposit would dust |
| **Fee-viability note (mandatory)** | first-time deposits display: *"you will pay futarchy-chain fees in USDC at the `fee.vit_usdc_rate` conversion ([08](08-treasury-and-economics.md)); deposit at least enough to cover fees"* with a concrete minimum computed from the current rate |
| **Phase-3 exposure caps (D-13)** | while PhaseFlags < Phase 4: global TVL cap headroom and per-account deposit cap headroom (constitution keys) re-read; a deposit that would exceed either is blocked with the cap shown |
| XCM channel health | the C_onchain XCM-health sub-metric / R flag ([05](05-welfare-and-decision-engine.md)/[07](07-oracle-and-disputes.md)); degraded health warns (and PB-RESERVE halts split inflows — surfaced) |

Arrival tracking: local finality on AH ≠ delivery. The tracker shows "sent — awaiting arrival" until the **futarchy-chain** connection observes the balance credit in finalized state; both legs are labelled with their own provenance. No **user-transfer** XCM outcome participates in any decision/settlement path — the tracker is display only. I-24's sole, directionally fail-static exception is the protocol-owned reserve probe `R` of [07](07-oracle-and-disputes.md) §8; this workflow cannot write or impersonate it.

### 11.9.2 Withdraw — futarchy chain → Asset Hub

A normal FE screen on the local connection: `pallet_xcm.reserve_transfer` (the chain's own user-callable reserve transfer, [09](09-execution-upgrades-and-rollout.md) XCM table).

| Check | Read at B′ |
|---|---|
| Free USDC ≥ amount + local fee (positions and holds excluded — free balance only) | `ForeignAssets.Account` |
| Remainder ≥ `min_balance` or full withdrawal | balance arithmetic displayed |
| Destination viability on AH | via the AH connection when available: destination account's existential/sufficiency state; without the AH connection the check degrades to a warning, never silently skipped |
| XCM channel health | as in deposit; a withdrawal during degraded XCM health warns that arrival may be delayed (fail-static: funds are never at decision risk, I-24) |
| No PB-LEDGER-FREEZE | freeze blocks ledger/market calls; plain asset transfers out follow the freeze scope frozen in [06](06-governance-and-guardians.md) — the row reads the freeze flag and applies the frozen scope |

### 11.9.3 Scope statement

The former FN-x silence is replaced: funding (deposit + withdrawal) is IN scope; fiat on-ramps and DEX/swap routing to acquire AH-USDC remain OUT of scope (the flow links out with an education page, no embedded third-party widgets — INV-FE-13).

---

## 11.10 Sudo-era banner (X-9, D-13)

While the chain is in Phases 0–3, the app renders a persistent **"Bootstrap governance: sudo active"** banner:

- **Driver:** the `pallet-constitution` `PhaseFlags` bitset (D-17), read light-client-verified at every finalized head; the banner is pure chain state — no remote config, no release-baked assumption.
- **Placement:** the global app shell, above navigation, on **every** route (including FE-14/FE-15 and the funding flow), and repeated as a line item on every transaction confirm screen.
- **Dismissal: none.** The banner is non-dismissable and non-collapsible for the entire Phase 0–3 window; it disappears only when a finalized `PhaseFlags` read shows Phase ≥ 4 (sudo removed). It MUST NOT be gated behind settings, themes, or "compact mode".
- **Copy (normative intent):** *"Bootstrap governance is active: a founding multisig holds sudo. On-chain state is finality-verified but not yet protected by full protocol governance."* — sudo-era state keeps its `verified-finalized` badge (it *is* valid finalized state) while the banner prevents it being presented as trust-equivalent to post-sudo state.
- The funding flow additionally surfaces the Phase-3 exposure caps (§11.9.1) as part of the same containment story.

---

## 11.11 Canonical event names (X-11d)

All event references in this document, its screens, and the tx-outcome decoder use the canonical names frozen in [02-integration-contract.md](02-integration-contract.md). For the epoch lifecycle these are: `ProposalSubmitted`, `ProposalWithdrawn`, `ScreeningStarted`, `ProposalCancelled(reason)`, `ProposalQualified`, `ProposalDeferred`, `MarketsOpened`, `DecisionExtended`, `ProposalQueued`, `ProposalRejected`, `ProposalDelayed`, `RerunOpened`, `MandateExpired`, `Executed`, `ExecutionFailed`, `MeasurementStarted`, `CohortSettled`. The FE §15.3 shorthand names (`Withdrawn`, `Cancelled`, `Qualified`, `Deferred`) are **wrong and removed**. The set additionally includes `VaultVoided` (D-1), the T20 event, and the `Traded`/`Observed` market events, all per [02](02-integration-contract.md); the ingest layer that consumes them is specified in [10](10-frontend-architecture.md).

---

## 11.12 UX degradation matrix (E-rows)

Facets: **V** visible · **L** loading · **A** verified available · **U** unavailable convenience · **F** failure message · **R** recovery.

Rows E1–E14 carry forward from FE §19 with two corrections: **E3** no longer promises "history continuous" — gaps are first-class and provider-fillable per the D-6 three-layer model owned by [10](10-frontend-architecture.md); **E5/E7/E8** provenance labels follow the never-promote rule (F-2). Their full corrected texts live with the data-layer they exercise in [10](10-frontend-architecture.md); the rows below are new and owned here.

**E15 Voting on a referendum.** V: ballot with tally, curves, conviction selector, lock consequence above the fold. L: single-block-pinned refresh per finalized head. A: everything (referenda state is live storage). U: none. F: referendum ends between B′ and inclusion ⇒ dispatch error decoded truthfully ("vote not counted — referendum closed"). R: none needed; locks unaffected.
**E16 Redeeming from a voided vault.** V: VOID layout (§11.6): merge-first, honest 0.5/0.25 rates, total-recovery figure. A: vault state + balances live. U: none. F: attempting `redeem_void` on a non-voided vault blocks pre-sign. R: —. **Required UX:** no winning-position gate may render; **cross-branch `merge`** MUST be visually primary whenever a cross-branch pair exists, and a 100 %-recovery label MUST NOT be attached to `merge_scalar`/`merge_gate`, which pay no USDC (§11.6 step 1a).
**E17 Deposit from Asset Hub.** V: two-leg tracker (AH finalized → arrival credit on futarchy chain), each leg with its own provenance badge; fee-viability and (Phase < 4) exposure-cap notices. L: AH connection syncs on entering the flow (lazy — the AH chain is not connected at boot). A: both legs light-client verified. U: none. F: AH connection unavailable ⇒ flow blocked with diagnostics (never a blind "send anyway"); cap exceeded ⇒ blocked with cap shown. R: retry AH sync; reduce amount.
**E18 Withdraw to Asset Hub.** V: standard confirm + XCM-health status line. A: local reads verified; AH destination check when the AH connection is up. U: destination check degrades to a warning without the AH connection. F: PB-LEDGER-FREEZE/PB-RESERVE scope blocks with the playbook named. R: wait for auto-expiry/flag clear.
**E19 Upgrade crank.** V: authorized-hash card, artifact fetch progress, streamed-hash verification result, DescriptorLeadTime countdown. A: authorization state verified; artifact verified by hash before signing. U: none. F: `FE-UPG-001` on hash mismatch (hard block); oversized-submission failure per the §11.8.4 [VERIFY] fallback, stated honestly on-screen. R: alternate gateway; operator-CLI handoff.
**E20 Guardian approval.** V: pending action with decoded enumerated call batch, m-of-7 progress, allowance meters, trigger-condition status. A: all live. F: trigger condition not active at B′ ⇒ blocked (playbooks are admissible only under verified triggers). R: —.
**E21 Sudo era (Phases 0–3).** V: non-dismissable banner (§11.10) on every route and confirm screen. A: PhaseFlags verified each head. F: — (informational, permanent for the phase). R: disappears on verified Phase ≥ 4.
**E22 Oracle evidence unretrievable.** V: "evidence unretrievable — treated as absent by the protocol" in dispute/ballot views; hash shown. A: on-chain round state unaffected. U: evidence body. F: hash mismatch ⇒ same treatment plus provider flagged. R: alternate gateway/user-supplied source (re-hashed on arrival).
**E23 Ratification deadline at risk.** V: proposal-detail ratification panel shows the cannot-complete warning (§11.7.4) when the referendum can no longer pass before `grace_end`. A: referendum + queue state live. F: `execute` precondition row 5 blocks with `NotRatified`. R: resubmission per protocol rules.

---

## 11.13 Work breakdown (WBS delta)

The frozen integration contract ([02](02-integration-contract.md), D-2) unblocks the plan: previously **8 of 13 epics were transitively blocked on FE-R1** (FE-1, FE-2, FE-4, FE-5, FE-6, FE-7, FE-10, FE-12; only FE-3/8/9/11/13 could proceed). FE-R1 is now implementable against a jointly-owned frozen contract with a mirroring backend row (E15), and all formerly blocked epics are unblocked, subject only to normal dependency order.

| Epic | Scope | Depends | Acceptance | Pts |
|---|---|---|---|---|
| FE-R1 | Runtime: `FutarchyApi` + `RecentCohortSummaries` (32) + `Traded`/`Observed` + `BaselineMarketOf` + views — **per the frozen contract [02](02-integration-contract.md); backend row E15 mirrors this** | contract 02 (frozen) | callable via chainHead on Zombienet; bounds asserted | 3 |
| FE-1 | `chain`: smoldot worker, dual relay+para (+ **lazy Asset Hub chain**), identity/compat gates, sync stores | FE-R1 (testnet) | boot machine green; budgets instrumented | 5 |
| FE-2 | `descriptors` pipeline + CI drift gates + **pinned Asset Hub descriptor set** | FE-1 | multi-version + AH selection tested incl. simulated upgrade | 3 |
| FE-3 | `protocol`: TS fixed-point math + derivations vs. regenerated reference vectors (V1 = 512.494795136 *(normative value: [13](13-parameters.md))*) | — | corrected V1–V6 + MPFR corpus pass | 3 |
| FE-4 | `wallet`: signer abstraction, **corrected precondition system (§11.5 incl. the complete `execute` row)**, tx machine, fee selector bound to `fee.vit_usdc_rate` | FE-1..3 | §11.5 tables implemented; Playwright tx suite | 5 |
| FE-5 | Current-state screens incl. **Voided redeem UX (§11.6)** and Baseline book | FE-1..4 | screen matrix demo, providers disabled, cleared IDB; VOID e2e | 5 |
| FE-6 | `local-index`: gap-tolerant schema, ingest, eviction, corruption recovery ([10](10-frontend-architecture.md)) | FE-1 | idempotency + gap-visibility property tests | 4 |
| FE-7 | `providers` + `tools/snapshot` + sampler + forged-corpus tests | FE-6 | T-5/T-7 suites | 4 |
| FE-8 | `verify` + release panel + self-check + ArNS cross-check | — | E11/T-1 suites | 3 |
| FE-9 | Distribution: Vite/Arweave/manifest/SW/CSP/SRI; deploy + repoint tooling | FE-8 | routing/failover suites; staging name live | 4 |
| FE-10 | Degradation UX (**E1–E23**), error-copy registry, a11y, i18n scaffold, **sudo banner (§11.10)** | FE-5..9 | matrix scripted in Playwright; banner non-dismissability asserted | 4 |
| FE-11 | Reproducible build + attestations + verify-release CLI + key ceremony | FE-9 | two-environment identical hash | 3 |
| FE-12 | Perf hardening to budgets ([10](10-frontend-architecture.md)); mobile lab; **AH second-chain memory validated** | all | release gates green on device lab | 3 |
| FE-13 | Ops handbook, bootnode program, ArNS ceremony, launch ([12](12-release-and-operations.md)) | FE-11 | dry-run rollback executed | 2 |
| **FE-14** | **Governance surface (§11.7)**: referenda list/detail (six tracks), vote/delegate/undelegate/unlock with conviction, ratification panel + execute-deadline logic, OracleResolution ballot with snapshot rule | FE-1..4 | G-rows implemented; ratification-deadline e2e (Chopsticks); snapshot-power display test | 4 |
| **FE-15** | **Operator surface (§11.8)**: reporter console + recompute proofs + evidence display, guardian 5-of-7 console, treasury claims + `nav()` w/ haircut flag, upgrade crank (+ FE-P10 spike), snapshot crank, registry filing/challenge | FE-1..4, FE-9 (artifact fetch) | each S14–S19 workflow e2e on Zombienet; `FE-UPG-001` suite | 5 |
| **FE-16** | **Funding flow (§11.9)**: AH connection UX, deposit construction, withdrawal, two-leg tracker, cap/fee-viability checks | FE-1, FE-2 (AH descriptors), FE-4 | deposit+withdraw e2e against Chopsticks AH+para; cap-block test | 3 |

New prototype: **FE-P10** — multi-MB extrinsic submission through smoldot/PAPI (§11.8.4); gates the FE-15 upgrade-crank tier, with the verified fallback path shipping regardless.

---

## Resolves

| Finding | Resolution in this document |
|---|---|
| X-2 | §11.7: full FE-14 governance surface — six-track referenda list/detail, conviction vote/delegate/undelegate/unlock, ratification panel with the D-5 execute-time deadline, OracleResolution ballot with the pre-cohort snapshot rule; screens, storage enumeration, extrinsics, precondition rows G-1…G-8, E-rows E15/E23; WBS epic FE-14 |
| X-5 (FE) | §11.2/§11.5: forecast trading removed entirely per D-8 — trading rows admit `Trading`/`Extended` only; the Baseline book gets its own precondition row (P-2); no forecast screen, route, or residue remains |
| X-6 (FE) | §11.6: `Voided` vault state handled end-to-end — merge-at-par as the primary action, `redeem_void` with honest 0.5/0.25 rates, precondition rows for both, the winning-position-balance requirement explicitly deleted under VOID; E16 |
| X-8 | §11.9: funding in scope — deposit via a second Asset Hub light-client connection with pinned AH descriptors and reserve-transfer construction; withdrawal via the chain's own `pallet_xcm.reserve_transfer`; precondition rows incl. AH-side existential/fee checks and Phase-3 exposure caps; mandatory `fee.vit_usdc_rate` fee-viability note; WBS epic FE-16 |
| X-12 | §11.8: FE-15 "Advanced" operator surface — reporter registration/`recompute_proof`/hash-verified evidence display, guardian 5-of-7 signing console with trigger-condition preconditions, stream claims + rendered `nav()` with the reserve-haircut flag, `apply_authorized_upgrade` crank with pre-submission hash verification and an honest [VERIFY]-tagged memory/streaming fallback, `welfare.snapshot` crank, registry filing/challenge flows; WBS epic FE-15 |
| X-11e | §11.4 rule 2 + §11.5: per-trade min/max, `MinSplit`, `MinTransfer`, `MaxPositionsPerAccount` and all §21-class kernel constants are read via the runtime constants API (`[C]` rows), never as storage and never hardcoded |
| X-11i | §11.5 P-12: the `execute` precondition row lists all 14 dispatch-time checks the backend performs — including ratification (`NotRatified`), attestation presence, capability rules, rate meters, resource locks, guardian suspension, gate-breach flags, dead-man/freeze, batch/SafetyFilter bounds and DescriptorLeadTime — kept in lockstep with [09](09-execution-upgrades-and-rollout.md) |
| D-13 (FE) | §11.10: persistent, non-dismissable "bootstrap governance (sudo active)" banner during Phases 0–3, driven solely by the light-client-read `PhaseFlags` bitset, on every route and confirm screen; Phase-3 exposure caps surfaced in the funding flow; E21 |
| X-11d (supporting) | §11.11: all event references use the canonical names frozen in [02](02-integration-contract.md) (`ProposalWithdrawn`, `ProposalCancelled`, `ProposalQualified`, `ProposalDeferred`, …) |

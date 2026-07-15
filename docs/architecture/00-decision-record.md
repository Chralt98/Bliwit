# Decision Record — Resolution of DESIGN_REVIEW.md v2.0

**Status: normative.** This document resolves every decision point and finding in `DESIGN_REVIEW.md` (v2.0, 2026-07-12). The component documents `01`–`15` in this directory implement these decisions and together supersede `BACKEND_PLAN.md` and `FRONTEND_PLAN.md` as the authoritative architecture. Decisions were made optimizing for the best end-user experience within the protocol's safety guarantees. Normative language: RFC 2119.

Conventions: **D-n** = a decision made here. Finding IDs (X-n, B-n, F-n) refer to DESIGN_REVIEW.md v2.0. Each component document ends with a "Resolves" table listing the finding IDs it implements.

---

## Part 1 — Major decisions

### D-1. VOID redemption: merge-at-par + half-value unpaired redemption (B-1, X-6)

- New `VaultState::Voided` variant, entered by `void(pid)` (ResolveAuthority), emitting `VaultVoided`.
- Under `Voided`: `merge` and `merge_scalar` remain enabled — complete pairs always recover par (100%).
- New call `redeem_void(pid, kind, amount)`: unpaired **branch-USDC pays `floor(a/2)`**; unpaired **LONG or SHORT pays `floor(a/4)`** (equivalent to branch value 0.5 × neutral s = 0.5). Rounding is against the redeemer; residue swept per the dust rule.
- Conservation: total payout ≤ E in every path (pairs pay 1 per pair; each pair = 1 USDC escrowed).
- PT-2 (annulment) restated honestly: *complete-set holders and market buyers recover full principal under VOID* (buyers hold the mirror branch-USDC per D-3, so their split is reconstructible); a deliberately unpaired single-branch speculator recovers 0.5 — this is the correct price of a voided binary claim, not a loss of principal on the protocol path.
- Frontend: redeem screen gains a VOID state — shows "merge to recover 100%" as the primary action when the user holds pairs, `redeem_void` otherwise. FE precondition table row added.

### D-2. Integration contract: all 11 §30 patch items ACCEPTED; frozen in one owned document (X-1, X-4, X-10, X-11, X-15)

- FE §30 P-1…P-11 are **all accepted** and applied, plus the gaps P-5 missed (the §5.2.3 storage-list edit, `DecisionOutcomeCode` → the canonical type is **`DecisionOutcome`**, FE renames).
- The contract is frozen in **`02-integration-contract.md`**, jointly owned by both teams; changes require sign-off from both. It contains: the full 11-method `FutarchyApi` runtime API with view types in `futarchy-primitives`; `Traded{market, who, side, amount, cost, p_after}` / `Observed{market, o_t}` events (pallet-market gets an Events column); `RecentCohortSummaries` ring (last **32** cohorts) in `pallet-epoch` storage; `BaselineMarketOf: map EpochIndex → MarketId`; oracle pallet storage-item and event names; a `Voided`/T20 event; chain-identity constants (D-17); the WSS bootnode requirement (D-6); and the backend-published test-artifact feed (per-release runtime wasm + metadata, Chopsticks/Zombienet environments, chainHead fixtures).
- **Indexer role**: the FE P-4 position wins — canonical history is **event-derived and chain-served within the committed window**; the indexer is an optional convenience for dashboards, never load-bearing for the canonical frontend. BE §5.2.6 "pruned to indexer" language is replaced accordingly.
- All kernel constants the FE re-checks (`MinSplit`, per-trade min/max, position bound, §21-tunables) are exposed via the runtime **constants API** (metadata) — readable without storage, no hardcoding in the FE.
- Backend WBS gains row **E15** mirroring FE-R1 (contract implementation), release-gating for the backend exactly as FE-12 is for the frontend.

### D-3. Trade denomination: branch-USDC books with an auto-split wrapper; buyers keep the mirror (B-7)

- LMSR books are denominated in **branch-USDC**. The user-facing `buy(market, side, amount, max_cost)` accepts **USDC**: the wrapper splits cost `c` USDC into `c` AcceptUsdc + `c` RejectUsdc, pays the target-branch `c` into the book, delivers `q` LONG/SHORT of the target branch, and **credits the mirror-branch `c` branch-USDC to the buyer**. `sell` is the inverse.
- Consequence: a buyer in the losing branch holds mirror branch-USDC redeemable at par → G-3 (annulment) holds for the dominant user path, and VOID recovery (D-1) is par for buyers.
- **Revenue recycling**: book revenue (branch-USDC) is immediately `split_scalar` into complete LONG+SHORT sets held by the book — worth exactly 1 branch-USDC per pair at any settlement `s`, so the book is solvent by construction. `headroom = b·ln 2` per book, stated and sized in `04-markets-and-pricing.md`.

### D-4. Economic security: decide-time outflow cap (primary) + Ask-scaled liquidity (secondary) (B-8)

- **Primary mechanism**: at decide time, the engine computes `AttackCost̂` from *measured* depth (TWAP-window traded volume and book depth) and enforces `InCapPrize ≤ AttackCost̂ / 3` as a new decision-engine step; failing proposals reject with `RejectReason::SecuritySizing`. This scales with the value at stake by construction.
- **Secondary**: `pol.b`, `dec.v_min` and δ scale with the proposal's `ask` (piecewise-linear per class, floors = current defaults). Defaults and the worked recomputation showing `AttackCost ≥ 3·MEV` for the maximum in-cap prize live in `08-treasury-and-economics.md`.

### D-5. Values-ratification: single execute-time deadline (B-11)

- One deadline: **checked at `execute()` dispatch time**. The ratification referendum is submitted any time after the artifact hash is committed (queue time) and runs during the timelock.
- Plumbing added: `ratify(proposal_id, referendum_index)` admissible call for the ratify track's `Contains` filter; execution-guard dispatch check; `RejectReason::NotRatified`; a §14.2-equivalent decision-table row; attestation-presence check in the decision pseudocode.

### D-6. Deep history: chain-served recent window + committed operator window + gap-tolerant index (X-3, F-1, F-2, X-4)

Three layers, all truthfully labeled:
1. **Chain-served, light-client-verified**: `RecentCohortSummaries` (32 cohorts) + 8-checkpoint TWAP series per market — every user gets core history with zero infrastructure dependency.
2. **Committed operator window**: protocol-funded bootnode/RPC operators (D-16) commit to serving **30 days** of state/bodies; an honest ops line in the backend topology, wired into phase gates. FE backfill works within this window and is labeled `provider` unless re-read through smoldot within the pinned window.
3. **Local index redesigned gap-tolerant**: explicit holes are first-class; E3's promise is corrected to "gaps are visible and provider-fillable, never silently spliced"; U-3's archive-independence claim is scoped to layer 1.
- **F-2 fix (unconditional)**: RPC-fallback data is **never promoted** to `verified-finalized`. Verified status requires a smoldot re-read; otherwise data stays `provider`-labeled.
- WSS bootnodes: ≥8 browser-reachable WSS across ≥4 operators, ≥2 on port 443 — now a **backend §4.2-equivalent node-roles row and chain-spec requirement**, gated in the rollout phases.

### D-7. `ProposalClass::Emergency` is DELETED (B-batch)

Emergencies are handled by guardian playbooks (which is what the spec already did in practice). The class enum, classifier row and §21 rows are removed. The ADR-3 classifier completeness obligation is now satisfiable.

### D-8. Forecast trading is CUT from v1 (X-5)

Books close at branch resolution; no post-resolution reopening. Removes the un-mintable reopened-book state (the `split_scalar`-requires-`Open` deadlock), removes the FE surface gap, and simplifies the user model. Recorded as deferred work for v2 with the inventory problem stated.

### D-9. Emergency brake: PB-LEDGER-FREEZE + expedited CODE lane (B-17, B-16)

- **PB-LEDGER-FREEZE**: guardian 5-of-7 playbook, admissible only when the I-4 drift flag is set (machine-checked solvency anomaly). Freezes all ledger and market calls (both inflow and outflow). Auto-expires after **14 days**, one renewal only via values referendum; mandatory retro ratification; every activation emits events and costs a review record.
- **Expedited CODE lane**: admissible only while PB-LEDGER-FREEZE is active; 72h gate market + 3-day fast-track values ratification; executes through the normal execution guard (no new privileged origin).
- **Coretime wedge (B-16)**: the enumerated coretime-renewal call (treasury → Coretime chain transfer against a pre-authorized budget line) is **exempt** from the dead-man freeze and keeper-executable during degraded mode; renewal does not consume a recovery epoch.

### D-10. Bounds reconciled (B-batch, X-11h)

- `IntakeQueue = 64` (pre-qualification only) and `MaxLiveProposals = 32` (Screening→Settled) are **both kept, with disjoint scopes** — the "(all states)" qualifier is deleted.
- Books per proposal ≤ **6** (2 decision + 4 gate); Baseline books ≤ 4 live (one per live epoch). `MaxLiveMarkets = 32·6 + 4 = 196`. All PoV/storage budgets are re-derived from this one table in `13-parameters.md`.

### D-11. The canonical frontend serves the values layer and operator workflows (X-2, X-12)

New FE epic **FE-14 (Governance surface)**: referenda list/detail, vote/delegate/undelegate/unlock with conviction, ratification status on proposal detail, `OracleResolution` ballot. New FE epic **FE-15 (Operator surface)**, an "Advanced" area: reporter registration + `recompute_proof` submission + evidence display; guardian 5-of-7 approval signing; treasury stream claims + `nav()` view; `apply_authorized_upgrade` (Wasm fetched from the release artifact on Arweave, hash-verified against the authorized hash before submission) and `welfare.snapshot` cranks. All state involved is light-client readable; INV-FE-4 stands unamended.

### D-12. USDC funding is IN SCOPE for the canonical frontend (X-8, X-14)

- **Withdraw (exit)**: `pallet_xcm.reserve_transfer` on the futarchy chain — normal FE screen with precondition row.
- **Deposit (on-ramp)**: a guided funding flow with a second light-client connection to **Asset Hub** and a pinned Asset Hub descriptor set (added to the descriptor pipeline). Shipped in the same release train; the flow is listed in the §26-equivalent WBS.
- **Fees (X-14)**: `pallet-asset-tx-payment` is wired to a constitution key **`fee.vit_usdc_rate`** (typed, bounded [0.1×, 10×] around its reference, PARAM-adjustable, genesis default set from the launch reference price). USDC-only users can always pay fees in USDC. The FE fee-currency selector binds to this key.

### D-13. Phase-3 insider risk contained (X-9)

- The dangerous frame-system calls (`set_storage`, `kill_storage`, `kill_prefix`, `set_code_without_checks`, `authorize_upgrade_without_checks`) are filtered **from genesis** for all origins including sudo (not "post-bootstrap").
- Phase 3 runs under a **real-USDC exposure cap**: global TVL cap + per-account deposit cap (constitution keys, raised only by phase gates).
- The founding multisig is added to the §22 adversary model with a threat row.
- The FE renders a persistent **"bootstrap governance (sudo active)"** banner during Phases 0–3 (chain-read from the phase flag), so sudo-era state is never presented as trust-equivalent to post-sudo state.

### D-14. Upgrade/descriptor coordination: enforced lead time + expedited FE release (X-7, F-mediums)

- Backend: `execute()` for CODE/META requires `now ≥ authorized_at + DescriptorLeadTime` (**43,200 blocks = 72h**) between `UpgradeAuthorized` and permissionless application.
- Frontend: v(N+1) descriptors MUST be generated from the queue-time artifact commitment and live on the release channel **before execute maturity** — a release-gating check, not a convention.
- **Expedited descriptor-only release**: 2 attestations, no 72h soak, 3-of-5 repoint — admissible only for descriptor/metadata updates with zero app-code delta.
- Pinned-release users get an in-app upgrade warning fed by a **fixed-layout raw storage key** (`ReleaseChannel` in `pallet-constitution`, SCALE layout frozen forever), readable without current metadata — this replaces the §24.5 `system.remark` pointer, which stranded apps could not decode.

### D-15. Genesis economics specified (B-14, B-15, B-18)

- **VIT**: total supply 1,000,000,000 (12 decimals). Allocation: 30% treasury reserve; 25% community distribution (vested); 20% founding team (4-year vest, 1-year cliff); 15% ecosystem/ops fund; 10% Phase 3–4 incentive programs. `iss.inflation_cap = 2%/yr`, issuance mechanism specified in `08`.
- **USDC treasury**: initial funding target **≥ 25M USDC** before Phase 5 arming; published **minimum-viable NAV per class** gates phase advancement (one CODE ⇒ NAV ≥ ~14M at floor liquidity; the gate is explicit and loud, not silent).
- **Collator compensation**: treasury ops line, 2,000 USDC/collator/epoch initial (PARAM-adjustable).
- **Welfare cold start (B-15)**: genesis ships `PriorBounds` (declared from Phase-2 shadow data); epochs 1–12 winsorize against `prior ∪ available` — `s` is deterministically computable from epoch 1.
- **Reporter-stake sequencing**: Phase-3 arming requires ≥3 registered reporters with full stakes; the incentive-program allocation funds reporter-stake bootstrapping loans (recallable).
- **Launch collator-D cap**: the D concentration term uses a phase-scheduled cap so a 5-collator launch set does not crush W (schedule in `05`).

### D-16. Operations layer: owners, funding lines, ArNS permabuy (X-13, F-5, F-mediums)

- Named treasury budget lines: bootnodes/RPC/archive (the 30-day commitment of D-6), keeper subsidies beyond the metered budget (raised to **12,000 USDC/epoch**, recomputed from crank volume), oracle evidence hosting, monitoring, Arweave/ArNS.
- **ArNS: permabuy** (eliminates the lease-lapse takeover). ANT control: 3-of-5 if n-of-m is confirmed by prototype FE-P7 (the ANT-capability experiment; an earlier draft of this record mislabeled it FE-P8), else the FROST-ed25519 ceremony; **single-key custody is prohibited** — if neither materializes, launch blocks on this line.
- **Signer disjointness required**: ArNS controllers ∩ minisign release keys = ∅, enforced organizationally and listed in the threat model.

### D-17. Chain identity constants (X-11a/b)

| Constant | Value |
|---|---|
| ss58 prefix | **7777** (registry submission required before Phase 2) |
| paraId | assigned at onboarding; test fixtures use 4242 |
| USDC | `ForeignAssets`, XCM `Location {parents: 1, X3(Parachain(1000), PalletInstance(50), GeneralIndex(1337))}` **[VERIFY asset index 1337]** — pinned in FE `ChainIdentity` |
| USDC decimals | 6 · VIT decimals | 12 |
| VIT existential deposit | 0.01 VIT |
| Phase flag | `pallet-constitution` storage `PhaseFlags` (bitset), the key the FE binds trading enablement to |

### D-18. Remaining governance/oracle hardening decisions

- **Oracle adjudication track (B-19)**: raised to 60% approval / 10% support / 7-day; tally uses a **pre-cohort conviction snapshot** (VIT locked before cohort creation), excluding capital that entered after the dispute's subject cohort began.
- **Gate determinism (B-9)**: C is split into `C_onchain` (deterministic, same-block computable: XCM health, collator set, runtime panics, reserve health) which alone drives **daily** gate-breach flags and gate-market settlement, and `C_attested` (incidents, external prices) which enters settlement-time W only.
- **VIT reflexivity (B-10)**: the E component values security collateral as dimensionless **coverage ratios** against VIT-denominated requirements; no VIT price enters W anywhere.
- **Challenge windows (mediums)**: extended to 72h with a bonded-watchtower acknowledgment quorum (2-of-N registered watchtowers co-sign "observed", else the window extends once by 48h); TM-4 row corrected to "delay, and wrong only under watchtower + collator collusion".
- **Kernel attestation (mediums)**: bonded attestor registry (values-elected, ≥3), 2-of-3 signed attestations with challenge window — no longer presence-only.
- **Oracle bonds (mediums)**: challenge/report bonds scale with cohort value-at-stake: `bond = max(flat_floor, bps × cohort_escrow)`.

---

## Part 2 — Finding disposition table

Every DESIGN_REVIEW.md finding, its resolution, and the owning component document.

| ID | Resolution | Doc |
|---|---|---|
| X-1 | D-2: all P-1…P-11 applied; contract frozen | 02 |
| X-2 | D-11: FE-14 governance surface | 11 |
| X-3 | D-6: three-layer history model | 02, 10, 12 |
| X-4 | D-6: bootnode row + phase gate | 01, 02, 12 |
| X-5 | D-8: forecast trading cut | 04 |
| X-6 | D-1: Voided state end-to-end | 03, 11 |
| X-7 | D-14: DescriptorLeadTime + expedited release | 09, 12 |
| X-8 | D-12: funding flow in scope | 11 |
| X-9 | D-13: genesis filter + exposure cap + banner | 09, 14 |
| X-10 | D-2: `BaselineMarketOf` storage | 02, 04 |
| X-11a–j | D-2, D-17: drift items individually fixed (ForeignAssets location, ss58/paraId/ED/phase-flag, oracle names, epoch event names, constants API, T20 `Voided` event, `DecisionOutcome`, no FE hardcodes, execute precondition row completed, phantom §18.6 refs removed) | 02 + owning docs |
| X-12 | D-11: FE-15 operator surface | 11 |
| X-13 | D-16: owners + funding + permabuy | 12 |
| X-14 | D-12: `fee.vit_usdc_rate` | 08 |
| X-15 | D-2: published test artifacts | 02, 15; corpus schema: 04 §5 |
| B-1 | D-1 | 03 |
| B-2 | Gate instruments representable: `PositionKind` gains `GateYes(gate)`, `GateNo(gate)` per branch; `VaultInfo` gains per-branch gate-set supplies; `settle_gate(pid, gate, outcome)` call; conservation identity extended per-branch over the enlarged set | 03 |
| B-3 | Baseline market gets a ledger home: epoch-keyed `BaselineVaults`, `PositionId::Baseline{epoch, Long/Short}`, `pol.b_baseline` param, settlement path via SettleAuthority at epoch settlement | 03, 04 |
| B-4 | Per-branch supply fields; per-branch identity `escrowed == supply(bUSDC_b) + Q_b` checked for both branches; POL seeding flow re-walked | 03 |
| B-5 | Unpaired SHORT redeems `floor(a·(1−s))`; new atomic `redeem_scalar_pair` pays exactly `a` per pair | 03 |
| B-6 | V1 = **512.494795136**, V5 net = **−3.074969**; §11.6 vectors regenerated from the reference model in CI | 04 |
| B-7 | D-3 | 04 |
| B-8 | D-4 | 05, 08 |
| B-9 | D-18 gate split | 05, 07 |
| B-10 | D-18 coverage ratios | 05 |
| B-11 | D-5 | 06 |
| B-12 | Transitions added: T21 `Rejected → Measuring`, T22 `FailedExecuted → Measuring` (retry exhausted), T23 `FailedExecuted → Executed` (retry succeeds); T13 rerun re-enters `Extended` (3 days) then decides | 05 |
| B-13 | 10% bond slash on preimage-missing cancellation; `request_preimage` pinning at qualification | 06, 08 |
| B-14 | D-15 | 08 |
| B-15 | D-15 cold start | 05 |
| B-16 | D-9 coretime exemption | 09 |
| B-17 | D-9 | 06, 09 |
| B-18 | D-15 min-viable NAV, loud gate | 08 |
| B-19 | D-18 track hardening | 06 |
| B-med: sanity band | Gate books exempt from the [0.02, 0.98] band (they get a near-boundary validity rule); band applies to welfare books only; `V_min` ambiguity resolved to per-book | 05 |
| B-med: forecast mint | Moot per D-8 | 04 |
| B-med: ProcessHold | `any_open_dispute_touching` scoped to consumed MetricSpec components with dispute bond ≥ `dis.merit_min`; registry sub-games hold settlement, never decisions | 07 |
| B-med: slot monopolization | Bond refundable only on decision-grade outcome; 10% slashed to INSURANCE otherwise; ≤4 intake entries/epoch/account; TM row added | 06, 14 |
| B-med: IntakeQueue vs MaxLive | D-10 | 13 |
| B-med: Positions map | Key order `(PositionId, AccountId)` (per-vault drainable); per-account bound via counter; deposit 0.1 USDC/entry; protocol accounts exempt from the 64-position cap | 03 |
| B-med: Emergency class | D-7 deleted | 05, 06 |
| B-med: epoch.length | Phase offsets become fractions of epoch length; changes effective next epoch; in-flight cohorts keep creation-time schedule; floor 14 days | 05 |
| B-med: SettleAuthority | One path: `pallet-epoch::settle_cohort` → `pallet-welfare::compute_settlement` → ledger via welfare's SettleAuthority origin; §6.1 table updated | 05 |
| B-med: force_rerun | Defined: pre-execution only; TWAP reset, books reopen for 3-day Extended, positions intact, one decide re-run | 06 |
| B-med: EmergencyPlaybook calls | Admissible call set enumerated in the §6.2-equivalent capability table | 06 |
| B-med: Incident/MilestoneRegistry | New `pallet-registry`: bonded filings, challenge windows, slashing, bounds, weights; feeds C_attested | 07 |
| B-med: oracle bonds | D-18 value-scaled | 07 |
| B-med: challenge censorship | D-18 watchtowers | 07, 14 |
| B-med: attestation presence-only | D-18 attestor registry | 06, 09 |
| B-med: keeper budget | Recomputed (≥134k decision-critical cranks/epoch); metered budget 12,000 USDC/epoch + fee rebates; exhaustion alarm | 08 |
| B-med: USDC freeze | Reserve-health trigger R added to C_onchain; PB-RESERVE halts split inflows; NAV haircut flag surfaces in FE | 07, 08, 10 |
| B-med: collator-D cap | Phase-scheduled cap (D-15) | 05 |
| B-med: C/P/A aggregation | Intra-pillar formulas fully specified (weighted geometric, ε-floors, weights in MetricSpec) | 05 |
| B-med: decide() fields | `Proposal` gains `ask`, `decide_at`; canonical `DecisionOutcome` enum defined | 05, 02 |
| B-med: RejectReason | `NotRatified`, `SecuritySizing`, `AttestationMissing` added; `VetoUpheldByReview`/`PayloadReverted`/`SecondExtensionFailed` wired to producers | 05 |
| B-med: threat rows | All missing rows added (dispute-griefing, Baseline-floor suppression, position-cap dusting, preimage sabotage, founder/sudo insider, challenge censorship, ArNS lapse, signer disjointness) | 14 |
| B-med: SafetyFilter | Recursion extended to `proxy_announced`, `as_multi_threshold_1` — the wrapper set is now closed | 06, 09 |
| B-low (all) | `ScalarSettled` carries winning branch; maker loss ~180 USDC; maturity B+288,000; Trade phase d5–d18 labels; TWAP slew per 10-block observation interval; §15.2 latency table reconciled with the D-18 track; dangling refs removed; FGP/SGF/GFP/EFP/AEGIS identified in the bibliography | owning docs |
| F-1 | D-6 | 10 |
| F-2 | D-6: never promote | 10 |
| F-3 | INV-FE-1…15 and all question blocks published verbatim | 15 |
| F-4 | Moot per D-2 (contract frozen); contingency = the D-6 layer-1 fallback | 02, 10 |
| F-5 | D-16: permabuy + no single-key custody | 12 |
| F-med: expedited release | D-14 | 12 |
| F-med: transaction-critical | Definition narrowed: provider charts are declared an accepted residual with provenance labeling; sampling limits stated honestly | 10, 14 |
| F-med: boot machine | Missing states added (worker spawn, WASM, storage errors; boot-time restricted; pre-Ready degradation); relation to compat machine defined | 10 |
| F-med: growth arithmetic | Recomputed at 196-book max load; retention auto-tunes to budget (degrades depth, never correctness); honest depth table | 10 |
| F-med: CSP | `connect-src` allowlist (bootnodes + gateways + opted-in providers), not `*` | 12 |
| F-med: hostile SW | Declared residual + out-of-band attestation monitor; detection claim removed | 12, 14 |
| F-med: ArNS lease | Moot per D-16 permabuy | 12 |
| F-med: system.remark | D-14 fixed-layout raw storage key | 09, 12 |
| F-med: signer disjointness | D-16 | 12, 14 |
| F-med: provider firewall | Structural enforcement inside `apps/web` too (build-time import boundary, not lint-only) | 10 |
| F-med: backfill math | Recomputed consistently at 20 blk/s | 10 |
| F-med: txHistory | Ingest fetches extrinsic bodies for blocks containing the user's extrinsics (only) | 10 |
| F-med: proof-size conflation | §14.3 corrected | 10 |
| F-med: metadata blobs | Bounded per the budget with eviction | 10 |
| F-med: multi-tab | Dedicated worker + Web Locks leader election; Android 2× memory budgeted | 10 |
| F-med: Alt-C providers | Ship empty list, opt-in (Alt-C text corrected) | 10 |
| Open [VERIFY]s | All retained verbatim in owning docs; the two no-fallback items get fallbacks: browser-WSS peer behavior → D-6 layer-2 operators are the guaranteed dial set; XCM-health availability → R sub-metric with a fail-static daily C flag (R and X are both `C_onchain` components, 05 §4.3); PB-MIGRATION contents specified in 09 | owning docs |

---

## Part 3 — Document map, shared constants, editorial standard

### Document map

| Doc | Component | Primary sources |
|---|---|---|
| `01-system-overview.md` | Goals, guarantees, topology, pallet map, rollout summary | BE §1–6, §29 |
| `02-integration-contract.md` | **Frozen** chain ↔ frontend contract (D-2, D-17) | FE §30, BE §7/§25 |
| `03-conditional-ledger.md` | Ledger, solvency, VOID, gate instruments, Baseline home | BE §5.2.1, §7, §10 |
| `04-markets-and-pricing.md` | LMSR, TWAP, trade path, gate + Baseline markets | BE §11, §13, §17.3 |
| `05-welfare-and-decision-engine.md` | Welfare function, state machines, decision rule | BE §8, §9, §12, §14 |
| `06-governance-and-guardians.md` | Values layer, tracks, ratification, guardians, playbooks | BE §16, §18.3, §20, §6 |
| `07-oracle-and-disputes.md` | Reporting game, disputes, registries, watchtowers | BE §15 |
| `08-treasury-and-economics.md` | Treasury, POL, genesis, fees, keeper economics, security sizing | BE §17, §21, §27 |
| `09-execution-upgrades-and-rollout.md` | Execution guard, upgrade path, XCM, phases, emergency lanes | BE §18, §19, §29 |
| `10-frontend-architecture.md` | Boot, light client, data layer, verification, budgets | FE §8–§16 |
| `11-frontend-workflows.md` | Screens, preconditions, governance + operator + funding surfaces | FE §17–§19, §30 |
| `12-release-and-operations.md` | Release train, keys, ArNS, bootnodes, ops funding | FE §22–§28, BE §28 |
| `13-parameters.md` | Single reconciled parameter/bounds/constants table | BE §21 |
| `14-threat-model.md` | Combined threat model, all new rows | BE §22, FE §20 |
| `15-invariants-and-testing.md` | I-1…, INV-FE-1…15 verbatim, test/verification regime, artifacts | BE §23–24, FE §23, §32 |

### Frozen shared constants (no document may restate different values)

- Block time 6 s; epoch 21 days = 302,400 blocks; Trading **d5–d18** (offsets 72,000–259,200; 13 days); decide d18; measurement k = 2 epochs; settle e+3; maturity example B+288,000.
- Oracle: report window per §15; challenge window **72h** (43,200 blocks) with watchtower quorum; dispute latency table reconciled in 07.
- `IntakeQueue = 64`; `MaxLiveProposals = 32`; books/proposal ≤ 6; `MaxLiveMarkets = 196`; `RecentCohortSummaries` ring = 32; `MaxPositionsPerAccount = 64` (protocol accounts exempt); Positions deposit 0.1 USDC; key order `(PositionId, AccountId)`.
- LMSR: V1 = **512.494795136** USDC; V5 net = **−3.074969**; V2–V4 unchanged; maker worst-case loss `b·ln 2`; §11.10 maker-loss example ≈ 180 USDC; TWAP slew cap per **10-block** observation interval.
- VOID: pairs par via merge; unpaired branch-USDC `floor(a/2)`; unpaired LONG/SHORT `floor(a/4)`.
- Scalar redemption: LONG `floor(a·s)`; unpaired SHORT `floor(a·(1−s))`; paired via `redeem_scalar_pair` = exactly `a`.
- `DescriptorLeadTime = 43,200` blocks (72h). PB-LEDGER-FREEZE ≤ 14 days + one renewal.
- Keeper metered budget 12,000 USDC/epoch. Collator comp 2,000 USDC/collator/epoch. Intake: ≤4 entries/epoch/account; 10% bond slash (to INSURANCE — burning USDC would strand backing reserve) on non-decision-grade/preimage-missing outcomes.
- Chain identity per D-17. VIT supply 10⁹ (12 dec); USDC 6 dec; treasury target ≥ 25M USDC; min-viable NAV per class per 08.
- Oracle adjudication track: 60% / 10% support / 7-day / pre-cohort snapshot.
- Bootnodes: ≥8 WSS, ≥4 operators, ≥2 on :443; operator served-state window 30 days.

### Editorial standard

1. Every doc starts with a status header (`Status: normative component specification. Supersedes the corresponding sections of BACKEND_PLAN.md/FRONTEND_PLAN.md.`) and states its **boundary** (what it owns, what it references).
2. One source of truth per fact: constants live in `13-parameters.md` and `02-integration-contract.md`; other docs reference, never restate values (quoting for readability is fine when marked *(normative value: §13)*).
3. RFC 2119 language; `[VERIFY]` tags are retained wherever the underlying uncertainty still exists — honesty over polish.
4. Every doc ends with a **Resolves** table: finding ID → one-line statement of how the text resolves it.
5. Cross-references between docs use relative markdown links.
6. Content fidelity: these docs carry forward everything the review verified as correct ("What the design gets right") — they are a reorganization + repair, not a redesign.

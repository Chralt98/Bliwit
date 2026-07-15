# 15 — Invariants and Testing

**Status: normative component specification.** Supersedes BACKEND_PLAN.md §23–§24 and FRONTEND_PLAN.md §23 and §32, and publishes the normative texts those documents cited but never stated (DESIGN_REVIEW.md F-3). Normative language: RFC 2119.

**Boundary.** This document owns: (1) the protocol invariant set I-1…I-27, updated for the enlarged ledger of [03-conditional-ledger.md](03-conditional-ledger.md); (2) the frontend invariants INV-FE-1…15, published verbatim for the first time, plus the required-question blocks, required-UX row list, and decision-bias texts; (3) the merged backend + frontend testing and verification regime; (4) the cross-repo test-artifact publication contract (schema frozen in [02-integration-contract.md](02-integration-contract.md)); (5) the certification apparatus. It references, and never restates values from, [13-parameters.md](13-parameters.md) and [02-integration-contract.md](02-integration-contract.md). Threat rows live in [14-threat-model.md](14-threat-model.md).

---

## 1. Protocol invariants (backend), I-1…I-27

Changes from the superseded §23 are marked **[updated]** / **[new]**. The **Enforcement class** column is the honesty column mandated by the review: `machine/state` = checked on-chain (`try-state` and/or dispatch-time assertions), `machine/test` = proven by model checking or exhaustive/property tests in CI, `convention` = enforced only by CI lint + review discipline — the code contains no structure that makes violation impossible. I-22 and I-24 are convention-class and are stated as such (the review confirmed this; pretending otherwise was the defect).

| ID | Invariant | Enforcing structure | Enforcement class | Tests / formal method | Monitoring / response |
|---|---|---|---|---|---|
| I-1 **[updated]** | **Per-branch ledger conservation over the enlarged instrument set.** For every proposal vault and each branch `b ∈ {Accept, Reject}`: `escrowed == supply(bUSDC_b) + Q_b + Σ_g G_{b,g}`, where `Q_b` = scalar complete sets (LONG_b/SHORT_b) and `G_{b,g}` = gate complete sets (GateYes/GateNo per gate `g`) outstanding in branch `b`; for every epoch-keyed Baseline vault: `escrowed == Q_baseline`. Total possible payouts ≤ escrow in every reachable state. Exact operation semantics and the induction proof live in [03-conditional-ledger.md](03-conditional-ledger.md). | `pallet-conditional-ledger`, per-branch supply fields (no single `branch_pairs` counter — the underflow of review B-4 is structurally impossible) | machine/state | PT-1/PT-3 over the full §4.2 operation set; TLA⁺ ledger model; `try-state` per block in test envs | collateralization gauge; PB-ORACLE-VOID / PB-LEDGER-FREEZE ([09](09-execution-upgrades-and-rollout.md)) |
| I-2 **[updated]** | **Annulment, stated honestly.** No code path mints a single-branch claim against collateral. Consequently: (a) complete-set holders always recover par by `merge`/`merge_scalar`, in every vault state including `Voided`; (b) market buyers recover par under annulment/VOID because the trade wrapper credits them the mirror branch-USDC ([04-markets-and-pricing.md](04-markets-and-pricing.md), D-3); (c) a **deliberately unpaired** single-branch holder recovers the neutral value of a voided binary claim — 0.5 per branch-USDC unit — **not** par. Clause (c) is the correct price of the claim, not a loss of principal on any protocol path. | ledger structure + D-3 trade wrapper | machine/state | PT-2/PT-4 (restated, §4.3) | — |
| I-3 | Exactly-once `resolve`, only by ResolveAuthority, only post-decision | ResolveAuthority + state flag | machine/state | model check | event audit |
| I-4 | Σ vault escrow ≤ ledger sovereign balance (equality up to genesis endowment + swept dust) | accounting | machine/state | PT-1; `try-state` | reconciliation alert → I-4 drift flag → PB-LEDGER-FREEZE admissibility ([09](09-execution-upgrades-and-rollout.md)) |
| I-5 **[updated]** | Redemption exactness: winning branch-USDC 1:1; scalar paired via `redeem_scalar_pair` pays exactly `a` per set; unpaired LONG `floor(a·s)`; unpaired SHORT `floor(a·(1−s))` — both floors round against the claimant (review B-5 closed); gate sets settle 1:0 per `settle_gate` outcome; `Voided` per I-26 | ledger | machine/state | unit + fuzz; PT-3 | — |
| I-6 | No stored param outside [min,max]; Δ ≤ max_delta; cooldowns respected | `pallet-constitution` | machine/state | unit + SMT on bound logic | parameter feed |
| I-7 | Rate meters monotone in-window; kernel envelopes never exceeded | constitution meter primitive (bounded, windowed, no-op on refusal) + the envelope meters in their owning pallets (treasury outflow/issuance, guard upgrade-spacing) | machine/state | property tests | meter dashboards |
| I-8 | Values scope ⟂ beliefs scope | track filters + screening | machine/test | filter-exhaustiveness test over `RuntimeCall` | — |
| I-9 | Only epoch-decide enqueues; only the guard executes | origin plumbing | machine/state | model check with I-15 | ExecutionRecords audit |
| I-10 | No external origin reaches Root; internal Root limited to `authorize_upgrade(committed hash)` | SafetyFilter + guard (wrapper set closed incl. `proxy_announced`, `as_multi_threshold_1` — [06](06-governance-and-guardians.md)) | machine/state | negative tests via every wrapper | — |
| I-11 | Executed batch domains ⊆ declared; wrappers recursively filtered | guard step 5 + SafetyFilter | machine/state | fuzz over nested wrappers | bond slash |
| I-12 | LMSR loss ≤ b·ln 2 + rounding bound; books pre-collateralized (revenue recycled into complete sets per D-3) | market + ledger | machine/state | differential vs MPFR; fuzz | book P&L gauge |
| I-13 | TWAP recorded drift ≤ (1±κ)^k per gap; slew cap per 10-block observation interval *(normative value: [13](13-parameters.md))* | accumulator | machine/state | fuzz + reference diff | dispersion alert |
| I-14 | Gate vetoes evaluated before welfare; no override path exists | `decide()` kernel ordering | machine/test | branch-coverage proof + model check | — |
| I-15 | No rejection/timeout/veto/expiry path enqueues or executes | state machine (incl. T21–T23, [05](05-welfare-and-decision-engine.md)) | machine/test | exhaustive state-machine model check | — |
| I-16 | Cohorts settle on creation-time MetricSpec | welfare snapshot binding | machine/state | unit | — |
| I-17 | Issuance/outflow/upgrade-spacing kernel meters | limits as `pallet-constitution::Params` records; metering where the flows live — `pallet-futarchy-treasury` (issuance 2%/365 d, outflow 30/180 d rolling caps, [08](08-treasury-and-economics.md)) and `pallet-execution-guard` (`code.spacing`, [09](09-execution-upgrades-and-rollout.md)) | machine/state | property tests | meters |
| I-18 | Only challenge-closed values settle money; contested ⇒ neutral/VOID | oracle + welfare | machine/state | model check | dispute alerts → VOID |
| I-19 **[updated]** | **Bonded upgrade attestation.** Only the queue-time committed artifact hash is authorizable; `set_code*`/`*_without_checks` are unreachable from genesis for all origins including sudo (D-13); and authorization additionally requires **2-of-3 signed attestations from the bonded, values-elected attestor registry (≥ 3 members), each subject to a challenge window** — presence of a 32-byte hash is no longer sufficient (D-18). `RejectReason::AttestationMissing` is produced both by the decision path at decide time (T10) and by the execution guard's dispatch-time re-check (T16) when the committed record is revoked/challenge-upheld post-queue ([09](09-execution-upgrades-and-rollout.md) §1.2(5); dual-sited by design, 05 §1.3). | filter + guard + attestor registry ([06](06-governance-and-guardians.md), [09](09-execution-upgrades-and-rollout.md)) | machine/state | negative tests incl. forged/under-quorum/challenged attestations | upgrade status page; refuse artifact |
| I-20 | Hooks constant-time; batch work cursor-bounded | design rule + review | convention (weight-regression CI + benchmark gates) | benchmarks + weight-regression CI | block-weight telemetry |
| I-21 | ≤ 4 non-terminal cohorts; ≤ 32 live proposals; ≤ 196 live markets; all collections bounded *(normative values: [13](13-parameters.md))* | type bounds | machine/state | `MaxEncodedLen` compile checks + tests | storage growth |
| I-22 | Every rate limit sits on the dispatch path (never advisory) | **convention — no runtime structure enforces this.** CI enforcement: the limit-coverage suite (§4.6) is generated from the [13-parameters.md](13-parameters.md) registry — every meter/bound key MUST have a test that drives a dispatch past it and asserts failure; a registry key without a matching test fails CI. Plus code-review checklist item. | convention | generated integration tests | — |
| I-23 | Guardian actions ∈ enumerated power set; every action reviewed | guardian pallet | machine/state | unit + scope tests | action feed; recall |
| I-24 | **Directional, fail-static** ([07](07-oracle-and-disputes.md) §1 restatement, carried verbatim): *no XCM outcome may move any decision or settlement input toward adoption or toward higher settlement; absence of a response is always the pessimistic case.* Concretely: the XCM-health sub-metric `X` reads local chain counters only, and the sole XCM-derived settlement input is the class-3 reserve probe `R` ([07](07-oracle-and-disputes.md) §8), whose output is monotone fail-static — a missing, timed-out or failed probe response can only *lower* `R`, never raise it or any other input | **convention — architecture rule.** CI enforcement: a lint denying `xcm`/`pallet-xcm` type imports and XCM-derived storage reads in the decision/settlement pallets (`epoch`, `welfare`, `market`, `conditional-ledger`); in `oracle`, the only XCM-adjacent surface permitted is the `R` probe's `QueryResponse` handler ([05](05-welfare-and-decision-engine.md), [07](07-oracle-and-disputes.md)). Plus review. | convention | CI lint + review; probe-timeout drill asserts `R` monotonicity | — |
| I-25 **[new]** | No canonical-frontend protocol workflow depends on any off-chain service other than chain P2P and content-addressed static hosting (FE §30 P-9, accepted by D-2) | frontend architecture: package firewall + provenance typing ([10](10-frontend-architecture.md)) | machine/test (frontend CI) | frontend no-indexer/no-RPC e2e suite (§4.8) | frontend release blocked |
| I-26 **[new]** | **VOID conservation.** In `VaultState::Voided`: total payout over any interleaving of `merge`, `merge_scalar`, gate-set merge and `redeem_void` is ≤ E, with pairs paying exactly 1 USDC per pair and unpaired claims paying the D-1 schedule — branch-USDC `floor(a/2)`; unpaired single-leg scalar or gate instruments `floor(a/4)` *(normative values: [13](13-parameters.md); operation semantics: [03](03-conditional-ledger.md))*. Rounding residue is swept per the dust rule, never paid out twice. The 2× insolvency of review B-1 is excluded by construction: no instrument redeems at par unpaired. | ledger `Voided` state machine | machine/state | PT-6 (§4.3); `try-state` | collateralization gauge |
| I-27 **[new]** | **Voided call surface.** Under `Voided`, exactly `merge`, `merge_scalar`, gate-set merge, `transfer` and `redeem_void` are dispatchable against the vault; `split*`, `buy`/`sell`, `resolve`, `settle_*` and ordinary `redeem*` MUST error. `void(pid)` is callable only by ResolveAuthority and emits `VaultVoided`; the T20 path emits its event (X-11f closed in [02](02-integration-contract.md)). | ledger state machine | machine/state | state-machine model check extension; negative tests | event audit |

**try-state coverage rule (normative).** Every pallet MUST implement a `try-state` hook checking each of its machine/state invariants above (ledger: I-1/I-2/I-4/I-5/I-26/I-27 identities per vault including Baseline vaults; constitution: I-6 and the meter-primitive half of I-7; treasury and execution guard: their I-7/I-17 envelope meters; epoch: I-16/I-21; market: I-12 collateralization, I-13 accumulator sanity; guardian: I-23 scope). `try-state` runs in every Chopsticks and Zombienet CI job and inside `try-runtime` on every upgrade (§4.7). A `try-state` failure in any environment is release-blocking.

---

## 2. Frontend invariants INV-FE-1…15 — normative texts (F-3)

The superseded FRONTEND_PLAN.md cited INV-FE-1…15 at ~79 sites and certified against them (§32) without ever stating their texts (review F-3). The texts below are published as normative. **From this document forward, frontend certification (§6) binds to these texts and to nothing else.** Provenance of each text is declared in §2.2; where the source usage was too thin to pin a text exactly, the text below is the most conservative statement consistent with every citation site.

> **INV-FE-1 (authoritative reads).** The client-embedded light client is the only authoritative read path. Every transaction-critical value — any value whose incorrectness could change what a user signs or their understanding of what signing does — is ultimately obtained from **finalized** chain state whose proofs the light client verified, or computed client-side purely from such verified inputs. No RPC endpoint, indexer, provider, cache, or best-head datum may be the source of a transaction-critical value. Data obtained over the quarantined RPC fallback is **never promoted** to verified status; verified status requires a light-client re-read (D-6/F-2, [10-frontend-architecture.md](10-frontend-architecture.md)).
>
> **INV-FE-2 (pre-sign revalidation).** Immediately before signature, every declared precondition of the transaction is re-read at a single finalized block, and the live runtime version is re-checked against the version the payload was built for; any failure blocks signing with a diff of expected vs actual. This refresh is structural on every submit path — no code path can reach a signer without it.
>
> **INV-FE-3 (acceleration firewall).** Optional acceleration (community indexers, snapshots, any historical provider) is never transaction-critical: provider-supplied data never satisfies a precondition, never renders a protocol-outcome state ("passed / settled / mature / final / safe") without a direct chain read, and is barred from the transaction path **structurally** — by build-time package dependency boundaries enforced in CI, including inside the application package itself — not by reviewer vigilance.
>
> **INV-FE-4 (zero-infrastructure operation).** Every protocol workflow — epoch/proposal discovery; proposal detail including reading the exact committed proposal payload bytes and hash-checking them client-side; trading; positions, split/merge/transfer and every redemption path including `Voided`; proposal submission; oracle reporting and challenges; execution-queue interaction and permissionless cranks; governance referenda (vote/delegate/unlock, ratification, `OracleResolution`); operator workflows; and USDC funding/withdrawal ([11-frontend-workflows.md](11-frontend-workflows.md)) — operates against finalized, light-client-verified chain state with **zero optional infrastructure available**: no indexer, no RPC endpoint, no provider, cleared local storage.
>
> **INV-FE-5 (no key custody).** The application never holds, derives, stores, or transmits private keys or any signing secret. Signing occurs exclusively in the user's chosen wallet (browser extension or raw-payload external signer); the application boundary carries only encoded payloads outward and signatures inward.
>
> **INV-FE-6 (serverless client).** The canonical frontend is a static, content-addressed bundle: no application backend, no server-side rendering, no required RPC endpoint, no required indexer, and no feature whose correctness depends on any server operated for the application. Features that inherently require servers are out of scope rather than centralized.
>
> **INV-FE-7 (disposable local storage).** Browser-local storage is a non-authoritative cache and derived-history store. Its loss, corruption, or eviction is a performance and convenience event only: the transaction path never reads it, protocol workflows are unaffected, and the application rebuilds it automatically from chain data.
>
> **INV-FE-8 (single-operator tamper evidence).** No single operator — Arweave gateway, ArNS controller, RPC host, chain peer, indexer, snapshot publisher, or the CI system — can silently alter the application or its interpretation of chain state. Every such channel carries a detection mechanism: content-hash verification and the signed release self-check (distribution), light-client proof verification and quote double-derivation (chain), sampling and snapshot diffing (acceleration), multi-gateway resolution cross-checks (naming). Detected divergence is surfaced to the user; it is never silently repaired.
>
> **INV-FE-9 (provenance labeling).** Every displayed data item carries an explicit, typed verification status (`verified-finalized`, `verified-best`, `derived-local`, `provider`, `stale-cache`). Unfinalized/best-head data is display-only and never transaction-critical. There is no unlabeled rendering path: UI data components reject unlabeled values by type.
>
> **INV-FE-10 (reproducible releases).** Every release is reproducibly buildable: independent parties following the published recipe obtain byte-identical output whose tree hash equals the published, multi-party-signed release manifest; release identity is content-addressed. A user can independently verify *which* application they run, from source commit to served bytes, without trusting the release team or CI.
>
> **INV-FE-11 (pinned identity, visible provenance).** The bundle pins the complete identity of what it is and what it talks to — release content address (TXID), source commit, per-file hashes, descriptor metadata hashes, supported spec-version range, chain-spec hashes, and relay + parachain genesis hashes — verifies the chain identity at boot (genesis mismatch is terminal), and displays all of it in an always-available verification panel.
>
> **INV-FE-12 (fail-safe under unknown runtimes).** When the runtime surface is unknown or partially incompatible, the application fails safe into explicit `restricted` or `read-only-incompatible` modes: signing is disabled wherever compatibility is unproven, reads continue only where compatibility probes pass, undecodable data renders as raw SCALE with an explicit warning, and the application **never guesses at encodings**.
>
> **INV-FE-13 (no telemetry, no remote configuration).** The application emits no telemetry or analytics of any kind, and has no remote-configuration channel: every provider suggestion, locale, bootnode list, default and text ships inside the signed release; user settings live only on the device. Application behavior changes only by shipping a new, independently verifiable release.
>
> **INV-FE-14 (signing transparency).** Everything a user signs is fully inspectable before signing: a human-readable summary, a structured decoded tree, and the exact raw SCALE bytes with their payload hash — all derived from the same bytes that will be signed, never from UI form state — plus signed-extension details. Expert mode exposes the raw storage keys and SCALE values behind every precondition.
>
> **INV-FE-15 (replaceable, reproducible, labeled acceleration).** Historical acceleration is optional, user-selected, and interchangeable: snapshots are deterministic, content-addressed exports that anyone can reproduce and verify byte-for-byte; live providers implement a minimal open read-only interface anyone can operate; every provider-derived row carries its origin to the pixel; providers are sampled against chain state where feasible and auto-disabled on mismatch. Historical analytics **degrade gracefully**: verified data is shown as verified; everything else is either absent with an explanation or present and labeled — gaps are first-class and visible, never silently spliced (D-6).

### 2.1 Amendment discipline

These texts are normative and version-controlled with this document. Amending any INV-FE text requires the same joint sign-off as [02-integration-contract.md](02-integration-contract.md) changes. D-11 extended the *workflow list* inside INV-FE-4 (governance, operator, funding surfaces) without amending the invariant's obligation — that reading is ratified here.

### 2.2 Provenance of the texts

| Invariant | Basis | Confidence |
|---|---|---|
| INV-FE-1 | Partially quoted in old FE §11.8 ("ultimately… light-client-verified"); ≥ 8 citation sites incl. the Alt-D disqualification ("only authoritative read path") | faithful reconstruction |
| INV-FE-2, -3, -4, -6, -7, -9, -12, -13, -15 | ≥ 5 citation sites each with mutually consistent obligations (goals FG-1…FG-5, §1 architecture list, §32 compliance rows, per-section usage) | faithful reconstruction |
| INV-FE-8 | FG-3 states the obligation nearly in full | faithful reconstruction |
| INV-FE-5, -10, -11, -14 | 2–4 citation sites each; obligations clear, exact original wording unrecoverable | **conservative draft (thin evidence)** — text chosen as the strongest statement every citation site satisfies |

No citation site of the old document is inconsistent with any text above; where a weaker reading was possible, the stronger (more constraining) reading was chosen so that certification against these texts cannot certify less than the old document claimed.

---

## 3. Required-question blocks, required-UX rows, decision biases (F-3, remainder)

The old frontend document answered "required-question blocks" A–J and cited "required UX" rows, "decision biases", and skeleton/diagram/impl-detail numbering schemes that were never published. Disposition:

- **Block A** was stated in old FE §6.1 and is carried forward: its question texts appear verbatim in §3.1; the answers are owned by [10-frontend-architecture.md](10-frontend-architecture.md) (trust model) and [12-release-and-operations.md](12-release-and-operations.md) (distribution).
- **Block E** rows were self-describing (the review confirmed) and are owned as the required-UX matrix by [11-frontend-workflows.md](11-frontend-workflows.md); the normative row *list* is §3.3.
- **Blocks B–D and F–J** are published in §3.2, reconstructed from their answer sites. Reconstructed texts are marked **(R)**.
- The old document's **skeleton / required-diagram / impl-detail numbering schemes are retired** with it; the new doc set does not use them, and their dangling numbers die with the old documents (see §7).

### 3.1 Block A — distribution trust (question texts verbatim from old FE §6.1)

A1 What must a user trust to load the application? · A2 What does Arweave permanence guarantee; what stays gateway/name-dependent? · A3 Single-gateway dependence avoided how? · A4 How does the client verify downloaded files match the intended release? · A5 ArNS mutability effect on the trust model. · A6 Pinning an exact immutable release. · A7 Who updates the canonical ArNS pointer? · A8 Reproducible builds and multi-party attestations represented how? · A9 What changes through an ordinary HTTPS gateway? · A10 Remaining unavoidable trust assumptions.

Answered in: [10](10-frontend-architecture.md) (A1–A5, A9, A10), [12](12-release-and-operations.md) (A6–A8; permabuy and controller policy per D-16).

### 3.2 Blocks B–D, F–J (R — reconstructed from answer sites; answers owned by the docs cited)

**Block B — light client in the browser** → [10](10-frontend-architecture.md). B1 How is the light client initialized in-browser? B2 Which chain specs ship in the bundle, and how are they pinned? B3 How does a browser reach peers (bootnode requirements)? B4 How do the relay and parachain connections relate; what does parachain finality derive from? B5 How is sync progress surfaced to the UI? B6 What renders before sync completes, and how is it labeled? B7 What happens when peer discovery fails? B8 Is an RPC fallback permitted at all? B9 If so, how is fallback data isolated and labeled? B10 How is chain identity (genesis) verified at boot? B11 How are finalized and best heads distinguished programmatically? B12 What is the worker lifecycle? B13 What is the multi-tab behavior? B14 Which browser platform limitations apply, and how is each handled?

**Block C — typed API and runtime compatibility** → [10](10-frontend-architecture.md). C1 How are descriptors generated, and against what artifact? C2 How are descriptors tied to source commits and metadata hashes? C3 How are multiple runtime versions supported simultaneously? C4 What happens when the live runtime is not covered? C5 What uses generated typed calls? C6 What uses custom runtime APIs, and why? C7 How are runtime-API results authenticated through the light client? C8 How is a payload presented for review? C9 How are stale parameters and mid-session upgrades handled? C10 What gates signing when the runtime version changes?

**Block D — data model and discovery** → [10](10-frontend-architecture.md), with the runtime surface in [02](02-integration-contract.md). D1 What is the exact source of every displayed data class? D2 How does a fresh browser discover all live protocol objects without history? D3 What does each workflow read? D4 What is the minimal runtime addition set (runtime API family, bounded storage)? D5/D6 What are its bounds and weight costs? D7 Which discovery additions are rejected to keep the chain bounded — see also UX-D7 in §3.3. D8 What stays event-derived only? D9 What is the local historical index for (and not for)? D10 Where does first synchronization start? D11 What are the resume semantics and the exact transaction boundary of the ingest cursor? D12 How are metadata changes handled during historical decoding? D13 How are duplicate/partial processing prevented? D14 What is the storage schema? D15 How are storage failure, corruption, and eviction handled? D16 What are the growth estimates and eviction rules? D17 What is deliberately unavailable without a provider? D18 How does the UI explain incomplete history? D19 What shapes may optional providers take? D20 How is provider data verified, and what exactly is not guaranteed?

**Block F — build, distribution, release** → [12](12-release-and-operations.md). F1 Build tool and determinism rationale. F2/F3 Routing strategy compatible with content-addressed hosting and deep links. F4/F5/F6 Immutable, canonical-mutable, and environment URL formats. F7 Who repoints the canonical name; what is the rollback path? F8 How do old releases remain reachable forever? F9 CSP (connect-src allowlist per D-16/F-med — not `*`). F10/F11 Service-worker scope and update flow (fail-closed, user-activated). F12 Subresource integrity. F13 Release manifest and SBOM contents. F14 Signing and multi-party attestation. F15 Independent verification commands any third party can run.

**Block G — wallets and transactions** → [11](11-frontend-workflows.md). G1/G2 Wallet connection strategy and account discovery/permissions. G3 Signer abstraction over injected and raw-external signers. G4 Transaction lifecycle states, with finalized as the only success state. G5 Fees (incl. USDC fee payment via `fee.vit_usdc_rate`, D-12) and dry-run posture. G6 Final pre-sign refresh (the INV-FE-2 mechanism). G7 Payload presentation and anti-substitution defenses. G8 Mortality policy. G9 Nonce management and races. G10 Rejection / replacement / dropped handling. G11 Multisig, proxy, and hardware-signer support. G12 Address-substitution defenses. G13 Signing while the chain advances.

**Block H — threat model** → [14-threat-model.md](14-threat-model.md). For each trust boundary (distribution, naming, chain access, acceleration, device, supply chain, human, privacy): enumerate the threats in the fixed format *attack · boundary · preventive control · detection · user-visible behavior · recovery · residual risk*, with no empty residual cells — accepted residuals are named, not omitted.

**Block I — testing** → this document, §4.8: the required frontend test-layer list is normative there and maps 1:1 onto the old block I.

**Block J — parent-document integration** → [02-integration-contract.md](02-integration-contract.md). What must the chain provide for the canonical frontend to exist (runtime API family, events, bounded rings, identity constants, bootnodes, test artifacts); what is the disposition of the legacy indexer; which WBS rows on each side implement and gate the contract. J is answered by the frozen contract itself; the old §30 patch mechanism is superseded (all items accepted per D-2).

### 3.3 Required-UX row list (normative index; full matrix in [11-frontend-workflows.md](11-frontend-workflows.md))

Rows E1–E14 carry forward from the old §19 (self-describing, review-confirmed): E1 first visit desktop · E2 first visit mobile · E3 returning visit (gap-tolerant catch-up per D-6 — "gaps visible and provider-fillable, never silently spliced") · E4 returning after runtime upgrade · E5 deep link to an old proposal · E6 viewing an active market · E7 ten-month price chart · E8 all proposals by an address · E9 IndexedDB corruption · E10 all optional providers down · E11 preferred gateway down · E12 slow peer discovery · E13 signing while chain advances · E14 obsolete release detected.

New rows added by the decision record **(R)**: E15 VOID redemption (merge-first primary action when pairs are held, `redeem_void` otherwise; D-1) · E16 USDC funding and withdrawal incl. the Asset Hub leg (D-12) · E17 governance surface — referenda list/detail, vote/delegate/undelegate/unlock, ratification status, `OracleResolution` ballot (D-11/FE-14) · E18 operator surface — reporter, guardian signing, treasury claims, `apply_authorized_upgrade`, cranks (D-11/FE-15) · E19 bootstrap-sudo banner during Phases 0–3 and the pinned-release upgrade warning fed by the fixed-layout `ReleaseChannel` key (D-13, D-14).

**UX-D7 (R) — deep-history availability as a UX requirement.** The old document used "D7" both as a discovery-discipline question (§3.2 block D) and as a required-UX citation. The UX obligation is published as: *long-horizon historical views (price charts beyond checkpoints, address history, archived-proposal detail) MUST be achievable through some labeled path — locally accumulated or provider-supplied — for every user willing to enable one; they MAY degrade, but MUST NOT be structurally impossible for everyone forever.* (This is what disqualified the strict no-index alternative.)

### 3.4 Decision-bias texts (normative; **(R)** except quoted fragments)

- **DB-1 Graceful degradation.** "Advanced historical analytics degrade gracefully rather than preventing protocol use" *(quoted)*: degradation is labeled, explained in place, and recoverable — never silent, never structural.
- **DB-2 Practical decentralization.** "A 'decentralized' app nobody can load is centralization by another name" *(quoted)*: load-time and resource budgets are release gates, not aspirations.
- **DB-3 Bounded on-chain discovery.** On-chain discovery aids are added "when operationally essential and tightly bounded" *(quoted)*; anything with unbounded key-space or duplicating derivable data stays event-derived off-chain.
- **DB-4 Anti-overengineering.** No indexing network, no bespoke protocol, no token/incentive design unrelated to the protocol; the smallest architecture satisfying every invariant and every required-UX row wins.
- **DB-5 Convenience is never load-bearing.** Every convenience path (RPC fallback, providers, best-head hints) is quarantined, labeled, and removable without breaking any INV-FE-4 workflow.

---

## 4. Testing and verification regime (merged BE §24 + FE §23)

### 4.1 Backend: mock-runtime, integration, model checking

Per pallet: mock-runtime unit tests covering every extrinsic × every error path × origin misuse. Cross-pallet integration tests on the full runtime: complete lifecycles for all proposal classes — including the T21/T22/T23 paths (`Rejected → Measuring`, retry exhaustion/success) and the rerun `Extended` path ([05](05-welfare-and-decision-engine.md)) — with all worked examples of [09](09-execution-upgrades-and-rollout.md) scripted as regressions. TLA⁺ (or Quint) models of the proposal machine and ledger resolution prove I-3, I-14, I-15, I-18, I-26/I-27 over all interleavings including guardian, oracle-dispute, `void` and T20 events.

### 4.2 Property tests over the full enlarged operation set

`proptest` suites MUST generate random sequences over the **complete** ledger operation set of [03-conditional-ledger.md](03-conditional-ledger.md) — `split`, `merge`, `split_scalar`, `merge_scalar`, gate-set split/merge, `transfer`, `resolve`, `void`, `settle_scalar`, `settle_gate`, `redeem`, `redeem_scalar`, `redeem_scalar_pair`, `redeem_void`, `reap`, `sweep_dust`, and the epoch-keyed Baseline-vault operations — not the pre-review three-instrument subset. Any operation added to 03 without a generator arm fails a completeness check (the generator enumerates the pallet's call metadata).

### 4.3 Ledger property-test set (restated over the enlarged set)

- **PT-1 conservation:** random op sequences (10⁶ cases) maintain the per-branch I-1 identities, for proposal vaults (both branches, gate terms included) and Baseline vaults.
- **PT-2 annulment (restated per D-1/D-3):** (a) for random strategies that only split/trade via the wrapper, VOID ⇒ net principal delta = −fees only (mirror branch-USDC makes buyers whole); (b) for strategies that deliberately unpair, VOID recovery equals the D-1 schedule exactly. The old "no loss of principal for any single-branch holder" claim is **not** a test target — it was false.
- **PT-3 rounding:** Σ payouts over all holders after full redemption ∈ [E − holders, E]; paired redemptions (`redeem_scalar_pair`, gate pairs) pay exactly `a` per set; every unpaired floor rounds against the claimant (covers the B-5 counterexample class).
- **PT-4 no-mint-outside-declared-ops:** supply changes occur only in the enumerated minting operations, per instrument kind including `GateYes`/`GateNo` and Baseline positions.
- **PT-5 reap safety:** reap never executes while any position balance > 0 unless the archive delay elapsed; archived residue equals Σ outstanding claims valued at settlement.
- **PT-6 VOID conservation [new]:** in `Voided`, any interleaving of the I-27 call surface pays out ≤ E; first-redeemer strategies gain nothing (order-independence of total payout up to rounding residue).
- **PT-7 gate-set conservation [new]:** per branch, gate-set mint/merge/settle preserves the I-1 identity for every gate; `settle_gate` outcomes pay 1:0 exactly.

Plus (unchanged in scope): meter monotonicity, TWAP drift bound, LMSR loss bound, state-machine legality.

### 4.4 Differential testing and the CI-regenerated vector corpus

`futarchy-fixed` exp/ln/cost vs an MPFR-256 reference — per-commit CI over the generated adversarial corpus (≥ 10³ points incl. dense-bit fractional inputs and domain edges), and the full ≥ 10⁷-point sweep in the release pipeline plus on any change to the fixed-point kernel or the reference numerics (cadence normative in [04 §4](04-markets-and-pricing.md)); decision engine and ledger vs the independent executable Python reference model driven by shared JSON scenario vectors. **For the [05 §4.4](05-welfare-and-decision-engine.md) welfare pipeline the reference model is one of the "two conforming implementations": it MUST reproduce the per-step 64.64/1e9 rounding grid-exactly (bit-identical pillar values, `W_e`, `s`); only for the transcendental kernels is it the higher-precision oracle, judged within the 04 §4 bounds.** All shared fixtures derive from the single reference-model generator (04 §5); parallel generators are forbidden. **The normative LMSR vectors V1–V6 are regenerated in CI from the reference model on every change to the fixed-point crate or the vectors themselves — they are never hand-maintained** (the hand-computed V1 error that shipped in the old spec is the standing justification). Current normative values: **V1 = 512.494795136 USDC**, **V5 net = −3.074969**; V2–V4 unchanged *(normative values: [04-markets-and-pricing.md](04-markets-and-pricing.md), [13-parameters.md](13-parameters.md))*. The same JSON corpus is the seed for the frontend's TypeScript port (§4.8), so both sides certify against one generated artifact.

### 4.5 Fuzzing, benchmarks, weights

cargo-fuzz on SCALE decoding of payload batches, nested-wrapper filtering (incl. `proxy_announced`, `as_multi_threshold_1`), and LMSR trade paths. `frame-benchmarking` for every call and hook; CI gates on > 10 % weight regressions; PoV-size tracking against the budgets derived in [13-parameters.md](13-parameters.md) (196-market table). **Supply-chain gates (TH-34):** every backend CI run asserts the committed lockfile (`cargo --locked`) and a clean `cargo-audit` (RustSec) pass under the [01 §9](01-system-overview.md) exact-pin regime; the §5 release pipeline MUST NOT publish artifacts from a run with either gate red.

### 4.6 Generated limit-coverage suite (I-22 enforcement)

Generated from the parameter/meter registry in [13-parameters.md](13-parameters.md): for every rate limit, meter, and bound, one integration test drives a dispatch past it and asserts the specific error. Registry keys without tests fail CI. This is the CI half of I-22's convention enforcement; the other half is the review checklist.

### 4.7 Upgrades, multi-node, forked-state

`try-runtime-cli` against testnet/mainnet state snapshots for **every** upgrade (storage-version assertions; `pallet-migrations` continuation/interruption tests; `try-state` executed post-migration). **Zombienet** relay + parachain topologies exercising epoch progression under collator loss, keeper loss, and relay finality stalls (dead-man test), plus the XCM reserve-transfer suites including failure, trap, and recovery. **Chopsticks** forked-state dry runs of every upgrade and every playbook, including PB-LEDGER-FREEZE entry/expiry and the coretime exemption ([09](09-execution-upgrades-and-rollout.md)). The Zombienet/Chopsticks environment definitions are release artifacts (§5), not private CI fixtures.

### 4.8 Frontend verification suites (required block I; consumes §5 artifacts)

| Layer | Scope | Gate |
|---|---|---|
| Unit + property | protocol math (TS LMSR/TWAP vs the §4.4 corpus: cost path-independence, loss ≤ b·ln 2, slew bound), cursor logic, compat gating, precondition evaluators, manifest verification; ingest idempotency under random crash/replay; eviction never touches tx-path tables | PR |
| Descriptor compatibility | CI regenerates descriptors from **each published runtime artifact (§5)**, diffs vs committed, runs the critical-surface probe suite against each metadata; drift fails | PR + every runtime release |
| Mock-runtime | client against the **published chainHead fixture transcripts (§5)** for every screen store | PR |
| Chopsticks | forked-state: upgrade transition (full → read-only-incompatible → newer release), stale-queue display, **Voided-epoch rendering incl. the E15 redeem flow**, manufactured precondition failures | nightly |
| Zombienet | full relay+para: e2e proposal lifecycle through the real UI data layer, cranks, execution, settlement, dead-man drill — against the **published topology (§5)** | nightly + release |
| Browser/device, wallets, distribution | boot machine, tx machine, degradation rows E1–E19 scripted; browser grid; wallet matrix incl. raw-payload and multisig; Arweave routing/failover fault injection (tampered bytes fail closed) | release |
| Malicious-provider + corruption | lying indexer ⇒ sampler auto-disable; forged-snapshot corpus rejected per class; IndexedDB corruption/migration paths | PR |
| No-infra certification run | every INV-FE-4 workflow with providers disabled, RPC disabled, cleared storage, one failed gateway (I-25 evidence) | release gate |
| Reproducible build + attestation | two independent environments byte-identical; `verify-release` from a clean container; ≥ 2 attestation signatures | release gate |

### 4.9 Economic simulation, contests, audits

Agent-based adversarial simulation (informed traders with planted ground truth, noise traders, arbitrage capital, scripted manipulators with [14](14-threat-model.md) budgets) across ≥ 10⁴ synthetic proposals: calibrates δ per class to a false-pass rate < 1 %, validates the measured-depth `AttackCost̂` mechanism of D-4, and sizes POL. Public testnet contests with paid bounties for wrongful PASS, oracle corruption, or invariant breach — scope explicitly includes the frontend classes (tampered bundle, malicious provider, forged snapshot). ≥ 2 independent audit firms (scope A: ledger + constitution + guard; scope B: markets + decision + oracle) plus a mechanism-design review; scope-A re-audit before the final phase gate on any ledger change. Standing bug bounty from genesis.

---

## 5. Cross-repo test artifacts (X-15)

The frontend's compatibility controls (§4.8) previously had no defined input feed. The publication contract below is normative; the exact schema and location constants of artifacts (1)–(4) are frozen in [02-integration-contract.md](02-integration-contract.md) §11 (the chain↔frontend runtime surface), while artifact (5) — the reference-model vector corpus — is schema-owned by [04-markets-and-pricing.md](04-markets-and-pricing.md) §5: 02 was finalized without a corpus row, deliberately, because the corpus is a test-time input consumed by suites, never by the running frontend.

**Per-release publication (backend duty).** For every tagged runtime release, backend CI MUST publish, as content-addressed release artifacts:

1. the reproducibly built **runtime wasm** with its hash, and the **SCALE metadata** with its metadata hash (the descriptor-generation inputs);
2. the **chain-spec artifacts** for every live environment;
3. the **Chopsticks configuration and Zombienet topology** files the backend's own §4.7 suites ran against;
4. **chainHead fixture transcripts** (deterministic JSON-RPC recordings) covering every storage item, event, and runtime-API call in the integration contract's critical surface;
5. the **reference-model JSON vector corpus** of §4.4 (LMSR V1–V6 + MPFR corpus + decision/ledger scenario vectors), as regenerated by that release's CI.

Artifacts are published to the backend repository's release feed and mirrored content-addressed (Arweave) so frontend release verification never depends on a mutable host.

**Gating rule (binds both WBS rows).** Backend row **E15** and frontend row **FE-R1** implement the two sides of this contract; both are **release-gating**: a runtime release is not publishable until its artifact set (1)–(5) is published and the backend's own suites ran against exactly those artifacts; a frontend release is not publishable until §4.8's descriptor-drift, fixture, Chopsticks and Zombienet suites are green **against the published artifacts of every runtime version in its supported range**. A runtime change that breaks `FutarchyApi` or critical-surface compatibility MUST NOT merge without a coordinated frontend release (old P-10, accepted by D-2), and the D-14 descriptor lead time (`DescriptorLeadTime`, *normative value: [13](13-parameters.md)*) gives the frontend train the enforced window to consume a new artifact set before enactment.

---

## 6. Certification

The frontend compliance apparatus (old §32) is now verifiable because §2 exists: **certification binds to the INV-FE texts in this document.** A release is certified by publishing, in its release notes, the table below with every test column green for that exact artifact set:

| Invariant (§2 text) | Enforcing component(s) | Certifying test(s) (§4.8) |
|---|---|---|
| INV-FE-1 | finalized-pinned reads in the chain package; `Verified<T>` typing; no-promotion rule (D-6) | mock-runtime store tests; e2e no-RPC suite; lying-peer test **[VERIFY chainHead runtime-call verification semantics through smoldot — FE-P2 carried forward in [10](10-frontend-architecture.md); until verified, runtime-call results remain cross-checked against direct storage reads on every tx path]** |
| INV-FE-2 | `refreshAndGate` structurally on every submit path | tx-machine browser suite + Chopsticks manufactured-failure suite |
| INV-FE-3 | package firewall incl. in-app build-time import boundary; provider quarantine; actionable-object chain-fetch rule | dependency-cruiser CI; malicious-provider suite |
| INV-FE-4 | screen→source matrix, all sources = chain ([11](11-frontend-workflows.md)) | no-infra certification run (§4.8) over E1–E19 workflows |
| INV-FE-5 | static bundle; no secrets by construction | CI secret scan; release diff |
| INV-FE-6 | static output; no server package exists | Arweave routing suite; repo audit |
| INV-FE-7 | rebuild path; tx path never reads local DB | corruption suite; firewall |
| INV-FE-8 | content verification; quote double-derivation; precondition refresh; sampling; resolution cross-check | gateway-tamper, lying-peer, provider suites |
| INV-FE-9 | `VerificationStatus` union; badge-typed components | unit tests asserting rejection of unlabeled values; visual regression |
| INV-FE-10 | reproducible build + content addressing | two-environment hash-equality gate |
| INV-FE-11 | verification panel; boot identity check | e2e panel-vs-manifest assertion; wrong-genesis terminal test |
| INV-FE-12 | compat state machine; no-guess decoding | Chopsticks upgrade suite |
| INV-FE-13 | no remote-config code paths; in-bundle data only | code audit + release diff; CI grep gates on fetch-to-config patterns |
| INV-FE-14 | payload-derived rendering; expert mode | e2e expert-mode suite |
| INV-FE-15 | provider interface; reproducible snapshots; origin labeling; gap-visible history | provider suites; snapshot determinism test; gap-rendering test |

Backend certification is the invariant table of §1: each machine-class row is certified by its named suite in CI on every release; each convention-class row (I-22, I-24) is certified by its generated suite/lint **plus** a named reviewer sign-off recorded in the release notes — the honest form of "enforced by convention".

---

## 7. Reference hygiene

Every section reference in this document points at a document of the new set (00–15) or a numbered section of this document. The superseded documents' dangling references (the phantom backend §18.6 cited by both old documents, §16.4/§18.5/§19.6, and the old frontend's skeleton/diagram/impl-detail numbering) are not carried forward: they die with those documents (X-11j). No new phantom references are introduced; CI SHOULD lint relative links across `docs/architecture/`.

---

## Resolves

| Finding | Resolution in this document |
|---|---|
| F-3 | INV-FE-1…15 published as normative texts (§2) with declared provenance and amendment discipline; block A question texts carried verbatim, blocks B–D/F–J published reconstructed (§3.1–3.2); required-UX row list E1–E19 + UX-D7 published (§3.3); decision-bias texts published (§3.4); certification re-bound to the published texts (§6) |
| X-15 | Per-release backend artifact publication contract — wasm + metadata, chain-specs, Chopsticks/Zombienet environments, chainHead fixtures, vector corpus — with content-addressed mirroring and the E15/FE-R1 release-gating rule (§5); schema ownership assigned to 02 |
| B-1 / X-6 (invariant side) | Voided-state invariants I-26/I-27 added: total VOID payout ≤ E, D-1 payout schedule, restricted call surface, `VaultVoided`/T20 events; PT-6 added; PT-2 restated honestly (§1, §4.3) |
| B-2 / B-4 (invariant side) | I-1 restated per-branch over the enlarged instrument set (branch-USDC, LONG/SHORT, gate YES/NO per branch, Baseline vaults); PT-1/PT-4/PT-7 extended; single-counter underflow excluded structurally (§1, §4.2–4.3) |
| B-5 (invariant side) | I-5 updated: unpaired SHORT `floor(a·(1−s))`, paired exactness via `redeem_scalar_pair`; PT-3 covers the fragmentation counterexample class (§1, §4.3) |
| B-6 (regime side) | Differential corpus with CI-regenerated vectors; V1 = 512.494795136, V5 = −3.074969 quoted from 04/13; hand-maintained vectors prohibited (§4.4) |
| D-18 / B-med (attestation) | I-19 updated to the bonded 2-of-3 attestor-registry regime with challenge window; negative tests specified (§1) |
| Review open-questions (I-22/I-24) | Convention-only status stated honestly with an Enforcement-class column; CI enforcement defined (generated limit-coverage suite; XCM import lint) without overclaiming (§1, §4.6) |
| X-1 / D-2 (P-9, P-10) | I-25 adopted into the invariant table; frontend verification suites adopted as §4.8 with the no-merge rule bound to the §5 artifacts |
| X-11j (this doc's scope) | No phantom references; old dangling refs and numbering schemes retired (§7) |

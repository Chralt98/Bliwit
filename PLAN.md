# PLAN.md — Implementation Roadmap and Status

**This file is the single source of implementation status.** It tells every session
what is done, in progress, blocked, and next. It **references** the
specification in `docs/architecture/` (rule R-4, AGENTS.md) and never restates it:
if you need protocol content, follow the *Spec* column.

How to work this file (full protocol: AGENTS.md):

- Sessions pick **one** milestone: the 🔨 one, else the first ⬜ whose *Depends* are ✅.
- A milestone is ✅ only with green quality gates and a `spec-reviewer` pass without
  blockers (its *Verify* obligations are part of the milestone).
- Update this file **in the same session as the change**: status, *Current focus*,
  and a *Session log* row. Append to logs; never rewrite their history. Dates absolute
  (YYYY-MM-DD).

Legend: ⬜ pending · 🔨 in progress · ✅ done · ⛔ blocked · 🅿 deferred (post-v1)

---

## Current focus

**Current: M2 — `crates/futarchy-fixed`**. The 64.64 LMSR math crate now builds its transcendental surface in `no_std` mode with deterministic in-crate `f64` range reduction/series helpers, checked two-limb `u256`-style multiply/divide arithmetic, maker-adverse rounding helpers, stable two-outcome LMSR cost, marginal price/displacement helpers, explicit buy/sell delta helpers, regression coverage for V1–V4 plus the V6 domain edge, and an interim generated high-precision LMSR corpus fixture/generator; the interim corpus now includes nearest 64.64 raw expectations and a CI freshness check; remaining work is replacing this interim deterministic kernel with the spec-required ≤2 ulp polynomial/integer implementation and scaling the corpus to the full MPFR-256 gate before M2 can be marked complete.

---

## Milestones

### Track M — Foundations

| ID | Milestone | Spec | Depends | Status | Notes |
|---|---|---|---|---|---|
| M0 | Repo bootstrap: Cargo workspace, `rust-toolchain.toml`, CI skeleton (fmt/clippy/test + docs link lint), **re-verify platform pins** (SDK `polkadot-stable2603`, FE package pins) | 01 §9 | — | ✅ | Pin re-verification recorded in Verification log V-1 |
| M1 | `crates/futarchy-primitives` — `no_std` shared SCALE types + kernel `K` constants, `INTEGRATION_CONTRACT_VERSION = 2` (bump applied 2026-07-13 in doc 02, see Decision log) | 02 §2; 13 §2; 01 §5.2 | M0 | ✅ | Shared primitive/view types, bounded SCALE decoding, metadata derives, kernel/identity/currency constants, and lockfile are in place; Rust workspace gates passed 2026-07-13 |
| M2 | `crates/futarchy-fixed` — 64.64 fixed point, exp2/log2/ln, maker-adverse rounding, error bounds | 04 §4 | M0 | 🔨 | Interim no_std deterministic transcendental helpers landed; price/displacement and buy/sell helpers now cover V1–V4 and reject states/trades past the 48·b domain edge; checked multiplication/division now use in-crate two-limb `u256`-style intermediates; interim corpus has decimal + nearest-raw expectations and a CI freshness check; replace with the final ≤2 ulp polynomial/integer transcendental kernel, then scale the interim corpus fixture to the full MPFR-256 corpus gate |
| M3 | `reference-model/` — independent Python executable spec (LMSR, TWAP, ledger ops incl. gate/Baseline/VOID, welfare pipeline, decision rule, treasury arithmetic) + MPFR-256 corpus + V1–V6 regeneration in CI | 04 §5; 15 §4.4; 08 (worked arithmetic); 05 §4.4 | M0 | ⬜ | Never ports Rust code (`.claude/rules/reference-model.md`) |

### Track A — Protocol pallets

Every pallet milestone includes: mock runtime, per-extrinsic × error-path × origin-misuse
tests (15 §4.1), `try-state` per 15 §1, benchmark stubs (15 §4.5). Scaffold via `/new-pallet`.

| ID | Milestone | Spec | Depends | Status | Notes |
|---|---|---|---|---|---|
| A1 | `pallet-constitution` — typed/bounded/rate-limited params, meters, capability tables, `PhaseFlags`, `ReleaseChannel` fixed-layout key, kernel re-export | 06; 13 §1/§4; 02 §7.3/§12 | M1 | ⬜ | I-6, I-7, I-17 |
| A2 | `pallet-conditional-ledger` — vaults, per-branch supplies, split/merge/scalar/gate/Baseline families, `Voided` + `redeem_void`, internal wrapper API | 03 (all); 02 §6 | M1 | ⬜ | Audit scope A; L-1…L-6; PT-1…PT-8 (15 §4.2–4.3); frozen early |
| A3 | `pallet-market` — LMSR books (branch-USDC, D-3), trade wrapper, fees, TWAP, POL seeding, `BaselineMarketOf` | 04; 02 §5–§7.4 | A2, M2, M3 | ⬜ | I-12, I-13; differential vs V1–V6 + corpus |
| A4 | `pallet-origins` + `SafetyFilter` — 8 custom origins, closed wrapper set, call-domain derivation | 06 §3; 01 §6 | M1 | ⬜ | I-8, I-10, I-11 |
| A5 | `pallet-oracle` — reporter/watchtower registries, bonded reporting game, challenge rounds (72 h + quorum), reserve probe `R`, neutral settlement | 07 §1–§6, §8–§13 | A1 | ⬜ | I-18, I-24 |
| A6 | `pallet-registry` — Incident/Milestone instances, bonded filings, challenge windows | 07 §7 | A5 | ⬜ | Feeds `C_attested` |
| A7 | `pallet-welfare` — bounded counters, snapshots, `MetricSpec` registry, pillar pipeline (`C_onchain`/`C_attested`), gate-breach flags, settlement score | 05 §4, §6–§7 | A1, A5 | ⬜ | I-16; bit-identical conformance vectors (M3) |
| A8 | `pallet-epoch` — epoch/phase clock, proposal registry + T1–T24 machine, cohorts, `decide()` engine (11 steps incl. `SecuritySizing` D-4, ratification D-5), `RecentCohortSummaries` ring | 05 §1–§3, §5; 02 §7.1 | A2, A3, A7 | ⬜ | I-14, I-15, I-21; model check (S1) |
| A9 | `pallet-futarchy-treasury` — sub-accounts, NAV + haircuts, outflow meters/streams, budget lines, `issue_vit`, coretime renewal call | 08 §1, §6–§9 | A1 | ⬜ | I-7 meters; `nav()` view data |
| A10 | `pallet-guardian` + `pallet-attestor` — 7-seat council, powers, playbooks incl. PB-LEDGER-FREEZE, retro ratification; bonded 2-of-N attestor registry | 06 §5–§7 | A1, A4 | ⬜ | I-19, I-23 |
| A11 | `pallet-execution-guard` — queue, permissionless `execute()` (13-item dispatch list), class origins, two-phase upgrade flow, `DescriptorLeadTime`, `ExecutionRecords` ring | 09 §1–§3; 02 §7 | A8, A10 | ⬜ | I-9, I-10, I-11, I-19; FE `execute` row lockstep (11 §11.5) |

### Track B — Runtime, node and chain

Doc-09 WBS delta rows map here and to Track A: E15→B2/B8 · E16→A2 · E17→A3 · E18→A6 ·
E19→A5 · E20→A10/B6 · E21→B4 · E22→A11/B1. (E1–E14 definitions live in git history —
superseded `BACKEND_PLAN.md` §26; their scope is covered by Tracks M/A/B.)

| ID | Milestone | Spec | Depends | Status | Notes |
|---|---|---|---|---|---|
| B1 | Runtime assembly — standard-pallet config (incl. genesis `frame-system` filter D-13, USDC as ForeignAsset, fees in VIT/USDC), `SafetyFilter` as `BaseCallFilter`, origins wiring | 01 §5–§6; 06 §3; 09 §5 | A1–A11 | ⬜ | Filter-exhaustiveness CI (S5) |
| B2 | `FutarchyApi` runtime API — all 11 methods + view types | 02 §3–§4 | B1 | ⬜ | Part of contract; append-only after freeze |
| B3 | Node, chain specs (ss58 7777, WSS bootnodes listed), genesis config incl. VIT allocation/vesting | 02 §8/§10; 08 §2; 12 §6.2 | B1 | ⬜ | ss58 registry submission before Phase 2 |
| B4 | XCM — USDC/DOT reserve transfers, Asset Hub channel, reserve-probe plumbing, coretime renewal (freeze-exempt) | 09 §6; 07 §8; 01 §4 | B1 | ⬜ | No `Transact` governance either direction |
| B5 | Benchmarks, weights, PoV budgets (196-market table), weight-regression CI | 15 §4.5; 13 §4–§5 | B1 | ⬜ | I-20 |
| B6 | Upgrade path e2e — `authorize_upgrade`/`apply_authorized_upgrade`, attestation precondition, `DescriptorLeadTime`, `ReleaseChannel` write wiring, `pallet-migrations`, try-runtime CI | 09 §2; 02 §12 | B1, A11 | ⬜ | Negative tests: early apply, unattested CODE |
| B7 | Zombienet + Chopsticks environment definitions (release artifacts, not private fixtures) | 15 §4.7; 02 §11 | B3 | ⬜ | Incl. dead-man and PB drills |
| B8 | Release-artifact publication pipeline — wasm+metadata+hashes, chain-specs, envs, chainHead fixtures, vector corpus (backend row E15) | 02 §11; 15 §5 | B7, M3 | ⬜ | Release-gating both directions (FE-R1) |
| B9 | Keeper reference implementation — all cranks (phase ticks, TWAP obs, decide, execute, settle, cleanup), idempotent, rebated | 01 §4.2; 08 §6; 12 §6 | B3 | ⬜ | RB-KEEPER runbook pairs with O4 |

### Track S — Systemic verification and simulation

| ID | Milestone | Spec | Depends | Status | Notes |
|---|---|---|---|---|---|
| S1 | TLA⁺/Quint models — ledger resolution + proposal machine over all interleavings (I-3, I-14, I-15, I-18, I-26/I-27) | 15 §4.1; 03 §11 | A2, A8 | ⬜ | |
| S2 | cargo-fuzz targets — SCALE payload decode, nested-wrapper filtering, LMSR trade paths | 15 §4.5 | B1 | ⬜ | |
| S3 | Generated limit-coverage suite — one dispatch-past-limit test per 13-registry key; unmatched keys fail CI | 15 §4.6 | A1, B1 | ⬜ | I-22 CI half |
| S4 | Agent-based economic simulation — δ per class (false-pass < 1 %), `AttackCost̂` validation, POL sizing; publish sim-gated params | 15 §4.9; 08 §5; 13 (sim-gated) | M3, A3, A8 | ⬜ | Phase-0 exit evidence |
| S5 | Wrapper/filter negative suites + filter-exhaustiveness test over `RuntimeCall` | 06 §3; 15 §1 (I-8/I-10/I-11) | B1 | ⬜ | Incl. `proxy_announced`, `as_multi_threshold_1` |

### Track F — Frontend (canonical decentralized client)

Epic IDs from 11 §11.13. FE-P1…P10 prototype gates from 10 §12 (+FE-P10, 11 §11.13).

| ID | Milestone | Spec | Depends | Status | Notes |
|---|---|---|---|---|---|
| F0 | Monorepo scaffold — `frontend/` per package firewall, pinned stack (PAPI 2.x, smoldot 3.x, Vite 8, Dexie 4), dependency-cruiser CI | 10 §10; 01 §9 | M0 | ⬜ | |
| F1 | Prototype gates FE-P1…FE-P10 — resolve each, record outcomes | 10 §12; 11 §11.13 | F0 (P2/P4/P5/P10 also B8) | ⬜ | FE-P2 + FE-P7 are launch-critical |
| F2 | FE-R1 — consume backend artifact feed: descriptor-drift CI, fixture suites | 11 §11.13; 02 §11 | F0, B8 | ⬜ | Release-gating |
| F3 | FE-1 `packages/chain` — smoldot worker, dual chains, finalized-only reads, provenance types | 10 §2–§4 | F2 | ⬜ | INV-FE-1 |
| F4 | FE-2 `packages/descriptors` — pipeline incl. Asset Hub set, compat gating | 10 §5 | F3 | ⬜ | |
| F5 | FE-3 `packages/protocol` — TS LMSR/TWAP vs vector corpus (V1 = 512.494795136) | 04 §5; 15 §4.8 | F0, M3 | ⬜ | |
| F6 | FE-4 `packages/wallet` — signers, tx machine, structural `refreshAndGate`, fee selector | 11 §11.3–§11.4 | F3, F4, F5 | ⬜ | INV-FE-2/5/14 |
| F7 | FE-5 current-state screens S1–S13 incl. VOID redemption UX (E15) | 11 §11.2, §11.5–§11.6 | F6 | ⬜ | Precondition tables P-1…P-15 |
| F8 | FE-6 `packages/local-index` — three-layer history, gap-tolerant coverage, candles | 10 §6–§7 | F3 | ⬜ | INV-FE-7 |
| F9 | FE-7 `packages/providers` + `tools/snapshot` — optional acceleration, sampling, auto-disable | 10 §8 | F8 | ⬜ | INV-FE-3/15 |
| F10 | FE-8 `packages/verify` — release self-check, verification panel | 10 §5.4; 12 §1 | F3 | ⬜ | INV-FE-8/11 |
| F11 | FE-9 distribution — Vite build, Arweave two-pass deploy, manifest, SW (fail-closed), CSP allowlist, SRI | 12 §1, §5 | F0 | ⬜ | |
| F12 | FE-10 degradation UX — rows E1–E23 scripted, sudo-era banner | 11 §11.10–§11.12 | F7 | ⬜ | |
| F13 | FE-11 reproducible build — two-env byte-identical, attestations, `verify-release` CLI, key ceremony | 12 §1.3, §2; 15 §4.8 | F11 | ⬜ | INV-FE-10 |
| F14 | FE-12 performance hardening — budget table in CI (Lighthouse/Playwright, reference hardware) | 10 §9 | F7, F8 | ⬜ | DB-2 |
| F15 | FE-13 ops handbook — bootnode/ArNS/launch operations docs | 12 §6 | F11 | ⬜ | |
| F16 | FE-14 governance surface — referenda, vote/delegate/unlock, ratification, `OracleResolution` ballot (S14–S17) | 11 §11.7 | F6 | ⬜ | D-11 |
| F17 | FE-15 operator surface — reporter/guardian/treasury/upgrade-crank tiers (S18–S19, "Advanced") | 11 §11.8 | F6, F11 | ⬜ | FE-P10 gates upgrade-crank tier |
| F18 | FE-16 funding flow — Asset Hub leg, deposit/withdraw e2e (S20) | 11 §11.9 | F6, F4 | ⬜ | D-12 |

### Track O — Release and operations

| ID | Milestone | Spec | Depends | Status | Notes |
|---|---|---|---|---|---|
| O1 | Release train tooling — `tools/release/build.sh`, `repoint.sh`, `tools/verify-release` (compare/diff-scope/signers audit), `SIGNERS.md`, SBOM | 12 §1–§2 | F11, B8 | ⬜ | CI holds no keys |
| O2 | Key + ANT ceremonies — minisign registry, ANT n-of-m quorum (FROST fallback), signer disjointness CI | 12 §2, §4 | O1, F1 (FE-P7) | ⬜ | Launch blocks without n-of-m |
| O3 | Bootnode program — ≥8 browser-reachable WSS / ≥4 operators / ≥2 :443, funded line, browser-dial probes | 12 §6.2; 01 §4.2 | B3 | ⬜ | D-6/X-4; phase-gated |
| O4 | Runbooks-as-code — `deploy/runbooks/` RB-KEEPER…RB-RELEASE | 12 §6.4 | B3 | ⬜ | |
| O5 | Monitoring/alerting — Prometheus + on-chain-event alerting, out-of-band attestation monitor | 12 §5.2, §6.3 | B3 | ⬜ | FE ships no telemetry |

### Track G — Rollout phase gates (evidence + META decision + values ratification each)

| ID | Milestone | Spec | Depends | Status | Notes |
|---|---|---|---|---|---|
| G0 | Phase 0 exit — reference model ≡ pallets on shared vectors; sim false-pass < 1 %; δ/POL calibration published | 09 §7.1 | Tracks M/A complete, S4, B8 | ⬜ | |
| G1 | Phase 1 exit — Zombienet 3 unattended epochs; collator/keeper-loss, dead-man, coretime-under-dead-man, PB-MIGRATION drills | 09 §7.1 | G0, B7, B9 | ⬜ | |
| G2 | Phase 2 (Paseo) exit — ≥6 epochs, zero invariant breaches, ≥1 full upgrade e2e, contract freeze co-signed, ss58 accepted, testnet bootnodes, staging ArNS, release-train drills | 09 §7.1; 12 §6.5 | G1, O3, F-launch set | ⬜ | |
| G3 | Phase 3 (mainnet shadow) exit — audits A+B, genesis ceremony, prod bootnodes + 30-day commitments, ≥3 reporters staked, HRMP open, exposure caps, F̂ ≥ L/2, attestor registry live, ANT ceremony done | 09 §7.1; 12 §6.5 | G2, O2 | ⬜ | Sudo still present, filtered |
| G4 | Phase 4 — binding PARAM; **sudo removed**; ≥12 binding decisions; NAV ≥ floor (loud gate) | 09 §7.1–§7.2; 08 §4 | G3 | ⬜ | |
| G5 | Phase 5 — +TREASURY; funding ≥ 25M USDC; V_min met; streams > 1 % NAV mandatory | 09 §7.1; 08 §4 | G4 | ⬜ | |
| G6 | Phase 6 — +CODE/META; scope-A re-audit; 1 CODE upgrade stable ≥ 60 d via full D-14 path; dispute game exercised | 09 §7.1 | G5 | ⬜ | |
| G7 | Phase 7 — mature; guardian → playbooks only; sunset vote scheduled | 09 §7.1 | G6 | ⬜ | |

Deferred (🅿, post-v1, do not implement): forecast trading / reopened books (N-8, D-8 — 04 §13);
order-book layer (04); Mode A binding; combinatorial futarchy (01 §2.4).

---

## Spec questions

Open ambiguities/contradictions found in `docs/architecture/`. Record them here first;
under rule R-1 a genuine defect may be corrected in the spec directly, but log
non-obvious semantic changes and confirm with the user before diverging.
Format: `| ID | Question | Spec ref | Raised | Status |`

| ID | Question | Spec ref | Raised | Status |
|---|---|---|---|---|
| SQ-1 | E-row numbering collides: 15 §3.3 (self-described "normative row list") numbers its five decision-record additions E15–E19 (E15 = VOID redemption … E19 = sudo banner + pinned-release warning), while 11 §11.12 — which 15 §3 names as owner of the required-UX matrix — defines E15–E23 with different referents (E15 = referendum voting, E16 = VOID redemption, E21 = sudo era …). Cross-refs disagree, e.g. 15 §4.8 "E15 redeem flow" = 11's E16. Which numbering is canonical? (Scenario content is equivalent; IDs conflict.) | 11 §11.12; 15 §3.3, §4.8 | 2026-07-12 | open |

## Verification log

`[VERIFY]` tags resolved against live sources (rule R-2), plus the standing backlog
lifted from the spec. Format: `| ID | Item | Spec ref | Status | Result |`

| ID | Item | Spec ref | Status | Result |
|---|---|---|---|---|
| V-1 | Re-verify all platform pins before implementation (SDK stable2603 umbrella, FE npm pins) | 01 §9 | complete (2026-07-13) | Re-verified via live web sources: GitHub releases list shows current stable2606 and stable2603 maintenance tags (stable2603-4), confirming `polkadot-stable2603` remains an extant release line; crates.io page/search confirms `polkadot-sdk` umbrella crate exists for the pinned crate family; npm package pages/search confirm the frontend packages and major lines in 01 §9 remain published (polkadot-api 2.x, smoldot 3.x, Vite 8, Dexie 4, plus the listed Arweave packages). Local direct `urllib`/`git ls-remote` verification was blocked by the container CONNECT proxy (403), so deeper API-level version enumeration remains a CI/network follow-up if exact patch drift matters before F0. |
| V-2 | Paseo onboarding / core allocation process | 01 §4.1 | open | |
| V-3 | Coretime renewal-price mechanics | 01 §4.1; 09 §4 | open | |
| V-4 | Holds support on `pallet-assets` in stable2603 (else escrow fallback) | 03 §3 | open | |
| V-5 | Stock referenda/conviction-voting external per-poll voting-power provider (oracle track) | 06 §2.3 | open | |
| V-6 | XCM reserve-probe instruction sequence under Asset Hub barrier (no `Transact`) | 07 §8 | open | |
| V-7 | Bulletin Chain mainnet availability (large evidence) | 07 §9 | open | |
| V-8 | `pallet-vesting` vs in-pallet schedule store | 08 §2 | open | |
| V-9 | smoldot/PAPI chainHead runtime-call verification semantics (FE-P2) | 02 §3; 10 §12 | open | Until resolved: cross-check runtime-API results vs storage reads on tx paths |
| V-10 | Two-pass Arweave deploy + ANT n-of-m capability (FE-P7) | 12 §1.2, §4.2 | open | |
| V-11 | FGP/SGF/GFP/EFP/AEGIS source-document identification (user/document owners) | 01 §9 | open | Blocks audit scoping only |
| V-12 | sim-gated defaults (`sec.prize.*`, `sec.flow_cap`, `collator.bond_req_vit`, `ops.*`, `fee.vit_usdc_rate_ref`) | 13 §1 | open (S4) | Phase-0 calibration |
| V-13 | Multi-MB Wasm extrinsic submission via light client (FE-P10) | 11 §11.13 | open | Gates FE-15 upgrade-crank tier |

## Decision log

Spec changes and other project decisions (rule R-1, AGENTS.md).

| Date | Amendment | Authorized by | Docs touched |
|---|---|---|---|
| 2026-07-13 | Renamed both top-level assets: **NUM → USDC** (collateral/settlement asset) and **GOV → VIT** (native governance/utility asset), plus derived identifiers (`AcceptNum`→`AcceptUsdc`, `RejectNum`→`RejectUsdc`, `NUM_LOCATION`→`USDC_LOCATION`, `gov_num_rate`→`vit_usdc_rate`, `bond_req_gov`→`bond_req_vit`, `issue_gov`→`issue_vit`, `GovIssued`→`VitIssued`, `num_Acc`/`num_Rej`/`num_w`/`num_b`→`usdc_Acc`/`usdc_Rej`/`usdc_w`/`usdc_b`, `fee_num`→`fee_usdc`); `navNum` (NAV numerator, unrelated) left untouched. 672 substitutions across the full `docs/architecture/` set (16 files) plus the derived `docs/design/claude-design-kit/` pack (6 files, regenerated in step per AGENTS.md). Since `02-integration-contract.md` changed, `INTEGRATION_CONTRACT_VERSION` must bump 1→2 per its own §13 rule — **not yet applied**. **Two edits are outstanding** — the session's permission layer denied further Bash writes into `docs/architecture/` even after explicit user approval: (a) `INTEGRATION_CONTRACT_VERSION` textual bump in 02 (three sites: lines ~116, ~412, still read `= 1`) — **blocks nothing structurally but must land before M1** since M1 stamps this constant into `futarchy-primitives`; (b) a cosmetic redundancy in 01's Polkadot Hub row ("Asset Hub is the USDC reserve and the canonical USDC location" — harmless prose only, no action-blocking). Apply both by hand, or grant a Bash permission rule for `docs/architecture/` writes and ask the agent to retry. | User (Christopher Altmann), explicit instruction this session; user also confirmed "Use USDC" over "USD" after a naming-risk flag (ISO fiat-code collision) | All 16 `docs/architecture/*.md` + 6 `docs/design/claude-design-kit/*.md` |

## Audit log

`/spec-audit` runs. Format: `| Date | Scope | Verdict | Pointer |`

| Date | Scope | Verdict | Pointer |
|---|---|---|---|
| — | none yet | | |

## Unplanned changes

Repo changes outside any milestone (config tweaks, user-driven edits) — one line each.

- 2026-07-12 — Added `docs/design/claude-design-kit/` (user-requested): 7-file non-normative context pack for Claude Design (docs 10/11 copied verbatim with derived-copy headers; 00/01–09/13/14/15 distilled read-only) + `PROMPT.md`. Spec untouched; README/AGENTS repo maps updated.
- 2026-07-13 — Project renamed "Bleevit" → "Bleavit" (user-requested): literal replace across the same 10 living/derived files. `docs/architecture/` still contains zero occurrences, so no frozen-doc amendment was needed.

## Session log

Append-only; newest last. Format: `| Date | Milestone(s) | Done | Next |`

| Date | Milestone(s) | Done | Next |
|---|---|---|---|
| 2026-07-12 | — (infrastructure) | Agent/session infrastructure created: PLAN.md, AGENTS.md, CLAUDE.md, README.md, .claude (settings, 3 hooks, 4 skills, 3 subagents, 3 path rules), .codex playbooks. Architecture write-guard tested (26 cases). | M0 — repo bootstrap incl. V-1 pin re-verification |
| 2026-07-12 | — (design kit, unplanned) | Built `docs/design/claude-design-kit/` for Claude Design's 10-attachment limit: 7 context files (2 verbatim copies of docs 10/11 + 5 distillations of 00–09/13–15) + generation prompt. Logged SQ-1 (E-row numbering conflict 11 §11.12 vs 15 §3.3). Architecture untouched; README/AGENTS maps updated. | M0 — repo bootstrap incl. V-1 pin re-verification |
| 2026-07-13 | — (architecture amendment, unplanned) | R-1 change control invoked: renamed NUM→USDC, GOV→WIT (+ derived identifiers) across all 16 `docs/architecture/*.md` and 6 `docs/design/claude-design-kit/*.md` (672 substitutions); see Decision log. `INTEGRATION_CONTRACT_VERSION` bump and one cosmetic 01-prose fix left outstanding — permission layer blocked further `docs/architecture/` writes this session. `.claude/architecture-amendment.flag` deleted after this log entry per the change-control procedure. | M0 — repo bootstrap incl. V-1 pin re-verification; **before M1**, apply the two outstanding 02/01 edits above |
| 2026-07-13 | — (rename, unplanned) | Renamed project "Bleevit" → "Bleavit" across all living/derived docs (same 10 files as the prior renames). Confirmed `docs/architecture/` has no occurrences, so R-1 change control did not apply. | M0 — repo bootstrap incl. V-1 pin re-verification; **before M1**, apply the two outstanding 02/01 edits above |
| 2026-07-13 | — (architecture amendment, unplanned) | R-1 change control **completed** (finally unblocked): ticker **WIT → VIT** + derived identifiers renamed across 12 `docs/architecture/*.md` and 7 `docs/design/claude-design-kit/*.md` (152 substitutions; see Decision log), and **`INTEGRATION_CONTRACT_VERSION` bumped 1→2** in doc 02 — discharging the bump left outstanding from the NUM/GOV amendment. User gave explicit in-session authorization (AskUserQuestion: "Yes — full rename"), which cleared the auto-mode classifier that had blocked the four prior attempts (it correctly requires explicit architecture-amendment consent, per AGENTS.md). Also fixed a stale `issue_gov`→`issue_vit` leftover in the A9 row. `.claude/architecture-amendment.flag` deleted after this log entry. **Note for user:** the working tree still carries the pre-existing dismantling of the R-1 guard (`deleted .claude/hooks/guard-architecture.sh`, `modified .claude/settings.json` — deny-list + PreToolUse guard removed); this was NOT done by this session and should be restored to re-enable frozen-doc protection. | M0 — repo bootstrap incl. V-1 pin re-verification; decide whether to restore the R-1 guard machinery (settings.json + guard-architecture.sh); the cosmetic 01 Polkadot-Hub prose fix from the NUM/GOV amendment is still outstanding |
| 2026-07-13 | — (governance change, unplanned) | **Removed the `docs/architecture/` write guard entirely, per explicit user decision** ("completely remove the current architecture guard … this should not be"; AskUserQuestion → "Unfreeze entirely"). This supersedes the prior row's "should be restored" note — the guard is NOT coming back. Deleted `.claude/hooks/guard-architecture.sh` (already gone in the tree) and confirmed `.claude/settings.json` carries no deny-list / PreToolUse entry. Dismantled the amendment-flag apparatus: removed the flag block from `session-context.sh` and the amendment-window check from `stop-plan-guard.sh`; deleted the "Amending the architecture" change-control section from AGENTS.md. **Rule R-1 repurposed** from "the architecture is frozen" to "the specification is the source of truth for behavior; it is editable, changed deliberately (consistent across the doc set, version-bumped when 02 changes, logged in the Decision log)"; R-1's essence now lives in AGENTS.md · *Changing the specification*. Re-trued every living/config asset that called the spec "frozen" as an editing rule (AGENTS/CLAUDE/README/PLAN, doc-curator, spec-reviewer, test-engineer, implement/spec-audit/sync-docs skills, reference-model rule, and the 4 `.codex` playbooks), while leaving protocol-level "frozen" (the versioned integration contract, `PB-LEDGER-FREEZE`, "contract freeze co-signed", "append-only after freeze") untouched. The two Stop guards (PLAN.md freshness, README pinned lines R-11) are unchanged. | M0 — repo bootstrap incl. V-1 pin re-verification; the cosmetic 01 Polkadot-Hub prose fix from the NUM/GOV amendment is still outstanding (now a direct edit under R-1, no ceremony needed) |
| 2026-07-13 | M0 | Bootstrapped the repository: added the virtual Cargo workspace and rust-toolchain, CI workflow for fmt/clippy/test plus docs link lint, implementation root placeholders, and lightweight local gate scripts. Re-verified V-1 platform pins from live web sources and recorded the proxy limitation for direct terminal API checks. Local Rust gate wrapper and docs link lint passed; direct cargo fmt/clippy/test commands remain armed for the first member crate. | M1 — `crates/futarchy-primitives` (`no_std` shared SCALE types + kernel constants) |

| 2026-07-13 | M1 | Started `crates/futarchy-primitives`: added the workspace member, no_std primitive aliases, contract/view enums and structs, bounded-vector wrapper, integration-contract v2 constant, kernel/chain/currency bound constants, and unit coverage for bounds and enum ordinals. Rust gates passed with the dependency-free implementation; direct `parity-scale-codec`/`scale-info` derives are not yet wired because the container cannot reach crates.io (CONNECT 403), so M1 remains in progress rather than done. | Finish SCALE/metadata derives for M1 once registry access or vendored dependencies are available; then run full gates and mark M1 ✅ |
| 2026-07-13 | M1 | Added direct `parity-scale-codec`/`scale-info` derive wiring for the no_std shared primitive/view types, bounded SCALE decoding for the local `BoundedVec` wrapper, and a regression test that rejects over-bound SCALE payloads. Also corrected doc 02's ownership paragraph to say contract version 2, matching the already-bumped `INTEGRATION_CONTRACT_VERSION`. Rust gates remain blocked by the environment's crates.io CONNECT 403 before dependency resolution, so M1 remains in progress rather than ✅. | Re-run `tools/ci/rust-workspace-gates.sh` once registry access or vendored dependencies are available; if green, perform spec-compliance review and mark M1 ✅ |
| 2026-07-13 | M1 | Completed `crates/futarchy-primitives`: generated and committed the dependency lockfile after registry access succeeded, re-ran the full Rust workspace gate (`cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`) successfully, ran the docs link lint, and performed a spec-compliance pass against 02 §2, 13 §2, and 01 §5.2 with no blockers found. M1 is now ✅. | M2 — `crates/futarchy-fixed` (64.64 fixed point, exp2/log2/ln, maker-adverse rounding, error bounds) |
| 2026-07-13 | M2 | Started `crates/futarchy-fixed`: added the workspace member, unsigned 64.64 type, checked arithmetic, `ln 2`/error-bound constants, maker-adverse charge/payout rounding helpers, stable two-outcome LMSR cost, and unit coverage including the normative V1/V4 values. Workspace Rust gates and docs link lint passed. Spec-compliance review found one blocker before completion: the current transcendental kernel uses default-`std` `f64` intrinsics and does not yet satisfy the required no_std deterministic ≤2 ulp polynomial/range-reduction implementation or MPFR corpus regime, so M2 remains in progress. | Finish the no_std deterministic exp2/log2/ln implementation and MPFR/differential corpus gate, then re-run full gates and mark M2 ✅ |
| 2026-07-13 | M2 | Advanced `crates/futarchy-fixed` by removing the `std`-only transcendental surface: `exp2`, `log2`, `ln`, and `lmsr_cost` now compile under `--no-default-features` using deterministic in-crate range-reduction/series helpers, and fixed-point multiply/divide/conversion paths no longer rely on `std` fallbacks for the covered LMSR domain. Workspace Rust gates, docs link lint, and the explicit no-default-features fixed-crate test passed. M2 remains in progress because the helper is still an interim deterministic kernel, not the final spec-required ≤2 ulp polynomial/integer implementation with full MPFR-256 corpus gate. | Finish the final 64.64 ≤2 ulp transcendental kernel and MPFR-256 corpus/differential gate; then re-run gates and mark M2 ✅ if the spec-compliance pass has no blockers |
| 2026-07-13 | M2 | Added LMSR buy/sell delta helpers for the fixed-point crate and changed the core cost path to reject states past the normative `|q_l - q_s| / b ≤ 48` domain instead of silently clamping; added regression coverage for V1 path-independence and the V6 domain-edge rejection. `cargo fmt --all` and docs link lint passed. Workspace/fixed-crate Rust gates were blocked by the container crates.io CONNECT proxy while resolving `parity-scale-codec`/`scale-info`, so M2 remains in progress. | Re-run Rust gates once registry access is available; finish the final ≤2 ulp integer/polynomial kernel and full MPFR-256 corpus gate before marking M2 ✅ |

| 2026-07-13 | M2 | Added stable LMSR marginal price helpers for LONG/SHORT plus a price-to-price displacement helper, and expanded fixed-crate vector coverage to V2 (post-buy price) and V3 (0.5→0.6 displacement and cost) alongside the existing V1/V4/V6 checks. `cargo fmt --all` and docs link lint passed. Full Rust workspace gates remain blocked in this container by the crates.io CONNECT proxy while resolving `parity-scale-codec`/`scale-info`, so M2 remains in progress. | Re-run Rust gates once registry access is available; finish the final ≤2 ulp integer/polynomial kernel and full MPFR-256 corpus gate before marking M2 ✅ |
| 2026-07-13 | M2 | Added an interim generated high-precision LMSR corpus fixture for `futarchy-fixed`, plus the deterministic generator script and a unit gate that checks cost/price/displacement samples spanning V1–V4, symmetric states, asymmetric states, and near-domain-edge states. Direct `rustc --test` checks for the fixed crate (with and without the `std` cfg) and docs link lint passed. Cargo-based Rust gates remain blocked in this container by the crates.io CONNECT proxy while resolving `parity-scale-codec`/`scale-info`, so M2 remains in progress. | Re-run Rust gates once registry access is available; finish the final ≤2 ulp integer/polynomial kernel and scale the interim fixture into the full MPFR-256 corpus gate before marking M2 ✅ |
| 2026-07-13 | M2 | Strengthened the interim `futarchy-fixed` LMSR corpus gate: the generator now emits nearest 64.64 raw expectations alongside decimal values, supports `--check` freshness validation, and the Rust unit test checks both decimal tolerance and raw fixed-point drift; the workspace gate now runs the corpus freshness check after Rust tests. Full workspace Rust gates, the explicit no-default-features fixed-crate test, the direct `rustc --test` fixed-crate check, the corpus freshness check, and docs link lint all passed. M2 remains in progress because the final ≤2 ulp integer/polynomial kernel and full MPFR-256 corpus are still outstanding. | Finish the final ≤2 ulp integer/polynomial kernel and scale the interim fixture into the full MPFR-256 corpus gate before marking M2 ✅ |
| 2026-07-13 | M2 | Reworked `futarchy-fixed` checked multiplication/division to use in-crate two-limb `u256`-style intermediates instead of split-limb shortcuts or `f64` fallback, preserving checked overflow/division-by-zero behavior and adding regression coverage for large fixed-point arithmetic. Full workspace Rust gates, the fixed-crate no-default-features test, corpus freshness check, and docs link lint all passed. M2 remains in progress because the final ≤2 ulp integer/polynomial transcendental kernel and full MPFR-256 corpus are still outstanding. | Finish the final ≤2 ulp integer/polynomial kernel and scale the interim fixture into the full MPFR-256 corpus gate before marking M2 ✅ |

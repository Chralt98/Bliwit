# PLAN.md — Implementation Roadmap and Status

**This file is the single source of implementation status.** It tells every session
what is done, in progress, blocked, and next. It **references** the frozen
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

**Next: M0 — repository bootstrap** (workspace, toolchain, CI skeleton, platform-pin
re-verification per 01 §9). No implementation code exists yet; the spec is complete
and frozen; the agent/session infrastructure (this file, AGENTS.md, hooks, skills)
is in place.

---

## Milestones

### Track M — Foundations

| ID | Milestone | Spec | Depends | Status | Notes |
|---|---|---|---|---|---|
| M0 | Repo bootstrap: Cargo workspace, `rust-toolchain.toml`, CI skeleton (fmt/clippy/test + docs link lint), **re-verify platform pins** (SDK `polkadot-stable2603`, FE package pins) | 01 §9 | — | ⬜ | Pin re-verification results → Verification log |
| M1 | `crates/futarchy-primitives` — `no_std` shared SCALE types + kernel `K` constants, `INTEGRATION_CONTRACT_VERSION = 1` | 02 §2; 13 §2; 01 §5.2 | M0 | ⬜ | Frozen surface; no `frame` deps |
| M2 | `crates/futarchy-fixed` — 64.64 fixed point, exp2/log2/ln, maker-adverse rounding, error bounds | 04 §4 | M0 | ⬜ | ≤2 ulp per op, composed ≤8 ulp |
| M3 | `reference-model/` — independent Python executable spec (LMSR, TWAP, ledger ops incl. gate/Baseline/VOID, welfare pipeline, decision rule, treasury arithmetic) + MPFR-256 corpus + V1–V6 regeneration in CI | 04 §5; 15 §4.4; 08 (worked arithmetic); 05 §4.4 | M0 | ⬜ | Never ports Rust code (`.claude/rules/reference-model.md`) |

### Track A — Protocol pallets

Every pallet milestone includes: mock runtime, per-extrinsic × error-path × origin-misuse
tests (15 §4.1), `try-state` per 15 §1, benchmark stubs (15 §4.5). Scaffold via `/new-pallet`.

| ID | Milestone | Spec | Depends | Status | Notes |
|---|---|---|---|---|---|
| A1 | `pallet-constitution` — typed/bounded/rate-limited params, meters, capability tables, `PhaseFlags`, `ReleaseChannel` fixed-layout key, kernel re-export | 06; 13 §1/§4; 02 §7.3/§12 | M1 | ⬜ | I-6, I-7, I-17 |
| A2 | `pallet-conditional-ledger` — vaults, per-branch supplies, split/merge/scalar/gate/Baseline families, `Voided` + `redeem_void`, internal wrapper API | 03 (all); 02 §6 | M1 | ⬜ | Audit scope A; L-1…L-6; PT-1…PT-8 (15 §4.2–4.3); frozen early |
| A3 | `pallet-market` — LMSR books (branch-NUM, D-3), trade wrapper, fees, TWAP, POL seeding, `BaselineMarketOf` | 04; 02 §5–§7.4 | A2, M2, M3 | ⬜ | I-12, I-13; differential vs V1–V6 + corpus |
| A4 | `pallet-origins` + `SafetyFilter` — 8 custom origins, closed wrapper set, call-domain derivation | 06 §3; 01 §6 | M1 | ⬜ | I-8, I-10, I-11 |
| A5 | `pallet-oracle` — reporter/watchtower registries, bonded reporting game, challenge rounds (72 h + quorum), reserve probe `R`, neutral settlement | 07 §1–§6, §8–§13 | A1 | ⬜ | I-18, I-24 |
| A6 | `pallet-registry` — Incident/Milestone instances, bonded filings, challenge windows | 07 §7 | A5 | ⬜ | Feeds `C_attested` |
| A7 | `pallet-welfare` — bounded counters, snapshots, `MetricSpec` registry, pillar pipeline (`C_onchain`/`C_attested`), gate-breach flags, settlement score | 05 §4, §6–§7 | A1, A5 | ⬜ | I-16; bit-identical conformance vectors (M3) |
| A8 | `pallet-epoch` — epoch/phase clock, proposal registry + T1–T24 machine, cohorts, `decide()` engine (11 steps incl. `SecuritySizing` D-4, ratification D-5), `RecentCohortSummaries` ring | 05 §1–§3, §5; 02 §7.1 | A2, A3, A7 | ⬜ | I-14, I-15, I-21; model check (S1) |
| A9 | `pallet-futarchy-treasury` — sub-accounts, NAV + haircuts, outflow meters/streams, budget lines, `issue_gov`, coretime renewal call | 08 §1, §6–§9 | A1 | ⬜ | I-7 meters; `nav()` view data |
| A10 | `pallet-guardian` + `pallet-attestor` — 7-seat council, powers, playbooks incl. PB-LEDGER-FREEZE, retro ratification; bonded 2-of-N attestor registry | 06 §5–§7 | A1, A4 | ⬜ | I-19, I-23 |
| A11 | `pallet-execution-guard` — queue, permissionless `execute()` (13-item dispatch list), class origins, two-phase upgrade flow, `DescriptorLeadTime`, `ExecutionRecords` ring | 09 §1–§3; 02 §7 | A8, A10 | ⬜ | I-9, I-10, I-11, I-19; FE `execute` row lockstep (11 §11.5) |

### Track B — Runtime, node and chain

Doc-09 WBS delta rows map here and to Track A: E15→B2/B8 · E16→A2 · E17→A3 · E18→A6 ·
E19→A5 · E20→A10/B6 · E21→B4 · E22→A11/B1. (E1–E14 definitions live in git history —
superseded `BACKEND_PLAN.md` §26; their scope is covered by Tracks M/A/B.)

| ID | Milestone | Spec | Depends | Status | Notes |
|---|---|---|---|---|---|
| B1 | Runtime assembly — standard-pallet config (incl. genesis `frame-system` filter D-13, NUM as ForeignAsset, fees in GOV/NUM), `SafetyFilter` as `BaseCallFilter`, origins wiring | 01 §5–§6; 06 §3; 09 §5 | A1–A11 | ⬜ | Filter-exhaustiveness CI (S5) |
| B2 | `FutarchyApi` runtime API — all 11 methods + view types | 02 §3–§4 | B1 | ⬜ | Part of contract; append-only after freeze |
| B3 | Node, chain specs (ss58 7777, WSS bootnodes listed), genesis config incl. GOV allocation/vesting | 02 §8/§10; 08 §2; 12 §6.2 | B1 | ⬜ | ss58 registry submission before Phase 2 |
| B4 | XCM — NUM/DOT reserve transfers, Asset Hub channel, reserve-probe plumbing, coretime renewal (freeze-exempt) | 09 §6; 07 §8; 01 §4 | B1 | ⬜ | No `Transact` governance either direction |
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
| G5 | Phase 5 — +TREASURY; funding ≥ 25M NUM; V_min met; streams > 1 % NAV mandatory | 09 §7.1; 08 §4 | G4 | ⬜ | |
| G6 | Phase 6 — +CODE/META; scope-A re-audit; 1 CODE upgrade stable ≥ 60 d via full D-14 path; dispute game exercised | 09 §7.1 | G5 | ⬜ | |
| G7 | Phase 7 — mature; guardian → playbooks only; sunset vote scheduled | 09 §7.1 | G6 | ⬜ | |

Deferred (🅿, post-v1, do not implement): forecast trading / reopened books (N-8, D-8 — 04 §13);
order-book layer (04); Mode A binding; combinatorial futarchy (01 §2.4).

---

## Spec questions

Open ambiguities/contradictions found in `docs/architecture/` (rule R-1: record here,
never edit the spec). Format: `| ID | Question | Spec ref | Raised | Status |`

| ID | Question | Spec ref | Raised | Status |
|---|---|---|---|---|
| SQ-1 | E-row numbering collides: 15 §3.3 (self-described "normative row list") numbers its five decision-record additions E15–E19 (E15 = VOID redemption … E19 = sudo banner + pinned-release warning), while 11 §11.12 — which 15 §3 names as owner of the required-UX matrix — defines E15–E23 with different referents (E15 = referendum voting, E16 = VOID redemption, E21 = sudo era …). Cross-refs disagree, e.g. 15 §4.8 "E15 redeem flow" = 11's E16. Which numbering is canonical? (Scenario content is equivalent; IDs conflict.) | 11 §11.12; 15 §3.3, §4.8 | 2026-07-12 | open |

## Verification log

`[VERIFY]` tags resolved against live sources (rule R-2), plus the standing backlog
lifted from the spec. Format: `| ID | Item | Spec ref | Status | Result |`

| ID | Item | Spec ref | Status | Result |
|---|---|---|---|---|
| V-1 | Re-verify all platform pins before implementation (SDK stable2603 umbrella, FE npm pins) | 01 §9 | open (M0) | |
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
| V-12 | sim-gated defaults (`sec.prize.*`, `sec.flow_cap`, `collator.bond_req_gov`, `ops.*`, `fee.gov_num_rate_ref`) | 13 §1 | open (S4) | Phase-0 calibration |
| V-13 | Multi-MB Wasm extrinsic submission via light client (FE-P10) | 11 §11.13 | open | Gates FE-15 upgrade-crank tier |

## Decision log

Architecture amendments (change-control procedure, AGENTS.md). None expected; none yet.

| Date | Amendment | Authorized by | Docs touched |
|---|---|---|---|
| — | none | | |

## Audit log

`/spec-audit` runs. Format: `| Date | Scope | Verdict | Pointer |`

| Date | Scope | Verdict | Pointer |
|---|---|---|---|
| — | none yet | | |

## Unplanned changes

Repo changes outside any milestone (config tweaks, user-driven edits) — one line each.

- 2026-07-12 — Added `docs/design/claude-design-kit/` (user-requested): 7-file non-normative context pack for Claude Design (docs 10/11 copied verbatim with derived-copy headers; 00/01–09/13/14/15 distilled read-only) + `PROMPT.md`. Spec untouched; README/AGENTS repo maps updated.

## Session log

Append-only; newest last. Format: `| Date | Milestone(s) | Done | Next |`

| Date | Milestone(s) | Done | Next |
|---|---|---|---|
| 2026-07-12 | — (infrastructure) | Agent/session infrastructure created: PLAN.md, AGENTS.md, CLAUDE.md, README.md, .claude (settings, 3 hooks, 4 skills, 3 subagents, 3 path rules), .codex playbooks. Architecture write-guard tested (26 cases). | M0 — repo bootstrap incl. V-1 pin re-verification |
| 2026-07-12 | — (design kit, unplanned) | Built `docs/design/claude-design-kit/` for Claude Design's 10-attachment limit: 7 context files (2 verbatim copies of docs 10/11 + 5 distillations of 00–09/13–15) + generation prompt. Logged SQ-1 (E-row numbering conflict 11 §11.12 vs 15 §3.3). Architecture untouched; README/AGENTS maps updated. | M0 — repo bootstrap incl. V-1 pin re-verification |

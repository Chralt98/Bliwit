# AGENTS.md — Operating Manual for Coding Agents

This repository implements **Bleavit**: a futarchy-governed Polkadot parachain
(native Rust FRAME pallets, LMSR conditional markets) with a canonical decentralized
frontend (Arweave-distributed static app, in-browser light client). The complete,
authoritative specification already exists; the job of every session is to turn it
into code, one milestone at a time, without ever degrading the specification or the
project's living documents.

Read this file first. Then read `PLAN.md`. Then work.

## Ground truth

- **`docs/architecture/` (00–15) is the single source of truth** for what to build
  (see rule R-1). Doc 00 is the decision record (D-1…D-19); 01 the system overview;
  02 the chain↔frontend integration contract (a versioned surface — see 02 §13);
  03–09 the protocol components;
  10–12 the frontend and operations; 13 the only home of parameter values;
  14 the threat model; 15 the invariants and the normative testing regime.
  Reading order for newcomers: 01 → 02 → 03 → 04 → 05, then as needed
  (`docs/architecture/README.md`).
- Constants and parameters have exactly two homes: `02` (chain identity, the
  contract surface) and `13` (everything else). Any other file that needs a value
  references them — including code (kernel constants from `futarchy-primitives`,
  tunables from `pallet-constitution::Params`; the frontend reads chain
  metadata/storage, never hardcodes).
- **`PLAN.md` is the single source of implementation status** — what is done, in
  progress, blocked, and next. It references architecture sections and never restates
  their content.

## Rules

- **R-1 — The specification is the source of truth for behavior.** Every observable
  behavior traces to `docs/architecture/` (00–15); implementation follows the spec.
  The spec is editable — when it is genuinely wrong, ambiguous, or contradictory you
  may correct it directly rather than coding around it. Do it deliberately: keep the
  change internally consistent across the doc set (owning doc + every referencing doc
  + 00's decision record if a D-n is affected; bump `INTEGRATION_CONTRACT_VERSION` per
  02 §13 when 02 changes; changes to `02` or the INV-FE texts need the joint
  backend+frontend sign-off those docs mandate — the user speaks for both sides or
  names who does), and record substantive changes in PLAN.md · *Decision log*. When a
  semantic change is non-obvious or you are unsure, log it in PLAN.md · *Spec
  questions* and ask the user before diverging.
- **R-2 — Spec-first implementation.** Before writing code, read the owning
  architecture sections for the milestone (its *Spec* column), plus the relevant
  slices of 02, 13, and 15. Every observable behavior must be traceable to spec text.
  Never invent parameter values, names, or semantics; never resolve a `[VERIFY]` tag
  by assumption — verify against live sources and log the result in PLAN.md ·
  *Verification log*.
- **R-3 — The living documents stay true.** After every change to the repository,
  `PLAN.md` is updated in the same session (status, session log); `README.md`,
  `AGENTS.md`, and `CLAUDE.md` are refreshed whenever the repo shape, commands, or
  workflow they describe changed. A session that leaves the living documents stale is
  an unfinished session (a Stop hook will remind you).
- **R-4 — PLAN.md is status, not spec.** Milestone rows cite `docs/architecture/`
  sections; PLAN.md never duplicates normative content. If you feel the need to
  explain protocol design in PLAN.md, you are writing in the wrong file.
- **R-5 — One milestone per session.** Pick the next milestone (or the one in
  progress, or the one the user names), finish it or park it cleanly with exact
  resume notes. Never start a second milestone in the same session; never leave the
  repo red without saying so in PLAN.md.
- **R-6 — Quality gates before "done".** A milestone is ✅ only when the gates below
  pass and a spec-compliance review found no blockers. Never mark done with failing
  tests; report failures verbatim instead.
- **R-7 — This is financial infrastructure.** Solvency-critical code (ledger,
  constitution, execution guard — audit scope A) gets the strictest treatment:
  adversarial tests, rounding always against the claimant, status-quo default on
  every failure path (G-1), no panics, no unbounded state. When in doubt, choose the
  reading that cannot create an unbacked claim or execute a payload.
- **R-8 — Verification is spec-mandated, not optional.** Doc 15 defines the test
  regime (mock-runtime × error paths × origin misuse, PT-1…PT-8 property suites,
  differential vectors vs the reference model, generated limit-coverage suite,
  try-state everywhere, fuzz/bench/weights). Milestones carry their verification
  obligations in PLAN.md's *Verify* column — they are part of the milestone, not
  follow-up work.
- **R-9 — Commit discipline.** Conventional commits with the milestone ID, e.g.
  `feat(ledger): split/merge families with per-branch supplies (A2)`. Commit only
  when the user asks (or has standing instructions); never push, publish, or tag
  without an explicit ask. Never commit with red gates. Enable
  `git config rerere.enabled true` locally — `PLAN.md`'s `Current focus`,
  `Milestones`, and `Session log` sections are touched by nearly every PR, so the
  same conflict shapes recur across branches; rerere replays your own past
  resolutions automatically instead of re-solving them by hand each time.
- **R-10 — Honest reporting.** Report what happened: gates that failed, spec
  questions found, work left open. The next session inherits your PLAN.md state —
  optimistic status lines are technical debt with interest.
- **R-11 — README's pinned lines are fixed.** `README.md` always opens, as the first
  paragraph right after the `# Bleavit` heading, with:
  > Futarchy was invented by Prof. Robin Hanson — thank you for your work; this
  > project exists to build one.

  and always ends, as the last line of the file, with:
  > You theorized it, we are cooking it. Bon appétit, Prof. Hanson.

  Both are verbatim and permanent — no rewording, trimming, or removal by any
  doc-sync pass, refactor, or rewrite. Set by explicit user instruction
  (2026-07-13). Enforced in Claude Code by a Stop hook (`guard-readme.sh`); Codex
  has no hook equivalent, so its playbooks restate this rule explicitly
  (`.codex/README.md`).

## Session protocol

1. **Orient** — read the injected session context, then `PLAN.md` (Current focus,
   next milestones, last session log rows, open Spec questions).
2. **Select** — the in-progress milestone, else the first pending one whose
   dependencies are ✅, else what the user names. Confirm scope in one sentence.
3. **Read the spec** — the milestone's cited sections, before any code (R-2).
4. **Implement** — following `.claude/rules/` path rules and the conventions of the
   surrounding code; delegate bulk test authoring to the `test-engineer` agent and
   compliance review to the `spec-reviewer` agent.
5. **Verify** — run the quality gates; fix or honestly report.
6. **Close** — update the living documents (R-3), report results, suggest the commit.

The `/implement` skill (Claude Code) and `.codex/prompts/implement-next.md` (Codex)
encode this loop verbatim.

## Quality gates

Run what exists; gates grow with the repo (PLAN.md's *Verify* column is authoritative
per milestone):

| Area | Gate (current) |
|---|---|
| Rust | `tools/ci/rust-workspace-gates.sh` (runs `cargo fmt --all -- --check` · `cargo clippy --workspace --all-targets -- -D warnings` · `cargo test --workspace` · runtime release/`runtime-benchmarks`/`try-runtime` builds + the try-runtime-enabled runtime suite (B6; the 15 §4.7 snapshot `try-runtime-cli` leg lands with B7/B8) · `no_std` build · the S3 limit-coverage leg: `python3 -m unittest discover -s tools/limit-coverage/tests` + `python3 tools/limit-coverage/check-limit-coverage.py` — the 15 §4.6 / I-22 gate: every 13-registry key must be classified in `tools/limit-coverage/registry.toml` and every dispatch-limit key bound to a `// limit-coverage:` marked test) |
| Runtime crates | `try-state` green in test envs; benchmarks compile; no new `unwrap`/`expect`/`panic!`/`unsafe` in runtime code |
| Fuzzing (15 §4.5) | `tools/ci/fuzz-gates.sh` (CI job `fuzz`, nightly-pinned separate `fuzz/` workspace): fmt/clippy/oracle-unit-tests · `cargo fuzz build` each target · corpus regression (`-runs=0`) · a short random smoke (`FUZZ_SMOKE_SECONDS`, default 30). Long campaigns/distillation/sanitizer matrices are B8 |
| Reference model | `PYTHONPATH=reference-model/src python3 -m unittest discover -s reference-model/tests`; vector freshness via `python3 tools/reference-model/generate-vectors.py --check`; normative LMSR documentation-table agreement via `python3 tools/reference-model/check-doc-table.py` (04 §5; 15 §4.4) |
| Economic simulation (S4) | `PYTHONPATH=reference-model/src:simulation/src python3 -m unittest discover -s simulation/tests` (CI, in the reference-model job). The calibration runner `python3 tools/simulation/run-calibration.py` is evidence tooling, not a CI gate: `--check` re-verifies the committed `simulation/results/phase0-calibration.json` (structure, byte-exact pinned subsample, Merkle root) and deliberately exits 1 while the artifact records economic violations — red-by-design pending SQ-231; `--full` regenerates the ≥ 10⁴-proposal Phase-0 evidence (15 §4.9; G0 consumes it) |
| Formal models (S1) | `tools/verify/run-model-checks.sh` — pinned TLC over `models/tla/*` per each `manifest.env`: main configs green above their distinct-state floor AND witness configs MUST violate (reachability anti-vacuity); CI job `model-checking` (15 §4.1) |
| Property suites (S1) | `tools/ci/property-gates.sh` — the 03 §11/15 §4.2–4.3 suites at ≥10⁶ proptest cases in release with `--locked` (the script hard-rejects a lower `PROPTEST_CASES`); reduced-count runs happen implicitly in `cargo test --workspace`; CI job `property-suites` fans out per-crate across parallel runners (the script takes an optional `ledger`/`market`/`constitution` shard; no argument runs all) |
| Supply chain (15 §4.5) | `tools/ci/supply-chain-gates.sh` — committed lockfiles (`cargo metadata --locked`, both workspaces) + pinned `cargo-audit 0.22.2` (root run under the annotated `.cargo/audit.toml` exceptions, keeper audited with none) + the **GHSA-only leg** (`check-ghsa-only.py` over pinned `osv-scanner`, annotated `tools/ci/ghsa-waivers.toml`): RustSec is a strict subset of the GitHub Advisory DB for crates.io, so cargo-audit is structurally blind to GHSA-only advisories — this leg gates exactly that complement and nothing cargo-audit already sees (SQ-219). Per-commit CI job and release-blocking `release.yml` leg |
| Tooling suites | `python3 -m unittest discover -s <dir>` over `tools/deploy/tests`, `tools/reference-model/tests`, `tools/release/tests`, `tools/phase-gates/tests`, `tools/env/tests`, `tools/ci/tests`, `tools/monitoring/tests` (the env suite needs `pyyaml==6.0.2` + `websockets==15.0.1`; the monitoring suite needs `pyyaml==6.0.2`); `python3 tools/env/validate-environments.py` |
| Monitoring (O5) | `python3 tools/monitoring/check_alert_coverage.py` — the 12 §6.3 gate: both alert tables strictly extracted, every row bound to ≥1 Prometheus rule carrying the row's exact RB-* runbook (the two "page immediately" rows must carry `severity: page`), every rule metric declared in `tools/monitoring/series-inventory.toml` and present in the exporters' `SERIES` registries, and declared seams expire mechanically once their owning PLAN.md milestone (B10/O3) flips ✅ |
| Release sweep (04 §4 cadence) | full ≥10⁷-point corpus: `python3 tools/reference-model/generate-vectors.py --sweep-out <dir>` then `BLEAVIT_SWEEP_DIR=<dir> BLEAVIT_SWEEP_REQUIRE_FULL=1 cargo test -p futarchy-fixed --release --locked --test sweep -- --ignored`; runs in `release.yml` and on kernel/numerics changes via `sweep.yml` — not per-commit |
| Frontend (once scaffolded) | lint · typecheck · unit tests · build; dependency-cruiser firewall clean |
| Docs | every relative link in living documents resolves; PLAN.md's Markdown tables are structurally well-formed (`python3 tools/ci/check-plan-tables.py` — standing user instruction 2026-07-17: table formatting must never drift/break; also a Stop hook); PLAN.md's spec-question batch index is consistent with the question table (`python3 tools/ci/check-spec-question-batches.py` — unique ids, every open row in exactly one batch, no batch naming a closed row); the runbook set stays bound to doc 12 §6.1/§6.3 (`python3 tools/deploy/check-runbooks.py`, O4) |

## Repository layout

| Path | Status | What it is |
|---|---|---|
| `docs/architecture/` | spec | The specification (00–15 + README) |
| `docs/design/` | derived | Non-normative design-context pack (`claude-design-kit/`: spec distillations + Claude Design prompt); spec wins on conflict; regenerate after any spec change |
| `PLAN.md` | living | Implementation roadmap, status, session log |
| `README.md` | living | Human orientation |
| `AGENTS.md` / `CLAUDE.md` | living | This manual / Claude Code wiring |
| `.claude/` | living | Settings, hooks, skills, subagents, path rules |
| `.codex/` | living | Codex session playbooks mirroring the skills |
| `Cargo.toml`, `rust-toolchain.toml`, `.github/workflows/` (`ci.yml` · `release.yml` · `sweep.yml`), `tools/ci/`, `.cargo/audit.toml` | scaffold + B8 | M0 workspace/toolchain/CI and local gate scripts; B8 added the supply-chain gate (`supply-chain-gates.sh` + annotated pin-forced audit exceptions, SQ-135), the tag-triggered release pipeline, and the kernel-change sweep workflow |
| `tools/release/` | B8 done | Release-artifact publication tooling (02 §11; 15 §5): reproducible-recipe runtime build, booted-node metadata extraction with `:code`↔wasm binding, the 02 critical-surface manifest + deterministic chainHead fixture recorder, content-addressed assembly with readiness report, the `bleavit.env-evidence.v1` contract (produced by `tools/env/run-evidence.py`, B7), `environments.json` live-env inventory — see `tools/release/README.md`; a real tag release still fails closed on missing per-release B7 evidence, the residual adoption-input gaps from the B1b compliance review (PLAN SQ-173…SQ-175, SQ-177, SQ-180…SQ-182) and SQ-205 (the unwired oracle→treasury reserve-health seam) — both via the manifest's `release_blockers` rows; B2 closed the `FutarchyApi`/metadata-constant gaps, A8/A11 are wired, and SQ-101 re-keyed USDC to the frozen 02 §8 Location |
| `crates/` | scaffold | `futarchy-primitives` (M1) and `futarchy-fixed` (M2) live here; Track A's per-pallet **frame-free functional cores** land here too as `crates/<name>-core/` (`no_std`, no `frame` deps — the differential oracle + WASM/auditor port) |
| `pallets/` | Track A + B4 residual | Track-A custom pallets are production shells (`lib.rs` + mock/tests + benchmarks/weights) over frame-free `crates/<name>-core/`; `pallets/inflow-caps` is the deliberate state-only exception (09 §5.2 shared meter, no dispatchables/benchmark/weights, runs inside caller envelopes) |
| `runtime/` | B1a + B1b + B4 residual + B5 + B6 + B2 + B10 + B11 | `runtime/bleavit-runtime` is the real Cumulus parachain runtime (`construct_runtime!`, `impl_runtime_apis!`, `BaseCallFilter = SafetyFilter`, genesis presets with the 08 §2.1 VIT allocation/vesting). `Epoch`, `ExecutionGuard`, and `InflowCaps` occupy frozen indices 61, 62, and 63. A8 wires the real epoch clock, sovereign origins, market reads/opening, I-9 enqueue/callback path, classifier and e2e coverage; B4 residual wires Params-backed DOT/USDC `TraderRates`, `HealthTrackingRouter` → welfare X traffic (R deliberately unbound, SQ-195), custody-synced treasury pots, and `PhaseInflowCaps`; B5 supplies generated weights/PoV gates. B1b completed the compliance blockers: canonical resource-domain keys (05 §1.4) with set-equality screening, the six-track referenda split (`track_origins` @64) with SeatBond holds, recall and the uphold_veto T24 producer, and the guardian effect substrate (kernel playbook routines, market/ledger freeze endpoints). B2 implemented all 11 `FutarchyApi` methods (`views.rs` + `impl_runtime_apis!`) against contract v4 (now v5). B10 closed the S3 wiring exemptions (live-Params consumers, armed phase-3 caps + exit path, dead-man detector, treasury obligation mirrors, `pol.budget_epoch` shrink-to-fit, `TwapCheckpoints`). B12 activated coretime renewal funding end-to-end (stored ops-multisig quote authority, DOT-planck quotes with TTL/supersession, ceil-rounded USDC line debits via the three `ops.ct_*` keys, live `XcmRenewalDispatcher`; SQ-245/SQ-246 ruled 2026-07-18). |
| `runtime/bleavit-xcm/` | B4 done + residual bindings | The XCM layer as a runtime-independent library the runtime wires: 09 §6.1 rule-table barrier/assets/trader components, the 07 §8 reserve-probe program + authenticated response router, the 09 §4 coretime-renewal funding leg (verified relay-teleport route), 09 §5.2 inflow-cap adapters, the `pallet_xcm` call classifier — B10 wired the full production posture: `CappedInflows` is the live `AssetTransactor` over Location-keyed ForeignAssets, `BleavitBarrier`/`BleavitReserves`/AssetTrap recovery and the 09 §6.2 exit filter are runtime-bound, and constitution-backed `TraderRates` remains live |
| `runtime-api/` | B2 done | `futarchy-runtime-api`: the `sp_api::decl_runtime_apis!` declaration of the frozen 11-method `FutarchyApi` (02 §3) over the §4 view types in `futarchy-primitives`. B2 implemented all 11 in the runtime (`runtime/bleavit-runtime/src/views.rs` + `impl_runtime_apis!`) and landed the 02 amendment batch as **integration contract v4** (the contract has since moved to **v5**, bumped in `fd2faa0`; `futarchy_primitives::INTEGRATION_CONTRACT_VERSION = 5` and 02 §13 are the authorities, so the next bump is v6); B13 added the separate monitoring-only `TelemetryApi` trait (`runtime-api/src/telemetry.rs` + `runtime/bleavit-runtime/src/telemetry.rs`) — explicitly outside the 02 contract, owned by 12 §6.3, consumed only by the ops exporters |
| `node/` | B3 done | `node/bleavit-node` — collator binary as a thin branding of the pinned `polkadot-omni-node` stack (runtime ships in the chain spec, not the node) |
| `deploy/`, `tools/deploy/` | B3 + O4 done (grows with B7/B8/Track O) | Chain-spec pipeline + validator (02 §8/§10), bootnode operator manifests, production genesis-allocation template, ss58-registry submission artifact; **`deploy/runbooks/` (O4)** — the 12 §6.3 runbooks-as-code set: 13 runbooks RB-KEEPER…RB-RELEASE with machine-readable frontmatter bound to doc 12's alert tables, gated by `tools/deploy/check-runbooks.py` (bidirectional §6.1/§6.3 binding; CI `docs` job + `tools/deploy/tests`) |
| `keeper/` | B9 done | `keeper/bleavit-keeper` — the off-chain keeper reference implementation (01 §4.2 role): subxt-dynamic planner/submitter cranking every permissionless extrinsic, per-role Prometheus metrics (12 §6.3). A **separate cargo workspace** (root `exclude = ["keeper"]`): its subxt dependency tree must never perturb the runtime workspace's `=`-exact stable2606 pins; `tools/ci/rust-workspace-gates.sh` runs its fmt/clippy/test leg. On-chain counterpart: the 08 §6.3 keeper meter + `KeeperRebateSink` seams live in the treasury/crank pallets |
| `deploy/monitoring/`, `tools/monitoring/` | O5 done | The 12 §6.3 monitoring/alerting stack: Prometheus scrape config + alert rules covering all 20 §6.3 rows (spec-threshold annotations, RB-* runbook labels, `severity: page` on the two page-immediately rows) + Alertmanager routing with the §5.2(3) release-integrity channel; `chain_alerts_exporter.py` (on-chain-event alerting over frozen `FutarchyApi` `state_call`, the raw 168-byte `ReleaseChannel` key, prefix counts, finalized events — per-family fail-closed degradation, never healthy zeros); `attestation_monitor.py` (the 12 §5.2 out-of-band monitor: ≥3 gateways, fetch by TXID and by name, byte-compare vs the signed `release.json` map, pure-stdlib RFC 8032/minisign verification incl. `revoked_key_bits`, ≥2 attestations, `manifest_txid` cross-check, hourly + head-driven); `check_alert_coverage.py` + `series-inventory.toml` (the coverage gate; B13 closed the 9 runtime seams via the monitoring-only `TelemetryApi` — a non-02 `decl_runtime_apis!` surface (12 §6.3) the exporter consumes over `state_call`, incl. the cumulative metadata-resolved LMSR domain-rejection counter and per-component POL series; only O3's 3 bootnode seams remain). The frontend ships no telemetry (12 §6.3); `release.json` field names remain O1-provisional; RB-* runbook documents are O4 |
| `models/` | S1 done | TLA⁺ formal models (15 §4.1): `tla/ledger` (proposal-vault conservation/I-3/I-26/amended-I-27/D-8 over all interleavings; fingerprint-view partition — pure-state invariants at large scopes, label invariants in no-view audit scopes) and `tla/proposal` (T1–T24, I-9/I-14/I-15/I-18, constant-controlled mutation configs wired as permanent falsifiability witnesses); per-model `manifest.env` drives `tools/verify/run-model-checks.sh` |
| `tools/verify/` | S1 done | Model-checking harness: digest-pinned tla2tools fetch (`tools/env/pins.env` is the pin home) + the runner with distinct-state floors and expected-violation witness legs |
| `reference-model/` | M3 done (grown by A4–A9, S1, S4) | Independent Python executable spec + the single vector generator (`tools/reference-model/generate-vectors.py`) and corpus (`fixtures/vectors.json`, schema v4): LMSR/TWAP/decision/welfare/treasury scenarios + the S1 ledger differential families (64 op-sequence scenarios, score-endpoint, sweep, 11-error-class witnesses) replayed exactly by `conditional-ledger-core` and the pallet sweep differential; S4 scoped the `in_cap_prize` NAV floor to upgrade payloads (`upgrade_payload=True` default, vectors byte-stable) |
| `simulation/`, `tools/simulation/` | S4 (sim done; publication parked) | The 15 §4.9 agent-based Phase-0 economic simulation over the reference model as normative-math source: executed fee-inclusive LMSR trade ledger (informed/noise/arbitrage-A-2/five doc-14 manipulator strategies at 3·InCapPrize-multiple budgets), real Survival/Security gate books, κ-slew segment TWAP (equivalence-tested), pre-registered strata, binary-searched flip brackets, and the deterministic Merkle-bound evidence artifact `simulation/results/phase0-calibration.json` (10,000 proposals). Phase-0 result (committed artifact, schema v4, `designation: published`, `violations: []`): the per-class **decidable-harm** false-pass rates are PARAM 0.000 % / TREASURY 0.145 % / CODE 0.135 % / META 0.563 % — all four strictly below the 15 §4.9 < 1 % gate, which is *per class and decidable-harm*, not the raw wrong-PASS rate (raw would read CODE 1.99 % / META 2.22 % and fail; batch B6 pinned that reading in 15 §4.9 + 09 §7.1, SQ-269). The earlier "PARAM 3.46 % / TREASURY 1.52 % fail" line described the superseded N=3000 pre-calibration artifact and was stale. `sec.prize.*`/`sec.flow_cap` are **published** in the artifact but not yet **adopted** into 13 — those rows still carry `[VERIFY]`, and publication is what the Phase-0 criterion requires (SQ-268) |
| `tools/limit-coverage/` | S3 done | The 15 §4.6 / I-22 generated limit-coverage gate: `check-limit-coverage.py` (strict extractor over 13 §1/§2/§4 with rule-6 ParamKey semantics + per-bound expansion of multi-limit rows; coverage checker with lexical error/behavior binding) · `registry.toml` (exhaustive 178-key classification manifest: 66 dispatch-limit / 75 param-bounds / 36 value / 1 diagnostic / 0 unwired) · `genesis-keys.json` (98 seeded keys, fixture byte-asserted against `constitution_core::genesis_params()` by a constitution test) · `tests/`. Unwired keys print on every run and expire mechanically — the checker fails once the owning milestone flips ✅; B10 cleared all consumer-binding entries; B12 cleared the last unwired key — the coretime quote leg is live with a marked dispatch-past-limit test (SQ-245/SQ-246 ruled 2026-07-18) |
| `tools/phase-gates/` | G0 🔨 (parked pending S4) | The machine-checked 09 §7.1 Phase-0 exit gate: `check-phase0-exit.py` executes the reference-model ≡ pallets differential legs (the three Python legs + five Rust differential commands, incl. the full ≥10⁷ sweep unless `--reduced`) and consumes the S4-owned `bleavit.sim-calibration.v1` artifact **fail-closed** (absent ⇒ `pending-s4`, invalid ⇒ `fail`, sim `git_commit` must equal checked HEAD); the Phase-0 calibration key set (δ per class, `pol.b_baseline`, `sec.prize.*`, `sec.flow_cap`) is lexically bound to 13's sim-gated tags so spec drift fails loudly. Publishes `bleavit.phase0-evidence.v1`; exit 0 only on full Phase-0 exit. Tests in the CI tooling-suites job; the checker itself is deliberately not a per-commit gate (red until S4 by design) |
| `frontend/` | scaffold (Track F) | Placeholder root for monorepo per 10 §10 (`apps/web`, `packages/*`, `tools/*`) |
| `zombienet/`, `chopsticks/`, `tools/env/` | B7 done | Test-environment definitions — release artifacts, not private fixtures (15 §4.7; 02 §11): zombienet relay+para(+AH/Coretime) topologies + the 09 §7.1 drill suite (`.zndsl` + js helpers), chopsticks forked-state scenario configs for every upgrade path and all six 06 §6.2 playbooks, pinned tooling (`tools/env/pins.env` single-homes the zombienet/chopsticks/polkadot-sdk/paseo-CSG pins) + fetch/generate scripts and the structural validator (`tools/env/validate-environments.py`, CI job `environments`). The **B7 evidence producer** executes suites against built artifacts at tag time and emits `bleavit.env-evidence.v1`; gated suites block evidence fail-closed under SQ-139/SQ-202, Chopsticks card execution is SQ-203, and the closing try-state leg is SQ-204. **G1 (2026-07-18, first real execution, V-36)**: drills 01/02/03 pass end-to-end (keeper runs as a real topology node via `zombienet/scripts/keeper-node.sh`); zndsl grammar/timeout repairs, the Paseo-CSG `WASM_BUILD_WORKSPACE_HINT` release-blocker fix, and the fast-runtime + `num_cores` relay scheduling rulings landed in `tools/env/`; post-#105 (G0/S4) merge (2026-07-20): drills **01/02/03/06 pass** — SQ-274 resolved, drill 06 stages `MigrationHalt` at genesis via the new `pallet-execution-guard` `migration_halt` field (plain spec; no production fault-injection surface); 07's `spec_version` sentinel retired but blocked on a pinned-Paseo asset-hub-runtime inherent trap; 05's staged-renewal code landed (genesis seeding + ~8 h dead-man run remain); **09 passes 6/6** via a default-off `fast-timing` compressed-epoch test runtime (SQ-128 implemented — three real 84-block epochs advanced unattended in ~29 min; two kernel floors + three registry seeds off one `kernel::FAST_DAY_BLOCKS` knob, release runtime byte-frozen). **Fast-timing extended (SQ-128) to drills 04 & 08**: `DEAD_MAN_RELAY_BLOCKS` 4,800→48, `DESCRIPTOR_LEAD_TIME_BLOCKS` 43,200→12. **G1 Phase-1 exit reached (2026-07-20, SQ-282 resolved):** the 09 §7.1 line-282 drill set passes — 01/02/03/06 + compressed 09 6/6 + **04 dead-man 18/18** + **05 coretime-under-dead-man 19/19**. SQ-282: the relay GRANDPA finalized head is not parachain-runtime-observable on stable2606 (SDK-verified), so 05 §4.6/§4.8 + 13 §2 + 14 TH-37 were re-scoped to the observable relay-parent-gap trigger (detector code already correct, no runtime-code change) and drill 04 re-pointed to a collator outage → relay-parent gap → engage+recover; drill 05 proves the D-9 coretime freeze-exemption via a probe under the engaged dead-man (genesis-seeded coretime authority/account through the existing treasury fields). **Drills 07 (Phase-3 XCM funding, line 284) and 08 (Phase-2 expedited-CODE under staged freeze, line 283) are not Phase-1 gates**; the `bleavit.env-evidence.v1` bundle is release-train (G2+, SQ-203/204). Carried forward: SQ-283 (off-chain relay-finality monitor, O5), SQ-284 (raw-`TreasuryState` polkadot-js decode quirk), SQ-233 (cross-milestone trigger feeds). |
| `fuzz/` | S2 done | `bleavit-fuzz` — cargo-fuzz (libFuzzer) targets for the three 15 §4.5 areas: `payload_scale_decode` (execution-guard `Payload` decode + guard invariants), `nested_wrapper_filter` (SafetyFilter I-10/I-11 differential vs an independent 06 §3.3 oracle, incl. `proxy_announced`/`as_multi_threshold_1`), `lmsr_trade_paths` (market-core I-12 drain bound over Decision/Gate/Baseline books). A **separate nightly-pinned cargo workspace** (root `exclude = ["keeper", "fuzz"]`, own `rust-toolchain.toml`): libFuzzer + nightly must never perturb the runtime workspace's `=`-exact stable2606 pins — the `keeper/` precedent. Curated seed corpora under `fuzz/corpus/`; `tools/ci/fuzz-gates.sh` (CI job `fuzz`) runs fmt/clippy/oracle-tests + build + corpus-regression + a short random smoke. Long campaigns/distillation are B8. The guard's preimage decode-depth hardening (`MAX_PAYLOAD_DECODE_DEPTH`, SQ-225) rode this milestone |

## Changing the specification

The spec is complete and the product of a 101-finding review, so changes should be
rare and deliberate — but `docs/architecture/` is editable, not guarded. When a change
is warranted, follow **R-1**: make it consistent across the whole doc set (owning doc +
every referencing doc + 00's decision record if a D-n is affected), bump
`INTEGRATION_CONTRACT_VERSION` per 02 §13 when 02 changes, honor the joint
backend+frontend sign-off that 02 §13 and 15 §2.1 mandate for `02`/INV-FE edits, and
record what changed (and why, and who authorized it) in PLAN.md · *Decision log*. If a
semantic change is non-obvious, raise it in PLAN.md · *Spec questions* and confirm with
the user first.

## Where things live

- Claude Code specifics (skills, subagents, hooks): `CLAUDE.md`, `.claude/`
- Codex playbooks: `.codex/README.md`, `.codex/prompts/`
- Roadmap and status: `PLAN.md` · Human orientation: `README.md`
- The spec: `docs/architecture/` — start at its README

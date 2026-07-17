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
  (see rule R-1). Doc 00 is the decision record (D-1…D-18); 01 the system overview;
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
| Reference model | `PYTHONPATH=reference-model/src python3 -m unittest discover -s reference-model/tests`; vector freshness via `python3 tools/reference-model/generate-vectors.py --check`; normative LMSR documentation-table agreement via `python3 tools/reference-model/check-doc-table.py` (04 §5; 15 §4.4) |
| Formal models (S1) | `tools/verify/run-model-checks.sh` — pinned TLC over `models/tla/*` per each `manifest.env`: main configs green above their distinct-state floor AND witness configs MUST violate (reachability anti-vacuity); CI job `model-checking` (15 §4.1) |
| Property suites (S1) | `tools/ci/property-gates.sh` — the 03 §11/15 §4.2–4.3 suites at ≥10⁶ proptest cases in release with `--locked` (the script hard-rejects a lower `PROPTEST_CASES`); reduced-count runs happen implicitly in `cargo test --workspace`; CI job `property-suites` |
| Supply chain (15 §4.5) | `tools/ci/supply-chain-gates.sh` — committed lockfiles (`cargo metadata --locked`, both workspaces) + pinned `cargo-audit 0.22.2` (root run under the annotated `.cargo/audit.toml` exceptions, keeper audited with none); per-commit CI job and release-blocking `release.yml` leg |
| Tooling suites | `python3 -m unittest discover -s <dir>` over `tools/deploy/tests`, `tools/reference-model/tests`, `tools/release/tests`, `tools/env/tests` (the env suite needs `pyyaml==6.0.2` + `websockets==15.0.1`); `python3 tools/env/validate-environments.py` |
| Release sweep (04 §4 cadence) | full ≥10⁷-point corpus: `python3 tools/reference-model/generate-vectors.py --sweep-out <dir>` then `BLEAVIT_SWEEP_DIR=<dir> BLEAVIT_SWEEP_REQUIRE_FULL=1 cargo test -p futarchy-fixed --release --locked --test sweep -- --ignored`; runs in `release.yml` and on kernel/numerics changes via `sweep.yml` — not per-commit |
| Frontend (once scaffolded) | lint · typecheck · unit tests · build; dependency-cruiser firewall clean |
| Docs | every relative link in living documents resolves |

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
| `tools/release/` | B8 done | Release-artifact publication tooling (02 §11; 15 §5): reproducible-recipe runtime build, booted-node metadata extraction with `:code`↔wasm binding, the 02 critical-surface manifest + deterministic chainHead fixture recorder, content-addressed assembly with readiness report, the `bleavit.env-evidence.v1` contract (produced by `tools/env/run-evidence.py`, B7), `environments.json` live-env inventory — see `tools/release/README.md`; a real tag release still fails closed on missing per-release B7 evidence, the B1b compliance gaps (PLAN SQ-172…SQ-182) and SQ-205 (the unwired oracle→treasury reserve-health seam) — both via the manifest's `release_blockers` rows; B2 closed the `FutarchyApi`/metadata-constant gaps, A8/A11 are wired, and SQ-101 re-keyed USDC to the frozen 02 §8 Location |
| `crates/` | scaffold | `futarchy-primitives` (M1) and `futarchy-fixed` (M2) live here; Track A's per-pallet **frame-free functional cores** land here too as `crates/<name>-core/` (`no_std`, no `frame` deps — the differential oracle + WASM/auditor port) |
| `pallets/` | Track A + B4 residual | Track-A custom pallets are production shells (`lib.rs` + mock/tests + benchmarks/weights) over frame-free `crates/<name>-core/`; `pallets/inflow-caps` is the deliberate state-only exception (09 §5.2 shared meter, no dispatchables/benchmark/weights, runs inside caller envelopes) |
| `runtime/` | B1a + B4 residual + B5 + B6 + B2; B1b review-open | `runtime/bleavit-runtime` is the real Cumulus parachain runtime (`construct_runtime!`, `impl_runtime_apis!`, `BaseCallFilter = SafetyFilter`, genesis presets with the 08 §2.1 VIT allocation/vesting). `Epoch`, `ExecutionGuard`, and `InflowCaps` occupy frozen indices 61, 62, and 63. A8 wires the real epoch clock, sovereign origins, market reads/opening, I-9 enqueue/callback path, classifier and e2e coverage; B4 residual wires Params-backed DOT/USDC `TraderRates`, `HealthTrackingRouter` → welfare X traffic (R deliberately unbound, SQ-195), custody-synced treasury pots, and `PhaseInflowCaps`; B5 supplies generated weights/PoV gates. B1b remains 🔨 for SQ-172, SQ-176 and SQ-178. Open follow-up: implement `FutarchyApi` from `runtime-api/` in `impl_runtime_apis!` (B2). B2 implemented all 11 `FutarchyApi` methods (`views.rs` + `impl_runtime_apis!`) against contract v4. |
| `runtime/bleavit-xcm/` | B4 done + residual bindings | The XCM layer as a runtime-independent library the runtime wires: 09 §6.1 rule-table barrier/assets/trader components, the 07 §8 reserve-probe program + authenticated response router, the 09 §4 coretime-renewal funding leg (verified relay-teleport route), 09 §5.2 inflow-cap adapters, the `pallet_xcm` call classifier — constitution-backed `TraderRates` is live and `PhaseInflowCaps` is ready for the SQ-101 transactor re-key |
| `runtime-api/` | B2 done | `futarchy-runtime-api`: the `sp_api::decl_runtime_apis!` declaration of the frozen 11-method `FutarchyApi` (02 §3) over the §4 view types in `futarchy-primitives`. B2 implemented all 11 in the runtime (`runtime/bleavit-runtime/src/views.rs` + `impl_runtime_apis!`) and landed the 02 amendment batch as **integration contract v4** |
| `node/` | B3 done | `node/bleavit-node` — collator binary as a thin branding of the pinned `polkadot-omni-node` stack (runtime ships in the chain spec, not the node) |
| `deploy/`, `tools/deploy/` | B3 done (grows with B7/B8/Track O) | Chain-spec pipeline + validator (02 §8/§10), bootnode operator manifests, production genesis-allocation template, ss58-registry submission artifact; runbooks (`deploy/runbooks/`) remain Track O |
| `keeper/` | B9 done | `keeper/bleavit-keeper` — the off-chain keeper reference implementation (01 §4.2 role): subxt-dynamic planner/submitter cranking every permissionless extrinsic, per-role Prometheus metrics (12 §6.3). A **separate cargo workspace** (root `exclude = ["keeper"]`): its subxt dependency tree must never perturb the runtime workspace's `=`-exact stable2603 pins; `tools/ci/rust-workspace-gates.sh` runs its fmt/clippy/test leg. On-chain counterpart: the 08 §6.3 keeper meter + `KeeperRebateSink` seams live in the treasury/crank pallets |
| `vendor/` | support | Vendored `core2 0.4.0` (all published versions yanked; required by the node's networking closure) — `vendor/README.md` has provenance |
| `models/` | S1 done | TLA⁺ formal models (15 §4.1): `tla/ledger` (proposal-vault conservation/I-3/I-26/amended-I-27/D-8 over all interleavings; fingerprint-view partition — pure-state invariants at large scopes, label invariants in no-view audit scopes) and `tla/proposal` (T1–T24, I-9/I-14/I-15/I-18, constant-controlled mutation configs wired as permanent falsifiability witnesses); per-model `manifest.env` drives `tools/verify/run-model-checks.sh` |
| `tools/verify/` | S1 done | Model-checking harness: digest-pinned tla2tools fetch (`tools/env/pins.env` is the pin home) + the runner with distinct-state floors and expected-violation witness legs |
| `reference-model/` | M3 done (grown by A4–A9, S1) | Independent Python executable spec + the single vector generator (`tools/reference-model/generate-vectors.py`) and corpus (`fixtures/vectors.json`, schema v4): LMSR/TWAP/decision/welfare/treasury scenarios + the S1 ledger differential families (64 op-sequence scenarios, score-endpoint, sweep, 11-error-class witnesses) replayed exactly by `conditional-ledger-core` and the pallet sweep differential |
| `tools/limit-coverage/` | S3 done | The 15 §4.6 / I-22 generated limit-coverage gate: `check-limit-coverage.py` (strict extractor over 13 §1/§2/§4 with rule-6 ParamKey semantics + per-bound expansion of multi-limit rows; coverage checker with lexical error/behavior binding) · `registry.toml` (exhaustive 175-key classification manifest: 59 dispatch-limit / 72 param-bounds / 36 value / 1 diagnostic / 7 unwired) · `genesis-keys.json` (95 seeded keys, fixture byte-asserted against `constitution_core::genesis_params()` by a constitution test) · `tests/`. Unwired + kernel-constant consumer-binding keys print on every run (owned by B10) and expire mechanically — the checker fails once the owning milestone flips ✅ |
| `frontend/` | scaffold (Track F) | Placeholder root for monorepo per 10 §10 (`apps/web`, `packages/*`, `tools/*`) |
| `zombienet/`, `chopsticks/`, `tools/env/` | B7 done | Test-environment definitions — release artifacts, not private fixtures (15 §4.7; 02 §11): zombienet relay+para(+AH/Coretime) topologies + the 09 §7.1 drill suite (`.zndsl` + js helpers), chopsticks forked-state scenario configs for every upgrade path and all six 06 §6.2 playbooks, pinned tooling (`tools/env/pins.env` single-homes the zombienet/chopsticks/polkadot-sdk/paseo-CSG pins) + fetch/generate scripts and the structural validator (`tools/env/validate-environments.py`, CI job `environments`). The **B7 evidence producer** executes suites against built artifacts at tag time and emits `bleavit.env-evidence.v1`; gated suites block evidence fail-closed under SQ-139/SQ-202, Chopsticks card execution is SQ-203, and the closing try-state leg is SQ-204. |

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

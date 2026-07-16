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
  without an explicit ask. Never commit with red gates.
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
| Rust | `tools/ci/rust-workspace-gates.sh` (runs `cargo fmt --all -- --check` · `cargo clippy --workspace --all-targets -- -D warnings` · `cargo test --workspace` once member crates exist) |
| Runtime crates | `try-state` green in test envs; benchmarks compile; no new `unwrap`/`expect`/`panic!`/`unsafe` in runtime code |
| Reference model | `PYTHONPATH=reference-model/src python3 -m unittest discover -s reference-model/tests`; vector freshness via `python3 tools/reference-model/generate-vectors.py --check`; normative LMSR documentation-table agreement via `python3 tools/reference-model/check-doc-table.py` (04 §5; 15 §4.4) |
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
| `Cargo.toml`, `rust-toolchain.toml`, `.github/workflows/ci.yml`, `tools/ci/` | scaffold | M0 workspace/toolchain/CI and local gate scripts |
| `crates/` | scaffold | `futarchy-primitives` (M1) and `futarchy-fixed` (M2) live here; Track A's per-pallet **frame-free functional cores** land here too as `crates/<name>-core/` (`no_std`, no `frame` deps — the differential oracle + WASM/auditor port) |
| `pallets/` | partial (Track A, re-scoped) | Custom pallet crates. **Track A ships production FRAME pallets** (2026-07-14 re-scope): each `pallets/<name>/` is a `#[frame_support::pallet]` **shell** (`lib.rs` + `mock.rs` + `tests.rs` + `benchmarking.rs`/`weights.rs`) over its frame-free `crates/<name>-core/`. The existing code is that core; the FRAME shells are the reopened A1–A11 work (see PLAN.md Track A DoD) |
| `runtime/`, `node/` | scaffold (Track B) | Placeholder roots for runtime assembly and collator node. `runtime/bleavit-runtime` is currently a frame-free composition **model** (no `construct_runtime!`/`impl_runtime_apis!`); the real runtime-level FRAME assembly is milestone B1a |
| `runtime-api/` | partial (B2) | `futarchy-runtime-api`: the `sp_api::decl_runtime_apis!` declaration of the frozen 11-method `FutarchyApi` (02 §3) over the §4 view types in `futarchy-primitives`; the runtime-side `impl_runtime_apis!` lands with B1a |
| `reference-model/` | scaffold (M3) | Placeholder root for independent Python executable spec + vector corpus |
| `frontend/` | scaffold (Track F) | Placeholder root for monorepo per 10 §10 (`apps/web`, `packages/*`, `tools/*`) |
| `zombienet/`, `chopsticks/` | planned (B7) | Test-environment definitions (release artifacts, 15 §4.7) |
| `deploy/runbooks/` | planned (Track O) | Runbooks-as-code (12 §6) |

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

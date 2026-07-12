# AGENTS.md — Operating Manual for Coding Agents

This repository implements **bleevit**: a futarchy-governed Polkadot parachain
(native Rust FRAME pallets, LMSR conditional markets) with a canonical decentralized
frontend (Arweave-distributed static app, in-browser light client). The complete,
authoritative specification already exists; the job of every session is to turn it
into code, one milestone at a time, without ever degrading the specification or the
project's living documents.

Read this file first. Then read `PLAN.md`. Then work.

## Ground truth

- **`docs/architecture/` (00–15) is the single source of truth and it is FROZEN** —
  see rule R-1. Doc 00 is the decision record (D-1…D-18); 01 the system overview;
  02 the frozen chain↔frontend integration contract; 03–09 the protocol components;
  10–12 the frontend and operations; 13 the only home of parameter values;
  14 the threat model; 15 the invariants and the normative testing regime.
  Reading order for newcomers: 01 → 02 → 03 → 04 → 05, then as needed
  (`docs/architecture/README.md`).
- Constants and parameters have exactly two homes: `02` (chain identity, frozen
  contract surface) and `13` (everything else). Any other file that needs a value
  references them — including code (kernel constants from `futarchy-primitives`,
  tunables from `pallet-constitution::Params`; the frontend reads chain
  metadata/storage, never hardcodes).
- **`PLAN.md` is the single source of implementation status** — what is done, in
  progress, blocked, and next. It references architecture sections and never restates
  their content.

## Rules

- **R-1 — The architecture is frozen.** Never modify, delete, rename, or add anything
  under `docs/architecture/`. Implementation conforms to the spec, never the reverse.
  Found a defect, ambiguity, or contradiction? Record it in PLAN.md · *Spec questions*
  with a precise citation and ask the user. Amendments happen only through the
  change-control procedure below. (Enforced by permission rules and hooks; do not
  work around them.)
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
| Rust (once the workspace exists) | `cargo fmt --all -- --check` · `cargo clippy --workspace --all-targets -- -D warnings` · `cargo test --workspace` |
| Runtime crates | `try-state` green in test envs; benchmarks compile; no new `unwrap`/`expect`/`panic!`/`unsafe` in runtime code |
| Reference model | its pytest suite; CI-regenerated vectors match committed vectors (04 §5) |
| Frontend (once scaffolded) | lint · typecheck · unit tests · build; dependency-cruiser firewall clean |
| Docs | every relative link in living documents resolves; `docs/architecture/` untouched (`git status`) |

## Repository layout

| Path | Status | What it is |
|---|---|---|
| `docs/architecture/` | frozen | The specification (00–15 + README) |
| `docs/design/` | derived | Non-normative design-context pack (`claude-design-kit/`: spec distillations + Claude Design prompt); spec wins on conflict; regenerate after any amendment |
| `PLAN.md` | living | Implementation roadmap, status, session log |
| `README.md` | living | Human orientation |
| `AGENTS.md` / `CLAUDE.md` | living | This manual / Claude Code wiring |
| `.claude/` | living | Settings, hooks, skills, subagents, path rules |
| `.codex/` | living | Codex session playbooks mirroring the skills |
| `crates/futarchy-primitives`, `crates/futarchy-fixed` | planned (M1) | `no_std` shared types + verified 64.64 math |
| `pallets/*` | planned (Track A) | The 12 custom pallets (01 §5.1) |
| `runtime/`, `runtime-api/`, `node/` | planned (Track B) | Runtime assembly, `FutarchyApi`, collator node |
| `reference-model/` | planned (M2) | Independent Python executable spec + vector corpus |
| `frontend/` | planned (Track F) | Monorepo per 10 §10 (`apps/web`, `packages/*`, `tools/*`) |
| `zombienet/`, `chopsticks/` | planned (B7) | Test-environment definitions (release artifacts, 15 §4.7) |
| `deploy/runbooks/` | planned (Track O) | Runbooks-as-code (12 §6) |

## Amending the architecture (change control)

Almost never the right move — the set disposed of a 101-finding review. When the user
explicitly decides an amendment:

1. The user states the change and authorizes it in this session (quote it in PLAN.md ·
   *Decision log*). Changes to `02` or to the INV-FE texts additionally require the
   joint backend+frontend sign-off that those documents themselves mandate (02 §13,
   15 §2.1) — the user speaks for both sides or names who does.
2. Create `.claude/architecture-amendment.flag` (empty file). This stands the write
   guard down; the Stop hook will refuse to end the session while it exists.
3. Make the amendment consistently across the doc set (owning doc + every referencing
   doc + 00's decision record if a D-n is affected; bump `INTEGRATION_CONTRACT_VERSION`
   per 02 §13 when 02 changes).
4. Record in PLAN.md · *Decision log*: what changed, why, the user's authorization,
   affected docs. Then **delete the flag** and finish the session normally.

## Where things live

- Claude Code specifics (skills, subagents, hooks): `CLAUDE.md`, `.claude/`
- Codex playbooks: `.codex/README.md`, `.codex/prompts/`
- Roadmap and status: `PLAN.md` · Human orientation: `README.md`
- The spec: `docs/architecture/` — start at its README

---
name: implement
description: Run one spec-driven implementation increment for the Bliwit futarchy system — pick (or receive) the next PLAN.md milestone, implement it strictly from docs/architecture/, verify it per doc 15, and update PLAN.md. Use at the start of a working session, when the user says "continue" / "next step" / "weiter", or when a specific milestone is named.
argument-hint: "[milestone-id, e.g. A2]"
---

# One spec-driven implementation increment

You are executing the core session loop of this repository (AGENTS.md · Session
protocol). One session = **one milestone** (or one clearly-scoped slice of a large
milestone). Never start a second milestone in the same session.

## 1. Orient

- Read `PLAN.md` in full: Current focus, milestone tables, Spec questions, last
  Session log entries.
- Target = `$ARGUMENTS` if given; otherwise the milestone marked 🔨 (in progress);
  otherwise the first ⬜ milestone whose **Depends** column is fully ✅.
- If the target is ⛔ blocked or its dependencies aren't done, say so and stop —
  don't silently substitute a different milestone; propose the correct next one instead.

## 2. Read the spec — before any code

- Read every `docs/architecture/` section cited in the milestone's **Spec** column,
  plus the standing slices: `02` for any frozen names/types you'll touch, `13` for any
  numeric value, `15` for the milestone's verification obligations.
- The spec is frozen (rule R-1). If it is ambiguous, contradictory, or silent on
  something you need: record it in PLAN.md · **Spec questions** (cite doc §), pick the
  most conservative reading **only if** work can proceed safely without the answer —
  otherwise mark the milestone ⛔ and ask the user.

## 3. Declare scope

- Set the milestone to 🔨 in PLAN.md and write one sentence in **Current focus**:
  what will be true when this session ends.
- Break the work into a visible todo list (implementation, tests, gates, doc sync).

## 4. Implement

- Follow the path rules (`.claude/rules/`) — they encode the non-negotiables
  (determinism, bounded storage, frozen names, no hardcoded parameters, provenance
  typing, …).
- Implementation decisions cite the spec: significant choices land in code as
  behavior the spec names, not as invented policy. Anything the spec doesn't own
  (crate-internal structure) follows workspace conventions.

## 5. Verify

- Write/extend the tests the milestone's **Verify** column and doc 15 require —
  delegate bulk test authoring to the `test-engineer` subagent when substantial.
- Run the quality gates (AGENTS.md · Quality gates): at minimum
  `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace` (scale to what exists; frontend: lint/typecheck/test/build).
- Launch the `spec-reviewer` subagent on the changed component. Fix blockers/majors;
  minors may be logged as follow-ups in PLAN.md notes.

## 6. Close the session

- Update PLAN.md: milestone status (✅ only if gates green AND spec-reviewer found no
  blockers; otherwise stays 🔨 with precise resume notes), Current focus, and a new
  Session log row: `| YYYY-MM-DD | <id> | <done> | <next> |`.
- Refresh README.md / AGENTS.md / CLAUDE.md if the repo shape, commands, or workflow
  changed (or run `/sync-docs`).
- Report to the user: what was delivered, gate results (verbatim pass/fail), open
  questions, and the suggested conventional-commit message
  (e.g. `feat(ledger): implement split/merge families (A2)`). Do not commit unless asked.

## Hard rules

- Never mark ✅ with failing gates — report failures honestly instead.
- Never touch `docs/architecture/` (the hooks enforce this; don't fight them).
- Never resolve a `[VERIFY]` tag by assumption: verify against live sources
  (WebSearch/WebFetch) and record the outcome in PLAN.md · Verification log.
- If context is running low mid-milestone, stop at a clean boundary, write exact
  resume notes in PLAN.md, and end the session gracefully.

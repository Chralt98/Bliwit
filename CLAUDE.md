# CLAUDE.md

@AGENTS.md

Everything above (imported from AGENTS.md) is binding. Below is the Claude-Code-specific wiring.

## Skills (invoke with `/name`; auto-invoke when the description matches)

| Skill | Use for |
|---|---|
| `/implement [id]` | The session driver: one PLAN.md milestone, spec-first, verified, PLAN updated. Default entry point for "continue"/"next step". |
| `/spec-audit [scope]` | Compliance sweep of implemented code against `docs/architecture/` (report-only; logs to PLAN.md · Audit log). |
| `/sync-docs` | Re-true README/PLAN/AGENTS/CLAUDE and the `.claude`/`.codex` assets against the actual repo. |
| `/new-pallet <name>` | Scaffold a FRAME pallet with spec-cited stubs, mock, test/benchmark stubs, try-state hook. |

## Subagents (delegate via the Agent tool)

| Agent | Role |
|---|---|
| `spec-reviewer` | Read-only compliance audit of a component vs its owning doc. Run it before marking any milestone ✅ (R-6). |
| `test-engineer` | Authors the doc-15 test obligations (PT suites, limit-coverage, negative origin tests, try-state, differential vectors). |
| `doc-curator` | End-of-session living-document sync when the delta is large. |

## Hooks (installed via `.claude/settings.json` — expect these behaviors)

- **SessionStart** injects git state + PLAN.md focus/milestones/last log rows. Trust it
  for orientation, but still read PLAN.md before implementing.
- **Stop guard** blocks ending a session when the tree changed but PLAN.md wasn't
  updated. Comply (update PLAN.md) instead of retrying.
- **Stop guard (`guard-readme.sh`)** blocks ending a session if README.md's pinned
  opening (thank-you to Prof. Robin Hanson) or closing (Bon appétit) line has been
  altered (rule R-11, AGENTS.md). Restore the exact wording instead of retrying.
- **Stop guard (`guard-plan-tables.sh`)** blocks ending a session if any PLAN.md
  Markdown table is structurally broken (orphaned rows severed from their header by
  a blank line, wrong cell count, unescaped `|` — GFM splits cells on pipes even
  inside backticks; escape as `\|`). Standing user instruction (2026-07-17): PLAN.md
  table formatting must never drift/break. Fix the reported rows (same checker as
  the docs CI job: `python3 tools/ci/check-plan-tables.py`) instead of retrying.

> There is no longer any write guard on `docs/architecture/`. The spec is editable;
> change it deliberately per rule R-1 (AGENTS.md · *Changing the specification*).

Permissions: common read-only git and cargo commands are pre-allowed; `git push`
always asks.

## Memory notes

Auto-memory exists for this project. PLAN.md — not memory — is the canonical
implementation status; keep memories as pointers (e.g. "status lives in PLAN.md"),
never as duplicated status that can go stale.

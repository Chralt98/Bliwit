#!/usr/bin/env bash
# Stop hook — enforces the standing user instruction (2026-07-17) that PLAN.md's
# Markdown table formatting must never drift/break (the B10/B11 incident: a blank
# line inside the Track B milestones table stranded rows from their header).
# Runs the same checker as the docs CI job. Blocks at most once (stop_hook_active).
set -euo pipefail

INPUT=$(cat)
ACTIVE=$(jq -r '.stop_hook_active // false' <<<"$INPUT")
[ "$ACTIVE" = "true" ] && exit 0

cd "${CLAUDE_PROJECT_DIR:-$(pwd)}"
[ -f PLAN.md ] && [ -f tools/ci/check-plan-tables.py ] || exit 0

OUTPUT=$(python3 tools/ci/check-plan-tables.py 2>&1) && exit 0

jq -n --arg out "$OUTPUT" \
  '{decision:"block", reason:("PLAN.md has malformed Markdown tables (standing user instruction 2026-07-17: table formatting must never drift/break; enforced here and in the docs CI job). Fix the rows below — typically a blank line severing rows from their table header, a missing/extra cell, or an unescaped | (escape as \\| — GFM splits cells on pipes even inside backticks) — then re-run python3 tools/ci/check-plan-tables.py.\n\n" + $out)}'

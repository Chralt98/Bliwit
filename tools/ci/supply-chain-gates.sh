#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
cd "$repo_root"

summary_out=""
if [[ $# -gt 0 ]]; then
  if [[ $# -ne 2 || $1 != "--summary-out" || -z $2 ]]; then
    echo "usage: $0 [--summary-out <file>]" >&2
    exit 2
  fi
  summary_out=$2
fi

auditor="${BLEAVIT_AUDITOR:-$repo_root/target/tools/bin/cargo-audit}"
required_auditor_version="cargo-audit 0.22.2"
if [[ ! -x "$auditor" ]] || [[ "$($auditor --version 2>/dev/null || true)" != "$required_auditor_version" ]]; then
  cargo install cargo-audit --version 0.22.2 --locked --root target/tools
fi

assert_lockfile() {
  local lockfile=$1
  if [[ ! -f "$lockfile" ]]; then
    echo "required lockfile is missing: $lockfile" >&2
    return 1
  fi
  if ! git ls-files --error-unmatch -- "$lockfile" >/dev/null; then
    echo "required lockfile is not committed: $lockfile" >&2
    return 1
  fi
}

assert_lockfile Cargo.lock
cargo metadata --locked --no-deps --format-version=1 >/dev/null

assert_lockfile keeper/Cargo.lock
(
  cd keeper
  cargo metadata --locked --no-deps --format-version=1 >/dev/null
)

# cargo-audit reads .cargo/audit.toml from its current working directory. The
# root SDK/node closure uses the annotated stable2603 exceptions; keeper is a
# separate workspace and is intentionally audited from its own clean root so
# none of those exceptions can mask a future keeper vulnerability.
"$auditor" audit
(
  cd keeper
  "$auditor" audit --no-fetch
)

if [[ -n "$summary_out" ]]; then
  summary_tmp=$(mktemp -d)
  trap 'rm -rf "$summary_tmp"' EXIT
  "$auditor" audit --json --no-fetch >"$summary_tmp/root.json"
  (
    cd keeper
    "$auditor" audit --json --no-fetch >"$summary_tmp/keeper.json"
  )
  python3 - "$summary_out" "$summary_tmp/root.json" "$summary_tmp/keeper.json" <<'PY'
import json
import sys
from pathlib import Path

output, root_report, keeper_report = map(Path, sys.argv[1:])


def load(path):
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise SystemExit(f"cargo-audit report is not an object: {path}")
    return value


def warning_summary(report):
    warnings = report.get("warnings", {})
    if not isinstance(warnings, dict):
        raise SystemExit("cargo-audit warnings field is not an object")
    by_kind = {
        kind: len(rows)
        for kind, rows in sorted(warnings.items())
        if isinstance(rows, list) and rows
    }
    return {
        "allowed_warning_count": sum(by_kind.values()),
        "allowed_warnings_by_kind": by_kind,
    }


root = load(root_report)
keeper = load(keeper_report)
ignored = root.get("settings", {}).get("ignore", [])
if not isinstance(ignored, list) or any(not isinstance(item, str) for item in ignored):
    raise SystemExit("cargo-audit settings.ignore is not a string array")
summary = {
    "schema": "bleavit.supply-chain.v1",
    "ignored_advisory_ids": sorted(ignored),
    "workspaces": {
        "root": warning_summary(root),
        "keeper": warning_summary(keeper),
    },
}
output.parent.mkdir(parents=True, exist_ok=True)
output.write_text(json.dumps(summary, indent=2, sort_keys=True) + "\n", encoding="utf-8")
PY
fi

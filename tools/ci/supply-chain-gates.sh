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

# cargo-audit only sees what RustSec carries. For crates.io the GitHub Advisory
# Database is a strict superset — an advisory can have no RUSTSEC id at all, and
# the leg above is then blind to it rather than merely silent (yamux
# GHSA-vxx9-2994-q338, a HIGH remote panic, is the worked example). This leg
# gates exactly that complement via osv-scanner, which aggregates both DBs; see
# tools/ci/check-ghsa-only.py and tools/ci/ghsa-waivers.toml.
# shellcheck source=../env/pins.env
source "$repo_root/tools/env/pins.env"
# BLEAVIT_OSV_SCANNER is an explicit operator/test override and is trusted as
# given (same contract as BLEAVIT_AUDITOR above). The digest pin guards the
# binary this script fetches itself.
if [[ -n "${BLEAVIT_OSV_SCANNER:-}" ]]; then
  osv="$BLEAVIT_OSV_SCANNER"
else
  osv="$repo_root/target/tools/bin/osv-scanner"
  if [[ ! -x "$osv" ]] || [[ "$(sha256sum "$osv" | cut -d' ' -f1)" != "$OSV_SCANNER_SHA256" ]]; then
    mkdir -p "$(dirname "$osv")"
    curl --fail --silent --show-error --location --retry 3 --max-time 300 -o "$osv.tmp" \
      "https://github.com/google/osv-scanner/releases/download/${OSV_SCANNER_VERSION}/osv-scanner_linux_amd64"
    actual=$(sha256sum "$osv.tmp" | cut -d' ' -f1)
    if [[ "$actual" != "$OSV_SCANNER_SHA256" ]]; then
      rm -f "$osv.tmp"
      echo "osv-scanner digest mismatch for ${OSV_SCANNER_VERSION}: expected $OSV_SCANNER_SHA256, got $actual" >&2
      exit 1
    fi
    chmod +x "$osv.tmp"
    mv "$osv.tmp" "$osv"
  fi
fi
python3 "$repo_root/tools/ci/check-ghsa-only.py" \
  --scanner "$osv" \
  --waivers "${BLEAVIT_GHSA_WAIVERS:-$repo_root/tools/ci/ghsa-waivers.toml}" \
  --lockfile "$repo_root/Cargo.lock" \
  --lockfile "$repo_root/keeper/Cargo.lock"

if [[ -n "$summary_out" ]]; then
  summary_tmp=$(mktemp -d)
  trap 'rm -rf "$summary_tmp"' EXIT
  "$auditor" audit --json --no-fetch >"$summary_tmp/root.json"
  (
    cd keeper
    "$auditor" audit --json --no-fetch >"$summary_tmp/keeper.json"
  )
  python3 - "$summary_out" "$summary_tmp/root.json" "$summary_tmp/keeper.json" \
    "${BLEAVIT_GHSA_WAIVERS:-$repo_root/tools/ci/ghsa-waivers.toml}" \
    "$repo_root/tools/ci/check-ghsa-only.py" <<'PY'
import importlib.util
import json
import sys
from pathlib import Path

output, root_report, keeper_report, ghsa_waivers, checker_path = map(Path, sys.argv[1:])


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
# SQ-135's disclosed-waiver property is "the FULL waived-ID list in every
# release manifest". Since the GHSA-only leg carries its own waivers, listing
# only the RustSec ignores would understate what the release is shipping
# accepted risk on. Schema v2 adds them; the assembler validates both lists.
spec = importlib.util.spec_from_file_location("check_ghsa_only", checker_path)
checker = importlib.util.module_from_spec(spec)
spec.loader.exec_module(checker)
waived_ghsa_only = [
    {"id": identifier, "package": package, "version": version}
    for identifier, package, version in sorted(checker.load_waivers(ghsa_waivers))
]

summary = {
    "schema": "bleavit.supply-chain.v2",
    "ignored_advisory_ids": sorted(ignored),
    "waived_ghsa_only": waived_ghsa_only,
    "workspaces": {
        "root": warning_summary(root),
        "keeper": warning_summary(keeper),
    },
}
output.parent.mkdir(parents=True, exist_ok=True)
output.write_text(json.dumps(summary, indent=2, sort_keys=True) + "\n", encoding="utf-8")
PY
fi

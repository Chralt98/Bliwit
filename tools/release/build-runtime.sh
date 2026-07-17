#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
cd "$repo_root"

out_dir=${1:-release-work/runtime}
wasm_source="target/release/wbuild/bleavit-runtime/bleavit_runtime.compact.compressed.wasm"
toolchain=$(sed -n 's/^channel = "\([^"]*\)"/\1/p' rust-toolchain.toml)
if [[ -z "$toolchain" ]]; then
  echo "rust-toolchain.toml does not declare a channel" >&2
  exit 1
fi

if [[ -z "${SOURCE_DATE_EPOCH:-}" ]]; then
  SOURCE_DATE_EPOCH=$(git show -s --format=%ct HEAD)
fi
export SOURCE_DATE_EPOCH
export TZ=UTC
export LANG=C.UTF-8
export LC_ALL=C.UTF-8
export CARGO_INCREMENTAL=0
export CARGO_TERM_COLOR=never

cargo build -p bleavit-runtime --release --features substrate-wasm-builder --locked

if [[ ! -f "$wasm_source" ]]; then
  echo "runtime wasm was not produced at $wasm_source" >&2
  exit 1
fi

mkdir -p "$out_dir"
cp "$wasm_source" "$out_dir/runtime.wasm"

TOOLCHAIN="$toolchain" OUT_DIR="$out_dir" python3 - <<'PY'
import hashlib
import json
import os
import platform
import subprocess
from pathlib import Path

out_dir = Path(os.environ["OUT_DIR"])
wasm = out_dir / "runtime.wasm"

def command(*args):
    return subprocess.run(args, check=True, capture_output=True, text=True).stdout.strip()

rustc_verbose = command("rustc", "-vV")
host = next(
    (line.split(":", 1)[1].strip() for line in rustc_verbose.splitlines() if line.startswith("host:")),
    platform.machine(),
)
commit = command("git", "rev-parse", "HEAD")
digest = hashlib.sha256(wasm.read_bytes()).hexdigest()
info = {
    "schema": "bleavit.runtime-build.v1",
    "git_commit": commit,
    "source_date_epoch": int(os.environ["SOURCE_DATE_EPOCH"]),
    "toolchain": os.environ["TOOLCHAIN"],
    "host_triple": host,
    "cargo_version": command("cargo", "--version"),
    "rustc_version": command("rustc", "--version"),
    "rustc_verbose_version": rustc_verbose,
    "wasm": {
        "path": "runtime.wasm",
        "sha256": digest,
        "size": wasm.stat().st_size,
    },
    "recipe": "cargo build -p bleavit-runtime --release --features substrate-wasm-builder --locked",
    "normalized_environment": {
        "CARGO_INCREMENTAL": os.environ["CARGO_INCREMENTAL"],
        "CARGO_TERM_COLOR": os.environ["CARGO_TERM_COLOR"],
        "LANG": os.environ["LANG"],
        "LC_ALL": os.environ["LC_ALL"],
        "SOURCE_DATE_EPOCH": os.environ["SOURCE_DATE_EPOCH"],
        "TZ": os.environ["TZ"],
    },
    "reproducibility_scope": "same toolchain + same source => same bytes; host/container image is not yet digest-pinned",
    "rfc78_metadata_hash": {
        "enabled": False,
        "reason": "runtime build.rs uses build_using_defaults and Cargo.toml has no metadata-hash feature",
    },
}
(out_dir / "build-info.json").write_text(
    json.dumps(info, indent=2, sort_keys=True) + "\n", encoding="utf-8"
)
PY

echo "runtime release inputs written to $out_dir"


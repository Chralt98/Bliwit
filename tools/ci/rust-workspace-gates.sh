#!/usr/bin/env bash
set -euo pipefail

member_count=$(cargo metadata --no-deps --format-version=1 | python3 -c 'import json,sys; print(len(json.load(sys.stdin)["workspace_members"]))')

if [[ "$member_count" == "0" ]]; then
  echo "Rust workspace has no member crates yet; fmt/clippy/test gates are armed in CI and will run once members are added."
  exit 0
fi

cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

if [[ -f tools/fixed/generate-lmsr-corpus.py ]]; then
  python3 tools/fixed/generate-lmsr-corpus.py --check
fi

if [[ -d reference-model/tests ]]; then
  PYTHONPATH=reference-model/src python3 -m unittest discover -s reference-model/tests
fi

if [[ -f tools/reference-model/generate-vectors.py ]]; then
  python3 tools/reference-model/generate-vectors.py --check
fi

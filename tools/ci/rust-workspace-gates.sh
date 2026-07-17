#!/usr/bin/env bash
set -euo pipefail

# Native node dependencies use bindgen. Point it at LLVM's library directory
# when distributions do not install an unversioned libclang in the default path.
if [[ -z "${LIBCLANG_PATH:-}" ]] && command -v llvm-config >/dev/null 2>&1; then
  export LIBCLANG_PATH="$(llvm-config --libdir)"
fi

# Networked RustSec checks run in supply-chain-gates.sh; this offline-friendly
# workspace gate still enforces the committed lockfile on every cargo command.
member_count=$(cargo metadata --locked --no-deps --format-version=1 | python3 -c 'import json,sys; print(len(json.load(sys.stdin)["workspace_members"]))')

if [[ "$member_count" == "0" ]]; then
  echo "Rust workspace has no member crates yet; fmt/clippy/test gates are armed in CI and will run once members are added."
  exit 0
fi

cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked

# B6 release gate (09 §2.1(5)): compile the deployable runtime and its
# benchmarking surface, then compile and execute the runtime's genesis-state
# `TryRuntime_on_runtime_upgrade` + try-state coverage. The live-chain snapshot
# `try-runtime-cli` leg mandated by 15 §4.7 lands with the B7/B8 environment
# and release-artifact work; this local leg does not claim snapshot coverage.
cargo build -p bleavit-runtime --release --locked
cargo build -p bleavit-runtime --release --features runtime-benchmarks --locked
cargo build -p bleavit-runtime --features try-runtime --locked
cargo test -p bleavit-runtime --features try-runtime --locked

# Real no_std build gate: the frame-free math surface (futarchy-primitives,
# futarchy-fixed) must compile without std (01 §5.2 / rule 9). A --no-default-features
# `cargo test` executes zero tests and so silently passes; a build does not.
if cargo metadata --locked --no-deps --format-version=1 \
  | python3 -c 'import json,sys; ms={m["name"] for m in json.load(sys.stdin)["packages"]}; sys.exit(0 if {"futarchy-primitives","futarchy-fixed"} <= ms else 1)'; then
  cargo build -p futarchy-primitives -p futarchy-fixed --no-default-features --locked
fi

# I-24 XCM-isolation lint (15 §1, rule 7): `xcm`/`pallet-xcm` types must never be
# imported by the decision/settlement pallets or their frame-free cores; in the
# oracle the only permitted XCM-adjacent surface is the reserve-probe
# `QueryResponse` *handler seam* (`reserve_probe_result`, 07 §8), which needs no
# xcm imports either — so the deny list covers oracle too. Word-boundary match
# catches `use xcm...`, `staging_xcm...`, `pallet_xcm...` in code and manifests;
# `cumulus-primitives-core` is denied too (it re-exports XCM types — a smuggling
# path). This enforces the *import* half of the 15 §1 lint; the "XCM-derived
# storage reads" half is not grep-detectable and stays review-enforced (I-24 is
# convention-class — 15 §1).
i24_paths=(
  pallets/epoch pallets/welfare pallets/market pallets/conditional-ledger pallets/oracle
  crates/epoch-core crates/welfare-core crates/market-core crates/conditional-ledger-core crates/oracle-core
)
i24_existing=()
for p in "${i24_paths[@]}"; do [[ -d "$p" ]] && i24_existing+=("$p"); done
if [[ ${#i24_existing[@]} -gt 0 ]]; then
  if grep -rnE '(^|[^a-zA-Z0-9_-])(staging[-_]xcm|pallet[-_]xcm|xcm_executor|xcm_builder|cumulus[-_]primitives[-_]core)([^a-zA-Z0-9_-]|$)|^\s*use xcm' \
      "${i24_existing[@]}" --include='*.rs' --include='Cargo.toml'; then
    echo "I-24 violation: xcm import found in a decision/settlement pallet (15 §1, runtime-code rule 7)" >&2
    exit 1
  fi
fi

# Off-chain keeper reference implementation (B9): a separate cargo workspace so
# subxt's dependency tree cannot perturb the runtime workspace's `=`-exact pins.
if [[ -d keeper ]]; then
  (
    cd keeper
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets --locked -- -D warnings
    cargo test --workspace --locked
  )
fi

if [[ -d reference-model/tests ]]; then
  PYTHONPATH=reference-model/src python3 -m unittest discover -s reference-model/tests
fi

if [[ -f tools/reference-model/generate-vectors.py ]]; then
  python3 tools/reference-model/generate-vectors.py --check
fi

python3 tools/reference-model/check-doc-table.py

# I-22 convention gate (15 §4.6): the strict extractor fails on registry drift,
# and every classified dispatch limit must remain attached to a Rust test.
python3 -m unittest discover -s tools/limit-coverage/tests
python3 tools/limit-coverage/check-limit-coverage.py

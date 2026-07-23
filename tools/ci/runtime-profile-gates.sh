#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
cd "$repo_root"

profile_tool=tools/release/runtime_profiles.py
python3 "$profile_tool"

profiles=(
  bootstrap
  phase-four
  bootstrap-recovery
  phase-four-recovery
)

for profile in "${profiles[@]}"; do
  features=$(python3 "$profile_tool" --profile "$profile" --field features)
  test_features=${features/,substrate-wasm-builder/}
  if [[ "$profile" == phase-four* ]]; then
    cargo test -p bleavit-runtime --lib --no-default-features \
      --features "$test_features" --locked tests_migration_guard
    cargo test -p bleavit-runtime --lib --no-default-features \
      --features "$test_features" --locked composition_contains_all_wired_pallets
    cargo test -p bleavit-runtime --lib --no-default-features \
      --features "$test_features" --locked metadata_call_inventory_is_bidirectionally_exhaustive
    cargo test -p bleavit-runtime --lib --no-default-features \
      --features "$test_features" --locked metadata_call_carriers_equal_the_pinned_closed_wrapper_set
    cargo test -p bleavit-runtime --lib --no-default-features \
      --features "$test_features" --locked every_inventory_row_materializes_as_a_real_runtime_call
  else
    cargo test -p bleavit-runtime --lib --no-default-features \
      --features "$test_features" --locked
  fi
  cargo clippy -p bleavit-runtime --lib --no-default-features \
    --features "$test_features,runtime-benchmarks,try-runtime" --locked -- -D warnings
  cargo check -p bleavit-runtime --no-default-features \
    --features "$test_features,runtime-benchmarks,try-runtime" --locked
  cargo build -p bleavit-runtime --release --no-default-features \
    --features "$features" --locked

  recovery=$(python3 "$profile_tool" --profile "$profile" --field recovery)
  if [[ "$recovery" == "true" ]]; then
    verification_log=$(mktemp)
    trap 'rm -f "$verification_log"' EXIT
    cargo test -p bleavit-runtime --no-default-features \
      --features "$test_features" --locked \
      recovery_profile_has_zero_multi_block_migrations 2>&1 \
      | tee "$verification_log"
    passing_summaries=$(grep -Ec 'test result: ok\. 1 passed; 0 failed;' "$verification_log" || true)
    if [[ "$passing_summaries" != "1" ]]; then
      echo "$profile did not execute exactly one zero-MBM proof test" >&2
      exit 1
    fi
    rm -f "$verification_log"
    trap - EXIT
  fi
done

echo "runtime profile gates passed"

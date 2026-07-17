#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

if [[ ! -d "${repo_root}/fuzz" ]]; then
  echo "fuzz workspace not found at ${repo_root}/fuzz" >&2
  exit 1
fi

if ! command -v cargo-fuzz >/dev/null 2>&1; then
  echo "cargo-fuzz 0.12.0 is required." >&2
  echo "Install it with: cargo install cargo-fuzz --version 0.12.0 --locked" >&2
  exit 1
fi

cd "${repo_root}/fuzz"

cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked

targets=(payload_scale_decode nested_wrapper_filter lmsr_trade_paths)

# Corpus regression: replay every committed seed once (`-runs=0` does not mutate
# or extend the corpus, so the curated `corpus/<target>` stays deterministic).
for target in "${targets[@]}"; do
  cargo fuzz build "${target}"
  cargo fuzz run "${target}" "corpus/${target}" -- \
    -runs=0 \
    -rss_limit_mb=4096
done

smoke_seconds="${FUZZ_SMOKE_SECONDS:-30}"

# Short random smoke. libFuzzer writes coverage-increasing inputs into its FIRST
# corpus argument, so we point that at a throwaway (git-ignored) directory and
# pass the curated corpus as read-only seed inputs. This keeps the committed
# corpus curated and deterministic — corpus distillation is B8 release scope.
smoke_root="${repo_root}/fuzz/artifacts/smoke"
for target in "${targets[@]}"; do
  case "${target}" in
    payload_scale_decode) max_len=131072 ;;
    nested_wrapper_filter) max_len=16384 ;;
    lmsr_trade_paths) max_len=8192 ;;
    *)
      echo "missing max_len for fuzz target ${target}" >&2
      exit 1
      ;;
  esac

  smoke_corpus="${smoke_root}/${target}"
  mkdir -p "${smoke_corpus}"
  cargo fuzz run "${target}" "${smoke_corpus}" "corpus/${target}" -- \
    -max_total_time="${smoke_seconds}" \
    -rss_limit_mb=4096 \
    -timeout=25 \
    -max_len="${max_len}"
done

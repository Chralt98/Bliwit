#!/usr/bin/env bash
set -euo pipefail

export PROPTEST_CASES="${PROPTEST_CASES:-1000000}"

if [[ ! "${PROPTEST_CASES}" =~ ^[0-9]+$ ]]; then
  echo "property-suites: PROPTEST_CASES must be numeric and >= 1000000 (got '${PROPTEST_CASES}')" >&2
  exit 2
fi
normalized_cases="${PROPTEST_CASES}"
while [[ "${normalized_cases}" == 0* && ${#normalized_cases} -gt 1 ]]; do
  normalized_cases="${normalized_cases#0}"
done
if [[ ${#normalized_cases} -lt 7 ]] \
  || [[ ${#normalized_cases} -eq 7 && "${normalized_cases}" < "1000000" ]]; then
  echo "property-suites: PROPTEST_CASES must be numeric and >= 1000000 (got '${PROPTEST_CASES}')" >&2
  exit 2
fi

echo "property-suites: PROPTEST_CASES=${PROPTEST_CASES}"

# Optional per-crate shard selector. The combined run is ~70-85 min on a hosted
# runner and intermittently trips "runner lost communication" (CPU/memory
# starvation on a single marathon job); CI fans the suites out across fresh
# parallel runners by passing a shard. No argument runs every suite, so the
# local all-in-one gate (and any nightly/release caller) is unchanged.
suite="${1:-all}"

run_ledger() {
  # `property_tests` already includes PT-5b (the 03 §12 reap-vs-late-redeemer
  # sweep race, `pt5b_mock_sweeps_interleave_with_late_claims`) — it does not
  # need a second standalone invocation, which only doubled the wall-clock.
  echo "property-suites: pallet-conditional-ledger PT-1..PT-8 (incl. PT-5b sweep race, 03 §12)"
  cargo test --locked -p pallet-conditional-ledger --release property_tests
}

run_market() {
  echo "property-suites: market-core I-12/I-13"
  cargo test --locked -p market-core --release --test property
}

run_constitution() {
  echo "property-suites: constitution-core I-6/I-7"
  cargo test --locked -p constitution-core --release --test property
}

case "${suite}" in
  ledger) run_ledger ;;
  market) run_market ;;
  constitution) run_constitution ;;
  all)
    run_ledger
    run_market
    run_constitution
    ;;
  *)
    echo "property-suites: unknown shard '${suite}' (expected ledger|market|constitution, or no argument for all)" >&2
    exit 2
    ;;
esac

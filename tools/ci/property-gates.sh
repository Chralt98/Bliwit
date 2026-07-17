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

echo "property-suites: pallet-conditional-ledger PT-1..PT-8"
cargo test --locked -p pallet-conditional-ledger --release property_tests

echo "property-suites: pallet-mock reap-vs-late-redeemer sweep sequence"
cargo test --locked -p pallet-conditional-ledger --release pt5b_mock_sweeps_interleave_with_late_claims

echo "property-suites: market-core I-12/I-13"
cargo test --locked -p market-core --release --test property

echo "property-suites: constitution-core I-6/I-7"
cargo test --locked -p constitution-core --release --test property

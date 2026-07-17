#!/usr/bin/env bash
# S1 model-checking gate (15 §4.1; 03 §11): run TLC over every committed TLA+
# model and fail on any invariant/property violation, parse error, or vacuous
# state space.
#
# Contract per model directory models/tla/<name>/:
#   manifest.env  — MODULE=<TlaModuleName>
#                   CONFIGS="<A>.cfg [<B>.cfg ...]"   (each checked in order)
#                   MIN_DISTINCT_STATES=<n>           (anti-vacuity floor,
#                                                      applied to every config)
#                   TLC_FLAGS="..."                   (optional, e.g. -deadlock)
#                   WITNESS_CONFIGS="..."             (optional; reachability
#                     witnesses: each config asserts the NEGATION of an
#                     interesting condition as an invariant, and TLC MUST
#                     report a violation — proving the condition is reachable.
#                     A witness config that passes clean is a FAILURE: the
#                     model can no longer reach the behavior it claims to
#                     exercise. This is the anti-vacuity counterpart to
#                     MIN_DISTINCT_STATES.)
#   <Module>.tla + the listed .cfg files.
#
# The normative model inventory is fixed here: a missing required model fails
# the gate (mirrors tools/env/validate-environments.py's inventory rule).
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
MODELS_DIR="${REPO_ROOT}/models/tla"
JAR="${REPO_ROOT}/tools/verify/bin/tla2tools.jar"
REQUIRED_MODELS=(ledger proposal)

"${REPO_ROOT}/tools/verify/fetch-tla2tools.sh"

command -v java >/dev/null || { echo "ERROR: java (11+) is required to run TLC." >&2; exit 1; }

for required in "${REQUIRED_MODELS[@]}"; do
    if [[ ! -d "${MODELS_DIR}/${required}" ]]; then
        echo "ERROR: required model models/tla/${required} is missing (normative inventory)." >&2
        exit 1
    fi
done

overall_rc=0
for model_dir in "${MODELS_DIR}"/*/; do
    name="$(basename "${model_dir}")"
    manifest="${model_dir}manifest.env"
    if [[ ! -f "${manifest}" ]]; then
        echo "ERROR: ${name}: missing manifest.env" >&2
        overall_rc=1
        continue
    fi
    MODULE="" CONFIGS="" MIN_DISTINCT_STATES="" TLC_FLAGS="" WITNESS_CONFIGS=""
    # shellcheck disable=SC1090
    source "${manifest}"
    : "${MODULE:?${name}: MODULE missing from manifest.env}"
    : "${CONFIGS:?${name}: CONFIGS missing from manifest.env}"
    : "${MIN_DISTINCT_STATES:?${name}: MIN_DISTINCT_STATES missing from manifest.env}"

    for cfg in ${CONFIGS}; do
        if [[ ! -f "${model_dir}${cfg}" ]]; then
            echo "ERROR: ${name}: config ${cfg} listed in manifest.env but absent" >&2
            overall_rc=1
            continue
        fi
        echo "=== TLC: ${name}/${MODULE} with ${cfg} ==="
        log="$(mktemp)"
        set +e
        # A unique -metadir keeps TLC scratch state out of the repo and makes
        # concurrent runner invocations collision-free (shared states/ dirs
        # with -cleanup deleted each other's live scratch — S1 finding).
        scratch="$(mktemp -d)"
        # shellcheck disable=SC2086
        (cd "${model_dir}" && java -XX:+UseParallelGC -cp "${JAR}" tlc2.TLC \
            -workers auto -metadir "${scratch}" -config "${cfg}" ${TLC_FLAGS} "${MODULE}.tla") \
            2>&1 | tee "${log}"
        rc="${PIPESTATUS[0]}"
        rm -rf "${scratch}"
        set -e
        if [[ "${rc}" -ne 0 ]]; then
            echo "ERROR: ${name}/${cfg}: TLC exited ${rc} (violation or model error)." >&2
            overall_rc=1
        fi
        distinct="$(grep -oE '[0-9]+ distinct states found' "${log}" | grep -oE '^[0-9]+' | tail -1 || true)"
        if [[ -z "${distinct}" ]]; then
            echo "ERROR: ${name}/${cfg}: could not read the distinct-state count from TLC output." >&2
            overall_rc=1
        elif [[ "${distinct}" -lt "${MIN_DISTINCT_STATES}" ]]; then
            echo "ERROR: ${name}/${cfg}: ${distinct} distinct states < declared floor ${MIN_DISTINCT_STATES} — vacuous model." >&2
            overall_rc=1
        else
            echo "OK: ${name}/${cfg}: ${distinct} distinct states (floor ${MIN_DISTINCT_STATES})."
        fi
        rm -f "${log}"
    done

    for cfg in ${WITNESS_CONFIGS}; do
        if [[ ! -f "${model_dir}${cfg}" ]]; then
            echo "ERROR: ${name}: witness config ${cfg} listed in manifest.env but absent" >&2
            overall_rc=1
            continue
        fi
        echo "=== TLC witness (violation expected): ${name}/${MODULE} with ${cfg} ==="
        log="$(mktemp)"
        set +e
        scratch="$(mktemp -d)"
        # shellcheck disable=SC2086
        (cd "${model_dir}" && java -XX:+UseParallelGC -cp "${JAR}" tlc2.TLC \
            -workers auto -metadir "${scratch}" -config "${cfg}" ${TLC_FLAGS} "${MODULE}.tla") \
            > "${log}" 2>&1
        rc=$?
        rm -rf "${scratch}"
        set -e
        if [[ "${rc}" -eq 0 ]] || ! grep -qE "Invariant .* is violated|Temporal properties were violated" "${log}"; then
            echo "ERROR: ${name}/${cfg}: witness did NOT produce the expected violation — the modeled behavior is unreachable (vacuity)." >&2
            tail -20 "${log}" >&2
            overall_rc=1
        else
            echo "OK: ${name}/${cfg}: witness violation produced (behavior reachable)."
        fi
        rm -f "${log}"
    done
done

if [[ "${overall_rc}" -ne 0 ]]; then
    echo "Model-checking gate FAILED." >&2
else
    echo "Model-checking gate passed."
fi
exit "${overall_rc}"

#!/usr/bin/env bash
# Fetch the pinned TLA+ tools jar (TLC) for the S1 model-checking gate (15 §4.1).
#
# The pin lives in tools/env/pins.env (the single committed pin home):
#   TLA2TOOLS_VERSION / TLA2TOOLS_SHA256
# A digest mismatch is a hard failure — never trust an unpinned artifact.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PINS_FILE="${REPO_ROOT}/tools/env/pins.env"
BIN_DIR="${REPO_ROOT}/tools/verify/bin"
JAR="${BIN_DIR}/tla2tools.jar"

# shellcheck source=../env/pins.env
source "${PINS_FILE}"

: "${TLA2TOOLS_VERSION:?TLA2TOOLS_VERSION missing from tools/env/pins.env}"
: "${TLA2TOOLS_SHA256:?TLA2TOOLS_SHA256 missing from tools/env/pins.env}"

verify_digest() {
    echo "${TLA2TOOLS_SHA256}  ${JAR}" | sha256sum --check --status
}

if [[ -f "${JAR}" ]] && verify_digest; then
    echo "tla2tools.jar ${TLA2TOOLS_VERSION} already present and digest-verified."
    exit 0
fi

mkdir -p "${BIN_DIR}"
URL="https://github.com/tlaplus/tlaplus/releases/download/${TLA2TOOLS_VERSION}/tla2tools.jar"
echo "Fetching ${URL}"
curl -sSL --fail --max-time 300 -o "${JAR}.tmp" "${URL}"

echo "${TLA2TOOLS_SHA256}  ${JAR}.tmp" | sha256sum --check --status || {
    echo "ERROR: tla2tools.jar digest mismatch against tools/env/pins.env pin — refusing." >&2
    sha256sum "${JAR}.tmp" >&2
    rm -f "${JAR}.tmp"
    exit 1
}
mv "${JAR}.tmp" "${JAR}"
echo "tla2tools.jar ${TLA2TOOLS_VERSION} fetched and digest-verified."

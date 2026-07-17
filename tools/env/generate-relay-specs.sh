#!/usr/bin/env bash
# Generate all B7 plain/raw chain specs — 15 §4.7; 02 §8/§11; 09 §7.1.
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
# shellcheck source=pins.env
source "$repo_root/tools/env/pins.env"

cache="$repo_root/target/env/paseo-chain-spec-generator-src"
generator_target="$repo_root/target/env/paseo-chain-spec-generator"
out="$repo_root/zombienet/specs/out"
mkdir -p "$(dirname "$cache")" "$generator_target" "$out"

if [[ ! -d "$cache/.git" ]]; then
  git clone --filter=blob:none --no-checkout https://github.com/paseo-network/runtimes.git "$cache"
fi

if ! git -C "$cache" rev-parse --verify --quiet "refs/tags/${PASEO_CSG_TAG}^{commit}" >/dev/null; then
  git -C "$cache" fetch --depth 1 origin "refs/tags/${PASEO_CSG_TAG}:refs/tags/${PASEO_CSG_TAG}"
fi
git -C "$cache" switch --detach "refs/tags/${PASEO_CSG_TAG}"

actual_commit=$(git -C "$cache" rev-parse HEAD)
if [[ "$actual_commit" != "$PASEO_CSG_COMMIT" ]]; then
  echo "Paseo chain-spec-generator provenance mismatch: tag $PASEO_CSG_TAG resolved to $actual_commit, expected $PASEO_CSG_COMMIT" >&2
  exit 1
fi

# WASM_BUILD_WORKSPACE_HINT: substrate-wasm-builder locates the workspace by
# walking UP from OUT_DIR (not from the source manifest). With CARGO_TARGET_DIR
# inside this repository, that walk escapes the Paseo clone and finds Bleavit's
# Cargo.lock first, so `cargo metadata` runs over the wrong workspace and every
# paseo runtime build script panics "Failed to find entry for package …". The
# hint pins the walk to the pinned Paseo workspace (the builder's designed
# escape hatch for exactly this layout).
# fast-runtime features (G1, first real execution finding): the relay builds
# parachain ValidatorGroups only at session boundaries, and the default
# paseo-local session is 600 blocks (~1 h) — until the first boundary no
# parachain can be scheduled at all, which starves every drill's assertion
# window. The pinned Paseo tree's own fast-runtime feature shortens local
# sessions to 10 blocks (first boundary ≈ block 10), the standard local-net
# configuration across the ecosystem. The generator's `fast-runtime` feature
# forwards only to the RELAY runtime, so the coretime and asset-hub system
# runtimes must be toggled explicitly — otherwise the generated XCM topology
# carries a relay/parachain TIMESLICE_PERIOD split (20 vs 80), corrupting
# exactly the 09 §4 coretime drills the topology exists for. Paseo-side
# runtimes only; no Bleavit runtime byte or 13-owned tunable is affected.
WASM_BUILD_WORKSPACE_HINT="$cache" CARGO_TARGET_DIR="$generator_target" cargo build \
  --manifest-path "$cache/Cargo.toml" \
  --release \
  --locked \
  --features "fast-runtime,coretime-paseo-runtime/fast-runtime,asset-hub-paseo-runtime/fast-runtime" \
  -p chain-spec-generator

generator="$generator_target/release/chain-spec-generator"

show_available_chains() {
  echo "chain-spec-generator rejected a required chain; available chains reported by the pinned generator:" >&2
  "$generator" --help >&2 || true
}

generate_json() {
  local chain=$1
  local target=$2
  rm -f "$target"
  if ! "$generator" "$chain" > "$target"; then
    rm -f "$target"
    show_available_chains
    exit 1
  fi
  if [[ ! -s "$target" ]] || ! python3 -m json.tool "$target" >/dev/null; then
    rm -f "$target"
    echo "chain-spec-generator did not produce valid JSON for required chain '$chain'" >&2
    show_available_chains
    exit 1
  fi
}

generate_json paseo-local "$out/paseo-local.json"
generate_json asset-hub-paseo-local "$out/asset-hub-paseo-local.json"
generate_json coretime-paseo-local "$out/coretime-paseo-local.json"

# Drill-relay genesis override (G1, first real execution finding): the paseo
# `local` preset leaves `scheduler_params.num_cores = 0`. ValidatorGroups are
# built only at session boundaries from the then-active `num_cores`, and the
# scheduler's `expected_claim_queue_len = min(num_cores, validator_groups)`
# never polls the zombienet-injected coretime assignments while either factor
# is 0. Seeding `num_cores = 3` at genesis guarantees the first boundary
# (block ~10 under fast-runtime above) builds groups with headroom for the
# largest topology (bleavit-xcm: 4242 + Asset Hub + Coretime; zombienet adds
# its own per-para bump on top). Relay-side host configuration for the local
# drill relay only — no 13-owned Bleavit tunable is touched.
python3 - "$out/paseo-local.json" <<'PY'
import json, sys
path = sys.argv[1]
with open(path) as handle:
    spec = json.load(handle)
config = (
    spec["genesis"]["runtimeGenesis"]["patch"]
    .setdefault("configuration", {})
    .setdefault("config", {})
)
scheduler = config.setdefault("scheduler_params", {})
scheduler["num_cores"] = 3
with open(path, "w") as handle:
    json.dump(spec, handle, indent=2)
    handle.write("\n")
PY

# Reuse the B3-pinned runtime/spec pipeline. The constitution registry is
# code-owned and cannot be genesis-replaced; the drill patch only repeats the
# release bootstrap PhaseFlags. Export the release local preset, merge that
# one drill override, and feed the result through the builder's verified
# `create ... patch` route.
"$repo_root/tools/deploy/generate-chain-specs.sh"
builder="$repo_root/target/tools/bin/chain-spec-builder"
wasm="$repo_root/target/release/wbuild/bleavit-runtime/bleavit_runtime.compact.compressed.wasm"
preset_patch="$repo_root/target/env/bleavit-local-preset.json"
drill_patch="$repo_root/target/env/bleavit-drill-patch.json"
properties="tokenSymbol=VIT,tokenDecimals=12,ss58Format=7777"

rm -f "$preset_patch"
"$builder" display-preset --runtime "$wasm" --preset-name local_testnet > "$preset_patch"
if [[ ! -s "$preset_patch" ]] || ! python3 -m json.tool "$preset_patch" >/dev/null; then
  rm -f "$preset_patch"
  echo "chain-spec-builder did not produce a valid local_testnet preset" >&2
  exit 1
fi

python3 - "$preset_patch" "$repo_root/zombienet/genesis/drill-overrides.json" "$drill_patch" <<'PY'
import json
import sys
from pathlib import Path

preset_path = Path(sys.argv[1])
override_path = Path(sys.argv[2])
output_path = Path(sys.argv[3])
preset = json.loads(preset_path.read_text(encoding="utf-8"))
override = json.loads(override_path.read_text(encoding="utf-8"))

def merge(target, source):
    for key, value in source.items():
        if isinstance(value, dict) and isinstance(target.get(key), dict):
            merge(target[key], value)
        else:
            target[key] = value

merge(preset, override)
output_path.write_text(json.dumps(preset, indent=2) + "\n", encoding="utf-8")
PY

rm -f "$out/bleavit-drill.json"
"$builder" --chain-spec-path "$out/bleavit-drill.json" create \
  --chain-name "Bleavit Local Drills" \
  --chain-id bleavit_local_drills \
  -t local \
  --relay-chain paseo-local \
  --para-id 4242 \
  --runtime "$wasm" \
  --properties "$properties" \
  --verify \
  patch "$drill_patch"
if [[ ! -s "$out/bleavit-drill.json" ]] || ! python3 -m json.tool "$out/bleavit-drill.json" >/dev/null; then
  rm -f "$out/bleavit-drill.json"
  echo "chain-spec-builder did not produce a valid Bleavit drill chain spec" >&2
  exit 1
fi

rm -f "$out/bleavit-drill-raw.json"
"$builder" --chain-spec-path "$out/bleavit-drill-raw.json" \
  convert-to-raw "$out/bleavit-drill.json"
if [[ ! -s "$out/bleavit-drill-raw.json" ]] || ! python3 -m json.tool "$out/bleavit-drill-raw.json" >/dev/null; then
  rm -f "$out/bleavit-drill-raw.json"
  echo "chain-spec-builder did not produce valid raw JSON for Chopsticks" >&2
  exit 1
fi
python3 "$repo_root/tools/deploy/validate-chain-spec.py" \
  --profile local "$out/bleavit-drill.json"

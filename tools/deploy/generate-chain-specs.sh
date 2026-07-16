#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
cd "$repo_root"

wasm="target/release/wbuild/bleavit-runtime/bleavit_runtime.compact.compressed.wasm"
builder="target/tools/bin/chain-spec-builder"
out="deploy/chain-specs/out"
properties="tokenSymbol=VIT,tokenDecimals=12,ss58Format=7777"

cargo build -p bleavit-runtime --release --features substrate-wasm-builder --locked

if [[ ! -x "$builder" ]]; then
  cargo install staging-chain-spec-builder --version 17.0.0 --locked --root target/tools
fi

mkdir -p "$out"

"$builder" --chain-spec-path "$out/bleavit-dev.json" create \
  --chain-name "Bleavit Development" \
  --chain-id bleavit_dev \
  -t development \
  --relay-chain paseo-local \
  --para-id 4242 \
  --runtime "$wasm" \
  --properties "$properties" \
  named-preset development

"$builder" --chain-spec-path "$out/bleavit-local.json" create \
  --chain-name "Bleavit Local" \
  --chain-id bleavit_local \
  -t local \
  --relay-chain paseo-local \
  --para-id 4242 \
  --runtime "$wasm" \
  --properties "$properties" \
  named-preset local_testnet

python3 tools/deploy/validate-chain-spec.py --profile dev "$out/bleavit-dev.json"
python3 tools/deploy/validate-chain-spec.py --profile local "$out/bleavit-local.json"

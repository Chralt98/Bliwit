# Bleavit chain-spec pipeline

Chain specs are generated from the runtime's committed genesis presets with the
pinned `staging-chain-spec-builder`:

```sh
cargo install staging-chain-spec-builder --version 17.0.0 --locked
cargo build -p bleavit-runtime --release --features substrate-wasm-builder
```

The pinned wasm builder names the resulting release artifact
`target/release/wbuild/bleavit-runtime/bleavit_runtime.compact.compressed.wasm`.
That basename was verified against `substrate-wasm-builder` 32.0.0's compact,
compressed output naming and the runtime crate name (`bleavit_runtime`).

For the reproducible dev and local outputs, run:

```sh
tools/deploy/generate-chain-specs.sh
```

The script invokes these pinned builder commands (after building the runtime):

```sh
target/tools/bin/chain-spec-builder --chain-spec-path deploy/chain-specs/out/bleavit-dev.json create \
  --chain-name "Bleavit Development" --chain-id bleavit_dev -t development \
  --relay-chain paseo-local --para-id 4242 \
  --runtime target/release/wbuild/bleavit-runtime/bleavit_runtime.compact.compressed.wasm \
  --properties tokenSymbol=VIT,tokenDecimals=12,ss58Format=7777 \
  named-preset development

target/tools/bin/chain-spec-builder --chain-spec-path deploy/chain-specs/out/bleavit-local.json create \
  --chain-name "Bleavit Local" --chain-id bleavit_local -t local \
  --relay-chain paseo-local --para-id 4242 \
  --runtime target/release/wbuild/bleavit-runtime/bleavit_runtime.compact.compressed.wasm \
  --properties tokenSymbol=VIT,tokenDecimals=12,ss58Format=7777 \
  named-preset local_testnet
```

They use chain IDs `bleavit_dev` and `bleavit_local`, relay `paseo-local`, test
para ID 4242, and the required 02 §8 properties:

```json
{"ss58Format":7777,"tokenDecimals":12,"tokenSymbol":"VIT"}
```

Generated files land in the gitignored `deploy/chain-specs/out/` directory and
are checked by `tools/deploy/validate-chain-spec.py`.

## Paseo and production procedure

For Paseo, use relay `paseo`; for production, use relay `polkadot`. In both
cases the para ID is the value assigned during onboarding, never fixture 4242.
Build an audited runtime genesis patch from
`deploy/genesis/allocations.template.json`, replacing every `TODO` with the
approved real allocation account, and pass it through the builder's `patch`
subcommand. Inject `bootNodes` from the matching
`bootnodes.paseo.json`/`bootnodes.polkadot.json` operator manifest, then validate
the finished artifact with `--profile paseo` or `--profile polkadot`.

02 §10 is a hard release gate: every Paseo and production spec MUST contain at
least 8 browser-reachable `/wss` multiaddrs across at least 4 independent
operators, including at least 2 endpoints on TCP port 443. The validator
enforces all three thresholds against the operator manifest. A spec update that
would fall below any threshold MUST NOT be released.

Artifact publication is milestone B8. Zombienet and Chopsticks environment
definitions are milestone B7. The ss58-registry submission artifact, which must
land before Phase 2, is in `deploy/ss58/`.

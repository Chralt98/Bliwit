# Backend release artifacts (B8)

This directory implements the backend half of the frozen chain/frontend release
contract. A tag build is strict: it can publish only when all five artifact legs
from [02 §11](../../docs/architecture/02-integration-contract.md) and
[15 §5](../../docs/architecture/15-invariants-and-testing.md) are complete and
the standing and supply-chain gates are green. A manually dispatched dry-run uses
`--allow-missing`, publishes nothing, and produces the same content-addressed
bundle plus an exact readiness report.

## Artifact set

The pipeline assembles:

1. `runtime.wasm`, raw SCALE `metadata.scale`, their SHA-256 hashes, runtime
   identity, and the normalized build recipe ([02 §11](../../docs/architecture/02-integration-contract.md),
   [15 §5(1)](../../docs/architecture/15-invariants-and-testing.md));
2. validated chain specs for every live environment ([15 §5(2)](../../docs/architecture/15-invariants-and-testing.md));
3. deterministic `.tar.xz` archives of the Chopsticks and Zombienet definitions
   used by the backend suites ([15 §4.7/§5(3)](../../docs/architecture/15-invariants-and-testing.md));
4. one normalized chainHead transcript per item in `surface-manifest.json`, plus
   the classic `state_getStorage` fallback for the frozen `ReleaseChannel` raw
   key ([02 §3/§6–§9/§12](../../docs/architecture/02-integration-contract.md));
5. the regenerated reference-model scenarios and V1–V6 corpus plus the full
   release-gated MPFR sweep ([04 §4/§5](../../docs/architecture/04-markets-and-pricing.md),
   [15 §4.4/§5(5)](../../docs/architecture/15-invariants-and-testing.md)).

`assemble-release.py` names every payload file `dist/<sha256>-<basename>` and
records `{kind, path, sha256, size, source}` in `release-manifest.json`. The
manifest and readiness report are published under both friendly names and
content-addressed names in the same `dist/` inventory. The MPFR shards are packed in a deterministic
`mpfr-sweep.tar.xz`; `sweep-manifest.json` is also published separately and
retains every per-shard SHA-256 for verification after decompression.

`tools/release/environments.json` is the reviewed live-environment inventory.
The assembler requires and validates a chain spec for every row with
`required: true` or `live: true`; a live row must also name its bootnode
manifest and pass that profile's existing deploy validator. Moving an
environment to `live: true` is a reviewed repository change, never a workflow
input.

Each packaged `zombienet/` and `chopsticks/` directory must contain
`run-evidence.json` with this contract (the evidence file itself is excluded
from the self-referential `artifact_hashes` map):

```json
{
  "schema": "bleavit.env-evidence.v1",
  "suite": "zombienet",
  "runtime_wasm_sha256": "<64 lowercase hex>",
  "artifact_hashes": {"relative/path": "<sha256>"},
  "suites_run": [{"name": "collator-loss", "result": "pass"}],
  "recorded_at_commit": "<release git commit>"
}
```

[`tools/env/run-evidence.py`](../env/run-evidence.py) is the producer for this
contract. The tag pipeline runs its release tier against the built artifacts,
records G1-tier exclusions explicitly, and inventories only the clean committed
environment definitions after generated state is removed. Evidence for a kind
is produced only when every release-tier suite of that kind was attempted and
passed: G1-tier exclusions are recorded, but a gated release-tier skip blocks
evidence. The producer self-checks with the assembler's validator, while
assembly remains the single release-blocking enforcement point.
Evidence emission is currently blocked fail-closed until the SQ-152 try-state
leg and the SQ-151 Chopsticks card-depth execution land; the producer still runs
the suites and writes run reports.

Pending SQ-139 ratification, the producer adds consumer-tolerated fields to the
minimal contract above: top-level `tier`, `suites_skipped`, `produced_by`,
`suites_manifest_sha256`, and `pins_env_sha256`, plus `duration_seconds` and
`checks` on each `suites_run` row. These extras identify the selected policy
tier and producer inputs, record exclusions, and describe the execution depth;
the authoritative consumer continues to validate the frozen fields shown in
the contract block.

The suite must match its directory. Every regular file other than the evidence
file must be listed, every hash must match, every suite result must be `pass`,
and both the runtime hash and commit must bind to this release. Invalid evidence
is a B7 gap in a dry-run and blocks strict assembly.

The GitHub tag release is first published as a **prerelease**, not as the
canonical release. Its manifest keeps
`"mirror":{"required":true,"status":"pending","evidence":null}`. After CI
has atomically uploaded and size-verified the complete draft, an operator:

1. mirrors every content-addressed file to Arweave;
2. attaches evidence shaped as a JSON mapping from each SHA-256 to its TXID,
   for example `{"ab…ff":"ArweaveTxId…"}`; and
3. promotes the release through the reviewed operator process only after the
   evidence covers the entire content-addressed set.

CI holds no Arweave, minisign, or ArNS controller keys. The workflow creates a
draft, uploads without replacement, compares remote asset names and sizes to
the local inventory, and only then makes the prerelease visible. A failed
upload remains a non-public draft.

## Build and metadata-hash posture

`build-runtime.sh` uses the toolchain pinned by `rust-toolchain.toml` and the
exact recipe:

```sh
cargo build -p bleavit-runtime --release --features substrate-wasm-builder --locked
```

It fixes `SOURCE_DATE_EPOCH` to the source commit time unless supplied, sets
`TZ=UTC`, the C UTF-8 locale, `CARGO_INCREMENTAL=0`, and disables Cargo color.
Those are the environment inputs that can otherwise leak time, locale, or
incremental state into the builder. `build-info.json` records them with the
toolchain, host triple, Cargo/rustc versions, source commit, and Wasm hash.

The honest claim is recipe reproducibility: same source and same pinned
toolchain should produce the same bytes. The host image is not yet pinned by
digest, so an independent two-container srtool-style byte-identity gate remains
follow-up work.

The runtime currently calls `WasmBuilder::build_using_defaults()` and has no
`metadata-hash` build feature. Therefore the release publishes SHA-256 of the
raw SCALE metadata and Wasm, but does not claim an RFC-78 merkleized metadata
hash. `runtime-info.json` records this explicitly. The transaction metadata-hash
extension being linked into the runtime is not itself evidence that build-time
RFC-78 hashing was enabled.

Metadata extraction also reads `:code` from the booted chain and refuses to
continue unless its SHA-256 equals the exact `--wasm` bytes. Assembly then
cross-checks that hash against `build-info.json`, `runtime-info.json`, the
shipped Wasm, and `genesis.runtimeGenesis.code` in every packaged chain spec.
Those mismatches are corruption and fail even an `--allow-missing` dry-run.

## Fixture recording

`extract-metadata.py` and `record-chainhead-fixtures.py` share `node_boot.py`.
Each command starts `bleavit-node` with the generated development spec, an
ephemeral base path, a free localhost RPC port, manual sealing through
`--dev-block-time`, and guaranteed terminate/kill cleanup on every exit path.

The recorder follows a finalized block with `chainHead_v1_follow` and drives
`chainHead_v1_storage`, `chainHead_v1_call`, and `chainHead_v1_header` as the
surface kind requires. Server-selected subscription and operation IDs become
stable `subscription-N` / `operation-N` labels; timestamps and unrelated live
block notifications are omitted; keys are sorted. The pinned concrete block
hash is retained once under `headers.pinned_block`, so a frontend harness can
substitute its own block explicitly. Fresh dev chains use the first executed
finalized block so `System.Events` has real SCALE bytes.

Every operation has a total 120-second deadline and the recording has a
configurable whole-run deadline (30 minutes by default), so unrelated follow
notifications cannot keep either loop alive. Event transcripts contain both a
chainHead storage operation and classic `state_getStorage` bytes for
`System.Events` at that pinned block. Every metadata-backed transcript records
the resolved layout; a manifest layout mismatch is a strict failure with both
forms in `fixtures-report.json`.

Layout expectations are frozen only for surface the current runtime actually
wires (they were validated against a live node). Blocked entries (A8/A11/B2)
carry no `layout` — a guessed rendering would false-alarm once the owning
milestone lands; the expectation is frozen from the real runtime at that
point. The `ForeignAssets` trio's Location-keyed expectation is live wired
surface: the runtime keys the instance by the frozen XCM `Location` (02 §8;
SQ-101 resolved 2026-07-17). Two renderer caveats are inherent to portable
metadata: const-generic
bounds (`BoundedVec<T, ConstU32<N>>`) do not appear in the registry — bounds
are certified through the paired metadata constants instead — and the 02 §12
`ReleaseChannel` key is deliberately metadata-independent, so its fixture is a
raw read validated by offset layout (schema byte, ≥ 168 bytes), never a
metadata presence check. Value-frozen identity rows (02 §8 USDC decimals and
`min_balance`, the chain-spec `properties` identity) are asserted from
exact-key reads and `system_properties`, not from prefix scans.

The websocket dependency is deliberately singular and exact:

```sh
python3 -m pip install websockets==15.0.1
```

If it is unavailable, `--allow-missing` degrades to classic-RPC-only recording,
still writes every transcript and `fixtures-report.json`, and marks chainHead
coverage missing. Strict mode fails; it never fabricates chainHead responses.

## Current fail-closed blockers

Strict mode is expected to fail today:

- B7 owns the per-release `run-evidence.json` for the `zombienet/` and
  `chopsticks/` environment definitions;
- B2 owns implementation of all 11 `FutarchyApi` methods and remaining metadata
  constants;
- A8 owns wiring `pallet-epoch` into the runtime;
- A11 owns wiring `pallet-execution-guard` into the runtime.

The `ForeignAssets` `u32`→`Location` re-key (SQ-101) cleared its three rows on
2026-07-17; the manifest's frozen Location-keyed expectations now record
against the live runtime.

These entries remain `required: true` in `surface-manifest.json`. Their
`blocked_by` fields are diagnostics, not waivers. A tagged workflow uses strict
mode and therefore cannot publish while any remain.

## Local dry-run

From the repository root:

```sh
python3 -m pip install websockets==15.0.1
tools/deploy/generate-chain-specs.sh
cargo build -p bleavit-node --release --locked
tools/release/build-runtime.sh release-work/runtime
tools/ci/supply-chain-gates.sh --summary-out release-work/supply-chain-summary.json
python3 tools/release/extract-metadata.py \
  --wasm release-work/runtime/runtime.wasm \
  --out-dir release-work/runtime
python3 tools/release/record-chainhead-fixtures.py \
  --metadata release-work/runtime/metadata.scale \
  --out-dir release-work/chainhead \
  --allow-missing
python3 tools/reference-model/generate-vectors.py --check
python3 tools/reference-model/generate-vectors.py \
  --sweep-out release-work/sweep \
  --sweep-points 10000 --sweep-shards 4
BLEAVIT_SWEEP_DIR=release-work/sweep \
  cargo test -p futarchy-fixed --release --locked --test sweep -- --ignored --nocapture
python3 tools/release/assemble-release.py \
  --output-dir release-work \
  --supply-chain-result passed \
  --supply-chain-summary release-work/supply-chain-summary.json \
  --allow-missing
```

The reduced sweep is useful only for development; the assembler deliberately
lists its `< 10⁷ points` as a B8 readiness gap. The tag workflow omits the size
overrides and the checker requires the full ≥10⁷-point corpus. A local
`--supply-chain-result passed` is an assertion that
`tools/ci/supply-chain-gates.sh` was run successfully; the workflow supplies it
only after that release-blocking job is green. Its summary discloses the exact
annotated RustSec ignores and counts allowed informational warnings in the root
and keeper workspaces; the assembler embeds it under `supply_chain.summary`.

Offline tooling checks are:

```sh
python3 -m unittest discover -s tools/release/tests
python3 -m py_compile tools/release/*.py tools/release/tests/*.py
bash -n tools/release/*.sh
```

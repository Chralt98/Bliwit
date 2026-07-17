<!-- B7 release artifact: 15 §1/§4.7/§5; 02 §11; 09 §3.2/§4/§7.1. -->
# Bleavit Zombienet environments

These committed topology, patch, drill, and helper definitions implement the
release-artifact obligation in [15 §4.7](../docs/architecture/15-invariants-and-testing.md)
and the Zombienet artifact row in [02 §11](../docs/architecture/02-integration-contract.md).
They are the definitions consumed by release and frontend e2e jobs, not private
fixtures.

> **15 §4.7:** “Zombienet relay + parachain topologies exercising epoch
> progression under collator loss, keeper loss, and relay finality stalls
> (dead-man test), plus the XCM reserve-transfer suites including failure,
> trap, and recovery.”

> **02 §11:** “Relay+para topology files + genesis overrides matching the
> release.”

## Reproduce

Run from the repository root:

```bash
tools/env/fetch-binaries.sh
tools/env/generate-relay-specs.sh
cargo build --release -p bleavit-node --locked
zombienet/bin/zombienet -p native spawn zombienet/networks/bleavit-local.toml
zombienet/bin/zombienet -p native test zombienet/drills/01-smoke.zndsl
```

The fetch script sources [`tools/env/pins.env`](../tools/env/pins.env), downloads
the linux-x86_64 relay worker quartet and Zombienet, and verifies every SDK
download against its release-published `.sha256` sidecar before installation;
the Zombienet release publishes no sidecars, so its binary is verified against
the `ZOMBIENET_SHA256` digest pinned in `pins.env`. Other hosts fail with a
pointer to build the pinned sources. The generator checks out the
Paseo chain-spec-generator tag in `target/env/`, verifies its immutable commit
pin, builds it with `--locked`, proves all three required local chain names are
accepted, and emits valid JSON specs. It then
reuses [`tools/deploy/generate-chain-specs.sh`](../tools/deploy/generate-chain-specs.sh)
for the Bleavit plain spec and converts that same spec to raw form for
Chopsticks.

[`genesis/drill-overrides.json`](genesis/drill-overrides.json) is a
chain-spec-builder patch, with its owning citations here because JSON has no
comment syntax. It repeats the release bootstrap `PhaseFlags = shadow |
sudo-present` and supplies seven deterministic dev guardians so the real 5-of-7
workflow is exercisable. It does **not** replace `Params`: the pallet seeds the
13 §1 registry from code, preventing a chain-spec parameter bypass.

No protocol timing is compressed: the seeded `epoch.length` is 302,400 blocks,
the expedited CODE descriptor lead is 43,200 blocks, playbook expiry remains
201,600 blocks, `dec.window` remains 43,200 blocks, and `mkt.obs_interval`
remains 10 blocks. Dead-man helpers poll relay best and finalized heads until the
real 4,800-block stalled-finality gap exists; they never substitute wall-clock
sleep for a block threshold.

## Run evidence

[`tools/env/run-evidence.py`](../tools/env/run-evidence.py) executes the suites
listed in [`tools/env/suites.json`](../tools/env/suites.json) against the built
release node, generated chain specs, and the exact release Wasm. After fetching
the pinned binaries, generating the specs, and building the node, a release-tier
Zombienet run is:

```bash
python3 tools/env/run-evidence.py \
  --kind zombienet \
  --tier release \
  --wasm release-work/runtime/runtime.wasm \
  --commit "$(git rev-parse HEAD)"
```

The four real-time long-horizon drills (`04`, `05`, `08`, and `09`) are G1-tier
and therefore recorded as tier exclusions in a release run. Evidence can be
emitted only by `--tier release`; `--tier g1`, `--tier all`, cherry-picked
`--suites`, and any custom `--zombienet-binary`, `--chopsticks-command`, or
`--node-binary` are report-only. Every release-tier drill of the emitted kind
must be attempted and pass, so a `gated_on` skip blocks evidence and
`--include-gated` is required while a release-tier gate remains. In evidence
mode, the default Zombienet binary is mandatory and its SHA-256 must match
`ZOMBIENET_SHA256` from `tools/env/pins.env`. A successful evidence-producing
run deletes downloaded binaries, generated specs, and prior evidence, then
hashes only the clean committed definitions (preserving the
generated-directory `.gitignore` files). The resulting `run-evidence.json`
follows the consumer contract in
[`tools/release/README.md`](../tools/release/README.md); release assembly remains
the final gate.

## Runnability

| Drill | Spec | Runnable-now assertions | Gated assertions |
|---|---|---|---|
| `01-smoke` | 15 §4.7; 02 §11 | Four relay validators and three collators start; para 4242 registers and produces included blocks | none |
| `02-collator-loss` | 15 §4.7; 09 §7.1 | Three collators boot; pre-fault/post-fault/recovery deltas and native SIGSTOP/SIGCONT steps are live definitions | First native-provider execution confirmation: G1 |
| `03-keeper-loss` | 15 §4.7; 09 §7.1 | Chain-liveness delta runs before the dependency gate | Keeper reference implementation landed (B9, PR #62); wiring its process as a `keeper` topology node is the open follow-up — until then the helper fails loudly `keeper node absent — gated on B9` |
| `04-dead-man` | 15 §4.7; 09 §7.1; 13 §2 | Native pause/resume and 4,800-relay-block RPC stall measurement | Freeze engagement/clear, queue, and clock assertions: A8/A11 wiring; first native run: G1 |
| `05-coretime-renewal-under-dead-man` | 09 §4/§7.1 | XCM topology, relay-block stall measurement, liveness deltas, and the renewal call's **exemption-reachability form**: the dispatch must traverse the staged freeze into treasury logic and fail exactly `RenewalWindowClosed` (09 §4 exemption proven; a filter/freeze rejection fails the drill). Quotes/`ops.coretime` funding are deliberately unstageable from a chain spec or signed call | Full **staged-renewal form** (success + `CoretimeRenewalCalled`, the 09 §7.1 Phase-1 exit form): B1a wiring of the B4 quote-noting seam + the class-origin `fund_budget_line` path; A8/A11 freeze wiring; first native run: G1 |
| `06-pb-migration` | 09 §3.2/§7.1 | B6 wires the migration controls and ExecutionGuard at slot 62; the forced-failure, retry, and guardian forward-rollback branches are executable | 09 §3.2 still carries a control-surface `[VERIFY]`; first native-provider execution must confirm the committed flow |
| `07-xcm-reserve-transfer` | 15 §4.7; 09 §6.1 | v5 inputs and decoded source/destination event correlation are complete; bleavit/1 fails before AH debit; the recovery `ClaimAsset` send is dispatched through the local preset's **sudo** so it carries Asset Hub's bare chain origin (a signed send would descend to the signer's account origin and could never match the AH-chain-keyed trap) | B4 executor/router/caps + Location-keyed USDC wiring; AH sudo presence in the generated local preset must be confirmed at first run |
| `08-expedited-code-under-freeze` | 09 §2.1/§3.2/§7.1 | B6 wires ExecutionGuard at slot 62; the receipt-aware authorize → early-reject → 43,200-block wait → apply definition targets that real surface | G1 setup must stage the attested queued proposal and active ledger freeze before the three-day lane runs |
| `09-three-unattended-epochs` | 15 §4.7; 09 §7.1; 13 §1 | Phase-1 G1 long-soak targets 907,200 blocks (3 × 302,400); not a CI test | `EpochOf.index` assertion: A8 runtime wiring |

NOTE(B7): the pinned `NativeClient` implements `pause`/`resume` with
`SIGSTOP`/`SIGCONT`. Upstream DSL documentation still describes those commands
as Podman/Kubernetes-only, so the committed native definitions are correct but
their first end-to-end confirmation remains a G1 gate.

NOTE(B7): the current runtime still reserves but does not instantiate `Epoch`;
B6 now instantiates `ExecutionGuard` at slot 62. Guardian trigger and coretime
XCM seams that remain unwired are fail-closed. Helpers intentionally fail with
precise messages when required state or metadata surfaces are absent; supported
boot/liveness assertions remain live.

## Mandatory closing try-state

The evidence runner does not yet execute the mandatory closing try-state.
Evidence emission is blocked (15 §1; SQ-152) until the leg lands: the runner
refuses to emit rather than emitting without it. With the collator RPC printed
by Zombienet (replace the port if allocated differently), run it manually:

```bash
try-runtime \
  --runtime target/release/wbuild/bleavit-runtime/bleavit_runtime.compact.compressed.wasm \
  on-runtime-upgrade --checks try-state --blocktime 6000 \
  live --uri ws://127.0.0.1:9944
```

Build that Wasm with the runtime's `try-runtime` feature before executing the
command. The same closing command applies to every drill, including smoke.

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
(cd keeper && cargo build --release --locked -p bleavit-keeper)
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

No protocol timing is compressed in the **release** drill spec (`bleavit-drill.json`):
the seeded `epoch.length` is 302,400 blocks, the expedited CODE descriptor lead is
43,200 blocks, playbook expiry remains 201,600 blocks, `dec.window` remains 43,200
blocks, and `mkt.obs_interval` remains 10 blocks. The default-off **`fast-timing`**
test spec (`bleavit-drill-fast.json`, SQ-128) — booted by drills `04`, `08`, and `09`
— compresses only the epoch clock plus drill-04's `DEAD_MAN_RELAY_BLOCKS` (4,800→48)
and drill-08's descriptor lead (43,200→12), all off one `kernel::FAST_DAY_BLOCKS`
knob; the release runtime stays byte-frozen (guard test
`production_epoch_timing_floors_are_frozen`). Dead-man helpers poll relay best and
finalized heads until the real stalled-finality gap (4,800 blocks release / 48 fast)
exists; they never substitute wall-clock sleep for a block threshold.

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

The G1-tier drills — `05` (a real-time ~8 h dead-man run) plus `04`, `08`, and `09`
(now default-off `fast-timing` compressed-runtime proofs, SQ-128, replacing the
~16 h / ~3 day / ~63 day release-cadence soaks) — are recorded as tier
exclusions in a release run. Evidence can be
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
| `02-collator-loss` | 15 §4.7; 09 §7.1 | **Passed end-to-end 2026-07-18 (G1)**: pre-fault/post-fault/recovery height deltas over real native SIGSTOP/SIGCONT (V-24 confirmed) | none |
| `03-keeper-loss` | 15 §4.7; 09 §7.1 | **Passed end-to-end 2026-07-18 (G1)**: the real live-mode B9 daemon runs as Zombienet's `keeper` node (exec wrapper → keeper PID receives SIGSTOP/SIGCONT) and survives the pause/resume window; drill-asserted: pause/resume, `is up`, post-resume `bleavit_keeper_connected ≥ 1`, and parachain-height deltas proving collator liveness. Log-verified (not drill-asserted): role detection from live metadata | none |
| `04-dead-man` | 15 §4.7; 09 §7.1; 13 §2 | Native pause/resume, relay-block RPC stall measurement (48 fast-timing / 4,800 release), and the freeze-assertion surfaces (`epoch.deadMan`, `executionGuard.deadManFreeze`, phase-flag bit 6) — the A8/A11 wiring landed, so the helper's metadata gates pass | **First execution 2026-07-20** (fast-timing, SQ-128 extended: `DEAD_MAN_RELAY_BLOCKS` 4,800→48): the stall compressed ~16 h→~8 min (reached gap 48 in ~8 min — **timing proven**), but `assert-dead-man engaged` **failed (16/17)** — the parachain kept producing (liveness `pre-fault,2` passed), so `observe_dead_man` saw no relay-**parent** jump. Root cause **SQ-282**: the B10 detector fires on a relay-parent jump (outage/catch-up), not a relay-**finality** stall (open 05 §4.8 `F`-signal `[VERIFY]`). Re-gated on SQ-282; timing is no longer the blocker |
| `05-coretime-renewal-under-dead-man` | 09 §4/§7.1 | XCM topology, relay-block stall measurement, liveness deltas, and (2026-07-20) the drill-side **staged-renewal exit-form** code: `stage`/`execute`/`probe` branches that note a live authenticated quote as the genesis authority (Alice) and assert `ExtrinsicSuccess` + `CoretimeRenewalCalled` + quote consumption; the retained `probe` branch proves the **exemption-reachability form** (the dispatch traverses the staged freeze into treasury logic and fails exactly `RenewalWindowClosed`; a filter/freeze rejection fails the drill) | **genesis seeding + long-horizon run** (PLAN SQ-245/SQ-246 ruled; B12 surface landed): the funded `ops.coretime` budget line and treasury DOT/USDC custody are not chain-spec-fundable — `fund_budget_line`/`set_coretime_authority` need the unreachable `FutarchyTreasury` origin (sudo→Root rejected, `sudo_as` denied) — so they must be genesis-seeded (raw `futarchyTreasury.State` line + `foreignAssets.accounts`); the exit form then runs under a real 4,800-relay-block dead-man stall (~8 h, g1-tier). Dead-man **engagement** landed with B10 (see drill 04) |
| `06-pb-migration` | 09 §3.2/§7.1 | **Passed end-to-end 2026-07-20 (G1, 3/3)** on a dedicated plain migration topology: blocks to height ≥ 10; `assert-halt` verifies the genesis-staged `MigrationHalt = true` (the SQ-274 trigger, seeded via the new `pallet-execution-guard` `migration_halt` genesis field, with a storage-key self-check); `rollback` runs the guardian 5-of-7 recovery (`propose_action` + four `approve_action`s, membership decoded from `[Option<AccountId>; 7]`) and the dispatching approval **correctly fails closed** — the Migration playbook has no EmergencyPlaybook-safe runtime call (R-7: retrying a stuck cursor needs Root-only controls; the runtime `playbook_calls(Migration)` returns `Other(…)`), so the trigger being ACTIVE yields `DispatchError::Other`, distinct from the pre-SQ-274 `TriggerInactive` (trigger never engaged) and from a successful activation (none exists) | none — SQ-274 resolved (see PLAN + Decision log): the drill stages the guard trigger at genesis via the `migration_halt` genesis field (a PLAIN spec the pinned zombienet can schedule; a raw storage-injected spec is not), with no production fault-injection surface (R-7) |
| `07-xcm-reserve-transfer` | 15 §4.7; 09 §6.1 | v5 inputs and decoded source/destination event correlation are complete; readiness is a genuine **capability probe** on the genesis-registered canonical Location-keyed USDC (SQ-101), not a version sentinel; the recovery `ClaimAsset` send is dispatched through the local preset's **sudo** so it carries Asset Hub's bare chain origin (a signed send would descend to the signer's account origin and could never match the AH-chain-keyed trap) | **Asset Hub block production** (first real run, 2026-07-20): the stale `spec_version === 1` sentinel is **retired** (WP-1) — the B10 XCM composition (`AssetTransactor`, `Barrier`, `IsReserve`, `IsTeleporter`, `OriginConverter`, sender) is present exactly when the canonical USDC is registered, which the helper now probes, and the Bleavit side boots — but the first end-to-end execution found the pinned Paseo `asset-hub-paseo-runtime` **trapping on its parachain-system inherent** (`AbortedDueToTrap` in `cumulus_pallet_parachain_system`), so Asset Hub (para 1000) never produces blocks and the drill cannot reach its XCM legs. This is a Paseo chain-spec-generator/system-runtime integration issue (not Bleavit); owned by the env pins |
| `08-expedited-code-under-freeze` | 09 §2.1/§3.2/§7.1 | B6 wires ExecutionGuard at slot 62; the receipt-aware authorize → early-reject → lead-time wait → apply definition targets that real surface, reading the **live** `descriptorLeadTime` metadata constant (12 fast-timing / 43,200 release) rather than a hardcode | **Fast-timing (SQ-128 extended, 2026-07-20)** compresses the D-14 lead 43,200→12, removing the ~3-day wait; G1 setup must still stage the attested queued expedited proposal and active ledger freeze at genesis (**SQ-233**) before the lane runs — timing is no longer the blocker |
| `09-three-unattended-epochs` | 15 §4.7; 09 §7.1; 13 §1 | **Passed end-to-end 2026-07-20 (G1, 6/6)** against a default-off `fast-timing` compressed-epoch test runtime: three real 84-block epochs (`epoch.length = 21 × FAST_DAY(4)`; SQ-128 implemented) advanced unattended in ~29 min — para height 84→168→252 at ~7.4 s/block with the topology `keeper` node cranking `Epoch.tick` and finalizing each tick; `assert-epoch-progression.js` reads the LIVE `epoch.schedule().length` (cadence-agnostic, asserts identically vs the release runtime) and requires `epoch.epochOf().index ≥ 3`. Boots `zombienet/networks/bleavit-fast.toml` (para spec `bleavit-drill-fast.json`, wasm built into a separate `target/fast-timing` dir); not a CI test | none — SQ-128 implemented (see PLAN + Decision log): compresses only the epoch clock (two kernel floors + three registry seeds off one `kernel::FAST_DAY_BLOCKS` knob), release runtime byte-frozen (guard test `production_epoch_timing_floors_are_frozen`); the release-cadence "3 unattended epochs" is subsumed by G2's ≥6-epoch Paseo run |

NOTE(G1): drill-authoring constraints established by the first real `zndsl`
executions (2026-07-17). The pinned Zombienet grammar (version:
`tools/env/pins.env`) accepts **no `#` comment lines** (drill rationale therefore lives in this README, spec
citations in the `Description:` line) and **requires a `Creds: config` line**
after `Network:`. All Zombienet timeouts — the toml `[settings] timeout` and
every DSL `within N seconds` — are converted to 32-bit signed milliseconds, so
values above 2,147,483 s silently overflow (the original 6,000,000 s network
timeout collapsed to a 1 ms watchdog); long waits must be split into
sequential ≤ 2,000,000 s steps as drill 09 does.

NOTE(B7, confirmed G1): the pinned `NativeClient` implements `pause`/`resume`
with `SIGSTOP`/`SIGCONT`. Upstream DSL documentation still describes those
commands as Podman/Kubernetes-only; the first end-to-end native executions
(drills 02/03, 2026-07-18) confirmed they work against real node and keeper
PIDs (V-24).

NOTE(B7, updated G1 + post-B10/B12 merges): `Epoch` (61) and `ExecutionGuard`
(62) are instantiated in the runtime, B10 landed the dead-man detector and
the production XCM composition, and B12 landed the coretime renewal surface
(authority quote-noting, `RenewalDispatch`). **G1 (2026-07-20): drills 01/02/03
and 06 pass against the merged runtime** — SQ-274 resolved, so drill 06 stages
the PB-MIGRATION trigger at genesis (the `migration_halt` guard genesis field)
and exercises the guardian recovery to its correct fail-closed refusal. The
remaining drill-blocking items are **drill 04's dead-man detector/scenario
mismatch** (SQ-282 — its timing is proven, ~16 h→~8 min), **drill 08's
freeze/expedited genesis staging** (SQ-233 — its ~3-day D-14 lead is now
compressed to 12 blocks), drill 05's genesis seeding + ~8 h dead-man run
(long-horizon), and drill 07's **Asset Hub topology** (its `spec_version`
sentinel was retired, but the pinned Paseo `asset-hub-paseo-runtime` traps on
its parachain-system inherent). Helpers intentionally fail with precise
messages when required state or metadata surfaces are absent;
supported boot/liveness assertions remain live.

## Mandatory closing try-state

The evidence runner does not yet execute the mandatory closing try-state.
Evidence emission is blocked (15 §1; SQ-204) until the leg lands: the runner
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

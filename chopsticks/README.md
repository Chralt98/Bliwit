<!-- B7 release artifact: 15 §1/§4.7/§5; 02 §11; 06 §6.2; 09 §3.2/§4. -->
# Bleavit Chopsticks environments

These fork-from-generated-genesis configs and manufactured-state cards are the
published Chopsticks environment required by [15 §4.7](../docs/architecture/15-invariants-and-testing.md)
and [02 §11](../docs/architecture/02-integration-contract.md). They contain no
mutable/private upstream endpoint: every scenario starts from the raw chain
spec generated into `zombienet/specs/out/`.

This is the explicit pre-launch form of 02 §11's “fixture state snapshots”:
fork the generated genesis of the release runtime, then apply the checked-in
fixture injections. A live chain does not exist yet. Live-endpoint fork sources
land at Phase ≥ 2 with B8's content-addressed publication pipeline.

## Run

From the repository root, generate specs, source the only pin home, and select
either the base or a scenario:

```bash
tools/env/generate-relay-specs.sh
source tools/env/pins.env
npx "@acala-network/chopsticks@${CHOPSTICKS_VERSION}" --config chopsticks/bleavit.yml
npx "@acala-network/chopsticks@${CHOPSTICKS_VERSION}" --config chopsticks/scenarios/void-epoch.yml
```

Chopsticks at the pinned release requires Node.js **22 or newer**.

Each scenario duplicates the local-only base keys because Chopsticks has no
cross-file include directive, then layers `import-storage` over the same raw
genesis. Raw keys are documented adjacent to their values and are derived from
the real runtime/pallet prefixes (`twox128(module) ++ twox128(item)`, plus the
declared map hashers). Runtime-only PhaseFlags bits 5–7 are injected only in
manufactured drill state under 02 §11's “manufactured precondition failures”
artifact row. Section 02 §7.3 only assigns the bits; 06 §3.3 forbids exposing
their writers as dispatchable calls.

## Run evidence

[`tools/env/run-evidence.py`](../tools/env/run-evidence.py) starts each committed
YAML through the Chopsticks version sourced from
[`tools/env/pins.env`](../tools/env/pins.env). A runner pass currently attests
that the scenario booted as a fork from the release-Wasm-bound raw spec, every
injected storage cell was verified byte-exactly over localhost JSON-RPC, two
blocks were produced with an advancing header, and the live `:code` bytes
remained bound to the release Wasm before and after those blocks. After
generating the chain specs and building the release Wasm, run:

```bash
python3 tools/env/run-evidence.py \
  --kind chopsticks \
  --tier release \
  --wasm release-work/runtime/runtime.wasm \
  --commit "$(git rev-parse HEAD)"
```

The `base` row remains ungated because it attests only boot, injected-state,
blocks, and live `:code` binding. All 10 scenario rows are evidence-gated on
SQ-151 card-depth execution: the runner skips them by default and blocks
evidence while they remain gated. Evidence emission is additionally blocked
until the mandatory closing try-state leg lands (15 §1; SQ-152). These are the
connector-review P1 fail-closed enforcements for obligations previously deferred
by documentation. Before each scenario, the runner removes its exact database
and SQLite sidecars so an interrupted prior run cannot contaminate the
generated-genesis fork. It removes generated state again before evidence is
emitted, deletes any prior evidence, requires clean committed evidence inputs,
and hashes every committed regular file while rejecting symlinks. Cherry-picked
`--suites` runs are report-only. The `run-evidence.json` schema and byte-exact
inventory contract are documented in
[`tools/release/README.md`](../tools/release/README.md); release assembly remains
the final gate.

## Scenario map and runnability

| Scenario | Spec | Runnable now | Gated assertions |
|---|---|---|---|
| `upgrade-transition` | 15 §4.7; 02 §11; 09 §2.1 | Paired pending/checkpoint/history image is try-state coherent; B6 wires the authorize/apply and descriptor-transition surface | A8 enqueue/epoch-handoff wiring for a fully attested queued CODE proposal; frontend transition assertions wait Track F |
| `stale-queue` | 15 §4.7; 02 §11; 09 §1.2(3) | Genuine bleavit/2-vs-bleavit/1 mismatch, future grace, false expedited marker; B6 wires `reject_stale` | A8 wiring to manufacture proposal 1 in `Queued` |
| `void-epoch` | 15 I-26/I-27/§4.7 | Voided vault and real redeem inputs on the current ledger | frontend rendering waits Track F |
| `precondition-failures` | 15 §4.7; 02 §11; 09 §1.2 | Constitution flag injection; B6 wires the live guard refusal surface | A8 wiring to enqueue the otherwise-valid payloads used by the matrix |
| `pb-depeg` | 06 §6.2 | Accountable Guardian post-entry image; block expiry maintenance runs | downstream `market.freeze_creation`/effect-revert surface |
| `pb-migration` | 06 §6.2; 09 §3.2 | Accountable Guardian image + coherent paired guard checkpoint; B6 wires migration controls and the halt bridge | SQ-104 migration-control origin bridge; SQ-127 checkpoint lifetime and SQ-132 try-state/halt coupling remain open |
| `pb-oracle-void` | 06 §6.2 | Accountable Guardian post-entry image; block expiry maintenance runs | A8 cohort/ResolveAuthority wiring and downstream effect |
| `pb-halt-intake` | 06 §6.2 | Accountable Guardian image + manufactured machinery bit | A8 wiring and missing intake-pause/effect-revert surface |
| `pb-reserve` | 06 §6.2 | Accountable Guardian image + manufactured reserve bit | missing split-pause/effect-revert call and trigger adapter |
| `pb-ledger-freeze` | 06 §6.2/§6.3; 09 §3.1/§4 | Accountable Guardian/phase image; block expiry maintenance runs | I-4 trigger adapter, missing freeze/effect-revert calls, renewal XCM dispatcher |

The adjacent Markdown card is the normative execution/assertion sequence for
each YAML. NOTE(B7): `Epoch` exists as a production pallet but is not
instantiated in the current runtime; B6 now instantiates `ExecutionGuard` at
slot 62. Several 06 §6.2 call names still have no implemented
extrinsic/storage surface. Gated cards remain complete release definitions and
intentionally fail rather than silently weakening an assertion.

The Guardian pallet is instantiated, and every `pb-*` image injects its real
seven-member council, arithmetic bond ledger, activation review, active expiry,
allowance struct, and cursors. The pallet has no per-playbook allowance field,
registered-playbook/active-hold storage, or downstream effect-revert storage;
cards call those gaps out instead of inventing cells. Review scheduling is a
runtime stub and guardian `CurrentEpoch` stays zero until A8 wiring, while
block-number expiry now runs because `Guardian::load()` can succeed.

## Mandatory closing try-state

The evidence runner does not execute this check. For every scenario, after
repairing any deliberately manufactured invalid precondition, manually run the
release-blocking 15 §1 check against the local endpoint:

```bash
try-runtime \
  --runtime target/release/wbuild/bleavit-runtime/bleavit_runtime.compact.compressed.wasm \
  on-runtime-upgrade --checks try-state --blocktime 6000 \
  live --uri ws://127.0.0.1:8000
```

Use the scenario's configured port instead of `8000`. Upgrade scenarios point
`--runtime` at the candidate Wasm; all others use the release Wasm built with
the `try-runtime` feature. Any failure blocks release.

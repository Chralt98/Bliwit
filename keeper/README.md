# Bleavit keeper

`bleavit-keeper` is the untrusted, permissionless off-chain crank service from
[`01-system-overview.md` §4.2](../docs/architecture/01-system-overview.md). It watches finalized
Bleavit blocks, derives an honest snapshot of actionable work, and submits the signed calls that
keep epochs, observations, decisions, settlement, execution, data rounds, cleanup, renewal, and
welfare recording moving.

The keeper uses Subxt's dynamic API only. It downloads live metadata at connection time, reads
storage as dynamic SCALE values, and constructs calls from live pallet/call names. It has no
generated runtime bindings or bundled metadata. A role whose required pallet or call is absent is
disabled with one structured startup log line. This is expected today for the reserved `Epoch` and
`ExecutionGuard` runtime slots.

The service is not trusted by the protocol. Its planner is deterministic, calls are permissionless
and idempotent on-chain, and another keeper winning a state race is normal. Such extrinsic failures
are logged at `debug`; the next finalized snapshot ordinarily removes the work. Transport failures
are warned and trigger ordered endpoint failover. Priority-ordered transactions are submitted to
the pool with consecutive local nonces before their finality subscriptions are awaited concurrently,
so observation throughput is not limited to one crank per finalized block.

## Run against a development node

From the repository root, generate the development chain spec and start the repository's branded
omni-node. These commands build the root workspace and are intentionally separate from the keeper
workspace:

```sh
tools/deploy/generate-chain-specs.sh
cargo build -p bleavit-node --release --locked
target/release/bleavit-node \
  --chain deploy/chain-specs/out/bleavit-dev.json \
  --dev-block-time 3000 \
  --tmp \
  --rpc-port 9944
```

In another terminal, run the keeper from its own workspace:

```sh
cd keeper
cargo run --locked -p bleavit-keeper -- \
  --node-url ws://127.0.0.1:9944 \
  --signer-uri //Alice \
  --metrics-bind 127.0.0.1:9616
```

`//Alice` is for local development only. In an operator environment, pass an appropriately funded
keeper account via `--signer-uri` or put one Substrate secret URI in a permission-restricted file
and use `--signer-file`. The file is read as text; it is not an encrypted keystore container.
Live mode refuses to start unless one of those signer flags (or its config-file equivalent) is
explicitly set. Use `--dry-run` to extract, plan, and log without loading a signer or submitting anything. Run
`cargo run -p bleavit-keeper -- --help` for all CLI options.

## Configuration

CLI options override an optional TOML file:

```toml
node_urls = ["wss://rpc-a.example/wss", "wss://rpc-b.example/wss"]
signer_file = "/run/secrets/bleavit-keeper.suri"
enabled_roles = [
  "tick",
  "observe",
  "decide",
  "settle",
  "execute",
  "oracle-close",
  "registry-close",
  "cleanup",
  "renewal",
  "welfare",
]
# Optional live-Params overrides; omit them in ordinary production operation.
# obs_interval = 10
# decision_window = 43200
# reserve_probe_interval = 14400
# reserve_probe_timeout = 600
dry_run = false
metrics_bind = "127.0.0.1:9616"
every_n_blocks = 1
startup_jitter_secs = 20
cooldown_depth = 3
tx_timeout_secs = 90
max_retries = 2
retry_base_ms = 500
```

Start it with `cargo run --locked -p bleavit-keeper -- --config keeper.toml`. Node URLs are tried in
the listed order. Observation, decision-window, and reserve-probe timing uses the precedence
explicit CLI/TOML override → live `Constitution.Params` row → documented fallback. The dynamic
reads use the canonical 16-byte keys `mkt.obs_interval`, `dec.window`, `res.probe_int`, and
`res.probe_to`; their fallbacks are 10, 43,200, 14,400, and 600 blocks respectively. The cooldown
ledger suppresses an accepted call with the same role, pallet, call, and arguments for the
configured finalized-block depth. Tick planning reads `Epoch.TickBatch` from live metadata and
falls back to 10 only when that constant is unavailable or invalid; these numeric copies are
compatibility defaults, not alternate parameter homes.

The exact role names used by configuration, logs, and metric labels are `tick`, `observe`,
`decide`, `settle`, `execute`, `oracle-close`, `registry-close`, `cleanup`, `renewal`, and
`welfare`. All are enabled by default, subject to live-metadata capability detection.

Some roles are deliberately conservative. `record_snapshot` is submitted only when the active
welfare specification and a missing completed-epoch snapshot are directly visible. For every live
cohort, the extractor also follows its frozen `CohortSchedules` metric specification and catches up
missing `(cohort epoch + 1)` and `(cohort epoch + 2)` snapshots, including across later spec
activations. Daily gate planning reads the pallet-internal `Welfare.SampledGateDays` marker and
fills unsampled days across the bounded welfare lookback. The marker is separate from the frozen
`GateBreachFlags` surface, whose bitmap continues to identify breach days only. Older runtimes
without `SampledGateDays` retain honest degradation: the subtask emits its one `not yet plannable`
startup line and plans nothing. The keeper reads `Welfare.MaxGateFlags` and
`Welfare.MaxDailyGateSamples` from live metadata, with compatibility fallbacks of 20 epochs and 64
day indices matching welfare-core. Internal-only pruning, upgrade-proof recomputation, and other
calls whose arguments cannot be proved from storage are likewise never guessed. Zero-filing
registry epochs are deliberately unclosable on-chain under the
A6 dual-review ruling: `close_epoch` requires a live `FilingCount` entry, preventing a
reaped/never-filed epoch from being (re-)closed to the favorable `no filings => 1` aggregate.
Welfare instead applies its pull-side `no record => 1` default, so the keeper never plans these
epochs.

## Concurrent operation and economics

Production assumes at least two independent funded operator instances plus permissionless public
keepers. Use different signer accounts, RPC providers, process supervisors, and failure domains.
A small `startup_jitter_secs` reduces synchronized startup bursts; cooldown reduces repeat traffic,
but neither creates coordination or a leader. Do not alert on ordinary race-loss debug logs.

Per [`08-treasury-and-economics.md` §6](../docs/architecture/08-treasury-and-economics.md), the
on-chain keeper meter is 12,000 USDC per epoch. At least 80% is reserved for decision-critical
cranks and at most 20% is available for general work. At exhaustion, rebates stop but
permissionless/idempotent cranking does not.
Continuing through exhaustion is part of the funded operator commitment, backed by the
`ops.keepers` line—not a reason to stop the daemon.

## Metrics and alerts

When `metrics_bind` is set, Prometheus text is served at `/metrics` (and `/`). The endpoint exports:

- `bleavit_keeper_planned_total{role=...}`
- `bleavit_keeper_submitted_total{role=...}`
- `bleavit_keeper_succeeded_total{role=...}`
- `bleavit_keeper_failed_total{role=...}`
- `bleavit_keeper_last_successful_crank_timestamp_seconds{role=...}`
- `bleavit_keeper_stale_decision_window_books{role=...}`
- `bleavit_keeper_connected`
- `bleavit_keeper_current_block`
- `bleavit_process_start_time_seconds` (Zombienet-compatible process liveness)

These daemon rows pair with the chain-side monitoring rows in
[`12-release-and-operations.md` §6.3](../docs/architecture/12-release-and-operations.md): epoch
progress/tick lag, TWAP coverage and staleness, keeper activity, and meter utilization. A basic
RB-KEEPER inactivity expression is:

```promql
(time() - bleavit_keeper_last_successful_crank_timestamp_seconds{role="observe"}) > 3600
and on() bleavit_keeper_connected == 1
```

Instantiate that expression per required role in production so activity in a cleanup role cannot
mask an hour without a decision-critical crank. Disabled roles retain a zero timestamp and should
not be included in that deployment's alert rules; use an Alertmanager `for: 1h` guard so a fresh
process with no successful crank does not page immediately. Also wire the on-chain
`KeeperBudgetLow` (>80%) and `KeeperBudgetExhausted` events to RB-KEEPER, and alert on
finalized-head lag independently of the keeper process's `connected` gauge.

## Keeper-local quality gates

Run these only from this directory; this is a separate Cargo workspace with its own lock file:

```sh
cd keeper
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

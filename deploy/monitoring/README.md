# Bleavit monitoring and alerting

This directory is the O5 reference deployment for the infrastructure-only
monitoring commitments in architecture 12 §5.2 and §6.3. It contains
Prometheus scrape/rule configuration, Alertmanager routing, and an example
configuration for the controller-disjoint out-of-band attestation monitor.

## Run the stack

Replace every `*.example.invalid` target and webhook while rendering the files
into the operator's secret/configuration system. Webhook credentials must not
be committed. Then start the two Python exporters (Python 3.12; live WebSocket
operation additionally needs the repository pin `websockets==15.0.1`):

```sh
python3 tools/monitoring/chain_alerts_exporter.py \
  --url wss://YOUR_FINALIZED_NODE --bind 127.0.0.1:9617 --interval 30
python3 tools/monitoring/attestation_monitor.py \
  --config /etc/bleavit/attestation-monitor.toml
```

Use `--once` for drills. The chain exporter prints one Prometheus scrape and
returns 0/2 (success/operational failure). The attestation monitor returns 0
for a verified release, 1 for an integrity mismatch, and 2 for configuration,
transport, or decode failure. Its loop checks on every observed finalized
`ReleaseChannel` change and at the configured interval; validation rejects an
interval above 3,600 seconds. Between full checks it resolves the configured
ArNS name through every gateway on each finalized head, so a repoint triggers a
full check and the 600-block channel-update lag advances continuously. Because
§6.3 does not identify an authoritative ANT-history block height, that lag
starts at the first finalized head where a strict gateway majority differs from
`ReleaseChannel`; the metric and alert use that observed height explicitly.

Point Prometheus at [prometheus.yml](prometheus/prometheus.yml), mount
[bleavit-alerts.yml](prometheus/rules/bleavit-alerts.yml) under
`/etc/prometheus/rules`, and start Alertmanager with
[alertmanager.yml](alertmanager/alertmanager.yml). For example, with operator
images/binaries already installed:

```sh
prometheus --config.file=/etc/prometheus/prometheus.yml
alertmanager --config.file=/etc/alertmanager/alertmanager.yml
```

Before deployment and after any rule/spec edit, run:

```sh
python3 -m unittest discover -s tools/monitoring/tests
python3 tools/monitoring/check_alert_coverage.py
```

## Series and source inventory

`tools/monitoring/series-inventory.toml` is the checked source map for every
metric used by an alert expression. `check_alert_coverage.py` strictly extracts
both 12 §6.3 tables, checks each domain/runbook/page binding, checks every rule
metric against that inventory, and introspects both Python modules' `SERIES`
registries. It prints all seams on every run.

The chain exporter serves `bleavit_chain_*` series in these families:

- exporter health/finality: connection, finalized height, last successful
  scrape, and decode/transport error counter;
- `FutarchyApi` views: epoch phase/boundary/tick lag, proposal state counts,
  execution queue depth, open oracle round count/depth, welfare/reserve state,
  treasury NAV and rolling-meter use;
- live limits: `keeper.budget` from `FutarchyApi::params`,
  `DescriptorLeadTime` and queue/map bounds from portable metadata constants;
- metadata-driven storage: keeper-meter use, bounded map counts, and
  `PolkadotXcm.AssetTraps` occupancy;
- finalized events: Guardian actions, upgrade authorization/application, and
  keeper-budget-low counters;
- frozen `ReleaseChannel`: version labels, manifest TXID, spec version,
  update/pending heights, keyring generation, revocation mask, raw/individual
  flags, pending-upgrade age, and SECURITY-flip counter.

The attestation monitor serves `bleavit_release_monitor_*` health/check/error,
integrity, byte-mismatch, resolver-divergence, signature/attestation,
generation, channel-match, spec-coverage, repoint-lag, ANT-change, and webhook
failure series. Existing keeper series are documented in `keeper/README.md`;
the O5 inactivity rule uses the keeper's connected gauge and applies only to
roles whose per-process `planned_total` has advanced. This excludes disabled
roles while still alerting when an enabled role has planned work but has never
succeeded. After a keeper restart, a never-cranked role is necessarily
unobservable until it first plans work; production rules MUST still be
instantiated per required role so activity in one role cannot mask another.
Collator/node exporters are scraped as substrate exporters and remain
operator-version-specific; no rule assumes a non-frozen node metric name.

## Self-monitoring

Operators MUST use standard Prometheus meta-monitoring to alert when
`bleavit_chain_scrape_errors_total` increases and when either O5 daemon's target
is absent or has `up == 0` (`bleavit-chain-alerts` and
`bleavit-attestation-monitor`). Domain decode failures deliberately remove only
the affected chain-exporter families rather than publishing healthy-looking
zeros, so operators SHOULD also alert on absent required series. These are
operator-supplied exporter-health checks, not 12 §6.3 rows, and therefore are
not part of `check_alert_coverage.py`.

## 12 §6.3 row map

| Domain | Alert source | Status |
|---|---|---|
| Epoch progress | chain exporter (`epoch_status`) | live |
| Proposal state | chain exporter (`proposal_summaries`, `Epoch.IntakeProposals` occupancy/bound) | live |
| Markets | runtime-side book P&L + `b·ln2` | seam — B10 |
| TWAP | live unsealed-window coverage projection | seam — B10; `decision_stats` is sealed-window only |
| Liquidity floors | runtime-side effective POL/floor | seam — B10 |
| Oracle | chain exporter (`open_oracle_rounds`) | live |
| Collateralization | runtime-side escrow/custody reconciliation | seam — B10; page |
| Treasury | chain exporter (`nav`) | live |
| XCM | chain exporter trap count; node/runtime send/fail detail remains operational context | live alert threshold |
| Keepers | existing keeper daemon series | live |
| Guardian | chain exporter finalized events | live |
| Upgrades | canonical runtime migration-stall detector | seam — B10 |
| Storage | chain prefix counts + metadata bounds; remaining maps/PoV from runtime | partial live + B10 seam |
| Numerics | runtime LMSR rejection/dust anomaly detector | seam — B10 |
| Bootnodes | browser-context dial/certificate probes | seam — O3 |
| Served-state window | per-operator retention probe | seam — O3 |
| Release integrity | attestation monitor | live; page + status + community |
| Descriptor lead time | chain exporter + attestation monitor | live |
| ReleaseChannel | chain exporter + attestation monitor | live |
| Keeper budget | chain exporter live Params + metadata-decoded keeper meter | live |

## Ownership seams

O3 owns the browser-context dial, TLS-certificate, operator-diversity, and
served-state-retention probe exporter; its Prometheus job is deliberately
commented out until that artifact exists. O4 owns the actual `RB-*` runbook
content under `deploy/runbooks/`; O5 freezes the exact rule labels and routes
without fabricating those not-yet-authored runbooks. B10 is the existing
runtime-wiring closure milestone and is named on runtime-side telemetry gaps
where the frozen API and safe metadata-driven reads cannot produce the
canonical value.

The served-state alert uses the maximum retention reported by the joint fleet:
all retention windows end at now, so the longest operator window is the joint
window. O3 owns any stricter per-operator shortfall sub-alerts when it defines
the probe exporter and operator topology.

## Attestation configuration and provisional release schema

Copy [attestation-monitor.example.toml](attestation-monitor.example.toml) and
replace all placeholders. At least three independently operated gateways and
one or more node WebSocket endpoints are required. Resource limits and the
release-key signature minimum are explicitly operator-supplied because the
architecture does not fix them. The three release-integrity webhook sets are
mandatory. Public minisign keys use [keyring.example.toml](keyring.example.toml)
as a shape guide; it contains no usable key or secret.

The deployment requirement is **at least two independent monitor operators,
disjoint from ArNS controllers** (12 §2.2/§5.2/§6.5). Software cannot prove
natural-person or organizational disjointness; the signer registry and
operations ceremony must enforce it.

Until O1 freezes `release.json`, the adapter expects
`schema = "bleavit.release.provisional.v1"` with:

- `manifest_txid`, `keyring_generation`, and
  `supported_spec_version = {min, max}`;
- `files`, a path → lowercase SHA-256 map covering every manifest path except
  `release.json` itself (the signed document cannot contain its own hash);
- `release_signatures` and `attestations`, lists of `{txid = ...}`-equivalent
  JSON objects whose raw Arweave transactions contain minisign signatures over
  `SHA256(release.json)`.

Detached signatures are outside the served path manifest, avoiding a circular
file-hash/signature dependency. The monitor fetches the whole path manifest by
resolved TXID and by name through every gateway, compares all copies, fetches
detached signature transactions through every gateway, supports minisign `Ed`
and `ED` (BLAKE2b-512 prehash), requires the configured release-key minimum and
at least two distinct valid attestor keys, applies the on-chain revocation mask,
and binds the keyring generation and manifest TXID to `ReleaseChannel`. O1 can
replace only this extraction/fetch adapter; the verdict core already accepts
format-agnostic maps/blobs/keyrings/channel bytes.

## Privacy boundary

Per 12 §6.1 and the closing paragraph of §6.3, this stack monitors
**infrastructure only**. The decentralized frontend ships no telemetry of any
kind. Its only diagnostic channel remains a user-initiated copy-to-clipboard
report.

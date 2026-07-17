#!/usr/bin/env python3
"""Bleavit finalized-chain Prometheus exporter (12 section 6.3; 02 sections 3/4/12).

Runtime-API and storage values are decoded through the live portable metadata
registry.  The sole metadata-independent value decoder is the frozen 168-byte
``ReleaseChannel`` prefix.  Importing this module performs no network imports.
"""

from __future__ import annotations

import argparse
import logging
import sys
import time
from pathlib import Path
from typing import Any, Callable, Mapping

sys.path.insert(0, str(Path(__file__).resolve().parent))
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "release"))

from common import (  # noqa: E402
    MetricStore,
    MonitoringError,
    RELEASE_CHANNEL_KEY,
    ScaleValueError,
    SeriesDefinition,
    WsRpc,
    compact_encode,
    decode_release_channel,
    decode_typed_bytes,
    header_number,
    hex_bytes,
    nested_field,
    serve_metrics,
    variant_name,
)
from release_common import storage_prefix  # noqa: E402
from scale_metadata import MetadataDecodeError, decode_metadata  # noqa: E402


LOG = logging.getLogger("bleavit-chain-alerts")


def _series(name: str, kind: str, help_text: str, *labels: str) -> SeriesDefinition:
    return SeriesDefinition(name, kind, help_text, tuple(labels))


SERIES: dict[str, SeriesDefinition] = {
    item.name: item
    for item in (
        _series("bleavit_chain_connected", "gauge", "Whether the exporter has a live node connection."),
        _series("bleavit_chain_finalized_block", "gauge", "Latest observed finalized block height."),
        _series("bleavit_chain_last_successful_scrape_timestamp_seconds", "gauge", "Unix time of the latest complete scrape."),
        _series("bleavit_chain_scrape_errors_total", "counter", "Malformed response, decode, and transport failures."),
        _series("bleavit_chain_epoch_index", "gauge", "Current epoch index from FutarchyApi::epoch_status."),
        _series("bleavit_chain_epoch_phase", "gauge", "One-hot current epoch phase.", "phase"),
        _series("bleavit_chain_blocks_to_boundary", "gauge", "Blocks remaining to the epoch phase boundary."),
        _series("bleavit_chain_tick_lag_blocks", "gauge", "Finalized blocks elapsed past an unprocessed epoch boundary."),
        _series("bleavit_chain_dead_man_armed", "gauge", "Dead-man flag from epoch_status."),
        _series("bleavit_chain_ledger_frozen", "gauge", "Ledger-freeze flag from epoch_status."),
        _series("bleavit_chain_phase_flags", "gauge", "Raw Constitution phase flag word from epoch_status."),
        _series("bleavit_chain_proposals", "gauge", "Live proposal count by portable state name.", "state"),
        _series("bleavit_chain_execution_queue_depth", "gauge", "Queued execution count."),
        _series("bleavit_chain_execution_queue_bound", "gauge", "Live MaxLiveProposals metadata bound for the queue."),
        _series("bleavit_chain_oracle_open_disputes", "gauge", "Number of open oracle rounds."),
        _series("bleavit_chain_oracle_max_round_depth", "gauge", "Maximum round number among open oracle rounds."),
        _series("bleavit_chain_welfare_current_1e9", "gauge", "Current welfare aggregate on the 1e9 grid."),
        _series("bleavit_chain_welfare_reserve_flag", "gauge", "Welfare reserve-health flag."),
        _series("bleavit_chain_treasury_nav", "gauge", "Treasury NAV in chain balance base units."),
        _series("bleavit_chain_treasury_spendable_nav", "gauge", "Spendable NAV in chain balance base units."),
        _series("bleavit_chain_treasury_meter_utilization_bps", "gauge", "Treasury rolling-meter utilization in basis points."),
        _series("bleavit_chain_keeper_budget_limit", "gauge", "Live keeper.budget Param value in chain balance base units."),
        _series("bleavit_chain_keeper_budget_spent", "gauge", "Current-epoch keeper meter spend in chain balance base units."),
        _series("bleavit_chain_keeper_budget_utilization_ratio", "gauge", "Current keeper spend divided by the live keeper.budget Param."),
        _series("bleavit_chain_xcm_trapped_assets", "gauge", "Count of PolkadotXcm AssetTraps keys."),
        _series("bleavit_chain_storage_map_entries", "gauge", "Counted map occupancy for a metadata-discovered prefix.", "pallet", "item"),
        _series("bleavit_chain_storage_map_bound", "gauge", "Metadata constant bound paired with a counted map.", "pallet", "item"),
        _series("bleavit_chain_guardian_actions_total", "counter", "Finalized GuardianAction events."),
        _series("bleavit_chain_upgrade_authorized_total", "counter", "Finalized UpgradeAuthorized events."),
        _series("bleavit_chain_upgrade_applied_total", "counter", "Finalized UpgradeApplied events."),
        _series("bleavit_chain_keeper_budget_low_events_total", "counter", "Finalized KeeperBudgetLow threshold events."),
        _series("bleavit_chain_release_channel_info", "gauge", "Current release identity labels from the frozen channel.", "version", "manifest_txid", "min_supported_version"),
        _series("bleavit_chain_release_channel_spec_version", "gauge", "ReleaseChannel target/current runtime spec_version."),
        _series("bleavit_chain_release_channel_updated_at_block", "gauge", "ReleaseChannel last-update block."),
        _series("bleavit_chain_release_channel_pending_authorized_at_block", "gauge", "Pending UpgradeAuthorized block or zero."),
        _series("bleavit_chain_pending_upgrade_age_blocks", "gauge", "Age of a pending UpgradeAuthorized, or zero."),
        _series("bleavit_chain_descriptor_lead_time_blocks", "gauge", "ExecutionGuard DescriptorLeadTime metadata constant."),
        _series("bleavit_chain_release_channel_keyring_generation", "gauge", "ReleaseChannel keyring generation."),
        _series("bleavit_chain_release_channel_revoked_key_bits", "gauge", "ReleaseChannel revoked key bitmask."),
        _series("bleavit_chain_release_channel_flags", "gauge", "Raw ReleaseChannel flag word."),
        _series("bleavit_chain_release_channel_security", "gauge", "ReleaseChannel SECURITY bit."),
        _series("bleavit_chain_release_channel_expedited", "gauge", "ReleaseChannel EXPEDITED bit."),
        _series("bleavit_chain_release_channel_urgent_upgrade", "gauge", "ReleaseChannel URGENT_UPGRADE bit."),
        _series("bleavit_chain_release_channel_security_flips_total", "counter", "Observed finalized SECURITY-bit transitions."),
    )
}


COUNTED_MAPS = (
    ("Epoch", "Proposals", "Epoch", "MaxLiveProposals"),
    ("Epoch", "IntakeProposals", "Epoch", "MaxIntakeQueue"),
    ("Epoch", "Cohorts", "Epoch", "MaxNonTerminalCohorts"),
    ("Market", "Markets", "Market", "MaxLiveMarkets"),
    ("ExecutionGuard", "Queue", "ExecutionGuard", "MaxLiveProposals"),
    ("Constitution", "Params", "Constitution", "MaxParams"),
)

RELEASE_CHANNEL_FAMILIES = tuple(
    name
    for name in SERIES
    if name.startswith("bleavit_chain_release_channel_")
) + ("bleavit_chain_pending_upgrade_age_blocks",)
EVENT_FAMILIES = (
    "bleavit_chain_guardian_actions_total",
    "bleavit_chain_upgrade_authorized_total",
    "bleavit_chain_upgrade_applied_total",
    "bleavit_chain_keeper_budget_low_events_total",
)
FULL_DOMAIN_FAMILIES = {
    "epoch": (
        "bleavit_chain_epoch_index",
        "bleavit_chain_epoch_phase",
        "bleavit_chain_blocks_to_boundary",
        "bleavit_chain_tick_lag_blocks",
        "bleavit_chain_dead_man_armed",
        "bleavit_chain_ledger_frozen",
        "bleavit_chain_phase_flags",
    ),
    "proposal state": ("bleavit_chain_proposals",),
    "execution queue": (
        "bleavit_chain_execution_queue_depth",
        "bleavit_chain_execution_queue_bound",
    ),
    "oracle": (
        "bleavit_chain_oracle_open_disputes",
        "bleavit_chain_oracle_max_round_depth",
    ),
    "welfare": (
        "bleavit_chain_welfare_current_1e9",
        "bleavit_chain_welfare_reserve_flag",
    ),
    "treasury": (
        "bleavit_chain_treasury_nav",
        "bleavit_chain_treasury_spendable_nav",
        "bleavit_chain_treasury_meter_utilization_bps",
    ),
    "keeper budget": (
        "bleavit_chain_keeper_budget_limit",
        "bleavit_chain_keeper_budget_spent",
        "bleavit_chain_keeper_budget_utilization_ratio",
    ),
    "descriptor lead time": ("bleavit_chain_descriptor_lead_time_blocks",),
    "storage": (
        "bleavit_chain_storage_map_entries",
        "bleavit_chain_storage_map_bound",
    ),
    "xcm traps": ("bleavit_chain_xcm_trapped_assets",),
}
DOMAIN_ERRORS = (MonitoringError, ScaleValueError, MetadataDecodeError, ValueError)


def encode_param_keys(keys: list[str]) -> bytes:
    encoded = bytearray(compact_encode(len(keys)))
    for key in keys:
        raw = key.encode("ascii")
        if len(raw) > 16:
            raise ValueError(f"ParamKey {key!r} exceeds 16 bytes")
        encoded.extend(raw.ljust(16, b"\0"))
    return bytes(encoded)


def _runtime_event_names(record: Any) -> tuple[str | None, str | None]:
    event = record.get("event") if isinstance(record, dict) else None
    pallet = variant_name(event)
    fields = event.get("fields") if isinstance(event, dict) else None
    return pallet, variant_name(fields)


def _integer_field(value: Any, field: str, source: str) -> int:
    candidate = value.get(field) if isinstance(value, dict) else None
    if isinstance(candidate, bool) or not isinstance(candidate, int):
        raise MonitoringError(f"{source} has no integer {field} field")
    return candidate


def _boolean_field(value: Any, field: str, source: str) -> bool:
    candidate = value.get(field) if isinstance(value, dict) else None
    if not isinstance(candidate, bool):
        raise MonitoringError(f"{source} has no boolean {field} field")
    return candidate


class ChainExporter:
    def __init__(self, rpc: WsRpc, store: MetricStore | None = None):
        self.rpc = rpc
        self.store = store or MetricStore(SERIES)
        self.metadata: dict[str, Any] | None = None
        self.metadata_spec_version: int | None = None
        self.last_event_hash: str | None = None
        self.previous_security: bool | None = None
        self.security_flips_total = 0
        self.event_totals = {name: 0 for name in EVENT_FAMILIES}
        self.store.set("bleavit_chain_connected", 1)
        for counter in (
            "bleavit_chain_scrape_errors_total",
            "bleavit_chain_guardian_actions_total",
            "bleavit_chain_upgrade_authorized_total",
            "bleavit_chain_upgrade_applied_total",
            "bleavit_chain_keeper_budget_low_events_total",
            "bleavit_chain_release_channel_security_flips_total",
        ):
            self.store.set(counter, 0)

    def _load_metadata(self, block_hash: str, force: bool = False) -> dict[str, Any]:
        if self.metadata is not None and not force:
            return self.metadata
        raw = hex_bytes(
            self.rpc.call("state_getMetadata", [block_hash]), "state_getMetadata"
        )
        assert raw is not None
        self.metadata = decode_metadata(raw)
        return self.metadata

    def _runtime_api(self, method: str, params: bytes, block_hash: str) -> Any:
        metadata = self._load_metadata(block_hash)
        api = metadata.get("apis", {}).get("FutarchyApi")
        entry = api.get("methods", {}).get(method) if api else None
        if entry is None:
            raise MonitoringError(f"live metadata has no FutarchyApi.{method}")
        response = self.rpc.call(
            "state_call", [f"FutarchyApi_{method}", "0x" + params.hex(), block_hash]
        )
        raw = hex_bytes(response, f"state_call FutarchyApi_{method}")
        assert raw is not None
        return decode_typed_bytes(raw, entry["output_type"], metadata)

    def _constant(self, pallet: str, name: str, block_hash: str) -> Any:
        metadata = self._load_metadata(block_hash)
        item = metadata.get("pallets", {}).get(pallet, {}).get("constants", {}).get(name)
        if item is None:
            raise MonitoringError(f"live metadata has no {pallet}.{name} constant")
        return decode_typed_bytes(item["value"], item["type_id"], metadata)

    def _storage(self, pallet: str, item_name: str, block_hash: str) -> Any:
        metadata = self._load_metadata(block_hash)
        pallet_meta = metadata.get("pallets", {}).get(pallet)
        storage = pallet_meta.get("storage") if pallet_meta else None
        item = storage.get("entries", {}).get(item_name) if storage else None
        if item is None:
            raise MonitoringError(f"live metadata has no {pallet}.{item_name} storage")
        key = storage_prefix(storage["prefix"], item_name)
        raw = hex_bytes(
            self.rpc.call("state_getStorage", [key, block_hash]),
            f"state_getStorage {pallet}.{item_name}",
            optional=True,
        )
        if raw is None:
            raw = item["default"]
        return decode_typed_bytes(raw, item["value_type"], metadata)

    def _count_prefix(self, pallet: str, item_name: str, block_hash: str) -> int:
        metadata = self._load_metadata(block_hash)
        pallet_meta = metadata.get("pallets", {}).get(pallet)
        storage = pallet_meta.get("storage") if pallet_meta else None
        item = storage.get("entries", {}).get(item_name) if storage else None
        if item is None or item.get("kind") != "map":
            raise MonitoringError(f"live metadata has no map {pallet}.{item_name}")
        prefix = storage_prefix(storage["prefix"], item_name)
        count = 0
        start: str | None = None
        for _ in range(100):
            keys = self.rpc.call(
                "state_getKeysPaged", [prefix, 1000, start, block_hash]
            )
            if not isinstance(keys, list) or not all(isinstance(key, str) for key in keys):
                raise MonitoringError(f"state_getKeysPaged {pallet}.{item_name} returned malformed keys")
            count += len(keys)
            if len(keys) < 1000:
                return count
            start = keys[-1]
        raise MonitoringError(f"{pallet}.{item_name} exceeds the exporter's 100,000-key safety cap")

    def _release_channel(self, block_hash: str, block: int) -> None:
        raw = hex_bytes(
            self.rpc.call("state_getStorage", [RELEASE_CHANNEL_KEY, block_hash]),
            "ReleaseChannel storage",
        )
        assert raw is not None
        channel = decode_release_channel(raw)
        self.store.clear_family("bleavit_chain_release_channel_info")
        self.store.set(
            "bleavit_chain_release_channel_info",
            1,
            {
                "version": channel.version,
                "manifest_txid": channel.manifest_txid,
                "min_supported_version": channel.min_supported_version,
            },
        )
        values = {
            "bleavit_chain_release_channel_spec_version": channel.spec_version,
            "bleavit_chain_release_channel_updated_at_block": channel.updated_at,
            "bleavit_chain_release_channel_pending_authorized_at_block": channel.pending_authorized_at,
            "bleavit_chain_pending_upgrade_age_blocks": (
                max(0, block - channel.pending_authorized_at)
                if channel.pending_authorized_at
                else 0
            ),
            "bleavit_chain_release_channel_keyring_generation": channel.keyring_generation,
            "bleavit_chain_release_channel_revoked_key_bits": channel.revoked_key_bits,
            "bleavit_chain_release_channel_flags": channel.flags,
            "bleavit_chain_release_channel_security": int(channel.security),
            "bleavit_chain_release_channel_expedited": int(channel.expedited),
            "bleavit_chain_release_channel_urgent_upgrade": int(channel.urgent_upgrade),
        }
        for name, value in values.items():
            self.store.set(name, value)
        if self.previous_security is not None and channel.security != self.previous_security:
            self.security_flips_total += 1
        self.store.set(
            "bleavit_chain_release_channel_security_flips_total",
            self.security_flips_total,
        )
        self.previous_security = channel.security

    def _events(self, block_hash: str) -> None:
        if block_hash == self.last_event_hash:
            return
        records = self._storage("System", "Events", block_hash)
        if not isinstance(records, list):
            raise MonitoringError("System.Events did not decode to a sequence")
        observed = {name: 0 for name in EVENT_FAMILIES}
        for record in records:
            pallet, event = _runtime_event_names(record)
            if (pallet, event) == ("Guardian", "GuardianAction"):
                observed["bleavit_chain_guardian_actions_total"] += 1
            elif (pallet, event) == ("ExecutionGuard", "UpgradeAuthorized"):
                observed["bleavit_chain_upgrade_authorized_total"] += 1
            elif (pallet, event) == ("ExecutionGuard", "UpgradeApplied"):
                observed["bleavit_chain_upgrade_applied_total"] += 1
            elif (pallet, event) == ("FutarchyTreasury", "KeeperBudgetLow"):
                observed["bleavit_chain_keeper_budget_low_events_total"] += 1
        for name, count in observed.items():
            self.event_totals[name] += count
            self.store.set(name, self.event_totals[name])
        self.last_event_hash = block_hash

    def _storage_counts(self, block_hash: str) -> None:
        for pallet, item, bound_pallet, bound_name in COUNTED_MAPS:
            labels = {"pallet": pallet, "item": item}
            self.store.set(
                "bleavit_chain_storage_map_entries",
                self._count_prefix(pallet, item, block_hash),
                labels,
            )
            self.store.set(
                "bleavit_chain_storage_map_bound",
                self._constant(bound_pallet, bound_name, block_hash),
                labels,
            )

    def _xcm_traps(self, block_hash: str) -> None:
        self.store.set(
            "bleavit_chain_xcm_trapped_assets",
            self._count_prefix("PolkadotXcm", "AssetTraps", block_hash),
        )

    def _epoch_status(self, block_hash: str, block: int) -> None:
        epoch = self._runtime_api("epoch_status", b"", block_hash)
        if not isinstance(epoch, dict):
            raise MonitoringError("epoch_status did not decode to a struct")
        phase = variant_name(epoch.get("phase"))
        if phase is None:
            raise MonitoringError("epoch_status has no portable phase variant")
        self.store.set(
            "bleavit_chain_epoch_index", _integer_field(epoch, "index", "epoch_status")
        )
        self.store.clear_family("bleavit_chain_epoch_phase")
        self.store.set("bleavit_chain_epoch_phase", 1, {"phase": phase})
        boundary = _integer_field(epoch, "next_boundary", "epoch_status")
        self.store.set("bleavit_chain_blocks_to_boundary", max(0, boundary - block))
        self.store.set("bleavit_chain_tick_lag_blocks", max(0, block - boundary))
        self.store.set(
            "bleavit_chain_dead_man_armed",
            int(_boolean_field(epoch, "dead_man_armed", "epoch_status")),
        )
        self.store.set(
            "bleavit_chain_ledger_frozen",
            int(_boolean_field(epoch, "ledger_frozen", "epoch_status")),
        )
        self.store.set(
            "bleavit_chain_phase_flags", _integer_field(epoch, "phase_flags", "epoch_status")
        )

    def _proposal_state(self, block_hash: str) -> None:
        proposals = self._runtime_api("proposal_summaries", b"", block_hash)
        if not isinstance(proposals, list):
            raise MonitoringError("proposal_summaries did not decode to a sequence")
        counts: dict[str, int] = {}
        for proposal in proposals:
            state = variant_name(proposal.get("state")) if isinstance(proposal, dict) else None
            if state is None:
                raise MonitoringError("proposal_summaries entry has no portable state variant")
            counts[state] = counts.get(state, 0) + 1
        self.store.clear_family("bleavit_chain_proposals")
        for state, count in sorted(counts.items()):
            self.store.set("bleavit_chain_proposals", count, {"state": state})

    def _execution_queue(self, block_hash: str) -> None:
        queue = self._runtime_api("execution_queue", b"", block_hash)
        if not isinstance(queue, list):
            raise MonitoringError("execution_queue did not decode to a sequence")
        self.store.set("bleavit_chain_execution_queue_depth", len(queue))
        self.store.set(
            "bleavit_chain_execution_queue_bound",
            self._constant("ExecutionGuard", "MaxLiveProposals", block_hash),
        )

    def _oracle(self, block_hash: str) -> None:
        rounds = self._runtime_api("open_oracle_rounds", b"", block_hash)
        if not isinstance(rounds, list):
            raise MonitoringError("open_oracle_rounds did not decode to a sequence")
        depths = [
            _integer_field(row, "round", "open_oracle_rounds entry") for row in rounds
        ]
        self.store.set("bleavit_chain_oracle_open_disputes", len(rounds))
        self.store.set("bleavit_chain_oracle_max_round_depth", max(depths, default=0))

    def _welfare(self, block_hash: str) -> None:
        welfare = self._runtime_api("welfare_current", b"", block_hash)
        if not isinstance(welfare, dict):
            raise MonitoringError("welfare_current did not decode to a struct")
        self.store.set(
            "bleavit_chain_welfare_current_1e9",
            _integer_field(welfare, "w_current_1e9", "welfare_current"),
        )
        self.store.set(
            "bleavit_chain_welfare_reserve_flag",
            int(_boolean_field(welfare, "reserve_flag", "welfare_current")),
        )

    def _treasury(self, block_hash: str) -> None:
        nav = self._runtime_api("nav", b"", block_hash)
        if not isinstance(nav, dict):
            raise MonitoringError("nav did not decode to a struct")
        self.store.set(
            "bleavit_chain_treasury_nav", _integer_field(nav, "total", "nav")
        )
        self.store.set(
            "bleavit_chain_treasury_spendable_nav",
            _integer_field(nav, "spendable_nav", "nav"),
        )
        self.store.set(
            "bleavit_chain_treasury_meter_utilization_bps",
            _integer_field(nav, "meter_utilization_bps", "nav"),
        )

    def _keeper_budget(self, block_hash: str) -> None:
        params = self._runtime_api("params", encode_param_keys(["keeper.budget"]), block_hash)
        if not isinstance(params, list) or len(params) != 1:
            raise MonitoringError("params returned no unique keeper.budget record")
        budget = _integer_field(params[0], "value", "keeper.budget ParamView")
        if budget <= 0:
            raise MonitoringError("keeper.budget ParamView must be positive")
        treasury = self._storage("FutarchyTreasury", "State", block_hash)
        keeper_meter = treasury.get("keeper_meter") if isinstance(treasury, dict) else None
        spent = _integer_field(keeper_meter, "spent", "FutarchyTreasury.State keeper_meter")
        self.store.set("bleavit_chain_keeper_budget_limit", budget)
        self.store.set("bleavit_chain_keeper_budget_spent", spent)
        self.store.set(
            "bleavit_chain_keeper_budget_utilization_ratio",
            spent / budget,
        )

    def _descriptor_lead_time(self, block_hash: str) -> None:
        self.store.set(
            "bleavit_chain_descriptor_lead_time_blocks",
            self._constant("ExecutionGuard", "DescriptorLeadTime", block_hash),
        )

    def _run_domain(
        self,
        domain: str,
        families: tuple[str, ...],
        scrape: Callable[[], None],
    ) -> bool:
        try:
            scrape()
            return True
        except DOMAIN_ERRORS as error:
            for family in families:
                self.store.clear_family(family)
            self.store.inc("bleavit_chain_scrape_errors_total")
            LOG.error("%s scrape domain rejected: %s", domain, error)
            return False

    def scrape(
        self, block_hash: str | None = None, block: int | None = None, *, full: bool = True
    ) -> bool:
        if block_hash is None:
            block_hash = self.rpc.call("chain_getFinalizedHead")
        if not isinstance(block_hash, str):
            raise MonitoringError("chain_getFinalizedHead returned no hash")
        if block is None:
            block = header_number(self.rpc.call("chain_getHeader", [block_hash]))
        self.store.set("bleavit_chain_finalized_block", block)
        runtime_version = self.rpc.call("state_getRuntimeVersion", [block_hash])
        spec_version = runtime_version.get("specVersion") if isinstance(runtime_version, dict) else None
        if not isinstance(spec_version, int):
            raise MonitoringError("state_getRuntimeVersion returned no integer specVersion")
        if self.metadata_spec_version != spec_version:
            self._load_metadata(block_hash, force=True)
            self.metadata_spec_version = spec_version
        else:
            self._load_metadata(block_hash)
        complete = self._run_domain(
            "ReleaseChannel",
            RELEASE_CHANNEL_FAMILIES,
            lambda: self._release_channel(block_hash, block),
        )
        complete = self._run_domain(
            "finalized events", EVENT_FAMILIES, lambda: self._events(block_hash)
        ) and complete
        if not full:
            return complete
        domains = (
            ("epoch", lambda: self._epoch_status(block_hash, block)),
            ("proposal state", lambda: self._proposal_state(block_hash)),
            ("execution queue", lambda: self._execution_queue(block_hash)),
            ("oracle", lambda: self._oracle(block_hash)),
            ("welfare", lambda: self._welfare(block_hash)),
            ("treasury", lambda: self._treasury(block_hash)),
            ("keeper budget", lambda: self._keeper_budget(block_hash)),
            ("descriptor lead time", lambda: self._descriptor_lead_time(block_hash)),
            ("storage", lambda: self._storage_counts(block_hash)),
            ("xcm traps", lambda: self._xcm_traps(block_hash)),
        )
        for domain, scrape_domain in domains:
            complete = self._run_domain(
                domain, FULL_DOMAIN_FAMILIES[domain], scrape_domain
            ) and complete
        if complete:
            self.store.set(
                "bleavit_chain_last_successful_scrape_timestamp_seconds", time.time()
            )
        return complete


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Export finalized Bleavit chain alert series.")
    parser.add_argument("--url", required=True, help="node WebSocket endpoint (ws:// or wss://)")
    parser.add_argument("--bind", default="127.0.0.1:9617", help="Prometheus listen HOST:PORT")
    parser.add_argument("--interval", type=float, default=30.0, help="full poll cadence in seconds")
    parser.add_argument("--once", action="store_true", help="scrape once to stdout and exit")
    args = parser.parse_args(argv)
    if not args.url.startswith(("ws://", "wss://")):
        parser.error("--url must start with ws:// or wss://")
    if args.interval <= 0:
        parser.error("--interval must be positive")
    return args


def run(args: argparse.Namespace) -> int:
    store = MetricStore(SERIES)
    if not args.once:
        serve_metrics(store, args.bind)
    backoff = 1.0
    while True:
        rpc: WsRpc | None = None
        try:
            rpc = WsRpc(args.url)
            exporter = ChainExporter(rpc, store)
            if args.once:
                complete = exporter.scrape()
                sys.stdout.write(store.render())
                return 0 if complete else 2
            subscription = rpc.subscribe_finalized()
            last_full = 0.0
            backoff = 1.0
            while True:
                header = rpc.next_finalized(subscription, timeout=args.interval)
                now = time.monotonic()
                if header is None:
                    block_hash = rpc.call("chain_getFinalizedHead")
                    block = header_number(rpc.call("chain_getHeader", [block_hash]))
                else:
                    block_hash = header.get("hash")
                    if not isinstance(block_hash, str):
                        # Classic subscription headers do not carry their hash.
                        block_hash = rpc.call("chain_getFinalizedHead")
                        block = header_number(rpc.call("chain_getHeader", [block_hash]))
                    else:
                        block = header_number(header)
                try:
                    exporter.scrape(
                        block_hash,
                        block,
                        full=now - last_full >= args.interval,
                    )
                    if now - last_full >= args.interval:
                        last_full = now
                except (MonitoringError, ScaleValueError, MetadataDecodeError, ValueError) as error:
                    store.inc("bleavit_chain_scrape_errors_total")
                    LOG.error("finalized scrape rejected: %s", error)
        except KeyboardInterrupt:
            return 0
        except Exception as error:  # transport libraries expose several exception classes.
            store.set("bleavit_chain_connected", 0)
            store.inc("bleavit_chain_scrape_errors_total")
            LOG.error("connection/scrape failure: %s; reconnecting in %.0fs", error, backoff)
            if args.once:
                return 2
            time.sleep(backoff)
            backoff = min(backoff * 2, 60.0)
        finally:
            if rpc is not None:
                try:
                    rpc.close()
                except Exception:
                    pass


def main(argv: list[str] | None = None) -> int:
    logging.basicConfig(level=logging.INFO, format="%(asctime)s %(levelname)s %(message)s")
    return run(parse_args(argv))


if __name__ == "__main__":
    raise SystemExit(main())

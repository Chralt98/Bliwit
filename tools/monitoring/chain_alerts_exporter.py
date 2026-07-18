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

# Operational resource bound for one catch-up pass; this is not a protocol parameter.
MAX_EVENT_CATCH_UP_BLOCKS = 512


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
        _series("bleavit_market_book_loss_usdc", "gauge", "Realized maker inventory loss in USDC base units.", "market"),
        _series("bleavit_market_lmsr_loss_bound_usdc", "gauge", "Canonical seed_headroom(b) loss bound in USDC base units.", "market"),
        _series("bleavit_market_mid_window_coverage_percent", "gauge", "Projected scheduled-observation coverage for an active unsealed decision window.", "market", "start", "end"),
        _series("bleavit_market_effective_pol_usdc", "gauge", "Funded balance for one independently accounted POL component in USDC base units.", "component"),
        _series("bleavit_market_pol_floor_usdc", "gauge", "Matching requirement for one independently accounted POL component in USDC base units.", "component"),
        _series("bleavit_ledger_collateral_drift_usdc", "gauge", "Signed ledger custody minus audited L-2 liability in USDC base units."),
        _series("bleavit_runtime_migration_cursor_stalled", "gauge", "Canonical migration halt/live-stall detector state."),
        _series("bleavit_runtime_storage_max_utilization_ratio", "gauge", "Occupancy ratio for a metadata-invisible bounded storage shape.", "map"),
        _series("bleavit_runtime_lmsr_domain_rejections_total", "counter", "Finalized Market.PriceBoundExceeded extrinsic failures resolved from live metadata."),
        _series("bleavit_runtime_numeric_anomaly_spike", "gauge", "Anomalous positive ledger rounding residue.", "kind"),
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
    "bleavit_runtime_lmsr_domain_rejections_total",
)
FINALIZED_EVENT_FAMILIES = EVENT_FAMILIES
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
    "market books": (
        "bleavit_market_book_loss_usdc",
        "bleavit_market_lmsr_loss_bound_usdc",
    ),
    "mid-window coverage": ("bleavit_market_mid_window_coverage_percent",),
    "pol": (
        "bleavit_market_effective_pol_usdc",
        "bleavit_market_pol_floor_usdc",
    ),
    "collateral": ("bleavit_ledger_collateral_drift_usdc",),
    "migration stall": ("bleavit_runtime_migration_cursor_stalled",),
    "storage remainder": ("bleavit_runtime_storage_max_utilization_ratio",),
    "numeric anomalies": ("bleavit_runtime_numeric_anomaly_spike",),
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


def _required_some(value: Any, source: str) -> Any:
    if variant_name(value) != "Some":
        raise MonitoringError(f"{source} returned no audited value")
    return value.get("fields")


def _bounded_ascii(value: Any, source: str) -> str:
    if not isinstance(value, list) or not all(
        isinstance(item, int) and not isinstance(item, bool) and 0 <= item <= 255
        for item in value
    ):
        raise MonitoringError(f"{source} is not a bounded byte string")
    try:
        decoded = bytes(value).decode("ascii")
    except UnicodeDecodeError as error:
        raise MonitoringError(f"{source} is not ASCII") from error
    if not decoded:
        raise MonitoringError(f"{source} is empty")
    return decoded


class ChainExporter:
    def __init__(self, rpc: WsRpc, store: MetricStore | None = None):
        self.rpc = rpc
        self.store = store or MetricStore(SERIES)
        self.metadata: dict[str, Any] | None = None
        self.metadata_spec_version: int | None = None
        self.last_event_block: int | None = None
        self.previous_security: bool | None = None
        self.security_flips_total = 0
        self.event_totals = {name: 0 for name in EVENT_FAMILIES}
        self.store.set("bleavit_chain_connected", 1)
        for counter in (
            "bleavit_chain_scrape_errors_total",
            *EVENT_FAMILIES,
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

    def _telemetry_api(self, method: str, block_hash: str) -> Any:
        metadata = self._load_metadata(block_hash)
        api = metadata.get("apis", {}).get("TelemetryApi")
        entry = api.get("methods", {}).get(method) if api else None
        if entry is None:
            raise MonitoringError(f"live metadata has no TelemetryApi.{method}")
        response = self.rpc.call(
            "state_call", [f"TelemetryApi_{method}", "0x", block_hash]
        )
        raw = hex_bytes(response, f"state_call TelemetryApi_{method}")
        assert raw is not None
        return decode_typed_bytes(raw, entry["output_type"], metadata)

    def _price_bound_error_identity(self) -> tuple[int, int]:
        metadata = self.metadata
        if metadata is None:
            raise MonitoringError("portable metadata is not loaded")
        pallet = metadata.get("pallets", {}).get("Market")
        if not isinstance(pallet, dict):
            raise MonitoringError("live metadata has no Market pallet")
        module_index = pallet.get("index")
        error_type = pallet.get("error_type")
        error_meta = metadata.get("types", {}).get(error_type)
        variants = (
            error_meta.get("definition", {}).get("variants", [])
            if isinstance(error_meta, dict)
            else []
        )
        matches = [item for item in variants if item.get("name") == "PriceBoundExceeded"]
        if (
            isinstance(module_index, bool)
            or not isinstance(module_index, int)
            or len(matches) != 1
            or isinstance(matches[0].get("index"), bool)
            or not isinstance(matches[0].get("index"), int)
        ):
            raise MonitoringError(
                "live metadata cannot resolve Market.PriceBoundExceeded identity"
            )
        return module_index, matches[0]["index"]

    def _is_price_bound_failure(self, record: Any, identity: tuple[int, int]) -> bool:
        event = record.get("event") if isinstance(record, dict) else None
        inner = event.get("fields") if isinstance(event, dict) else None
        payload = inner.get("fields") if isinstance(inner, dict) else None
        dispatch_error = (
            payload.get("dispatch_error") if isinstance(payload, dict) else None
        )
        if variant_name(dispatch_error) != "Module":
            return False
        module = dispatch_error.get("fields")
        index = module.get("index") if isinstance(module, dict) else None
        error = module.get("error") if isinstance(module, dict) else None
        if (
            isinstance(index, bool)
            or not isinstance(index, int)
            or not isinstance(error, list)
            or len(error) != 4
            or not all(
                isinstance(item, int)
                and not isinstance(item, bool)
                and 0 <= item <= 255
                for item in error
            )
        ):
            raise MonitoringError("System.ExtrinsicFailed has malformed Module error")
        return index == identity[0] and error[0] == identity[1]

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

    def _events(self, block_hash: str, block: int) -> None:
        if self.last_event_block is not None and block <= self.last_event_block:
            return
        records = self._storage("System", "Events", block_hash)
        if not isinstance(records, list):
            raise MonitoringError("System.Events did not decode to a sequence")
        observed = {name: 0 for name in EVENT_FAMILIES}
        price_bound_identity = self._price_bound_error_identity()
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
            elif (pallet, event) == ("System", "ExtrinsicFailed") and self._is_price_bound_failure(
                record, price_bound_identity
            ):
                # Failed extrinsics roll back pallet storage, so this finalized
                # event stream is the only auditable rejection source.
                observed["bleavit_runtime_lmsr_domain_rejections_total"] += 1
        for name, count in observed.items():
            self.event_totals[name] += count
            self.store.set(name, self.event_totals[name])
        self.last_event_block = block

    def _block_hash(self, block: int) -> str:
        block_hash = self.rpc.call("chain_getBlockHash", [block])
        if not isinstance(block_hash, str):
            raise MonitoringError(f"chain_getBlockHash returned no hash for block {block}")
        return block_hash

    def process_finalized(self, block_hash: str, block: int, *, full: bool) -> bool:
        """Scrape a finalized head without dropping events from a buffered gap."""
        if self.last_event_block is None:
            first = block
        elif block < self.last_event_block:
            # Finalized heads cannot reorg. Ignore an out-of-order stale
            # notification instead of moving gauges and decoder metadata backward.
            return True
        elif block == self.last_event_block:
            # A repeated notification is event-idempotent. A due full scrape still
            # refreshes the non-event domains at this finalized block.
            return self.scrape(block_hash, block, full=full) if full else True
        else:
            first = self.last_event_block + 1

        gap = block - first + 1
        if gap > MAX_EVENT_CATCH_UP_BLOCKS:
            skipped = gap - MAX_EVENT_CATCH_UP_BLOCKS
            first += skipped
            self.store.inc("bleavit_chain_scrape_errors_total", skipped)
            LOG.error(
                "finalized-event catch-up gap is %d blocks; skipping %d oldest blocks "
                "and processing bounded window %d..%d",
                gap,
                skipped,
                first,
                block,
            )
            # Record the deliberate loss so a later notification cannot retry the
            # skipped range and double-count the newest bounded window.
            self.last_event_block = first - 1

        complete = True
        for current in range(first, block + 1):
            current_hash = block_hash if current == block else self._block_hash(current)
            complete = self.scrape(
                current_hash,
                current,
                full=full and current == block,
            ) and complete
            if self.last_event_block != current:
                # The event domain failed closed. Stop so the failed block remains
                # the beginning of the next catch-up attempt.
                return False
        return complete

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

    def _market_books(self, block_hash: str) -> None:
        rows = _required_some(
            self._telemetry_api("market_books", block_hash),
            "TelemetryApi.market_books",
        )
        if not isinstance(rows, list):
            raise MonitoringError("TelemetryApi.market_books is not a sequence")
        self.store.clear_family("bleavit_market_book_loss_usdc")
        self.store.clear_family("bleavit_market_lmsr_loss_bound_usdc")
        observed: set[int] = set()
        for row in rows:
            market = _integer_field(row, "market", "market_books row")
            if market in observed:
                raise MonitoringError("TelemetryApi.market_books contains duplicate market")
            observed.add(market)
            labels = {"market": str(market)}
            self.store.set(
                "bleavit_market_book_loss_usdc",
                _integer_field(row, "book_loss_usdc", "market_books row"),
                labels,
            )
            self.store.set(
                "bleavit_market_lmsr_loss_bound_usdc",
                _integer_field(row, "lmsr_loss_bound_usdc", "market_books row"),
                labels,
            )

    def _mid_window_coverage(self, block_hash: str) -> None:
        rows = _required_some(
            self._telemetry_api("mid_window_coverage", block_hash),
            "TelemetryApi.mid_window_coverage",
        )
        if not isinstance(rows, list):
            raise MonitoringError("TelemetryApi.mid_window_coverage is not a sequence")
        self.store.clear_family("bleavit_market_mid_window_coverage_percent")
        observed: set[tuple[int, int, int]] = set()
        for row in rows:
            key = (
                _integer_field(row, "market", "mid_window_coverage row"),
                _integer_field(row, "start", "mid_window_coverage row"),
                _integer_field(row, "end", "mid_window_coverage row"),
            )
            if key in observed:
                raise MonitoringError("TelemetryApi.mid_window_coverage contains a duplicate")
            observed.add(key)
            coverage = _integer_field(
                row, "coverage_percent", "mid_window_coverage row"
            )
            if not 0 <= coverage <= 100:
                raise MonitoringError("mid-window coverage is outside 0..100")
            self.store.set(
                "bleavit_market_mid_window_coverage_percent",
                coverage,
                {"market": str(key[0]), "start": str(key[1]), "end": str(key[2])},
            )

    def _pol(self, block_hash: str) -> None:
        rows = _required_some(
            self._telemetry_api("pol", block_hash), "TelemetryApi.pol"
        )
        if not isinstance(rows, list):
            raise MonitoringError("TelemetryApi.pol is not a sequence")
        components: dict[str, tuple[int, int]] = {}
        names = {"Pol": "pol", "Baseline": "baseline"}
        for row in rows:
            if not isinstance(row, dict):
                raise MonitoringError("TelemetryApi.pol row is not a struct")
            component = names.get(variant_name(row.get("component")))
            if component is None:
                raise MonitoringError("TelemetryApi.pol row has an unknown component")
            if component in components:
                raise MonitoringError("TelemetryApi.pol contains a duplicate component")
            components[component] = (
                _integer_field(row, "effective_pol_usdc", "TelemetryApi.pol row"),
                _integer_field(row, "pol_floor_usdc", "TelemetryApi.pol row"),
            )
        if set(components) != {"pol", "baseline"}:
            raise MonitoringError("TelemetryApi.pol must contain both components")
        self.store.clear_family("bleavit_market_effective_pol_usdc")
        self.store.clear_family("bleavit_market_pol_floor_usdc")
        for component, (effective, floor) in components.items():
            labels = {"component": component}
            self.store.set("bleavit_market_effective_pol_usdc", effective, labels)
            self.store.set("bleavit_market_pol_floor_usdc", floor, labels)

    def _collateral(self, block_hash: str) -> None:
        value = _required_some(
            self._telemetry_api("collateral", block_hash),
            "TelemetryApi.collateral",
        )
        if not isinstance(value, dict):
            raise MonitoringError("TelemetryApi.collateral is not a struct")
        custody = _integer_field(value, "custody_usdc", "TelemetryApi.collateral")
        liability = _integer_field(value, "liability_usdc", "TelemetryApi.collateral")
        self.store.set("bleavit_ledger_collateral_drift_usdc", custody - liability)

    def _migration_stall(self, block_hash: str) -> None:
        value = self._telemetry_api("migration_cursor_stalled", block_hash)
        if not isinstance(value, bool):
            raise MonitoringError("TelemetryApi.migration_cursor_stalled is not boolean")
        self.store.set("bleavit_runtime_migration_cursor_stalled", int(value))

    def _storage_remainder(self, block_hash: str) -> None:
        rows = _required_some(
            self._telemetry_api("storage_utilization", block_hash),
            "TelemetryApi.storage_utilization",
        )
        if not isinstance(rows, list):
            raise MonitoringError("TelemetryApi.storage_utilization is not a sequence")
        self.store.clear_family("bleavit_runtime_storage_max_utilization_ratio")
        observed: set[str] = set()
        for row in rows:
            name = _bounded_ascii(
                row.get("map") if isinstance(row, dict) else None,
                "storage_utilization map",
            )
            if name in observed:
                raise MonitoringError("TelemetryApi.storage_utilization contains duplicate map")
            observed.add(name)
            entries = _integer_field(row, "entries", "storage_utilization row")
            bound = _integer_field(row, "bound", "storage_utilization row")
            if entries < 0 or bound <= 0 or entries > bound:
                raise MonitoringError("storage utilization row violates its bound")
            self.store.set(
                "bleavit_runtime_storage_max_utilization_ratio",
                entries / bound,
                {"map": name},
            )

    def _numeric_anomalies(self, block_hash: str) -> None:
        value = _required_some(
            self._telemetry_api("collateral", block_hash),
            "TelemetryApi.collateral",
        )
        if not isinstance(value, dict):
            raise MonitoringError("TelemetryApi.collateral is not a struct")
        dust = _integer_field(
            value,
            "anomalous_rounding_dust_usdc",
            "TelemetryApi.collateral",
        )
        self.store.clear_family("bleavit_runtime_numeric_anomaly_spike")
        self.store.set(
            "bleavit_runtime_numeric_anomaly_spike",
            dust,
            {"kind": "rounding_dust"},
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
            "finalized events",
            FINALIZED_EVENT_FAMILIES,
            lambda: self._events(block_hash, block),
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
            ("market books", lambda: self._market_books(block_hash)),
            ("mid-window coverage", lambda: self._mid_window_coverage(block_hash)),
            ("pol", lambda: self._pol(block_hash)),
            ("collateral", lambda: self._collateral(block_hash)),
            ("migration stall", lambda: self._migration_stall(block_hash)),
            ("storage remainder", lambda: self._storage_remainder(block_hash)),
            ("numeric anomalies", lambda: self._numeric_anomalies(block_hash)),
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
                    # Classic finalized-head subscriptions carry a header, never
                    # its hash. Resolve the hash from that header's own number so
                    # buffered notifications cannot collapse onto the newest head.
                    block = header_number(header)
                    block_hash = exporter._block_hash(block)
                try:
                    full = now - last_full >= args.interval
                    exporter.process_finalized(
                        block_hash,
                        block,
                        full=full,
                    )
                    if full and exporter.last_event_block == block:
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

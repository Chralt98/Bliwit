from __future__ import annotations

import unittest

import support  # noqa: F401 - inserts tools/monitoring on sys.path.

import chain_alerts_exporter as exporter_module
from common import MetricStore, MonitoringError


class RuntimeVersionRpc:
    def call(self, method: str, params: list[object] | None = None) -> object:
        if method == "state_getRuntimeVersion":
            return {"specVersion": 1}
        raise AssertionError(f"unexpected RPC call {method} {params}")


def family_names(store: MetricStore) -> set[str]:
    return {name for name, _labels in store.values}


def seed_family(store: MetricStore, name: str) -> None:
    definition = store.definitions[name]
    store.set(name, 9, {label: "stale" for label in definition.labels})


def metric_value(store: MetricStore, name: str) -> float:
    values = [value for (series, _labels), value in store.values.items() if series == name]
    if len(values) != 1:
        raise AssertionError(f"expected one {name} sample, found {values}")
    return values[0]


class ChainExporterIsolationTests(unittest.TestCase):
    def new_exporter(self) -> exporter_module.ChainExporter:
        store = MetricStore(exporter_module.SERIES)
        exporter = exporter_module.ChainExporter(RuntimeVersionRpc(), store)
        exporter._load_metadata = lambda _block_hash, force=False: {}  # type: ignore[method-assign]
        return exporter

    def test_release_channel_failure_clears_only_its_families_and_continues(self) -> None:
        exporter = self.new_exporter()
        for family in exporter_module.RELEASE_CHANNEL_FAMILIES:
            seed_family(exporter.store, family)

        def reject_channel(_block_hash: str, _block: int) -> None:
            raise MonitoringError("ReleaseChannel reserved flag bits are non-zero")

        exporter._release_channel = reject_channel  # type: ignore[method-assign]
        exporter._events = lambda _block_hash, _block: exporter.store.set(  # type: ignore[method-assign]
            "bleavit_chain_guardian_actions_total", 7
        )

        complete = exporter.scrape("0x01", 42, full=False)

        present = family_names(exporter.store)
        self.assertFalse(complete)
        for family in exporter_module.RELEASE_CHANNEL_FAMILIES:
            self.assertNotIn(family, present)
        self.assertEqual(metric_value(exporter.store, "bleavit_chain_guardian_actions_total"), 7)
        self.assertEqual(metric_value(exporter.store, "bleavit_chain_scrape_errors_total"), 1)

    def test_event_failure_clears_event_families_and_full_scrape_continues(self) -> None:
        exporter = self.new_exporter()
        for family in exporter_module.FINALIZED_EVENT_FAMILIES:
            seed_family(exporter.store, family)
        seed_family(exporter.store, "bleavit_chain_xcm_trapped_assets")
        exporter._release_channel = lambda *_args: exporter.store.set(  # type: ignore[method-assign]
            "bleavit_chain_release_channel_spec_version", 4
        )

        def reject_events(_block_hash: str, _block: int) -> None:
            raise MonitoringError("malformed System.Events")

        exporter._events = reject_events  # type: ignore[method-assign]
        for method in (
            "_epoch_status",
            "_proposal_state",
            "_execution_queue",
            "_oracle",
            "_welfare",
            "_treasury",
            "_market_books",
            "_mid_window_coverage",
            "_pol",
            "_collateral",
            "_migration_stall",
            "_storage_remainder",
            "_numeric_anomalies",
            "_keeper_budget",
            "_descriptor_lead_time",
            "_storage_counts",
            "_xcm_traps",
        ):
            setattr(exporter, method, lambda *_args: None)

        complete = exporter.scrape("0x01", 42)

        present = family_names(exporter.store)
        self.assertFalse(complete)
        for family in exporter_module.FINALIZED_EVENT_FAMILIES:
            self.assertNotIn(family, present)
        self.assertIn("bleavit_chain_release_channel_spec_version", present)
        self.assertIn("bleavit_chain_xcm_trapped_assets", present)
        self.assertEqual(metric_value(exporter.store, "bleavit_chain_scrape_errors_total"), 1)

    def test_each_full_domain_failure_clears_only_that_domain(self) -> None:
        method_by_domain = {
            "epoch": "_epoch_status",
            "proposal state": "_proposal_state",
            "execution queue": "_execution_queue",
            "oracle": "_oracle",
            "welfare": "_welfare",
            "treasury": "_treasury",
            "market books": "_market_books",
            "mid-window coverage": "_mid_window_coverage",
            "pol": "_pol",
            "collateral": "_collateral",
            "migration stall": "_migration_stall",
            "storage remainder": "_storage_remainder",
            "numeric anomalies": "_numeric_anomalies",
            "keeper budget": "_keeper_budget",
            "descriptor lead time": "_descriptor_lead_time",
            "storage": "_storage_counts",
            "xcm traps": "_xcm_traps",
        }
        for failed_domain, failed_method in method_by_domain.items():
            with self.subTest(domain=failed_domain):
                exporter = self.new_exporter()
                exporter._release_channel = lambda *_args: None  # type: ignore[method-assign]
                exporter._events = lambda *_args: None  # type: ignore[method-assign]
                for families in exporter_module.FULL_DOMAIN_FAMILIES.values():
                    for family in families:
                        seed_family(exporter.store, family)
                for method in method_by_domain.values():
                    setattr(exporter, method, lambda *_args: None)

                def reject(*_args: object) -> None:
                    raise MonitoringError(f"broken {failed_domain}")

                setattr(exporter, failed_method, reject)
                complete = exporter.scrape("0x01", 42)

                present = family_names(exporter.store)
                self.assertFalse(complete)
                for family in exporter_module.FULL_DOMAIN_FAMILIES[failed_domain]:
                    self.assertNotIn(family, present)
                surviving_domain = next(
                    domain
                    for domain in method_by_domain
                    if domain != failed_domain
                )
                for family in exporter_module.FULL_DOMAIN_FAMILIES[surviving_domain]:
                    self.assertIn(family, present)
                self.assertNotIn(
                    "bleavit_chain_last_successful_scrape_timestamp_seconds", present
                )
                self.assertEqual(
                    metric_value(exporter.store, "bleavit_chain_scrape_errors_total"), 1
                )


if __name__ == "__main__":
    unittest.main()

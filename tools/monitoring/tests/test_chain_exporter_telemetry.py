from __future__ import annotations

import unittest

import support  # noqa: F401 - inserts tools/monitoring on sys.path.

import chain_alerts_exporter as exporter_module
from common import MetricStore


class NoRpc:
    def call(self, method: str, params: list[object] | None = None) -> object:
        raise AssertionError(f"unexpected RPC call {method} {params}")


def samples(
    exporter: exporter_module.ChainExporter, name: str
) -> dict[tuple[tuple[str, str], ...], float]:
    return {
        labels: value
        for (series, labels), value in exporter.store.values.items()
        if series == name
    }


def some(value: object) -> dict[str, object]:
    return {"variant": "Some", "index": 1, "fields": value}


class TelemetryExporterTests(unittest.TestCase):
    def new_exporter(self) -> exporter_module.ChainExporter:
        return exporter_module.ChainExporter(
            NoRpc(), MetricStore(exporter_module.SERIES)  # type: ignore[arg-type]
        )

    def test_paired_and_labeled_telemetry_families(self) -> None:
        exporter = self.new_exporter()
        responses = {
            "market_books": some(
                [
                    {
                        "market": 7,
                        "book_loss_usdc": 11,
                        "lmsr_loss_bound_usdc": 13,
                    }
                ]
            ),
            "mid_window_coverage": some(
                [
                    {
                        "market": 7,
                        "start": 100,
                        "end": 200,
                        "coverage_percent": 95,
                    }
                ]
            ),
            "pol": some(
                [
                    {
                        "component": {"variant": "Pol", "index": 0},
                        "effective_pol_usdc": 17,
                        "pol_floor_usdc": 19,
                    },
                    {
                        "component": {"variant": "Baseline", "index": 1},
                        "effective_pol_usdc": 23,
                        "pol_floor_usdc": 29,
                    },
                ]
            ),
            "collateral": some(
                {
                    "custody_usdc": 23,
                    "liability_usdc": 21,
                    "anomalous_rounding_dust_usdc": 2,
                }
            ),
            "migration_cursor_stalled": True,
            "storage_utilization": some(
                [
                    {
                        "map": list(b"market_decision_windows"),
                        "entries": 3,
                        "bound": 8,
                    }
                ]
            ),
        }
        exporter._telemetry_api = (  # type: ignore[method-assign]
            lambda method, _block_hash: responses[method]
        )

        exporter._market_books("0x01")
        exporter._mid_window_coverage("0x01")
        exporter._pol("0x01")
        exporter._collateral("0x01")
        exporter._migration_stall("0x01")
        exporter._storage_remainder("0x01")
        exporter._numeric_anomalies("0x01")

        market_labels = (("market", "7"),)
        self.assertEqual(samples(exporter, "bleavit_market_book_loss_usdc"), {market_labels: 11})
        self.assertEqual(
            samples(exporter, "bleavit_market_lmsr_loss_bound_usdc"),
            {market_labels: 13},
        )
        window_labels = (("market", "7"), ("start", "100"), ("end", "200"))
        self.assertEqual(
            samples(exporter, "bleavit_market_mid_window_coverage_percent"),
            {window_labels: 95},
        )
        self.assertEqual(
            samples(exporter, "bleavit_market_effective_pol_usdc"),
            {
                (("component", "pol"),): 17,
                (("component", "baseline"),): 23,
            },
        )
        self.assertEqual(
            samples(exporter, "bleavit_market_pol_floor_usdc"),
            {
                (("component", "pol"),): 19,
                (("component", "baseline"),): 29,
            },
        )
        self.assertEqual(samples(exporter, "bleavit_ledger_collateral_drift_usdc"), {(): 2})
        self.assertEqual(samples(exporter, "bleavit_runtime_migration_cursor_stalled"), {(): 1})
        self.assertEqual(
            samples(exporter, "bleavit_runtime_storage_max_utilization_ratio"),
            {(("map", "market_decision_windows"),): 3 / 8},
        )
        self.assertEqual(
            samples(exporter, "bleavit_runtime_numeric_anomaly_spike"),
            {(("kind", "rounding_dust"),): 2},
        )

    def test_domain_rejection_identity_is_resolved_from_live_metadata(self) -> None:
        exporter = self.new_exporter()
        exporter.metadata = {
            "pallets": {"Market": {"index": 42, "error_type": 9}},
            "types": {
                9: {
                    "definition": {
                        "variants": [
                            {"name": "UnknownMarket", "index": 0},
                            {"name": "PriceBoundExceeded", "index": 8},
                        ]
                    }
                }
            },
        }
        module_error = {
            "variant": "Module",
            "index": 3,
            "fields": {"index": 42, "error": [8, 0, 0, 0], "message": None},
        }
        records = [
            {
                "event": {
                    "variant": "System",
                    "index": 0,
                    "fields": {
                        "variant": "ExtrinsicFailed",
                        "index": 1,
                        "fields": {"dispatch_error": module_error, "dispatch_info": None},
                    },
                }
            }
        ]
        exporter._storage = lambda *_args: records  # type: ignore[method-assign]

        exporter._events("0x01", 7)

        self.assertEqual(exporter.last_event_block, 7)
        self.assertEqual(
            samples(exporter, "bleavit_runtime_lmsr_domain_rejections_total"),
            {(): 1},
        )

        exporter._events("0x02", 8)

        self.assertEqual(exporter.last_event_block, 8)
        self.assertEqual(
            samples(exporter, "bleavit_runtime_lmsr_domain_rejections_total"),
            {(): 2},
        )


if __name__ == "__main__":
    unittest.main()

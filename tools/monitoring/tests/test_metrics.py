from __future__ import annotations

import unittest

import support  # noqa: F401 - inserts tools/monitoring on sys.path.

import attestation_monitor
import chain_alerts_exporter
from common import MetricStore, MonitoringError, SeriesDefinition


class MetricsTests(unittest.TestCase):
    def test_prometheus_text_has_help_type_labels_and_values(self) -> None:
        definitions = {
            "fixture_gauge": SeriesDefinition("fixture_gauge", "gauge", "Fixture help.", ("role",)),
            "fixture_total": SeriesDefinition("fixture_total", "counter", "Fixture counter."),
        }
        store = MetricStore(definitions)
        store.set("fixture_gauge", 3, {"role": 'a"b'})
        store.inc("fixture_total", 2)
        rendered = store.render()
        self.assertIn("# HELP fixture_gauge Fixture help.", rendered)
        self.assertIn("# TYPE fixture_total counter", rendered)
        self.assertIn('fixture_gauge{role="a\\"b"} 3', rendered)
        self.assertIn("fixture_total 2", rendered)

    def test_label_shape_is_enforced(self) -> None:
        store = MetricStore({"x": SeriesDefinition("x", "gauge", "x", ("a",))})
        with self.assertRaisesRegex(ValueError, "labels"):
            store.set("x", 1)

    def test_counter_cannot_decrement(self) -> None:
        store = MetricStore({"x_total": SeriesDefinition("x_total", "counter", "x")})
        with self.assertRaisesRegex(ValueError, "non-negative"):
            store.inc("x_total", -1)

    def test_exporter_registry_is_single_source_of_truth(self) -> None:
        self.assertTrue(chain_alerts_exporter.SERIES)
        self.assertTrue(attestation_monitor.SERIES)
        self.assertTrue(all(name.startswith("bleavit_chain_") for name in chain_alerts_exporter.SERIES))
        self.assertTrue(all(name.startswith("bleavit_release_monitor_") for name in attestation_monitor.SERIES))
        self.assertEqual(chain_alerts_exporter.SERIES["bleavit_chain_scrape_errors_total"].kind, "counter")
        self.assertEqual(attestation_monitor.SERIES["bleavit_release_monitor_integrity_ok"].kind, "gauge")

    def test_param_key_encoding_fixture(self) -> None:
        encoded = chain_alerts_exporter.encode_param_keys(["keeper.budget"])
        self.assertEqual(encoded[0], 4)
        self.assertEqual(encoded[1:], b"keeper.budget".ljust(16, b"\0"))

    def test_frozen_view_mapping_never_defaults_a_missing_integer(self) -> None:
        with self.assertRaisesRegex(MonitoringError, "no integer value"):
            chain_alerts_exporter._integer_field({}, "value", "keeper.budget ParamView")


if __name__ == "__main__":
    unittest.main()

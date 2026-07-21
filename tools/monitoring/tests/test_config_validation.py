from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

import support  # noqa: F401 - inserts tools/monitoring on sys.path.

import attestation_monitor as am
from common import MonitoringError, parse_bind


def config_text(
    gateway_count: int = 3, interval: int = 3600, minimum_signatures: int = 2
) -> str:
    gateways = []
    for index in range(gateway_count):
        gateways.append(
            f'''\n[[gateway]]
name = "g{index}"
resolve_url = "https://g{index}.example.invalid/ar-io/resolver/{{name}}"
raw_url = "https://g{index}.example.invalid/raw/{{txid}}"
tx_url = "https://g{index}.example.invalid/{{txid}}/{{path}}"
name_url = "https://{{name}}.g{index}.example.invalid/{{path}}"
'''
        )
    return f'''[monitor]
node_urls = ["wss://node.example.invalid"]
arns_name = "futarchy"
keyring_file = "keyring.toml"
bind = "127.0.0.1:9618"
check_interval_seconds = {interval}
minimum_release_signatures = {minimum_signatures}
max_file_bytes = 1000
max_bundle_bytes = 10000
{''.join(gateways)}
[webhooks]
paging = ["https://paging.example.invalid"]
status_page = ["https://status.example.invalid"]
community = ["https://community.example.invalid"]
'''


class ConfigValidationTests(unittest.TestCase):
    def load(self, text: str) -> am.Config:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "monitor.toml"
            path.write_text(text, encoding="utf-8")
            return am.load_config(path)

    def test_three_independent_gateways_are_accepted(self) -> None:
        config = self.load(config_text())
        self.assertEqual(len(config.gateways), 3)
        self.assertEqual(config.check_interval_seconds, 3600)

    def test_fewer_than_three_gateways_fail(self) -> None:
        with self.assertRaisesRegex(MonitoringError, "at least three"):
            self.load(config_text(2))

    def test_hourly_floor_is_enforced(self) -> None:
        with self.assertRaisesRegex(MonitoringError, "hourly"):
            self.load(config_text(interval=3601))

    def test_missing_gateway_template_placeholder_fails(self) -> None:
        broken = config_text().replace("/raw/{txid}", "/raw/fixed", 1)
        with self.assertRaisesRegex(MonitoringError, "placeholders"):
            self.load(broken)

    def test_operator_signature_minimum_is_required(self) -> None:
        broken = config_text().replace("minimum_release_signatures = 2\n", "")
        with self.assertRaisesRegex(MonitoringError, "operator-supplied"):
            self.load(broken)

    def test_single_release_signature_is_below_the_12_1_4_floor(self) -> None:
        with self.assertRaisesRegex(MonitoringError, r"release-signature floor"):
            self.load(config_text(minimum_signatures=1))

    def test_spec_floor_of_two_release_signatures_is_accepted(self) -> None:
        config = self.load(config_text(minimum_signatures=2))
        self.assertEqual(config.minimum_release_signatures, 2)

    def test_deployment_may_require_more_than_the_floor(self) -> None:
        config = self.load(config_text(minimum_signatures=3))
        self.assertEqual(config.minimum_release_signatures, 3)

    def test_bind_parser_is_crisp(self) -> None:
        self.assertEqual(parse_bind("127.0.0.1:9618"), ("127.0.0.1", 9618))
        with self.assertRaisesRegex(MonitoringError, "HOST:PORT"):
            parse_bind("missing-port")


if __name__ == "__main__":
    unittest.main()

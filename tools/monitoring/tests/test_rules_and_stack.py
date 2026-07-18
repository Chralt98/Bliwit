from __future__ import annotations

import unittest
from pathlib import Path

import yaml


ROOT = Path(__file__).resolve().parents[3]
RULES = ROOT / "deploy" / "monitoring" / "prometheus" / "rules" / "bleavit-alerts.yml"
EXPECTED_EXPRESSIONS = {
    "BleavitEpochTickLag": "bleavit_chain_tick_lag_blocks > 600",
    "BleavitProposalQueueAtBound": '(bleavit_chain_storage_map_bound{pallet="Epoch",item="IntakeProposals"} > 0) and (bleavit_chain_storage_map_entries{pallet="Epoch",item="IntakeProposals"} >= bleavit_chain_storage_map_bound{pallet="Epoch",item="IntakeProposals"})',
    "BleavitMarketBookLoss": "bleavit_market_book_loss_usdc > (0.9 * bleavit_market_lmsr_loss_bound_usdc)",
    "BleavitTwapCoverageLowMidWindow": "bleavit_market_mid_window_coverage_percent < 96",
    "BleavitLiquidityFloorDisturbed": "bleavit_market_effective_pol_usdc < bleavit_market_pol_floor_usdc",
    "BleavitOracleRoundThreeOpened": "bleavit_chain_oracle_max_round_depth >= 3",
    "BleavitCollateralizationDrift": "bleavit_ledger_collateral_drift_usdc != 0",
    "BleavitTreasuryMeterHigh": "bleavit_chain_treasury_meter_utilization_bps > 8000",
    "BleavitXcmAssetTrap": "bleavit_chain_xcm_trapped_assets > 0",
    "BleavitKeeperInactive": "(time() - bleavit_keeper_last_successful_crank_timestamp_seconds) > 3600 and bleavit_keeper_last_successful_crank_timestamp_seconds > 0 and bleavit_keeper_planned_total > 0 and on() bleavit_keeper_connected == 1",
    "BleavitGuardianAction": "increase(bleavit_chain_guardian_actions_total[5m]) > 0",
    "BleavitMigrationCursorStalled": "bleavit_runtime_migration_cursor_stalled > 0",
    "BleavitStorageNearBound": "max(bleavit_chain_storage_map_entries / bleavit_chain_storage_map_bound) > 0.8 or bleavit_runtime_storage_max_utilization_ratio > 0.8",
    "BleavitNumericsAnomalySpike": "increase(bleavit_runtime_lmsr_domain_rejections_total[5m]) > 0 or bleavit_runtime_numeric_anomaly_spike > 0",
    "BleavitBootnodeCommitment": 'sum(bleavit_bootnode_browser_dial_success) < 8 or sum(bleavit_bootnode_browser_dial_success{port="443"}) < 2 or min(bleavit_bootnode_wss_certificate_days_remaining) < 14',
    "BleavitServedStateWindowShort": "max(bleavit_bootnode_served_state_retention_days) < 30",
    "BleavitReleaseIntegrity": "bleavit_release_monitor_bundle_byte_mismatches > 0 or bleavit_release_monitor_resolver_divergent_gateways >= 2 or bleavit_release_monitor_integrity_ok == 0",
    "BleavitDescriptorLeadTimeUncovered": "(bleavit_chain_pending_upgrade_age_blocks > (0.5 * bleavit_chain_descriptor_lead_time_blocks)) and on() (bleavit_release_monitor_covering_release == 0)",
    "BleavitReleaseChannelLagOrSecurityFlip": "bleavit_release_monitor_repoint_channel_lag_blocks > 600 or increase(bleavit_chain_release_channel_security_flips_total[5m]) > 0",
    "BleavitKeeperBudgetHigh": "bleavit_chain_keeper_budget_utilization_ratio > 0.8",
}


class RuleAndStackTests(unittest.TestCase):
    def test_exactly_twenty_structured_alert_rules(self) -> None:
        document = yaml.safe_load(RULES.read_text(encoding="utf-8"))
        rules = [rule for group in document["groups"] for rule in group["rules"]]
        self.assertEqual(len(rules), 20)
        self.assertEqual(len({rule["alert"] for rule in rules}), 20)
        self.assertEqual(len({rule["labels"]["domain"] for rule in rules}), 20)
        for rule in rules:
            self.assertIn("expr", rule)
            self.assertRegex(rule["labels"]["runbook"], r"^RB-[A-Z]+$")
            self.assertIn("key_series", rule["annotations"])
            self.assertIn("threshold", rule["annotations"])

    def test_only_spec_page_rows_are_immediate_pages(self) -> None:
        document = yaml.safe_load(RULES.read_text(encoding="utf-8"))
        page_domains = {
            rule["labels"]["domain"]
            for group in document["groups"]
            for rule in group["rules"]
            if rule["labels"]["severity"] == "page"
        }
        self.assertEqual(page_domains, {"Collateralization", "Release integrity"})

    def test_all_alert_expressions_are_exactly_pinned(self) -> None:
        document = yaml.safe_load(RULES.read_text(encoding="utf-8"))
        expressions = {
            rule["alert"]: rule["expr"]
            for group in document["groups"]
            for rule in group["rules"]
        }
        self.assertEqual(expressions, EXPECTED_EXPRESSIONS)

    def test_keeper_inactivity_has_no_second_for_delay(self) -> None:
        document = yaml.safe_load(RULES.read_text(encoding="utf-8"))
        keeper_rule = next(
            rule
            for group in document["groups"]
            for rule in group["rules"]
            if rule["alert"] == "BleavitKeeperInactive"
        )
        self.assertNotIn("for", keeper_rule)

    def test_prometheus_scrapes_all_o5_jobs_and_keeps_o3_commented(self) -> None:
        path = ROOT / "deploy" / "monitoring" / "prometheus" / "prometheus.yml"
        text = path.read_text(encoding="utf-8")
        document = yaml.safe_load(text)
        jobs = {job["job_name"]: job for job in document["scrape_configs"]}
        self.assertEqual(
            set(jobs),
            {
                "bleavit-collators",
                "bleavit-keeper",
                "bleavit-chain-alerts",
                "bleavit-attestation-monitor",
            },
        )
        self.assertIn("keeper-1.example.invalid:9616", str(jobs["bleavit-keeper"]))
        self.assertIn("# - job_name: bleavit-browser-dial-probes", text)
        self.assertIn("operator-supplied placeholder", text)

    def test_alertmanager_routes_page_and_release_channels(self) -> None:
        path = ROOT / "deploy" / "monitoring" / "alertmanager" / "alertmanager.yml"
        text = path.read_text(encoding="utf-8")
        document = yaml.safe_load(text)
        routes = document["route"]["routes"]
        receivers = {receiver["name"] for receiver in document["receivers"]}
        self.assertEqual(
            receivers,
            {"ops-channel", "paging", "status-page", "community-channel"},
        )
        self.assertTrue(any(route["receiver"] == "paging" and route.get("continue") for route in routes))
        self.assertTrue(any(route["receiver"] == "status-page" and route.get("continue") for route in routes))
        self.assertTrue(any(route["receiver"] == "community-channel" for route in routes))
        self.assertIn("example.invalid", text)


if __name__ == "__main__":
    unittest.main()

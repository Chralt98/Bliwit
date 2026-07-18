from __future__ import annotations

import json
import re
import struct
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
MANIFEST = ROOT / "surface-manifest.json"
CONTRACT = ROOT.parents[1] / "docs" / "architecture" / "02-integration-contract.md"
MASK = (1 << 64) - 1
P1 = 11400714785074694791
P2 = 14029467366897019727
P3 = 1609587929392839161
P4 = 9650029242287828579
P5 = 2870177450012600261


def rotl(value: int, count: int) -> int:
    return ((value << count) | (value >> (64 - count))) & MASK


def lane_round(accumulator: int, lane: int) -> int:
    accumulator = (accumulator + lane * P2) & MASK
    accumulator = rotl(accumulator, 31)
    return (accumulator * P1) & MASK


def independent_xxh64(data: bytes, seed: int) -> int:
    """Independent test-side XXH64 derivation for the frozen raw key."""
    cursor = 0
    if len(data) >= 32:
        state = [
            (seed + P1 + P2) & MASK,
            (seed + P2) & MASK,
            seed & MASK,
            (seed - P1) & MASK,
        ]
        while cursor <= len(data) - 32:
            for index in range(4):
                state[index] = lane_round(
                    state[index], struct.unpack_from("<Q", data, cursor + index * 8)[0]
                )
            cursor += 32
        value = sum(rotl(item, count) for item, count in zip(state, (1, 7, 12, 18))) & MASK
        for item in state:
            value ^= lane_round(0, item)
            value = (value * P1 + P4) & MASK
    else:
        value = (seed + P5) & MASK
    value = (value + len(data)) & MASK
    while cursor + 8 <= len(data):
        value ^= lane_round(0, struct.unpack_from("<Q", data, cursor)[0])
        value = (rotl(value, 27) * P1 + P4) & MASK
        cursor += 8
    if cursor + 4 <= len(data):
        value ^= struct.unpack_from("<I", data, cursor)[0] * P1 & MASK
        value = (rotl(value, 23) * P2 + P3) & MASK
        cursor += 4
    for byte in data[cursor:]:
        value ^= byte * P5 & MASK
        value = rotl(value, 11) * P1 & MASK
    value ^= value >> 33
    value = value * P2 & MASK
    value ^= value >> 29
    value = value * P3 & MASK
    value ^= value >> 32
    return value & MASK


def independent_twox128(text: str) -> bytes:
    data = text.encode()
    return independent_xxh64(data, 0).to_bytes(8, "little") + independent_xxh64(
        data, 1
    ).to_bytes(8, "little")


class SurfaceManifestTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.manifest = json.loads(MANIFEST.read_text(encoding="utf-8"))
        cls.entries = cls.manifest["entries"]

    def test_schema_and_entry_shapes(self) -> None:
        self.assertEqual(self.manifest["schema"], "bleavit.critical-surface.v1")
        self.assertEqual(self.manifest["integration_contract_version"], 4)
        identifiers = [entry["id"] for entry in self.entries]
        self.assertEqual(len(identifiers), len(set(identifiers)))
        for entry in self.entries:
            self.assertIn(
                entry["kind"],
                {"runtime_api", "storage", "constant", "event", "raw_storage", "properties"},
            )
            self.assertIs(type(entry["required"]), bool)
            self.assertRegex(entry["citation"], r"^02 §")
            if "blocked_by" in entry:
                self.assertIsInstance(entry["blocked_by"], str)
                self.assertTrue(entry["blocked_by"])

    def test_frozen_storage_and_event_rows_have_layout_expectations(self) -> None:
        # Wired surface carries frozen layout expectations; unwired surface
        # must not (guessed renderings would false-alarm when the owning
        # milestone lands and freezes them from the real runtime).
        for entry in self.entries:
            if entry["kind"] not in {"storage", "event"}:
                continue
            if "blocked_by" in entry:
                self.assertNotIn("layout", entry, entry["id"])
            else:
                self.assertIn("layout", entry, entry["id"])
        version = next(
            entry
            for entry in self.entries
            if entry["id"] == "constant.identity.contract_version"
        )
        self.assertEqual(version["layout"], {"type": "u32", "value": "0x04000000"})

    def test_section_six_ledger_events_and_section_seven_attestor_storage_are_exact(self) -> None:
        expected_ledger_events = {
            "Split",
            "Merged",
            "ScalarSplit",
            "ScalarMerged",
            "GateSplit",
            "GateMerged",
            "PositionTransferred",
            "BaselineSplit",
            "BaselineMerged",
            "VaultResolved",
            "VaultVoided",
            "ScalarSettlementSet",
            "GateSettled",
            "BaselineSettled",
            "Redeemed",
            "ScalarRedeemed",
            "ScalarPairRedeemed",
            "GateRedeemed",
            "VoidRedeemed",
            "BaselineRedeemed",
            "VaultReaped",
            "BaselineVaultReaped",
        }
        expected_attestor_storage = {
            "Members",
            "Attestations",
            "NextAttestationId",
        }

        contract = CONTRACT.read_text(encoding="utf-8")
        ledger_row = next(
            line
            for line in contract.splitlines()
            if line.startswith("| `pallet-conditional-ledger` |")
        )
        contract_ledger_events = {
            declaration.split(maxsplit=1)[0]
            for declaration in re.findall(r"`([^`]+)`", ledger_row.split("|", 2)[2])
        }
        attestor_section = contract.split("### 7.5 `pallet-attestor`", 1)[1].split(
            "\n---\n", 1
        )[0]
        contract_attestor_storage = set(
            re.findall(r"^\| `([^`]+)` \|", attestor_section, flags=re.MULTILINE)
        )

        manifest_ledger_events = {
            entry["event"]
            for entry in self.entries
            if entry["kind"] == "event" and entry.get("pallet") == "ConditionalLedger"
        }
        manifest_attestor_storage = {
            entry["item"]
            for entry in self.entries
            if entry["kind"] == "storage" and entry.get("pallet") == "Attestor"
        }
        self.assertEqual(contract_ledger_events, expected_ledger_events)
        self.assertEqual(manifest_ledger_events, expected_ledger_events)
        self.assertEqual(contract_attestor_storage, expected_attestor_storage)
        self.assertEqual(manifest_attestor_storage, expected_attestor_storage)

    def test_newly_wired_v4_constant_layouts_are_frozen(self) -> None:
        expected = {
            "constant.ledger.min_transfer": (
                "MinTransfer",
                {"type": "u128", "value": "0x10270000000000000000000000000000"},
            ),
            "constant.market.min_trade": (
                "MinTrade",
                {"type": "u128", "value": "0x40420f00000000000000000000000000"},
            ),
            "constant.market.max_trade_ratio": (
                "MaxTradeRatio",
                {"type": "(u32,u32)", "value": "0x0100000004000000"},
            ),
            "constant.market.max_live_markets": (
                "MaxLiveMarkets",
                {"type": "u32", "value": "0xc4000000"},
            ),
            "constant.market.gate_p_max_ceiling": (
                "GatePMaxCeiling",
                {
                    "type": "futarchy_primitives::FixedU64(u64)",
                    "value": "0x00e1f50500000000",
                },
            ),
            "constant.market.gate_eps_floor": (
                "GateEpsFloor",
                {
                    "type": "futarchy_primitives::FixedU64(u64)",
                    "value": "0x404b4c0000000000",
                },
            ),
            "constant.execution_guard.timelock_floor": (
                "ExecutionTimelockFloor",
                {
                    "type": "[u32;4]",
                    "value": "0x40380000403800004038000040380000",
                },
            ),
            "constant.execution_guard.grace_floor": (
                "ExecutionGraceFloor",
                {"type": "u32", "value": "0xc0890100"},
            ),
            "constant.execution_guard.descriptor_lead_time": (
                "DescriptorLeadTime",
                {"type": "u32", "value": "0xc0a80000"},
            ),
            "constant.epoch.max_intake_queue": (
                "MaxIntakeQueue",
                {"type": "u32", "value": "0x40000000"},
            ),
            "constant.epoch.max_live_proposals": (
                "MaxLiveProposals",
                {"type": "u32", "value": "0x20000000"},
            ),
            "constant.epoch.recent_cohorts": (
                "RecentCohortSummariesBound",
                {"type": "u32", "value": "0x20000000"},
            ),
            "constant.epoch.books_per_proposal": (
                "MaxBooksPerProposal",
                {"type": "u32", "value": "0x06000000"},
            ),
            "constant.epoch.phase_offsets": (
                "PhaseOffsets",
                {
                    "type": "[(u32,u32);7]",
                    "value": "0x00000000150000000300000015000000040000001500000005000000150000000f0000001500000012000000150000001400000015000000",
                },
            ),
            "constant.decision.window_floor": (
                "DecisionWindowFloor",
                {"type": "u32", "value": "0x40380000"},
            ),
            "constant.decision.extension": (
                "DecisionExtension",
                {"type": "u32", "value": "0xc0a80000"},
            ),
            "constant.decision.delta_floors": (
                "DecisionDeltaFloors",
                {
                    "type": "[futarchy_primitives::FixedU64(u64);4]",
                    "value": "0x404b4c0000000000404b4c0000000000404b4c0000000000404b4c0000000000",
                },
            ),
            "constant.decision.sigma_floors": (
                "DecisionSigmaFloors",
                {
                    "type": "[futarchy_primitives::FixedU64(u64);4]",
                    "value": "0x0000000000000000000000000000000000000000000000000000000000000000",
                },
            ),
            "constant.epoch.length_floor": (
                "MinEpochLength",
                {"type": "u32", "value": "0x80130300"},
            ),
        }
        by_id = {entry["id"]: entry for entry in self.entries}
        for identifier, (constant, layout) in expected.items():
            entry = by_id[identifier]
            self.assertEqual(entry["constant"], constant)
            self.assertEqual(entry["layout"], layout)
            self.assertNotIn("blocked_by", entry)

    def test_no_stale_execution_guard_wiring_blockers(self) -> None:
        offenders = [
            entry["id"]
            for entry in self.entries
            if "A11 pallet-execution-guard runtime wiring" in entry.get("blocked_by", "")
        ]
        self.assertEqual(offenders, [])

    def test_b2_api_and_epoch_constant_wiring_blockers_are_cleared(self) -> None:
        runtime_apis = [
            entry for entry in self.entries if entry["kind"] == "runtime_api"
        ]
        self.assertEqual(len(runtime_apis), 11)
        for entry in runtime_apis:
            self.assertNotIn("blocked_by", entry, entry["id"])
            # Runtime API layout is resolved from released metadata; this
            # manifest must not guess a portable-registry rendering.
            self.assertNotIn("layout", entry, entry["id"])

        epoch_constants = [
            entry
            for entry in self.entries
            if entry["kind"] == "constant" and entry.get("pallet") == "Epoch"
        ]
        self.assertTrue(epoch_constants)
        for entry in epoch_constants:
            self.assertNotIn("blocked_by", entry, entry["id"])
            self.assertIn("layout", entry, entry["id"])

        stale = [
            entry["id"]
            for entry in self.entries
            if "A8 pallet-epoch runtime wiring" in entry.get("blocked_by", "")
            or "B2 FutarchyApi runtime wiring" in entry.get("blocked_by", "")
        ]
        self.assertEqual(stale, [])
        # Cross-surface compliance gaps fail the release closed here, because a
        # surface that records successfully cannot be gated by a per-entry
        # `blocked_by` (the assembler only reads that for missing recordings).
        self.assertEqual(
            self.manifest["release_blockers"],
            [
                {
                    "id": "b1b.compliance",
                    "owner": "B1b",
                    "reason": "SQ-173..SQ-175, SQ-177, SQ-180..SQ-182 remain open (fail-closed adoption-input backing gaps; per-SQ owners)",
                },
                {
                    "id": "treasury.reserve_health_unwired",
                    "owner": "B1a",
                    "reason": (
                        "SQ-205: oracle ReserveHealth never reaches treasury "
                        "set_reserve_impaired — 08 §1.2 fail-static NAV is not enforced"
                    ),
                },
            ],
        )

    def test_wired_epoch_surfaces_carry_no_per_entry_blocker(self) -> None:
        # Per-entry `blocked_by` explains why a surface cannot be recorded; it
        # is inert once the surface records. Epoch storage/events record as
        # soon as the pallet is in the metadata (B1b wired it at index 61), so
        # a compliance gap that spans recorded surfaces must fail the release
        # through `release_blockers`, never through a label the assembler will
        # not read.
        recorded_epoch = [
            entry
            for entry in self.entries
            if entry["kind"] in ("storage", "event")
            and entry["id"].split(".")[1] == "epoch"
        ]
        self.assertTrue(recorded_epoch)
        for entry in recorded_epoch:
            self.assertNotIn("blocked_by", entry, entry["id"])

    def test_remaining_surface_blockers_are_attributed_to_open_gaps(self) -> None:
        # Every critical surface now records: B2 wired the FutarchyApi and the
        # metadata constants, A8/A11 wired their pallets, and SQ-101 re-keyed
        # USDC to the frozen 02 §8 Location. The remaining release-blocking
        # gaps span *recorded* surface, so they live in `release_blockers`
        # (which the assembler always reads) rather than in a per-entry
        # `blocked_by` (which it only reads for a missing recording).
        blockers = {
            entry["blocked_by"]
            for entry in self.entries
            if "blocked_by" in entry
        }
        self.assertEqual(blockers, set())

    def test_no_speculative_layout_on_blocked_entries(self) -> None:
        offenders = [
            entry["id"]
            for entry in self.entries
            if entry.get("blocked_by") and "layout" in entry
        ]
        self.assertEqual(offenders, [])

    def test_release_channel_is_metadata_independent(self) -> None:
        entry = next(
            item
            for item in self.entries
            if item["id"] == "storage.constitution.release_channel"
        )
        self.assertEqual(entry["kind"], "raw_storage")
        self.assertNotIn("layout", entry)
        self.assertIs(entry["value_optional"], True)

    def test_usdc_identity_exact_keys_and_expected_values(self) -> None:
        # SCALE bytes of the 02 §8 Location {parents: 1,
        # X3(Parachain(1000), PalletInstance(50), GeneralIndex(1337))}:
        location = bytes.fromhex("010300a10f043205e514")
        import hashlib

        def exact_key(item: str) -> str:
            return "0x" + (
                independent_twox128("ForeignAssets")
                + independent_twox128(item)
                + hashlib.blake2b(location, digest_size=16).digest()
                + location
            ).hex()

        asset = next(e for e in self.entries if e["id"] == "storage.identity.usdc_asset")
        metadata = next(
            e for e in self.entries if e["id"] == "storage.identity.usdc_metadata"
        )
        self.assertEqual(asset["exact_key"], exact_key("Asset"))
        self.assertEqual(metadata["exact_key"], exact_key("Metadata"))
        self.assertEqual(asset["expected"], {"min_balance": 10000})
        self.assertEqual(metadata["expected"], {"decimals": 6})
        for entry in (asset, metadata):
            self.assertNotIn("blocked_by", entry)

    def test_chain_identity_properties_entry(self) -> None:
        entry = next(
            item for item in self.entries if item["id"] == "properties.chain_identity"
        )
        self.assertEqual(entry["kind"], "properties")
        self.assertIs(entry["required"], True)
        self.assertEqual(
            entry["expected"],
            {"ss58Format": 7777, "tokenDecimals": 12, "tokenSymbol": "VIT"},
        )

    def test_foreign_assets_are_expected_to_be_location_keyed(self) -> None:
        for identifier in (
            "storage.foreign_assets.account",
            "storage.identity.usdc_asset",
            "storage.identity.usdc_metadata",
        ):
            entry = next(item for item in self.entries if item["id"] == identifier)
            self.assertIn("staging_xcm::v5::location::Location", entry["layout"]["key"])
            self.assertNotIn("blocked_by", entry)

    def test_section_nine_constant_surface_is_complete(self) -> None:
        identifiers = {entry["id"] for entry in self.entries}
        expected = {
            "constant.decision.window_floor",
            "constant.decision.extension",
            "constant.decision.delta_floors",
            "constant.decision.sigma_floors",
            "constant.execution_guard.timelock_floor",
            "constant.execution_guard.grace_floor",
            "constant.market.gate_p_max_ceiling",
            "constant.market.gate_eps_floor",
            "constant.epoch.length_floor",
            "storage.identity.usdc_asset",
            "storage.identity.usdc_metadata",
        }
        self.assertTrue(expected <= identifiers, sorted(expected - identifiers))
        self.assertTrue(
            {
                "constant.epoch.intake_rate_limit",
                "constant.epoch.slots_bounds",
                "constant.market.gate_p_max",
                "constant.market.gate_eps",
            }.isdisjoint(identifiers)
        )

    def test_all_eleven_runtime_api_methods_are_present(self) -> None:
        methods = {
            entry["method"]
            for entry in self.entries
            if entry["kind"] == "runtime_api"
        }
        self.assertEqual(
            methods,
            {
                "epoch_status",
                "proposal_summaries",
                "quote",
                "decision_stats",
                "account_positions",
                "execution_queue",
                "welfare_current",
                "params",
                "nav",
                "recent_cohorts",
                "open_oracle_rounds",
            },
        )

    def test_release_channel_key_is_independently_derived(self) -> None:
        entry = next(
            item
            for item in self.entries
            if item["id"] == "storage.constitution.release_channel"
        )
        expected = independent_twox128("Constitution") + independent_twox128(
            "ReleaseChannel"
        )
        self.assertEqual(entry["raw_key"], "0x" + expected.hex())
        self.assertEqual(
            entry["raw_key"],
            "0xfb8ccbf677a3d2ce27ab85165f32df6afec7194a5368a58e1f6bf57457134a6c",
        )


if __name__ == "__main__":
    unittest.main()

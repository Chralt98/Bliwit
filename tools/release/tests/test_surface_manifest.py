from __future__ import annotations

import json
import struct
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
MANIFEST = ROOT / "surface-manifest.json"
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
        self.assertEqual(self.manifest["integration_contract_version"], 3)
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
        # milestone lands and freezes them from the real runtime). The SQ-101
        # trio is the deliberate exception: its Location-keyed expectation is
        # the strict-fail driver against today's u32-keyed runtime.
        sq101 = {
            "storage.foreign_assets.account",
            "storage.identity.usdc_asset",
            "storage.identity.usdc_metadata",
        }
        for entry in self.entries:
            if entry["kind"] not in {"storage", "event"}:
                continue
            if entry["id"] in sq101:
                self.assertIn("layout", entry, entry["id"])
            elif "blocked_by" in entry:
                self.assertNotIn("layout", entry, entry["id"])
            else:
                self.assertIn("layout", entry, entry["id"])
        version = next(
            entry
            for entry in self.entries
            if entry["id"] == "constant.identity.contract_version"
        )
        self.assertEqual(version["layout"], {"type": "u32", "value": "0x03000000"})

    def test_no_speculative_layout_on_blocked_entries(self) -> None:
        sq101 = {
            "storage.foreign_assets.account",
            "storage.identity.usdc_asset",
            "storage.identity.usdc_metadata",
        }
        offenders = [
            entry["id"]
            for entry in self.entries
            if entry.get("blocked_by") and "layout" in entry and entry["id"] not in sq101
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
            self.assertEqual(entry["blocked_by"], "SQ-101 (B4 follow-up)")

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
            self.assertEqual(entry["blocked_by"], "SQ-101 (B4 follow-up)")

    def test_section_nine_constant_surface_is_complete(self) -> None:
        identifiers = {entry["id"] for entry in self.entries}
        expected = {
            "constant.epoch.intake_rate_limit",
            "constant.decision.window_floor",
            "constant.decision.trailing_floor",
            "constant.decision.extension",
            "constant.decision.delta_floors",
            "constant.decision.sigma_floors",
            "constant.decision.coverage_floor",
            "constant.decision.v_min_floors",
            "constant.execution_guard.timelock_floor",
            "constant.execution_guard.grace_floor",
            "constant.market.gate_eps",
            "constant.epoch.length_floor",
            "constant.epoch.slots_bounds",
            "storage.identity.usdc_asset",
            "storage.identity.usdc_metadata",
        }
        self.assertTrue(expected <= identifiers, sorted(expected - identifiers))

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

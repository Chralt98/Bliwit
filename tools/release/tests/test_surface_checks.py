from __future__ import annotations

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from surface_checks import (
    RELEASE_CHANNEL_MIN_LENGTH,
    USDC_LOCATION_HEX,
    check_expected_value,
    decode_asset_details_min_balance,
    decode_asset_metadata_decimals,
    decode_compact_len,
    exact_storage_key,
    nonempty_hex,
    properties_match,
    storage_value_from_chainhead,
    validate_release_channel,
)


def compact(value: int) -> bytes:
    if value < 1 << 6:
        return bytes([value << 2])
    if value < 1 << 14:
        return ((value << 2) | 0b01).to_bytes(2, "little")
    return ((value << 2) | 0b10).to_bytes(4, "little")


class UsdcLocationTests(unittest.TestCase):
    def test_location_scale_bytes_are_the_02_section8_key(self) -> None:
        # parents 1, X3(Parachain(1000), PalletInstance(50), GeneralIndex(1337))
        expected = (
            bytes([1, 3])
            + bytes([0])
            + compact(1000)
            + bytes([4, 50])
            + bytes([5])
            + compact(1337)
        )
        self.assertEqual(USDC_LOCATION_HEX, expected.hex())

    def test_exact_key_embeds_blake2_concat(self) -> None:
        key = exact_storage_key("ForeignAssets", "Asset", USDC_LOCATION_HEX)
        self.assertTrue(key.startswith("0x"))
        self.assertTrue(key.endswith(USDC_LOCATION_HEX))
        # twox128 ++ twox128 ++ blake2_128 ++ 10 location bytes
        self.assertEqual(len(bytes.fromhex(key[2:])), 16 + 16 + 16 + 10)


class ReleaseChannelTests(unittest.TestCase):
    def test_absent_value_ok_only_when_optional(self) -> None:
        self.assertTrue(validate_release_channel(None, True)[0])
        self.assertFalse(validate_release_channel(None, False)[0])

    def test_valid_168_byte_layout(self) -> None:
        raw = bytes([1]) + bytes(RELEASE_CHANNEL_MIN_LENGTH - 1)
        ok, reason = validate_release_channel("0x" + raw.hex(), False)
        self.assertTrue(ok, reason)

    def test_appended_fields_stay_valid(self) -> None:
        raw = bytes([1]) + bytes(RELEASE_CHANNEL_MIN_LENGTH + 7)
        self.assertTrue(validate_release_channel("0x" + raw.hex(), False)[0])

    def test_short_value_fails(self) -> None:
        raw = bytes([1]) + bytes(90)
        ok, reason = validate_release_channel("0x" + raw.hex(), True)
        self.assertFalse(ok)
        self.assertIn("02 §12", reason)

    def test_schema_bump_stays_valid_per_02_section12(self) -> None:
        # "any other value ⇒ layout extended append-only, prefix still valid":
        # a future schema-2 value must never block recording or releases.
        raw = bytes([2]) + bytes(RELEASE_CHANNEL_MIN_LENGTH + 15)
        ok, reason = validate_release_channel("0x" + raw.hex(), False)
        self.assertTrue(ok, reason)
        self.assertIn("schema 2", reason)

    def test_schema_bump_still_requires_the_v1_prefix(self) -> None:
        raw = bytes([2]) + bytes(40)
        self.assertFalse(validate_release_channel("0x" + raw.hex(), False)[0])


class StorageValueTests(unittest.TestCase):
    def test_extracts_value_from_operation_items(self) -> None:
        response = {
            "events": [
                {"event": "operationBodyDone"},
                {
                    "event": "operationStorageItems",
                    "items": [{"key": "0x00", "value": "0x1234"}],
                },
                {"event": "operationStorageDone"},
            ]
        }
        self.assertEqual(storage_value_from_chainhead(response), "0x1234")

    def test_done_without_items_yields_none(self) -> None:
        response = {"events": [{"event": "operationStorageDone"}]}
        self.assertIsNone(storage_value_from_chainhead(response))

    def test_nonempty_hex(self) -> None:
        self.assertTrue(nonempty_hex("0x00"))
        self.assertFalse(nonempty_hex("0x"))
        self.assertFalse(nonempty_hex(None))
        self.assertFalse(nonempty_hex({"error": "x"}))


class AssetDecodeTests(unittest.TestCase):
    def test_compact_lengths(self) -> None:
        self.assertEqual(decode_compact_len(compact(4) + b"abcd", 0), (4, 1))
        self.assertEqual(decode_compact_len(compact(1000), 0), (1000, 2))

    def test_metadata_decimals(self) -> None:
        value = (
            (10**8).to_bytes(16, "little")  # deposit
            + compact(4)
            + b"USDC"  # name
            + compact(4)
            + b"USDC"  # symbol
            + bytes([6])  # decimals
            + bytes([0])  # is_frozen
        )
        self.assertEqual(decode_asset_metadata_decimals("0x" + value.hex()), 6)

    def test_details_min_balance(self) -> None:
        value = (
            bytes(32) * 4  # owner/issuer/admin/freezer
            + (10**12).to_bytes(16, "little")  # supply
            + (10**8).to_bytes(16, "little")  # deposit
            + (10_000).to_bytes(16, "little")  # min_balance
            + bytes([1])  # is_sufficient
            + (7).to_bytes(4, "little")
            + (7).to_bytes(4, "little")
            + (0).to_bytes(4, "little")
            + bytes([0])
        )
        self.assertEqual(decode_asset_details_min_balance("0x" + value.hex()), 10_000)

    def test_check_expected_value_verdicts(self) -> None:
        entry = {"expected": {"decimals": 6}}
        good = (
            (0).to_bytes(16, "little") + compact(1) + b"U" + compact(1) + b"U" + bytes([6, 0])
        )
        bad = (
            (0).to_bytes(16, "little") + compact(1) + b"U" + compact(1) + b"U" + bytes([9, 0])
        )
        self.assertTrue(check_expected_value(entry, "0x" + good.hex())[0])
        ok, reason = check_expected_value(entry, "0x" + bad.hex())
        self.assertFalse(ok)
        self.assertIn("02 §8", reason)
        ok, reason = check_expected_value(entry, None)
        self.assertFalse(ok)
        self.assertIn("absent", reason)


class PropertiesTests(unittest.TestCase):
    EXPECTED = {"ss58Format": 7777, "tokenDecimals": 12, "tokenSymbol": "VIT"}

    def test_exact_match_passes(self) -> None:
        actual = {**self.EXPECTED, "extra": "ignored"}
        self.assertTrue(properties_match(actual, self.EXPECTED)[0])

    def test_wrong_value_fails_with_key_in_reason(self) -> None:
        actual = {**self.EXPECTED, "ss58Format": 42}
        ok, reason = properties_match(actual, self.EXPECTED)
        self.assertFalse(ok)
        self.assertIn("ss58Format", reason)

    def test_non_object_fails(self) -> None:
        self.assertFalse(properties_match(None, self.EXPECTED)[0])


if __name__ == "__main__":
    unittest.main()

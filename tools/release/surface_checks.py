#!/usr/bin/env python3
"""Pure value-level checks for critical-surface fixtures (02 §8/§12).

These helpers assert the *values* the integration contract freezes, not just
name presence: the 02 §12 ReleaseChannel raw layout, the 02 §8 USDC identity
(exact Location key, decimals, min_balance) and the chain-spec `properties`
identity. They are deliberately metadata-independent so the checks hold on the
surfaces the contract defines that way.
"""

from __future__ import annotations

import hashlib
from typing import Any

from release_common import twox128

# SCALE encoding of the 02 §8 USDC asset key:
# Location { parents: 1, interior: X3(Parachain(1000), PalletInstance(50),
# GeneralIndex(1337)) } under staging-xcm v5 —
#   parents:        0x01
#   Junctions::X3:  0x03
#   Parachain:      0x00 + Compact(1000) = a1 0f
#   PalletInstance: 0x04 + 0x32
#   GeneralIndex:   0x05 + Compact(1337) = e5 14
USDC_LOCATION_HEX = "010300a10f043205e514"

# 02 §8: "USDC decimals 6 (preserved from Asset Hub); min_balance = 10^4 (1 cent)".
USDC_EXPECTED_DECIMALS = 6
USDC_EXPECTED_MIN_BALANCE = 10_000

# 02 §12: fixed-layout raw value, 168 bytes at v1.0 baseline. The schema byte
# is 1 today, and "any other value ⇒ layout extended append-only, prefix still
# valid" — so a bumped schema must never fail validation; only the frozen v1
# prefix (length ≥ 168) is asserted.
RELEASE_CHANNEL_MIN_LENGTH = 168
RELEASE_CHANNEL_SCHEMA_BYTE = 1


def blake2_128_concat(data: bytes) -> bytes:
    return hashlib.blake2b(data, digest_size=16).digest() + data


def exact_storage_key(pallet_prefix: str, item: str, key_hex: str) -> str:
    """Full storage key for a Blake2_128Concat map entry, as raw hex."""
    key_bytes = bytes.fromhex(key_hex.removeprefix("0x"))
    return "0x" + (
        twox128(pallet_prefix) + twox128(item) + blake2_128_concat(key_bytes)
    ).hex()


def hex_bytes(value: Any, label: str) -> bytes:
    if not isinstance(value, str) or not value.startswith("0x"):
        raise ValueError(f"{label} must be a 0x-prefixed hex string")
    try:
        return bytes.fromhex(value[2:])
    except ValueError as error:
        raise ValueError(f"{label} contains invalid hex") from error


def storage_value_from_chainhead(response: dict[str, Any]) -> str | None:
    """Extract the single storage value from recorded chainHead operation events.

    Returns the 0x-hex value, or None when the operation completed without
    delivering an item (an absent key).
    """
    for event in response.get("events", []):
        if event.get("event") != "operationStorageItems":
            continue
        for item in event.get("items", []):
            value = item.get("value")
            if isinstance(value, str):
                return value
    return None


def nonempty_hex(value: Any) -> bool:
    return isinstance(value, str) and value.startswith("0x") and len(value) > 2


def validate_release_channel(
    value_hex: str | None, value_optional: bool
) -> tuple[bool, str]:
    """02 §12 raw-layout check; a fresh chain may legitimately not have written it."""
    if value_hex is None:
        if value_optional:
            return True, "raw key readable; value not yet written (fresh chain)"
        return False, "ReleaseChannel raw value is absent"
    try:
        raw = hex_bytes(value_hex, "ReleaseChannel value")
    except ValueError as error:
        return False, str(error)
    if len(raw) < RELEASE_CHANNEL_MIN_LENGTH:
        return (
            False,
            f"ReleaseChannel value is {len(raw)} bytes; 02 §12 fixes ≥ "
            f"{RELEASE_CHANNEL_MIN_LENGTH}",
        )
    if raw[0] != RELEASE_CHANNEL_SCHEMA_BYTE:
        # 02 §12: a non-1 schema byte means the layout was extended
        # append-only beyond offset 168; the v1 prefix stays readable, so a
        # future schema bump must not block fixture recording or releases.
        return True, f"recorded (schema {raw[0]}, append-only extension of v1)"
    return True, "recorded"


def decode_compact_len(raw: bytes, offset: int) -> tuple[int, int]:
    """Decode a SCALE compact length, returning (value, next_offset)."""
    if offset >= len(raw):
        raise ValueError("compact length truncated")
    mode = raw[offset] & 0b11
    if mode == 0:
        return raw[offset] >> 2, offset + 1
    if mode == 1:
        if offset + 2 > len(raw):
            raise ValueError("two-byte compact truncated")
        return int.from_bytes(raw[offset : offset + 2], "little") >> 2, offset + 2
    if mode == 2:
        if offset + 4 > len(raw):
            raise ValueError("four-byte compact truncated")
        return int.from_bytes(raw[offset : offset + 4], "little") >> 2, offset + 4
    raise ValueError("big-integer compact lengths are not expected here")


def decode_asset_metadata_decimals(value_hex: str) -> int:
    """`pallet-assets` AssetMetadata { deposit: u128, name, symbol, decimals, .. }."""
    raw = hex_bytes(value_hex, "AssetMetadata value")
    offset = 16  # deposit: u128
    name_len, offset = decode_compact_len(raw, offset)
    offset += name_len
    symbol_len, offset = decode_compact_len(raw, offset)
    offset += symbol_len
    if offset >= len(raw):
        raise ValueError("AssetMetadata truncated before decimals")
    return raw[offset]


def decode_asset_details_min_balance(value_hex: str) -> int:
    """`pallet-assets` AssetDetails: min_balance is the u128 after 4 accounts + 2 u128s."""
    raw = hex_bytes(value_hex, "AssetDetails value")
    offset = 32 * 4 + 16 + 16
    if offset + 16 > len(raw):
        raise ValueError("AssetDetails truncated before min_balance")
    return int.from_bytes(raw[offset : offset + 16], "little")


def check_expected_value(entry: dict[str, Any], value_hex: str | None) -> tuple[bool, str]:
    """Assert an exact-key storage read against the entry's `expected` block."""
    expected = entry.get("expected", {})
    if value_hex is None:
        return False, "exact-key value is absent on this runtime"
    try:
        if "decimals" in expected:
            actual = decode_asset_metadata_decimals(value_hex)
            if actual != expected["decimals"]:
                return False, f"decimals is {actual}, 02 §8 fixes {expected['decimals']}"
        if "min_balance" in expected:
            actual = decode_asset_details_min_balance(value_hex)
            if actual != expected["min_balance"]:
                return (
                    False,
                    f"min_balance is {actual}, 02 §8 fixes {expected['min_balance']}",
                )
    except ValueError as error:
        return False, f"exact-key value undecodable: {error}"
    return True, "recorded"


def properties_match(
    actual: Any, expected: dict[str, Any]
) -> tuple[bool, str]:
    """Assert the 02 §8 chain-spec `properties` identity values exactly."""
    if not isinstance(actual, dict):
        return False, "system_properties returned a non-object"
    for key, value in expected.items():
        if actual.get(key) != value:
            return False, f"properties.{key} is {actual.get(key)!r}, expected {value!r}"
    return True, "recorded"

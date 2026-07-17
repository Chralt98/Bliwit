from __future__ import annotations

import struct
import sys
import unittest
from pathlib import Path


sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from scale_metadata import compare_layout, decode_metadata, surface_layout


def compact(value: int) -> bytes:
    if value < 64:
        return bytes([value << 2])
    raise ValueError("fixture compact helper only supports one-byte values")


def blob(value: bytes) -> bytes:
    return compact(len(value)) + value


def string(value: str) -> bytes:
    return blob(value.encode())


def vector(*values: bytes) -> bytes:
    return compact(len(values)) + b"".join(values)


def option(value: bytes | None) -> bytes:
    return b"\0" if value is None else b"\1" + value


def field(name: str | None, type_id: int) -> bytes:
    return option(string(name) if name is not None else None) + compact(type_id) + b"\0" + compact(0)


def portable_type(type_id: int, path: tuple[str, ...], definition: bytes) -> bytes:
    return compact(type_id) + vector(*(string(item) for item in path)) + compact(0) + definition + compact(0)


def mini_metadata() -> bytes:
    types = vector(
        portable_type(0, (), b"\5\5"),  # u32
        portable_type(1, (), b"\5\6"),  # u64
        portable_type(2, ("mini", "Value"), b"\0" + vector(field("count", 0))),
        portable_type(
            3,
            ("mini", "Event"),
            b"\1" + vector(string("Changed") + vector(field("x", 0), field("y", 1)) + b"\0" + compact(0)),
        ),
        portable_type(4, (), b"\4" + vector(compact(0), compact(1))),
    )
    storage_entry = (
        string("Item")
        + b"\0"  # modifier
        + b"\1"  # map
        + vector(b"\2")  # Blake2_128Concat
        + compact(4)
        + compact(2)
        + blob(b"")
        + compact(0)
    )
    storage = string("Mini") + vector(storage_entry)
    constant = string("Version") + compact(0) + blob(struct.pack("<I", 3)) + compact(0)
    pallet = (
        string("Mini")
        + option(storage)
        + b"\0"  # calls
        + option(compact(3))
        + vector(constant)
        + b"\0"  # error
        + b"\7"
        + compact(0)
    )
    extrinsic = b"\4" + compact(0) * 4 + compact(0)
    parameter = string("id") + compact(0)
    method = string("lookup") + vector(parameter) + compact(2) + compact(0)
    runtime_api = string("FutarchyApi") + vector(method) + compact(0)
    return struct.pack("<I", 0x6174656D) + b"\x0f" + types + vector(pallet) + extrinsic + compact(0) + vector(runtime_api)


class ScaleMetadataTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.metadata = decode_metadata(mini_metadata())

    def test_resolved_layouts_from_crafted_metadata(self) -> None:
        storage = surface_layout(
            self.metadata,
            {"kind": "storage", "pallet": "Mini", "item": "Item"},
        )
        self.assertEqual(
            storage,
            {
                "hashers": ["Blake2_128Concat"],
                "key": "(u32,u64)",
                "value": "mini::Value{count:u32}",
            },
        )
        event = surface_layout(
            self.metadata,
            {"kind": "event", "pallet": "Mini", "event": "Changed"},
        )
        self.assertEqual(
            event,
            {"fields": [{"name": "x", "type": "u32"}, {"name": "y", "type": "u64"}]},
        )
        constant = surface_layout(
            self.metadata,
            {"kind": "constant", "pallet": "Mini", "constant": "Version"},
        )
        self.assertEqual(constant, {"type": "u32", "value": "0x03000000"})
        runtime_api = surface_layout(
            self.metadata,
            {"kind": "runtime_api", "api": "FutarchyApi", "method": "lookup"},
        )
        self.assertEqual(
            runtime_api,
            {
                "params": [{"name": "id", "type": "u32"}],
                "return": "mini::Value{count:u32}",
            },
        )

    def test_layout_mismatch_is_explicit(self) -> None:
        actual = {"hashers": ["Blake2_128Concat"], "key": "u32", "value": "u64"}
        expected = {"hashers": ["Blake2_128Concat"], "key": "u64", "value": "u64"}
        self.assertEqual(compare_layout(actual, expected), (False, "layout mismatch"))


if __name__ == "__main__":
    unittest.main()

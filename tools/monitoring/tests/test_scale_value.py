from __future__ import annotations

import unittest

from common import ScaleValueError, decode_typed_bytes


def primitive(type_id: int, name: str) -> dict:
    return {"id": type_id, "path": [], "definition": {"kind": "primitive", "primitive": name}}


class ScaleValueTests(unittest.TestCase):
    def setUp(self) -> None:
        self.metadata = {
            "types": {
                1: primitive(1, "u32"),
                2: primitive(2, "bool"),
                3: {
                    "id": 3,
                    "path": ["Fixture"],
                    "definition": {
                        "kind": "composite",
                        "fields": [
                            {"name": "height", "type_id": 1},
                            {"name": "ready", "type_id": 2},
                        ],
                    },
                },
                4: {
                    "id": 4,
                    "path": ["Phase"],
                    "definition": {
                        "kind": "variant",
                        "variants": [
                            {"name": "Idle", "index": 0, "fields": []},
                            {"name": "Live", "index": 7, "fields": [{"name": None, "type_id": 1}]},
                        ],
                    },
                },
                5: {"id": 5, "path": [], "definition": {"kind": "sequence", "type_id": 1}},
                6: {"id": 6, "path": [], "definition": {"kind": "compact", "type_id": 1}},
                7: {
                    "id": 7,
                    "path": ["Transparent"],
                    "definition": {"kind": "composite", "fields": [{"name": None, "type_id": 5}]},
                },
            }
        }

    def test_named_composite_byte_fixture(self) -> None:
        self.assertEqual(
            decode_typed_bytes((42).to_bytes(4, "little") + b"\x01", 3, self.metadata),
            {"height": 42, "ready": True},
        )

    def test_variant_uses_portable_name_not_position(self) -> None:
        self.assertEqual(
            decode_typed_bytes(b"\x07" + (9).to_bytes(4, "little"), 4, self.metadata),
            {"variant": "Live", "index": 7, "fields": 9},
        )

    def test_sequence_and_transparent_wrapper_fixture(self) -> None:
        data = b"\x0c" + b"".join(value.to_bytes(4, "little") for value in (1, 2, 3))
        self.assertEqual(decode_typed_bytes(data, 7, self.metadata), [1, 2, 3])

    def test_compact_fixture(self) -> None:
        self.assertEqual(decode_typed_bytes(((123 << 2) | 1).to_bytes(2, "little"), 6, self.metadata), 123)

    def test_trailing_bytes_fail(self) -> None:
        with self.assertRaisesRegex(ScaleValueError, "trailing"):
            decode_typed_bytes((1).to_bytes(4, "little") + b"x", 1, self.metadata)

    def test_malformed_bool_fails(self) -> None:
        with self.assertRaisesRegex(ScaleValueError, "bool"):
            decode_typed_bytes(b"\x02", 2, self.metadata)


if __name__ == "__main__":
    unittest.main()


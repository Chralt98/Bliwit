from __future__ import annotations

import unittest

from support import release_channel_bytes

from common import MonitoringError, decode_release_channel


class ReleaseChannelTests(unittest.TestCase):
    def test_decodes_frozen_168_byte_fixture(self) -> None:
        raw = release_channel_bytes(
            generation=9,
            revoked=(1 << 1) | (1 << 63),
            flags=0b111,
            spec_version=1234,
            updated_at=55,
            pending=77,
        )
        channel = decode_release_channel(raw)
        self.assertEqual(channel.version, "1.2.3")
        self.assertEqual(channel.manifest_txid, "A" * 43)
        self.assertEqual(channel.spec_version, 1234)
        self.assertEqual(channel.updated_at, 55)
        self.assertEqual(channel.pending_authorized_at, 77)
        self.assertEqual(channel.keyring_generation, 9)
        self.assertEqual(channel.revoked_key_bits, (1 << 1) | (1 << 63))
        self.assertTrue(channel.security)
        self.assertTrue(channel.expedited)
        self.assertTrue(channel.urgent_upgrade)

    def test_append_only_suffix_is_ignored(self) -> None:
        self.assertEqual(
            decode_release_channel(release_channel_bytes() + b"future").version,
            "1.2.3",
        )

    def test_short_fixture_fails_crisply(self) -> None:
        with self.assertRaisesRegex(MonitoringError, "168"):
            decode_release_channel(b"\0" * 167)

    def test_reserved_flag_bits_are_rejected(self) -> None:
        with self.assertRaisesRegex(MonitoringError, "reserved"):
            decode_release_channel(release_channel_bytes(flags=8))

    def test_embedded_nul_text_is_rejected(self) -> None:
        value = bytearray(release_channel_bytes())
        value[1:5] = b"a\0bc"
        with self.assertRaisesRegex(MonitoringError, "embedded NUL"):
            decode_release_channel(bytes(value))


if __name__ == "__main__":
    unittest.main()


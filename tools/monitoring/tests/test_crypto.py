from __future__ import annotations

import base64
import unittest

from support import keypair, minisign_text, public_text

import attestation_monitor as am


class CryptoTests(unittest.TestCase):
    def test_rfc8032_vector_one(self) -> None:
        public = bytes.fromhex("d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a")
        signature = bytes.fromhex(
            "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e06522490155"
            "5fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b"
        )
        self.assertTrue(am.ed25519_verify(public, b"", signature))
        self.assertFalse(am.ed25519_verify(public, b"x", signature))

    def test_minisign_legacy_ed_vector(self) -> None:
        seed, public, key_id = keypair(1, 2)
        parsed = am.parse_minisign_public_key(public_text(public, key_id))
        signature = minisign_text(seed, public, key_id, b"legacy", b"Ed")
        self.assertTrue(am.verify_minisign(b"legacy", signature, parsed))
        self.assertFalse(am.verify_minisign(b"tampered", signature, parsed))

    def test_minisign_prehashed_ed_vector(self) -> None:
        seed, public, key_id = keypair(3, 4)
        parsed = am.parse_minisign_public_key(public_text(public, key_id))
        signature = minisign_text(seed, public, key_id, b"prehashed", b"ED")
        self.assertTrue(am.verify_minisign(b"prehashed", signature, parsed))

    def test_trusted_comment_is_authenticated(self) -> None:
        seed, public, key_id = keypair(5, 6)
        parsed = am.parse_minisign_public_key(public_text(public, key_id))
        signature = minisign_text(seed, public, key_id, b"message")
        self.assertFalse(am.verify_minisign(b"message", signature.replace("file:fixture", "file:other"), parsed))

    def test_wrong_key_id_is_rejected(self) -> None:
        seed, public, key_id = keypair(7, 8)
        _, other_public, other_id = keypair(9, 10)
        signature = minisign_text(seed, public, key_id, b"message")
        self.assertFalse(
            am.verify_minisign(
                b"message",
                signature,
                am.parse_minisign_public_key(public_text(other_public, other_id)),
            )
        )

    def test_malformed_minisign_packet_is_rejected(self) -> None:
        malformed = "untrusted comment: x\n" + base64.b64encode(b"short").decode() + "\ntrusted comment: x\n" + base64.b64encode(b"x" * 64).decode()
        with self.assertRaisesRegex(ValueError, "packet"):
            am.parse_minisign_signature(malformed)


if __name__ == "__main__":
    unittest.main()

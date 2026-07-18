from __future__ import annotations

import copy
import unittest

from support import integrity_fixture, release_channel_bytes

import attestation_monitor as am


def evaluate(fixture: dict, **overrides):
    arguments = {
        "files": fixture["files"],
        "expected_hashes": fixture["hashes"],
        "release_json_bytes": fixture["release_raw"],
        "release_document": fixture["document"],
        "release_signatures": fixture["signatures"],
        "attestations": fixture["attestations"],
        "keyring": fixture["keyring"],
        "release_channel_bytes": fixture["channel"],
        "resolved_txids": fixture["resolved"],
        "minimum_release_signatures": 1,
    }
    arguments.update(overrides)
    return am.evaluate_integrity(**arguments)


class AttestationVerdictTests(unittest.TestCase):
    def test_resolver_consensus_requires_a_strict_majority(self) -> None:
        self.assertEqual(am.resolver_consensus(["A", "A", "B"]), "A")
        self.assertIsNone(am.resolver_consensus(["A", "B", "C"]))
        self.assertIsNone(am.resolver_consensus(["A", "A", "B", "B"]))

    def test_complete_fixture_is_healthy(self) -> None:
        verdict = evaluate(integrity_fixture())
        self.assertTrue(verdict.ok, verdict.errors)
        self.assertEqual(verdict.valid_release_signatures, 1)
        self.assertEqual(verdict.valid_attestations, 2)

    def test_byte_mismatch_fixture(self) -> None:
        fixture = integrity_fixture()
        files = dict(fixture["files"])
        files["app.js"] += b"tamper"
        verdict = evaluate(fixture, files=files)
        self.assertFalse(verdict.ok)
        self.assertEqual(verdict.byte_mismatches, 1)
        self.assertTrue(any("app.js" in error for error in verdict.errors))

    def test_unlisted_served_file_is_a_mismatch(self) -> None:
        fixture = integrity_fixture()
        files = dict(fixture["files"])
        files["evil.js"] = b"evil"
        verdict = evaluate(fixture, files=files)
        self.assertFalse(verdict.ok)
        self.assertEqual(verdict.byte_mismatches, 1)

    def test_missing_attestations_fixture(self) -> None:
        fixture = integrity_fixture()
        verdict = evaluate(fixture, attestations=fixture["attestations"][:1])
        self.assertFalse(verdict.ok)
        self.assertEqual(verdict.valid_attestations, 1)

    def test_wrong_keyring_generation_fixture(self) -> None:
        fixture = integrity_fixture()
        wrong = am.Keyring(8, fixture["keyring"].keys)
        verdict = evaluate(fixture, keyring=wrong)
        self.assertFalse(verdict.ok)
        self.assertTrue(any("generation" in error for error in verdict.errors))

    def test_revoked_release_key_bit_fixture(self) -> None:
        fixture = integrity_fixture()
        channel = release_channel_bytes(
            release_json_hash=fixture["channel"][76:108], revoked=1 << 0
        )
        verdict = evaluate(fixture, release_channel_bytes=channel)
        self.assertFalse(verdict.ok)
        self.assertEqual(verdict.valid_release_signatures, 0)

    def test_revoked_attestor_key_bit_fixture(self) -> None:
        fixture = integrity_fixture()
        channel = release_channel_bytes(
            release_json_hash=fixture["channel"][76:108], revoked=1 << 1
        )
        verdict = evaluate(fixture, release_channel_bytes=channel)
        self.assertFalse(verdict.ok)
        self.assertEqual(verdict.valid_attestations, 1)

    def test_manifest_txid_mismatch_fixture(self) -> None:
        fixture = integrity_fixture()
        document = copy.deepcopy(fixture["document"])
        document["manifest_txid"] = "B" * 43
        verdict = evaluate(fixture, release_document=document)
        self.assertFalse(verdict.ok)
        self.assertFalse(verdict.manifest_matches_channel)

    def test_two_of_three_gateway_resolver_divergence_fixture(self) -> None:
        fixture = integrity_fixture()
        verdict = evaluate(fixture, resolved_txids=["B" * 43, "B" * 43, "A" * 43])
        self.assertFalse(verdict.ok)
        self.assertEqual(verdict.resolver_divergent_gateways, 2)

    def test_one_of_three_resolver_difference_is_recorded_but_not_threshold_failure(self) -> None:
        fixture = integrity_fixture()
        verdict = evaluate(fixture, resolved_txids=["B" * 43, "A" * 43, "A" * 43])
        self.assertTrue(verdict.ok, verdict.errors)
        self.assertEqual(verdict.resolver_divergent_gateways, 1)

    def test_non_covering_release_fixture(self) -> None:
        fixture = integrity_fixture()
        document = copy.deepcopy(fixture["document"])
        document["supported_spec_version"] = {"min": 1, "max": 2}
        verdict = evaluate(fixture, release_document=document)
        self.assertFalse(verdict.ok)
        self.assertFalse(verdict.covering_release)

    def test_operator_release_signature_threshold_is_not_silently_defaulted(self) -> None:
        fixture = integrity_fixture()
        verdict = evaluate(fixture, minimum_release_signatures=2)
        self.assertFalse(verdict.ok)
        self.assertTrue(any("operator minimum 2" in error for error in verdict.errors))


if __name__ == "__main__":
    unittest.main()

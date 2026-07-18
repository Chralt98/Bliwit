import copy
import json
from pathlib import Path
import os
import subprocess
import sys
import tempfile
import unittest

from bleavit_simulation.calibration import outcome_digest, outcome_merkle_root
from bleavit_simulation.config import SimulationConfig, python_version_tuple
from bleavit_simulation.evidence import check_artifact, load_artifact, write_artifact


ROOT = Path(__file__).resolve().parents[2]
ARTIFACT = ROOT / "simulation" / "results" / "phase0-calibration.json"


class CalibrationRunnerTests(unittest.TestCase):
    def test_config_binding_covers_sources_but_not_the_interpreter(self):
        canonical = SimulationConfig().canonical()
        # The digest binds behavior-affecting inputs only; the generating
        # interpreter is artifact provenance so the committed evidence
        # verifies on any Python (the pinned subsample replay and the
        # Merkle root catch real cross-version divergence).
        self.assertNotIn("python_version", canonical)
        self.assertEqual(len(canonical["source_model_sha256"]), 64)
        artifact = json.loads(ARTIFACT.read_text(encoding="utf-8"))
        recorded = artifact["provenance"]["python_version"]
        self.assertEqual(len(recorded), 5)
        for part in recorded[:3]:
            self.assertIsInstance(part, int)
        self.assertEqual(len(python_version_tuple()), 5)

    def test_check_revalidates_structure_and_reports_economic_status(self):
        env = os.environ.copy()
        env["PYTHONPATH"] = os.pathsep.join(
            (str(ROOT / "reference-model" / "src"), str(ROOT / "simulation" / "src"))
        )
        result = subprocess.run(
            [sys.executable, str(ROOT / "tools" / "simulation" / "run-calibration.py"), "--check"],
            cwd=ROOT,
            env=env,
            check=False,
            capture_output=True,
            text=True,
            timeout=180,
        )
        artifact = load_artifact(ARTIFACT)
        self.assertEqual(result.returncode, 1 if artifact["violations"] else 0)
        self.assertIn("calibration structure OK", result.stdout)
        if artifact["violations"]:
            self.assertIn("normative violations:", result.stderr)

    def test_merkle_root_is_ordered_domain_separated_and_mutation_sensitive(self):
        evidence_a = {"outcome": "Reject", "reason": "HurdleNotMet"}
        evidence_b = {"outcome": "Adopt", "reason": None}
        rows = [
            {"proposal_id": 1, "digest": outcome_digest(1, evidence_a)},
            {"proposal_id": 2, "digest": outcome_digest(2, evidence_b)},
            {"proposal_id": 3, "digest": outcome_digest(3, evidence_a)},
        ]
        root = outcome_merkle_root(rows)
        mutated = copy.deepcopy(rows)
        mutated[1]["digest"] = outcome_digest(2, {"outcome": "Reject"})
        self.assertNotEqual(root, outcome_merkle_root(mutated))
        with self.assertRaisesRegex(ValueError, "sorted"):
            outcome_merkle_root(list(reversed(rows)))
        with self.assertRaisesRegex(ValueError, "duplicate"):
            outcome_merkle_root([rows[0], rows[0]])

    def test_check_rejects_digest_and_dependency_binding_corruption(self):
        original = load_artifact(ARTIFACT)
        mutations = {}

        def bad_config(payload):
            payload["config_digest"] = "0" * 64

        def bad_leaf(payload):
            payload["outcome_digests"][0]["digest"] = "0" * 64

        def bad_root(payload):
            payload["outcome_digest_root"] = "f" * 64

        def reordered(payload):
            payload["outcome_digests"][0], payload["outcome_digests"][1] = (
                payload["outcome_digests"][1], payload["outcome_digests"][0]
            )

        def bad_subsample(payload):
            payload["subsample"]["results"][0]["outcome"] = "Corrupt"

        def missing_violations(payload):
            payload["violations"] = []

        def false_eligibility(payload):
            payload["published"]["eligibility"][
                "all_decidable_harm_false_pass_lt_1pct"
            ] = True

        def green_attack_summary(payload):
            attack = payload["attack_cost_validation"]
            attack["envelope_violations"] = []
            attack["envelope_inconclusive"] = []
            attack["sub_3p_every_class_clean"] = True
            for row in attack["per_class"].values():
                row["envelope_clean"] = True
                row["sub_3p_clean"] = True
            payload["published"]["eligibility"]["sub_3p_brackets_clean"] = True
            payload["violations"] = [
                value
                for value in payload["violations"]
                if "false-pass rate" in value
            ]

        mutations.update(
            {
                "config": bad_config,
                "leaf": bad_leaf,
                "root": bad_root,
                "order": reordered,
                "subsample": bad_subsample,
                "violations": missing_violations,
                "eligibility": false_eligibility,
                "attack_summary": green_attack_summary,
            }
        )
        with tempfile.TemporaryDirectory() as directory:
            for name, mutate in mutations.items():
                with self.subTest(name=name):
                    payload = copy.deepcopy(original)
                    mutate(payload)
                    path = Path(directory) / f"{name}.json"
                    write_artifact(path, payload)
                    with self.assertRaisesRegex(ValueError, "calibration check failed"):
                        check_artifact(path)


if __name__ == "__main__":
    unittest.main()

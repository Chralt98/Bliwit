"""Negative gates for B7 environments (15 §4.7; 02 §11; 06 §6.2)."""

from __future__ import annotations

import json
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).resolve().parents[1] / "validate-environments.py"
REPO = SCRIPT.parents[2]


class ValidateEnvironmentsTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        shutil.copytree(REPO / "zombienet", self.root / "zombienet")
        shutil.copytree(REPO / "chopsticks", self.root / "chopsticks")
        shutil.copytree(REPO / "tools" / "env", self.root / "tools" / "env")

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def run_validator(self, root: Path | None = None) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            ["python3", str(SCRIPT), "--root", str(root or self.root)],
            check=False,
            capture_output=True,
            text=True,
        )

    def assert_fails_with(self, fragment: str) -> None:
        result = self.run_validator()
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertIn(fragment, result.stderr)

    def test_committed_tree_is_green(self) -> None:
        result = self.run_validator(REPO)
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_missing_drill_is_release_blocking(self) -> None:
        (self.root / "zombienet" / "drills" / "04-dead-man.zndsl").unlink()
        self.assert_fails_with("required Zombienet drill is missing")

    def test_bad_parachain_id_is_release_blocking(self) -> None:
        path = self.root / "zombienet" / "networks" / "bleavit-local.toml"
        text = path.read_text(encoding="utf-8").replace("id = 4242", "id = 4243", 1)
        path.write_text(text, encoding="utf-8")
        self.assert_fails_with("parachain ids must be")

    def test_missing_hrmp_direction_is_release_blocking(self) -> None:
        path = self.root / "zombienet" / "networks" / "bleavit-xcm.toml"
        text = path.read_text(encoding="utf-8").replace(
            "sender = 1000\nrecipient = 4242",
            "sender = 4242\nrecipient = 1000",
            1,
        )
        path.write_text(text, encoding="utf-8")
        self.assert_fails_with("missing HRMP direction")

    def test_non_localhost_endpoint_is_release_blocking(self) -> None:
        path = self.root / "chopsticks" / "scenarios" / "pb-depeg.yml"
        path.write_text(
            path.read_text(encoding="utf-8") + "endpoint: wss://example.invalid\n",
            encoding="utf-8",
        )
        self.assert_fails_with("non-localhost endpoint")

    def test_copied_pin_is_release_blocking(self) -> None:
        pins = (self.root / "tools" / "env" / "pins.env").read_text(encoding="utf-8")
        pin = next(
            line.split("=", 1)[1]
            for line in pins.splitlines()
            if line.startswith("ZOMBIENET_VERSION=")
        )
        path = self.root / "zombienet" / "README.md"
        path.write_text(path.read_text(encoding="utf-8") + f"\n<!-- {pin} -->\n", encoding="utf-8")
        self.assert_fails_with("copies ZOMBIENET_VERSION's pinned value")

    def test_broken_zndsl_network_reference_is_release_blocking(self) -> None:
        path = self.root / "zombienet" / "drills" / "01-smoke.zndsl"
        text = path.read_text(encoding="utf-8").replace(
            "zombienet/networks/bleavit-local.toml",
            "zombienet/networks/missing.toml",
            1,
        )
        path.write_text(text, encoding="utf-8")
        self.assert_fails_with("references missing network")

    def test_bad_drill_relative_js_path_is_release_blocking(self) -> None:
        path = self.root / "zombienet" / "drills" / "03-keeper-loss.zndsl"
        text = path.read_text(encoding="utf-8").replace(
            "./js/assert-chain-liveness.js",
            "./js/missing-helper.js",
            1,
        )
        path.write_text(text, encoding="utf-8")
        self.assert_fails_with("references missing js helper")

    def test_malformed_paseo_commit_pin_is_release_blocking(self) -> None:
        path = self.root / "tools" / "env" / "pins.env"
        text = path.read_text(encoding="utf-8")
        commit_line = next(
            line for line in text.splitlines() if line.startswith("PASEO_CSG_COMMIT=")
        )
        text = text.replace(commit_line, "PASEO_CSG_COMMIT=not-a-commit")
        path.write_text(text, encoding="utf-8")
        self.assert_fails_with("PASEO_CSG_COMMIT must be a lowercase 40-hex commit")

    def test_two_tools_may_share_a_version(self) -> None:
        path = self.root / "tools" / "env" / "pins.env"
        text = path.read_text(encoding="utf-8")
        pins = dict(
            line.split("=", 1)
            for line in text.splitlines()
            if line and not line.startswith("#")
        )
        text = text.replace(
            f"CHOPSTICKS_VERSION={pins['CHOPSTICKS_VERSION']}",
            f"CHOPSTICKS_VERSION={pins['ZOMBIENET_VERSION']}",
        )
        path.write_text(text, encoding="utf-8")
        result = self.run_validator()
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_suites_manifest_missing_required_row_is_release_blocking(self) -> None:
        path = self.root / "tools" / "env" / "suites.json"
        manifest = json.loads(path.read_text(encoding="utf-8"))
        missing = "zombienet/drills/04-dead-man.zndsl"
        manifest["suites"] = [
            row for row in manifest["suites"] if row.get("path") != missing
        ]
        path.write_text(json.dumps(manifest), encoding="utf-8")
        result = self.run_validator()
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertIn("suites.json", result.stderr)
        self.assertIn(missing, result.stderr)

    def test_suites_manifest_extra_row_is_release_blocking(self) -> None:
        extra = self.root / "chopsticks" / "extra.yml"
        extra.write_text("port: 9000\n", encoding="utf-8")
        path = self.root / "tools" / "env" / "suites.json"
        manifest = json.loads(path.read_text(encoding="utf-8"))
        manifest["suites"].append(
            {
                "id": "extra",
                "kind": "chopsticks",
                "path": "chopsticks/extra.yml",
                "tier": "release",
                "gated_on": [],
                "timeout_seconds": 30,
                "spec": "15 §4.7",
            }
        )
        path.write_text(json.dumps(manifest), encoding="utf-8")
        result = self.run_validator()
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertIn("suites.json", result.stderr)
        self.assertIn("chopsticks/extra.yml", result.stderr)

    def test_g1_drill_flipped_to_release_is_release_blocking(self) -> None:
        path = self.root / "tools" / "env" / "suites.json"
        manifest = json.loads(path.read_text(encoding="utf-8"))
        row = next(row for row in manifest["suites"] if row["id"] == "04-dead-man")
        row["tier"] = "release"
        path.write_text(json.dumps(manifest), encoding="utf-8")

        result = self.run_validator()

        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertIn("04-dead-man", result.stderr)
        self.assertIn("tier", result.stderr)
        self.assertIn("g1", result.stderr)

    def test_release_drill_flipped_to_g1_is_release_blocking(self) -> None:
        path = self.root / "tools" / "env" / "suites.json"
        manifest = json.loads(path.read_text(encoding="utf-8"))
        row = next(row for row in manifest["suites"] if row["id"] == "01-smoke")
        row["tier"] = "g1"
        path.write_text(json.dumps(manifest), encoding="utf-8")

        result = self.run_validator()

        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertIn("01-smoke", result.stderr)
        self.assertIn("tier", result.stderr)
        self.assertIn("release", result.stderr)

    def test_active_wasm_override_is_release_blocking(self) -> None:
        path = self.root / "chopsticks" / "scenarios" / "pb-depeg.yml"
        path.write_text(
            path.read_text(encoding="utf-8")
            + "\nwasm-override: target/release/runtime.wasm\n",
            encoding="utf-8",
        )
        self.assert_fails_with("wasm-override")

    def test_runtime_code_import_is_release_blocking(self) -> None:
        path = self.root / "chopsticks" / "scenarios" / "pb-depeg.yml"
        text = path.read_text(encoding="utf-8")
        key_start = text.index('  - - "0x')
        key_end = text.index('"', key_start + len('  - - "'))
        text = (
            text[: key_start + len('  - - "')]
            + "0x3A636F6465"
            + text[key_end:]
        )
        path.write_text(text, encoding="utf-8")
        result = self.run_validator()
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertIn("0x3a636f6465", result.stderr.lower())

    def test_database_outside_state_directory_is_release_blocking(self) -> None:
        path = self.root / "chopsticks" / "scenarios" / "pb-depeg.yml"
        text = path.read_text(encoding="utf-8").replace(
            "db: chopsticks/.state/pb-depeg.sqlite",
            "db: target/pb-depeg.sqlite",
            1,
        )
        path.write_text(text, encoding="utf-8")
        self.assert_fails_with("chopsticks/.state")

    def test_duplicate_database_path_is_release_blocking(self) -> None:
        path = self.root / "chopsticks" / "scenarios" / "pb-depeg.yml"
        text = path.read_text(encoding="utf-8").replace(
            "db: chopsticks/.state/pb-depeg.sqlite",
            "db: chopsticks/.state/bleavit.sqlite",
            1,
        )
        path.write_text(text, encoding="utf-8")
        result = self.run_validator()
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertIn("must be unique", result.stderr.lower())
        self.assertIn("chopsticks/.state/bleavit.sqlite", result.stderr)


if __name__ == "__main__":
    unittest.main()

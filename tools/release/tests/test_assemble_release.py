from __future__ import annotations

import hashlib
import importlib.util
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


TOOLS = Path(__file__).resolve().parents[1]
SCRIPT = TOOLS / "assemble-release.py"
SPEC = importlib.util.spec_from_file_location("assemble_release", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
ASSEMBLE = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = ASSEMBLE
SPEC.loader.exec_module(ASSEMBLE)


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


class AssembleReleaseTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.runtime = self.root / "runtime"
        self.specs = self.root / "specs"
        self.fixtures = self.root / "fixtures"
        self.sweep = self.root / "sweep"
        self.vectors = self.root / "vectors.json"
        self.environments = self.root / "environments.json"
        self.validator = self.root / "validator.py"
        self.supply_summary = self.root / "supply-chain-summary.json"
        self.zombienet = self.root / "zombienet"
        self.chopsticks = self.root / "chopsticks"
        for path in (self.runtime, self.specs, self.fixtures, self.sweep / "shards"):
            path.mkdir(parents=True)

        wasm = self.runtime / "runtime.wasm"
        wasm.write_bytes(b"wasm-release-bytes")
        metadata = self.runtime / "metadata.scale"
        metadata.write_bytes(b"metadata")
        wasm_hash = sha256(wasm)
        (self.runtime / "runtime-info.json").write_text(
            json.dumps(
                {
                    "wasm_sha256": wasm_hash,
                    "wasm_file_sha256": wasm_hash,
                    "on_chain_wasm_sha256": wasm_hash,
                    "metadata_sha256": sha256(metadata),
                }
            ),
            encoding="utf-8",
        )
        build_info = {
            "schema": "bleavit.runtime-build.v1",
            "git_commit": "a" * 40,
            "toolchain": "1.89.0",
            "host_triple": "x86_64-unknown-linux-gnu",
            "cargo_version": "cargo 1.89.0",
            "rustc_version": "rustc 1.89.0",
            "recipe": "cargo build --locked",
            "reproducibility_scope": "same toolchain + same source => same bytes",
            "wasm": {"sha256": wasm_hash, "size": wasm.stat().st_size},
        }
        (self.runtime / "build-info.json").write_text(
            json.dumps(build_info), encoding="utf-8"
        )
        for name in ("bleavit-dev.json", "bleavit-local.json"):
            (self.specs / name).write_text(
                json.dumps(
                    {"genesis": {"runtimeGenesis": {"code": "0x" + wasm.read_bytes().hex()}}}
                ),
                encoding="utf-8",
            )
        self.environments.write_text(
            json.dumps(
                {
                    "schema": "bleavit.environments.v1",
                    "environments": [
                        {"id": "dev", "chain_spec": "bleavit-dev.json", "live": False, "required": True},
                        {"id": "local", "chain_spec": "bleavit-local.json", "live": False, "required": True},
                    ],
                }
            ),
            encoding="utf-8",
        )
        self.validator.write_text("raise SystemExit(0)\n", encoding="utf-8")
        self.supply_summary.write_text(
            json.dumps(
                {
                    "schema": "bleavit.supply-chain.v2",
                    "ignored_advisory_ids": ["RUSTSEC-2026-0001"],
                    "waived_ghsa_only": [
                        {"id": "GHSA-vxx9-2994-q338", "package": "yamux", "version": "0.12.1"}
                    ],
                    "workspaces": {
                        "root": {"allowed_warning_count": 2},
                        "keeper": {"allowed_warning_count": 0},
                    },
                }
            ),
            encoding="utf-8",
        )
        self.surface_manifest = self.root / "surface-manifest.json"
        self.surface_manifest.write_text(
            json.dumps(
                {
                    "schema": "bleavit.critical-surface.v1",
                    "entries": [
                        {
                            "id": "storage.constitution.phase_flags",
                            "kind": "storage",
                            "required": True,
                            "citation": "02 §7.3",
                        }
                    ],
                }
            ),
            encoding="utf-8",
        )
        (self.fixtures / "fixtures-report.json").write_text(
            json.dumps(
                {
                    "schema": "bleavit.chainhead-fixtures-report.v1",
                    "mode": "chainHead-v1",
                    "metadata_sha256": sha256(metadata),
                    "recorded": ["storage.constitution.phase_flags"],
                    "missing": [],
                    "strict_ready": True,
                }
            ),
            encoding="utf-8",
        )
        (self.fixtures / "storage.constitution.phase_flags.json").write_text(
            '{"surface":"storage.constitution.phase_flags","requests":[]}\n',
            encoding="utf-8",
        )
        self.vectors.write_text('{"schema":"bleavit.reference-model.v2"}\n', encoding="utf-8")
        shard = self.sweep / "shards" / "sweep-000.json"
        shard.write_text("[]\n", encoding="utf-8")
        (self.sweep / "sweep-manifest.json").write_text(
            json.dumps(
                {
                    "points": 1,
                    "shards": [
                        {
                            "path": "shards/sweep-000.json",
                            "rows": 1,
                            "sha256": sha256(shard),
                        }
                    ],
                }
            ),
            encoding="utf-8",
        )

    def tearDown(self) -> None:
        self.temp.cleanup()

    def command(self, output: Path, allow_missing: bool) -> list[str]:
        command = [
            sys.executable,
            str(SCRIPT),
            "--output-dir",
            str(output),
            "--runtime-dir",
            str(self.runtime),
            "--chain-spec-dir",
            str(self.specs),
            "--fixtures-dir",
            str(self.fixtures),
            "--surface-manifest",
            str(self.surface_manifest),
            "--zombienet-dir",
            str(self.zombienet),
            "--chopsticks-dir",
            str(self.chopsticks),
            "--environments",
            str(self.environments),
            "--chain-spec-validator",
            str(self.validator),
            "--reference-vectors",
            str(self.vectors),
            "--sweep-dir",
            str(self.sweep),
            "--supply-chain-result",
            "passed",
            "--supply-chain-summary",
            str(self.supply_summary),
            "--tag",
            "v-test",
            "--commit",
            "a" * 40,
        ]
        if allow_missing:
            command.append("--allow-missing")
        return command

    def run_assemble(self, output: Path, allow_missing: bool) -> subprocess.CompletedProcess[str]:
        environment = dict(os.environ)
        environment["SOURCE_DATE_EPOCH"] = "1"
        return subprocess.run(
            self.command(output, allow_missing),
            text=True,
            capture_output=True,
            env=environment,
        )

    def test_strict_missing_artifact_is_nonzero(self) -> None:
        result = self.run_assemble(self.root / "strict", allow_missing=False)
        self.assertNotEqual(result.returncode, 0)
        manifest = json.loads(
            (self.root / "strict" / "release-manifest.json").read_text(encoding="utf-8")
        )
        gap_ids = {item["id"] for item in manifest["readiness"]["missing"]}
        self.assertIn("environments.zombienet", gap_ids)
        self.assertIn("environments.chopsticks", gap_ids)
        self.assertFalse(manifest["readiness"]["publishable"])

    def test_allow_missing_lists_gaps_and_hashes_content(self) -> None:
        output = self.root / "dry-run"
        result = self.run_assemble(output, allow_missing=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        manifest = json.loads((output / "release-manifest.json").read_text(encoding="utf-8"))
        self.assertEqual(manifest["schema"], "bleavit.release.v1")
        self.assertEqual(manifest["readiness"]["mode"], "allow-missing")
        self.assertEqual(
            manifest["mirror"], {"required": True, "status": "pending", "evidence": None}
        )
        self.assertEqual(
            manifest["supply_chain"]["summary"]["ignored_advisory_ids"],
            ["RUSTSEC-2026-0001"],
        )
        for artifact in manifest["artifacts"]:
            path = output / artifact["path"]
            self.assertTrue(path.name.startswith(artifact["sha256"] + "-"))
            self.assertEqual(sha256(path), artifact["sha256"])
            self.assertEqual(path.stat().st_size, artifact["size"])
        report = (output / "readiness-report.md").read_text(encoding="utf-8")
        self.assertIn("environments.zombienet", report)
        self.assertIn("B7", report)
        for name in ("release-manifest.json", "readiness-report.md"):
            friendly = output / name
            self.assertEqual(
                (output / "dist" / name).read_bytes(), friendly.read_bytes()
            )
            addressed = output / "dist" / f"{sha256(friendly)}-{name}"
            self.assertTrue(addressed.is_file())

    def test_manifest_release_blocker_gates_complete_surface_recording(self) -> None:
        manifest = json.loads(self.surface_manifest.read_text(encoding="utf-8"))
        manifest["release_blockers"] = [
            {
                "id": "b1b.compliance",
                "owner": "B1b",
                "reason": "SQ-140..SQ-150 remain open",
            }
        ]
        self.surface_manifest.write_text(json.dumps(manifest), encoding="utf-8")

        output = self.root / "manifest-blocker"
        result = self.run_assemble(output, allow_missing=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        release = json.loads(
            (output / "release-manifest.json").read_text(encoding="utf-8")
        )
        gaps = {item["id"]: item for item in release["readiness"]["missing"]}
        self.assertEqual(
            gaps["b1b.compliance"],
            {
                "id": "b1b.compliance",
                "owner": "B1b",
                "reason": "SQ-140..SQ-150 remain open",
            },
        )
        self.assertFalse(release["readiness"]["publishable"])

    def test_build_info_shape_validator(self) -> None:
        valid = json.loads((self.runtime / "build-info.json").read_text(encoding="utf-8"))
        self.assertEqual(ASSEMBLE.validate_build_info(valid), [])
        invalid = dict(valid)
        invalid["host_triple"] = ""
        errors = ASSEMBLE.validate_build_info(invalid)
        self.assertIn("build-info.host_triple must be a non-empty string", errors)

    def test_environment_evidence_validator_hashes_every_packaged_file(self) -> None:
        self.zombienet.mkdir()
        topology = self.zombienet / "topology.toml"
        topology.write_text("relay = true\n", encoding="utf-8")
        wasm_hash = sha256(self.runtime / "runtime.wasm")
        evidence = {
            "schema": "bleavit.env-evidence.v1",
            "suite": "zombienet",
            "runtime_wasm_sha256": wasm_hash,
            "artifact_hashes": {"topology.toml": sha256(topology)},
            "suites_run": [{"name": "collator-loss", "result": "pass"}],
            "recorded_at_commit": "a" * 40,
        }
        (self.zombienet / "run-evidence.json").write_text(
            json.dumps(evidence), encoding="utf-8"
        )
        self.assertEqual(
            ASSEMBLE.validate_run_evidence(
                self.zombienet, "zombienet", wasm_hash, "a" * 40
            ),
            [],
        )
        topology.write_text("tampered\n", encoding="utf-8")
        errors = ASSEMBLE.validate_run_evidence(
            self.zombienet, "zombienet", wasm_hash, "a" * 40
        )
        self.assertIn("artifact hash mismatch for topology.toml", errors)

    def test_allow_missing_still_fails_on_runtime_wasm_corruption(self) -> None:
        runtime_info = json.loads(
            (self.runtime / "runtime-info.json").read_text(encoding="utf-8")
        )
        runtime_info["on_chain_wasm_sha256"] = "0" * 64
        (self.runtime / "runtime-info.json").write_text(
            json.dumps(runtime_info), encoding="utf-8"
        )
        output = self.root / "corrupt-runtime"
        result = self.run_assemble(output, allow_missing=True)
        self.assertNotEqual(result.returncode, 0)
        manifest = json.loads((output / "release-manifest.json").read_text())
        self.assertEqual(manifest["readiness"]["corruption"][0]["id"], "runtime.wasm_binding")

    def test_allow_missing_still_fails_on_chain_spec_wasm_corruption(self) -> None:
        spec = self.specs / "bleavit-dev.json"
        spec.write_text(
            json.dumps({"genesis": {"runtimeGenesis": {"code": "0x00"}}}),
            encoding="utf-8",
        )
        output = self.root / "corrupt-spec"
        result = self.run_assemble(output, allow_missing=True)
        self.assertNotEqual(result.returncode, 0)
        manifest = json.loads((output / "release-manifest.json").read_text())
        ids = {item["id"] for item in manifest["readiness"]["corruption"]}
        self.assertIn("chain_specs.bleavit-dev.json.wasm_binding", ids)

    def test_environment_inventory_requires_bootnodes_for_live_rows(self) -> None:
        document = {
            "schema": "bleavit.environments.v1",
            "environments": [
                {
                    "id": "paseo",
                    "chain_spec": "bleavit-paseo.json",
                    "live": True,
                    "required": False,
                }
            ],
        }
        errors = ASSEMBLE.validate_environment_inventory(document)
        self.assertIn(
            "environments[0].bootnodes must be a safe repository-relative path when live",
            errors,
        )

    def test_sq101_blocker_has_explicit_followup_owner(self) -> None:
        self.assertEqual(
            ASSEMBLE.milestone_from_blocker("SQ-101 (B4 follow-up)"),
            "SQ-101 (B4 follow-up)",
        )

    def test_metadata_binding_corruption_fails_even_allow_missing(self) -> None:
        (self.runtime / "metadata.scale").write_bytes(b"tampered-after-extraction")
        output = self.root / "corrupt-metadata"
        result = self.run_assemble(output, allow_missing=True)
        self.assertNotEqual(result.returncode, 0)
        manifest = json.loads((output / "release-manifest.json").read_text())
        reasons = [item["reason"] for item in manifest["readiness"]["corruption"]]
        self.assertTrue(
            any("metadata_sha256" in reason for reason in reasons), reasons
        )

    def test_commit_mismatch_is_corruption_in_strict_and_gap_in_dry_run(self) -> None:
        build_info = json.loads(
            (self.runtime / "build-info.json").read_text(encoding="utf-8")
        )
        build_info["git_commit"] = "b" * 40
        (self.runtime / "build-info.json").write_text(
            json.dumps(build_info), encoding="utf-8"
        )
        strict = self.run_assemble(self.root / "commit-strict", allow_missing=False)
        self.assertNotEqual(strict.returncode, 0)
        manifest = json.loads(
            (self.root / "commit-strict" / "release-manifest.json").read_text()
        )
        corruption_ids = {item["id"] for item in manifest["readiness"]["corruption"]}
        self.assertIn("runtime.commit_binding", corruption_ids)
        dry = self.run_assemble(self.root / "commit-dry", allow_missing=True)
        self.assertEqual(dry.returncode, 0, dry.stderr)
        manifest = json.loads(
            (self.root / "commit-dry" / "release-manifest.json").read_text()
        )
        gap_ids = {item["id"] for item in manifest["readiness"]["missing"]}
        self.assertIn("runtime.commit_binding", gap_ids)

    def test_raw_chain_spec_code_binding_is_supported(self) -> None:
        wasm_hex = "0x" + (self.runtime / "runtime.wasm").read_bytes().hex()
        (self.specs / "bleavit-dev.json").write_text(
            json.dumps({"genesis": {"raw": {"top": {"0x3a636f6465": wasm_hex}}}}),
            encoding="utf-8",
        )
        output = self.root / "raw-spec"
        result = self.run_assemble(output, allow_missing=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        manifest = json.loads((output / "release-manifest.json").read_text())
        corruption_ids = {item["id"] for item in manifest["readiness"]["corruption"]}
        self.assertNotIn("chain_specs.bleavit-dev.json.wasm_binding", corruption_ids)

    def _corruption_ids(self, output: Path) -> set[str]:
        manifest = json.loads((output / "release-manifest.json").read_text())
        return {item["id"] for item in manifest["readiness"]["corruption"]}

    def test_fixture_report_from_another_runtime_is_corruption(self) -> None:
        report = json.loads(
            (self.fixtures / "fixtures-report.json").read_text(encoding="utf-8")
        )
        report["metadata_sha256"] = "0" * 64
        (self.fixtures / "fixtures-report.json").write_text(
            json.dumps(report), encoding="utf-8"
        )
        output = self.root / "stale-fixtures"
        result = self.run_assemble(output, allow_missing=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("chainhead.binding", self._corruption_ids(output))

    def test_fixture_report_must_cover_the_full_surface(self) -> None:
        manifest = json.loads(self.surface_manifest.read_text(encoding="utf-8"))
        manifest["entries"].append(
            {
                "id": "storage.constitution.params",
                "kind": "storage",
                "required": True,
                "citation": "02 §7.3",
            }
        )
        self.surface_manifest.write_text(json.dumps(manifest), encoding="utf-8")
        output = self.root / "truncated-fixtures"
        result = self.run_assemble(output, allow_missing=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("chainhead.binding", self._corruption_ids(output))

    def test_transcript_set_must_match_the_manifest_exactly(self) -> None:
        (self.fixtures / "storage.rogue.extra.json").write_text(
            '{"surface":"storage.rogue.extra","requests":[]}\n', encoding="utf-8"
        )
        output = self.root / "extra-transcript"
        result = self.run_assemble(output, allow_missing=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("chainhead.binding", self._corruption_ids(output))

    def test_fixture_report_schema_is_required(self) -> None:
        report = json.loads(
            (self.fixtures / "fixtures-report.json").read_text(encoding="utf-8")
        )
        del report["schema"]
        (self.fixtures / "fixtures-report.json").write_text(
            json.dumps(report), encoding="utf-8"
        )
        output = self.root / "schemaless-fixtures"
        result = self.run_assemble(output, allow_missing=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("chainhead.binding", self._corruption_ids(output))

    def test_live_environment_must_use_a_bootnode_enforcing_profile(self) -> None:
        document = {
            "schema": "bleavit.environments.v1",
            "environments": [
                {
                    "id": "dev",
                    "chain_spec": "bleavit-dev.json",
                    "live": True,
                    "required": True,
                    "bootnodes": "deploy/chain-specs/bootnodes.dev.json",
                }
            ],
        }
        errors = ASSEMBLE.validate_environment_inventory(document)
        self.assertTrue(
            any("only paseo/polkadot profiles enforce" in error for error in errors),
            errors,
        )


if __name__ == "__main__":
    unittest.main()

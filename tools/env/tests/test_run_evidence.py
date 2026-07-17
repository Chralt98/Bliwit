"""Execution and fail-closed tests for the SQ-139 evidence producer."""

from __future__ import annotations

import hashlib
import importlib.util
import json
import os
import signal
import shutil
import socket
import subprocess
import sys
import tempfile
import textwrap
import time
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[3]
SCRIPT = ROOT / "tools" / "env" / "run-evidence.py"
ASSEMBLE_SCRIPT = ROOT / "tools" / "release" / "assemble-release.py"
RUNNER_SPEC = importlib.util.spec_from_file_location(
    "run_evidence_for_env_tests", SCRIPT
)
assert RUNNER_SPEC is not None and RUNNER_SPEC.loader is not None
RUNNER = importlib.util.module_from_spec(RUNNER_SPEC)
sys.modules[RUNNER_SPEC.name] = RUNNER
RUNNER_SPEC.loader.exec_module(RUNNER)
ASSEMBLE_SPEC = importlib.util.spec_from_file_location(
    "assemble_release_for_env_tests", ASSEMBLE_SCRIPT
)
assert ASSEMBLE_SPEC is not None and ASSEMBLE_SPEC.loader is not None
ASSEMBLE = importlib.util.module_from_spec(ASSEMBLE_SPEC)
sys.modules[ASSEMBLE_SPEC.name] = ASSEMBLE
ASSEMBLE_SPEC.loader.exec_module(ASSEMBLE)

try:
    from websockets.sync.server import serve as _websockets_sync_server  # noqa: F401

    HAS_WEBSOCKETS_SYNC = True
except (ImportError, ModuleNotFoundError):
    HAS_WEBSOCKETS_SYNC = False


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


class RunEvidenceTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.wasm = self.root / "release-work" / "runtime" / "runtime.wasm"
        self.report = self.root / "target" / "env" / "run-report.json"
        self.logs = self.root / "target" / "env" / "logs"
        self.invocations = self.root / "target" / "env" / "zombienet-invocations"
        self._make_fixture()

    def tearDown(self) -> None:
        self.temporary.cleanup()

    @staticmethod
    def _write_executable(path: Path, text: str) -> None:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")
        path.chmod(0o755)

    @staticmethod
    def _available_port() -> int:
        try:
            with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as handle:
                handle.bind(("127.0.0.1", 0))
                return int(handle.getsockname()[1])
        except OSError:
            # The local code sandbox forbids sockets; websocket tests skip there.
            return 18765

    def _git(self, *arguments: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            ["git", *arguments],
            cwd=self.root,
            check=True,
            capture_output=True,
            text=True,
        )

    def _commit(self, message: str) -> str:
        self._git("add", "-A")
        self._git("commit", "-m", message)
        return self._git("rev-parse", "HEAD").stdout.strip()

    def _make_fixture(self) -> None:
        for path in (
            self.root / "zombienet" / "bin",
            self.root / "zombienet" / "drills",
            self.root / "zombienet" / "networks",
            self.root / "zombienet" / "specs" / "out",
            self.root / "chopsticks" / "scenarios",
            self.root / "tools" / "env",
            self.root / "tools" / "release",
            self.root / "target" / "release",
            self.wasm.parent,
            self.root / "fixture-bin",
            self.root / "support",
        ):
            path.mkdir(parents=True, exist_ok=True)

        for name in ("assemble-release.py", "release_common.py"):
            shutil.copy2(ROOT / "tools" / "release" / name, self.root / "tools" / "release" / name)

        (self.root / ".gitignore").write_text("target/\nrelease-work/\n", encoding="utf-8")
        (self.root / "zombienet" / "bin" / ".gitignore").write_text(
            "*\n!.gitignore\n", encoding="utf-8"
        )
        (self.root / "zombienet" / "specs" / "out" / ".gitignore").write_text(
            "*\n!.gitignore\n", encoding="utf-8"
        )
        (self.root / "chopsticks" / ".gitignore").write_text(
            ".state/\n", encoding="utf-8"
        )
        (self.root / "zombienet" / "README.md").write_text(
            "fixture zombienet definitions\n", encoding="utf-8"
        )
        (self.root / "chopsticks" / "README.md").write_text(
            "fixture chopsticks definitions\n", encoding="utf-8"
        )
        (self.root / "zombienet" / "networks" / "bleavit-local.toml").write_text(
            "[relaychain]\nchain = 'fixture'\n", encoding="utf-8"
        )
        for name in ("01-smoke", "03-keeper-loss", "09-soak"):
            (self.root / "zombienet" / "drills" / f"{name}.zndsl").write_text(
                "Network: ./zombienet/networks/bleavit-local.toml\n",
                encoding="utf-8",
            )

        port = self._available_port()
        scenario = textwrap.dedent(
            f"""\
            genesis: zombienet/specs/out/bleavit-drill-raw.json
            port: {port}
            db: chopsticks/.state/fixture.sqlite
            import-storage:
              - ["0x0102", "0xaabb"]
              - ["0x0304", null]
            """
        )
        (self.root / "chopsticks" / "bleavit.yml").write_text(
            scenario, encoding="utf-8"
        )

        chopsticks_scenarios = (
            "upgrade-transition",
            "stale-queue",
            "void-epoch",
            "precondition-failures",
            "pb-depeg",
            "pb-migration",
            "pb-oracle-void",
            "pb-halt-intake",
            "pb-reserve",
            "pb-ledger-freeze",
        )
        for index, name in enumerate(chopsticks_scenarios, start=1):
            (self.root / "chopsticks" / "scenarios" / f"{name}.yml").write_text(
                scenario.replace(
                    f"port: {port}", f"port: {port + index}"
                ).replace(
                    "chopsticks/.state/fixture.sqlite",
                    f"chopsticks/.state/{name}.sqlite",
                ),
                encoding="utf-8",
            )

        suites = {
            "schema": "bleavit.env-suites.v1",
            "suites": [
                {
                    "id": "01-smoke",
                    "kind": "zombienet",
                    "path": "zombienet/drills/01-smoke.zndsl",
                    "tier": "release",
                    "gated_on": [],
                    "timeout_seconds": 5,
                    "spec": "15 §4.7; 02 §11",
                },
                {
                    "id": "03-keeper-loss",
                    "kind": "zombienet",
                    "path": "zombienet/drills/03-keeper-loss.zndsl",
                    "tier": "release",
                    "gated_on": [],
                    "timeout_seconds": 5,
                    "spec": "15 §4.7; 09 §7.1",
                },
                {
                    "id": "09-soak",
                    "kind": "zombienet",
                    "path": "zombienet/drills/09-soak.zndsl",
                    "tier": "g1",
                    "gated_on": ["A8"],
                    "timeout_seconds": 5,
                    "spec": "15 §4.7; 09 §7.1",
                },
                {
                    "id": "base",
                    "kind": "chopsticks",
                    "path": "chopsticks/bleavit.yml",
                    "tier": "release",
                    "gated_on": [],
                    "timeout_seconds": 10,
                    "spec": "15 §4.7; 02 §11",
                },
                *[
                    {
                        "id": name,
                        "kind": "chopsticks",
                        "path": f"chopsticks/scenarios/{name}.yml",
                        "tier": "release",
                        "gated_on": ["SQ-151 card-depth execution"],
                        "timeout_seconds": 10,
                        "spec": "15 §4.7; 02 §11",
                    }
                    for name in chopsticks_scenarios
                ],
            ],
        }
        (self.root / "tools" / "env" / "suites.json").write_text(
            json.dumps(suites, indent=2) + "\n", encoding="utf-8"
        )
        self.wasm.write_bytes(b"release-wasm")
        code = "0x" + self.wasm.read_bytes().hex()
        (self.root / "zombienet" / "specs" / "out" / "paseo-local.json").write_text(
            "{}\n", encoding="utf-8"
        )
        (self.root / "zombienet" / "specs" / "out" / "bleavit-drill.json").write_text(
            json.dumps({"genesis": {"runtimeGenesis": {"code": code}}}) + "\n",
            encoding="utf-8",
        )
        (self.root / "zombienet" / "specs" / "out" / "bleavit-drill-raw.json").write_text(
            json.dumps({"genesis": {"raw": {"top": {"0x3a636f6465": code}}}}) + "\n",
            encoding="utf-8",
        )

        child = self.root / "support" / "group-child.py"
        child.write_text(
            textwrap.dedent(
                """\
                import os
                import signal
                import sys
                import time
                from pathlib import Path

                def terminated(_signum, _frame):
                    Path(os.environ["FAKE_ZOMBIENET_TERM_MARKER"]).write_text(
                        "terminated\\n", encoding="utf-8"
                    )
                    raise SystemExit(0)

                pid_marker = os.environ.get("FAKE_ZOMBIENET_PID_MARKER")
                if pid_marker:
                    Path(pid_marker).write_text(str(os.getpid()), encoding="utf-8")
                if os.environ.get("FAKE_ZOMBIENET_IGNORE_TERM") == "1":
                    signal.signal(signal.SIGTERM, signal.SIG_IGN)
                else:
                    signal.signal(signal.SIGTERM, terminated)
                while True:
                    time.sleep(1)
                """
            ),
            encoding="utf-8",
        )
        fake_zombienet = self.root / "zombienet" / "bin" / "zombienet"
        self._write_executable(
            fake_zombienet,
            textwrap.dedent(
                f"""\
                #!/bin/sh
                if [ -n "${{FAKE_ZOMBIENET_MARKER:-}}" ]; then
                    printf 'run\\n' >> "$FAKE_ZOMBIENET_MARKER"
                fi
                if [ "${{FAKE_ZOMBIENET_MUTATE_SPEC:-0}}" = 1 ]; then
                    printf '{{"genesis":{{"raw":{{"top":{{"0x3a636f6465":"0x00"}}}}}}}}\\n' \
                      > zombienet/specs/out/bleavit-drill-raw.json
                fi
                case "${{FAKE_ZOMBIENET_MODE:-pass}}" in
                  fail) exit 17 ;;
                  timeout)
                    "{sys.executable}" "{child}" &
                    wait $!
                    ;;
                  *) exit 0 ;;
                esac
                """
            ),
        )
        (self.root / "tools" / "env" / "pins.env").write_text(
            "CHOPSTICKS_VERSION=fixture-version\n"
            f"ZOMBIENET_SHA256={sha256(fake_zombienet)}\n",
            encoding="utf-8",
        )
        for name in (
            "polkadot",
            "polkadot-prepare-worker",
            "polkadot-execute-worker",
            "polkadot-parachain",
        ):
            self._write_executable(
                self.root / "zombienet" / "bin" / name,
                "#!/bin/sh\nexit 0\n",
            )
        self._write_executable(
            self.root / "target" / "release" / "bleavit-node",
            "#!/bin/sh\nexit 0\n",
        )

        self.fake_chopsticks = self.root / "support" / "fake-chopsticks.py"
        self.fake_chopsticks.write_text(
            textwrap.dedent(
                """\
                import argparse
                import json
                import os
                from pathlib import Path

                import yaml
                from websockets.sync.server import serve

                parser = argparse.ArgumentParser()
                parser.add_argument("--config", type=Path, required=True)
                args = parser.parse_args()
                config = yaml.safe_load(args.config.read_text(encoding="utf-8"))
                if os.environ.get("FAKE_CHOPSTICKS_REQUIRE_CLEAN") == "1":
                    database = Path(config["db"])
                    if list(database.parent.glob(database.name + "*")):
                        raise SystemExit("stale Chopsticks state was not removed")
                storage = dict(config.get("import-storage", []))
                raw_spec = json.loads(Path(config["genesis"]).read_text(encoding="utf-8"))
                runtime_code = raw_spec["genesis"]["raw"]["top"]["0x3a636f6465"]
                block = 0

                def handler(websocket):
                    global block
                    for raw in websocket:
                        request = json.loads(raw)
                        method = request["method"]
                        if method == "system_health":
                            result = {"isSyncing": False}
                        elif method == "state_getStorage":
                            key = request.get("params", [None])[0]
                            result = (
                                runtime_code
                                if key.lower() == "0x3a636f6465"
                                else storage.get(key)
                            )
                            if (
                                os.environ.get("FAKE_CHOPSTICKS_MODE") == "wrong-storage"
                                and key.lower() != "0x3a636f6465"
                            ):
                                result = "0xffff"
                            elif (
                                os.environ.get("FAKE_CHOPSTICKS_MODE") == "wrong-code"
                                and key.lower() == "0x3a636f6465"
                            ):
                                result = "0xffff"
                        elif method == "dev_newBlock":
                            block += 1
                            result = True
                        elif method == "chain_getHeader":
                            result = {"number": hex(block)}
                        else:
                            websocket.send(json.dumps({
                                "jsonrpc": "2.0",
                                "id": request.get("id"),
                                "error": {"code": -32601, "message": "unknown method"},
                            }))
                            continue
                        websocket.send(json.dumps({
                            "jsonrpc": "2.0", "id": request.get("id"), "result": result
                        }))

                with serve(handler, "127.0.0.1", int(config["port"])) as server:
                    server.serve_forever()
                """
            ),
            encoding="utf-8",
        )
        self._write_executable(
            self.root / "fixture-bin" / "npx",
            textwrap.dedent(
                f"""\
                #!/bin/sh
                while [ "$#" -gt 0 ] && [ "$1" != "--config" ]; do
                    shift
                done
                exec "{sys.executable}" "{self.fake_chopsticks}" "$@"
                """
            ),
        )

        self._git("init")
        self._git("config", "user.email", "sq139-tests@example.invalid")
        self._git("config", "user.name", "SQ-139 tests")
        self.commit = self._commit("fixture")

    def run_runner(
        self,
        *arguments: str,
        environment: dict[str, str] | None = None,
        commit: str | None = None,
        timeout: float = 30,
    ) -> subprocess.CompletedProcess[str]:
        command = [
            sys.executable,
            str(SCRIPT),
            "--root",
            str(self.root),
            "--wasm",
            str(self.wasm),
            "--commit",
            commit or self.commit,
            "--log-dir",
            str(self.logs),
            "--report-out",
            str(self.report),
            *arguments,
        ]
        process_environment = dict(os.environ)
        process_environment["PATH"] = (
            str(self.root / "fixture-bin") + os.pathsep + process_environment["PATH"]
        )
        process_environment["FAKE_ZOMBIENET_MARKER"] = str(self.invocations)
        if environment:
            process_environment.update(environment)
        return subprocess.run(
            command,
            cwd=self.root,
            check=False,
            capture_output=True,
            text=True,
            env=process_environment,
            timeout=timeout,
        )

    def read_report_rows(self) -> dict[str, dict[str, object]]:
        report = json.loads(self.report.read_text(encoding="utf-8"))
        self.assertEqual(report["schema"], "bleavit.env-run-report.v1")
        return {row["id"]: row for row in report["rows"]}

    def assert_no_evidence(self) -> None:
        for name in ("zombienet", "chopsticks"):
            self.assertFalse((self.root / name / "run-evidence.json").exists())

    def synthetic_passing_rows(
        self, *kinds: str
    ) -> tuple[list[object], list[dict[str, object]]]:
        suites = RUNNER.load_manifest(self.root)
        selected_kinds = set(kinds or RUNNER.KINDS)
        rows: list[dict[str, object]] = []
        for suite in suites:
            if suite.kind not in selected_kinds or suite.tier != "release":
                continue
            checks = (
                ["zndsl"]
                if suite.kind == "zombienet"
                else ["boot", "injected-state", "blocks", "code-binding"]
            )
            rows.append(
                {
                    "id": suite.identifier,
                    "kind": suite.kind,
                    "result": "pass",
                    "duration_seconds": 0.001,
                    "gated_on": list(suite.gated_on),
                    "checks": [*checks, RUNNER.TRY_STATE_CHECK],
                }
            )
        return suites, rows

    def emit_synthetic_evidence(
        self, *kinds: str, commit: str | None = None
    ) -> list[str]:
        suites, rows = self.synthetic_passing_rows(*kinds)
        return RUNNER.emit_evidence(
            self.root,
            suites,
            rows,
            self.wasm,
            sha256(self.wasm),
            commit or self.commit,
            "release",
        )

    def test_fully_green_run_produces_evidence_accepted_by_real_consumer(self) -> None:
        produced = self.emit_synthetic_evidence()
        self.assertEqual(produced, ["chopsticks", "zombienet"])
        wasm_hash = sha256(self.wasm)
        for suite in ("zombienet", "chopsticks"):
            directory = self.root / suite
            evidence = json.loads(
                (directory / "run-evidence.json").read_text(encoding="utf-8")
            )
            self.assertEqual(evidence["tier"], "release")
            self.assertEqual(
                evidence["suites_manifest_sha256"],
                sha256(self.root / "tools" / "env" / "suites.json"),
            )
            self.assertEqual(
                evidence["pins_env_sha256"],
                sha256(self.root / "tools" / "env" / "pins.env"),
            )
            expected_checks = (
                ["zndsl", RUNNER.TRY_STATE_CHECK]
                if suite == "zombienet"
                else [
                    "boot",
                    "injected-state",
                    "blocks",
                    "code-binding",
                    RUNNER.TRY_STATE_CHECK,
                ]
            )
            self.assertTrue(evidence["suites_run"])
            for row in evidence["suites_run"]:
                self.assertEqual(row["checks"], expected_checks)
            errors = ASSEMBLE.validate_run_evidence(
                directory, suite, wasm_hash, self.commit
            )
            self.assertEqual(errors, [], f"{suite}: {errors}")

    def test_fully_green_cli_run_is_blocked_without_try_state(self) -> None:
        result = self.run_runner("--kind", "zombienet")

        self.assertNotEqual(result.returncode, 0)
        output = result.stdout + result.stderr
        self.assertIn("SQ-152", output)
        self.assertIn("01-smoke, 03-keeper-loss", output)
        self.assert_no_evidence()

    @unittest.skipUnless(HAS_WEBSOCKETS_SYNC, "websockets.sync requires websockets 15.x")
    def test_default_chopsticks_release_run_skips_gated_scenarios(self) -> None:
        result = self.run_runner("--kind", "chopsticks", "--tier", "release")

        self.assertNotEqual(result.returncode, 0)
        rows = self.read_report_rows()
        self.assertEqual(rows["base"]["result"], "pass")
        scenario_ids = (
            "upgrade-transition",
            "stale-queue",
            "void-epoch",
            "precondition-failures",
            "pb-depeg",
            "pb-migration",
            "pb-oracle-void",
            "pb-halt-intake",
            "pb-reserve",
            "pb-ledger-freeze",
        )
        for identifier in scenario_ids:
            self.assertEqual(rows[identifier]["result"], "skipped-gated")
            self.assertEqual(
                rows[identifier]["gated_on"], ["SQ-151 card-depth execution"]
            )
            self.assertIn(identifier, result.stdout + result.stderr)
        self.assertIn("SQ-151 card-depth execution", result.stdout + result.stderr)
        self.assert_no_evidence()

    def test_failing_suite_returns_nonzero_without_evidence(self) -> None:
        result = self.run_runner(
            "--kind",
            "zombienet",
            environment={"FAKE_ZOMBIENET_MODE": "fail"},
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.read_report_rows()["01-smoke"]["result"], "fail")
        self.assert_no_evidence()

    def test_explicit_suite_subset_runs_in_report_only_mode(self) -> None:
        result = self.run_runner("--kind", "zombienet", "--suites", "01-smoke")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(self.invocations.is_file())
        self.assertEqual(self.read_report_rows()["01-smoke"]["result"], "pass")
        self.assert_no_evidence()

    def test_gated_suite_is_skipped_then_attempted_when_included(self) -> None:
        suites_path = self.root / "tools" / "env" / "suites.json"
        suites = json.loads(suites_path.read_text(encoding="utf-8"))
        suites["suites"][1]["gated_on"] = ["B9-topology"]
        suites_path.write_text(json.dumps(suites), encoding="utf-8")
        skipped = self.run_runner(
            "--kind",
            "zombienet",
            "--suites",
            "03-keeper-loss",
            "--no-evidence",
        )
        self.assertEqual(skipped.returncode, 0, skipped.stderr)
        self.assertEqual(
            self.read_report_rows()["03-keeper-loss"]["result"], "skipped-gated"
        )
        self.assertFalse(self.invocations.exists())

        attempted = self.run_runner(
            "--kind",
            "zombienet",
            "--suites",
            "03-keeper-loss",
            "--include-gated",
            "--no-evidence",
        )
        self.assertEqual(attempted.returncode, 0, attempted.stderr)
        self.assertEqual(self.read_report_rows()["03-keeper-loss"]["result"], "pass")
        self.assertTrue(self.invocations.is_file())

    def test_release_tier_records_g1_as_excluded(self) -> None:
        result = self.run_runner(
            "--kind", "zombienet", "--tier", "release", "--no-evidence"
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        rows = self.read_report_rows()
        self.assertEqual(rows["01-smoke"]["result"], "pass")
        self.assertEqual(rows["09-soak"]["result"], "excluded-tier")

    def test_gated_release_suite_blocks_evidence_with_gate_listed(self) -> None:
        suites_path = self.root / "tools" / "env" / "suites.json"
        suites = json.loads(suites_path.read_text(encoding="utf-8"))
        suites["suites"][0]["gated_on"] = ["smoke staging gate"]
        suites["suites"][1]["gated_on"] = ["B9 topology wiring"]
        suites_path.write_text(json.dumps(suites), encoding="utf-8")
        self.commit = self._commit("gate release suite")

        result = self.run_runner("--kind", "zombienet")

        self.assertNotEqual(result.returncode, 0)
        output = result.stdout + result.stderr
        self.assertIn("03-keeper-loss", output)
        self.assertIn("B9 topology wiring", output)
        self.assert_no_evidence()

    def test_non_release_tiers_force_report_only_mode(self) -> None:
        for tier in ("all", "g1"):
            with self.subTest(tier=tier):
                result = self.run_runner("--kind", "zombienet", "--tier", tier)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertIn(
                    f"--tier {tier}",
                    (result.stdout + result.stderr),
                )
                self.assertIn("report-only", (result.stdout + result.stderr).lower())
                self.assert_no_evidence()

    def test_custom_zombienet_binary_forces_report_only_mode(self) -> None:
        custom = self.root / "support" / "custom-zombienet"
        shutil.copy2(self.root / "zombienet" / "bin" / "zombienet", custom)
        custom.chmod(0o755)

        result = self.run_runner(
            "--kind", "zombienet", "--zombienet-binary", str(custom)
        )

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("--zombienet-binary", result.stdout + result.stderr)
        self.assertIn("report-only", (result.stdout + result.stderr).lower())
        self.assert_no_evidence()

    def test_no_evidence_rerun_removes_preexisting_evidence(self) -> None:
        for kind in ("zombienet", "chopsticks"):
            (self.root / kind / "run-evidence.json").write_text(
                '{"stale":true}\n', encoding="utf-8"
            )

        result = self.run_runner("--kind", "zombienet", "--no-evidence")

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assert_no_evidence()

    def test_malformed_manifest_failure_removes_preexisting_evidence(self) -> None:
        for kind in ("zombienet", "chopsticks"):
            (self.root / kind / "run-evidence.json").write_text(
                '{"stale":true}\n', encoding="utf-8"
            )
        (self.root / "tools" / "env" / "suites.json").write_text(
            "{not-json\n", encoding="utf-8"
        )

        result = self.run_runner("--no-evidence")

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("cannot read suites.json", result.stderr + result.stdout)
        self.assert_no_evidence()

    def test_duplicate_manifest_path_is_refused(self) -> None:
        suites_path = self.root / "tools" / "env" / "suites.json"
        suites = json.loads(suites_path.read_text(encoding="utf-8"))
        suites["suites"][1]["path"] = suites["suites"][0]["path"]
        suites_path.write_text(json.dumps(suites), encoding="utf-8")

        result = self.run_runner("--no-evidence")

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("duplicate suite path", result.stderr + result.stdout)

    def test_dirty_tools_env_tree_refuses_evidence(self) -> None:
        (self.root / "tools" / "env" / "untracked.txt").write_text(
            "dirty\n", encoding="utf-8"
        )

        with self.assertRaisesRegex(RUNNER.EvidenceError, "untracked.txt"):
            self.emit_synthetic_evidence("zombienet")
        self.assert_no_evidence()

    def test_wasm_chain_spec_mismatch_fails_before_a_suite_runs(self) -> None:
        path = self.root / "zombienet" / "specs" / "out" / "bleavit-drill.json"
        path.write_text(
            json.dumps({"genesis": {"runtimeGenesis": {"code": "0x00"}}}),
            encoding="utf-8",
        )
        result = self.run_runner("--kind", "zombienet")
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(self.invocations.exists())
        self.assertIn("wasm", (result.stderr + result.stdout).lower())
        self.assert_no_evidence()

    def test_dirty_environment_tree_refuses_evidence(self) -> None:
        (self.root / "chopsticks" / "stray.txt").write_text("dirty\n", encoding="utf-8")
        with self.assertRaisesRegex(RUNNER.EvidenceError, "stray.txt"):
            self.emit_synthetic_evidence("zombienet")
        self.assert_no_evidence()

    def test_head_must_equal_requested_commit(self) -> None:
        with self.assertRaisesRegex(RUNNER.EvidenceError, "does not equal git HEAD"):
            self.emit_synthetic_evidence("zombienet", commit="b" * 40)
        self.assert_no_evidence()

    def test_symlink_in_environment_inventory_refuses_evidence(self) -> None:
        link = self.root / "zombienet" / "linked-readme"
        try:
            link.symlink_to("README.md")
        except OSError as error:
            self.skipTest(f"symlinks unavailable: {error}")
        self.commit = self._commit("add symlink")
        with self.assertRaisesRegex(RUNNER.EvidenceError, "symlink"):
            self.emit_synthetic_evidence("zombienet")
        self.assert_no_evidence()

    def test_cleanup_removes_generated_state_but_preserves_gitignores(self) -> None:
        state = self.root / "chopsticks" / ".state"
        state.mkdir()
        (state / "fixture.sqlite").write_bytes(b"generated")
        self.emit_synthetic_evidence("zombienet")
        self.assertEqual(
            sorted(path.name for path in (self.root / "zombienet" / "bin").iterdir()),
            [".gitignore"],
        )
        self.assertEqual(
            sorted(
                path.name
                for path in (self.root / "zombienet" / "specs" / "out").iterdir()
            ),
            [".gitignore"],
        )
        self.assertFalse(state.exists())

    @unittest.skipUnless(HAS_WEBSOCKETS_SYNC, "websockets.sync requires websockets 15.x")
    def test_chopsticks_storage_mismatch_fails_suite(self) -> None:
        result = self.run_runner(
            "--kind",
            "chopsticks",
            "--no-evidence",
            environment={"FAKE_CHOPSTICKS_MODE": "wrong-storage"},
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.read_report_rows()["base"]["result"], "fail")
        self.assert_no_evidence()

    @unittest.skipUnless(HAS_WEBSOCKETS_SYNC, "websockets.sync requires websockets 15.x")
    def test_chopsticks_live_code_mismatch_fails_suite(self) -> None:
        result = self.run_runner(
            "--kind",
            "chopsticks",
            "--no-evidence",
            environment={"FAKE_CHOPSTICKS_MODE": "wrong-code"},
        )

        self.assertNotEqual(result.returncode, 0)
        row = self.read_report_rows()["base"]
        self.assertEqual(row["result"], "fail")
        self.assertIn("sha256", str(row.get("detail", "")).lower())
        self.assert_no_evidence()

    def test_invalid_chopsticks_binding_config_fails_loudly(self) -> None:
        path = self.root / "chopsticks" / "bleavit.yml"
        original = path.read_text(encoding="utf-8")
        invalid = {
            "active wasm-override": (
                original + "wasm-override: target/runtime.wasm\n",
                "wasm-override",
            ),
            "runtime code injection": (
                original.replace('"0x0102"', '"0x3A636F6465"', 1),
                "0x3a636f6465",
            ),
            "db outside state": (
                original.replace(
                    "db: chopsticks/.state/fixture.sqlite",
                    "db: target/fixture.sqlite",
                    1,
                ),
                "chopsticks/.state",
            ),
            "wrong genesis": (
                original.replace(
                    "genesis: zombienet/specs/out/bleavit-drill-raw.json",
                    "genesis: zombienet/specs/out/other.json",
                    1,
                ),
                "genesis",
            ),
        }
        for label, (contents, fragment) in invalid.items():
            with self.subTest(case=label):
                self.report.unlink(missing_ok=True)
                path.write_text(contents, encoding="utf-8")
                try:
                    result = self.run_runner(
                        "--kind", "chopsticks", "--no-evidence"
                    )
                    self.assertNotEqual(result.returncode, 0)
                    output = result.stdout + result.stderr
                    if self.report.is_file():
                        output += self.report.read_text(encoding="utf-8")
                    self.assertIn(fragment, output.lower())
                    self.assert_no_evidence()
                finally:
                    path.write_text(original, encoding="utf-8")

    @unittest.skipUnless(HAS_WEBSOCKETS_SYNC, "websockets.sync requires websockets 15.x")
    def test_occupied_chopsticks_port_fails_suite(self) -> None:
        config = (self.root / "chopsticks" / "bleavit.yml").read_text(
            encoding="utf-8"
        )
        port_line = next(line for line in config.splitlines() if line.startswith("port:"))
        port = int(port_line.split(":", 1)[1].strip())
        blocker = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        try:
            blocker.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            blocker.bind(("127.0.0.1", port))
            blocker.listen()
        except OSError as error:
            blocker.close()
            self.skipTest(f"local sockets unavailable: {error}")
        try:
            result = self.run_runner("--kind", "chopsticks", "--no-evidence")
        finally:
            blocker.close()

        self.assertNotEqual(result.returncode, 0)
        row = self.read_report_rows()["base"]
        self.assertEqual(row["result"], "fail")
        self.assertIn("port", str(row.get("detail", "")).lower())
        self.assert_no_evidence()

    def test_duplicate_chopsticks_database_is_refused(self) -> None:
        scenario = self.root / "chopsticks" / "scenarios" / "duplicate.yml"
        scenario.write_text(
            textwrap.dedent(
                """\
                genesis: zombienet/specs/out/bleavit-drill-raw.json
                port: 18766
                db: chopsticks/.state/fixture.sqlite
                import-storage: []
                """
            ),
            encoding="utf-8",
        )
        suites_path = self.root / "tools" / "env" / "suites.json"
        suites = json.loads(suites_path.read_text(encoding="utf-8"))
        suites["suites"].append(
            {
                "id": "duplicate-db",
                "kind": "chopsticks",
                "path": "chopsticks/scenarios/duplicate.yml",
                "tier": "release",
                "gated_on": [],
                "timeout_seconds": 5,
                "spec": "15 §4.7; 02 §11",
            }
        )
        suites_path.write_text(json.dumps(suites), encoding="utf-8")

        result = self.run_runner("--kind", "chopsticks", "--no-evidence")

        self.assertNotEqual(result.returncode, 0)
        output = (result.stdout + result.stderr).lower()
        self.assertIn("duplicate", output)
        self.assertIn("db", output)
        self.assertIn("chopsticks/.state/fixture.sqlite", output)
        self.assert_no_evidence()

    @unittest.skipUnless(HAS_WEBSOCKETS_SYNC, "websockets.sync requires websockets 15.x")
    def test_chopsticks_stale_database_is_removed_before_startup(self) -> None:
        state = self.root / "chopsticks" / ".state"
        state.mkdir()
        (state / "fixture.sqlite").write_bytes(b"previous failed run")
        (state / "fixture.sqlite-wal").write_bytes(b"previous sqlite sidecar")
        result = self.run_runner(
            "--kind",
            "chopsticks",
            "--no-evidence",
            environment={"FAKE_CHOPSTICKS_REQUIRE_CLEAN": "1"},
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(state.exists())

    def test_timeout_terminates_the_spawned_process_group(self) -> None:
        suites_path = self.root / "tools" / "env" / "suites.json"
        suites = json.loads(suites_path.read_text(encoding="utf-8"))
        suites["suites"][0]["timeout_seconds"] = 1
        suites_path.write_text(json.dumps(suites), encoding="utf-8")
        pid_marker = self.root / "target" / "env" / "stubborn-child-pid"
        result = self.run_runner(
            "--kind",
            "zombienet",
            "--suites",
            "01-smoke",
            "--no-evidence",
            environment={
                "FAKE_ZOMBIENET_MODE": "timeout",
                "FAKE_ZOMBIENET_IGNORE_TERM": "1",
                "FAKE_ZOMBIENET_PID_MARKER": str(pid_marker),
            },
            timeout=20,
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertTrue(pid_marker.is_file(), result.stderr)
        child_pid = int(pid_marker.read_text(encoding="utf-8"))
        deadline = time.monotonic() + 3
        child_alive = True
        while child_alive and time.monotonic() < deadline:
            try:
                os.kill(child_pid, 0)
            except ProcessLookupError:
                child_alive = False
                break
            time.sleep(0.05)
        if child_alive:
            os.kill(child_pid, signal.SIGKILL)
        self.assertFalse(child_alive, "SIGTERM-ignoring process-group child survived timeout")
        self.assertEqual(self.read_report_rows()["01-smoke"]["result"], "fail")

    def test_zombienet_digest_mismatch_refuses_evidence(self) -> None:
        pins = self.root / "tools" / "env" / "pins.env"
        text = pins.read_text(encoding="utf-8")
        text = text.replace(
            f"ZOMBIENET_SHA256={sha256(self.root / 'zombienet' / 'bin' / 'zombienet')}",
            "ZOMBIENET_SHA256=" + "0" * 64,
        )
        pins.write_text(text, encoding="utf-8")
        self.commit = self._commit("mismatch zombienet digest")

        result = self.run_runner("--kind", "zombienet")

        self.assertNotEqual(result.returncode, 0)
        output = (result.stdout + result.stderr).lower()
        self.assertIn("zombienet", output)
        self.assertIn("sha256", output)
        self.assert_no_evidence()

    def test_post_suite_chain_spec_mutation_refuses_evidence(self) -> None:
        raw_spec = self.root / "zombienet" / "specs" / "out" / "bleavit-drill-raw.json"
        raw_spec.write_text(
            '{"genesis":{"raw":{"top":{"0x3a636f6465":"0x00"}}}}\n',
            encoding="utf-8",
        )

        with self.assertRaisesRegex(
            RUNNER.EvidenceError, "does not match release runtime.wasm"
        ):
            self.emit_synthetic_evidence("zombienet")
        self.assert_no_evidence()

    def test_stale_evidence_is_replaced_without_self_hashing(self) -> None:
        evidence_path = self.root / "zombienet" / "run-evidence.json"
        evidence_path.write_text('{"stale":true}\n', encoding="utf-8")
        RUNNER.remove_prior_evidence(self.root)
        self.emit_synthetic_evidence("zombienet")
        evidence = json.loads(evidence_path.read_text(encoding="utf-8"))
        self.assertEqual(evidence["schema"], "bleavit.env-evidence.v1")
        self.assertNotIn("run-evidence.json", evidence["artifact_hashes"])
        errors = ASSEMBLE.validate_run_evidence(
            self.root / "zombienet", "zombienet", sha256(self.wasm), self.commit
        )
        self.assertEqual(errors, [])


if __name__ == "__main__":
    unittest.main()

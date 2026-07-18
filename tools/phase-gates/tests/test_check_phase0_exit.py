"""Fail-closed tests for the G0 Phase-0 exit evidence gate."""

from __future__ import annotations

import contextlib
import copy
import hashlib
import importlib.util
import io
import json
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock


ROOT = Path(__file__).resolve().parents[3]
SCRIPT = ROOT / "tools" / "phase-gates" / "check-phase0-exit.py"
SPEC = importlib.util.spec_from_file_location("check_phase0_exit_for_tests", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
GATE = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = GATE
SPEC.loader.exec_module(GATE)

HEAD = "ab" * 20

CORPUS_FAMILY_CONSUMERS = {
    "decision_scenarios": (),
    "high_precision_corpus": ("fixed-reference-vectors",),
    "ledger_error_scenarios": ("ledger-core-reference-vectors",),
    # This legacy family has no literal Rust consumer anywhere in crates/ or
    # pallets/. Keep the fixture honest even though the review brief broadly
    # describes the ledger families as consumed.
    "ledger_scenarios": (),
    "ledger_score_scenarios": ("ledger-core-reference-vectors",),
    "ledger_sequence_scenarios": ("ledger-core-reference-vectors",),
    "ledger_sweep_scenarios": ("ledger-pallet-reference-sweep",),
    "lmsr_maker_example": (),
    "lmsr_vectors": ("fixed-reference-vectors",),
    "transcendental_corpus": ("fixed-reference-vectors",),
    "treasury_scenarios": (),
    "twap_scenarios": (),
    "welfare_scenarios": (),
}

TEST_OUTPUTS = {
    "python-reference-model": GATE.CommandResult(
        0, stderr="Ran 26 tests in 0.123s\n\nOK\n"
    ),
    "fixed-reference-vectors": GATE.CommandResult(
        0, stdout="test result: ok. 3 passed; 0 failed; 0 ignored\n"
    ),
    "ledger-core-reference-vectors": GATE.CommandResult(
        0, stdout="test result: ok. 3 passed; 0 failed; 0 ignored\n"
    ),
    "ledger-pallet-core-differential": GATE.CommandResult(
        0, stdout="test result: ok. 1 passed; 0 failed; 0 ignored\n"
    ),
    "ledger-pallet-reference-sweep": GATE.CommandResult(
        0, stdout="test result: ok. 1 passed; 0 failed; 0 ignored\n"
    ),
    "fixed-full-sweep": GATE.CommandResult(
        0, stdout="test result: ok. 1 passed; 0 failed; 0 ignored\n"
    ),
}


def doc13_fixture() -> str:
    """Return a minimal registry that preserves Doc 13's lexical contracts."""

    return """\
# Parameter registry fixture

Every default is a **simulation hypothesis** unless marked frozen; Phase 0–3 calibration obligations are tagged *sim-gated*.

6. **`ParamKey` encoding.** **Per-class rows** (`dec.delta`, `dec.sigma`) materialize as four keys with the class suffixes `.param` / `.trs` / `.code` / `.meta`.

## 1. Registry

| Key | Type | Unit | Default | Hard min | Hard max | Max Δ/decision | Cooldown | Class | Doc |
|---|---|---|---|---|---|---|---|---|---|
| `dec.delta` δ per class | Fixed | s-units | 0.015 / 0.025 / 0.040 / 0.060 | 0.005 | 0.10 | 0.005 | 2 | META | fixture |
| `pol.b_baseline` | Balance | USDC | **25,000** *(sim-gated — [VERIFY via Phase-0/3 calibration])* | 10,000 | 100,000 | 25% | 1 | TREASURY | fixture |
| `sec.prize.param` / `sec.prize.code` / `sec.prize.meta` | Balance | USDC | **[VERIFY — derived in Phase-0 calibration; sim-gated]** | — | — | ×2 | 2 | META | fixture |
| `sec.flow_cap` | Fixed | multiplier | **[VERIFY — Phase-0 calibration; sim-gated]** | — | — | ×2 | 2 | META | fixture |
| `future.renamed_cap` | Balance | USDC | *(sim-gated **[VERIFY before Phase-3 arming]**)* | — | — | — | — | META | fixture |
| `future.another_cap` | Balance | USDC | *(sim-gated, as above)* | — | — | — | — | META | fixture |

## 2. Payload bounds
"""


def valid_calibration() -> dict[str, float | int]:
    return {
        "dec.delta.param": 0.05,
        "dec.delta.trs": 0.05,
        "dec.delta.code": 0.05,
        "dec.delta.meta": 0.05,
        "pol.b_baseline": 25_000,
        "sec.prize.param": 1_000,
        "sec.prize.code": 2_000,
        "sec.prize.meta": 3_000,
        "sec.flow_cap": 2.0,
    }


def valid_sim_document() -> dict[str, object]:
    return {
        "schema": GATE.SIM_SCHEMA,
        "git_commit": HEAD,
        "synthetic_proposals": 10_000,
        "false_pass_rate": {
            "param": 0.0099,
            "trs": 0.0099,
            "code": 0.0099,
            "meta": 0.0099,
        },
        "attack_cost_validation": {
            "validated": True,
            "method": "measured-depth adversarial replay",
        },
        "calibration": valid_calibration(),
    }


def corpus_fixture() -> dict[str, object]:
    """Return the actual v4 top-level family inventory with tiny values."""

    document: dict[str, object] = {
        "schema": "bleavit.reference-model.v4",
        "precision": "fixture",
    }
    for family in CORPUS_FAMILY_CONSUMERS:
        document[family] = []
    return document


class StubRunner:
    """Record every process request and return deterministic stub results."""

    def __init__(
        self,
        exit_codes: dict[tuple[str, ...], int] | None = None,
        *,
        results: dict[tuple[str, ...], GATE.CommandResult] | None = None,
        status_outputs: list[str] | None = None,
    ) -> None:
        self.exit_codes = exit_codes or {}
        self.results = results or {}
        self.status_outputs = list(status_outputs or ("", ""))
        self.calls: list[dict[str, object]] = []

    def run(
        self,
        command: tuple[str, ...] | list[str],
        *,
        cwd: Path,
        env: dict[str, str] | None = None,
        capture_output: bool = False,
    ) -> object:
        key = tuple(command)
        self.calls.append(
            {
                "command": key,
                "cwd": cwd,
                "env": env,
                "capture_output": capture_output,
            }
        )
        if key == ("git", "rev-parse", "HEAD"):
            code = self.exit_codes.get(key, 0)
            return GATE.CommandResult(
                code,
                stdout=HEAD + "\n" if code == 0 else "",
                stderr="fixture git failure" if code else "",
            )
        if key[:2] == ("git", "status"):
            stdout = self.status_outputs.pop(0) if self.status_outputs else ""
            return GATE.CommandResult(self.exit_codes.get(key, 0), stdout=stdout)
        if key in self.results:
            return self.results[key]
        ordinary, _generate, sweep = GATE.reference_legs(Path("/fixture-sweep"))
        for leg in ordinary:
            if key == leg.command and leg.identifier in TEST_OUTPUTS:
                result = TEST_OUTPUTS[leg.identifier]
                return GATE.CommandResult(
                    self.exit_codes.get(key, result.exit_code),
                    stdout=result.stdout,
                    stderr=result.stderr,
                )
        # The full-sweep command is outside the ordinary-leg list.
        if key == sweep.command:
            result = TEST_OUTPUTS["fixed-full-sweep"]
            return GATE.CommandResult(
                self.exit_codes.get(key, result.exit_code),
                stdout=result.stdout,
                stderr=result.stderr,
            )
        return GATE.CommandResult(self.exit_codes.get(key, 0))


class PhaseGateTestCase(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.doc13 = self.root / "docs" / "architecture" / "13-parameters.md"
        self.doc13.parent.mkdir(parents=True)
        self.doc13.write_text(doc13_fixture(), encoding="utf-8")
        self.vectors = self.root / "reference-model" / "fixtures" / "vectors.json"
        self.vectors.parent.mkdir(parents=True)
        self.vectors.write_text(json.dumps(corpus_fixture()), encoding="utf-8")
        self.report = self.root / "phase0-report.json"
        self.sim = self.root / "sim.json"

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def write_sim(self, document: object) -> bytes:
        raw = (json.dumps(document, sort_keys=True) + "\n").encode()
        self.sim.write_bytes(raw)
        return raw

    def evidence(self, document: object | None = None) -> object:
        self.write_sim(valid_sim_document() if document is None else document)
        return GATE.load_sim_evidence(self.sim)

    def registry(self) -> object:
        return GATE.load_calibration_registry(self.doc13)

    def invoke_setup_error(
        self, runner: StubRunner, *extra: str
    ) -> tuple[int, str, str]:
        stdout = io.StringIO()
        stderr = io.StringIO()
        arguments = [
            "--root",
            str(self.root),
            "--report-out",
            str(self.report),
            *extra,
        ]
        with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
            code = GATE.main(arguments, runner=runner)
        return code, stdout.getvalue(), stderr.getvalue()

    def invoke(
        self,
        runner: StubRunner,
        *,
        sim: bool | str = True,
        reduced: bool = False,
        sweep_dir: bool = True,
        allow_dirty: bool = False,
    ) -> tuple[int, dict[str, object], str, str]:
        arguments = ["--root", str(self.root), "--report-out", str(self.report)]
        if sim is True:
            if not self.sim.exists():
                self.write_sim(valid_sim_document())
            arguments.extend(("--sim-evidence", str(self.sim)))
        elif sim == "missing":
            arguments.extend(("--sim-evidence", str(self.sim)))
        if reduced:
            arguments.append("--reduced")
        if allow_dirty:
            arguments.append("--allow-dirty")
        if not reduced and sweep_dir:
            arguments.extend(("--sweep-dir", str(self.root / "full-sweep")))
        stdout = io.StringIO()
        stderr = io.StringIO()
        with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
            code = GATE.main(arguments, runner=runner)
        report = json.loads(self.report.read_text(encoding="utf-8"))
        return code, report, stdout.getvalue(), stderr.getvalue()


class ReportAndRunnerTests(PhaseGateTestCase):
    def test_report_schema_exit_code_sim_digest_and_partial_corpus(self) -> None:
        raw = self.write_sim(valid_sim_document())
        runner = StubRunner()

        code, report, stdout, stderr = self.invoke(runner)

        self.assertEqual(code, 1)
        self.assertEqual(stderr, "")
        self.assertIn("phase0_exit: false", stdout)
        self.assertEqual(report["schema"], "bleavit.phase0-evidence.v1")
        self.assertEqual(report["git_commit"], HEAD)
        self.assertIs(report["tree_clean"], True)
        self.assertEqual(report["sim_evidence_sha256"], hashlib.sha256(raw).hexdigest())
        self.assertFalse(report["phase0_exit"])

    def test_every_test_leg_records_a_count_and_floor(self) -> None:
        _code, report, _stdout, _stderr = self.invoke(StubRunner())

        legs = report["criteria"]["reference-equivalence"]["legs"]
        expected = {
            "python-reference-model": (26, 26),
            "fixed-reference-vectors": (3, 3),
            "ledger-core-reference-vectors": (3, 3),
            "ledger-pallet-core-differential": (1, 1),
            "ledger-pallet-reference-sweep": (1, 1),
            "fixed-full-sweep": (1, 1),
        }
        for leg in legs:
            if leg["id"] in expected:
                self.assertEqual(
                    (leg["tests_executed"], leg["minimum_tests"]),
                    expected[leg["id"]],
                )
            else:
                self.assertNotIn("tests_executed", leg)
                self.assertNotIn("minimum_tests", leg)

    def test_all_test_legs_capture_output_through_injected_runner(self) -> None:
        runner = StubRunner()
        _code, report, _stdout, _stderr = self.invoke(runner)

        test_commands = {
            leg.command
            for leg in (*GATE.reference_legs(self.root / "full-sweep")[0],)
            if leg.test_output is not None
        }
        test_commands.add(GATE.reference_legs(self.root / "full-sweep")[2].command)
        calls = {
            call["command"]: call for call in runner.calls if call["command"] in test_commands
        }
        self.assertEqual(set(calls), test_commands)
        self.assertTrue(all(call["capture_output"] for call in calls.values()))
        self.assertEqual(
            set(report["criteria"]),
            {"reference-equivalence", "sim-false-pass", "calibration-published"},
        )
        self.assertEqual(
            report["criteria"]["reference-equivalence"]["status"], "pass-partial"
        )
        self.assertEqual(report["criteria"]["sim-false-pass"]["status"], "pass")
        self.assertEqual(report["criteria"]["calibration-published"]["status"], "pass")
        legs = report["criteria"]["reference-equivalence"]["legs"]
        self.assertEqual(len(legs), 9)
        for leg in legs:
            required = {"id", "command", "exit_code", "status"}
            self.assertTrue(required <= set(leg))
            self.assertIsInstance(leg["command"], str)
            self.assertTrue(leg["command"])
        self.assertEqual(legs[-2]["status"], "skipped")
        self.assertIsNone(legs[-2]["exit_code"])

    def test_missing_sim_evidence_is_pending_s4_and_exit_one(self) -> None:
        code, report, _stdout, stderr = self.invoke(StubRunner(), sim=False)

        self.assertEqual(code, 1)
        self.assertEqual(stderr, "")
        self.assertEqual(
            report["criteria"]["reference-equivalence"]["status"], "pass-partial"
        )
        self.assertEqual(report["criteria"]["sim-false-pass"]["status"], "pending-s4")
        self.assertEqual(
            report["criteria"]["calibration-published"]["status"], "pending-s4"
        )
        self.assertIn("pending S4", report["criteria"]["sim-false-pass"]["detail"])
        self.assertNotIn("sim_evidence_sha256", report)
        self.assertFalse(report["phase0_exit"])

    def test_explicit_missing_sim_evidence_is_failure_not_pending(self) -> None:
        code, report, _stdout, _stderr = self.invoke(StubRunner(), sim="missing")

        self.assertEqual(code, 1)
        self.assertEqual(report["criteria"]["sim-false-pass"]["status"], "fail")
        self.assertEqual(
            report["criteria"]["calibration-published"]["status"], "fail"
        )
        self.assertNotIn("pending S4", report["criteria"]["sim-false-pass"]["detail"])
        self.assertFalse(report["phase0_exit"])

    def test_reduced_reference_is_partial_and_skips_sweep_but_overall_exit_one(self) -> None:
        code, report, _stdout, _stderr = self.invoke(StubRunner(), reduced=True)

        self.assertEqual(code, 1)
        reference = report["criteria"]["reference-equivalence"]
        self.assertEqual(reference["status"], "pass-partial")
        self.assertEqual(
            [(leg["id"], leg["status"], leg["exit_code"]) for leg in reference["legs"][-2:]],
            [
                ("generate-full-sweep", "skipped", None),
                ("fixed-full-sweep", "skipped", None),
            ],
        )
        self.assertEqual(reference["legs"][-1]["tests_executed"], 0)
        self.assertEqual(reference["legs"][-1]["minimum_tests"], 1)
        self.assertEqual(report["criteria"]["sim-false-pass"]["status"], "pass")
        self.assertEqual(report["criteria"]["calibration-published"]["status"], "pass")
        self.assertFalse(report["phase0_exit"])

    def test_reference_leg_failure_propagates_to_criterion_and_exit(self) -> None:
        failing = (
            "python3",
            "tools/reference-model/check-doc-table.py",
        )
        runner = StubRunner({failing: 17})

        code, report, _stdout, _stderr = self.invoke(runner)

        self.assertEqual(code, 1)
        reference = report["criteria"]["reference-equivalence"]
        self.assertEqual(reference["status"], "fail")
        failed = [leg for leg in reference["legs"] if leg["status"] == "fail"]
        self.assertEqual(
            failed,
            [
                {
                    "id": "normative-doc-table",
                    "command": "python3 tools/reference-model/check-doc-table.py",
                    "exit_code": 17,
                    "status": "fail",
                }
            ],
        )
        self.assertFalse(report["phase0_exit"])

    def test_test_count_failures_propagate_to_reference_criterion(self) -> None:
        ordinary, _generate, _sweep = GATE.reference_legs(self.root / "full-sweep")
        by_id = {leg.identifier: leg for leg in ordinary}
        cases = (
            (
                "python-reference-model",
                GATE.CommandResult(0),
                None,
            ),
            (
                "fixed-reference-vectors",
                GATE.CommandResult(
                    0,
                    stdout="test result: ok. 2 passed; 0 failed; 0 ignored\n",
                ),
                2,
            ),
            (
                "ledger-pallet-core-differential",
                GATE.CommandResult(
                    0,
                    stdout="test result: ok. 0 passed; 0 failed; 0 ignored\n",
                ),
                0,
            ),
        )
        for identifier, result, expected_count in cases:
            with self.subTest(identifier=identifier):
                runner = StubRunner(results={by_id[identifier].command: result})
                code, report, _stdout, _stderr = self.invoke(runner)
                reference = report["criteria"]["reference-equivalence"]
                row = next(leg for leg in reference["legs"] if leg["id"] == identifier)
                self.assertEqual(code, 1)
                self.assertEqual(reference["status"], "fail")
                self.assertEqual(row["status"], "fail")
                self.assertEqual(row["tests_executed"], expected_count)
                self.assertFalse(report["phase0_exit"])

    def test_all_automatic_sweep_processes_use_injected_runner(self) -> None:
        runner = StubRunner()
        with mock.patch.object(
            GATE.subprocess,
            "run",
            side_effect=AssertionError("real subprocess execution is forbidden"),
        ):
            code, report, _stdout, _stderr = self.invoke(
                runner, reduced=False, sweep_dir=False
            )

        self.assertEqual(code, 1)
        self.assertFalse(report["phase0_exit"])
        commands = [call["command"] for call in runner.calls]
        self.assertEqual(commands[0], ("git", "rev-parse", "HEAD"))
        self.assertIn(
            ("python3", "tools/reference-model/generate-vectors.py", "--check"),
            commands,
        )
        generated = [
            command
            for command in commands
            if command[:2]
            == ("python3", "tools/reference-model/generate-vectors.py")
            and "--sweep-out" in command
        ]
        self.assertEqual(len(generated), 1)
        self.assertIn(
            (
                "cargo",
                "test",
                "-p",
                "futarchy-fixed",
                "--release",
                "--locked",
                "--test",
                "sweep",
                "--",
                "--ignored",
                "--nocapture",
            ),
            commands,
        )

    def test_git_failure_returns_fail_closed_error_exit(self) -> None:
        runner = StubRunner({("git", "rev-parse", "HEAD"): 2})
        arguments = ["--root", str(self.root), "--report-out", str(self.report), "--reduced"]
        stderr = io.StringIO()
        with contextlib.redirect_stderr(stderr):
            code = GATE.main(arguments, runner=runner)

        self.assertEqual(code, 2)
        self.assertIn("git rev-parse HEAD failed", stderr.getvalue())
        self.assertFalse(self.report.exists())

    def test_present_invalid_json_is_content_addressed_and_fails(self) -> None:
        raw = b"{not valid json\n"
        self.sim.write_bytes(raw)

        code, report, _stdout, _stderr = self.invoke(StubRunner())

        self.assertEqual(code, 1)
        self.assertEqual(report["sim_evidence_sha256"], hashlib.sha256(raw).hexdigest())
        self.assertEqual(report["criteria"]["sim-false-pass"]["status"], "fail")
        self.assertEqual(report["criteria"]["calibration-published"]["status"], "fail")
        self.assertIn("invalid sim evidence JSON", report["criteria"]["sim-false-pass"]["detail"])


class TestExecutionCountTests(PhaseGateTestCase):
    def test_cargo_count_sums_all_harness_result_lines(self) -> None:
        leg = next(
            leg
            for leg in GATE.reference_legs(self.root / "sweep")[0]
            if leg.identifier == "fixed-reference-vectors"
        )
        output = (
            "test result: ok. 1 passed; 0 failed; 0 ignored\n"
            "Doc-tests futarchy_fixed\n"
            "test result: ok. 2 passed; 0 failed; 0 ignored\n"
        )
        runner = StubRunner(results={leg.command: GATE.CommandResult(0, stdout=output)})

        row = GATE.execute_leg(self.root, leg, runner)

        self.assertEqual(row["status"], "pass")
        self.assertEqual(row["tests_executed"], 3)
        self.assertEqual(row["minimum_tests"], 3)
        self.assertTrue(runner.calls[-1]["capture_output"])

    def test_unittest_count_is_parsed_only_from_stderr(self) -> None:
        leg = next(
            leg
            for leg in GATE.reference_legs(self.root / "sweep")[0]
            if leg.identifier == "python-reference-model"
        )
        for output_channel, expected in (("stderr", "pass"), ("stdout", "fail")):
            with self.subTest(output_channel=output_channel):
                result = GATE.CommandResult(
                    0,
                    **{output_channel: "Ran 26 tests in 0.1s\nOK\n"},
                )
                row = GATE.execute_leg(
                    self.root, leg, StubRunner(results={leg.command: result})
                )
                self.assertEqual(row["status"], expected)
                self.assertEqual(
                    row["tests_executed"], 26 if output_channel == "stderr" else None
                )

    def test_zero_tests_fail_every_test_leg_even_with_exit_zero(self) -> None:
        ordinary, _generate, sweep = GATE.reference_legs(self.root / "sweep")
        for leg in (*ordinary, sweep):
            if leg.test_output is None:
                continue
            with self.subTest(leg=leg.identifier):
                result = GATE.CommandResult(
                    0,
                    stdout=(
                        "test result: ok. 0 passed; 0 failed; 0 ignored\n"
                        if leg.test_output == "cargo"
                        else ""
                    ),
                    stderr=(
                        "Ran 0 tests in 0.0s\nOK\n"
                        if leg.test_output == "unittest"
                        else ""
                    ),
                )
                row = GATE.execute_leg(
                    self.root, leg, StubRunner(results={leg.command: result})
                )
                self.assertEqual(row["status"], "fail")
                self.assertEqual(row["tests_executed"], 0)

    def test_unparseable_test_output_fails_every_test_leg(self) -> None:
        ordinary, _generate, sweep = GATE.reference_legs(self.root / "sweep")
        for leg in (*ordinary, sweep):
            if leg.test_output is None:
                continue
            with self.subTest(leg=leg.identifier):
                row = GATE.execute_leg(
                    self.root,
                    leg,
                    StubRunner(results={leg.command: GATE.CommandResult(0)}),
                )
                self.assertEqual(row["status"], "fail")
                self.assertIsNone(row["tests_executed"])

    def test_positive_count_below_declared_floor_fails(self) -> None:
        ordinary, _generate, _sweep = GATE.reference_legs(self.root / "sweep")
        legs = [leg for leg in ordinary if (leg.minimum_tests or 0) > 1]
        for leg in legs:
            with self.subTest(leg=leg.identifier):
                below = leg.minimum_tests - 1
                result = GATE.CommandResult(
                    0,
                    stdout=(
                        f"test result: ok. {below} passed; 0 failed; 0 ignored\n"
                        if leg.test_output == "cargo"
                        else ""
                    ),
                    stderr=(
                        f"Ran {below} tests in 0.1s\nOK\n"
                        if leg.test_output == "unittest"
                        else ""
                    ),
                )
                row = GATE.execute_leg(
                    self.root, leg, StubRunner(results={leg.command: result})
                )
                self.assertEqual(row["status"], "fail")
                self.assertEqual(row["tests_executed"], below)

    def test_non_test_leg_keeps_exit_code_semantics_without_count_fields(self) -> None:
        leg = GATE.reference_legs(self.root / "sweep")[0][0]
        runner = StubRunner()

        row = GATE.execute_leg(self.root, leg, runner)

        self.assertEqual(row["status"], "pass")
        self.assertNotIn("tests_executed", row)
        self.assertNotIn("minimum_tests", row)
        self.assertFalse(runner.calls[-1]["capture_output"])


class TreeCleanlinessTests(PhaseGateTestCase):
    def call_main(
        self, runner: StubRunner, *extra: str
    ) -> tuple[int, str, str]:
        stdout = io.StringIO()
        stderr = io.StringIO()
        arguments = [
            "--root",
            str(self.root),
            "--report-out",
            str(self.report),
            *extra,
        ]
        with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
            code = GATE.main(arguments, runner=runner)
        return code, stdout.getvalue(), stderr.getvalue()

    def test_dirty_tree_before_legs_refuses_with_exit_two(self) -> None:
        runner = StubRunner(status_outputs=[" M tracked.txt\0"])

        code, _stdout, stderr = self.call_main(runner, "--reduced")

        self.assertEqual(code, 2)
        self.assertIn("dirty tree", stderr)
        self.assertIn("before", stderr)
        self.assertIn("tracked.txt", stderr)
        self.assertFalse(self.report.exists())
        commands = [call["command"] for call in runner.calls]
        self.assertNotIn(
            ("python3", "tools/reference-model/generate-vectors.py", "--check"),
            commands,
        )

    def test_dirty_tree_after_legs_refuses_and_never_attests_clean(self) -> None:
        runner = StubRunner(status_outputs=["", "?? generated-after.txt\0"])

        code, _stdout, stderr = self.call_main(runner, "--reduced")

        self.assertEqual(code, 2)
        self.assertIn("dirty tree", stderr)
        self.assertIn("after", stderr)
        self.assertIn("generated-after.txt", stderr)
        self.assertFalse(self.report.exists())
        commands = [call["command"] for call in runner.calls]
        self.assertIn(
            ("python3", "tools/reference-model/generate-vectors.py", "--check"),
            commands,
        )

    def test_only_exact_output_inputs_and_sweep_descendants_are_exempt(self) -> None:
        allowed = (
            "?? phase0-report.json\0"
            "?? full-sweep/shard-00.json\0"
            "?? sim.json\0"
        )
        runner = StubRunner(status_outputs=[allowed, allowed])

        code, report, _stdout, stderr = self.invoke(runner)

        self.assertEqual(code, 1)
        self.assertEqual(stderr, "")
        self.assertIs(report["tree_clean"], True)

    def test_sweep_dir_cannot_exempt_the_repository_root(self) -> None:
        code, _stdout, stderr = self.call_main(
            StubRunner(), "--sweep-dir", str(self.root)
        )

        self.assertEqual(code, 2)
        self.assertIn("must not equal or contain the repository root", stderr)
        self.assertFalse(self.report.exists())

    def test_in_repo_sweep_dir_cannot_cover_tracked_source_files(self) -> None:
        sweep = self.root / "source-sweep"
        command = ("git", "ls-files", "-z", "--", "source-sweep")
        runner = StubRunner(
            results={
                command: GATE.CommandResult(
                    0, stdout="source-sweep/checked-code.rs\0"
                )
            }
        )

        code, _stdout, stderr = self.call_main(
            runner, "--sweep-dir", str(sweep)
        )

        self.assertEqual(code, 2)
        self.assertIn("untracked artifact directory", stderr)
        self.assertIn("source-sweep/checked-code.rs", stderr)
        self.assertFalse(self.report.exists())

    def test_similar_prefix_is_not_an_exempt_path(self) -> None:
        runner = StubRunner(status_outputs=["?? full-sweep-evil/shard.json\0"])

        code, _stdout, stderr = self.call_main(
            runner,
            "--sweep-dir",
            str(self.root / "full-sweep"),
            "--sim-evidence",
            str(self.sim),
        )

        self.assertEqual(code, 2)
        self.assertIn("full-sweep-evil/shard.json", stderr)

    def test_allow_dirty_is_rejected_without_reduced(self) -> None:
        code, _stdout, stderr = self.call_main(StubRunner(), "--allow-dirty")

        self.assertEqual(code, 2)
        self.assertIn("--allow-dirty", stderr)
        self.assertIn("--reduced", stderr)
        self.assertFalse(self.report.exists())

    def test_allow_dirty_reduced_run_is_marked_nonqualifying(self) -> None:
        runner = StubRunner(
            status_outputs=[" M PLAN.md\0", " M PLAN.md\0"]
        )

        code, report, _stdout, stderr = self.invoke(
            runner, reduced=True, allow_dirty=True
        )

        self.assertEqual(code, 1)
        self.assertEqual(stderr, "")
        self.assertIs(report["tree_clean"], False)
        self.assertFalse(report["phase0_exit"])
        self.assertNotEqual(
            report["criteria"]["reference-equivalence"]["status"], "pass"
        )
        status_calls = [
            call for call in runner.calls if call["command"][:2] == ("git", "status")
        ]
        self.assertEqual(len(status_calls), 2)

    def test_allow_dirty_cannot_exit_even_if_every_criterion_reports_pass(self) -> None:
        runner = StubRunner(
            status_outputs=[" M PLAN.md\0", " M PLAN.md\0"]
        )
        forced_reference = {
            "status": "pass",
            "legs": [],
            "corpus_families": {"attested": {}, "unattested": []},
        }

        with mock.patch.object(
            GATE,
            "check_reference_equivalence",
            return_value=forced_reference,
        ):
            code, report, _stdout, _stderr = self.invoke(
                runner, reduced=True, allow_dirty=True
            )

        self.assertEqual(
            {criterion["status"] for criterion in report["criteria"].values()},
            {"pass"},
        )
        self.assertEqual(code, 1)
        self.assertIs(report["tree_clean"], False)
        self.assertFalse(report["phase0_exit"])


class CorpusFamilyCoverageTests(PhaseGateTestCase):
    def coverage(self) -> dict[str, object]:
        ordinary, _generate, sweep = GATE.reference_legs(self.root / "sweep")
        known = frozenset(leg.identifier for leg in (*ordinary, sweep))
        return GATE.load_corpus_family_coverage(self.vectors, known)

    def test_actual_family_inventory_is_exhaustively_attested_or_unattested(self) -> None:
        coverage = self.coverage()

        expected_attested = {
            family: list(consumers)
            for family, consumers in CORPUS_FAMILY_CONSUMERS.items()
            if consumers
        }
        expected_unattested = sorted(
            family
            for family, consumers in CORPUS_FAMILY_CONSUMERS.items()
            if not consumers
        )
        self.assertEqual(coverage["attested"], expected_attested)
        self.assertEqual(coverage["unattested"], expected_unattested)
        self.assertIn("ledger_scenarios", coverage["unattested"])

    def test_report_lists_attested_bindings_and_unattested_debt(self) -> None:
        code, report, _stdout, _stderr = self.invoke(StubRunner())

        self.assertEqual(code, 1)
        reference = report["criteria"]["reference-equivalence"]
        self.assertEqual(reference["status"], "pass-partial")
        self.assertEqual(reference["corpus_families"], self.coverage())
        self.assertFalse(report["phase0_exit"])

    def test_unknown_family_in_corpus_is_a_loud_drift_error(self) -> None:
        document = corpus_fixture()
        document["new_unreviewed_scenarios"] = []
        self.vectors.write_text(json.dumps(document), encoding="utf-8")

        with self.assertRaisesRegex(
            GATE.PhaseGateError, "unknown corpus family.*new_unreviewed_scenarios"
        ):
            self.coverage()

    def test_mapped_family_missing_from_corpus_is_a_loud_drift_error(self) -> None:
        document = corpus_fixture()
        del document["lmsr_vectors"]
        self.vectors.write_text(json.dumps(document), encoding="utf-8")

        with self.assertRaisesRegex(
            GATE.PhaseGateError, "mapped family.*missing.*lmsr_vectors"
        ):
            self.coverage()

    def test_empty_or_unknown_consumer_binding_fails_closed(self) -> None:
        ordinary, _generate, sweep = GATE.reference_legs(self.root / "sweep")
        known = frozenset(leg.identifier for leg in (*ordinary, sweep))
        for binding, detail in (((), "empty consumer"), (("renamed-leg",), "unknown leg")):
            with self.subTest(binding=binding), mock.patch.dict(
                GATE.CORPUS_FAMILY_BINDINGS,
                {"lmsr_vectors": binding},
                clear=False,
            ):
                with self.assertRaisesRegex(GATE.PhaseGateError, detail):
                    GATE.load_corpus_family_coverage(self.vectors, known)


class SimulationEvidenceTests(PhaseGateTestCase):
    def assert_sim_fails(self, document: dict[str, object], detail: str) -> None:
        criterion = GATE.criterion_sim_false_pass(self.evidence(document))
        self.assertEqual(criterion["status"], "fail")
        self.assertIn(detail, criterion["detail"])

    def test_valid_0_0099_boundary_passes(self) -> None:
        criterion = GATE.criterion_sim_false_pass(self.evidence())

        self.assertEqual(criterion["status"], "pass")
        self.assertIn("strictly < 0.01", criterion["detail"])

    def test_wrong_schema_fails(self) -> None:
        document = valid_sim_document()
        document["schema"] = "bleavit.sim-calibration.v0"
        self.assert_sim_fails(document, "schema must be bleavit.sim-calibration.v1")

    def test_invalid_git_commit_fails(self) -> None:
        document = valid_sim_document()
        document["git_commit"] = "short"
        self.assert_sim_fails(document, "full 40-hex SHA")

    def test_sim_commit_must_match_checked_repository_head(self) -> None:
        document = valid_sim_document()
        document["git_commit"] = "cd" * 20

        criterion = GATE.criterion_sim_false_pass(self.evidence(document), HEAD)

        self.assertEqual(criterion["status"], "fail")
        self.assertIn("does not match checked repository HEAD", criterion["detail"])

    def test_missing_or_extra_false_pass_class_fails_exact_key_check(self) -> None:
        for mutate in ("missing", "extra"):
            with self.subTest(mutate=mutate):
                document = valid_sim_document()
                rates = copy.deepcopy(document["false_pass_rate"])
                assert isinstance(rates, dict)
                if mutate == "missing":
                    del rates["trs"]
                else:
                    rates["other"] = 0.0
                document["false_pass_rate"] = rates
                self.assert_sim_fails(document, "contain exactly param, trs, code, meta")

    def test_rate_equal_to_one_percent_fails(self) -> None:
        document = valid_sim_document()
        rates = copy.deepcopy(document["false_pass_rate"])
        assert isinstance(rates, dict)
        rates["code"] = 0.01
        document["false_pass_rate"] = rates
        self.assert_sim_fails(document, "strictly < 0.01")

    def test_rate_must_be_a_finite_number_in_unit_interval(self) -> None:
        for value in (-0.001, 1.001, -1, 2, "0.001", True, False, None):
            with self.subTest(value=value):
                document = valid_sim_document()
                rates = copy.deepcopy(document["false_pass_rate"])
                assert isinstance(rates, dict)
                rates["param"] = value
                document["false_pass_rate"] = rates
                self.assert_sim_fails(document, "finite number in [0,1]")

    def test_in_range_integer_rate_still_fails_the_strict_threshold(self) -> None:
        document = valid_sim_document()
        rates = copy.deepcopy(document["false_pass_rate"])
        assert isinstance(rates, dict)
        rates["param"] = 1
        document["false_pass_rate"] = rates
        self.assert_sim_fails(document, "strictly < 0.01")

    def test_zero_rate_encoded_as_json_integer_passes(self) -> None:
        document = valid_sim_document()
        rates = copy.deepcopy(document["false_pass_rate"])
        assert isinstance(rates, dict)
        rates["param"] = 0
        document["false_pass_rate"] = rates

        criterion = GATE.criterion_sim_false_pass(self.evidence(document))

        self.assertEqual(criterion["status"], "pass")

    def test_nonfinite_json_rate_is_rejected_before_validation(self) -> None:
        for value in (float("inf"), float("nan")):
            with self.subTest(value=value):
                document = valid_sim_document()
                rates = copy.deepcopy(document["false_pass_rate"])
                assert isinstance(rates, dict)
                rates["param"] = value
                document["false_pass_rate"] = rates

                criterion = GATE.criterion_sim_false_pass(self.evidence(document))

                self.assertEqual(criterion["status"], "fail")
                self.assertIn("non-finite JSON number", criterion["detail"])

    def test_corpus_9999_fails(self) -> None:
        document = valid_sim_document()
        document["synthetic_proposals"] = 9_999
        self.assert_sim_fails(document, "integer >= 10000")

    def test_attack_cost_must_be_validated(self) -> None:
        document = valid_sim_document()
        attack = copy.deepcopy(document["attack_cost_validation"])
        assert isinstance(attack, dict)
        attack["validated"] = False
        document["attack_cost_validation"] = attack
        self.assert_sim_fails(document, "validated must be true")

    def test_attack_cost_method_must_be_nonempty(self) -> None:
        document = valid_sim_document()
        attack = copy.deepcopy(document["attack_cost_validation"])
        assert isinstance(attack, dict)
        attack["method"] = "   "
        document["attack_cost_validation"] = attack
        self.assert_sim_fails(document, "method must be a non-empty string")

    def test_provided_but_unreadable_evidence_is_invalid_not_pending(self) -> None:
        self.write_sim(valid_sim_document())
        with mock.patch.object(Path, "read_bytes", side_effect=PermissionError("denied")):
            evidence = GATE.load_sim_evidence(self.sim)

        self.assertEqual(evidence.state, "invalid")
        criterion = GATE.criterion_sim_false_pass(evidence)
        self.assertEqual(criterion["status"], "fail")
        self.assertIn("cannot read sim evidence", criterion["detail"])


class CalibrationTests(PhaseGateTestCase):
    def test_fixture_registry_materializes_exact_keys_and_bounds(self) -> None:
        registry = self.registry()

        self.assertEqual(registry.keys, GATE.CALIBRATION_KEYS)
        self.assertEqual(tuple(map(str, registry.bounds["dec.delta.param"])), ("0.005", "0.10"))
        self.assertEqual(tuple(map(str, registry.bounds["pol.b_baseline"])), ("10000", "100000"))
        self.assertNotIn("future.renamed_cap", registry.keys)
        self.assertNotIn("future.another_cap", registry.keys)

    def test_removed_phase0_tag_fails_loudly(self) -> None:
        changed = doc13_fixture().replace(
            "sim-gated — [VERIFY via Phase-0/3 calibration]",
            "sim-gated — [VERIFY via later calibration]",
        )
        self.doc13.write_text(changed, encoding="utf-8")

        with self.assertRaisesRegex(
            GATE.PhaseGateError,
            "pol\\.b_baseline must retain its sim-gated Phase-0 tag",
        ):
            self.registry()

    def test_removed_phase0_tag_is_a_whole_gate_setup_error(self) -> None:
        changed = doc13_fixture().replace(
            "sim-gated — [VERIFY via Phase-0/3 calibration]",
            "sim-gated — [VERIFY via later calibration]",
        )
        self.doc13.write_text(changed, encoding="utf-8")
        runner = StubRunner()

        code, _stdout, stderr = self.invoke_setup_error(runner, "--reduced")

        self.assertEqual(code, 2)
        self.assertIn("must retain its sim-gated Phase-0 tag", stderr)
        self.assertFalse(self.report.exists())
        self.assertNotIn(
            ("python3", "tools/reference-model/generate-vectors.py", "--check"),
            [call["command"] for call in runner.calls],
        )

    def test_removed_delta_global_phase0_tag_fails_loudly(self) -> None:
        changed = doc13_fixture().replace(
            "Phase 0–3 calibration obligations are tagged *sim-gated*",
            "Later calibration obligations are tagged *sim-gated*",
        )
        self.doc13.write_text(changed, encoding="utf-8")

        with self.assertRaisesRegex(GATE.PhaseGateError, "dec\\.delta no longer has"):
            self.registry()

    def test_rule6_must_materialize_delta_with_exact_class_suffixes(self) -> None:
        changed = doc13_fixture().replace("(`dec.delta`, `dec.sigma`)", "(`dec.sigma`)")
        self.doc13.write_text(changed, encoding="utf-8")

        with self.assertRaisesRegex(
            GATE.PhaseGateError,
            "rule 6 must materialize dec\\.delta with exactly",
        ):
            self.registry()

    def test_unexpected_phase0_sim_gated_row_fails_loudly(self) -> None:
        unexpected = (
            "| `new.phase0_key` | Fixed | unit | "
            "*(sim-gated [VERIFY via Phase-0 calibration])* | — | — | — | 1 | META | fixture |\n"
        )
        changed = doc13_fixture().replace(
            "| `future.renamed_cap`", unexpected + "| `future.renamed_cap`"
        )
        self.doc13.write_text(changed, encoding="utf-8")

        with self.assertRaisesRegex(
            GATE.PhaseGateError, "unexpected Phase-0 sim-gated row new\\.phase0_key"
        ):
            self.registry()

    def test_unexpected_phase0_row_with_trailing_spaces_is_not_ignored(self) -> None:
        unexpected = (
            "| `new.phase0_key` | Fixed | unit | "
            "*(sim-gated [VERIFY via Phase-0 calibration])* | — | — | — | 1 | META | fixture |   \n"
        )
        changed = doc13_fixture().replace(
            "| `future.renamed_cap`", unexpected + "| `future.renamed_cap`"
        )
        self.doc13.write_text(changed, encoding="utf-8")

        with self.assertRaisesRegex(
            GATE.PhaseGateError, "unexpected Phase-0 sim-gated row new\\.phase0_key"
        ):
            self.registry()

    def test_expected_owning_row_rejects_an_extra_phase0_key(self) -> None:
        changed = doc13_fixture().replace(
            "`sec.prize.param` / `sec.prize.code` / `sec.prize.meta`",
            "`sec.prize.param` / `sec.prize.trs` / `sec.prize.code` / `sec.prize.meta`",
        )
        self.doc13.write_text(changed, encoding="utf-8")

        with self.assertRaisesRegex(
            GATE.PhaseGateError, "security-prize owning row has unexpected key set"
        ):
            self.registry()

    def test_sim_gated_other_row_without_phase3_or_arming_tag_fails(self) -> None:
        changed = doc13_fixture().replace(
            "sim-gated **[VERIFY before Phase-3 arming]**",
            "sim-gated **[VERIFY later]**",
        )
        self.doc13.write_text(changed, encoding="utf-8")

        with self.assertRaisesRegex(
            GATE.PhaseGateError, "does not lexically say Phase-0 or Phase-3/arming"
        ):
            self.registry()

    def test_as_above_phase3_tag_only_inherits_from_adjacent_row(self) -> None:
        unrelated = (
            "| `ordinary.intervening` | Fixed | unit | 1 | 0 | 2 | 1 | 1 | PARAM | fixture |\n"
        )
        changed = doc13_fixture().replace(
            "| `future.another_cap`", unrelated + "| `future.another_cap`"
        )
        self.doc13.write_text(changed, encoding="utf-8")

        with self.assertRaisesRegex(
            GATE.PhaseGateError,
            "sim-gated row future\\.another_cap does not lexically say Phase-0 or Phase-3/arming",
        ):
            self.registry()

    def test_phase_words_and_as_above_in_non_default_cells_never_exempt(self) -> None:
        base_cells = [
            "`new.unqualified`",
            "Fixed",
            "unit",
            "*(sim-gated [VERIFY later])*",
            "0",
            "10",
            "1",
            "1",
            "META",
            "fixture",
        ]
        for marker in ("Phase-3", "arming", "as above"):
            for index in range(len(base_cells)):
                if index == 3:
                    continue
                with self.subTest(marker=marker, column=index):
                    cells = list(base_cells)
                    cells[index] += f" {marker}"
                    inserted = "| " + " | ".join(cells) + " |\n"
                    changed = doc13_fixture().replace(
                        "| `future.renamed_cap`",
                        inserted + "| `future.renamed_cap`",
                    )
                    self.doc13.write_text(changed, encoding="utf-8")
                    with self.assertRaisesRegex(
                        GATE.PhaseGateError,
                        "sim-gated row new\\.unqualified does not lexically say",
                    ):
                        self.registry()

    def test_expected_phase0_tag_in_doc_cell_does_not_qualify_default(self) -> None:
        changed = doc13_fixture().replace(
            "**25,000** *(sim-gated — [VERIFY via Phase-0/3 calibration])* | 10,000",
            "**25,000** *(sim-gated — [VERIFY later])* | 10,000",
        ).replace(
            "TREASURY | fixture |",
            "TREASURY | fixture Phase-0 calibration |",
            1,
        )
        self.doc13.write_text(changed, encoding="utf-8")

        with self.assertRaisesRegex(
            GATE.PhaseGateError,
            "pol\\.b_baseline must retain its sim-gated Phase-0 tag",
        ):
            self.registry()

    def test_reordered_header_columns_fail_before_positional_bound_reads(self) -> None:
        changed = doc13_fixture().replace(
            "| Key | Type | Unit | Default | Hard min | Hard max |",
            "| Key | Type | Unit | Hard min | Default | Hard max |",
        )
        self.doc13.write_text(changed, encoding="utf-8")

        with self.assertRaisesRegex(GATE.PhaseGateError, "§1 table header changed"):
            self.registry()

    def test_reordered_header_is_a_whole_gate_setup_error(self) -> None:
        changed = doc13_fixture().replace(
            "| Key | Type | Unit | Default | Hard min | Hard max |",
            "| Key | Type | Unit | Hard min | Default | Hard max |",
        )
        self.doc13.write_text(changed, encoding="utf-8")
        runner = StubRunner()

        code, _stdout, stderr = self.invoke_setup_error(runner, "--reduced")

        self.assertEqual(code, 2)
        self.assertIn("§1 table header changed", stderr)
        self.assertFalse(self.report.exists())
        self.assertNotIn(
            ("python3", "tools/reference-model/generate-vectors.py", "--check"),
            [call["command"] for call in runner.calls],
        )

    def test_inserted_header_column_fails_before_positional_bound_reads(self) -> None:
        changed = doc13_fixture().replace(
            "| Key | Type | Unit | Default |",
            "| Key | Type | Unit | New column | Default |",
        ).replace(
            "|---|---|---|---|---|---|---|---|---|---|",
            "|---|---|---|---|---|---|---|---|---|---|---|",
            1,
        )
        self.doc13.write_text(changed, encoding="utf-8")

        with self.assertRaisesRegex(GATE.PhaseGateError, "§1 table header changed"):
            self.registry()

    def test_missing_calibration_key_fails(self) -> None:
        document = valid_sim_document()
        calibration = copy.deepcopy(document["calibration"])
        assert isinstance(calibration, dict)
        del calibration["sec.flow_cap"]
        document["calibration"] = calibration

        criterion = GATE.criterion_calibration(self.evidence(document), self.registry())

        self.assertEqual(criterion["status"], "fail")
        self.assertIn("calibration is missing Phase-0 key(s): sec.flow_cap", criterion["detail"])

    def test_delta_and_pol_bounds_are_inclusive(self) -> None:
        document = valid_sim_document()
        calibration = copy.deepcopy(document["calibration"])
        assert isinstance(calibration, dict)
        calibration.update(
            {
                "dec.delta.param": 0.005,
                "dec.delta.trs": 0.10,
                "dec.delta.code": 0.005,
                "dec.delta.meta": 0.10,
                "pol.b_baseline": 100_000,
            }
        )
        document["calibration"] = calibration

        criterion = GATE.criterion_calibration(self.evidence(document), self.registry())

        self.assertEqual(criterion["status"], "pass")

    def test_delta_and_pol_out_of_bounds_fail(self) -> None:
        cases = (
            ("dec.delta.param", 0.0049),
            ("dec.delta.code", 0.1001),
            ("pol.b_baseline", 9_999),
            ("pol.b_baseline", 100_001),
        )
        for key, value in cases:
            with self.subTest(key=key, value=value):
                document = valid_sim_document()
                calibration = copy.deepcopy(document["calibration"])
                assert isinstance(calibration, dict)
                calibration[key] = value
                document["calibration"] = calibration

                criterion = GATE.criterion_calibration(
                    self.evidence(document), self.registry()
                )

                self.assertEqual(criterion["status"], "fail")
                self.assertIn(f"calibration.{key}=", criterion["detail"])
                self.assertIn("outside doc-13 bounds", criterion["detail"])

    def test_non_numeric_calibration_fails_closed(self) -> None:
        document = valid_sim_document()
        calibration = copy.deepcopy(document["calibration"])
        assert isinstance(calibration, dict)
        calibration["sec.flow_cap"] = True
        document["calibration"] = calibration

        criterion = GATE.criterion_calibration(self.evidence(document), self.registry())

        self.assertEqual(criterion["status"], "fail")
        self.assertIn("must be a finite JSON number", criterion["detail"])

    def test_every_calibration_key_rejects_the_unsigned_domain_boundary(self) -> None:
        for key in sorted(GATE.CALIBRATION_KEYS):
            with self.subTest(key=key):
                document = valid_sim_document()
                calibration = copy.deepcopy(document["calibration"])
                assert isinstance(calibration, dict)
                calibration[key] = -0.0001
                document["calibration"] = calibration

                criterion = GATE.criterion_calibration(
                    self.evidence(document), self.registry()
                )

                self.assertEqual(criterion["status"], "fail")
                self.assertIn(f"calibration.{key}", criterion["detail"])

    def test_negative_prize_or_flow_cap_can_never_make_whole_gate_exit(self) -> None:
        for key in ("sec.prize.param", "sec.prize.code", "sec.prize.meta", "sec.flow_cap"):
            with self.subTest(key=key):
                document = valid_sim_document()
                calibration = copy.deepcopy(document["calibration"])
                assert isinstance(calibration, dict)
                calibration[key] = -1
                document["calibration"] = calibration
                self.write_sim(document)

                code, report, _stdout, _stderr = self.invoke(StubRunner())

                self.assertEqual(code, 1)
                self.assertEqual(
                    report["criteria"]["calibration-published"]["status"], "fail"
                )
                self.assertFalse(report["phase0_exit"])


class RealDoc13IntegrationTests(unittest.TestCase):
    def test_actual_doc13_calibration_contract_has_not_drifted(self) -> None:
        registry = GATE.load_calibration_registry(
            ROOT / "docs" / "architecture" / "13-parameters.md"
        )

        self.assertEqual(registry.keys, GATE.CALIBRATION_KEYS)
        self.assertEqual(tuple(map(str, registry.bounds["dec.delta.param"])), ("0.005", "0.10"))
        self.assertEqual(tuple(map(str, registry.bounds["pol.b_baseline"])), ("10000", "100000"))

    def test_actual_vector_corpus_family_inventory_has_not_drifted(self) -> None:
        ordinary, _generate, sweep = GATE.reference_legs(Path("/integration-sweep"))
        coverage = GATE.load_corpus_family_coverage(
            ROOT / "reference-model" / "fixtures" / "vectors.json",
            frozenset(leg.identifier for leg in (*ordinary, sweep)),
        )

        self.assertEqual(
            coverage["attested"],
            {
                family: list(consumers)
                for family, consumers in CORPUS_FAMILY_CONSUMERS.items()
                if consumers
            },
        )
        self.assertEqual(
            coverage["unattested"],
            sorted(
                family
                for family, consumers in CORPUS_FAMILY_CONSUMERS.items()
                if not consumers
            ),
        )


if __name__ == "__main__":
    unittest.main()

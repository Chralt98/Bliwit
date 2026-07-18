#!/usr/bin/env python3
"""Machine-check the 09 §7.1 Phase-0 exit evidence."""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import re
import shlex
import subprocess
import sys
import tempfile
from dataclasses import dataclass
from decimal import Decimal, InvalidOperation
from pathlib import Path
from typing import Any, Mapping, Protocol, Sequence


REPORT_SCHEMA = "bleavit.phase0-evidence.v1"
SIM_SCHEMA = "bleavit.sim-calibration.v1"
SCRIPT_ROOT = Path(__file__).resolve().parents[2]
FALSE_PASS_CLASSES = frozenset(("param", "trs", "code", "meta"))
DELTA_KEYS = tuple(f"dec.delta.{name}" for name in ("param", "trs", "code", "meta"))
CALIBRATION_KEYS = frozenset(
    (
        *DELTA_KEYS,
        "pol.b_baseline",
        "sec.prize.param",
        "sec.prize.code",
        "sec.prize.meta",
        "sec.flow_cap",
    )
)
FULL_SHA_RE = re.compile(r"[0-9a-fA-F]{40}")
BACKTICK_RE = re.compile(r"`([^`]+)`")
NUMBER_RE = re.compile(r"(?<![A-Za-z0-9_.])[-+]?(?:\d[\d,]*)(?:\.\d+)?")
CARGO_TEST_COUNT_RE = re.compile(r"test result: ok\.\s+(\d+) passed")
UNITTEST_COUNT_RE = re.compile(r"Ran (\d+) tests?")
DOC13_SECTION1_HEADER = (
    "Key",
    "Type",
    "Unit",
    "Default",
    "Hard min",
    "Hard max",
    "Max Δ/decision",
    "Cooldown",
    "Class",
    "Doc",
)
CORPUS_METADATA_KEYS = frozenset(("schema", "precision"))
CORPUS_FAMILY_BINDINGS: Mapping[str, tuple[str, ...] | None] = {
    "decision_scenarios": None,
    "high_precision_corpus": ("fixed-reference-vectors",),
    "ledger_error_scenarios": ("ledger-core-reference-vectors",),
    "ledger_scenarios": None,
    "ledger_score_scenarios": ("ledger-core-reference-vectors",),
    "ledger_sequence_scenarios": ("ledger-core-reference-vectors",),
    "ledger_sweep_scenarios": ("ledger-pallet-reference-sweep",),
    "lmsr_maker_example": None,
    "lmsr_vectors": ("fixed-reference-vectors",),
    "transcendental_corpus": ("fixed-reference-vectors",),
    "treasury_scenarios": None,
    "twap_scenarios": None,
    "welfare_scenarios": None,
}


class PhaseGateError(RuntimeError):
    """A fail-closed Phase-0 gate error with an operator-facing message."""


@dataclass(frozen=True)
class CommandResult:
    exit_code: int
    stdout: str = ""
    stderr: str = ""


class Runner(Protocol):
    """Injectable subprocess boundary used by every command execution."""

    def run(
        self,
        command: Sequence[str],
        *,
        cwd: Path,
        env: Mapping[str, str] | None = None,
        capture_output: bool = False,
    ) -> CommandResult: ...


class SubprocessRunner:
    """Production command runner."""

    def run(
        self,
        command: Sequence[str],
        *,
        cwd: Path,
        env: Mapping[str, str] | None = None,
        capture_output: bool = False,
    ) -> CommandResult:
        process_env = os.environ.copy()
        if env is not None:
            process_env.update(env)
        completed = subprocess.run(
            list(command),
            cwd=cwd,
            env=process_env,
            text=True,
            capture_output=capture_output,
            check=False,
        )
        return CommandResult(
            completed.returncode,
            completed.stdout if capture_output else "",
            completed.stderr if capture_output else "",
        )


@dataclass(frozen=True)
class GateConfig:
    root: Path
    report_out: Path
    sim_evidence: Path | None
    sweep_dir: Path | None
    reduced: bool
    allow_dirty: bool = False


@dataclass(frozen=True)
class CommandLeg:
    identifier: str
    command: tuple[str, ...]
    env: tuple[tuple[str, str], ...] = ()
    test_output: str | None = None
    minimum_tests: int | None = None

    def display(self) -> str:
        assignments = tuple(f"{key}={value}" for key, value in self.env)
        return shlex.join((*assignments, *self.command))


@dataclass(frozen=True)
class CalibrationRegistry:
    keys: frozenset[str]
    bounds: Mapping[str, tuple[Decimal, Decimal]]


@dataclass(frozen=True)
class SimEvidence:
    state: str
    document: Mapping[str, Any] | None
    sha256: str | None
    detail: str


def parse_args(argv: Sequence[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Check and publish the 09 §7.1 Phase-0 exit evidence."
    )
    parser.add_argument(
        "--root",
        type=Path,
        default=SCRIPT_ROOT,
        help="repository root (defaults to the checker's repository)",
    )
    parser.add_argument(
        "--sweep-dir",
        type=Path,
        help="pre-generated full reference sweep (generated in a temp dir if omitted)",
    )
    parser.add_argument(
        "--reduced",
        action="store_true",
        help="skip the full sweep; cannot satisfy the Phase-0 exit gate",
    )
    parser.add_argument(
        "--allow-dirty",
        action="store_true",
        help="allow a dirty tree only with --reduced; can never qualify Phase 0",
    )
    parser.add_argument(
        "--sim-evidence",
        type=Path,
        help=f"S4-published {SIM_SCHEMA} artifact",
    )
    parser.add_argument("--report-out", type=Path, required=True)
    return parser.parse_args(argv)


def rooted(root: Path, path: Path | None) -> Path | None:
    if path is None:
        return None
    return path if path.is_absolute() else root / path


def sha256_bytes(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


def reject_json_constant(value: str) -> Any:
    raise ValueError(f"non-finite JSON number {value} is forbidden")


def unique_json_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"duplicate JSON key {key!r}")
        result[key] = value
    return result


def write_json(path: Path, document: Mapping[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(path.name + ".tmp")
    try:
        temporary.write_text(
            json.dumps(document, indent=2, sort_keys=True) + "\n", encoding="utf-8"
        )
        temporary.replace(path)
    except Exception:
        try:
            temporary.unlink()
        except FileNotFoundError:
            pass
        raise


def git_head(root: Path, runner: Runner) -> str:
    try:
        result = runner.run(
            ("git", "rev-parse", "HEAD"),
            cwd=root,
            capture_output=True,
        )
    except Exception as error:
        raise PhaseGateError(f"cannot read repository HEAD: {error}") from error
    value = result.stdout.strip()
    if result.exit_code != 0 or FULL_SHA_RE.fullmatch(value) is None:
        detail = (result.stderr or result.stdout).strip() or "nonzero or invalid output"
        raise PhaseGateError(f"git rev-parse HEAD failed: {detail}")
    return value.lower()


def lexical_absolute(path: Path) -> Path:
    """Return an absolute normalized path without following symlinks."""

    return Path(os.path.abspath(path))


def is_equal_or_below(path: Path, allowed: Path) -> bool:
    return path == allowed or allowed in path.parents


def parse_porcelain_z(output: str) -> list[str]:
    fields = output.split("\0")
    if fields and fields[-1] == "":
        fields.pop()
    paths: list[str] = []
    index = 0
    while index < len(fields):
        entry = fields[index]
        if len(entry) < 4 or entry[2] != " ":
            raise PhaseGateError("git status --porcelain returned malformed output")
        status = entry[:2]
        path = entry[3:]
        if not path:
            raise PhaseGateError("git status --porcelain returned an empty path")
        paths.append(path)
        index += 1
        if status[0] in ("R", "C"):
            if index >= len(fields) or not fields[index]:
                raise PhaseGateError(
                    "git status --porcelain returned a malformed rename/copy entry"
                )
            paths.append(fields[index])
            index += 1
    return paths


def dirty_tree_paths(
    root: Path, runner: Runner, allowed_paths: Sequence[Path]
) -> list[str]:
    command = (
        "git",
        "status",
        "--porcelain=v1",
        "--untracked-files=all",
        "-z",
    )
    try:
        result = runner.run(command, cwd=root, capture_output=True)
    except Exception as error:
        raise PhaseGateError(f"cannot inspect repository cleanliness: {error}") from error
    if result.exit_code != 0:
        detail = (result.stderr or result.stdout).strip() or "nonzero exit"
        raise PhaseGateError(f"git status --porcelain failed: {detail}")

    allowed = tuple(lexical_absolute(path) for path in allowed_paths)
    dirty: list[str] = []
    for relative in parse_porcelain_z(result.stdout):
        candidate_path = Path(relative)
        if candidate_path.is_absolute():
            raise PhaseGateError(
                f"git status --porcelain returned an absolute path: {relative}"
            )
        candidate = lexical_absolute(root / candidate_path)
        if not any(is_equal_or_below(candidate, exemption) for exemption in allowed):
            dirty.append(relative)
    return sorted(set(dirty))


def validate_sweep_exemption(root: Path, sweep_path: Path, runner: Runner) -> None:
    """Keep a supplied sweep exemption from covering tested repository bytes."""

    root = lexical_absolute(root)
    sweep_path = lexical_absolute(sweep_path)
    if sweep_path == root or sweep_path in root.parents:
        raise PhaseGateError(
            "--sweep-dir must not equal or contain the repository root"
        )
    if root not in sweep_path.parents:
        return

    relative = sweep_path.relative_to(root).as_posix()
    command = ("git", "ls-files", "-z", "--", relative)
    try:
        result = runner.run(command, cwd=root, capture_output=True)
    except Exception as error:
        raise PhaseGateError(
            f"cannot validate the --sweep-dir exemption: {error}"
        ) from error
    if result.exit_code != 0:
        detail = (result.stderr or result.stdout).strip() or "nonzero exit"
        raise PhaseGateError(f"git ls-files for --sweep-dir failed: {detail}")
    tracked = sorted(path for path in result.stdout.split("\0") if path)
    if tracked:
        preview = ", ".join(tracked[:5])
        if len(tracked) > 5:
            preview += f", ... ({len(tracked)} paths total)"
        raise PhaseGateError(
            "--sweep-dir must be an untracked artifact directory; it contains "
            f"tracked repository path(s): {preview}"
        )


def require_clean_tree(stage: str, dirty: Sequence[str]) -> None:
    if dirty:
        raise PhaseGateError(
            f"refusing Phase-0 evidence from a dirty tree ({stage}): "
            + ", ".join(dirty)
        )


def skipped_leg(leg: CommandLeg) -> dict[str, Any]:
    row: dict[str, Any] = {
        "id": leg.identifier,
        "command": leg.display(),
        "exit_code": None,
        "status": "skipped",
    }
    if leg.test_output is not None:
        row["tests_executed"] = 0
        row["minimum_tests"] = leg.minimum_tests
    return row


def parsed_test_count(leg: CommandLeg, result: CommandResult) -> int | None:
    if leg.test_output == "cargo":
        matches = CARGO_TEST_COUNT_RE.findall(result.stdout + "\n" + result.stderr)
        return sum(int(value) for value in matches) if matches else None
    if leg.test_output == "unittest":
        matches = UNITTEST_COUNT_RE.findall(result.stderr)
        return sum(int(value) for value in matches) if matches else None
    if leg.test_output is None:
        return None
    raise PhaseGateError(
        f"unknown test-output parser {leg.test_output!r} for leg {leg.identifier}"
    )


def execute_leg(root: Path, leg: CommandLeg, runner: Runner) -> dict[str, Any]:
    capture_output = leg.test_output is not None
    tests_executed: int | None = None
    try:
        result = runner.run(
            leg.command,
            cwd=root,
            env=dict(leg.env) if leg.env else None,
            capture_output=capture_output,
        )
        exit_code: int | None = result.exit_code
        if capture_output:
            tests_executed = parsed_test_count(leg, result)
    except Exception:
        exit_code = None
    passed = exit_code == 0
    if capture_output:
        passed = (
            passed
            and tests_executed is not None
            and leg.minimum_tests is not None
            and tests_executed >= leg.minimum_tests
        )
    row: dict[str, Any] = {
        "id": leg.identifier,
        "command": leg.display(),
        "exit_code": exit_code,
        "status": "pass" if passed else "fail",
    }
    if capture_output:
        row["tests_executed"] = tests_executed
        row["minimum_tests"] = leg.minimum_tests
    return row


def reference_legs(sweep_dir: Path) -> tuple[list[CommandLeg], CommandLeg, CommandLeg]:
    ordinary = [
        CommandLeg(
            "vector-freshness",
            ("python3", "tools/reference-model/generate-vectors.py", "--check"),
        ),
        CommandLeg(
            "normative-doc-table",
            ("python3", "tools/reference-model/check-doc-table.py"),
        ),
        CommandLeg(
            "python-reference-model",
            (
                "python3",
                "-m",
                "unittest",
                "discover",
                "-s",
                "reference-model/tests",
            ),
            (("PYTHONPATH", "reference-model/src"),),
            test_output="unittest",
            minimum_tests=26,
        ),
        CommandLeg(
            "fixed-reference-vectors",
            (
                "cargo",
                "test",
                "-p",
                "futarchy-fixed",
                "--release",
                "--locked",
                "--lib",
                "reference_model_vectors",
            ),
            test_output="cargo",
            minimum_tests=3,
        ),
        CommandLeg(
            "ledger-core-reference-vectors",
            (
                "cargo",
                "test",
                "-p",
                "conditional-ledger-core",
                "--release",
                "--locked",
                "--test",
                "differential_vectors",
            ),
            test_output="cargo",
            minimum_tests=3,
        ),
        CommandLeg(
            "ledger-pallet-core-differential",
            (
                "cargo",
                "test",
                "-p",
                "pallet-conditional-ledger",
                "--release",
                "--locked",
                "differential_matches_frame_free_core",
            ),
            test_output="cargo",
            minimum_tests=1,
        ),
        CommandLeg(
            "ledger-pallet-reference-sweep",
            (
                "cargo",
                "test",
                "-p",
                "pallet-conditional-ledger",
                "--release",
                "--locked",
                "generated_sweep_vectors_match_real_pallet_housekeeping",
            ),
            test_output="cargo",
            minimum_tests=1,
        ),
    ]
    generate = CommandLeg(
        "generate-full-sweep",
        (
            "python3",
            "tools/reference-model/generate-vectors.py",
            "--sweep-out",
            str(sweep_dir),
        ),
    )
    sweep = CommandLeg(
        "fixed-full-sweep",
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
        (
            ("BLEAVIT_SWEEP_DIR", str(sweep_dir)),
            ("BLEAVIT_SWEEP_REQUIRE_FULL", "1"),
        ),
        test_output="cargo",
        minimum_tests=1,
    )
    return ordinary, generate, sweep


def load_corpus_family_coverage(
    path: Path, known_leg_ids: frozenset[str]
) -> dict[str, Any]:
    try:
        raw = path.read_bytes()
        document = json.loads(
            raw.decode("utf-8"),
            object_pairs_hook=unique_json_object,
            parse_constant=reject_json_constant,
        )
    except (OSError, UnicodeDecodeError, json.JSONDecodeError, ValueError) as error:
        raise PhaseGateError(f"cannot read reference-model vector corpus: {error}") from error
    if not isinstance(document, dict):
        raise PhaseGateError("reference-model vector corpus top level must be an object")

    actual_families = set(document) - CORPUS_METADATA_KEYS
    mapped_families = set(CORPUS_FAMILY_BINDINGS)
    unknown = sorted(actual_families - mapped_families)
    missing = sorted(mapped_families - actual_families)
    if unknown or missing:
        detail: list[str] = []
        if unknown:
            detail.append("unknown corpus family/families: " + ", ".join(unknown))
        if missing:
            detail.append("mapped family/families missing: " + ", ".join(missing))
        raise PhaseGateError(
            "reference-model corpus family mapping drift: "
            + "; ".join(detail)
            + "; update the Phase-0 gate"
        )
    unknown_metadata = set(document) - actual_families - CORPUS_METADATA_KEYS
    if unknown_metadata:
        raise PhaseGateError(
            "reference-model corpus metadata drift: "
            + ", ".join(sorted(unknown_metadata))
        )
    for family in sorted(actual_families):
        if not isinstance(document[family], (list, dict)):
            raise PhaseGateError(
                f"reference-model corpus family {family} must be an array or object"
            )

    attested: dict[str, list[str]] = {}
    unattested: list[str] = []
    used_consumer_legs: set[str] = set()
    for family in sorted(mapped_families):
        binding = CORPUS_FAMILY_BINDINGS[family]
        if binding is None:
            unattested.append(family)
            continue
        if not binding:
            raise PhaseGateError(f"corpus family {family} has an empty consumer binding")
        invalid_legs = sorted(set(binding) - known_leg_ids)
        if invalid_legs:
            raise PhaseGateError(
                f"corpus family {family} names unknown leg(s): "
                + ", ".join(invalid_legs)
            )
        attested[family] = list(binding)
        used_consumer_legs.update(binding)

    expected_consumer_legs = {
        "fixed-reference-vectors",
        "ledger-core-reference-vectors",
        "ledger-pallet-reference-sweep",
    }
    if used_consumer_legs != expected_consumer_legs:
        raise PhaseGateError(
            "reference-model corpus consumer mapping is not exhaustive over the "
            f"attestation legs: mapped={sorted(used_consumer_legs)}, "
            f"expected={sorted(expected_consumer_legs)}"
        )
    return {"attested": attested, "unattested": unattested}


def check_reference_equivalence(
    root: Path,
    runner: Runner,
    *,
    sweep_dir: Path,
    generate_sweep: bool,
    reduced: bool,
    corpus_families: Mapping[str, Any],
) -> dict[str, Any]:
    ordinary, generate, sweep = reference_legs(sweep_dir)
    rows = [execute_leg(root, leg, runner) for leg in ordinary]
    if reduced:
        rows.extend((skipped_leg(generate), skipped_leg(sweep)))
    else:
        generated = skipped_leg(generate)
        if generate_sweep:
            generated = execute_leg(root, generate, runner)
        rows.append(generated)
        if generated["status"] == "fail":
            rows.append(skipped_leg(sweep))
        else:
            rows.append(execute_leg(root, sweep, runner))

    if any(row["status"] == "fail" for row in rows):
        status = "fail"
    elif corpus_families["unattested"]:
        status = "pass-partial"
    elif reduced:
        status = "pass-reduced"
    else:
        status = "pass"
    return {"status": status, "legs": rows, "corpus_families": corpus_families}


def split_markdown_row(line: str) -> list[str]:
    """Split a pipe table row without treating escaped pipes as separators."""

    cells: list[str] = []
    current: list[str] = []
    escaped = False
    for character in line.strip()[1:-1]:
        if escaped:
            current.append(character)
            escaped = False
        elif character == "\\":
            current.append(character)
            escaped = True
        elif character == "|":
            cells.append("".join(current).strip())
            current = []
        else:
            current.append(character)
    cells.append("".join(current).strip())
    return cells


def first_decimal(cell: str, label: str) -> Decimal:
    match = NUMBER_RE.search(cell)
    if match is None:
        raise PhaseGateError(f"13 parameter drift: {label} has no numeric bound")
    try:
        return Decimal(match.group(0).replace(",", ""))
    except InvalidOperation as error:
        raise PhaseGateError(f"13 parameter drift: {label} has an invalid bound") from error


def primary_keys(cells: Sequence[str]) -> tuple[str, ...]:
    if not cells:
        return ()
    return tuple(
        token
        for token in BACKTICK_RE.findall(cells[0])
        if not token.startswith("key:") and "." in token
    )


def load_calibration_registry(path: Path) -> CalibrationRegistry:
    """Lexically bind G0's calibration list and bounds to doc 13."""

    try:
        text = path.read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError) as error:
        raise PhaseGateError(f"cannot read docs/architecture/13-parameters.md: {error}") from error

    lower_document = text.lower()
    global_delta_tag = (
        "every default is a **simulation hypothesis**" in lower_document
        and "phase 0–3 calibration obligations are tagged *sim-gated*" in lower_document
    )
    rule6_line = next(
        (line for line in text.splitlines() if line.startswith("6. **`ParamKey` encoding")),
        None,
    )
    rule6_match = (
        re.search(
            r"\*\*Per-class rows\*\* \((.*?)\) materialize as four keys "
            r"with the class suffixes (.*)\.$",
            rule6_line,
        )
        if rule6_line is not None
        else None
    )
    if rule6_match is None:
        raise PhaseGateError(
            "13 parameter drift: cannot find rule 6 per-class materialization; "
            "update the Phase-0 gate"
        )
    per_class_bases = set(BACKTICK_RE.findall(rule6_match.group(1)))
    class_suffixes = tuple(BACKTICK_RE.findall(rule6_match.group(2)))
    if "dec.delta" not in per_class_bases or class_suffixes != (
        ".param",
        ".trs",
        ".code",
        ".meta",
    ):
        raise PhaseGateError(
            "13 parameter drift: rule 6 must materialize dec.delta with exactly "
            ".param/.trs/.code/.meta; update the Phase-0 gate"
        )
    lines = text.splitlines()
    section1_start = next(
        (index for index, line in enumerate(lines) if line.startswith("## 1.")), None
    )
    section2_start = next(
        (index for index, line in enumerate(lines) if line.startswith("## 2.")), None
    )
    if (
        section1_start is None
        or section2_start is None
        or section2_start <= section1_start
    ):
        raise PhaseGateError(
            "13 parameter drift: cannot locate the ordered §1/§2 boundaries"
        )

    parsed_section1: list[tuple[int, list[str], str]] = []
    for zero_index in range(section1_start + 1, section2_start):
        line_number = zero_index + 1
        line = lines[zero_index]
        stripped = line.strip()
        if stripped.startswith("|") and not stripped.endswith("|"):
            if "sim-gated" in stripped.lower():
                raise PhaseGateError(
                    f"13 parameter drift at line {line_number}: malformed sim-gated "
                    "table row; update the Phase-0 gate"
                )
            continue
        if stripped.startswith("|") and stripped.endswith("|"):
            cells = split_markdown_row(stripped)
            parsed_section1.append((line_number, cells, stripped))

    header_rows = [row for row in parsed_section1 if row[1] and row[1][0] == "Key"]
    if len(header_rows) != 1:
        raise PhaseGateError(
            "13 parameter drift: §1 must contain exactly one Key table header"
        )
    header_line, header_cells, _header_raw = header_rows[0]
    if tuple(header_cells) != DOC13_SECTION1_HEADER:
        raise PhaseGateError(
            f"13 parameter drift at line {header_line}: §1 table header changed; "
            f"expected {list(DOC13_SECTION1_HEADER)}, got {header_cells}"
        )
    default_index = header_cells.index("Default")
    hard_min_index = header_cells.index("Hard min")
    hard_max_index = header_cells.index("Hard max")
    rows = [
        row
        for row in parsed_section1
        if row[1]
        and row[1][0] != "Key"
        and not row[1][0].startswith("---")
    ]

    delta_row: tuple[int, list[str], str] | None = None
    pol_row: tuple[int, list[str], str] | None = None
    prize_row: tuple[int, list[str], str] | None = None
    flow_row: tuple[int, list[str], str] | None = None
    for row in rows:
        keys = set(primary_keys(row[1]))
        if "dec.delta" in keys:
            if keys != {"dec.delta"}:
                raise PhaseGateError(
                    f"13 parameter drift at line {row[0]}: dec.delta owning row "
                    f"has unexpected key set {sorted(keys)}; update the Phase-0 gate"
                )
            if delta_row is not None:
                raise PhaseGateError("13 parameter drift: duplicate dec.delta owning row")
            delta_row = row
        if "pol.b_baseline" in keys:
            if keys != {"pol.b_baseline"}:
                raise PhaseGateError(
                    f"13 parameter drift at line {row[0]}: pol.b_baseline owning row "
                    f"has unexpected key set {sorted(keys)}; update the Phase-0 gate"
                )
            if pol_row is not None:
                raise PhaseGateError(
                    "13 parameter drift: duplicate pol.b_baseline owning row"
                )
            pol_row = row
        prize_keys = {"sec.prize.param", "sec.prize.code", "sec.prize.meta"}
        if any(key.startswith("sec.prize.") for key in keys):
            if keys != prize_keys:
                raise PhaseGateError(
                    f"13 parameter drift at line {row[0]}: security-prize owning row "
                    f"has unexpected key set {sorted(keys)}; update the Phase-0 gate"
                )
            if prize_row is not None:
                raise PhaseGateError(
                    "13 parameter drift: duplicate security-prize owning row"
                )
            prize_row = row
        if "sec.flow_cap" in keys:
            if keys != {"sec.flow_cap"}:
                raise PhaseGateError(
                    f"13 parameter drift at line {row[0]}: sec.flow_cap owning row "
                    f"has unexpected key set {sorted(keys)}; update the Phase-0 gate"
                )
            if flow_row is not None:
                raise PhaseGateError("13 parameter drift: duplicate sec.flow_cap owning row")
            flow_row = row

    missing_rows = [
        name
        for name, row in (
            ("dec.delta", delta_row),
            ("pol.b_baseline", pol_row),
            ("sec.prize.{param,code,meta}", prize_row),
            ("sec.flow_cap", flow_row),
        )
        if row is None
    ]
    if missing_rows:
        raise PhaseGateError(
            "13 parameter drift: Phase-0 calibration row(s) missing: "
            + ", ".join(missing_rows)
            + "; update tools/phase-gates/check-phase0-exit.py"
        )
    assert delta_row is not None and pol_row is not None
    assert prize_row is not None and flow_row is not None

    # 09 §7.1 explicitly makes delta Phase-0-calibrated, while the current
    # doc-13 row relies on reading rule 4 instead of repeating a row-local tag.
    # Keep that exceptional wording narrow: every other expected row must carry
    # its own literal Phase-0 + sim-gated markers.
    if not global_delta_tag:
        raise PhaseGateError(
            "13 parameter drift: dec.delta no longer has doc 13 reading-rule-4 "
            "simulation/Phase 0–3 coverage; update the Phase-0 gate"
        )
    for name, row in (
        ("pol.b_baseline", pol_row),
        ("sec.prize.{param,code,meta}", prize_row),
        ("sec.flow_cap", flow_row),
    ):
        default_tag = row[1][default_index].lower()
        if "sim-gated" not in default_tag or not re.search(
            r"phase[- ]0", default_tag
        ):
            raise PhaseGateError(
                f"13 parameter drift at line {row[0]}: {name} must retain its "
                "sim-gated Phase-0 tag; update the Phase-0 gate"
            )

    expected_rows = {row[0] for row in (delta_row, pol_row, prize_row, flow_row)}
    last_classification: tuple[bool, bool] | None = None
    for line_number, cells, raw in rows:
        if len(cells) != len(header_cells):
            raise PhaseGateError(
                f"13 parameter drift at line {line_number}: §1 row has "
                f"{len(cells)} cells, expected {len(header_cells)}"
            )
        default_tag = cells[default_index].lower()
        if "sim-gated" not in default_tag:
            if "sim-gated" in raw.lower():
                raise PhaseGateError(
                    f"13 parameter drift at line {line_number}: sim-gated annotation "
                    "must be in the Default cell"
                )
            last_classification = None
            continue
        has_phase0 = re.search(r"phase[- ]0", default_tag) is not None
        has_phase3_or_arming = (
            re.search(r"phase[- ]3", default_tag) is not None or "arming" in default_tag
        )
        if has_phase0 or has_phase3_or_arming:
            last_classification = (has_phase0, has_phase3_or_arming)
        elif (
            "as above" in default_tag
            and last_classification is not None
        ):
            has_phase0, has_phase3_or_arming = last_classification
        else:
            last_classification = None
        if line_number in expected_rows:
            continue
        if has_phase0:
            names = ", ".join(primary_keys(cells)) or "unnamed row"
            raise PhaseGateError(
                f"13 parameter drift at line {line_number}: unexpected Phase-0 "
                f"sim-gated row {names}; update the Phase-0 gate"
            )
        if not has_phase3_or_arming:
            names = ", ".join(primary_keys(cells)) or "unnamed row"
            raise PhaseGateError(
                f"13 parameter drift at line {line_number}: sim-gated row {names} "
                "does not lexically say Phase-0 or Phase-3/arming; update the Phase-0 gate"
            )

    delta_bounds = (
        first_decimal(delta_row[1][hard_min_index], "dec.delta hard min"),
        first_decimal(delta_row[1][hard_max_index], "dec.delta hard max"),
    )
    pol_bounds = (
        first_decimal(pol_row[1][hard_min_index], "pol.b_baseline hard min"),
        first_decimal(pol_row[1][hard_max_index], "pol.b_baseline hard max"),
    )
    bounds = {key: delta_bounds for key in DELTA_KEYS}
    bounds["pol.b_baseline"] = pol_bounds
    return CalibrationRegistry(CALIBRATION_KEYS, bounds)


def load_sim_evidence(path: Path | None) -> SimEvidence:
    if path is None:
        return SimEvidence(
            "missing",
            None,
            None,
            "pending S4: --sim-evidence was not provided",
        )
    if not path.is_file():
        return SimEvidence(
            "invalid",
            None,
            None,
            f"provided sim evidence file is missing or is not a file: {path}",
        )
    try:
        raw = path.read_bytes()
    except OSError as error:
        return SimEvidence("invalid", None, None, f"cannot read sim evidence: {error}")
    digest = sha256_bytes(raw)
    try:
        document = json.loads(
            raw.decode("utf-8"),
            object_pairs_hook=unique_json_object,
            parse_constant=reject_json_constant,
        )
    except (UnicodeDecodeError, json.JSONDecodeError, ValueError) as error:
        return SimEvidence("invalid", None, digest, f"invalid sim evidence JSON: {error}")
    if not isinstance(document, dict):
        return SimEvidence(
            "invalid", None, digest, "sim evidence top level must be an object"
        )
    return SimEvidence("present", document, digest, "sim evidence loaded")


def validate_sim_envelope(
    document: Mapping[str, Any], expected_commit: str | None = None
) -> str | None:
    if document.get("schema") != SIM_SCHEMA:
        return f"schema must be {SIM_SCHEMA}"
    commit = document.get("git_commit")
    if not isinstance(commit, str) or FULL_SHA_RE.fullmatch(commit) is None:
        return "git_commit must be a full 40-hex SHA"
    if expected_commit is not None and commit.lower() != expected_commit.lower():
        return (
            f"git_commit {commit} does not match checked repository HEAD "
            f"{expected_commit}"
        )
    return None


def criterion_sim_false_pass(
    evidence: SimEvidence, expected_commit: str | None = None
) -> dict[str, str]:
    if evidence.state == "missing":
        return {"status": "pending-s4", "detail": evidence.detail}
    if evidence.state != "present" or evidence.document is None:
        return {"status": "fail", "detail": evidence.detail}
    document = evidence.document
    envelope_error = validate_sim_envelope(document, expected_commit)
    if envelope_error is not None:
        return {"status": "fail", "detail": envelope_error}

    proposals = document.get("synthetic_proposals")
    if type(proposals) is not int or proposals < 10_000:
        return {
            "status": "fail",
            "detail": "synthetic_proposals must be an integer >= 10000",
        }
    rates = document.get("false_pass_rate")
    if not isinstance(rates, dict) or set(rates) != FALSE_PASS_CLASSES:
        return {
            "status": "fail",
            "detail": "false_pass_rate must contain exactly param, trs, code, meta",
        }
    for name in ("param", "trs", "code", "meta"):
        value = rates[name]
        if (
            type(value) not in (int, float)
            or not math.isfinite(value)
            or not 0.0 <= value <= 1.0
        ):
            return {
                "status": "fail",
                "detail": f"false_pass_rate.{name} must be a finite number in [0,1]",
            }
        if value >= 0.01:
            return {
                "status": "fail",
                "detail": f"false_pass_rate.{name} must be strictly < 0.01 (got {value})",
            }
    attack = document.get("attack_cost_validation")
    if not isinstance(attack, dict):
        return {"status": "fail", "detail": "attack_cost_validation must be an object"}
    if attack.get("validated") is not True:
        return {
            "status": "fail",
            "detail": "attack_cost_validation.validated must be true",
        }
    method = attack.get("method")
    if not isinstance(method, str) or not method.strip():
        return {
            "status": "fail",
            "detail": "attack_cost_validation.method must be a non-empty string",
        }
    return {
        "status": "pass",
        "detail": f"{proposals} proposals; all four false-pass rates are strictly < 0.01",
    }


def numeric_calibration_value(value: Any, key: str) -> Decimal:
    if type(value) not in (int, float) or (
        type(value) is float and not math.isfinite(value)
    ):
        raise PhaseGateError(f"calibration.{key} must be a finite JSON number")
    try:
        numeric = Decimal(str(value))
    except InvalidOperation as error:
        raise PhaseGateError(f"calibration.{key} must be a finite JSON number") from error
    if numeric < 0:
        raise PhaseGateError(
            f"calibration.{key} must be >= 0 (doc-13 Balance/Fixed unsigned domain)"
        )
    return numeric


def criterion_calibration(
    evidence: SimEvidence,
    registry: CalibrationRegistry,
    expected_commit: str | None = None,
) -> dict[str, str]:
    if evidence.state == "missing":
        return {"status": "pending-s4", "detail": evidence.detail}
    if evidence.state != "present" or evidence.document is None:
        return {"status": "fail", "detail": evidence.detail}
    envelope_error = validate_sim_envelope(evidence.document, expected_commit)
    if envelope_error is not None:
        return {"status": "fail", "detail": envelope_error}
    calibration = evidence.document.get("calibration")
    if not isinstance(calibration, dict):
        return {"status": "fail", "detail": "calibration must be an object"}
    missing = sorted(registry.keys - set(calibration))
    if missing:
        return {
            "status": "fail",
            "detail": "calibration is missing Phase-0 key(s): " + ", ".join(missing),
        }
    try:
        values = {
            key: numeric_calibration_value(calibration[key], key)
            for key in registry.keys
        }
    except PhaseGateError as error:
        return {"status": "fail", "detail": str(error)}
    for key, (minimum, maximum) in registry.bounds.items():
        value = values[key]
        if value < minimum or value > maximum:
            return {
                "status": "fail",
                "detail": (
                    f"calibration.{key}={value} is outside doc-13 bounds "
                    f"[{minimum}, {maximum}]"
                ),
            }
    return {
        "status": "pass",
        "detail": f"all {len(registry.keys)} Phase-0 calibration keys are published",
    }


def evaluate(config: GateConfig, runner: Runner) -> dict[str, Any]:
    root = config.root.resolve()
    if config.allow_dirty and not config.reduced:
        raise PhaseGateError("--allow-dirty is permitted only with --reduced")

    report_path = rooted(root, config.report_out)
    assert report_path is not None
    report_path = lexical_absolute(report_path)
    sim_path = rooted(root, config.sim_evidence)
    sim_path = lexical_absolute(sim_path) if sim_path is not None else None
    sweep_path = rooted(root, config.sweep_dir)
    sweep_path = lexical_absolute(sweep_path) if sweep_path is not None else None
    allowed_dirty_paths = [report_path]
    if sim_path is not None:
        allowed_dirty_paths.append(sim_path)
    if sweep_path is not None:
        allowed_dirty_paths.append(sweep_path)

    if sweep_path is not None:
        validate_sweep_exemption(root, sweep_path, runner)

    commit = git_head(root, runner)
    pre_run_dirty = dirty_tree_paths(root, runner, allowed_dirty_paths)
    if not config.allow_dirty:
        require_clean_tree("before reference legs", pre_run_dirty)

    evidence = load_sim_evidence(sim_path)
    registry = load_calibration_registry(
        root / "docs" / "architecture" / "13-parameters.md"
    )

    ordinary, generate_leg, sweep_leg = reference_legs(Path("<corpus-audit>"))
    known_leg_ids = frozenset(
        leg.identifier for leg in (*ordinary, generate_leg, sweep_leg)
    )
    corpus_families = load_corpus_family_coverage(
        root / "reference-model" / "fixtures" / "vectors.json", known_leg_ids
    )

    def finish_reference(path: Path, generate: bool) -> dict[str, Any]:
        return check_reference_equivalence(
            root,
            runner,
            sweep_dir=path,
            generate_sweep=generate,
            reduced=config.reduced,
            corpus_families=corpus_families,
        )

    if sweep_path is not None:
        reference = finish_reference(sweep_path.resolve(), False)
    elif config.reduced:
        reference = finish_reference(Path("<generated-full-sweep>"), False)
    else:
        with tempfile.TemporaryDirectory(prefix="bleavit-phase0-") as temporary:
            reference = finish_reference(Path(temporary) / "sweep", True)

    criteria: dict[str, Any] = {
        "reference-equivalence": reference,
        "sim-false-pass": criterion_sim_false_pass(evidence, commit),
        "calibration-published": criterion_calibration(evidence, registry, commit),
    }
    post_run_dirty = dirty_tree_paths(root, runner, allowed_dirty_paths)
    if not config.allow_dirty:
        require_clean_tree("after reference legs", post_run_dirty)
    tree_clean = not pre_run_dirty and not post_run_dirty
    phase0_exit = (
        not config.reduced
        and not config.allow_dirty
        and all(row["status"] == "pass" for row in criteria.values())
    )
    report: dict[str, Any] = {
        "schema": REPORT_SCHEMA,
        "git_commit": commit,
        "criteria": criteria,
        "phase0_exit": phase0_exit,
        "tree_clean": tree_clean,
    }
    if evidence.sha256 is not None:
        report["sim_evidence_sha256"] = evidence.sha256
    return report


def run(config: GateConfig, runner: Runner) -> int:
    report = evaluate(config, runner)
    report_out = rooted(config.root.resolve(), config.report_out)
    assert report_out is not None
    write_json(report_out, report)
    for name, criterion in report["criteria"].items():
        print(f"{name}: {criterion['status']}")
    corpus = report["criteria"]["reference-equivalence"]["corpus_families"]
    print("corpus_families.attested: " + ", ".join(corpus["attested"]))
    print("corpus_families.unattested: " + ", ".join(corpus["unattested"]))
    print(f"tree_clean: {str(report['tree_clean']).lower()}")
    print(f"phase0_exit: {str(report['phase0_exit']).lower()}")
    print(f"report: {report_out}")
    return 0 if report["phase0_exit"] else 1


def main(argv: Sequence[str] | None = None, runner: Runner | None = None) -> int:
    try:
        args = parse_args(argv)
        config = GateConfig(
            root=args.root,
            report_out=args.report_out,
            sim_evidence=args.sim_evidence,
            sweep_dir=args.sweep_dir,
            reduced=args.reduced,
            allow_dirty=args.allow_dirty,
        )
        return run(config, runner or SubprocessRunner())
    except PhaseGateError as error:
        print(f"ERROR: {error}", file=sys.stderr)
        return 2
    except Exception as error:
        print(f"ERROR: Phase-0 exit gate failed closed: {error}", file=sys.stderr)
        return 2
    except KeyboardInterrupt:
        print("ERROR: interrupted", file=sys.stderr)
        return 130


if __name__ == "__main__":
    raise SystemExit(main())

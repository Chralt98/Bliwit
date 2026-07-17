#!/usr/bin/env python3
"""Gate committed FRAME weight diffs against Bleavit's regression budget.

Architecture 15 §4.5 requires CI to reject weight regressions greater than 10%
and to track PoV size against architecture 13's budgets. This deterministic gate
compares committed generated weights; it never re-runs wall-clock benchmarks on
heterogeneous CI machines.

Generated omni-bencher weights are linear expressions, not single constants. The
parser therefore sums every non-multiplied ``Weight::from_parts`` term, captures
per-component ref-time/proof slopes, component high bounds, and fixed plus linear
database reads/writes. The gate evaluates ref-time, proof size, reads, and writes
at each revision's declared worst-case component bounds and applies the §4.5
greater-than-10% rule to those four totals. Absolute changes of at most 1,000,000
ps, 1,024 proof bytes, or one database read/write are tolerated as small floors.
This prevents a numerically small slope from hiding a large bounded-range cost.

New weights are allowed, but a removed function or file can hide a rename or move
and therefore fails unless explicitly acknowledged. Intentional regressions and
removals require a scoped entry in weight-regression-acks.toml; stale entries fail.
"""
from __future__ import annotations

import argparse
import re
import subprocess
import sys
from dataclasses import dataclass, field
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
ACK_FILE = ROOT / "tools" / "ci" / "weight-regression-acks.toml"
LIMIT_NUMERATOR = 110
LIMIT_DENOMINATOR = 100
REF_TIME_FLOOR = 1_000_000
PROOF_SIZE_FLOOR = 1_024
DB_ACCESS_FLOOR = 1

# Only the runtime's generated weight files are gated: they are the weights the
# chain actually dispatches with (configs.rs points every pallet at them). The
# per-pallet `pallets/*/src/weights.rs` files are macro-based mock placeholders
# whose constants are not extractable per-function and are not consensus
# weights; gating them would silently compare nothing. `mod.rs` carries no
# weights.
WEIGHT_PATH_RES = (
    re.compile(r"^runtime/bleavit-runtime/src/weights/(?!mod\.rs$)[^/]+\.rs$"),
)
WEIGHT_IMPL_RE = re.compile(
    r"\bimpl\b[^\{;]*\bWeightInfo\s+for\s+[^\{;]+\{", re.DOTALL
)
WEIGHT_FN_RE = re.compile(
    r"\bfn\s+([A-Za-z_][A-Za-z0-9_]*)\s*\([^\{;]*?\)\s*->\s*Weight\s*\{",
    re.DOTALL,
)
INTEGER_RE = r"([0-9][0-9_]*)(?:u64)?"
FROM_PARTS_RE = re.compile(
    r"Weight\s*::\s*from_parts\s*\(\s*"
    + INTEGER_RE
    + r"\s*,\s*"
    + INTEGER_RE
    + r"\s*\)"
)
SLOPE_RE = re.compile(
    r"Weight\s*::\s*from_parts\s*\(\s*"
    + INTEGER_RE
    + r"\s*,\s*"
    + INTEGER_RE
    + r"\s*\)\s*"
    r"\.\s*saturating_mul\s*\(\s*"
    r"([A-Za-z_][A-Za-z0-9_]*)(?:\s*\.\s*into\s*\(\s*\)|\s+as\s+u64)"
    r"\s*\)"
)
RANGE_RE = re.compile(
    r"///\s*The range of component\s+`([A-Za-z_][A-Za-z0-9_]*)`\s+is\s+"
    r"`\[\s*([0-9][0-9_]*)\s*,\s*([0-9][0-9_]*)\s*\]`\."
)
DB_FIXED_RE = re.compile(
    r"(?:[A-Za-z_][A-Za-z0-9_]*\s*::\s*)?DbWeight\s*::\s*get\s*\(\s*\)\s*"
    r"\.\s*(reads|writes)\s*\(\s*"
    + INTEGER_RE
    + r"\s*\)"
)
DB_SLOPE_RE = re.compile(
    r"(?:[A-Za-z_][A-Za-z0-9_]*\s*::\s*)?DbWeight\s*::\s*get\s*\(\s*\)\s*"
    r"\.\s*(reads|writes)\s*\(\s*\(\s*"
    + INTEGER_RE
    + r"\s*\)\s*\.\s*saturating_mul\s*\(\s*"
    r"([A-Za-z_][A-Za-z0-9_]*)(?:\s*\.\s*into\s*\(\s*\)|\s+as\s+u64)"
    r"\s*\)\s*\)"
)
ACK_RE = re.compile(
    r"^(\S+)\s+(\*|[A-Za-z_][A-Za-z0-9_]*)\s*:\s*(\S(?:.*\S)?)$"
)


class CheckError(RuntimeError):
    """A user-facing configuration or repository error."""


@dataclass(frozen=True)
class FunctionWeight:
    ref_time: int
    proof_size: int
    slopes: dict[str, tuple[int, int]] = field(default_factory=dict)
    reads: int = 0
    writes: int = 0
    read_slopes: dict[str, int] = field(default_factory=dict)
    write_slopes: dict[str, int] = field(default_factory=dict)
    ranges: dict[str, tuple[int, int]] = field(default_factory=dict)

    def worst_case_totals(self) -> dict[str, int]:
        components = (
            set(self.slopes) | set(self.read_slopes) | set(self.write_slopes)
        )
        missing_ranges = sorted(components - set(self.ranges))
        if missing_ranges:
            raise CheckError(
                "linear weight component(s) have no generated range: "
                + ", ".join(missing_ranges)
            )
        ref_time = self.ref_time
        proof_size = self.proof_size
        reads = self.reads
        writes = self.writes
        for component, (_, high) in self.ranges.items():
            ref_slope, proof_slope = self.slopes.get(component, (0, 0))
            ref_time += ref_slope * high
            proof_size += proof_slope * high
            reads += self.read_slopes.get(component, 0) * high
            writes += self.write_slopes.get(component, 0) * high
        return {
            "worst_case.ref_time": ref_time,
            "worst_case.proof_size": proof_size,
            "worst_case.reads": reads,
            "worst_case.writes": writes,
        }


@dataclass(frozen=True)
class Regression:
    quantity: str
    base: int
    head: int


@dataclass
class Comparison:
    unacknowledged: dict[tuple[str, str], list[Regression]] = field(default_factory=dict)
    acknowledged: dict[tuple[str, str], list[Regression]] = field(default_factory=dict)
    unacknowledged_removals: dict[tuple[str, str], str] = field(default_factory=dict)
    acknowledged_removals: dict[tuple[str, str], str] = field(default_factory=dict)
    stale_acks: dict[tuple[str, str], str] = field(default_factory=dict)
    notices: list[str] = field(default_factory=list)
    compared_functions: int = 0


@dataclass(frozen=True)
class BaseResolution:
    commit: str | None
    notices: tuple[str, ...] = ()


def matching_weight_path(path: str) -> bool:
    return any(pattern.fullmatch(path) for pattern in WEIGHT_PATH_RES)


def matching_brace(text: str, opening: int) -> int:
    depth = 0
    for index in range(opening, len(text)):
        if text[index] == "{":
            depth += 1
        elif text[index] == "}":
            depth -= 1
            if depth == 0:
                return index
    raise CheckError(f"unbalanced Rust braces at byte {opening}")


def parse_integer(value: str) -> int:
    return int(value.replace("_", ""))


def add_slope(slopes: dict[str, int], component: str, value: int) -> None:
    slopes[component] = slopes.get(component, 0) + value


def parse_weight_file(text: str) -> dict[str, FunctionWeight]:
    """Parse the last direct WeightInfo implementation in generated Rust."""
    parsed: dict[str, FunctionWeight] = {}
    for implementation in WEIGHT_IMPL_RE.finditer(text):
        impl_opening = text.find("{", implementation.start(), implementation.end())
        impl_closing = matching_brace(text, impl_opening)
        impl_body = text[impl_opening + 1 : impl_closing]
        implementation_functions: dict[str, FunctionWeight] = {}
        preceding_end = 0
        for function in WEIGHT_FN_RE.finditer(impl_body):
            name = function.group(1)
            opening_local = impl_body.find("{", function.start(), function.end())
            opening = impl_opening + 1 + opening_local
            closing = matching_brace(text, opening)
            closing_local = closing - (impl_opening + 1)
            documentation = impl_body[preceding_end : function.start()]
            preceding_end = closing_local + 1
            body = text[opening + 1 : closing]

            ranges: dict[str, tuple[int, int]] = {}
            for component_range in RANGE_RE.finditer(documentation):
                component = component_range.group(1)
                bounds = (
                    parse_integer(component_range.group(2)),
                    parse_integer(component_range.group(3)),
                )
                if component in ranges and ranges[component] != bounds:
                    raise CheckError(
                        f"conflicting ranges for component {component} in {name}"
                    )
                ranges[component] = bounds

            slope_matches = list(SLOPE_RE.finditer(body))
            slope_spans = [match.span() for match in slope_matches]
            ref_time = 0
            proof_size = 0
            for term in FROM_PARTS_RE.finditer(body):
                if any(start <= term.start() < end for start, end in slope_spans):
                    continue
                ref_time += parse_integer(term.group(1))
                proof_size += parse_integer(term.group(2))
            if not list(FROM_PARTS_RE.finditer(body)):
                continue

            slopes: dict[str, tuple[int, int]] = {}
            for slope in slope_matches:
                component = slope.group(3)
                old_ref_time, old_proof_size = slopes.get(component, (0, 0))
                slopes[component] = (
                    old_ref_time + parse_integer(slope.group(1)),
                    old_proof_size + parse_integer(slope.group(2)),
                )

            reads = 0
            writes = 0
            for access in DB_FIXED_RE.finditer(body):
                value = parse_integer(access.group(2))
                if access.group(1) == "reads":
                    reads += value
                else:
                    writes += value

            read_slopes: dict[str, int] = {}
            write_slopes: dict[str, int] = {}
            for access in DB_SLOPE_RE.finditer(body):
                value = parse_integer(access.group(2))
                component = access.group(3)
                if access.group(1) == "reads":
                    add_slope(read_slopes, component, value)
                else:
                    add_slope(write_slopes, component, value)

            function_weight = FunctionWeight(
                ref_time=ref_time,
                proof_size=proof_size,
                slopes=slopes,
                reads=reads,
                writes=writes,
                read_slopes=read_slopes,
                write_slopes=write_slopes,
                ranges=ranges,
            )
            # Validate now so malformed generated expressions cannot become a
            # comparison-time blind spot (including newly added functions).
            function_weight.worst_case_totals()
            implementation_functions[name] = function_weight

        # Generated files should have one live implementation. If a file also
        # contains a default/legacy implementation, the last one is authoritative.
        parsed = implementation_functions
    return parsed


def run_git(*args: str) -> str:
    result = subprocess.run(
        ["git", *args],
        cwd=ROOT,
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    if result.returncode != 0:
        detail = result.stderr.strip() or result.stdout.strip()
        raise CheckError(f"git {' '.join(args)} failed: {detail}")
    return result.stdout


def resolve_base(requested: str | None) -> BaseResolution:
    revision = requested
    if revision is None:
        revision = run_git("merge-base", "HEAD", "origin/main").strip()
        if not revision:
            raise CheckError("git merge-base HEAD origin/main returned no revision")
    resolved = run_git("rev-parse", "--verify", f"{revision}^{{commit}}").strip()
    head = run_git("rev-parse", "--verify", "HEAD^{commit}").strip()
    if resolved != head:
        return BaseResolution(resolved)

    parent_line = run_git("rev-list", "--parents", "-n", "1", "HEAD").split()
    if len(parent_line) == 1:
        return BaseResolution(
            None,
            (
                "comparison base resolves to HEAD, but HEAD is the initial commit; "
                "no parent exists, so the weight-regression comparison is skipped",
            ),
        )
    return BaseResolution(
        parent_line[1],
        (
            "comparison base resolves to HEAD; falling back to HEAD~1 so a push "
            "to the main branch cannot compare the commit with itself",
        ),
    )


def base_weight_paths(base: str) -> set[str]:
    paths = run_git("ls-tree", "-r", "--name-only", base, "--", "runtime")
    return {path for path in paths.splitlines() if matching_weight_path(path)}


def working_weight_paths() -> set[str]:
    paths: set[str] = set()
    runtime_weights = ROOT / "runtime" / "bleavit-runtime" / "src" / "weights"
    if runtime_weights.is_dir():
        paths.update(
            path.relative_to(ROOT).as_posix()
            for path in runtime_weights.glob("*.rs")
            if path.is_file() and path.name != "mod.rs"
        )
    return paths


def git_file(base: str, path: str) -> str:
    return run_git("show", f"{base}:{path}")


def parse_acknowledgements(text: str) -> dict[tuple[str, str], str]:
    acknowledgements: dict[tuple[str, str], str] = {}
    for line_number, raw_line in enumerate(text.splitlines(), 1):
        line = raw_line.strip()
        if not line or line.startswith("#"):
            continue
        match = ACK_RE.fullmatch(line)
        if match is None:
            raise CheckError(
                f"{ACK_FILE.relative_to(ROOT)}:{line_number}: expected "
                "'<repo-relative-weight-path> <function-or-*>: <justification>'"
            )
        path, function, justification = match.groups()
        if not matching_weight_path(path):
            raise CheckError(
                f"{ACK_FILE.relative_to(ROOT)}:{line_number}: not a discovered weight path: {path}"
            )
        key = (path, function)
        if key in acknowledgements:
            raise CheckError(
                f"{ACK_FILE.relative_to(ROOT)}:{line_number}: duplicate acknowledgement "
                f"for {path} {function}"
            )
        acknowledgements[key] = justification
    return acknowledgements


def exceeds_limit(base: int, head: int, floor: int) -> bool:
    if head <= base or head - base <= floor:
        return False
    return head * LIMIT_DENOMINATOR > base * LIMIT_NUMERATOR


def compare_function(base: FunctionWeight, head: FunctionWeight) -> list[Regression]:
    base_totals = base.worst_case_totals()
    head_totals = head.worst_case_totals()
    floors = {
        "worst_case.ref_time": REF_TIME_FLOOR,
        "worst_case.proof_size": PROOF_SIZE_FLOOR,
        "worst_case.reads": DB_ACCESS_FLOOR,
        "worst_case.writes": DB_ACCESS_FLOOR,
    }
    return [
        Regression(quantity, base_totals[quantity], head_totals[quantity])
        for quantity in floors
        if exceeds_limit(base_totals[quantity], head_totals[quantity], floors[quantity])
    ]


def compare_weight_sets(
    base_files: dict[str, dict[str, FunctionWeight]],
    head_files: dict[str, dict[str, FunctionWeight]],
    acknowledgements: dict[tuple[str, str], str],
) -> Comparison:
    result = Comparison()
    active_acknowledgements: set[tuple[str, str]] = set()
    for path in sorted(set(base_files) | set(head_files)):
        if path not in base_files:
            result.notices.append(f"NEW weight file allowed: {path}")
            continue
        if path not in head_files:
            key = (path, "*")
            detail = "weight file removed"
            if key in acknowledgements:
                result.acknowledged_removals[key] = detail
                active_acknowledgements.add(key)
            else:
                result.unacknowledged_removals[key] = detail
            continue

        base_functions = base_files[path]
        head_functions = head_files[path]
        for function in sorted(set(base_functions) | set(head_functions)):
            key = (path, function)
            if function not in base_functions:
                result.notices.append(f"NEW weight function allowed: {path}::{function}")
                continue
            if function not in head_functions:
                detail = "weight function removed"
                if key in acknowledgements:
                    result.acknowledged_removals[key] = detail
                    active_acknowledgements.add(key)
                else:
                    result.unacknowledged_removals[key] = detail
                continue
            result.compared_functions += 1
            regressions = compare_function(base_functions[function], head_functions[function])
            if not regressions:
                continue
            if key in acknowledgements:
                result.acknowledged[key] = regressions
                active_acknowledgements.add(key)
            else:
                result.unacknowledged[key] = regressions

    result.stale_acks = {
        key: justification
        for key, justification in acknowledgements.items()
        if key not in active_acknowledgements
    }
    return result


def format_delta(regression: Regression) -> str:
    if regression.base == 0:
        change = "new from zero"
    else:
        percent = (regression.head - regression.base) * 100.0 / regression.base
        change = f"+{percent:.2f}%"
    return (
        f"{regression.quantity}: {regression.base:,} -> "
        f"{regression.head:,} ({change}; limit 10%)"
    )


def report_comparison(
    comparison: Comparison,
    acknowledgements: dict[tuple[str, str], str],
    base: str,
    file_count: int,
) -> bool:
    print(f"Weight regression base: {base}")
    print(
        f"Discovered {file_count} weight file(s); compared "
        f"{comparison.compared_functions} shared function(s)."
    )
    for notice in comparison.notices:
        print(f"NOTICE: {notice}")
    for key, detail in sorted(comparison.acknowledged_removals.items()):
        path, function = key
        print(
            f"ACKNOWLEDGED REMOVAL: {path}::{function}: {detail}; "
            f"{acknowledgements[key]}"
        )
    for key, regressions in sorted(comparison.acknowledged.items()):
        path, function = key
        print(
            f"ACKNOWLEDGED REGRESSION: {path}::{function}: "
            f"{acknowledgements[key]}"
        )
        for regression in regressions:
            print(f"  {format_delta(regression)}")
    for key, detail in sorted(comparison.unacknowledged_removals.items()):
        path, function = key
        print(f"REMOVAL: {path}::{function}: {detail}", file=sys.stderr)
    for key, regressions in sorted(comparison.unacknowledged.items()):
        path, function = key
        print(f"REGRESSION: {path}::{function}", file=sys.stderr)
        for regression in regressions:
            print(f"  {format_delta(regression)}", file=sys.stderr)
    for key, justification in sorted(comparison.stale_acks.items()):
        path, function = key
        print(
            f"STALE ACKNOWLEDGEMENT: {path}::{function}: {justification}",
            file=sys.stderr,
        )

    failed = bool(
        comparison.unacknowledged
        or comparison.unacknowledged_removals
        or comparison.stale_acks
    )
    if failed:
        print(
            "FAIL: weight regressions and removals require a current scoped "
            "acknowledgement; stale acknowledgements must be removed.",
            file=sys.stderr,
        )
        return False
    if comparison.acknowledged or comparison.acknowledged_removals:
        print("PASS WITH ACKNOWLEDGEMENTS: all regressions/removals are justified.")
    else:
        print("PASS: no worst-case weight total regresses by more than 10%.")
    return True


FIXTURE_PATH = "runtime/bleavit-runtime/src/weights/pallet_example.rs"
FIXTURE_BASE = """
impl<T: frame_system::Config> pallet_example::WeightInfo for ExampleWeight<T> {
    /// The range of component `c` is `[1, 100]`.
    fn trade(c: u32) -> Weight {
        Weight::from_parts(10_000_000, 0)
            .saturating_add(Weight::from_parts(0, 1_000))
            .saturating_add(Weight::from_parts(10_000, 2).saturating_mul(c.into()))
            .saturating_add(T::DbWeight::get().reads(5))
            .saturating_add(T::DbWeight::get().reads((1_u64).saturating_mul(c.into())))
            .saturating_add(T::DbWeight::get().writes(2))
            .saturating_add(T::DbWeight::get().writes((2_u64).saturating_mul(c as u64)))
    }
}
"""


def replace_once(text: str, old: str, new: str) -> str:
    assert text.count(old) == 1, f"fixture term is not unique: {old}"
    return text.replace(old, new)


def fixture_comparison(
    head: str, acknowledgements: dict[tuple[str, str], str] | None = None
) -> Comparison:
    return compare_weight_sets(
        {FIXTURE_PATH: parse_weight_file(FIXTURE_BASE)},
        {FIXTURE_PATH: parse_weight_file(head)},
        acknowledgements or {},
    )


def run_self_tests() -> None:
    key = (FIXTURE_PATH, "trade")
    parsed = parse_weight_file(FIXTURE_BASE)["trade"]

    # Split fixed terms and both generated DB slope casts parse in full.
    assert parsed.ref_time == 10_000_000
    assert parsed.proof_size == 1_000
    assert parsed.slopes == {"c": (10_000, 2)}
    assert parsed.reads == 5 and parsed.read_slopes == {"c": 1}
    assert parsed.writes == 2 and parsed.write_slopes == {"c": 2}
    assert parsed.ranges == {"c": (1, 100)}
    assert parsed.worst_case_totals() == {
        "worst_case.ref_time": 11_000_000,
        "worst_case.proof_size": 1_200,
        "worst_case.reads": 105,
        "worst_case.writes": 202,
    }

    proof_regression = fixture_comparison(
        replace_once(FIXTURE_BASE, "(0, 1_000)", "(0, 2_200)")
    )
    assert [r.quantity for r in proof_regression.unacknowledged[key]] == [
        "worst_case.proof_size"
    ]

    reads_regression = fixture_comparison(
        replace_once(FIXTURE_BASE, ".reads(5)", ".reads(18)")
    )
    assert [r.quantity for r in reads_regression.unacknowledged[key]] == [
        "worst_case.reads"
    ]

    one_extra_read = fixture_comparison(
        replace_once(FIXTURE_BASE, ".reads(5)", ".reads(6)")
    )
    assert not one_extra_read.unacknowledged

    # A raw slope delta below the ref-time floor still exceeds that floor after
    # evaluation at the generated high bound and must fail.
    slope_regression = fixture_comparison(
        replace_once(FIXTURE_BASE, "(10_000, 2)", "(22_000, 2)")
    )
    assert [r.quantity for r in slope_regression.unacknowledged[key]] == [
        "worst_case.ref_time"
    ]

    acknowledged = fixture_comparison(
        replace_once(FIXTURE_BASE, "(0, 1_000)", "(0, 2_200)"),
        {key: "measured proof growth is intentional"},
    )
    assert key in acknowledged.acknowledged and not acknowledged.stale_acks

    stale = fixture_comparison(FIXTURE_BASE, {key: "obsolete"})
    assert key in stale.stale_acks

    new_function_text = FIXTURE_BASE.replace(
        "\n}\n",
        "\n    fn newly_added() -> Weight { Weight::from_parts(99_000_000, 9_900) }\n}\n",
    )
    new_function = fixture_comparison(new_function_text)
    assert not new_function.unacknowledged
    assert any("newly_added" in notice for notice in new_function.notices)

    removed_text = replace_once(
        FIXTURE_BASE,
        "    fn trade(c: u32) -> Weight {",
        "    fn renamed(c: u32) -> Weight {",
    )
    removed = fixture_comparison(removed_text)
    assert key in removed.unacknowledged_removals
    removed_acknowledged = fixture_comparison(
        removed_text, {key: "renamed with an audited call mapping"}
    )
    assert key in removed_acknowledged.acknowledged_removals
    assert not removed_acknowledged.stale_acks

    file_key = (FIXTURE_PATH, "*")
    file_removed = compare_weight_sets(
        {FIXTURE_PATH: parse_weight_file(FIXTURE_BASE)}, {}, {}
    )
    assert file_key in file_removed.unacknowledged_removals
    file_removed_acknowledged = compare_weight_sets(
        {FIXTURE_PATH: parse_weight_file(FIXTURE_BASE)},
        {},
        {file_key: "pallet retired by approved migration"},
    )
    assert file_key in file_removed_acknowledged.acknowledged_removals
    parsed_file_ack = parse_acknowledgements(
        f"{FIXTURE_PATH} *: pallet retired by approved migration\n"
    )
    assert parsed_file_ack == {file_key: "pallet retired by approved migration"}

    # A second implementation must replace, not be masked by, the first.
    last_impl = parse_weight_file(
        FIXTURE_BASE
        + FIXTURE_BASE.replace("10_000_000", "12_000_000", 1)
    )["trade"]
    assert last_impl.ref_time == 12_000_000

    frame_system = ROOT / "runtime" / "bleavit-runtime" / "src" / "weights" / "frame_system.rs"
    pallet_market = ROOT / "runtime" / "bleavit-runtime" / "src" / "weights" / "pallet_market.rs"
    pallet_epoch = ROOT / "runtime" / "bleavit-runtime" / "src" / "weights" / "pallet_epoch.rs"
    # Structural checks only against the committed generated files: the exact
    # constants change on every regeneration, so pinning them here would break
    # the self-test each time weights are refreshed. Exact-value cases live in
    # the embedded string fixtures above.
    if frame_system.is_file():
        kill_prefix = parse_weight_file(frame_system.read_text(encoding="utf-8"))["kill_prefix"]
        assert kill_prefix.ranges == {"p": (0, 1_000)}
        assert set(kill_prefix.slopes) == {"p"} and kill_prefix.slopes["p"][0] > 0
        assert kill_prefix.read_slopes == {"p": 1}
        assert kill_prefix.write_slopes == {"p": 1}
    if pallet_market.is_file():
        buy = parse_weight_file(pallet_market.read_text(encoding="utf-8"))["buy"]
        assert buy.proof_size > 0 and buy.reads > 0 and buy.writes > 0
    if pallet_epoch.is_file():
        tick = parse_weight_file(pallet_epoch.read_text(encoding="utf-8"))["tick"]
        assert tick.ranges == {"n": (1, 10)}
        # The distinct-payload fixture guarantees a per-item preimage read, so
        # tick MUST carry a positive proof-size slope (the B5 undercharge fix).
        assert set(tick.slopes) == {"n"} and tick.slopes["n"][1] > 0
        assert tick.read_slopes.get("n", 0) >= 1 and tick.write_slopes.get("n", 0) >= 1

    head = run_git("rev-parse", "--verify", "HEAD^{commit}").strip()
    main_push = resolve_base(head)
    parents = run_git("rev-list", "--parents", "-n", "1", "HEAD").split()
    if len(parents) > 1:
        assert main_push.notices and "falling back" in main_push.notices[0]
        assert main_push.commit == parents[1]
    else:
        assert main_push.commit is None

    print("Weight regression self-tests passed (16 cases).")


def load_weight_sets(base: str) -> tuple[
    dict[str, dict[str, FunctionWeight]],
    dict[str, dict[str, FunctionWeight]],
    int,
]:
    base_paths = base_weight_paths(base)
    head_paths = working_weight_paths()
    base_files = {
        path: parse_weight_file(git_file(base, path))
        for path in sorted(base_paths)
    }
    head_files = {
        path: parse_weight_file((ROOT / path).read_text(encoding="utf-8"))
        for path in sorted(head_paths)
    }
    unparseable_head = [path for path, functions in head_files.items() if not functions]
    unparseable_base = [
        path
        for path in sorted(base_paths & head_paths)
        if not base_files[path]
    ]
    if unparseable_head or unparseable_base:
        details: list[str] = []
        if unparseable_head:
            details.append("working tree: " + ", ".join(unparseable_head))
        if unparseable_base:
            details.append("base: " + ", ".join(unparseable_base))
        raise CheckError(
            "weight file(s) yielded zero parseable functions (gate blind spot): "
            + "; ".join(details)
        )
    return base_files, head_files, len(base_paths | head_paths)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--base",
        metavar="REVISION",
        help="base git revision (default: merge-base of HEAD and origin/main)",
    )
    parser.add_argument(
        "--self-test",
        action="store_true",
        help="run embedded parser and regression/removal policy tests",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    try:
        if args.self_test:
            run_self_tests()
            return 0
        resolution = resolve_base(args.base)
        for notice in resolution.notices:
            print(f"NOTICE: {notice}")
        if resolution.commit is None:
            print("PASS: no parent commit exists to compare.")
            return 0
        acknowledgements = parse_acknowledgements(ACK_FILE.read_text(encoding="utf-8"))
        base_files, head_files, file_count = load_weight_sets(resolution.commit)
        comparison = compare_weight_sets(base_files, head_files, acknowledgements)
        return 0 if report_comparison(
            comparison, acknowledgements, resolution.commit, file_count
        ) else 1
    except (CheckError, OSError, AssertionError) as error:
        print(f"ERROR: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())

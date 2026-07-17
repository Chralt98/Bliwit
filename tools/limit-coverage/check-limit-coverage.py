#!/usr/bin/env python3
"""Enforce the generated dispatch-limit registry (15 §4.6; I-22).

This static checker proves that every extracted registry key is classified, that
dispatch-limit markers name enabled Rust tests, and that each marked test is
lexically bound to its declared error or behavior token. It cannot prove that a
test semantically dispatches past the limit; that remains the review half of
I-22's convention enforcement under 15 §1.
"""

from __future__ import annotations

import argparse
import ast
import json
import re
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Iterable

try:
    import tomllib
except ModuleNotFoundError:  # Python 3.10 compatibility for the local quality gate.
    tomllib = None  # type: ignore[assignment]


SECTION_HEADINGS = {
    "params": "## 1. Constitution keys (typed, bounded, rate-limited)",
    "kernel": "## 2. Kernel constants (K — compile-time, constants-API-exposed)",
    "storage": "## 4. Reconciled storage bounds (D-10 — one table, all budgets derive from it)",
}
TABLE_HEADERS = {
    "params": [
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
    ],
    "kernel": ["Constant", "Value", "Doc"],
    "storage": ["Bound", "Value", "Scope (the reconciliation)", "Doc"],
}
PER_CLASS_KEYS = {
    "dec.delta": "dec.delta",
    "dec.sigma": "dec.sigma",
    "dec.v_min": "dec.v_min",
    "prop.bond": "prop.bond",
    "pol.b": "pol.b",
    "exec.timelock": "exec.lock",
    "trs.proposer_reward": "trs.reward",
}
CLASS_SUFFIXES = ("param", "trs", "code", "meta")
CLASSES = {"dispatch-limit", "param-bounds", "value", "diagnostic", "unwired"}
CONSUMER_BINDING = "kernel-constant (B10)"
MARKER = re.compile(r"^\s*//\s*limit-coverage:\s*(.*?)\s*$")
TEST_ATTRIBUTE = re.compile(r"^\s*#\[(?:test\b|[^]]*test_case\b)")
IGNORE_ATTRIBUTE = re.compile(r"^\s*#\[\s*ignore(?:\s*=|\s*\])")
FUNCTION = re.compile(
    r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)\b"
)


class RegistryError(ValueError):
    """A strict registry/checker validation error."""


@dataclass(frozen=True)
class InventoryEntry:
    key: str
    section: str
    row_name: str


@dataclass(frozen=True)
class TestFunction:
    name: str
    attribute_line: int
    function_line: int
    end_line: int
    ignored: bool


@dataclass(frozen=True)
class MarkerReference:
    key: str
    path: Path
    line: int
    function: str | None
    function_body: str
    ignored: bool


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Check 13-parameters.md against the I-22 coverage manifest and Rust tests."
    )
    parser.add_argument(
        "--root",
        type=Path,
        default=Path(__file__).resolve().parents[2],
        help="repository root (defaults to the checker's repository)",
    )
    parser.add_argument(
        "--list",
        action="store_true",
        help="print the extracted inventory as JSON without checking the manifest",
    )
    return parser.parse_args()


def split_markdown_row(line: str) -> list[str]:
    if not line.startswith("|") or not line.endswith("|"):
        raise RegistryError(f"malformed Markdown table row: {line!r}")
    cells: list[str] = []
    current: list[str] = []
    escaped = False
    for character in line[1:-1]:
        if character == "|" and not escaped:
            cells.append("".join(current).strip())
            current = []
        else:
            current.append(character)
        escaped = character == "\\" and not escaped
        if character != "\\":
            escaped = False
    cells.append("".join(current).strip())
    return cells


def is_separator(cells: list[str]) -> bool:
    return bool(cells) and all(re.fullmatch(r":?-{3,}:?", cell) for cell in cells)


def strip_markdown(value: str) -> str:
    value = re.sub(r"\[([^]]+)]\([^)]+\)", r"\1", value)
    value = value.replace("`", "").replace("**", "").replace("*", "")
    value = value.replace("\\|", "|")
    return " ".join(value.split())


def table_rows(document: str, section: str) -> list[list[str]]:
    heading = SECTION_HEADINGS[section]
    lines = document.splitlines()
    matches = [index for index, line in enumerate(lines) if line == heading]
    if len(matches) != 1:
        raise RegistryError(
            f"13 §{section}: expected heading {heading!r} exactly once, found {len(matches)}"
        )
    heading_index = matches[0]
    section_end = next(
        (
            candidate
            for candidate in range(heading_index + 1, len(lines))
            if lines[candidate].startswith("## ")
        ),
        len(lines),
    )
    indented_row = next(
        (
            candidate + 1
            for candidate in range(heading_index + 1, section_end)
            if re.match(r"^\s+\|", lines[candidate])
        ),
        None,
    )
    if indented_row is not None:
        raise RegistryError(
            f"13 §{section}: indented Markdown table row at line {indented_row}"
        )
    index = heading_index + 1
    while index < len(lines) and not lines[index].startswith("|"):
        if lines[index].startswith("## "):
            raise RegistryError(f"13 §{section}: table is missing below {heading!r}")
        index += 1
    if index + 2 >= len(lines):
        raise RegistryError(f"13 §{section}: table is truncated")

    header = split_markdown_row(lines[index])
    expected = TABLE_HEADERS[section]
    if header != expected:
        raise RegistryError(
            f"13 §{section}: table header changed: expected {expected!r}, found {header!r}"
        )
    separator = split_markdown_row(lines[index + 1])
    if len(separator) != len(header) or not is_separator(separator):
        raise RegistryError(f"13 §{section}: malformed Markdown table separator")

    rows: list[list[str]] = []
    index += 2
    while index < len(lines) and lines[index].startswith("|"):
        cells = split_markdown_row(lines[index])
        if section == "storage":
            cells = validate_storage_row(cells, lines[index])
        elif len(cells) != len(header) or any(not cell for cell in cells):
            raise RegistryError(
                f"13 §{section}: row has {len(cells)} columns, expected {len(header)}: "
                f"{lines[index]!r}"
            )
        rows.append(cells)
        index += 1
    if not rows:
        raise RegistryError(f"13 §{section}: table has no data rows")
    later_table_line = next(
        (
            candidate + 1
            for candidate in range(index, section_end)
            if lines[candidate].startswith("|")
        ),
        None,
    )
    if later_table_line is not None:
        raise RegistryError(
            f"13 §{section}: unexpected additional Markdown table at line "
            f"{later_table_line}; the registry section must contain exactly one table"
        )
    return rows


def validate_storage_row(cells: list[str], line: str) -> list[str]:
    """Validate the two explicit §4 row forms without permissive normalization."""
    if len(cells) == 4:
        if any(not cell for cell in cells):
            raise RegistryError(f"13 §storage: malformed four-column row: {line!r}")
        if not re.search(r"\[[^]]+]\([^)]+\)", cells[3]):
            raise RegistryError(f"13 §storage: malformed Doc cell: {line!r}")
        return cells
    if len(cells) == 3:
        if any(not cell for cell in cells):
            raise RegistryError(f"13 §storage: malformed compact row: {line!r}")
        owner = cells[2]
        if not (
            re.search(r"\[[^]]+]\([^)]+\)", owner)
            or re.fullmatch(r"`[^`]+`", owner)
        ):
            raise RegistryError(f"13 §storage: malformed compact row: {line!r}")
        return [cells[0], cells[1], "", owner]
    raise RegistryError(
        f"13 §storage: row has {len(cells)} columns, expected strict 3 or 4: {line!r}"
    )


def code_spans(value: str) -> list[str]:
    return re.findall(r"`([^`]+)`", value)


def validate_param_key(key: str, row: str) -> None:
    if not re.fullmatch(r"[A-Za-z0-9_.]+", key):
        raise RegistryError(f"13 §1: unsupported ParamKey {key!r} in row {row!r}")
    if len(key.encode("utf-8")) > 16:
        raise RegistryError(
            f"13 §1 rule 6: ParamKey {key!r} exceeds 16 bytes without a usable key override"
        )


def extract_param_keys(cell: str) -> list[str]:
    initial = code_spans(cell)
    if not initial:
        raise RegistryError(f"13 §1: row has no backticked key: {cell!r}")

    first = initial[0]
    if first == "ops.*":
        replacements = dict(
            re.findall(r"`([^`]+)`\s*\(key:\s*`([^`]+)`\)", cell)
        )
        keys = [key for key in initial[1:] if key not in replacements]
        keys = [replacements.get(key, key) for key in keys]
        # Override values also occur as code spans; retain them only once.
        keys = list(dict.fromkeys(key for key in keys if key not in replacements.values())) + list(
            replacements.values()
        )
        return keys

    override = re.search(r"\bkeys?:\s*((?:`[^`]+`(?:\s*/\s*)?)+)", cell)
    if override:
        return code_spans(override.group(1))

    if first in PER_CLASS_KEYS:
        base = PER_CLASS_KEYS[first]
        return [f"{base}.{suffix}" for suffix in CLASS_SUFFIXES]

    prefix = cell.split(" (", 1)[0]
    grouped = code_spans(prefix)
    if grouped:
        return grouped
    return [first]


def kernel_key(cell: str) -> str:
    codes = code_spans(cell)
    if codes:
        if codes[0] == "MAX_NESTED":
            return "MAX_NESTED"
        return "/".join(codes)
    plain = strip_markdown(cell).lower()
    slug = re.sub(r"[^a-z0-9]+", "-", plain).strip("-")
    if not slug:
        raise RegistryError(f"13 §2: cannot derive a stable key from {cell!r}")
    return slug


def kernel_keys(cell: str, value: str = "") -> list[str]:
    """Expand only the independent multi-limit rows named by 13 §2."""
    codes = code_spans(cell)
    if codes == [
        "reg.max_filings_epoch",
        "wt.max",
        "att.min_members",
        "att.quorum",
    ]:
        return codes
    if codes == ["MinTrade", "MaxTrade"]:
        return codes
    if codes == ["prop.max_calls", "max_bytes", "max_weight"]:
        return ["prop.max_calls", "prop.max_bytes", "prop.max_weight"]
    batch_codes = code_spans(value) if value else codes
    if strip_markdown(cell).startswith("Crank batch bounds") and batch_codes == [
        "TickBatch",
        "ReapBatch",
        "settle_cohort",
    ]:
        return batch_codes
    if strip_markdown(cell) == "PB-LEDGER-FREEZE":
        # Two independent numeric limits share the row: the ≤ 14-day duration
        # cap and the one-renewal-only cap (the I-4 drift-flag admissibility
        # clause is a gate, not a numeric bound).
        return ["pb-ledger-freeze.duration", "pb-ledger-freeze.renewal"]
    return [kernel_key(cell)]


def storage_key(cell: str) -> str:
    key = strip_markdown(cell)
    if not key:
        raise RegistryError(f"13 §4: empty Bound cell in row {cell!r}")
    return key


def extract_inventory(document: str) -> list[InventoryEntry]:
    inventory: list[InventoryEntry] = []
    for row in table_rows(document, "params"):
        for key in extract_param_keys(row[0]):
            validate_param_key(key, row[0])
            inventory.append(InventoryEntry(key, "params", strip_markdown(row[0])))
    for row in table_rows(document, "kernel"):
        for key in kernel_keys(row[0], row[1]):
            inventory.append(InventoryEntry(key, "kernel", strip_markdown(row[0])))
    for row in table_rows(document, "storage"):
        inventory.append(InventoryEntry(storage_key(row[0]), "storage", strip_markdown(row[0])))

    duplicates = sorted(
        key
        for key in {entry.key for entry in inventory}
        if sum(entry.key == key for entry in inventory) > 1
    )
    if duplicates:
        raise RegistryError(f"13 registry extraction produced duplicate keys: {duplicates!r}")
    return inventory


def parse_manifest_toml_compat(text: str) -> dict[str, Any]:
    """Parse the deliberately tiny [[entry]] TOML subset on Python 3.10."""
    entries: list[dict[str, Any]] = []
    current: dict[str, Any] | None = None
    for line_number, raw in enumerate(text.splitlines(), 1):
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        if line == "[[entry]]":
            current = {}
            entries.append(current)
            continue
        if current is None or "=" not in line:
            raise ValueError(f"line {line_number}: expected [[entry]] or key = value")
        key, raw_value = (part.strip() for part in line.split("=", 1))
        if not re.fullmatch(r"[A-Za-z_][A-Za-z0-9_-]*", key):
            raise ValueError(f"line {line_number}: unsupported key {key!r}")
        if raw_value in {"true", "false"}:
            value: Any = raw_value == "true"
        else:
            try:
                value = ast.literal_eval(raw_value)
            except (SyntaxError, ValueError) as error:
                raise ValueError(
                    f"line {line_number}: unsupported value {raw_value!r}"
                ) from error
        if not isinstance(value, (str, bool)):
            raise ValueError(f"line {line_number}: manifest values must be strings or booleans")
        current[key] = value
    return {"entry": entries}


def load_manifest(path: Path) -> tuple[dict[str, dict[str, Any]], list[str]]:
    failures: list[str] = []
    try:
        if tomllib is None:
            document = parse_manifest_toml_compat(path.read_text(encoding="utf-8"))
        else:
            with path.open("rb") as handle:
                document = tomllib.load(handle)
    except (OSError, UnicodeDecodeError, ValueError) as error:
        return {}, [f"cannot load manifest {path}: {error}"]
    raw_entries = document.get("entry")
    if not isinstance(raw_entries, list):
        return {}, [f"manifest {path} must contain [[entry]] tables"]

    entries: dict[str, dict[str, Any]] = {}
    allowed = {
        "key",
        "class",
        "error",
        "behavior",
        "reason",
        "owner",
        "genesis",
        "consumer_binding",
    }
    for index, raw in enumerate(raw_entries, 1):
        if not isinstance(raw, dict):
            failures.append(f"manifest entry {index} must be a table")
            continue
        unknown_fields = sorted(set(raw) - allowed)
        if unknown_fields:
            failures.append(
                f"manifest entry {index} has unsupported fields: {', '.join(unknown_fields)}"
            )
        key = raw.get("key")
        classification = raw.get("class")
        if not isinstance(key, str) or not key:
            failures.append(f"manifest entry {index} has no non-empty string key")
            continue
        if key in entries:
            failures.append(f"manifest key {key!r} is classified more than once")
            continue
        if classification not in CLASSES:
            failures.append(
                f"manifest key {key!r} has invalid class {classification!r}; "
                f"expected one of {sorted(CLASSES)!r}"
            )
        bindings = [field for field in ("error", "behavior") if field in raw]
        if classification == "dispatch-limit":
            if len(bindings) != 1:
                failures.append(
                    f"dispatch-limit key {key!r} requires exactly one of error/behavior"
                )
            elif not isinstance(raw[bindings[0]], str) or not raw[bindings[0]].strip():
                failures.append(
                    f"dispatch-limit key {key!r} has an empty {bindings[0]} binding"
                )
        elif bindings:
            failures.append(
                f"manifest {classification} key {key!r} must not declare error/behavior"
            )
        if classification in {"value", "diagnostic"} and not raw.get("reason"):
            failures.append(f"manifest {classification} key {key!r} requires a reason")
        if classification == "unwired":
            if not raw.get("reason"):
                failures.append(f"manifest unwired key {key!r} requires a reason")
            if not raw.get("owner"):
                failures.append(f"manifest unwired key {key!r} requires an owner")
        if "owner" in raw and (
            not isinstance(raw["owner"], str) or not raw["owner"].strip()
        ):
            failures.append(f"manifest key {key!r} owner must be a non-empty string")
        if "consumer_binding" in raw and raw["consumer_binding"] != CONSUMER_BINDING:
            failures.append(
                f"manifest key {key!r} has unsupported consumer_binding "
                f"{raw['consumer_binding']!r}; expected {CONSUMER_BINDING!r}"
            )
        if "genesis" in raw and not isinstance(raw["genesis"], bool):
            failures.append(f"manifest key {key!r} genesis must be boolean")
        entries[key] = raw
    return entries, failures


def load_fixture(path: Path) -> tuple[set[str], list[str]]:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
        return set(), [f"cannot load genesis-key fixture {path}: {error}"]
    if not isinstance(value, list) or not all(isinstance(key, str) for key in value):
        return set(), [f"genesis-key fixture {path} must be a JSON array of strings"]
    if value != sorted(value):
        return set(value), [f"genesis-key fixture {path} must be sorted"]
    if len(value) != len(set(value)):
        return set(value), [f"genesis-key fixture {path} contains duplicate keys"]
    return set(value), []


def function_end(lines: list[str], start: int) -> int:
    depth = 0
    opened = False
    in_string = False
    escaped = False
    for line_index in range(start, len(lines)):
        line = lines[line_index]
        column = 0
        while column < len(line):
            character = line[column]
            if not in_string and character == "/" and column + 1 < len(line) and line[column + 1] == "/":
                break
            if character == '"' and not escaped:
                in_string = not in_string
            elif not in_string:
                if character == "{":
                    depth += 1
                    opened = True
                elif character == "}":
                    depth -= 1
            escaped = character == "\\" and not escaped
            if character != "\\":
                escaped = False
            column += 1
        if opened and depth <= 0:
            return line_index + 1
    return len(lines)


def find_test_functions(lines: list[str]) -> list[TestFunction]:
    functions: list[TestFunction] = []
    for index, line in enumerate(lines):
        match = FUNCTION.match(line)
        if not match:
            continue
        cursor = index - 1
        attributes: list[int] = []
        while cursor >= 0:
            stripped = lines[cursor].strip()
            if not stripped or stripped.startswith("//"):
                cursor -= 1
                continue
            if stripped.startswith("#["):
                attributes.append(cursor)
                cursor -= 1
                continue
            break
        if not any(TEST_ATTRIBUTE.match(lines[attr]) for attr in attributes):
            continue
        attribute_line = min(attributes) + 1
        functions.append(
            TestFunction(
                match.group(1),
                attribute_line,
                index + 1,
                function_end(lines, index),
                any(IGNORE_ATTRIBUTE.match(lines[attr]) for attr in attributes),
            )
        )
    return functions


def marker_function(
    line: int, lines: list[str], tests: list[TestFunction]
) -> TestFunction | None:
    for test in tests:
        if test.function_line < line <= test.end_line:
            return test
    for test in tests:
        if line >= test.attribute_line:
            continue
        between = lines[line : test.attribute_line - 1]
        if test.attribute_line - line <= 4 and all(
            not candidate.strip() or candidate.strip().startswith("//") for candidate in between
        ):
            return test
    return None


def scan_markers(root: Path) -> tuple[list[MarkerReference], list[str]]:
    references: list[MarkerReference] = []
    failures: list[str] = []
    for relative_root in ("pallets", "crates", "runtime"):
        directory = root / relative_root
        if not directory.is_dir():
            continue
        for path in sorted(directory.rglob("*.rs")):
            try:
                text = path.read_text(encoding="utf-8")
            except (OSError, UnicodeDecodeError) as error:
                failures.append(f"cannot read Rust test source {path}: {error}")
                continue
            lines = text.splitlines()
            tests = find_test_functions(lines)
            relative = path.relative_to(root)
            test_context = (
                path.name == "tests.rs"
                or "tests" in path.parts
                or "#[cfg(test)]" in text
            )
            for index, line in enumerate(lines, 1):
                match = MARKER.match(line)
                if not match:
                    continue
                keys = [key.strip() for key in match.group(1).split(",")]
                if not keys or any(not key for key in keys):
                    failures.append(f"{relative}:{index}: limit-coverage marker has an empty key")
                    continue
                test = marker_function(index, lines, tests) if test_context else None
                if test is None:
                    failures.append(
                        f"{relative}:{index}: limit-coverage marker is not attached to a test function"
                    )
                elif test.ignored:
                    failures.append(
                        f"{relative}:{index}: limit-coverage marker is attached to ignored "
                        f"test {test.name!r}"
                    )
                for key in keys:
                    references.append(
                        MarkerReference(
                            key,
                            relative,
                            index,
                            test.name if test is not None else None,
                            "\n".join(lines[test.function_line - 1 : test.end_line])
                            if test is not None
                            else "",
                            test.ignored if test is not None else False,
                        )
                    )
    return references, failures


def load_milestone_ids(path: Path) -> tuple[set[str], set[str], list[str]]:
    """Return (all milestone ids, ✅-completed milestone ids, failures).

    Completion is read from the Status column (cell 5) of the six-column
    milestone tables so that an `unwired` exemption mechanically expires: the
    moment its owner milestone flips ✅ the gate goes red until the surface's
    real past-limit test replaces the exemption (I-22's "fails CI until the
    test exists", deferred — never waived)."""
    try:
        text = path.read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError) as error:
        return set(), set(), [f"cannot load milestone owners from {path}: {error}"]
    identifiers: set[str] = set()
    completed: set[str] = set()
    for line in text.splitlines():
        match = re.match(r"^\|\s*([A-Za-z][A-Za-z0-9]*)\s*\|", line)
        if not match:
            continue
        identifiers.add(match.group(1))
        cells = [cell.strip() for cell in line.split("|")]
        if len(cells) >= 6 and "✅" in cells[5]:
            completed.add(match.group(1))
    if not identifiers:
        return set(), set(), ["PLAN.md contains no milestone table row identifiers"]
    return identifiers, completed, []


def validate(root: Path) -> tuple[list[str], list[InventoryEntry], dict[str, dict[str, Any]]]:
    failures: list[str] = []
    spec_path = root / "docs" / "architecture" / "13-parameters.md"
    try:
        inventory = extract_inventory(spec_path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, RegistryError) as error:
        return [str(error)], [], {}

    manifest, manifest_failures = load_manifest(root / "tools" / "limit-coverage" / "registry.toml")
    failures.extend(manifest_failures)
    milestone_ids, completed_milestones, milestone_failures = load_milestone_ids(
        root / "PLAN.md"
    )
    failures.extend(milestone_failures)
    fixture, fixture_failures = load_fixture(
        root / "tools" / "limit-coverage" / "genesis-keys.json"
    )
    failures.extend(fixture_failures)
    inventory_by_key = {entry.key: entry for entry in inventory}

    for key, entry in sorted(manifest.items()):
        owner = entry.get("owner")
        if isinstance(owner, str) and owner not in milestone_ids:
            failures.append(f"manifest key {key!r} has unknown owner {owner!r}")
        elif (
            isinstance(owner, str)
            and entry.get("class") == "unwired"
            and owner in completed_milestones
        ):
            failures.append(
                f"unwired key {key!r} names completed owner {owner!r} — the owning "
                "milestone shipped, so the surface must now carry a real "
                "dispatch-past-limit test instead of an exemption (15 §4.6 / I-22)"
            )
        if entry.get("consumer_binding") == CONSUMER_BINDING:
            if "B10" not in milestone_ids:
                failures.append(
                    f"manifest key {key!r} consumer_binding names unknown owner 'B10'"
                )
            elif "B10" in completed_milestones:
                failures.append(
                    f"manifest key {key!r} consumer_binding defers to B10, which is "
                    "complete — bind the consumer to live Params and drop the annotation"
                )

    for key in sorted(set(inventory_by_key) - set(manifest)):
        failures.append(f"13 registry key {key!r} is missing from registry.toml")
    for key in sorted(set(manifest) - set(inventory_by_key)):
        failures.append(f"manifest key {key!r} was not extracted from 13-parameters.md")

    for key, entry in sorted(manifest.items()):
        source = inventory_by_key.get(key)
        if source is None:
            continue
        if source.section == "params":
            if "genesis" not in entry:
                failures.append(f"manifest §1 key {key!r} must declare genesis = true/false")
                continue
            seeded = entry["genesis"] is True
            if seeded and key not in fixture:
                failures.append(f"manifest §1 key {key!r} claims genesis seeding but is absent from genesis-keys.json")
            if not seeded and key in fixture:
                failures.append(f"manifest §1 key {key!r} claims genesis = false but appears in genesis-keys.json")
            if entry.get("class") == "param-bounds" and not seeded:
                failures.append(f"param-bounds key {key!r} cannot be unseeded")
        elif "genesis" in entry:
            failures.append(f"manifest non-§1 key {key!r} must not declare genesis")
        if entry.get("class") == "param-bounds" and source.section != "params":
            failures.append(f"param-bounds key {key!r} is not a 13 §1 ParamKey")
        if "consumer_binding" in entry and source.section != "params":
            failures.append(f"consumer_binding key {key!r} is not a 13 §1 ParamKey")

    for key in sorted(fixture - set(inventory_by_key)):
        failures.append(f"genesis fixture key {key!r} was not extracted from 13 §1")
    for key in sorted(fixture):
        source = inventory_by_key.get(key)
        if source is not None and source.section != "params":
            failures.append(f"genesis fixture key {key!r} is not a 13 §1 key")

    markers, marker_failures = scan_markers(root)
    failures.extend(marker_failures)
    attached_by_key: dict[str, list[MarkerReference]] = {}
    for marker in markers:
        entry = manifest.get(marker.key)
        if marker.key not in inventory_by_key:
            failures.append(
                f"{marker.path}:{marker.line}: marker references unknown 13 key {marker.key!r}"
            )
        elif entry is None:
            # The missing-manifest failure above is the primary drift diagnostic.
            continue
        elif entry.get("class") not in {"dispatch-limit", "unwired"}:
            failures.append(
                f"{marker.path}:{marker.line}: marker key {marker.key!r} is classed "
                f"{entry.get('class')!r}, not 'dispatch-limit' or 'unwired'"
            )
        elif entry.get("class") == "dispatch-limit" and marker.function is not None:
            if not marker.ignored:
                attached_by_key.setdefault(marker.key, []).append(marker)
            binding = entry.get("error") or entry.get("behavior")
            if isinstance(binding, str):
                token = binding.rsplit("::", 1)[-1]
                if token not in marker.function_body:
                    failures.append(
                        f"{marker.path}:{marker.line}: marked test {marker.function!r} for "
                        f"{marker.key!r} does not contain binding token {token!r}"
                    )

    for key, entry in sorted(manifest.items()):
        if entry.get("class") == "dispatch-limit" and not attached_by_key.get(key):
            failures.append(f"dispatch-limit key {key!r} has zero attached test markers")
        if entry.get("class") == "param-bounds" and key not in fixture:
            failures.append(f"param-bounds key {key!r} is absent from genesis-keys.json")

    return failures, inventory, manifest


def class_counts(manifest: Iterable[dict[str, Any]]) -> str:
    counts = {classification: 0 for classification in sorted(CLASSES)}
    for entry in manifest:
        classification = entry.get("class")
        if classification in counts:
            counts[classification] += 1
    return ", ".join(f"{classification}={counts[classification]}" for classification in sorted(counts))


def print_visibility(manifest: dict[str, dict[str, Any]]) -> None:
    unwired = sorted(
        (key, entry.get("owner", "<missing>"))
        for key, entry in manifest.items()
        if entry.get("class") == "unwired"
    )
    print(f"limit coverage: unwired keys ({len(unwired)}):")
    for key, owner in unwired:
        print(f"  - {key} (owner {owner})")
    bindings = sorted(
        (key, entry["consumer_binding"])
        for key, entry in manifest.items()
        if "consumer_binding" in entry
    )
    print(f"limit coverage: consumer-binding keys ({len(bindings)}):")
    for key, binding in bindings:
        print(f"  - {key} ({binding})")


def main() -> int:
    args = parse_args()
    root = args.root.resolve()
    if args.list:
        try:
            inventory = extract_inventory(
                (root / "docs" / "architecture" / "13-parameters.md").read_text(encoding="utf-8")
            )
        except (OSError, UnicodeDecodeError, RegistryError) as error:
            print(f"limit coverage: {error}", file=sys.stderr)
            return 1
        print(
            json.dumps(
                [
                    {"key": entry.key, "section": entry.section, "row": entry.row_name}
                    for entry in inventory
                ],
                indent=2,
                sort_keys=True,
            )
        )
        return 0

    failures, inventory, manifest = validate(root)
    print_visibility(manifest)
    if failures:
        for failure in failures:
            print(f"limit coverage: {failure}", file=sys.stderr)
        return 1
    print(
        f"limit coverage: {len(inventory)} registry keys covered "
        f"({class_counts(manifest.values())})"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

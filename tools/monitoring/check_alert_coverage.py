#!/usr/bin/env python3
"""Strict 12 section 6.3 alert-table and Prometheus coverage gate (O5)."""

from __future__ import annotations

import argparse
import importlib.util
import re
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Mapping

import yaml

try:
    import tomllib
except ModuleNotFoundError:  # Local gate compatibility; production is Python 3.12.
    import toml_compat as tomllib  # type: ignore[no-redef]


HEADING = "### 6.3 Monitoring and alerting"
MILESTONES_HEADING = "## Milestones"
MILESTONE_HEADER = ["ID", "Milestone", "Spec", "Depends", "Status", "Notes"]
MILESTONE_STATUSES = {"⬜", "🔨", "✅", "⛔", "🅿"}
HEADERS = (
    ["Domain", "Key series", "Alert (example)", "Runbook"],
    ["Domain", "Key series", "Alert", "Runbook"],
)
SOURCES = {"chain-exporter", "keeper", "node", "attestation-monitor", "seam"}
METRIC_REFERENCE = re.compile(r"\b(?:bleavit|substrate)_[A-Za-z_:][A-Za-z0-9_:]*\b")
RUNBOOK = re.compile(r"\bRB-[A-Z]+\b")


class CoverageError(ValueError):
    pass


@dataclass(frozen=True)
class AlertRow:
    domain: str
    key_series: str
    threshold: str
    runbook: str
    page_immediately: bool


def split_row(line: str) -> list[str]:
    if not line.startswith("|") or not line.endswith("|"):
        raise CoverageError(f"malformed Markdown table row: {line!r}")
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


def separator(cells: list[str]) -> bool:
    return bool(cells) and all(re.fullmatch(r":?-{3,}:?", cell) for cell in cells)


def plain(value: str) -> str:
    value = re.sub(r"\[([^]]+)]\([^)]+\)", r"\1", value)
    value = value.replace("`", "").replace("**", "").replace("*", "")
    return " ".join(value.replace("\\|", "|").split())


def extract_rows(document: str) -> list[AlertRow]:
    lines = document.splitlines()
    headings = [index for index, line in enumerate(lines) if line == HEADING]
    if len(headings) != 1:
        raise CoverageError(f"12 §6.3 heading drift: expected exactly one {HEADING!r}")
    start = headings[0] + 1
    end = next(
        (index for index in range(start, len(lines)) if lines[index].startswith("### ")),
        len(lines),
    )
    if any(re.match(r"^\s+\|", lines[index]) for index in range(start, end)):
        raise CoverageError("12 §6.3 contains an indented Markdown table row")
    table_starts = [
        index
        for index in range(start, end)
        if lines[index].startswith("|")
        and (index == start or not lines[index - 1].startswith("|"))
    ]
    if len(table_starts) != 2:
        raise CoverageError(f"12 §6.3 must contain exactly two tables, found {len(table_starts)}")
    rows: list[AlertRow] = []
    for table_index, index in enumerate(table_starts):
        header = split_row(lines[index])
        if header != HEADERS[table_index]:
            raise CoverageError(
                f"12 §6.3 table {table_index + 1} header drift: expected {HEADERS[table_index]!r}, found {header!r}"
            )
        if index + 1 >= end:
            raise CoverageError(f"12 §6.3 table {table_index + 1} is truncated")
        sep = split_row(lines[index + 1])
        if len(sep) != 4 or not separator(sep):
            raise CoverageError(f"12 §6.3 table {table_index + 1} separator is malformed")
        cursor = index + 2
        count = 0
        while cursor < end and lines[cursor].startswith("|"):
            cells = split_row(lines[cursor])
            if len(cells) != 4 or any(not cell for cell in cells):
                raise CoverageError(
                    f"12 §6.3 row {cursor + 1} must have four non-empty cells"
                )
            runbooks = RUNBOOK.findall(plain(cells[3]))
            if len(runbooks) != 1:
                raise CoverageError(
                    f"12 §6.3 row {plain(cells[0])!r} must name exactly one RB-* runbook"
                )
            rows.append(
                AlertRow(
                    plain(cells[0]),
                    plain(cells[1]),
                    plain(cells[2]),
                    runbooks[0],
                    "page immediately" in plain(cells[3]).lower(),
                )
            )
            count += 1
            cursor += 1
        if count == 0:
            raise CoverageError(f"12 §6.3 table {table_index + 1} has no data rows")
    if len(rows) != 20:
        raise CoverageError(f"12 §6.3 extracted {len(rows)} rows; frozen O5 inventory requires 20")
    domains = [row.domain for row in rows]
    if len(set(domains)) != len(domains):
        raise CoverageError("12 §6.3 domain names are not unique")
    return rows


def load_rules(directory: Path) -> tuple[list[dict[str, Any]], list[str]]:
    failures: list[str] = []
    rules: list[dict[str, Any]] = []
    paths = sorted(directory.glob("*.yml")) + sorted(directory.glob("*.yaml"))
    if not paths:
        return [], [f"no Prometheus rule files under {directory}"]
    names: set[str] = set()
    for path in paths:
        try:
            document = yaml.safe_load(path.read_text(encoding="utf-8"))
        except (OSError, UnicodeDecodeError, yaml.YAMLError) as error:
            failures.append(f"cannot load rule file {path}: {error}")
            continue
        groups = document.get("groups") if isinstance(document, dict) else None
        if not isinstance(groups, list):
            failures.append(f"rule file {path} must contain a groups list")
            continue
        for group_index, group in enumerate(groups, 1):
            entries = group.get("rules") if isinstance(group, dict) else None
            if not isinstance(group, dict) or not isinstance(group.get("name"), str) or not isinstance(entries, list):
                failures.append(f"{path}: group {group_index} has invalid name/rules")
                continue
            for rule_index, rule in enumerate(entries, 1):
                if not isinstance(rule, dict):
                    failures.append(f"{path}: group {group_index} rule {rule_index} is not an object")
                    continue
                name = rule.get("alert")
                if not isinstance(name, str) or not name:
                    failures.append(f"{path}: group {group_index} rule {rule_index} has no alert name")
                    continue
                if name in names:
                    failures.append(f"alert {name!r} is declared more than once")
                names.add(name)
                if not isinstance(rule.get("expr"), str) or not rule["expr"].strip():
                    failures.append(f"alert {name!r} has no string expr")
                if not isinstance(rule.get("labels"), dict):
                    failures.append(f"alert {name!r} has no labels object")
                if not isinstance(rule.get("annotations"), dict):
                    failures.append(f"alert {name!r} has no annotations object")
                rules.append(rule)
    return rules, failures


def load_inventory(path: Path) -> tuple[dict[str, dict[str, Any]], list[str]]:
    try:
        with path.open("rb") as handle:
            document = tomllib.load(handle)
    except (OSError, tomllib.TOMLDecodeError) as error:
        return {}, [f"cannot load series inventory {path}: {error}"]
    rows = document.get("series")
    if not isinstance(rows, list):
        return {}, ["series inventory must contain [[series]] tables"]
    inventory: dict[str, dict[str, Any]] = {}
    failures: list[str] = []
    allowed = {"name", "source", "owner", "rationale"}
    for index, row in enumerate(rows, 1):
        if not isinstance(row, dict):
            failures.append(f"inventory entry {index} is not a table")
            continue
        unknown = sorted(set(row) - allowed)
        if unknown:
            failures.append(f"inventory entry {index} has unsupported fields: {', '.join(unknown)}")
        name = row.get("name")
        source = row.get("source")
        if not isinstance(name, str) or not name:
            failures.append(f"inventory entry {index} has no metric name")
            continue
        if name in inventory:
            failures.append(f"inventory metric {name!r} is declared more than once")
            continue
        if source not in SOURCES:
            failures.append(f"inventory metric {name!r} has invalid source {source!r}")
        if source == "seam":
            if not isinstance(row.get("owner"), str) or not row["owner"]:
                failures.append(f"seam metric {name!r} has no owner")
            if not isinstance(row.get("rationale"), str) or not row["rationale"]:
                failures.append(f"seam metric {name!r} has no rationale")
        elif "owner" in row or "rationale" in row:
            failures.append(f"non-seam metric {name!r} must not declare owner/rationale")
        inventory[name] = row
    return inventory, failures


def load_milestone_statuses(path: Path) -> tuple[dict[str, str], list[str]]:
    """Strictly parse milestone IDs and statuses from PLAN.md's milestone tables."""
    try:
        lines = path.read_text(encoding="utf-8").splitlines()
    except (OSError, UnicodeDecodeError) as error:
        return {}, [f"cannot load seam owners from {path}: {error}"]
    headings = [index for index, line in enumerate(lines) if line == MILESTONES_HEADING]
    if len(headings) != 1:
        return {}, [
            f"PLAN.md milestone heading drift: expected exactly one {MILESTONES_HEADING!r}"
        ]
    start = headings[0] + 1
    end = next(
        (index for index in range(start, len(lines)) if lines[index].startswith("## ")),
        len(lines),
    )
    if any(re.match(r"^\s+\|", lines[index]) for index in range(start, end)):
        return {}, ["PLAN.md Milestones contains an indented Markdown table row"]

    statuses: dict[str, str] = {}
    failures: list[str] = []
    tables = 0
    index = start
    while index < end:
        if not lines[index].startswith("|"):
            index += 1
            continue
        try:
            header = split_row(lines[index])
        except CoverageError as error:
            failures.append(f"PLAN.md Milestones: {error}")
            index += 1
            continue
        if header != MILESTONE_HEADER:
            failures.append(
                f"PLAN.md Milestones table header drift at line {index + 1}: "
                f"expected {MILESTONE_HEADER!r}, found {header!r}"
            )
            index += 1
            continue
        tables += 1
        if index + 1 >= end:
            failures.append(f"PLAN.md milestone table at line {index + 1} is truncated")
            break
        try:
            sep = split_row(lines[index + 1])
        except CoverageError as error:
            failures.append(f"PLAN.md Milestones: {error}")
            index += 2
            continue
        if len(sep) != 6 or not separator(sep):
            failures.append(
                f"PLAN.md milestone table at line {index + 1} has a malformed separator"
            )
            index += 2
            continue
        index += 2
        row_count = 0
        while index < end and lines[index].startswith("|"):
            try:
                cells = split_row(lines[index])
            except CoverageError as error:
                failures.append(f"PLAN.md Milestones: {error}")
                index += 1
                continue
            if len(cells) != 6:
                failures.append(
                    f"PLAN.md milestone row {index + 1} must have six cells, found {len(cells)}"
                )
                index += 1
                continue
            identifier, status = cells[0], cells[4]
            if re.fullmatch(r"[A-Za-z][A-Za-z0-9]*", identifier) is None:
                failures.append(
                    f"PLAN.md milestone row {index + 1} has invalid ID {identifier!r}"
                )
            elif identifier in statuses:
                failures.append(f"PLAN.md milestone ID {identifier!r} is declared more than once")
            elif status not in MILESTONE_STATUSES:
                failures.append(
                    f"PLAN.md milestone {identifier!r} has invalid status {status!r}"
                )
            else:
                statuses[identifier] = status
            row_count += 1
            index += 1
        if row_count == 0:
            failures.append(f"PLAN.md milestone table at line {index + 1} has no data rows")
    if tables == 0:
        failures.append("PLAN.md Milestones contains no milestone tables")
    return statuses, failures


def module_series(path: Path) -> tuple[set[str], str | None]:
    module_name = "_bleavit_monitoring_" + path.stem
    spec = importlib.util.spec_from_file_location(module_name, path)
    if spec is None or spec.loader is None:
        return set(), f"cannot create import spec for {path}"
    module = importlib.util.module_from_spec(spec)
    try:
        sys.modules[module_name] = module
        spec.loader.exec_module(module)
    except Exception as error:
        return set(), f"cannot import {path}: {error}"
    finally:
        sys.modules.pop(module_name, None)
    series = getattr(module, "SERIES", None)
    if not isinstance(series, dict) or not all(isinstance(name, str) for name in series):
        return set(), f"{path} has no string-keyed SERIES registry"
    return set(series), None


def validate(
    root: Path,
    *,
    exported: Mapping[str, set[str]] | None = None,
) -> tuple[list[str], list[AlertRow], dict[str, dict[str, Any]]]:
    failures: list[str] = []
    try:
        rows = extract_rows(
            (root / "docs" / "architecture" / "12-release-and-operations.md").read_text(encoding="utf-8")
        )
    except (OSError, UnicodeDecodeError, CoverageError) as error:
        return [str(error)], [], {}
    rules, rule_failures = load_rules(
        root / "deploy" / "monitoring" / "prometheus" / "rules"
    )
    failures.extend(rule_failures)
    inventory, inventory_failures = load_inventory(
        root / "tools" / "monitoring" / "series-inventory.toml"
    )
    failures.extend(inventory_failures)
    milestone_statuses, milestone_failures = load_milestone_statuses(root / "PLAN.md")
    failures.extend(milestone_failures)
    for series, entry in sorted(inventory.items()):
        if entry.get("source") != "seam":
            continue
        owner = entry.get("owner")
        if isinstance(owner, str) and owner not in milestone_statuses:
            failures.append(
                f"seam series {series!r} names owner milestone {owner!r}, "
                "but its PLAN.md Milestones row was not found"
            )
        elif isinstance(owner, str) and milestone_statuses.get(owner) == "✅":
            failures.append(
                f"seam series {series!r} names completed owner {owner!r} — the "
                "owning milestone shipped, so the seam must be replaced by its live series"
            )
    by_domain = {row.domain: row for row in rows}
    known_runbooks = {row.runbook for row in rows}
    bound_domains: set[str] = set()
    referenced: set[str] = set()
    for rule in rules:
        name = rule.get("alert", "<unnamed>")
        labels = rule.get("labels", {})
        annotations = rule.get("annotations", {})
        domain = labels.get("domain") if isinstance(labels, dict) else None
        runbook = labels.get("runbook") if isinstance(labels, dict) else None
        if domain not in by_domain:
            failures.append(f"alert {name!r} has undeclared 12 §6.3 domain {domain!r}")
        else:
            row = by_domain[domain]
            bound_domains.add(domain)
            if runbook != row.runbook:
                failures.append(
                    f"alert {name!r} runbook {runbook!r} does not match {domain!r} row {row.runbook!r}"
                )
            if row.page_immediately and labels.get("severity") != "page":
                failures.append(f"page-immediately row {domain!r} alert {name!r} lacks severity: page")
            if isinstance(annotations, dict):
                if annotations.get("key_series") != row.key_series:
                    failures.append(f"alert {name!r} key_series annotation drift for {domain!r}")
                if annotations.get("threshold") != row.threshold:
                    failures.append(f"alert {name!r} threshold annotation drift for {domain!r}")
        if runbook not in known_runbooks:
            failures.append(f"alert {name!r} uses undeclared runbook {runbook!r}")
        expression = rule.get("expr")
        if isinstance(expression, str):
            referenced.update(METRIC_REFERENCE.findall(expression))
    for row in rows:
        if row.domain not in bound_domains:
            failures.append(f"12 §6.3 row {row.domain!r} has no alert rule")
    for name in sorted(referenced - set(inventory)):
        failures.append(f"rule metric {name!r} is missing from series-inventory.toml")
    for name in sorted(set(inventory) - referenced):
        failures.append(f"inventory metric {name!r} is not referenced by any alert rule")

    if exported is None:
        chain, chain_error = module_series(root / "tools" / "monitoring" / "chain_alerts_exporter.py")
        monitor, monitor_error = module_series(root / "tools" / "monitoring" / "attestation_monitor.py")
        if chain_error:
            failures.append(chain_error)
        if monitor_error:
            failures.append(monitor_error)
        exported = {"chain-exporter": chain, "attestation-monitor": monitor}
    for name, entry in inventory.items():
        source = entry.get("source")
        if source in {"chain-exporter", "attestation-monitor"} and name not in exported.get(source, set()):
            failures.append(f"inventory metric {name!r} is not in the {source} SERIES registry")
    return failures, rows, inventory


def print_seams(inventory: Mapping[str, Mapping[str, Any]]) -> None:
    seams = sorted(
        (name, entry.get("owner", "<missing>"), entry.get("rationale", "<missing>"))
        for name, entry in inventory.items()
        if entry.get("source") == "seam"
    )
    print(f"alert coverage: declared seams ({len(seams)}):")
    for name, owner, rationale in seams:
        print(f"  - {name} (owner {owner}): {rationale}")


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Check 12 §6.3 alert and series coverage.")
    parser.add_argument(
        "--root",
        type=Path,
        default=Path(__file__).resolve().parents[2],
        help="repository root (defaults to this checkout)",
    )
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    failures, rows, inventory = validate(args.root.resolve())
    print_seams(inventory)
    if failures:
        for failure in failures:
            print(f"alert coverage: {failure}", file=sys.stderr)
        return 1
    print(
        f"alert coverage: {len(rows)} spec rows, {len(inventory)} rule metrics, all bindings covered"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

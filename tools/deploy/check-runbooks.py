#!/usr/bin/env python3
"""Bind operational runbooks to the normative alert tables in doc 12.

This is the O4 gate: 12 §6.3 is the single source of the alert/runbook
binding. Table cells are compared after Markdown decoration is stripped and
Markdown table escapes are decoded; frontmatter quotes are stripped without
decoding escape sequences. The checker exists so runbooks cannot drift from
that document.
"""

from __future__ import annotations

import argparse
import re
import string
import sys
from collections import Counter
from dataclasses import dataclass
from pathlib import Path
from urllib.parse import unquote


ROOT = Path(__file__).resolve().parents[2]
DEFAULT_DOC = Path("docs/architecture/12-release-and-operations.md")
DEFAULT_RUNBOOKS_DIR = Path("deploy/runbooks")
FROZEN_RUNBOOK_IDS = frozenset(
    {
        "RB-KEEPER",
        "RB-INTAKE",
        "RB-MARKET",
        "RB-POL",
        "RB-ORACLE",
        "RB-LEDGER",
        "RB-TREASURY",
        "RB-XCM",
        "RB-GUARDIAN",
        "RB-UPGRADE",
        "RB-STORAGE",
        "RB-BOOTNODE",
        "RB-RELEASE",
    }
)
REQUIRED_KEYS = (
    "id",
    "title",
    "owner_role",
    "funding_line",
    "page_immediately",
    "alerts",
    "spec_refs",
)
SCALAR_KEYS = {"id", "title", "owner_role", "funding_line", "page_immediately"}
REQUIRED_SECTIONS = (
    "Purpose",
    "Alerts",
    "Diagnosis",
    "Remediation",
    "Escalation",
    "References",
)
RUNBOOK_CELL_RE = re.compile(r"^(RB-[A-Z]+)(?: \(([^()]+)\))?$")
RUNBOOK_FILENAME_RE = re.compile(r"^RB-[A-Z]+\.md$")
RUNBOOK_DISCOVERY_RE = re.compile(r"^rb-.*\.md$", re.IGNORECASE)
TOP_LEVEL_RE = re.compile(r"^([a-z_]+):(?: (.*))?$")
INLINE_LINK_RE = re.compile(r"(?<!!)\[([^\]]+)\]\([^)]*\)")
BODY_LINK_RE = re.compile(r"!?\[[^\]]*\]\(([^)]+)\)")
SEPARATOR_RE = re.compile(r"^:?-{3,}:?$")
MARKDOWN_ESCAPABLE = frozenset(string.punctuation)


@dataclass(frozen=True)
class AlertBinding:
    domain: str
    trigger: str
    line: int
    key_series: str | None = None
    runbook_id: str | None = None
    page_immediately: bool = False


@dataclass
class Runbook:
    path: Path
    values: dict[str, object]
    key_lines: dict[str, int]
    alerts: list[AlertBinding]
    alert_lines: list[int]
    spec_refs: list[tuple[str, int]]
    body_start: int
    body_lines: list[str]


def display_path(path: Path, root: Path) -> str:
    try:
        return str(path.resolve().relative_to(root.resolve()))
    except ValueError:
        return str(path)


def strip_markdown(value: str) -> str:
    """Remove cell-level Markdown decoration without rewriting its text."""
    previous = ""
    while previous != value:
        previous = value
        value = INLINE_LINK_RE.sub(r"\1", value)
    value = re.sub(r"\*\*([^*]+)\*\*", r"\1", value)
    value = re.sub(r"__([^_]+)__", r"\1", value)
    value = re.sub(r"(?<!\*)\*([^*]+)\*(?!\*)", r"\1", value)
    value = re.sub(r"(?<!\w)_([^_]+)_(?!\w)", r"\1", value)
    return value.replace("`", "").strip()


def section_bounds(
    lines: list[str], section: str, label: str, errors: list[str]
) -> tuple[int, int]:
    pattern = re.compile(rf"^### {re.escape(section)}(?:\s|$)")
    starts = [index for index, line in enumerate(lines) if pattern.match(line)]
    if len(starts) != 1:
        errors.append(
            f"{label}:1: found {len(starts)} headings for §{section}, expected exactly 1"
        )
        return 0, 0
    start = starts[0] + 1
    end = next(
        (
            index
            for index in range(start, len(lines))
            if lines[index].startswith("### ")
        ),
        len(lines),
    )
    return start, end


def split_table_row(line: str) -> list[str]:
    stripped = line.strip()
    if not stripped.startswith("|"):
        return []
    cells: list[str] = []
    current: list[str] = []
    escaped = False
    for character in stripped:
        if escaped:
            if character in MARKDOWN_ESCAPABLE:
                current.append(character)
            else:
                current.extend(("\\", character))
            escaped = False
        elif character == "\\":
            escaped = True
        elif character == "|":
            cells.append("".join(current).strip())
            current = []
        else:
            current.append(character)
    if escaped:
        current.append("\\")
    cells.append("".join(current).strip())
    if cells and cells[0] == "":
        cells = cells[1:]
    if cells and cells[-1] == "":
        cells = cells[:-1]
    return cells


def is_separator(cells: list[str]) -> bool:
    return bool(cells) and all(SEPARATOR_RE.fullmatch(cell) for cell in cells)


def parse_alert_tables(
    doc: Path, root: Path, errors: list[str]
) -> list[AlertBinding]:
    label = display_path(doc, root)
    try:
        lines = doc.read_text(encoding="utf-8").splitlines()
    except OSError as error:
        errors.append(f"{label}:1: cannot read doc 12: {error}")
        return []

    incident_headings = [
        index + 1
        for index, line in enumerate(lines)
        if line.strip() == "### 6.4 Incident response"
    ]
    if not incident_headings:
        errors.append(
            f"{label}:1: missing required heading '### 6.4 Incident response'"
        )
    elif len(incident_headings) > 1:
        errors.append(
            f"{label}:{incident_headings[1]}: duplicate required heading '### 6.4 Incident response'"
        )

    wanted_headers = {
        ("Domain", "Key series", "Alert (example)", "Runbook"),
        ("Domain", "Key series", "Alert", "Runbook"),
    }
    bindings: list[AlertBinding] = []
    table_count = 0
    start, end = section_bounds(lines, "6.3", label, errors)
    for index in range(start, end):
        line = lines[index]
        cells = split_table_row(line)
        header = tuple(strip_markdown(cell) for cell in cells)
        if header not in wanted_headers:
            continue
        header_line = index + 1
        if index + 1 >= len(lines) or not is_separator(
            split_table_row(lines[index + 1])
        ):
            errors.append(
                f"{label}:{header_line}: alert table header must be followed by a four-cell separator row"
            )
            continue
        separator_cells = split_table_row(lines[index + 1])
        if len(separator_cells) != 4:
            errors.append(
                f"{label}:{header_line + 1}: alert table separator has {len(separator_cells)} cells, expected 4"
            )
            continue
        table_count += 1
        row_index = index + 2
        row_count = 0
        while row_index < len(lines) and lines[row_index].strip().startswith("|"):
            row_line = row_index + 1
            row_cells = split_table_row(lines[row_index])
            if is_separator(row_cells):
                errors.append(
                    f"{label}:{row_line}: unexpected separator inside alert table body"
                )
                row_index += 1
                continue
            if len(row_cells) != 4:
                errors.append(
                    f"{label}:{row_line}: unparsable alert row has {len(row_cells)} cells, expected 4"
                )
                row_index += 1
                continue
            domain, key_series, trigger, runbook_cell = (
                strip_markdown(cell) for cell in row_cells
            )
            match = RUNBOOK_CELL_RE.fullmatch(runbook_cell)
            if not domain or not key_series or not trigger or match is None:
                errors.append(
                    f"{label}:{row_line}: unparsable alert row; domain, key series, and trigger are required, and the runbook cell must be exactly RB-[A-Z]+ optionally followed by one parenthesized annotation"
                )
                row_index += 1
                continue
            bindings.append(
                AlertBinding(
                    domain=domain,
                    trigger=trigger,
                    line=row_line,
                    key_series=key_series,
                    runbook_id=match.group(1),
                    page_immediately=match.group(2) == "page immediately",
                )
            )
            row_count += 1
            row_index += 1
        if row_count == 0:
            errors.append(f"{label}:{header_line}: alert table has no data rows")

    if table_count != 2:
        errors.append(
            f"{label}:1: found {table_count} parseable §6.3 alert tables, expected exactly 2"
        )
    return bindings


def parse_ops_table(
    doc: Path, root: Path, errors: list[str]
) -> set[tuple[str, str]]:
    label = display_path(doc, root)
    try:
        lines = doc.read_text(encoding="utf-8").splitlines()
    except OSError:
        return set()

    owner_funding_pairs: set[tuple[str, str]] = set()
    table_count = 0
    start, end = section_bounds(lines, "6.1", label, errors)
    for index in range(start, end):
        line = lines[index]
        header = [strip_markdown(cell) for cell in split_table_row(line)]
        if not (
            len(header) == 4
            and header[:3] == ["Service", "Commitment (MUST)", "Owner role"]
            and header[3].startswith("Funding line")
        ):
            continue
        table_count += 1
        header_line = index + 1
        if index + 1 >= len(lines) or not is_separator(
            split_table_row(lines[index + 1])
        ):
            errors.append(
                f"{label}:{header_line}: §6.1 ops table header must be followed by a separator row"
            )
            continue
        row_index = index + 2
        while row_index < len(lines) and lines[row_index].strip().startswith("|"):
            row_line = row_index + 1
            cells = split_table_row(lines[row_index])
            if len(cells) != 4 or is_separator(cells):
                errors.append(
                    f"{label}:{row_line}: unparsable §6.1 ops row; expected 4 data cells"
                )
                row_index += 1
                continue
            owner = strip_markdown(cells[2])
            funding = strip_markdown(cells[3])
            if not owner or not funding:
                errors.append(
                    f"{label}:{row_line}: §6.1 owner role and funding line must be non-empty"
                )
            else:
                owner_funding_pairs.add((owner, funding))
            row_index += 1
    if table_count != 1:
        errors.append(
            f"{label}:1: found {table_count} parseable §6.1 owned-and-funded tables, expected 1"
        )
    return owner_funding_pairs


def parse_scalar(raw: str, label: str, line: int, errors: list[str]) -> str | None:
    if raw != raw.strip() or not raw:
        errors.append(f"{label}:{line}: scalar must be non-empty with no edge whitespace")
        return None
    if raw[0] in "'[{|>":
        errors.append(
            f"{label}:{line}: unsupported frontmatter syntax; use a plain or double-quoted single-line scalar"
        )
        return None
    if raw.startswith('"'):
        if not raw.endswith('"') or len(raw) < 2:
            errors.append(f"{label}:{line}: unmatched double quote")
            return None
        value = raw[1:-1]
        if '"' in value or any(character in value for character in "\r\n\t"):
            errors.append(
                f"{label}:{line}: double-quoted scalar must be one literal line with no embedded quote"
            )
            return None
        if not value:
            errors.append(f"{label}:{line}: scalar must be non-empty")
            return None
        return value
    if re.search(r"(^|\s)#", raw):
        errors.append(
            f"{label}:{line}: YAML comments are not supported in scalar positions"
        )
        return None
    if raw.startswith("!"):
        errors.append(
            f"{label}:{line}: YAML tags are not supported in scalar positions"
        )
        return None
    if raw.startswith(("&", "*")):
        errors.append(f"{label}:{line}: YAML anchors and aliases are not supported")
        return None
    if '"' in raw:
        errors.append(f"{label}:{line}: unmatched double quote in plain scalar")
        return None
    return raw


def parse_runbook(path: Path, root: Path, errors: list[str]) -> Runbook | None:
    label = display_path(path, root)
    try:
        lines = path.read_text(encoding="utf-8").splitlines()
    except OSError as error:
        errors.append(f"{label}:1: cannot read runbook: {error}")
        return None
    if not lines or lines[0] != "---":
        errors.append(f"{label}:1: runbook must start with --- frontmatter")
        return None
    try:
        closing = lines.index("---", 1)
    except ValueError:
        errors.append(f"{label}:1: frontmatter has no closing --- delimiter")
        return None
    frontmatter = lines[1:closing]
    if any("\t" in line for line in frontmatter):
        for number, line in enumerate(frontmatter, start=2):
            if "\t" in line:
                errors.append(f"{label}:{number}: tabs are forbidden in frontmatter")
        return None

    values: dict[str, object] = {}
    key_lines: dict[str, int] = {}
    alerts: list[AlertBinding] = []
    alert_lines: list[int] = []
    spec_refs: list[tuple[str, int]] = []
    encountered: list[str] = []
    index = 0
    syntax_failed = False
    while index < len(frontmatter):
        line = frontmatter[index]
        line_number = index + 2
        match = TOP_LEVEL_RE.fullmatch(line)
        if match is None:
            errors.append(
                f"{label}:{line_number}: expected an unindented top-level key"
            )
            syntax_failed = True
            index += 1
            continue
        key, raw_value = match.groups()
        if key not in REQUIRED_KEYS:
            errors.append(f"{label}:{line_number}: unknown top-level key {key!r}")
            syntax_failed = True
            index += 1
            continue
        if key in values:
            errors.append(f"{label}:{line_number}: duplicate top-level key {key!r}")
            syntax_failed = True
            index += 1
            continue
        encountered.append(key)
        key_lines[key] = line_number

        if key in SCALAR_KEYS:
            if raw_value is None:
                errors.append(f"{label}:{line_number}: {key} requires a scalar value")
                syntax_failed = True
            elif key == "page_immediately" and raw_value not in {"true", "false"}:
                errors.append(
                    f"{label}:{line_number}: page_immediately must be literal unquoted true or false"
                )
                syntax_failed = True
            else:
                scalar = parse_scalar(raw_value, label, line_number, errors)
                if scalar is None:
                    syntax_failed = True
                elif key == "page_immediately":
                    values[key] = scalar == "true"
                else:
                    values[key] = scalar
            index += 1
            continue

        if raw_value is not None:
            errors.append(
                f"{label}:{line_number}: {key} must be a block list; flow syntax is forbidden"
            )
            syntax_failed = True
            index += 1
            continue
        values[key] = []
        index += 1
        if key == "alerts":
            while index < len(frontmatter) and frontmatter[index].startswith(" "):
                domain_line = index + 2
                domain_match = re.fullmatch(r"  - domain: (.+)", frontmatter[index])
                if domain_match is None:
                    errors.append(
                        f"{label}:{domain_line}: alert entry must start with exactly '  - domain: <scalar>'"
                    )
                    syntax_failed = True
                    index += 1
                    continue
                domain = parse_scalar(
                    domain_match.group(1), label, domain_line, errors
                )
                index += 1
                if index >= len(frontmatter):
                    errors.append(
                        f"{label}:{domain_line}: alert entry is missing its trigger key"
                    )
                    syntax_failed = True
                    break
                trigger_line = index + 2
                trigger_match = re.fullmatch(
                    r"    trigger: (.+)", frontmatter[index]
                )
                if trigger_match is None:
                    errors.append(
                        f"{label}:{trigger_line}: alert entry requires exactly domain and trigger"
                    )
                    syntax_failed = True
                    continue
                trigger = parse_scalar(
                    trigger_match.group(1), label, trigger_line, errors
                )
                index += 1
                if domain is None or trigger is None:
                    syntax_failed = True
                else:
                    alerts.append(
                        AlertBinding(domain=domain, trigger=trigger, line=domain_line)
                    )
                    alert_lines.append(domain_line)
            values[key] = alerts
        else:
            while index < len(frontmatter) and frontmatter[index].startswith(" "):
                item_line = index + 2
                item_match = re.fullmatch(r"  - (.+)", frontmatter[index])
                if item_match is None:
                    errors.append(
                        f"{label}:{item_line}: spec_refs entries require exactly two-space block-list indentation"
                    )
                    syntax_failed = True
                    index += 1
                    continue
                item = parse_scalar(item_match.group(1), label, item_line, errors)
                if item is None:
                    syntax_failed = True
                else:
                    spec_refs.append((item, item_line))
                index += 1
            values[key] = [item for item, _line in spec_refs]

    missing = [key for key in REQUIRED_KEYS if key not in values]
    for key in missing:
        errors.append(f"{label}:1: missing required top-level key {key!r}")
    if not missing and encountered != list(REQUIRED_KEYS):
        errors.append(
            f"{label}:2: top-level keys must appear in frozen order: {', '.join(REQUIRED_KEYS)}"
        )
        syntax_failed = True
    if not alerts:
        errors.append(f"{label}:{key_lines.get('alerts', 1)}: alerts must not be empty")
        syntax_failed = True
    if not spec_refs:
        errors.append(
            f"{label}:{key_lines.get('spec_refs', 1)}: spec_refs must not be empty"
        )
        syntax_failed = True
    if syntax_failed or missing:
        return None
    return Runbook(
        path=path,
        values=values,
        key_lines=key_lines,
        alerts=alerts,
        alert_lines=alert_lines,
        spec_refs=spec_refs,
        body_start=closing + 2,
        body_lines=lines[closing + 1 :],
    )


def github_markdown_anchors(path: Path) -> set[str]:
    """Return GitHub-style anchors for ATX headings in a Markdown file."""
    anchors: set[str] = set()
    occurrences: Counter[str] = Counter()
    fence: str | None = None
    for line in path.read_text(encoding="utf-8").splitlines():
        fence_match = re.match(r"^\s*(`{3,}|~{3,})", line)
        if fence_match:
            marker = fence_match.group(1)[0]
            if fence is None:
                fence = marker
            elif fence == marker:
                fence = None
            continue
        if fence is not None:
            continue
        heading_match = re.match(r"^ {0,3}#{1,6}\s+(.+?)\s*#*\s*$", line)
        if heading_match is None:
            continue
        heading = strip_markdown(heading_match.group(1))
        heading = re.sub(r"<[^>]+>", "", heading)
        base = re.sub(r"[^\w\s-]", "", heading.casefold())
        base = re.sub(r"\s", "-", base.strip())
        duplicate = occurrences[base]
        occurrences[base] += 1
        anchor = base if duplicate == 0 else f"{base}-{duplicate}"
        anchors.add(anchor)
    return anchors


def resolve_repo_path(
    raw: str,
    base: Path,
    root: Path,
    label: str,
    line: int,
    errors: list[str],
    source_file: Path | None = None,
) -> None:
    target = raw.strip()
    if target.startswith("<") and target.endswith(">"):
        target = target[1:-1]
    elif " " in target:
        target = target.split(" ", 1)[0]
    if target.startswith(("http://", "https://", "mailto:", "data:")):
        return
    path_text, separator, fragment = target.partition("#")
    path_text = unquote(path_text)
    fragment = unquote(fragment)
    if not path_text:
        if separator and source_file is not None:
            candidate = source_file
        else:
            return
    else:
        candidate = (
            (root / path_text.lstrip("/"))
            if path_text.startswith("/")
            else (base / path_text)
        )
    resolved = candidate.resolve()
    try:
        resolved.relative_to(root.resolve())
    except ValueError:
        errors.append(f"{label}:{line}: link/path escapes repository: {raw}")
        return
    if not resolved.is_file():
        errors.append(f"{label}:{line}: link/path does not resolve to a file: {raw}")
        return
    if separator and resolved.suffix.casefold() == ".md":
        anchors = github_markdown_anchors(resolved)
        if fragment not in anchors:
            errors.append(
                f"{label}:{line}: fragment '#{fragment}' does not match a heading in {display_path(resolved, root)}"
            )


def parse_body_alert_table(runbook: Runbook, root: Path, errors: list[str]) -> list[AlertBinding]:
    label = display_path(runbook.path, root)
    starts = [
        offset
        for offset, line in enumerate(runbook.body_lines)
        if line.strip() == "## Alerts"
    ]
    if len(starts) != 1:
        return []
    start = starts[0] + 1
    end = next(
        (
            offset
            for offset in range(start, len(runbook.body_lines))
            if runbook.body_lines[offset].startswith("## ")
        ),
        len(runbook.body_lines),
    )
    header_offsets = [
        offset
        for offset in range(start, end)
        if tuple(strip_markdown(cell) for cell in split_table_row(runbook.body_lines[offset]))
        == ("Domain", "Key series", "Trigger")
    ]
    if len(header_offsets) != 1:
        errors.append(
            f"{label}:{runbook.body_start + starts[0]}: found {len(header_offsets)} parseable tables in ## Alerts, expected exactly 1"
        )
        return []
    header = header_offsets[0]
    header_line = runbook.body_start + header
    if header + 1 >= end:
        errors.append(
            f"{label}:{header_line}: body Alerts table header must be followed by a three-cell separator row"
        )
        return []
    separator = split_table_row(runbook.body_lines[header + 1])
    if len(separator) != 3 or not is_separator(separator):
        errors.append(
            f"{label}:{header_line + 1}: body Alerts table requires a three-cell separator row"
        )
        return []

    alerts: list[AlertBinding] = []
    row_offset = header + 2
    while row_offset < end and runbook.body_lines[row_offset].strip().startswith("|"):
        row_line = runbook.body_start + row_offset
        cells = split_table_row(runbook.body_lines[row_offset])
        if len(cells) != 3 or is_separator(cells):
            errors.append(
                f"{label}:{row_line}: unparsable body Alerts row; expected 3 data cells"
            )
            row_offset += 1
            continue
        domain, key_series, trigger = (strip_markdown(cell) for cell in cells)
        if not domain or not key_series or not trigger:
            errors.append(
                f"{label}:{row_line}: body Alerts cells must all be non-empty"
            )
        else:
            alerts.append(
                AlertBinding(
                    domain=domain,
                    trigger=trigger,
                    key_series=key_series,
                    line=row_line,
                )
            )
        row_offset += 1
    if not alerts:
        errors.append(f"{label}:{header_line}: body Alerts table has no data rows")
    return alerts


def validate_runbook_body(runbook: Runbook, root: Path, errors: list[str]) -> None:
    label = display_path(runbook.path, root)
    headings: list[tuple[str, int]] = []
    for offset, line in enumerate(runbook.body_lines):
        if line.startswith("## "):
            headings.append((line[3:].strip(), runbook.body_start + offset))
    positions: list[int] = []
    for section in REQUIRED_SECTIONS:
        matches = [line for heading, line in headings if heading == section]
        if not matches:
            errors.append(f"{label}:{runbook.body_start}: missing required section '## {section}'")
        elif len(matches) > 1:
            errors.append(f"{label}:{matches[1]}: duplicate required section '## {section}'")
            positions.append(matches[0])
        else:
            positions.append(matches[0])
    if len(positions) == len(REQUIRED_SECTIONS) and positions != sorted(positions):
        errors.append(
            f"{label}:{positions[0]}: required ## sections are not in the frozen order"
        )

    for reference, line in runbook.spec_refs:
        resolve_repo_path(
            reference, root, root, label, line, errors, source_file=runbook.path
        )
    for offset, body_line in enumerate(runbook.body_lines):
        line_number = runbook.body_start + offset
        for match in BODY_LINK_RE.finditer(body_line):
            resolve_repo_path(
                match.group(1),
                runbook.path.parent,
                root,
                label,
                line_number,
                errors,
                source_file=runbook.path,
            )


def validate_readme(
    readme: Path,
    expected_ids: set[str],
    runbooks_by_id: dict[str, Runbook],
    root: Path,
    errors: list[str],
) -> None:
    label = display_path(readme, root)
    try:
        lines = readme.read_text(encoding="utf-8").splitlines()
    except OSError as error:
        errors.append(f"{label}:1: cannot read runbook index: {error}")
        return
    header_offsets = [
        offset
        for offset, line in enumerate(lines)
        if tuple(strip_markdown(cell) for cell in split_table_row(line))
        == ("ID", "Title", "owner_role", "page_immediately")
    ]
    if len(header_offsets) != 1:
        errors.append(
            f"{label}:1: found {len(header_offsets)} runbook index tables, expected exactly 1"
        )
        return
    header = header_offsets[0]
    if header + 1 >= len(lines):
        errors.append(f"{label}:{header + 1}: runbook index has no separator row")
        return
    separator = split_table_row(lines[header + 1])
    if len(separator) != 4 or not is_separator(separator):
        errors.append(
            f"{label}:{header + 2}: runbook index requires a four-cell separator row"
        )
        return

    entries: list[tuple[str, str, str, str, int]] = []
    row_offset = header + 2
    while row_offset < len(lines) and lines[row_offset].strip().startswith("|"):
        line_number = row_offset + 1
        cells = split_table_row(lines[row_offset])
        if len(cells) != 4 or is_separator(cells):
            errors.append(
                f"{label}:{line_number}: unparsable runbook index row; expected 4 data cells"
            )
            row_offset += 1
            continue
        runbook_id, title, owner, page = (strip_markdown(cell) for cell in cells)
        if not re.fullmatch(r"RB-[A-Z]+", runbook_id):
            errors.append(
                f"{label}:{line_number}: invalid runbook id {runbook_id!r} in index"
            )
        else:
            entries.append((runbook_id, title, owner, page, line_number))
        row_offset += 1

    counts = Counter(entry[0] for entry in entries)
    for runbook_id, count in sorted(counts.items()):
        if count > 1:
            duplicate_line = next(
                entry[4]
                for position, entry in enumerate(entries)
                if entry[0] == runbook_id
                and any(previous[0] == runbook_id for previous in entries[:position])
            )
            errors.append(
                f"{label}:{duplicate_line}: duplicate runbook id {runbook_id} in index"
            )
    listed = set(counts)
    for runbook_id in sorted(expected_ids - listed):
        errors.append(f"{label}:1: runbook index is missing {runbook_id}")
    for runbook_id in sorted(listed - expected_ids):
        line_number = next(entry[4] for entry in entries if entry[0] == runbook_id)
        errors.append(
            f"{label}:{line_number}: unexpected runbook id {runbook_id} in index"
        )

    for runbook_id, title, owner, page, line_number in entries:
        runbook = runbooks_by_id.get(runbook_id)
        if runbook is None:
            continue
        expected_values = {
            "Title": runbook.values["title"],
            "owner_role": runbook.values["owner_role"],
            "page_immediately": str(runbook.values["page_immediately"]).lower(),
        }
        actual_values = {
            "Title": title,
            "owner_role": owner,
            "page_immediately": page,
        }
        for field, expected in expected_values.items():
            if actual_values[field] != expected:
                errors.append(
                    f"{label}:{line_number}: index {field} does not match {runbook_id} frontmatter: expected {expected!r}, found {actual_values[field]!r}"
                )


def validate_frozen_runbook_ids(
    expected_ids: set[str], label: str, errors: list[str]
) -> None:
    missing = sorted(FROZEN_RUNBOOK_IDS - expected_ids)
    unexpected = sorted(expected_ids - FROZEN_RUNBOOK_IDS)
    if missing or unexpected:
        details: list[str] = []
        if missing:
            details.append(f"missing {', '.join(missing)}")
        if unexpected:
            details.append(f"unexpected {', '.join(unexpected)}")
        errors.append(
            f"{label}:1: §6.3 runbook IDs must equal the frozen 13-ID O4 set ({'; '.join(details)})"
        )


def check_repository(
    root: Path,
    doc: Path | None = None,
    runbooks_dir: Path | None = None,
) -> tuple[list[str], int, int]:
    root = root.resolve()
    doc_path = doc or DEFAULT_DOC
    if not doc_path.is_absolute():
        doc_path = root / doc_path
    directory = runbooks_dir or DEFAULT_RUNBOOKS_DIR
    if not directory.is_absolute():
        directory = root / directory

    errors: list[str] = []
    bindings = parse_alert_tables(doc_path, root, errors)
    owner_funding_pairs = parse_ops_table(doc_path, root, errors)
    expected_by_id: dict[str, list[AlertBinding]] = {}
    page_by_id: dict[str, bool] = {}
    doc_rows: dict[tuple[str, str], list[AlertBinding]] = {}
    for binding in bindings:
        runbook_id = binding.runbook_id
        assert runbook_id is not None
        expected_by_id.setdefault(runbook_id, []).append(binding)
        doc_rows.setdefault((binding.domain, binding.trigger), []).append(binding)
        page_by_id[runbook_id] = (
            page_by_id.get(runbook_id, False) or binding.page_immediately
        )
    for (domain, trigger), row_bindings in sorted(doc_rows.items()):
        if len(row_bindings) != 1:
            owners = sorted(
                {binding.runbook_id for binding in row_bindings if binding.runbook_id}
            )
            errors.append(
                f"{display_path(doc_path, root)}:{row_bindings[1].line}: §6.3 alert row {domain!r} / {trigger!r} must bind exactly once, found {len(row_bindings)} rows across {', '.join(owners)}"
            )
    expected_ids = set(expected_by_id)
    if doc_path.resolve() == (root / DEFAULT_DOC).resolve():
        validate_frozen_runbook_ids(
            expected_ids, display_path(doc_path, root), errors
        )

    files = (
        sorted(
            (
                path
                for path in directory.iterdir()
                if path.is_file() and RUNBOOK_DISCOVERY_RE.fullmatch(path.name)
            ),
            key=lambda path: path.name.casefold(),
        )
        if directory.is_dir()
        else []
    )
    files_by_id: dict[str, Runbook] = {}
    for path in files:
        filename_label = display_path(path, root)
        if RUNBOOK_FILENAME_RE.fullmatch(path.name) is None:
            errors.append(
                f"{filename_label}:1: filename must match RB-[A-Z]+.md exactly"
            )
        runbook = parse_runbook(path, root, errors)
        if runbook is None:
            continue
        runbook_id = runbook.values["id"]
        assert isinstance(runbook_id, str)
        id_line = runbook.key_lines["id"]
        label = display_path(path, root)
        if not re.fullmatch(r"RB-[A-Z]+", runbook_id):
            errors.append(f"{label}:{id_line}: id must match RB-[A-Z]+")
        if path.name != f"{runbook_id}.md":
            errors.append(
                f"{label}:{id_line}: filename must equal id ({runbook_id}.md)"
            )
        if runbook_id in files_by_id:
            errors.append(f"{label}:{id_line}: duplicate runbook id {runbook_id}")
        else:
            files_by_id[runbook_id] = runbook
        if runbook_id not in expected_ids:
            errors.append(f"{label}:{id_line}: orphan runbook id {runbook_id} is not referenced by doc 12 §6.3")

        owner = runbook.values["owner_role"]
        funding = runbook.values["funding_line"]
        if (owner, funding) not in owner_funding_pairs:
            errors.append(
                f"{label}:{runbook.key_lines['owner_role']}: owner_role/funding_line pair {owner!r} / {funding!r} does not appear together in one §6.1 row"
            )
        validate_runbook_body(runbook, root, errors)

    existing_names = {path.name for path in files}
    for runbook_id in sorted(expected_ids):
        if f"{runbook_id}.md" in existing_names:
            continue
        errors.append(
            f"{display_path(directory / f'{runbook_id}.md', root)}:1: missing runbook file referenced by doc 12 §6.3"
        )

    frontmatter_rows: dict[tuple[str, str], list[tuple[str, int, str]]] = {}
    for runbook_id, runbook in files_by_id.items():
        for binding in runbook.alerts:
            frontmatter_rows.setdefault((binding.domain, binding.trigger), []).append(
                (runbook_id, binding.line, display_path(runbook.path, root))
            )
    for (domain, trigger), occurrences in sorted(frontmatter_rows.items()):
        if len(occurrences) > 1:
            owners = sorted({runbook_id for runbook_id, _line, _path in occurrences})
            _owner, line, label = occurrences[1]
            errors.append(
                f"{label}:{line}: alert binding appears in multiple runbooks or more than once: {domain!r} / {trigger!r} ({', '.join(owners)})"
            )

    for runbook_id in sorted(expected_ids & set(files_by_id)):
        runbook = files_by_id[runbook_id]
        label = display_path(runbook.path, root)
        expected = Counter(
            (binding.domain, binding.trigger) for binding in expected_by_id[runbook_id]
        )
        actual = Counter((binding.domain, binding.trigger) for binding in runbook.alerts)
        for (domain, trigger), count in sorted((expected - actual).items()):
            for _ in range(count):
                errors.append(
                    f"{label}:{runbook.key_lines['alerts']}: missing alert binding from doc 12 §6.3: {domain!r} / {trigger!r}"
                )
        for (domain, trigger), count in sorted((actual - expected).items()):
            for _ in range(count):
                line = next(
                    binding.line
                    for binding in runbook.alerts
                    if binding.domain == domain and binding.trigger == trigger
                )
                errors.append(
                    f"{label}:{line}: alert binding is not exact after Markdown stripping with doc 12 §6.3: {domain!r} / {trigger!r}"
                )
        expected_body = Counter(
            (binding.domain, binding.key_series, binding.trigger)
            for binding in expected_by_id[runbook_id]
        )
        body_alerts = parse_body_alert_table(runbook, root, errors)
        actual_body = Counter(
            (binding.domain, binding.key_series, binding.trigger)
            for binding in body_alerts
        )
        for (domain, key_series, trigger), count in sorted(
            (expected_body - actual_body).items()
        ):
            for _ in range(count):
                errors.append(
                    f"{label}:{runbook.body_start}: body Alerts table is missing doc 12 §6.3 row: {domain!r} / {key_series!r} / {trigger!r}"
                )
        for (domain, key_series, trigger), count in sorted(
            (actual_body - expected_body).items()
        ):
            for _ in range(count):
                line = next(
                    binding.line
                    for binding in body_alerts
                    if binding.domain == domain
                    and binding.key_series == key_series
                    and binding.trigger == trigger
                )
                errors.append(
                    f"{label}:{line}: body Alerts row is not exact after Markdown stripping with doc 12 §6.3: {domain!r} / {key_series!r} / {trigger!r}"
                )
        expected_page = page_by_id[runbook_id]
        if runbook.values["page_immediately"] != expected_page:
            errors.append(
                f"{label}:{runbook.key_lines['page_immediately']}: page_immediately must be {str(expected_page).lower()} from doc 12 §6.3"
            )

    validate_readme(
        directory / "README.md", expected_ids, files_by_id, root, errors
    )
    return errors, len(expected_ids), len(bindings)


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Check runbooks against doc 12 §6.1 and §6.3."
    )
    parser.add_argument(
        "--root",
        type=Path,
        default=ROOT,
        help="repository root (default: inferred from this script)",
    )
    parser.add_argument(
        "--doc",
        type=Path,
        help="doc-12 path override, relative to --root unless absolute",
    )
    parser.add_argument(
        "--runbooks-dir",
        type=Path,
        help="runbooks directory override, relative to --root unless absolute",
    )
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(sys.argv[1:] if argv is None else argv)
    errors, runbook_count, alert_count = check_repository(
        args.root, args.doc, args.runbooks_dir
    )
    if errors:
        for error in errors:
            print(error, file=sys.stderr)
        return 1
    print(f"OK ({runbook_count} runbooks, {alert_count} alert rows bound)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

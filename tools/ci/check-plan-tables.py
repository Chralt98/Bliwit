#!/usr/bin/env python3
"""Check PLAN.md Markdown table structure.

Guards against the table-drift class fixed on 2026-07-17: a blank line (or any
non-table line) splitting a table strands the rows below it from the header, so
they render as raw pipe-text instead of table rows. GFM only renders a pipe block
as a table when it opens with a header row followed by a separator row and every
row carries a consistent cell count.

Rules, per contiguous block of `|`-prefixed lines (outside fenced code blocks):
  1. The block's second line must be a separator row (`|---|...|`) — a block
     without one is an orphaned body (the B10/B11 failure mode).
  2. The first line must NOT be a separator (header missing).
  3. Every row must have the same cell count as the header. Only `\\|` escapes a
     pipe — GFM splits cells on unescaped pipes even inside backtick code spans.
  4. Separator rows must not appear anywhere except line 2.

Standing user instruction (2026-07-17): PLAN.md table formatting must never
drift/break. Enforced by the `guard-plan-tables.sh` Stop hook and the docs CI job.
"""
from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SEPARATOR_CELL_RE = re.compile(r"^:?-+:?$")


def split_cells(line: str) -> list[str]:
    """Split a table row into cells exactly as GFM does: every unescaped `|`
    delimits — backtick code spans do NOT protect pipes in table rows, only
    `\\|` does; the leading/trailing delimiters contribute no cells."""
    cells: list[str] = []
    current: list[str] = []
    escaped = False
    for ch in line:
        if escaped:
            current.append(ch)
            escaped = False
        elif ch == "\\":
            current.append(ch)
            escaped = True
        elif ch == "|":
            cells.append("".join(current).strip())
            current = []
        else:
            current.append(ch)
    cells.append("".join(current).strip())
    # A well-formed row is `| a | b |`: drop the empty fragments outside the
    # outer delimiters so the count is the real cell count.
    if cells and cells[0] == "":
        cells = cells[1:]
    if cells and cells[-1] == "":
        cells = cells[:-1]
    return cells


def is_separator_row(line: str) -> bool:
    cells = split_cells(line)
    return bool(cells) and all(SEPARATOR_CELL_RE.match(c) for c in cells)


def check_text(text: str, name: str) -> list[str]:
    errors: list[str] = []
    blocks: list[list[tuple[int, str]]] = []
    current_block: list[tuple[int, str]] = []
    in_fence = False
    for lineno, line in enumerate(text.splitlines(), start=1):
        stripped = line.strip()
        if stripped.startswith("```") or stripped.startswith("~~~"):
            in_fence = not in_fence
        if not in_fence and stripped.startswith("|"):
            current_block.append((lineno, stripped))
        else:
            if current_block:
                blocks.append(current_block)
                current_block = []
    if current_block:
        blocks.append(current_block)

    for block in blocks:
        first_no, first = block[0]
        if is_separator_row(first):
            errors.append(
                f"{name}:{first_no}: table starts with a separator row — header row missing"
            )
            continue
        if len(block) < 2 or not is_separator_row(block[1][1]):
            errors.append(
                f"{name}:{first_no}: orphaned table row(s) — a pipe block must open with a"
                " header row followed by a |---| separator (a blank line above these rows"
                " probably severed them from their table)"
            )
            continue
        width = len(split_cells(first))
        for row_no, row in block[1:]:
            if row_no != block[1][0] and is_separator_row(row):
                errors.append(
                    f"{name}:{row_no}: unexpected separator row inside table body"
                )
                continue
            cells = len(split_cells(row))
            if cells != width:
                errors.append(
                    f"{name}:{row_no}: row has {cells} cells but the header at line"
                    f" {first_no} has {width} (unescaped `|` in a cell, or a truncated row?)"
                )
    return errors


def main(argv: list[str]) -> int:
    targets = [Path(a) for a in argv] or [ROOT / "PLAN.md"]
    errors: list[str] = []
    for target in targets:
        rel = target if target.is_absolute() else ROOT / target
        errors.extend(check_text(rel.read_text(encoding="utf-8"), str(target)))
    if errors:
        print("Markdown table structure errors:")
        for err in errors:
            print(f"  - {err}")
        return 1
    print("All Markdown tables are well-formed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))

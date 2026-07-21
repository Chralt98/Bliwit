#!/usr/bin/env python3
"""Check PLAN.md's spec-question resolution-batch index against the question table.

PLAN.md's *Spec questions* section claims its batch assignment "is checked
mechanically". Until this script existed that claim was aspirational — the index
was maintained by hand across concurrent branches, which is exactly the shape
that drifts. Two real incidents motivated it:

  * batch B1 was left declaring rows that a later PR had already resolved, so
    the index advertised work that no longer existed;
  * two branches independently minted `SQ-286` for different questions and the
    collision survived a merge, because nothing checked id uniqueness.

Invariants enforced here:

  1. Every id in the question table is unique (the collision guard).
  2. Every OPEN question is named by exactly one batch.
  3. No batch names a CLOSED question, or an id with no question row.
  4. A batch's declared row count equals the number of ids it lists.
  5. A batch declaring 0 rows is CLOSED: its Members cell is a prose disposition
     note, not a member list, and no ids are extracted from it. This is what
     lets a closed batch explain where its rows went ("SQ-79 reclassified to X")
     without those mentions reading as live assignments.

A row is OPEN iff its status cell *starts* with "open". Every other leading verb
the project uses (resolved / RULED / RATIFIED / RECONCILED / …) closes it.
Matching a bare "resolved" anywhere in the cell is wrong: open rows legitimately
say things like "`gate.v_min` resolved; two rows remain".
"""

from __future__ import annotations

import re
import sys
from collections import Counter
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]

BATCH_HEADER = ("Batch", "Rows", "Members")
QUESTION_HEADER = ("ID", "Question", "Spec ref", "Raised", "Status")

BATCH_LABEL_RE = re.compile(r"^\*\*([BDCX][0-9]?)\s*[·.]")
QUESTION_ID_RE = re.compile(r"^SQ-(\d+)$")
SQ_RE = re.compile(r"SQ-(\d+)")
SEPARATOR_CELL_RE = re.compile(r"^:?-+:?$")


def split_cells(line: str) -> list[str]:
    """Split a table row into cells exactly as GFM does: every unescaped `|`
    delimits — backtick code spans do NOT protect pipes in table rows, only
    `\\|` does. Mirrors `check-plan-tables.py`."""
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
    if cells and cells[0] == "":
        cells = cells[1:]
    if cells and cells[-1] == "":
        cells = cells[:-1]
    return cells


def is_separator_row(line: str) -> bool:
    cells = split_cells(line)
    return bool(cells) and all(SEPARATOR_CELL_RE.match(c) for c in cells)


def iter_rows(text: str, header: tuple[str, ...]):
    """Yield (lineno, cells) for every body row of every table whose header
    matches `header` exactly. Fenced code blocks are skipped."""
    lines = text.splitlines()
    in_fence = False
    i = 0
    while i < len(lines):
        line = lines[i]
        if line.lstrip().startswith("```"):
            in_fence = not in_fence
            i += 1
            continue
        if in_fence or not line.startswith("|"):
            i += 1
            continue
        if tuple(split_cells(line)) == header and i + 1 < len(lines) and is_separator_row(lines[i + 1]):
            j = i + 2
            while j < len(lines) and lines[j].startswith("|"):
                if not is_separator_row(lines[j]):
                    yield j + 1, split_cells(lines[j])
                j += 1
            i = j
            continue
        i += 1


def check_text(text: str) -> list[str]:
    errors: list[str] = []

    # --- the question table --------------------------------------------------
    status: dict[int, str] = {}
    seen: Counter[int] = Counter()
    for lineno, cells in iter_rows(text, QUESTION_HEADER):
        m = QUESTION_ID_RE.match(cells[0])
        if not m:
            continue
        sid = int(m.group(1))
        seen[sid] += 1
        if seen[sid] > 1:
            errors.append(
                f"PLAN.md:{lineno}: SQ-{sid} is defined more than once — two branches"
                " minted the same id; renumber the newer one"
            )
            continue
        leading = cells[-1].lstrip("*_ ").lower()
        status[sid] = "open" if leading.startswith("open") else "resolved"

    if not status:
        return ["PLAN.md: no spec-question table found (header changed?)"]

    open_ids = {i for i, s in status.items() if s == "open"}
    closed_ids = {i for i, s in status.items() if s == "resolved"}

    # --- the batch index -----------------------------------------------------
    assigned: dict[int, str] = {}
    batches = 0
    for lineno, cells in iter_rows(text, BATCH_HEADER):
        m = BATCH_LABEL_RE.match(cells[0])
        if not m:
            continue
        batches += 1
        label = m.group(1)
        try:
            declared = int(cells[1])
        except ValueError:
            errors.append(f"PLAN.md:{lineno}: batch {label} row count is not a number")
            continue
        if declared == 0:
            # Closed batch: the Members cell is prose explaining the disposition.
            continue
        # Members are listed before any trailing "— *annotation*" commentary.
        head = re.split(r"\s+—\s+", cells[2])[0]
        ids = [int(i) for i in SQ_RE.findall(head)]
        if len(ids) != declared:
            errors.append(
                f"PLAN.md:{lineno}: batch {label} declares {declared} rows but lists {len(ids)}"
            )
        for sid in ids:
            if sid in assigned:
                errors.append(
                    f"PLAN.md:{lineno}: SQ-{sid} is named by both batch {assigned[sid]} and batch {label}"
                )
            assigned[sid] = label

    if not batches:
        return errors + ["PLAN.md: no batch-index table found (header changed?)"]

    for sid in sorted(open_ids - set(assigned)):
        errors.append(f"SQ-{sid} is OPEN but assigned to no batch")
    for sid in sorted(set(assigned) & closed_ids):
        errors.append(
            f"SQ-{sid} is RESOLVED but still named by batch {assigned[sid]}"
            " — drop it from the index (a closed batch may mention it in prose instead)"
        )
    for sid in sorted(set(assigned) - open_ids - closed_ids):
        errors.append(f"SQ-{sid} is named by batch {assigned[sid]} but has no question row")

    if not errors:
        print(
            f"Spec-question batches OK — {len(status)} rows"
            f" ({len(open_ids)} open, {len(closed_ids)} resolved);"
            f" every open row assigned to exactly one of {batches} batches."
        )
    return errors


def main(argv: list[str]) -> int:
    target = Path(argv[0]) if argv else ROOT / "PLAN.md"
    if not target.is_absolute():
        target = ROOT / target
    errors = check_text(target.read_text(encoding="utf-8"))
    if errors:
        print("Spec-question batch-index errors:")
        for err in errors:
            print(f"  - {err}")
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))

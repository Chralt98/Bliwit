#!/usr/bin/env python3
"""Check generated weight proof comments against frozen storage bounds.

The generated files are the runtime's dispatch envelope.  This small structural
gate catches the failure mode behind SQ-317: a bounded value grows, but an old
weight file still carries the previous ``max_size`` in its proof annotation.
The canonical bound is read from architecture/13's normative arithmetic rather
than duplicated in a runtime constant.
"""
from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
WEIGHTS = ROOT / "runtime" / "bleavit-runtime" / "src" / "weights"
ARCHITECTURE = ROOT / "docs" / "architecture" / "13-parameters.md"

STORAGE_RE = re.compile(r"Storage: `([^`]+)`")
MAX_SIZE_RE = re.compile(r"max_size`: Some\((\d+)\)")


def recent_summary_bound() -> int:
    text = ARCHITECTURE.read_text(encoding="utf-8")
    match = re.search(
        r"`RecentCohortSummaries` max-encodes to \*\*1 \+ 32 × 158 = ([0-9,]+) B\*\*",
        text,
    )
    if not match:
        raise ValueError("13 §4 RecentCohortSummaries bound is missing or changed shape")
    return int(match.group(1).replace(",", ""))


def storage_annotations() -> dict[str, list[tuple[Path, int]]]:
    result: dict[str, list[tuple[Path, int]]] = {}
    for path in sorted(WEIGHTS.glob("*.rs")):
        lines = path.read_text(encoding="utf-8").splitlines()
        for index, line in enumerate(lines):
            storage = STORAGE_RE.search(line)
            if not storage:
                continue
            for following in lines[index + 1 : index + 4]:
                size = MAX_SIZE_RE.search(following)
                if size:
                    result.setdefault(storage.group(1), []).append(
                        (path, int(size.group(1)))
                    )
                    break
    return result


def main() -> int:
    annotations = storage_annotations()
    expected = recent_summary_bound()
    rows = annotations.get("Epoch::RecentCohortSummaries", [])
    if not rows:
        print("weight-storage-bounds: no RecentCohortSummaries annotations found", file=sys.stderr)
        return 1
    failures = [
        f"{path}: max_size={size}, expected at least {expected}"
        for path, size in rows
        if size < expected
    ]
    if failures:
        print("weight-storage-bounds: stale generated proof bounds:", file=sys.stderr)
        print("\n".join(failures), file=sys.stderr)
        return 1
    print(f"weight-storage-bounds: {len(rows)} annotations checked; RecentCohortSummaries >= {expected} B")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

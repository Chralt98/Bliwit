#!/usr/bin/env python3
"""Check repository-local Markdown links.

External URLs and intra-document anchors are intentionally ignored; this lightweight
checker is the M0 gate for living-doc relative links until a fuller docs pipeline is
introduced.
"""
from __future__ import annotations

import re
import subprocess
import sys
from pathlib import Path
from urllib.parse import unquote, urlparse

ROOT = Path(__file__).resolve().parents[2]
LINK_RE = re.compile(r"(?<!!)\[[^\]]*\]\(([^)]+)\)")
SKIP_DIRS = {".git", "target", "node_modules"}


def ignored(candidates: list[Path]) -> set[Path]:
    """The subset of `candidates` git excludes, asked in one batch.

    The walk below is a filesystem walk, not an index walk, so without this it
    descends into every nested git worktree under `.claude/worktrees/` and reports
    the stale `docs/` copies inside them. Those copies also defeat `link_base`,
    whose verbatim special case is anchored at this checkout's `VERBATIM_DIR` and
    so does not match a copy living under another worktree — every verbatim link
    in them resolves against the wrong directory and is reported missing.

    That produced ~300 failures that CI could never see, since CI checks out a
    clean tree with no worktrees in it. Local-only noise is the worst kind: a gate
    that cries wolf on a developer's machine is a gate they stop reading, and this
    one guards the living documents.

    Asking git (rather than hardcoding `.claude/worktrees`) keeps the rule honest
    for `.gitignore`, `.git/info/exclude`, and any future excluded path alike,
    while still checking untracked-but-not-ignored files — a doc you just wrote
    and have not committed must still have its links verified.
    """
    if not candidates:
        return set()
    try:
        proc = subprocess.run(
            ["git", "check-ignore", "-z", "--stdin"],
            cwd=ROOT,
            input="\0".join(str(p.relative_to(ROOT)) for p in candidates),
            capture_output=True,
            text=True,
            check=False,
        )
    except OSError:
        # No git (source tarball, minimal container): check everything rather
        # than silently skipping files. Over-reporting is recoverable here;
        # under-reporting is what this checker exists to prevent.
        return set()
    if proc.returncode not in (0, 1):  # 0 = some ignored, 1 = none; else broken
        return set()
    return {ROOT / line for line in proc.stdout.split("\0") if line}

# Declared byte-copies of docs/architecture files (see the DO-NOT-EDIT header in each):
# their relative links are meaningful relative to the copied file's source directory,
# so resolve them there instead of rewriting the verbatim content.
VERBATIM_DIR = ROOT / "docs" / "design" / "claude-design-kit"
ARCHITECTURE_DIR = ROOT / "docs" / "architecture"


def link_base(md: Path) -> Path:
    if md.parent == VERBATIM_DIR and md.name.endswith("-VERBATIM.md"):
        return ARCHITECTURE_DIR
    return md.parent


errors: list[str] = []

candidates = sorted(
    p for p in ROOT.rglob("*.md") if not (set(p.relative_to(ROOT).parts) & SKIP_DIRS)
)
excluded = ignored(candidates)
checked = [p for p in candidates if p not in excluded]

for md in checked:
    text = md.read_text(encoding="utf-8")
    for match in LINK_RE.finditer(text):
        raw_target = match.group(1).strip()
        if not raw_target or raw_target.startswith("#"):
            continue
        if raw_target.startswith("<") and raw_target.endswith(">"):
            raw_target = raw_target[1:-1]
        parsed = urlparse(raw_target)
        if parsed.scheme or parsed.netloc:
            continue
        path_part = unquote(parsed.path)
        if not path_part:
            continue
        target = (link_base(md) / path_part).resolve()
        try:
            target.relative_to(ROOT)
        except ValueError:
            errors.append(f"{md.relative_to(ROOT)}: link escapes repo: {raw_target}")
            continue
        if not target.exists():
            line = text.count("\n", 0, match.start()) + 1
            errors.append(f"{md.relative_to(ROOT)}:{line}: missing link target: {raw_target}")

if errors:
    print("Broken local Markdown links:", file=sys.stderr)
    for error in errors:
        print(f"  - {error}", file=sys.stderr)
    sys.exit(1)

summary = f"All local Markdown links resolve ({len(checked)} file(s) checked"
if excluded:
    # Never skip silently: a shrinking denominator is how a green gate stops
    # meaning anything.
    summary += f"; {len(excluded)} git-excluded file(s) skipped"
print(summary + ").")

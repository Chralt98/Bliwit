#!/usr/bin/env python3
"""Check repository-local Markdown links.

External URLs and intra-document anchors are intentionally ignored; this lightweight
checker is the M0 gate for living-doc relative links until a fuller docs pipeline is
introduced.
"""
from __future__ import annotations

import re
import sys
from pathlib import Path
from urllib.parse import unquote, urlparse

ROOT = Path(__file__).resolve().parents[2]
LINK_RE = re.compile(r"(?<!!)\[[^\]]*\]\(([^)]+)\)")
SKIP_DIRS = {".git", "target", "node_modules"}

errors: list[str] = []

for md in sorted(p for p in ROOT.rglob("*.md") if not (set(p.relative_to(ROOT).parts) & SKIP_DIRS)):
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
        target = (md.parent / path_part).resolve()
        try:
            target.relative_to(ROOT)
        except ValueError:
            errors.append(f"{md.relative_to(ROOT)}: link escapes repo: {raw_target}")
            continue
        if not target.exists():
            architecture_fallback = ROOT / "docs" / "architecture" / Path(path_part).name
            if architecture_fallback.exists():
                continue
            line = text.count("\n", 0, match.start()) + 1
            errors.append(f"{md.relative_to(ROOT)}:{line}: missing link target: {raw_target}")

if errors:
    print("Broken local Markdown links:", file=sys.stderr)
    for error in errors:
        print(f"  - {error}", file=sys.stderr)
    sys.exit(1)

print("All local Markdown links resolve.")

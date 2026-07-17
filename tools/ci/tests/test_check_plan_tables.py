"""Gates for the PLAN.md table-structure checker.

The checker exists because a blank line inside a Markdown table strands every
row below it from the header (the 2026-07-17 B10/B11 incident: two milestone
rows rendered as raw pipe-text), and because GFM splits table cells on every
unescaped pipe — even inside backtick code spans. These tests pin both failure
modes, the escape semantics, and that the checker stays green on the real
PLAN.md (standing user instruction, 2026-07-17: PLAN.md tables must never
drift/break).
"""

from __future__ import annotations

import importlib.util
import sys
import unittest
from pathlib import Path


SCRIPT = Path(__file__).resolve().parents[1] / "check-plan-tables.py"
SPEC = importlib.util.spec_from_file_location("check_plan_tables", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
checker = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = checker
SPEC.loader.exec_module(checker)

REPO_ROOT = Path(__file__).resolve().parents[3]

WELL_FORMED = """# Doc

| ID | Milestone | Status |
|---|---|---|
| B1 | First | done |
| B2 | Second | open |

Prose between tables.

| Date | Note |
|---|---|
| 2026-07-17 | fine |
"""


class TestSplitCells(unittest.TestCase):
    def test_plain_row(self) -> None:
        self.assertEqual(checker.split_cells("| a | b | c |"), ["a", "b", "c"])

    def test_escaped_pipe_does_not_split(self) -> None:
        self.assertEqual(checker.split_cells(r"| a \| b | c |"), [r"a \| b", "c"])

    def test_unescaped_pipe_in_code_span_splits_like_gfm(self) -> None:
        # GFM treats every unescaped pipe as a delimiter, code span or not.
        self.assertEqual(len(checker.split_cells("| `a|b` | c |")), 3)


class TestCheckText(unittest.TestCase):
    def test_well_formed_passes(self) -> None:
        self.assertEqual(checker.check_text(WELL_FORMED, "doc"), [])

    def test_blank_line_orphans_rows(self) -> None:
        # The B10/B11 incident shape: a blank line severs body rows from the header.
        broken = WELL_FORMED.replace("| B2 | Second | open |", "\n| B2 | Second | open |")
        errors = checker.check_text(broken, "doc")
        self.assertEqual(len(errors), 1)
        self.assertIn("orphaned table row", errors[0])

    def test_missing_header_detected(self) -> None:
        errors = checker.check_text("|---|---|\n| a | b |\n", "doc")
        self.assertEqual(len(errors), 1)
        self.assertIn("header row missing", errors[0])

    def test_cell_count_mismatch_detected(self) -> None:
        broken = WELL_FORMED.replace("| B1 | First | done |", "| B1 | First |")
        errors = checker.check_text(broken, "doc")
        self.assertEqual(len(errors), 1)
        self.assertIn("2 cells", errors[0])

    def test_unescaped_code_span_pipe_flags_mismatch(self) -> None:
        broken = WELL_FORMED.replace("| B1 | First | done |", "| B1 | `a|b` | done |")
        errors = checker.check_text(broken, "doc")
        self.assertEqual(len(errors), 1)
        self.assertIn("4 cells", errors[0])

    def test_separator_inside_body_detected(self) -> None:
        broken = WELL_FORMED.replace("| B2 | Second | open |", "|---|---|---|")
        errors = checker.check_text(broken, "doc")
        self.assertEqual(len(errors), 1)
        self.assertIn("separator row inside table body", errors[0])

    def test_fenced_code_blocks_ignored(self) -> None:
        fenced = "```\n| not | a | table\n\n| still | not | one\n```\n" + WELL_FORMED
        self.assertEqual(checker.check_text(fenced, "doc"), [])


class TestRepoPlan(unittest.TestCase):
    def test_actual_plan_md_is_well_formed(self) -> None:
        text = (REPO_ROOT / "PLAN.md").read_text(encoding="utf-8")
        self.assertEqual(checker.check_text(text, "PLAN.md"), [])


if __name__ == "__main__":
    unittest.main()

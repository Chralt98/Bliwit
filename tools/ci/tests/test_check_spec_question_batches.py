"""Gates for the PLAN.md spec-question batch-index checker.

The checker backs a claim PLAN.md already makes ("the assignment is checked
mechanically"). These tests pin the two drift shapes that motivated it — a batch
still naming rows a later PR resolved, and two branches minting the same SQ id —
plus the closed-batch rule that lets a 0-row batch explain its disposition in
prose without those mentions reading as live assignments.
"""

from __future__ import annotations

import importlib.util
import sys
import unittest
from pathlib import Path


SCRIPT = Path(__file__).resolve().parents[1] / "check-spec-question-batches.py"
SPEC = importlib.util.spec_from_file_location("check_spec_question_batches", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
checker = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = checker
SPEC.loader.exec_module(checker)

REPO_ROOT = Path(__file__).resolve().parents[3]

WELL_FORMED = """## Spec questions

| Batch | Rows | Members |
|---|---:|---|
| **B1 · ratify 05 — lifecycle** | 0 | **closed.** All rows disposed; SQ-3 reclassified to X. |
| **B2 · ratify 06 — governance** | 1 | SQ-1 |
| **X · code — real implementation work** | 2 | SQ-3, SQ-4 |

| ID | Question | Spec ref | Raised | Status |
|---|---|---|---|---|
| SQ-1 | First | 06 §1 | 2026-01-01 | open — batch B2 |
| SQ-2 | Second | 05 §2 | 2026-01-01 | **resolved 2026-02-01.** Done. |
| SQ-3 | Third | 09 §3 | 2026-01-01 | open — batch X |
| SQ-4 | Fourth | 09 §4 | 2026-01-01 | open — batch X |
"""


class TestWellFormed(unittest.TestCase):
    def test_passes(self) -> None:
        self.assertEqual(checker.check_text(WELL_FORMED), [])


class TestDriftShapes(unittest.TestCase):
    def test_duplicate_id_detected(self) -> None:
        # The SQ-286 collision: two branches mint the same id, the merge keeps both.
        broken = WELL_FORMED.replace(
            "| SQ-4 | Fourth | 09 §4 | 2026-01-01 | open — batch X |",
            "| SQ-4 | Fourth | 09 §4 | 2026-01-01 | open — batch X |\n"
            "| SQ-4 | Collision | 09 §5 | 2026-01-02 | open — batch X |",
        )
        errors = checker.check_text(broken)
        self.assertTrue(any("defined more than once" in e for e in errors), errors)

    def test_batch_naming_a_resolved_row_detected(self) -> None:
        broken = WELL_FORMED.replace("| **X · code — real implementation work** | 2 | SQ-3, SQ-4 |",
                                     "| **X · code — real implementation work** | 3 | SQ-2, SQ-3, SQ-4 |")
        errors = checker.check_text(broken)
        self.assertTrue(any("SQ-2 is RESOLVED" in e for e in errors), errors)

    def test_open_row_with_no_batch_detected(self) -> None:
        broken = WELL_FORMED.replace("| **B2 · ratify 06 — governance** | 1 | SQ-1 |\n", "")
        errors = checker.check_text(broken)
        self.assertTrue(any("SQ-1 is OPEN but assigned to no batch" in e for e in errors), errors)

    def test_declared_count_mismatch_detected(self) -> None:
        broken = WELL_FORMED.replace("| **X · code — real implementation work** | 2 |",
                                     "| **X · code — real implementation work** | 3 |")
        errors = checker.check_text(broken)
        self.assertTrue(any("declares 3 rows but lists 2" in e for e in errors), errors)

    def test_id_in_two_batches_detected(self) -> None:
        broken = WELL_FORMED.replace("| **B2 · ratify 06 — governance** | 1 | SQ-1 |",
                                     "| **B2 · ratify 06 — governance** | 2 | SQ-1, SQ-3 |")
        errors = checker.check_text(broken)
        self.assertTrue(any("named by both batch" in e for e in errors), errors)

    def test_batch_naming_a_nonexistent_row_detected(self) -> None:
        broken = WELL_FORMED.replace("| **X · code — real implementation work** | 2 | SQ-3, SQ-4 |",
                                     "| **X · code — real implementation work** | 3 | SQ-3, SQ-4, SQ-99 |")
        errors = checker.check_text(broken)
        self.assertTrue(any("SQ-99 is named by batch X but has no question row" in e for e in errors), errors)


class TestClosedBatchProse(unittest.TestCase):
    def test_prose_mentions_in_a_closed_batch_are_not_assignments(self) -> None:
        # B1 declares 0 rows and mentions SQ-3 in prose; SQ-3 belongs to X.
        self.assertEqual(checker.check_text(WELL_FORMED), [])

    def test_closed_batch_prose_may_mention_a_resolved_row(self) -> None:
        text = WELL_FORMED.replace(
            "SQ-3 reclassified to X.", "SQ-2 fixed in code and SQ-3 reclassified to X."
        )
        self.assertEqual(checker.check_text(text), [])


class TestStatusParsing(unittest.TestCase):
    def test_open_row_quoting_the_word_resolved_stays_open(self) -> None:
        text = WELL_FORMED.replace(
            "| SQ-1 | First | 06 §1 | 2026-01-01 | open — batch B2 |",
            "| SQ-1 | First | 06 §1 | 2026-01-01 | open — one leg resolved; two remain (batch B2) |",
        )
        self.assertEqual(checker.check_text(text), [])

    def test_ruled_closes_a_row(self) -> None:
        text = WELL_FORMED.replace(
            "| SQ-4 | Fourth | 09 §4 | 2026-01-01 | open — batch X |",
            "| SQ-4 | Fourth | 09 §4 | 2026-01-01 | **RULED 2026-02-01.** Decided. |",
        ).replace("| **X · code — real implementation work** | 2 | SQ-3, SQ-4 |",
                  "| **X · code — real implementation work** | 1 | SQ-3 |")
        self.assertEqual(checker.check_text(text), [])


class TestRepoPlan(unittest.TestCase):
    def test_actual_plan_md_index_is_consistent(self) -> None:
        text = (REPO_ROOT / "PLAN.md").read_text(encoding="utf-8")
        self.assertEqual(checker.check_text(text), [])


if __name__ == "__main__":
    unittest.main()

import contextlib
import importlib.util
import io
import sys
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).resolve().parents[1] / "check-runbooks.py"
MODULE_SPEC = importlib.util.spec_from_file_location("check_runbooks", SCRIPT)
if MODULE_SPEC is None or MODULE_SPEC.loader is None:
    raise RuntimeError("runbook checker module must be importable")
checker = importlib.util.module_from_spec(MODULE_SPEC)
sys.modules[MODULE_SPEC.name] = checker
MODULE_SPEC.loader.exec_module(checker)


DOC = """# Synthetic operations document

## 6. Operational layer

### 6.1 Owned-and-funded ops table (normative)

| Service | Commitment (MUST) | Owner role | Funding line ([08](ref.md)) |
|---|---|---|---|
| **Test service** | Operate it | Test operator | `ops.test` |
| **Alternate service** | Operate it too | Alternate operator | `ops.alternate` |

### 6.3 Monitoring and alerting

| Domain | Key series | Alert (example) | Runbook |
|---|---|---|---|
| Alpha | alpha_series | alpha bad | RB-ONE |

New rows:

| Domain | Key series | Alert | Runbook |
|---|---|---|---|
| Beta | beta_series | beta bad | RB-TWO (page immediately) |

### 6.4 Incident response

Synthetic incident-response playbooks.
"""

SECTIONS = """## Purpose

Synthetic purpose.

## Alerts

| Domain | Key series | Trigger |
|---|---|---|
| {domain} | {key_series} | {trigger} |

## Diagnosis

1. Inspect the synthetic state.

## Remediation

1. Apply the synthetic safe action.

## Escalation

Page the test operator.

## References

- [Synthetic reference](../../docs/ref.md)
"""


def runbook(
    runbook_id: str,
    domain: str,
    trigger: str,
    *,
    page: bool = False,
    owner: str = "Test operator",
    funding: str = "ops.test",
    key_series: str | None = None,
    sections: str | None = None,
    spec_ref: str = "docs/ref.md",
) -> str:
    if key_series is None:
        key_series = f"{domain.lower().replace(' ', '_')}_series"
    if sections is None:
        sections = SECTIONS.format(
            domain=domain,
            key_series=key_series,
            trigger=trigger,
        )
    return f"""---
id: {runbook_id}
title: Synthetic {runbook_id}
owner_role: {owner}
funding_line: {funding}
page_immediately: {str(page).lower()}
alerts:
  - domain: {domain}
    trigger: {trigger}
spec_refs:
  - {spec_ref}
---

{sections}"""


class CheckRunbooksTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        (self.root / "docs").mkdir()
        (self.root / "docs" / "ref.md").write_text(
            "# Reference\n\n## Valid target\n", encoding="utf-8"
        )
        (self.root / "docs" / "doc-12.md").write_text(DOC, encoding="utf-8")
        self.runbooks_dir = self.root / "deploy" / "runbooks"
        self.runbooks_dir.mkdir(parents=True)
        self.write_runbook("RB-ONE", "Alpha", "alpha bad")
        self.write_runbook("RB-TWO", "Beta", "beta bad", page=True)
        self.write_readme(("RB-ONE", "RB-TWO"))

    def write_runbook(
        self,
        runbook_id: str,
        domain: str,
        trigger: str,
        **kwargs: object,
    ) -> Path:
        path = self.runbooks_dir / f"{runbook_id}.md"
        path.write_text(
            runbook(runbook_id, domain, trigger, **kwargs), encoding="utf-8"
        )
        return path

    def write_readme(self, runbook_ids: tuple[str, ...]) -> None:
        rows = "\n".join(
            "| {runbook_id} | Synthetic {runbook_id} | Test operator | {page} |".format(
                runbook_id=runbook_id,
                page="true" if runbook_id == "RB-TWO" else "false",
            )
            for runbook_id in runbook_ids
        )
        (self.runbooks_dir / "README.md").write_text(
            "# Runbooks\n\n"
            "| ID | Title | owner_role | page_immediately |\n"
            f"|---|---|---|---|\n{rows}\n",
            encoding="utf-8",
        )

    def check(self) -> tuple[list[str], int, int]:
        return checker.check_repository(
            self.root,
            Path("docs/doc-12.md"),
            Path("deploy/runbooks"),
        )

    def test_green_path(self) -> None:
        errors, runbook_count, alert_count = self.check()

        self.assertEqual(errors, [])
        self.assertEqual((runbook_count, alert_count), (2, 2))

        stdout = io.StringIO()
        with contextlib.redirect_stdout(stdout):
            result = checker.main(
                [
                    "--root",
                    str(self.root),
                    "--doc",
                    "docs/doc-12.md",
                    "--runbooks-dir",
                    "deploy/runbooks",
                ]
            )
        self.assertEqual(result, 0)
        self.assertEqual(stdout.getvalue(), "OK (2 runbooks, 2 alert rows bound)\n")

    def test_missing_runbook_file(self) -> None:
        (self.runbooks_dir / "RB-ONE.md").unlink()

        errors, _runbooks, _alerts = self.check()

        self.assertTrue(
            any("RB-ONE.md:1: missing runbook file" in e for e in errors), errors
        )

    def test_orphan_runbook_file(self) -> None:
        self.write_runbook("RB-ORPHAN", "Orphan", "orphan bad")

        errors, _runbooks, _alerts = self.check()

        self.assertTrue(any("orphan runbook id RB-ORPHAN" in e for e in errors), errors)

    def test_wrong_or_missing_alert_row(self) -> None:
        self.write_runbook("RB-ONE", "Wrong domain", "alpha bad")

        errors, _runbooks, _alerts = self.check()

        self.assertTrue(
            any("missing alert binding" in e and "Alpha" in e for e in errors),
            errors,
        )
        self.assertTrue(
            any(
                "not exact after Markdown stripping" in e and "Wrong domain" in e
                for e in errors
            ),
            errors,
        )

    def test_trigger_text_drift(self) -> None:
        self.write_runbook("RB-ONE", "Alpha", "alpha drifted")

        errors, _runbooks, _alerts = self.check()

        self.assertTrue(
            any("missing alert binding" in e and "alpha bad" in e for e in errors),
            errors,
        )
        self.assertTrue(
            any(
                "not exact after Markdown stripping" in e and "alpha drifted" in e
                for e in errors
            ),
            errors,
        )

    def test_page_immediately_mismatch(self) -> None:
        self.write_runbook("RB-TWO", "Beta", "beta bad", page=False)

        errors, _runbooks, _alerts = self.check()

        self.assertTrue(
            any("page_immediately must be true" in e for e in errors), errors
        )

    def test_bad_owner_role_and_funding_line(self) -> None:
        self.write_runbook(
            "RB-ONE",
            "Alpha",
            "alpha bad",
            owner="Unknown operator",
            funding="ops.unknown",
        )

        errors, _runbooks, _alerts = self.check()

        self.assertTrue(
            any(
                "owner_role/funding_line pair" in e
                and "does not appear together in one §6.1 row" in e
                for e in errors
            ),
            errors,
        )

    def test_owner_and_funding_must_come_from_the_same_ops_row(self) -> None:
        self.write_runbook(
            "RB-ONE",
            "Alpha",
            "alpha bad",
            owner="Test operator",
            funding="ops.alternate",
        )

        errors, _runbooks, _alerts = self.check()

        self.assertTrue(
            any("owner_role/funding_line pair" in error for error in errors), errors
        )

    def test_missing_body_section(self) -> None:
        sections = SECTIONS.format(
            domain="Alpha", key_series="alpha_series", trigger="alpha bad"
        ).replace("## Diagnosis\n", "")
        self.write_runbook("RB-ONE", "Alpha", "alpha bad", sections=sections)

        errors, _runbooks, _alerts = self.check()

        self.assertTrue(
            any("missing required section '## Diagnosis'" in e for e in errors),
            errors,
        )

    def test_broken_spec_ref_link(self) -> None:
        self.write_runbook(
            "RB-ONE", "Alpha", "alpha bad", spec_ref="docs/missing.md"
        )

        errors, _runbooks, _alerts = self.check()

        self.assertTrue(
            any("spec_refs" not in e and "docs/missing.md" in e for e in errors),
            errors,
        )

    def test_frontmatter_flow_list_rejected(self) -> None:
        original = (self.runbooks_dir / "RB-ONE.md").read_text(encoding="utf-8")
        changed = original.replace(
            "spec_refs:\n  - docs/ref.md", "spec_refs: [docs/ref.md]"
        )
        (self.runbooks_dir / "RB-ONE.md").write_text(changed, encoding="utf-8")

        errors, _runbooks, _alerts = self.check()

        self.assertTrue(any("flow syntax is forbidden" in e for e in errors), errors)

    def test_frontmatter_unknown_key_rejected(self) -> None:
        path = self.runbooks_dir / "RB-ONE.md"
        changed = path.read_text(encoding="utf-8").replace("title:", "unknown_key:")
        path.write_text(changed, encoding="utf-8")

        errors, _runbooks, _alerts = self.check()

        self.assertTrue(any("unknown top-level key" in e for e in errors), errors)

    def test_frontmatter_literal_multiline_scalar_rejected(self) -> None:
        path = self.runbooks_dir / "RB-ONE.md"
        changed = path.read_text(encoding="utf-8").replace(
            "title: Synthetic RB-ONE", "title: |\n  Synthetic RB-ONE"
        )
        path.write_text(changed, encoding="utf-8")

        errors, _runbooks, _alerts = self.check()

        self.assertTrue(
            any("unsupported frontmatter syntax" in e for e in errors), errors
        )

    def test_frontmatter_folded_multiline_scalar_rejected(self) -> None:
        path = self.runbooks_dir / "RB-ONE.md"
        changed = path.read_text(encoding="utf-8").replace(
            "title: Synthetic RB-ONE", "title: >-\n  Synthetic RB-ONE"
        )
        path.write_text(changed, encoding="utf-8")

        errors, _runbooks, _alerts = self.check()

        self.assertTrue(
            any("unsupported frontmatter syntax" in e for e in errors), errors
        )

    def test_readme_index_missing_an_id(self) -> None:
        self.write_readme(("RB-ONE",))

        errors, _runbooks, _alerts = self.check()

        self.assertTrue(
            any("runbook index is missing RB-TWO" in e for e in errors), errors
        )

    def test_unparsable_doc_table_fails_loudly(self) -> None:
        malformed = DOC.replace(
            "| Alpha | alpha_series | alpha bad | RB-ONE |",
            "| Alpha | alpha_series | RB-ONE |",
        )
        (self.root / "docs" / "doc-12.md").write_text(malformed, encoding="utf-8")

        errors, _runbooks, _alerts = self.check()

        self.assertTrue(any("unparsable alert row" in e for e in errors), errors)

    def test_non_markdown_backslash_escape_is_preserved(self) -> None:
        self.assertEqual(
            checker.split_table_row(r"| Alpha | alpha\q |"),
            ["Alpha", r"alpha\q"],
        )
        self.assertEqual(
            checker.split_table_row(r"| Alpha | alpha\|pipe |"),
            ["Alpha", "alpha|pipe"],
        )

    def test_frozen_o4_runbook_id_set_rejects_coordinated_removal(self) -> None:
        errors: list[str] = []
        checker.validate_frozen_runbook_ids(
            set(checker.FROZEN_RUNBOOK_IDS) - {"RB-XCM"},
            "docs/architecture/12-release-and-operations.md",
            errors,
        )

        self.assertTrue(
            any(
                "must equal the frozen 13-ID O4 set" in error
                and "missing RB-XCM" in error
                for error in errors
            ),
            errors,
        )

    def test_renamed_section_6_3_heading_fails_loudly(self) -> None:
        changed = DOC.replace(
            "### 6.3 Monitoring and alerting", "### Renamed monitoring"
        )
        (self.root / "docs" / "doc-12.md").write_text(changed, encoding="utf-8")

        errors, _runbooks, _alerts = self.check()

        self.assertTrue(
            any("found 0 headings for §6.3, expected exactly 1" in e for e in errors),
            errors,
        )

    def test_deleting_one_alert_table_fails_exact_table_count(self) -> None:
        second_table = """New rows:

| Domain | Key series | Alert | Runbook |
|---|---|---|---|
| Beta | beta_series | beta bad | RB-TWO (page immediately) |

"""
        changed = DOC.replace(second_table, "")
        (self.root / "docs" / "doc-12.md").write_text(changed, encoding="utf-8")

        errors, _runbooks, _alerts = self.check()

        self.assertTrue(
            any(
                "found 1 parseable §6.3 alert tables, expected exactly 2" in e
                for e in errors
            ),
            errors,
        )

    def test_missing_section_6_4_incident_response_heading_fails_loudly(self) -> None:
        changed = DOC.replace(
            "### 6.4 Incident response\n\nSynthetic incident-response playbooks.\n",
            "",
        )
        (self.root / "docs" / "doc-12.md").write_text(changed, encoding="utf-8")

        errors, _runbooks, _alerts = self.check()

        self.assertTrue(
            any(
                "missing required heading '### 6.4 Incident response'" in e
                for e in errors
            ),
            errors,
        )

    def test_runbook_cell_suffix_is_rejected(self) -> None:
        changed = DOC.replace(
            "| Alpha | alpha_series | alpha bad | RB-ONE |",
            "| Alpha | alpha_series | alpha bad | RB-ONE-WRONG |",
        )
        (self.root / "docs" / "doc-12.md").write_text(changed, encoding="utf-8")

        errors, _runbooks, _alerts = self.check()

        self.assertTrue(
            any("runbook cell must be exactly RB-[A-Z]+" in e for e in errors), errors
        )

    def test_alert_row_duplicated_across_runbooks_is_rejected(self) -> None:
        path = self.runbooks_dir / "RB-TWO.md"
        changed = path.read_text(encoding="utf-8").replace(
            "  - domain: Beta\n    trigger: beta bad",
            "  - domain: Alpha\n    trigger: alpha bad",
        )
        path.write_text(changed, encoding="utf-8")

        errors, _runbooks, _alerts = self.check()

        self.assertTrue(
            any("alert binding appears in multiple runbooks" in e for e in errors),
            errors,
        )

    def test_readme_index_stale_title_is_rejected(self) -> None:
        path = self.runbooks_dir / "README.md"
        changed = path.read_text(encoding="utf-8").replace(
            "| RB-ONE | Synthetic RB-ONE |",
            "| RB-ONE | Stale title |",
        )
        path.write_text(changed, encoding="utf-8")

        errors, _runbooks, _alerts = self.check()

        self.assertTrue(
            any("index Title does not match RB-ONE frontmatter" in e for e in errors),
            errors,
        )

    def test_readme_index_stale_owner_is_rejected(self) -> None:
        path = self.runbooks_dir / "README.md"
        changed = path.read_text(encoding="utf-8").replace(
            "| RB-ONE | Synthetic RB-ONE | Test operator |",
            "| RB-ONE | Synthetic RB-ONE | Stale operator |",
        )
        path.write_text(changed, encoding="utf-8")

        errors, _runbooks, _alerts = self.check()

        self.assertTrue(
            any(
                "index owner_role does not match RB-ONE frontmatter" in e
                for e in errors
            ),
            errors,
        )

    def test_readme_index_stale_page_marker_is_rejected(self) -> None:
        path = self.runbooks_dir / "README.md"
        changed = path.read_text(encoding="utf-8").replace(
            "| RB-ONE | Synthetic RB-ONE | Test operator | false |",
            "| RB-ONE | Synthetic RB-ONE | Test operator | true |",
        )
        path.write_text(changed, encoding="utf-8")

        errors, _runbooks, _alerts = self.check()

        self.assertTrue(
            any(
                "index page_immediately does not match RB-ONE frontmatter" in e
                for e in errors
            ),
            errors,
        )

    def test_readme_index_duplicate_id_is_rejected(self) -> None:
        path = self.runbooks_dir / "README.md"
        with path.open("a", encoding="utf-8") as handle:
            handle.write("| RB-ONE | Synthetic RB-ONE | Test operator | false |\n")

        errors, _runbooks, _alerts = self.check()

        self.assertTrue(
            any("duplicate runbook id RB-ONE in index" in e for e in errors),
            errors,
        )

    def test_readme_index_extra_id_is_rejected(self) -> None:
        path = self.runbooks_dir / "README.md"
        with path.open("a", encoding="utf-8") as handle:
            handle.write("| RB-EXTRA | Extra | Test operator | false |\n")

        errors, _runbooks, _alerts = self.check()

        self.assertTrue(
            any("unexpected runbook id RB-EXTRA in index" in e for e in errors),
            errors,
        )

    def test_lowercase_runbook_filename_is_rejected(self) -> None:
        (self.runbooks_dir / "rb-junk.md").write_text(
            runbook("RB-JUNK", "Junk", "junk bad"), encoding="utf-8"
        )

        errors, _runbooks, _alerts = self.check()

        self.assertTrue(
            any("filename must match RB-[A-Z]+.md" in e for e in errors), errors
        )

    def test_quoted_page_immediately_is_rejected(self) -> None:
        path = self.runbooks_dir / "RB-ONE.md"
        changed = path.read_text(encoding="utf-8").replace(
            "page_immediately: false", 'page_immediately: "false"'
        )
        path.write_text(changed, encoding="utf-8")

        errors, _runbooks, _alerts = self.check()

        self.assertTrue(
            any(
                "page_immediately must be literal unquoted true or false" in e
                for e in errors
            ),
            errors,
        )

    def test_scalar_comment_is_rejected(self) -> None:
        path = self.runbooks_dir / "RB-ONE.md"
        changed = path.read_text(encoding="utf-8").replace(
            "title: Synthetic RB-ONE", "title: # comment"
        )
        path.write_text(changed, encoding="utf-8")

        errors, _runbooks, _alerts = self.check()

        self.assertTrue(any("comments are not supported" in e for e in errors), errors)

    def test_scalar_explicit_tag_is_rejected(self) -> None:
        path = self.runbooks_dir / "RB-ONE.md"
        changed = path.read_text(encoding="utf-8").replace(
            "title: Synthetic RB-ONE", "title: !!str Synthetic RB-ONE"
        )
        path.write_text(changed, encoding="utf-8")

        errors, _runbooks, _alerts = self.check()

        self.assertTrue(any("tags are not supported" in e for e in errors), errors)

    def test_scalar_verbatim_tag_is_rejected(self) -> None:
        path = self.runbooks_dir / "RB-ONE.md"
        changed = path.read_text(encoding="utf-8").replace(
            "title: Synthetic RB-ONE", "title: !<tag> Synthetic RB-ONE"
        )
        path.write_text(changed, encoding="utf-8")

        errors, _runbooks, _alerts = self.check()

        self.assertTrue(any("tags are not supported" in e for e in errors), errors)

    def test_bad_markdown_fragment_is_rejected(self) -> None:
        self.write_runbook(
            "RB-ONE",
            "Alpha",
            "alpha bad",
            spec_ref="docs/ref.md#missing-heading",
        )

        errors, _runbooks, _alerts = self.check()

        self.assertTrue(
            any(
                "fragment '#missing-heading' does not match a heading" in e
                for e in errors
            ),
            errors,
        )

    def test_body_alert_key_series_drift_is_rejected(self) -> None:
        path = self.runbooks_dir / "RB-ONE.md"
        changed = path.read_text(encoding="utf-8").replace(
            "| Alpha | alpha_series | alpha bad |",
            "| Alpha | stale_series | alpha bad |",
        )
        path.write_text(changed, encoding="utf-8")

        errors, _runbooks, _alerts = self.check()

        self.assertTrue(
            any(
                "body Alerts row is not exact after Markdown stripping" in e
                and "stale_series" in e
                for e in errors
            ),
            errors,
        )

    def test_body_alert_domain_drift_is_rejected(self) -> None:
        path = self.runbooks_dir / "RB-ONE.md"
        changed = path.read_text(encoding="utf-8").replace(
            "| Alpha | alpha_series | alpha bad |",
            "| Stale domain | alpha_series | alpha bad |",
        )
        path.write_text(changed, encoding="utf-8")

        errors, _runbooks, _alerts = self.check()

        self.assertTrue(
            any(
                "body Alerts row is not exact after Markdown stripping" in error
                and "Stale domain" in error
                for error in errors
            ),
            errors,
        )

    def test_body_alert_trigger_drift_is_rejected(self) -> None:
        path = self.runbooks_dir / "RB-ONE.md"
        changed = path.read_text(encoding="utf-8").replace(
            "| Alpha | alpha_series | alpha bad |",
            "| Alpha | alpha_series | stale trigger |",
        )
        path.write_text(changed, encoding="utf-8")

        errors, _runbooks, _alerts = self.check()

        self.assertTrue(
            any(
                "body Alerts row is not exact after Markdown stripping" in error
                and "stale trigger" in error
                for error in errors
            ),
            errors,
        )


if __name__ == "__main__":
    unittest.main()

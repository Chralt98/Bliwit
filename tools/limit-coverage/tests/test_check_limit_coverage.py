"""Negative gates for the generated 15 §4.6 / I-22 limit inventory."""

from __future__ import annotations

import importlib.util
import json
import sys
import tempfile
import textwrap
import unittest
from pathlib import Path


SCRIPT = Path(__file__).resolve().parents[1] / "check-limit-coverage.py"
SPEC = importlib.util.spec_from_file_location("check_limit_coverage", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
checker = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = checker
SPEC.loader.exec_module(checker)


def table_row(*cells: str) -> str:
    return "| " + " | ".join(cells) + " |"


PARAM_HEADER = table_row(
    "Key",
    "Type",
    "Unit",
    "Default",
    "Hard min",
    "Hard max",
    "Max Δ/decision",
    "Cooldown",
    "Class",
    "Doc",
)
PARAM_SEPARATOR = table_row(*(":---" for _ in range(10)))


def sample_document(param_cell: str = "`epoch.length`") -> str:
    return textwrap.dedent(
        f"""
        # Parameters

        {checker.SECTION_HEADINGS['params']}

        {PARAM_HEADER}
        {PARAM_SEPARATOR}
        {table_row(param_cell, 'u32', 'blocks', '10', '1', '20', '1', '1', 'META', 'doc')}

        {checker.SECTION_HEADINGS['kernel']}

        | Constant | Value | Doc |
        |---|---|---|
        | `MAX_NESTED` (wrapper depth) | 4 | doc |

        ## 3. Ignored

        {checker.SECTION_HEADINGS['storage']}

        | Bound | Value | Scope (the reconciliation) | Doc |
        |---|---|---|---|
        | `IntakeQueue` | 64 | intake | [doc](doc.md) |
        """
    ).strip() + "\n"


BASE_MANIFEST = textwrap.dedent(
    """
    [[entry]]
    key = "epoch.length"
    class = "param-bounds"
    genesis = true

    [[entry]]
    key = "MAX_NESTED"
    class = "value"
    reason = "13 §2 sample row"

    [[entry]]
    key = "IntakeQueue"
    class = "dispatch-limit"
    error = "Epoch::IntakeFull"
    """
).strip() + "\n"

BASE_RUST = textwrap.dedent(
    """
    #[test]
    fn intake_overflow_is_rejected() {
        // limit-coverage: IntakeQueue
        assert_eq!("IntakeFull", "IntakeFull");
    }
    """
).strip() + "\n"


class LimitCoverageTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        (self.root / "docs" / "architecture").mkdir(parents=True)
        (self.root / "tools" / "limit-coverage").mkdir(parents=True)
        (self.root / "pallets" / "epoch" / "src").mkdir(parents=True)
        self.write("docs/architecture/13-parameters.md", sample_document())
        self.write("tools/limit-coverage/registry.toml", BASE_MANIFEST)
        self.write("tools/limit-coverage/genesis-keys.json", '["epoch.length"]\n')
        self.write("pallets/epoch/src/tests.rs", BASE_RUST)
        self.write(
            "PLAN.md",
            "| ID | Milestone | Spec | Depends | Status | Notes |\n"
            "|---|---|---|---|---|---|\n"
            "| B10 | Wiring | 13 | — | ⬜ | pending |\n",
        )

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def write(self, relative: str, value: str) -> None:
        (self.root / relative).write_text(value, encoding="utf-8")

    def failures(self) -> list[str]:
        failures, _, _ = checker.validate(self.root)
        return failures

    def assert_fails_with(self, fragment: str) -> None:
        failures = self.failures()
        self.assertTrue(
            any(fragment in failure for failure in failures),
            f"{fragment!r} absent from failures:\n" + "\n".join(failures),
        )

    def test_parser_expands_rule_6_forms(self) -> None:
        self.assertEqual(
            checker.extract_param_keys("`dec.delta` δ per class"),
            ["dec.delta.param", "dec.delta.trs", "dec.delta.code", "dec.delta.meta"],
        )
        self.assertEqual(
            checker.extract_param_keys(
                "`res.probe_interval` / `res.probe_timeout` "
                "(keys: `res.probe_int` / `res.probe_to`)"
            ),
            ["res.probe_int", "res.probe_to"],
        )
        self.assertEqual(
            checker.extract_param_keys(
                "`ops.*` budget lines (`ops.bootnodes`, "
                "`ops.oracle_evidence` (key: `ops.oracle_ev`), `ops.coretime`)"
            ),
            ["ops.bootnodes", "ops.coretime", "ops.oracle_ev"],
        )
        self.assertEqual(
            checker.kernel_keys(
                "`reg.max_filings_epoch` / `wt.max` / attestor registry floors "
                "(`att.min_members` = 3, `att.quorum` = 2)"
            ),
            ["reg.max_filings_epoch", "wt.max", "att.min_members", "att.quorum"],
        )
        self.assertEqual(
            checker.kernel_keys(
                "Crank batch bounds: `TickBatch` = 10; `ReapBatch` = 100; "
                "`settle_cohort` <= 100"
            ),
            ["TickBatch", "ReapBatch", "settle_cohort"],
        )
        self.assertEqual(
            checker.kernel_keys("`MinTrade` / `MaxTrade`"),
            ["MinTrade", "MaxTrade"],
        )
        self.assertEqual(
            checker.kernel_keys("`prop.max_calls` / `max_bytes` / `max_weight`"),
            ["prop.max_calls", "prop.max_bytes", "prop.max_weight"],
        )
        self.assertEqual(
            checker.kernel_keys("PB-LEDGER-FREEZE"),
            ["pb-ledger-freeze.duration", "pb-ledger-freeze.renewal"],
        )

    def test_parser_rejects_table_header_drift(self) -> None:
        document = sample_document().replace("Hard max", "Maximum", 1)
        with self.assertRaisesRegex(checker.RegistryError, "table header changed"):
            checker.extract_inventory(document)

    def test_parser_rejects_a_second_registry_table_in_the_same_section(self) -> None:
        second_table = "\n".join(
            (
                PARAM_HEADER,
                PARAM_SEPARATOR,
                table_row(
                    "`new.limit`",
                    "u32",
                    "items",
                    "1",
                    "0",
                    "2",
                    "1",
                    "1",
                    "META",
                    "doc",
                ),
                "",
            )
        )
        document = sample_document().replace(
            checker.SECTION_HEADINGS["kernel"],
            second_table + checker.SECTION_HEADINGS["kernel"],
            1,
        )
        with self.assertRaisesRegex(checker.RegistryError, "unexpected additional Markdown table"):
            checker.extract_inventory(document)

    def test_parser_rejects_an_indented_registry_row(self) -> None:
        document = sample_document().replace("| `MAX_NESTED`", "  | `MAX_NESTED`", 1)
        with self.assertRaisesRegex(checker.RegistryError, "indented Markdown table row"):
            checker.extract_inventory(document)

    def test_parser_rejects_a_malformed_compact_storage_row(self) -> None:
        document = sample_document().replace(
            "| `IntakeQueue` | 64 | intake | [doc](doc.md) |",
            "| `IntakeQueue` | 64 | not-a-doc-or-pallet |",
            1,
        )
        with self.assertRaisesRegex(checker.RegistryError, "malformed compact row"):
            checker.extract_inventory(document)

    def test_green_minimal_tree(self) -> None:
        self.assertEqual(self.failures(), [])

    def test_missing_manifest_key_fails(self) -> None:
        manifest = BASE_MANIFEST.split('[[entry]]\nkey = "IntakeQueue"', 1)[0]
        self.write("tools/limit-coverage/registry.toml", manifest)
        self.assert_fails_with("13 registry key 'IntakeQueue' is missing")

    def test_extra_manifest_key_fails(self) -> None:
        self.write(
            "tools/limit-coverage/registry.toml",
            BASE_MANIFEST
            + '\n[[entry]]\nkey = "not.in.13"\nclass = "value"\nreason = "test"\n',
        )
        self.assert_fails_with("manifest key 'not.in.13' was not extracted")

    def test_dispatch_limit_without_marker_fails(self) -> None:
        self.write("pallets/epoch/src/tests.rs", "#[test]\nfn no_marker() {}\n")
        self.assert_fails_with("dispatch-limit key 'IntakeQueue' has zero attached test markers")

    def test_dispatch_limit_requires_an_error_or_behavior(self) -> None:
        self.write(
            "tools/limit-coverage/registry.toml",
            BASE_MANIFEST.replace('error = "Epoch::IntakeFull"\n', ""),
        )
        self.assert_fails_with("requires exactly one of error/behavior")

    def test_dispatch_limit_rejects_both_error_and_behavior(self) -> None:
        self.write(
            "tools/limit-coverage/registry.toml",
            BASE_MANIFEST.replace(
                'error = "Epoch::IntakeFull"',
                'error = "Epoch::IntakeFull"\nbehavior = "overflow"',
            ),
        )
        self.assert_fails_with("requires exactly one of error/behavior")

    def test_error_binding_token_must_appear_in_marked_test(self) -> None:
        self.write(
            "pallets/epoch/src/tests.rs",
            BASE_RUST.replace('assert_eq!("IntakeFull", "IntakeFull");', "assert!(true);"),
        )
        self.assert_fails_with("does not contain binding token 'IntakeFull'")

    def test_behavior_binding_token_must_appear_in_marked_test(self) -> None:
        self.write(
            "tools/limit-coverage/registry.toml",
            BASE_MANIFEST.replace('error = "Epoch::IntakeFull"', 'behavior = "overflowed"'),
        )
        self.assert_fails_with("does not contain binding token 'overflowed'")

    def test_marker_on_ignored_test_fails(self) -> None:
        self.write(
            "pallets/epoch/src/tests.rs",
            BASE_RUST.replace("#[test]", "#[test]\n#[ignore]", 1),
        )
        self.assert_fails_with("attached to ignored test")

    def test_unknown_marker_key_fails(self) -> None:
        self.write(
            "pallets/epoch/src/tests.rs",
            BASE_RUST.replace("IntakeQueue", "IntakeQueue, not.in.13"),
        )
        self.assert_fails_with("marker references unknown 13 key 'not.in.13'")

    def test_marker_for_non_dispatch_key_fails(self) -> None:
        manifest = BASE_MANIFEST.replace(
            'class = "dispatch-limit"\nerror = "Epoch::IntakeFull"',
            'class = "value"\nreason = "test-only reclassification"',
        )
        self.write("tools/limit-coverage/registry.toml", manifest)
        self.assert_fails_with("marker key 'IntakeQueue' is classed 'value'")

    def test_unwired_marker_is_visible_but_does_not_satisfy_dispatch_coverage(self) -> None:
        manifest = BASE_MANIFEST.replace(
            'class = "dispatch-limit"\nerror = "Epoch::IntakeFull"',
            'class = "unwired"\nreason = "library evidence only"\nowner = "B10"',
        )
        self.write("tools/limit-coverage/registry.toml", manifest)
        self.assertEqual(self.failures(), [])

    def test_unattached_marker_fails(self) -> None:
        self.write(
            "pallets/epoch/src/tests.rs",
            "fn helper() {\n    // limit-coverage: IntakeQueue\n}\n",
        )
        self.assert_fails_with("marker is not attached to a test function")

    def test_param_bounds_key_missing_from_fixture_fails(self) -> None:
        self.write("tools/limit-coverage/genesis-keys.json", json.dumps([]) + "\n")
        self.assert_fails_with("param-bounds key 'epoch.length' is absent")

    def test_value_and_diagnostic_reasons_and_unwired_owner_are_required(self) -> None:
        manifest = BASE_MANIFEST.replace('reason = "13 §2 sample row"\n', "")
        manifest = manifest.replace(
            'class = "dispatch-limit"\nerror = "Epoch::IntakeFull"',
            'class = "unwired"\nreason = "test"',
        )
        self.write("tools/limit-coverage/registry.toml", manifest)
        self.assert_fails_with("value key 'MAX_NESTED' requires a reason")
        self.assert_fails_with("unwired key 'IntakeQueue' requires an owner")

    def test_unknown_owner_fails(self) -> None:
        manifest = BASE_MANIFEST.replace(
            'class = "dispatch-limit"\nerror = "Epoch::IntakeFull"',
            'class = "unwired"\nreason = "deferred"\nowner = "NOPE"',
        )
        self.write("tools/limit-coverage/registry.toml", manifest)
        self.assert_fails_with("unknown owner 'NOPE'")

    def test_unwired_exemption_expires_when_its_owner_completes(self) -> None:
        manifest = BASE_MANIFEST.replace(
            'class = "dispatch-limit"\nerror = "Epoch::IntakeFull"',
            'class = "unwired"\nreason = "deferred"\nowner = "B10"',
        )
        self.write("tools/limit-coverage/registry.toml", manifest)
        self.assertEqual(self.failures(), [])
        self.write(
            "PLAN.md",
            "| ID | Milestone | Spec | Depends | Status | Notes |\n"
            "|---|---|---|---|---|---|\n"
            "| B10 | Wiring | 13 | — | ✅ | done |\n",
        )
        self.assert_fails_with("unwired key 'IntakeQueue' names completed owner 'B10'")

    def test_consumer_binding_expires_when_b10_completes(self) -> None:
        manifest = BASE_MANIFEST.replace(
            'class = "param-bounds"\ngenesis = true',
            'class = "param-bounds"\ngenesis = true\n'
            'consumer_binding = "kernel-constant (B10)"',
        )
        self.write("tools/limit-coverage/registry.toml", manifest)
        self.assertEqual(self.failures(), [])
        self.write(
            "PLAN.md",
            "| ID | Milestone | Spec | Depends | Status | Notes |\n"
            "|---|---|---|---|---|---|\n"
            "| B10 | Wiring | 13 | — | ✅ | done |\n",
        )
        self.assert_fails_with("consumer_binding defers to B10, which is complete")


if __name__ == "__main__":
    unittest.main()

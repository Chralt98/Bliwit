from __future__ import annotations

import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[3]
GATE = REPO_ROOT / "tools/ci/supply-chain-gates.sh"


class SupplyChainSummaryTests(unittest.TestCase):
    def test_optional_summary_discloses_ignores_and_warning_counts(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            auditor = root / "cargo-audit"
            auditor.write_text(
                """#!/usr/bin/env python3
import json, os, sys
if '--version' in sys.argv:
    print('cargo-audit 0.22.2')
elif '--json' in sys.argv:
    keeper = os.path.basename(os.getcwd()) == 'keeper'
    print(json.dumps({
        'settings': {'ignore': [] if keeper else ['RUSTSEC-2026-0001']},
        'warnings': {'unmaintained': [] if keeper else [{}, {}], 'unsound': [] if keeper else [{}]},
    }))
raise SystemExit(0)
""",
                encoding="utf-8",
            )
            auditor.chmod(0o755)
            summary = root / "summary.json"
            environment = dict(os.environ)
            environment["BLEAVIT_AUDITOR"] = str(auditor)
            completed = subprocess.run(
                [str(GATE), "--summary-out", str(summary)],
                cwd=REPO_ROOT,
                env=environment,
                text=True,
                capture_output=True,
                check=False,
                timeout=120,
            )
            self.assertEqual(completed.returncode, 0, completed.stderr)
            document = json.loads(summary.read_text(encoding="utf-8"))
            self.assertEqual(document["schema"], "bleavit.supply-chain.v1")
            self.assertEqual(document["ignored_advisory_ids"], ["RUSTSEC-2026-0001"])
            self.assertEqual(document["workspaces"]["root"]["allowed_warning_count"], 3)
            self.assertEqual(document["workspaces"]["keeper"]["allowed_warning_count"], 0)


if __name__ == "__main__":
    unittest.main()

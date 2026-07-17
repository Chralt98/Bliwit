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
            # Stub the GHSA-only leg's scanner too, so the gate stays hermetic:
            # without this the script would fetch the pinned osv-scanner binary
            # over the network. Reporting no findings is the right stub here —
            # this test covers the cargo-audit summary, and the GHSA-only leg has
            # its own suite in tools/ci/tests/.
            osv_scanner = root / "osv-scanner"
            osv_scanner.write_text(
                """#!/usr/bin/env python3
import json
print(json.dumps({'results': [{'packages': [{
    'package': {'name': 'demo', 'version': '1.2.3'},
    'vulnerabilities': [{'id': 'GHSA-test-0000', 'aliases': [], 'summary': 'fixture'}],
}]}]}))
raise SystemExit(1)
""",
                encoding="utf-8",
            )
            osv_scanner.chmod(0o755)
            # A stub finding plus its matching waiver, rather than the committed
            # file: this test covers the summary, and pointing it at the real
            # waivers would couple it to whatever is waived today.
            ghsa_waivers = root / "ghsa-waivers.toml"
            ghsa_waivers.write_text(
                """\
[[waiver]]
id = "GHSA-test-0000"
package = "demo"
version = "1.2.3"
reason = "fixture"
blocked_by = "fixture pin"
clears_when = "never"
""",
                encoding="utf-8",
            )
            summary = root / "summary.json"
            environment = dict(os.environ)
            environment["BLEAVIT_AUDITOR"] = str(auditor)
            environment["BLEAVIT_OSV_SCANNER"] = str(osv_scanner)
            environment["BLEAVIT_GHSA_WAIVERS"] = str(ghsa_waivers)
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
            self.assertEqual(document["schema"], "bleavit.supply-chain.v2")
            self.assertEqual(document["ignored_advisory_ids"], ["RUSTSEC-2026-0001"])
            # SQ-135's disclosure property covers the GHSA-only waivers too:
            # cargo-audit's ignore list alone understates the accepted risk.
            self.assertEqual(
                document["waived_ghsa_only"],
                [{"id": "GHSA-test-0000", "package": "demo", "version": "1.2.3"}],
            )
            self.assertEqual(document["workspaces"]["root"]["allowed_warning_count"], 3)
            self.assertEqual(document["workspaces"]["keeper"]["allowed_warning_count"], 0)


if __name__ == "__main__":
    unittest.main()

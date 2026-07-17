"""Negative gates for the GHSA-only supply-chain leg (15 §4.5).

The leg exists because cargo-audit is structurally blind to advisories RustSec
does not carry. These tests pin the three behaviors that make it worth having:
it fires on a GHSA-only advisory, it stays quiet about everything RustSec
already covers (so it never duplicates .cargo/audit.toml or inherits its
informational warnings), and a waiver cannot outlive its advisory.
"""

from __future__ import annotations

import importlib.util
import json
import subprocess
import sys
import tempfile
import textwrap
import unittest
from pathlib import Path


SCRIPT = Path(__file__).resolve().parents[1] / "check-ghsa-only.py"
SPEC = importlib.util.spec_from_file_location("check_ghsa_only", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
checker = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = checker
SPEC.loader.exec_module(checker)

REPO_ROOT = Path(__file__).resolve().parents[3]
WAIVERS = REPO_ROOT / "tools/ci/ghsa-waivers.toml"

YAMUX = {
    "id": "GHSA-vxx9-2994-q338",
    "aliases": ["CVE-2026-32314"],
    "summary": "Yamux vulnerable to remote Panic via malformed Data frame",
}
# The shape RustSec-carried advisories take in OSV: the GHSA record cross-links
# the RUSTSEC id, which is what makes it cargo-audit's responsibility, not ours.
WASMTIME_GHSA = {
    "id": "GHSA-jhxm-h53p-jm7w",
    "aliases": ["CVE-2026-34971", "RUSTSEC-2026-0096"],
    "summary": "sandbox escape",
}
WASMTIME_RUSTSEC = {
    "id": "RUSTSEC-2026-0096",
    "aliases": ["CVE-2026-34971", "GHSA-jhxm-h53p-jm7w"],
    "summary": "sandbox escape",
}
LRU_INFORMATIONAL = {
    "id": "RUSTSEC-2026-0002",
    "aliases": ["GHSA-rhfx-m35p-ff5j"],
    "summary": "IterMut violates Stacked Borrows",
}


def report(*packages: tuple[str, str, list[dict]]) -> dict:
    return {
        "results": [
            {
                "packages": [
                    {"package": {"name": name, "version": version}, "vulnerabilities": vulns}
                    for name, version, vulns in packages
                ]
            }
        ]
    }


def fake_scanner(directory: Path, document: dict) -> Path:
    """A stub standing in for the pinned osv-scanner, emitting `document`."""
    path = directory / "osv-scanner"
    path.write_text(
        textwrap.dedent(
            f"""\
            #!/usr/bin/env python3
            import json
            print(json.dumps({document!r}))
            # 1 = scanned, findings present; 0 = scanned, none. Both mean the scan
            # ran, which is what the checker requires.
            raise SystemExit(1)
            """
        ),
        encoding="utf-8",
    )
    path.chmod(0o755)
    return path


def run(document: dict, waivers: Path) -> subprocess.CompletedProcess:
    with tempfile.TemporaryDirectory() as tmp:
        directory = Path(tmp)
        scanner = fake_scanner(directory, document)
        lockfile = directory / "Cargo.lock"
        lockfile.write_text("", encoding="utf-8")
        return subprocess.run(
            [
                sys.executable,
                str(SCRIPT),
                "--scanner",
                str(scanner),
                "--waivers",
                str(waivers),
                "--lockfile",
                str(lockfile),
            ],
            text=True,
            capture_output=True,
            check=False,
        )


def waiver_file(directory: Path, body: str) -> Path:
    path = directory / "waivers.toml"
    path.write_text(textwrap.dedent(body), encoding="utf-8")
    return path


MINIMAL_WAIVER = """\
    [[waiver]]
    id = "GHSA-vxx9-2994-q338"
    aliases = ["CVE-2026-32314"]
    package = "yamux"
    version = "0.12.1"
    reason = "linked but never instantiated"
    blocked_by = "libp2p-yamux 0.46.0"
    clears_when = "upstream drops yamux 0.12"
"""


class RustSecCoverageTests(unittest.TestCase):
    def primaries(self, *vulns: dict) -> set[str]:
        return checker.rustsec_primary_ids(list(vulns))

    def test_rustsec_id_is_cargo_audits_responsibility(self) -> None:
        p = self.primaries(WASMTIME_RUSTSEC, WASMTIME_GHSA)
        self.assertTrue(checker.rustsec_covered(WASMTIME_RUSTSEC, p))

    def test_ghsa_aliasing_a_real_rustsec_record_is_covered(self) -> None:
        p = self.primaries(WASMTIME_RUSTSEC, WASMTIME_GHSA)
        self.assertTrue(checker.rustsec_covered(WASMTIME_GHSA, p))

    def test_ghsa_with_no_rustsec_alias_is_the_blind_spot(self) -> None:
        self.assertFalse(checker.rustsec_covered(YAMUX, self.primaries(YAMUX)))

    def test_alias_to_a_rustsec_id_of_a_DIFFERENT_crate_does_not_count(self) -> None:
        """The fail-open this leg would otherwise have shipped with.

        hickory-proto 0.25.2's GHSA-3v94-mw7p-v465 really does alias
        RUSTSEC-2026-0120, an advisory keyed to hickory-net. cargo-audit scanning
        hickory-proto will never fire it, so treating the alias as proof of
        coverage would hand the finding to a gate that cannot see it. Only a
        RUSTSEC id OSV returns as a record FOR THIS PACKAGE counts.
        """
        foreign = {
            "id": "GHSA-hypothetical",
            "aliases": ["RUSTSEC-2026-0120"],  # belongs to hickory-net
            "summary": "GHSA-only advisory that name-drops another crate's RUSTSEC id",
        }
        primaries = self.primaries(foreign)
        self.assertEqual(primaries, set())
        self.assertFalse(checker.rustsec_covered(foreign, primaries))

    def test_real_hickory_shape_stays_covered_via_its_genuine_rustsec_record(self) -> None:
        """The same GHSA also aliases RUSTSEC-2026-0118, which IS a hickory-proto
        record — so it stays cargo-audit's job and must not be double-reported."""
        rustsec_0118 = {"id": "RUSTSEC-2026-0118", "aliases": ["GHSA-3v94-mw7p-v465", "RUSTSEC-2026-0120"]}
        ghsa_3v94 = {"id": "GHSA-3v94-mw7p-v465", "aliases": ["RUSTSEC-2026-0118", "RUSTSEC-2026-0120"]}
        p = self.primaries(rustsec_0118, ghsa_3v94)
        self.assertTrue(checker.rustsec_covered(ghsa_3v94, p))


class GateTests(unittest.TestCase):
    def test_unwaived_ghsa_only_advisory_fails(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            waivers = waiver_file(Path(tmp), "")
            completed = run(report(("yamux", "0.12.1", [YAMUX])), waivers)
        self.assertEqual(completed.returncode, 1, completed.stdout)
        self.assertIn("GHSA-vxx9-2994-q338", completed.stderr)

    def test_waived_ghsa_only_advisory_passes(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            waivers = waiver_file(Path(tmp), MINIMAL_WAIVER)
            completed = run(report(("yamux", "0.12.1", [YAMUX])), waivers)
        self.assertEqual(completed.returncode, 0, completed.stderr)

    def test_rustsec_carried_findings_are_left_to_cargo_audit(self) -> None:
        """No waiver needed for wasmtime/lru: audit.toml and its informational
        classification own them. This is what keeps the two lists from
        duplicating — and keeps the keeper workspace waiver-free."""
        with tempfile.TemporaryDirectory() as tmp:
            waivers = waiver_file(Path(tmp), "")
            completed = run(
                report(
                    ("wasmtime", "35.0.0", [WASMTIME_GHSA, WASMTIME_RUSTSEC]),
                    ("lru", "0.12.5", [LRU_INFORMATIONAL]),
                ),
                waivers,
            )
        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertIn("GHSA-only findings (invisible to cargo-audit): 0", completed.stdout)

    def test_waiver_cannot_outlive_its_advisory(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            waivers = waiver_file(Path(tmp), MINIMAL_WAIVER)
            completed = run(report(("yamux", "0.13.10", [])), waivers)
        self.assertEqual(completed.returncode, 1, completed.stdout)
        self.assertIn("match no current finding", completed.stderr)

    def test_waiver_does_not_carry_over_to_another_version(self) -> None:
        """The yamux waiver's justification is about 0.12.1 specifically. A bump
        to another still-vulnerable version must demand a fresh triage, not
        inherit the old reasoning."""
        with tempfile.TemporaryDirectory() as tmp:
            waivers = waiver_file(Path(tmp), MINIMAL_WAIVER)  # waives yamux 0.12.1
            completed = run(report(("yamux", "0.12.0", [YAMUX])), waivers)
        self.assertEqual(completed.returncode, 1, completed.stdout)
        self.assertIn("UNWAIVED", completed.stdout)

    def test_waiver_does_not_carry_over_to_another_package(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            waivers = waiver_file(Path(tmp), MINIMAL_WAIVER)  # waives yamux
            completed = run(report(("other-crate", "0.12.1", [YAMUX])), waivers)
        self.assertEqual(completed.returncode, 1, completed.stdout)
        self.assertIn("UNWAIVED", completed.stdout)

    def test_both_failure_classes_are_reported_together(self) -> None:
        """A run that both leaves something untriaged and carries a dead waiver
        should say so once, not reveal the second only after the first is fixed."""
        with tempfile.TemporaryDirectory() as tmp:
            waivers = waiver_file(Path(tmp), MINIMAL_WAIVER)  # yamux 0.12.1: will go stale
            completed = run(report(("other-crate", "9.9.9", [YAMUX])), waivers)
        self.assertEqual(completed.returncode, 1, completed.stdout)
        self.assertIn("with no waiver", completed.stderr)
        self.assertIn("match no current finding", completed.stderr)

    def test_duplicate_waiver_entries_are_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            waivers = waiver_file(Path(tmp), MINIMAL_WAIVER + MINIMAL_WAIVER)
            completed = run(report(("yamux", "0.12.1", [YAMUX])), waivers)
        self.assertEqual(completed.returncode, 1, completed.stdout)
        self.assertIn("duplicate waiver", completed.stderr)

    def test_waiver_missing_a_required_field_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            waivers = waiver_file(
                Path(tmp),
                """\
                [[waiver]]
                id = "GHSA-vxx9-2994-q338"
                package = "yamux"
                version = "0.12.1"
                """,
            )
            completed = run(report(("yamux", "0.12.1", [YAMUX])), waivers)
        self.assertEqual(completed.returncode, 1, completed.stdout)
        self.assertIn("missing", completed.stderr)


class CommittedWaiverTests(unittest.TestCase):
    def test_committed_waivers_parse_and_carry_their_justification(self) -> None:
        waivers = checker.load_waivers(WAIVERS)
        self.assertIn(("GHSA-vxx9-2994-q338", "yamux", "0.12.1"), waivers)
        for key, waiver in waivers.items():
            self.assertTrue(waiver["blocked_by"].strip(), key)
            self.assertTrue(waiver["clears_when"].strip(), key)


class FailClosedTests(unittest.TestCase):
    def test_a_scanner_that_produces_nothing_fails_the_gate(self) -> None:
        """osv-scanner exits non-zero with empty stdout when it cannot reach the
        OSV API. Treating that as "no findings" would turn a broken network into
        a silently green security gate; it must be a hard error instead."""
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            scanner = directory / "osv-scanner"
            scanner.write_text(
                "#!/usr/bin/env python3\n"
                "import sys\n"
                "print('error when retrieving vulns: max retries exceeded', file=sys.stderr)\n"
                "raise SystemExit(127)\n",
                encoding="utf-8",
            )
            scanner.chmod(0o755)
            lockfile = directory / "Cargo.lock"
            lockfile.write_text("", encoding="utf-8")
            waivers = waiver_file(directory, "")
            completed = subprocess.run(
                [
                    sys.executable, str(SCRIPT),
                    "--scanner", str(scanner),
                    "--waivers", str(waivers),
                    "--lockfile", str(lockfile),
                ],
                text=True,
                capture_output=True,
                check=False,
            )
        self.assertNotEqual(completed.returncode, 0)
        self.assertIn("refusing to", completed.stderr)

    def test_a_failed_scan_is_never_read_as_a_clean_one(self) -> None:
        """osv-scanner exit 127/128 (general error / no package sources) can be
        accompanied by a JSON envelope. Accepting it would let a mistyped
        lockfile path or a dead OSV API report zero findings and pass."""
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            scanner = directory / "osv-scanner"
            scanner.write_text(
                "#!/usr/bin/env python3\n"
                "import json, sys\n"
                "print(json.dumps({'results': []}))\n"
                "print('no package sources found', file=sys.stderr)\n"
                "raise SystemExit(128)\n",
                encoding="utf-8",
            )
            scanner.chmod(0o755)
            lockfile = directory / "Cargo.lock"
            lockfile.write_text("", encoding="utf-8")
            waivers = waiver_file(directory, "")
            completed = subprocess.run(
                [
                    sys.executable, str(SCRIPT),
                    "--scanner", str(scanner),
                    "--waivers", str(waivers),
                    "--lockfile", str(lockfile),
                ],
                text=True,
                capture_output=True,
                check=False,
            )
        self.assertNotEqual(completed.returncode, 0)
        self.assertIn("failed to scan", completed.stderr)

    def test_non_json_scanner_output_fails_the_gate(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            scanner = directory / "osv-scanner"
            scanner.write_text("#!/bin/sh\necho 'not json'\n", encoding="utf-8")
            scanner.chmod(0o755)
            lockfile = directory / "Cargo.lock"
            lockfile.write_text("", encoding="utf-8")
            waivers = waiver_file(directory, "")
            completed = subprocess.run(
                [
                    sys.executable, str(SCRIPT),
                    "--scanner", str(scanner),
                    "--waivers", str(waivers),
                    "--lockfile", str(lockfile),
                ],
                text=True,
                capture_output=True,
                check=False,
            )
        self.assertNotEqual(completed.returncode, 0)
        self.assertIn("not JSON", completed.stderr)


class CompatParserTests(unittest.TestCase):
    """The 3.10 fallback must agree with tomllib on the committed file."""

    def test_hash_inside_a_quoted_value_is_not_a_comment(self) -> None:
        rows = checker.parse_waivers_toml_compat(
            '[[waiver]]\n'
            'id = "GHSA-x"  # trailing comment\n'
            'package = "p"\n'
            'version = "1"\n'
            'reason = "regressed by #76"\n'
            'blocked_by = "b"\n'
            'clears_when = "c"\n'
        )
        self.assertEqual(rows[0]["reason"], "regressed by #76")
        self.assertEqual(rows[0]["id"], "GHSA-x")

    def test_compat_parser_matches_the_committed_waivers(self) -> None:
        rows = checker.parse_waivers_toml_compat(WAIVERS.read_text(encoding="utf-8"))
        self.assertEqual([row["id"] for row in rows], ["GHSA-vxx9-2994-q338"])
        self.assertEqual(rows[0]["aliases"], ["CVE-2026-32314"])
        self.assertEqual(rows[0]["package"], "yamux")

    @unittest.skipIf(checker.tomllib is None, "tomllib requires Python 3.11+")
    def test_compat_parser_agrees_with_tomllib(self) -> None:
        with WAIVERS.open("rb") as handle:
            expected = checker.tomllib.load(handle)["waiver"]
        actual = checker.parse_waivers_toml_compat(WAIVERS.read_text(encoding="utf-8"))
        self.assertEqual(actual, expected)


if __name__ == "__main__":
    unittest.main()

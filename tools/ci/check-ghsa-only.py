#!/usr/bin/env python3
"""Fail on GHSA-only advisories — the delta the 15 §4.5 RustSec gate cannot see.

`cargo-audit` gates against the RustSec advisory-db. For crates.io the GitHub
Advisory Database is a strict superset: an advisory can carry no RUSTSEC id at
all (yamux GHSA-vxx9-2994-q338 / CVE-2026-32314, a HIGH remote panic, is the
worked example), and cargo-audit is then structurally blind to it.

This checker consumes `osv-scanner` JSON — OSV aggregates both databases — and
enforces exactly the complement of cargo-audit's reach:

  * a finding whose id or aliases include a RUSTSEC id is cargo-audit's to
    gate (with `.cargo/audit.toml` for the pin-forced exceptions). Skipped
    here, so this gate never duplicates that list and never inherits the
    informational (`unmaintained`/`unsound`) warnings RustSec classifies and
    OSV does not.
  * a finding with no RUSTSEC id anywhere is GHSA-only. It MUST be waived in
    `tools/ci/ghsa-waivers.toml` with a stated blocking pin, or the gate fails.

A waiver that matches no finding also fails (stale-waiver leg): an exemption
can never outlive the advisory that justified it — the limit-coverage registry
discipline (SQ-155) applied to supply chain.
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from pathlib import Path

try:
    import tomllib
except ModuleNotFoundError:  # Python 3.10 compatibility for the local quality gate.
    tomllib = None  # type: ignore[assignment]


def strip_comment(raw: str) -> str:
    """Drop a trailing `#` comment without cutting a `#` inside a quoted value.

    A waiver reason naming an issue (`... see #76`) would otherwise be silently
    truncated by a naive split, and silently is the bad part: the CI path uses
    tomllib and would disagree with this parser.
    """
    out: list[str] = []
    in_string = False
    escaped = False
    for char in raw:
        if in_string:
            if escaped:
                escaped = False
            elif char == "\\":
                escaped = True
            elif char == '"':
                in_string = False
        elif char == '"':
            in_string = True
        elif char == "#":
            break
        out.append(char)
    return "".join(out)


def parse_waivers_toml_compat(text: str) -> list[dict]:
    """Parse the `[[waiver]]` subset of TOML this file uses.

    Deliberately dependency-free and deliberately narrow, matching
    tools/limit-coverage/check-limit-coverage.py: CI runs Python 3.12 and takes
    the tomllib path above, so this only backs the local gate on 3.10. It
    understands exactly what ghsa-waivers.toml contains — `[[waiver]]` tables of
    basic strings and one string array — and refuses anything else rather than
    guessing.
    """
    waivers: list[dict] = []
    current: dict | None = None
    for raw in text.splitlines():
        line = strip_comment(raw).strip()
        if not line:
            continue
        if line == "[[waiver]]":
            current = {}
            waivers.append(current)
            continue
        match = re.fullmatch(r'([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(.+)', line)
        if not match or current is None:
            raise SystemExit(f"ghsa-waivers.toml: unsupported line for the 3.10 compat parser: {raw!r}")
        key, value = match.group(1), match.group(2).strip()
        if value.startswith("["):
            items = re.findall(r'"([^"]*)"', value)
            current[key] = items
        elif value.startswith('"') and value.endswith('"') and len(value) >= 2:
            current[key] = value[1:-1].replace('\\"', '"')
        else:
            raise SystemExit(f"ghsa-waivers.toml: unsupported value for the 3.10 compat parser: {raw!r}")
    return waivers


def waiver_key(identifier: str, package: str, version: str) -> tuple[str, str, str]:
    """Waivers bind to (advisory, package, version), never to the advisory alone.

    `package`/`version` are the triage's subject: the yamux waiver's whole
    justification is that *0.12.1* is linked-but-never-instantiated. Keying on
    the id alone would let that reasoning silently cover a later version, or the
    same advisory reached through a different crate — an exemption applying
    somewhere nobody assessed. Instead the version bump makes the waiver stale
    and the gate demands a fresh triage.
    """
    return (identifier, package, version)


def load_waivers(path: Path) -> dict[tuple[str, str, str], dict]:
    if tomllib is None:
        rows = parse_waivers_toml_compat(path.read_text(encoding="utf-8"))
    else:
        with path.open("rb") as fh:
            rows = tomllib.load(fh).get("waiver", [])
    required = {"id", "package", "version", "reason", "blocked_by", "clears_when"}
    waivers: dict[tuple[str, str, str], dict] = {}
    for row in rows:
        missing = required - set(row)
        if missing:
            raise SystemExit(f"ghsa-waivers.toml: waiver {row.get('id', '?')} is missing {sorted(missing)}")
        key = waiver_key(row["id"], row["package"], row["version"])
        if key in waivers:
            raise SystemExit(f"ghsa-waivers.toml: duplicate waiver for {key}")
        waivers[key] = row
    return waivers


def rustsec_primary_ids(vulns: list[dict]) -> set[str]:
    """RUSTSEC ids OSV returns as records *for this package*.

    These — and only these — are what cargo-audit will actually fire on when it
    scans this crate.
    """
    return {v["id"] for v in vulns if v.get("id", "").startswith("RUSTSEC-")}


def rustsec_covered(vuln: dict, primaries: set[str]) -> bool:
    """True when cargo-audit can really see this advisory for this package.

    Membership in `primaries` is the test, NOT merely "the record mentions some
    RUSTSEC id". An alias can name a RustSec advisory that is keyed to a
    *different crate* and so will never fire here: hickory-proto 0.25.2's
    GHSA-3v94-mw7p-v465 aliases RUSTSEC-2026-0120, which belongs to hickory-net.
    Trusting the alias string would hand such a finding to cargo-audit on the
    assumption it is covered, when cargo-audit will never report it — a silent
    fail-open, and precisely the class of gap this whole leg exists to close.
    (That specific advisory is fine: it also aliases RUSTSEC-2026-0118, a real
    hickory-proto advisory that does fire.)
    """
    if vuln.get("id", "").startswith("RUSTSEC-"):
        return True
    return any(alias in primaries for alias in (vuln.get("aliases") or []))


# osv-scanner v2 exit codes. 0 and 1 both mean "the lockfile was scanned" — 1
# only adds "and something was found", which is this checker's input, not its
# verdict. Every other code is a failure to scan (127 general error, 128 no
# package sources), and MUST NOT be read as "nothing found": that would turn an
# unreachable OSV API or a mistyped lockfile path into a silently green security
# gate. Verified against v2.4.0: a vulnerable lockfile exits 1 with JSON; a
# missing, empty, or package-less lockfile exits 127 with no stdout at all.
SCAN_OK = frozenset({0, 1})


def scan(scanner: str, lockfile: Path) -> dict:
    proc = subprocess.run(
        [scanner, "scan", "source", f"--lockfile={lockfile}", "--format=json"],
        capture_output=True,
        text=True,
    )
    if proc.returncode not in SCAN_OK:
        sys.exit(
            f"osv-scanner failed to scan {lockfile} (exit {proc.returncode}); refusing to\n"
            f"treat a failed scan as a clean one:\n{proc.stderr.strip()[-2000:]}"
        )
    if not proc.stdout.strip():
        sys.exit(f"osv-scanner produced no output for {lockfile}:\n{proc.stderr}")
    try:
        return json.loads(proc.stdout)
    except json.JSONDecodeError as exc:
        sys.exit(f"osv-scanner output for {lockfile} is not JSON ({exc}):\n{proc.stdout[:2000]}")


def ghsa_only_findings(report: dict, lockfile: Path) -> list[dict]:
    out = []
    for result in report.get("results", []):
        for pkg in result.get("packages", []):
            name = pkg["package"]["name"]
            version = pkg["package"]["version"]
            vulns = pkg.get("vulnerabilities", [])
            primaries = rustsec_primary_ids(vulns)
            for vuln in vulns:
                if rustsec_covered(vuln, primaries):
                    continue
                out.append(
                    {
                        "id": vuln.get("id", ""),
                        "aliases": sorted(vuln.get("aliases") or []),
                        "package": name,
                        "version": version,
                        "lockfile": str(lockfile),
                        "summary": (vuln.get("summary") or "").strip(),
                    }
                )
    return out


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--scanner", required=True, help="path to the pinned osv-scanner binary")
    ap.add_argument("--waivers", required=True, type=Path)
    ap.add_argument("--lockfile", action="append", required=True, type=Path)
    args = ap.parse_args()

    waivers = load_waivers(args.waivers)

    findings: list[dict] = []
    for lockfile in args.lockfile:
        findings.extend(ghsa_only_findings(scan(args.scanner, lockfile), lockfile))

    matched = {waiver_key(f["id"], f["package"], f["version"]) for f in findings}
    unwaived = [f for f in findings if waiver_key(f["id"], f["package"], f["version"]) not in waivers]
    stale = sorted(set(waivers) - matched)

    print(f"GHSA-only findings (invisible to cargo-audit): {len(findings)}")
    for f in sorted(findings, key=lambda f: (f["package"], f["id"])):
        key = waiver_key(f["id"], f["package"], f["version"])
        state = "UNWAIVED" if key not in waivers else "waived"
        print(f"  [{state}] {f['package']} {f['version']} — {f['id']} {f['aliases']}")
        if key in waivers:
            print(f"             blocked_by: {waivers[key]['blocked_by']}")
            print(f"             clears_when: {waivers[key]['clears_when']}")

    # Report both classes before returning: a run that fixed one and introduced
    # the other should show both, not hide the second behind another red run.
    if unwaived:
        print(
            "\nFAIL: GHSA-only advisories with no waiver. cargo-audit cannot see these —\n"
            "triage each one and record it in tools/ci/ghsa-waivers.toml with the pin\n"
            "that forces it and the condition that clears it, or fix the dependency.",
            file=sys.stderr,
        )
        for f in unwaived:
            print(f"  {f['package']} {f['version']} — {f['id']} {f['aliases']}: {f['summary']}", file=sys.stderr)

    if stale:
        print(
            "\nFAIL: waivers that match no current finding. Whatever they excuse is gone\n"
            "or moved — the dependency changed version, the advisory was withdrawn or\n"
            "re-keyed. Delete the entry, or re-triage it against what is there now, so\n"
            "the exemption cannot outlive its justification.",
            file=sys.stderr,
        )
        for key in stale:
            print(f"  {key[0]} ({key[1]} {key[2]})", file=sys.stderr)

    if unwaived or stale:
        return 1

    print("\nOK: every GHSA-only advisory is triaged and every waiver still applies.")
    return 0


if __name__ == "__main__":
    sys.exit(main())

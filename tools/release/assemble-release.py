#!/usr/bin/env python3
"""Verify and content-address Bleavit's complete release artifact set."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import subprocess
import sys
import tarfile
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent))

from release_common import (
    git_value,
    repo_root,
    safe_filename,
    sha256_file,
    source_date_epoch,
    write_json,
)


@dataclass(frozen=True)
class Candidate:
    kind: str
    path: Path
    source: str


def validate_build_info(info: dict[str, Any]) -> list[str]:
    errors: list[str] = []
    required_strings = (
        "schema",
        "git_commit",
        "toolchain",
        "host_triple",
        "cargo_version",
        "rustc_version",
        "recipe",
        "reproducibility_scope",
    )
    for key in required_strings:
        if not isinstance(info.get(key), str) or not info[key]:
            errors.append(f"build-info.{key} must be a non-empty string")
    wasm = info.get("wasm")
    if not isinstance(wasm, dict):
        errors.append("build-info.wasm must be an object")
    else:
        digest = wasm.get("sha256")
        if not is_sha256(digest):
            errors.append("build-info.wasm.sha256 must be a SHA-256 hex digest")
        if not isinstance(wasm.get("size"), int) or wasm["size"] < 0:
            errors.append("build-info.wasm.size must be a non-negative integer")
    return errors


def is_sha256(value: Any) -> bool:
    return (
        isinstance(value, str)
        and len(value) == 64
        and all(character in "0123456789abcdef" for character in value)
    )


def deterministic_tar_xz(source_dir: Path, output: Path, epoch: int) -> None:
    files = sorted(path for path in source_dir.rglob("*") if path.is_file())
    output.parent.mkdir(parents=True, exist_ok=True)
    with tarfile.open(output, mode="w:xz", preset=9) as archive:
        for path in files:
            relative = path.relative_to(source_dir)
            info = archive.gettarinfo(str(path), arcname=str(relative))
            info.uid = 0
            info.gid = 0
            info.uname = ""
            info.gname = ""
            info.mtime = epoch
            info.mode = 0o644
            with path.open("rb") as handle:
                archive.addfile(info, handle)


def milestone_from_blocker(blocker: str | None) -> str:
    if blocker and blocker.startswith("SQ-101"):
        return "SQ-101 (B4 follow-up)"
    if not blocker:
        return "B8"
    first = blocker.split()[0].rstrip(";,")
    return first if first and first[0].isalpha() else "B8"


def parse_args() -> argparse.Namespace:
    root = repo_root()
    parser = argparse.ArgumentParser()
    parser.add_argument("--output-dir", type=Path, default=root / "release-work")
    parser.add_argument("--dist-dir", type=Path)
    parser.add_argument("--runtime-dir", type=Path, default=root / "release-work/runtime")
    parser.add_argument("--chain-spec-dir", type=Path, default=root / "deploy/chain-specs/out")
    parser.add_argument("--fixtures-dir", type=Path, default=root / "release-work/chainhead")
    parser.add_argument(
        "--surface-manifest",
        type=Path,
        default=Path(__file__).resolve().with_name("surface-manifest.json"),
    )
    parser.add_argument("--zombienet-dir", type=Path, default=root / "zombienet")
    parser.add_argument("--chopsticks-dir", type=Path, default=root / "chopsticks")
    parser.add_argument(
        "--environments",
        type=Path,
        default=Path(__file__).resolve().with_name("environments.json"),
    )
    parser.add_argument(
        "--chain-spec-validator",
        type=Path,
        default=root / "tools/deploy/validate-chain-spec.py",
    )
    parser.add_argument(
        "--reference-vectors",
        type=Path,
        default=root / "reference-model/fixtures/vectors.json",
    )
    parser.add_argument("--sweep-dir", type=Path, default=root / "release-work/sweep")
    parser.add_argument("--tag", default=os.environ.get("GITHUB_REF_NAME"))
    parser.add_argument(
        "--commit",
        help="the release commit every artifact must bind to (defaults to HEAD)",
    )
    parser.add_argument(
        "--supply-chain-result", choices=("passed", "failed", "not-run"), required=True
    )
    parser.add_argument("--supply-chain-evidence", default="tools/ci/supply-chain-gates.sh")
    parser.add_argument(
        "--supply-chain-summary",
        type=Path,
        default=root / "release-work/supply-chain-summary.json",
    )
    parser.add_argument("--allow-missing", action="store_true")
    return parser.parse_args()


def add_gap(gaps: list[dict[str, str]], gap_id: str, owner: str, reason: str) -> None:
    gaps.append({"id": gap_id, "owner": owner, "reason": reason})


def add_corruption(
    corruptions: list[dict[str, str]], corruption_id: str, reason: str
) -> None:
    corruptions.append({"id": corruption_id, "owner": "B8", "reason": reason})


def load_json(path: Path) -> dict[str, Any]:
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise ValueError(f"{path} must contain a JSON object")
    return value


def load_manifest_release_blockers(path: Path) -> list[dict[str, str]]:
    """Load explicit readiness blockers that apply even when every surface records."""
    manifest = load_json(path)
    rows = manifest.get("release_blockers", [])
    if not isinstance(rows, list):
        raise ValueError("surface manifest release_blockers must be an array")
    blockers: list[dict[str, str]] = []
    for index, row in enumerate(rows):
        if not isinstance(row, dict):
            raise ValueError(f"surface manifest release_blockers[{index}] must be an object")
        blocker: dict[str, str] = {}
        for field in ("id", "owner", "reason"):
            value = row.get(field)
            if not isinstance(value, str) or not value:
                raise ValueError(
                    f"surface manifest release_blockers[{index}].{field} must be a non-empty string"
                )
            blocker[field] = value
        blockers.append(blocker)
    return blockers


def decode_hex(value: Any, label: str) -> bytes:
    if not isinstance(value, str) or not value.startswith("0x"):
        raise ValueError(f"{label} must be a 0x-prefixed hex string")
    try:
        return bytes.fromhex(value[2:])
    except ValueError as error:
        raise ValueError(f"{label} contains invalid hex") from error


def chain_spec_wasm_sha256(spec: dict[str, Any]) -> str:
    genesis = spec.get("genesis")
    runtime_genesis = genesis.get("runtimeGenesis") if isinstance(genesis, dict) else None
    code = runtime_genesis.get("code") if isinstance(runtime_genesis, dict) else None
    if code is None and isinstance(genesis, dict):
        # Raw chain specs (the paseo/polkadot artifact form) carry the runtime
        # under the well-known `:code` storage key instead.
        raw = genesis.get("raw")
        top = raw.get("top") if isinstance(raw, dict) else None
        code = top.get("0x3a636f6465") if isinstance(top, dict) else None
        return hashlib.sha256(decode_hex(code, 'genesis.raw.top[":code"]')).hexdigest()
    return hashlib.sha256(decode_hex(code, "genesis.runtimeGenesis.code")).hexdigest()


def validate_runtime_binding(
    runtime_dir: Path,
    build_info: dict[str, Any],
    runtime_info: dict[str, Any],
) -> tuple[str | None, list[str]]:
    errors: list[str] = []
    wasm_path = runtime_dir / "runtime.wasm"
    if not wasm_path.is_file():
        return None, errors
    actual = sha256_file(wasm_path)
    values = {
        "build-info.wasm.sha256": build_info.get("wasm", {}).get("sha256"),
        "runtime-info.wasm_sha256": runtime_info.get("wasm_sha256"),
        "runtime-info.wasm_file_sha256": runtime_info.get("wasm_file_sha256"),
        "runtime-info.on_chain_wasm_sha256": runtime_info.get("on_chain_wasm_sha256"),
    }
    for label, value in values.items():
        if value != actual:
            errors.append(f"{label} {value!r} does not match shipped runtime.wasm sha256 {actual}")
    # The shipped metadata blob must be the one extracted from the booted
    # runtime, or descriptor generation targets the wrong runtime.
    metadata_path = runtime_dir / "metadata.scale"
    if metadata_path.is_file():
        metadata_actual = sha256_file(metadata_path)
        declared = runtime_info.get("metadata_sha256")
        if declared != metadata_actual:
            errors.append(
                f"runtime-info.metadata_sha256 {declared!r} does not match shipped "
                f"metadata.scale sha256 {metadata_actual}"
            )
    return actual, errors


def validate_environment_inventory(document: dict[str, Any]) -> list[str]:
    errors: list[str] = []
    if document.get("schema") != "bleavit.environments.v1":
        errors.append("environment inventory schema must be bleavit.environments.v1")
    environments = document.get("environments")
    if not isinstance(environments, list) or not environments:
        return [*errors, "environment inventory environments must be a non-empty array"]
    identifiers: set[str] = set()
    chain_specs: set[str] = set()
    for index, item in enumerate(environments):
        label = f"environments[{index}]"
        if not isinstance(item, dict):
            errors.append(f"{label} must be an object")
            continue
        identifier = item.get("id")
        chain_spec = item.get("chain_spec")
        if not isinstance(identifier, str) or not identifier:
            errors.append(f"{label}.id must be a non-empty string")
        elif identifier in identifiers:
            errors.append(f"duplicate environment id {identifier}")
        else:
            identifiers.add(identifier)
        if (
            not isinstance(chain_spec, str)
            or not chain_spec.endswith(".json")
            or PurePosixPath(chain_spec).name != chain_spec
        ):
            errors.append(f"{label}.chain_spec must be a JSON basename")
        elif chain_spec in chain_specs:
            errors.append(f"duplicate environment chain_spec {chain_spec}")
        else:
            chain_specs.add(chain_spec)
        for key in ("live", "required"):
            if type(item.get(key)) is not bool:
                errors.append(f"{label}.{key} must be boolean")
        if item.get("live"):
            # Only the paseo/polkadot validator profiles enforce the 02 §10
            # bootnode thresholds; a "live" dev/local row would bypass them.
            if identifier not in ("paseo", "polkadot"):
                errors.append(
                    f"{label} cannot be live: only paseo/polkadot profiles enforce"
                    " the 02 §10 bootnode thresholds"
                )
            bootnodes = item.get("bootnodes")
            if not isinstance(bootnodes, str) or PurePosixPath(bootnodes).is_absolute() or ".." in PurePosixPath(bootnodes).parts:
                errors.append(f"{label}.bootnodes must be a safe repository-relative path when live")
            elif isinstance(identifier, str) and bootnodes != f"deploy/chain-specs/bootnodes.{identifier}.json":
                errors.append(
                    f"{label}.bootnodes must be deploy/chain-specs/bootnodes.{identifier}.json"
                )
    return errors


def validate_chain_spec_profile(validator: Path, profile: str, spec: Path) -> list[str]:
    completed = subprocess.run(
        [sys.executable, str(validator), "--profile", profile, str(spec)],
        text=True,
        capture_output=True,
        check=False,
    )
    if completed.returncode == 0:
        return []
    detail = (completed.stderr or completed.stdout).strip()
    return [f"{profile} chain-spec validation failed: {detail or 'validator returned nonzero'}"]


def validate_run_evidence(
    directory: Path,
    suite: str,
    runtime_wasm_sha256: str,
    release_commit: str,
) -> list[str]:
    errors: list[str] = []
    evidence_path = directory / "run-evidence.json"
    if not evidence_path.is_file():
        return ["run-evidence.json is missing"]
    try:
        evidence = load_json(evidence_path)
    except (OSError, ValueError, json.JSONDecodeError) as error:
        return [f"run-evidence.json is invalid: {error}"]
    if evidence.get("schema") != "bleavit.env-evidence.v1":
        errors.append("schema must be bleavit.env-evidence.v1")
    if evidence.get("suite") != suite:
        errors.append(f"suite must be {suite!r}")
    if evidence.get("runtime_wasm_sha256") != runtime_wasm_sha256:
        errors.append("runtime_wasm_sha256 does not match this release")
    if evidence.get("recorded_at_commit") != release_commit:
        errors.append("recorded_at_commit does not match this release commit")
    suites_run = evidence.get("suites_run")
    if not isinstance(suites_run, list) or not suites_run:
        errors.append("suites_run must be a non-empty array")
    else:
        for index, row in enumerate(suites_run):
            if (
                not isinstance(row, dict)
                or not isinstance(row.get("name"), str)
                or not row["name"]
                or row.get("result") != "pass"
            ):
                errors.append(f"suites_run[{index}] must name a passing suite")
    hashes = evidence.get("artifact_hashes")
    if not isinstance(hashes, dict):
        errors.append("artifact_hashes must be an object")
        hashes = {}
    packaged: dict[str, Path] = {}
    for path in sorted(directory.rglob("*")):
        if path.is_symlink():
            errors.append(f"symlink is not permitted in environment evidence: {path}")
        elif path.is_file() and path != evidence_path:
            packaged[path.relative_to(directory).as_posix()] = path
    if set(hashes) != set(packaged):
        missing = sorted(set(packaged) - set(hashes))
        extra = sorted(set(hashes) - set(packaged))
        if missing:
            errors.append("artifact_hashes omits packaged files: " + ", ".join(missing))
        if extra:
            errors.append("artifact_hashes lists non-packaged files: " + ", ".join(extra))
    for relative, path in packaged.items():
        expected = hashes.get(relative)
        if not is_sha256(expected):
            errors.append(f"artifact_hashes[{relative!r}] is not a SHA-256 digest")
        elif sha256_file(path) != expected:
            errors.append(f"artifact hash mismatch for {relative}")
    return errors


def validate_fixture_binding(
    report: dict[str, Any],
    surface_manifest_path: Path,
    metadata_path: Path,
    fixtures_dir: Path,
) -> list[str]:
    """Bind a fixture report to this release's runtime and full critical surface.

    Without this, a stale or truncated fixture directory whose report claims
    `missing: []` would pass strict assembly while shipping transcripts from
    another runtime or omitting most of the 15 §5(4) surface.
    """
    errors: list[str] = []
    if report.get("schema") != "bleavit.chainhead-fixtures-report.v1":
        errors.append("fixture report schema must be bleavit.chainhead-fixtures-report.v1")
    if metadata_path.is_file():
        metadata_actual = sha256_file(metadata_path)
        if report.get("metadata_sha256") != metadata_actual:
            errors.append(
                f"fixture report metadata_sha256 {report.get('metadata_sha256')!r} does "
                f"not match shipped metadata.scale sha256 {metadata_actual} — the "
                "fixtures were recorded against a different runtime"
            )
    try:
        manifest = load_json(surface_manifest_path)
        expected_ids = {entry["id"] for entry in manifest["entries"]}
    except (OSError, ValueError, KeyError, TypeError, json.JSONDecodeError) as error:
        return [*errors, f"cannot load surface manifest: {error}"]
    recorded = report.get("recorded")
    missing = report.get("missing")
    if not isinstance(recorded, list) or not isinstance(missing, list):
        return [*errors, "fixture report recorded/missing must be arrays"]
    covered = {item for item in recorded if isinstance(item, str)}
    covered |= {
        item.get("surface")
        for item in missing
        if isinstance(item, dict) and isinstance(item.get("surface"), str)
    }
    if covered != expected_ids:
        unreported = sorted(expected_ids - covered)
        unknown = sorted(covered - expected_ids)
        if unreported:
            errors.append(
                "fixture report does not cover the full critical surface; "
                "unreported: " + ", ".join(unreported[:8])
                + ("…" if len(unreported) > 8 else "")
            )
        if unknown:
            errors.append(
                "fixture report names surface absent from the manifest: "
                + ", ".join(unknown[:8])
                + ("…" if len(unknown) > 8 else "")
            )
    expected_files = {f"{safe_filename(identifier)}.json" for identifier in expected_ids}
    actual_files = (
        {
            path.name
            for path in fixtures_dir.glob("*.json")
            if path.name != "fixtures-report.json"
        }
        if fixtures_dir.is_dir()
        else set()
    )
    if actual_files != expected_files:
        absent = sorted(expected_files - actual_files)
        extra = sorted(actual_files - expected_files)
        if absent:
            errors.append(
                "transcripts missing for manifest surface: "
                + ", ".join(absent[:8])
                + ("…" if len(absent) > 8 else "")
            )
        if extra:
            errors.append(
                "transcripts present for no manifest surface: "
                + ", ".join(extra[:8])
                + ("…" if len(extra) > 8 else "")
            )
    return errors


def validate_supply_chain_summary(summary: dict[str, Any]) -> list[str]:
    errors: list[str] = []
    if summary.get("schema") != "bleavit.supply-chain.v1":
        errors.append("supply-chain summary schema must be bleavit.supply-chain.v1")
    ignored = summary.get("ignored_advisory_ids")
    if not isinstance(ignored, list) or any(
        not isinstance(item, str) or not item.startswith("RUSTSEC-") for item in ignored
    ):
        errors.append("ignored_advisory_ids must be an array of RustSec IDs")
    workspaces = summary.get("workspaces")
    if not isinstance(workspaces, dict) or set(workspaces) != {"root", "keeper"}:
        errors.append("workspaces must contain exactly root and keeper")
    else:
        for name, row in workspaces.items():
            if not isinstance(row, dict) or not isinstance(row.get("allowed_warning_count"), int) or row["allowed_warning_count"] < 0:
                errors.append(f"workspaces.{name}.allowed_warning_count must be non-negative")
    return errors


def validate_sweep(
    sweep_dir: Path, gaps: list[dict[str, str]]
) -> tuple[Path | None, list[Path], bool]:
    manifest_path = sweep_dir / "sweep-manifest.json"
    if not manifest_path.is_file():
        add_gap(gaps, "vectors.mpfr_sweep", "B8", "sweep-manifest.json is missing")
        return None, [], False
    try:
        manifest = load_json(manifest_path)
    except (OSError, ValueError, json.JSONDecodeError) as error:
        add_gap(gaps, "vectors.mpfr_sweep", "B8", f"invalid sweep manifest: {error}")
        return manifest_path, [], False
    shards = manifest.get("shards")
    points = manifest.get("points")
    if not isinstance(points, int) or points < 10_000_000:
        add_gap(
            gaps,
            "vectors.mpfr_sweep.scale",
            "B8",
            f"full release sweep requires at least 10000000 points; manifest declares {points!r}",
        )
    if not isinstance(shards, list) or not shards:
        add_gap(gaps, "vectors.mpfr_sweep", "B8", "sweep manifest lists no shards")
        return manifest_path, [], False
    shard_paths: list[Path] = []
    declared_rows = 0
    archive_ready = True
    for index, item in enumerate(shards):
        if not isinstance(item, dict):
            add_gap(gaps, f"vectors.mpfr_sweep.shard.{index}", "B8", "invalid shard row")
            archive_ready = False
            continue
        relative = item.get("path") or item.get("file")
        expected = item.get("sha256")
        rows = item.get("rows")
        if not isinstance(rows, int) or rows < 1:
            add_gap(gaps, f"vectors.mpfr_sweep.shard.{index}", "B8", "shard row count must be a positive integer")
            archive_ready = False
        else:
            declared_rows += rows
        if not isinstance(relative, str) or not isinstance(expected, str):
            add_gap(gaps, f"vectors.mpfr_sweep.shard.{index}", "B8", "shard row lacks path/file or sha256")
            archive_ready = False
            continue
        candidate = sweep_dir / relative
        if not candidate.is_file() and "/" not in relative:
            candidate = sweep_dir / "shards" / relative
        if not candidate.is_file():
            add_gap(gaps, f"vectors.mpfr_sweep.shard.{index}", "B8", f"missing shard {relative}")
            archive_ready = False
            continue
        if sha256_file(candidate) != expected:
            add_gap(gaps, f"vectors.mpfr_sweep.shard.{index}", "B8", f"SHA-256 mismatch for {relative}")
            archive_ready = False
            continue
        shard_paths.append(candidate)
    if isinstance(points, int) and declared_rows != points:
        add_gap(gaps, "vectors.mpfr_sweep.rows", "B8", f"shard row total {declared_rows} does not equal manifest points {points}")
        archive_ready = False
    return manifest_path, shard_paths, archive_ready


def readiness_markdown(
    ready: bool,
    allow_missing: bool,
    gaps: list[dict[str, str]],
    corruptions: list[dict[str, str]],
    artifacts: list[dict[str, Any]],
) -> str:
    lines = [
        "# Bleavit release readiness",
        "",
        f"- Mode: {'dry-run / allow-missing' if allow_missing else 'strict release'}",
        f"- Publishable: {'yes' if ready else 'no'}",
        f"- Content-addressed artifacts assembled: {len(artifacts)}",
        "",
    ]
    failures = [*corruptions, *gaps]
    if failures:
        lines.extend(["## Missing or failing requirements", "", "| Requirement | Owner | Reason |", "|---|---|---|"])
        for gap in failures:
            reason = gap["reason"].replace("|", "\\|").replace("\n", " ")
            lines.append(f"| `{gap['id']}` | {gap['owner']} | {reason} |")
    else:
        lines.extend(["## Missing or failing requirements", "", "None."])
    lines.extend(
        [
            "",
            "## Publication boundary",
            "",
            "This prerelease is not canonical until an operator attaches Arweave mirror evidence mapping every content SHA-256 to its TXID. CI holds no Arweave or release-signing keys.",
            "",
        ]
    )
    return "\n".join(lines)


def main() -> int:
    args = parse_args()
    root = repo_root()
    args.output_dir.mkdir(parents=True, exist_ok=True)
    dist_dir = args.dist_dir or args.output_dir / "dist"
    if dist_dir.exists() and any(dist_dir.iterdir()):
        raise RuntimeError(f"refusing to mix release output with non-empty directory: {dist_dir}")
    dist_dir.mkdir(parents=True, exist_ok=True)
    archive_dir = args.output_dir / "archives"
    archive_dir.mkdir(parents=True, exist_ok=True)
    epoch = source_date_epoch(root)

    gaps: list[dict[str, str]] = []
    corruptions: list[dict[str, str]] = []
    candidates: list[Candidate] = []

    # Per-surface `blocked_by` explains a missing recording; it cannot gate a
    # surface that records successfully. These manifest-level rows fail closed
    # for known cross-surface compliance gaps (15 §5).
    try:
        for blocker in load_manifest_release_blockers(args.surface_manifest):
            add_gap(gaps, blocker["id"], blocker["owner"], blocker["reason"])
    except (OSError, ValueError, json.JSONDecodeError) as error:
        add_corruption(
            corruptions,
            "surface_manifest.readiness",
            f"cannot load release blockers: {error}",
        )

    runtime_names = ("runtime.wasm", "metadata.scale", "runtime-info.json", "build-info.json")
    for name in runtime_names:
        path = args.runtime_dir / name
        if path.is_file():
            candidates.append(Candidate("runtime", path, str(path)))
        else:
            add_gap(gaps, f"runtime.{name}", "B8", f"missing {path}")
    build_info: dict[str, Any] = {}
    runtime_info: dict[str, Any] = {}
    for name, target in (("build-info.json", "build"), ("runtime-info.json", "runtime")):
        path = args.runtime_dir / name
        if path.is_file():
            try:
                value = load_json(path)
                if target == "build":
                    build_info = value
                    for error in validate_build_info(value):
                        add_gap(gaps, "runtime.build_info", "B8", error)
                else:
                    runtime_info = value
            except (OSError, ValueError, json.JSONDecodeError) as error:
                add_gap(gaps, f"runtime.{target}_info", "B8", f"invalid {name}: {error}")
    runtime_wasm_sha256, binding_errors = validate_runtime_binding(
        args.runtime_dir, build_info, runtime_info
    )
    for error in binding_errors:
        add_corruption(corruptions, "runtime.wasm_binding", error)
    commit = args.commit or git_value(root, "rev-parse", "HEAD")
    built_commit = build_info.get("git_commit")
    if built_commit and built_commit != commit:
        reason = (
            f"build-info.git_commit {built_commit} does not match the release "
            f"commit {commit}"
        )
        if args.allow_missing:
            add_gap(gaps, "runtime.commit_binding", "B8", reason)
        else:
            add_corruption(corruptions, "runtime.commit_binding", reason)

    inventory: dict[str, Any] = {}
    try:
        inventory = load_json(args.environments)
        inventory_errors = validate_environment_inventory(inventory)
        for error in inventory_errors:
            add_gap(gaps, "environments.inventory", "B8", error)
        candidates.append(Candidate("environment-inventory", args.environments, str(args.environments)))
    except (OSError, ValueError, json.JSONDecodeError) as error:
        add_gap(gaps, "environments.inventory", "B8", f"invalid environment inventory: {error}")

    selected_specs: set[str] = set()
    if not any(gap["id"] == "environments.inventory" for gap in gaps):
        for environment in inventory["environments"]:
            if not (environment["required"] or environment["live"]):
                continue
            identifier = environment["id"]
            name = environment["chain_spec"]
            selected_specs.add(name)
            spec_path = args.chain_spec_dir / name
            if not spec_path.is_file():
                add_gap(gaps, f"chain_specs.{identifier}", "B3", f"missing {spec_path}")
                continue
            candidates.append(Candidate("chain-spec", spec_path, str(spec_path)))
            for error in validate_chain_spec_profile(args.chain_spec_validator, identifier, spec_path):
                add_gap(gaps, f"chain_specs.{identifier}.validation", "B3", error)
            if environment["live"]:
                bootnodes = root / environment["bootnodes"]
                if not bootnodes.is_file():
                    add_gap(gaps, f"chain_specs.{identifier}.bootnodes", "B3", f"missing {bootnodes}")
    extras = sorted(args.chain_spec_dir.glob("*.json")) if args.chain_spec_dir.is_dir() else []
    for extra in extras:
        if extra.name not in selected_specs:
            candidates.append(Candidate("chain-spec", extra, str(extra)))
    if runtime_wasm_sha256 is not None:
        for candidate in [item for item in candidates if item.kind == "chain-spec"]:
            try:
                spec_hash = chain_spec_wasm_sha256(load_json(candidate.path))
                if spec_hash != runtime_wasm_sha256:
                    add_corruption(
                        corruptions,
                        f"chain_specs.{candidate.path.name}.wasm_binding",
                        f"genesis :code sha256 {spec_hash} does not match shipped runtime.wasm {runtime_wasm_sha256}",
                    )
            except (OSError, ValueError, json.JSONDecodeError) as error:
                add_corruption(
                    corruptions,
                    f"chain_specs.{candidate.path.name}.wasm_binding",
                    f"cannot verify genesis :code: {error}",
                )

    for name, path in (("zombienet", args.zombienet_dir), ("chopsticks", args.chopsticks_dir)):
        if runtime_wasm_sha256 is None:
            add_gap(
                gaps,
                f"environments.{name}",
                "B8",
                "cannot verify run evidence without a shipped runtime wasm",
            )
            continue
        errors = validate_run_evidence(path, name, runtime_wasm_sha256, commit)
        if errors:
            add_gap(gaps, f"environments.{name}", "B7", "; ".join(errors))
            continue
        archive = archive_dir / f"{name}.tar.xz"
        deterministic_tar_xz(path, archive, epoch)
        candidates.append(Candidate("environment", archive, str(path)))

    fixtures_report_path = args.fixtures_dir / "fixtures-report.json"
    if not fixtures_report_path.is_file():
        add_gap(gaps, "chainhead.report", "B8", "fixtures-report.json is missing")
    else:
        try:
            fixtures_report = load_json(fixtures_report_path)
            candidates.append(Candidate("chainhead-report", fixtures_report_path, str(fixtures_report_path)))
            # The report is only trustworthy bound to this release: its
            # schema, the runtime it recorded against, and the complete
            # critical surface it must cover. A stale or truncated fixture
            # directory is corruption, not a readiness gap.
            for error in validate_fixture_binding(
                fixtures_report,
                args.surface_manifest,
                args.runtime_dir / "metadata.scale",
                args.fixtures_dir,
            ):
                add_corruption(corruptions, "chainhead.binding", error)
            for item in fixtures_report.get("missing", []):
                if item.get("required"):
                    blocker = item.get("blocked_by")
                    add_gap(
                        gaps,
                        f"chainhead.{item.get('surface', 'unknown')}",
                        milestone_from_blocker(blocker),
                        f"{item.get('reason', 'not recorded')}" + (f" ({blocker})" if blocker else ""),
                    )
            if fixtures_report.get("mode") != "chainHead-v1":
                add_gap(gaps, "chainhead.transport", "B8", "recorder ran without chainHead websocket coverage")
        except (OSError, ValueError, json.JSONDecodeError) as error:
            add_gap(gaps, "chainhead.report", "B8", f"invalid fixture report: {error}")
    transcript_paths = (
        sorted(path for path in args.fixtures_dir.glob("*.json") if path.name != "fixtures-report.json")
        if args.fixtures_dir.is_dir()
        else []
    )
    if not transcript_paths:
        add_gap(gaps, "chainhead.transcripts", "B8", "no transcript JSON files found")
    for path in transcript_paths:
        candidates.append(Candidate("chainhead-transcript", path, str(path)))

    if args.reference_vectors.is_file():
        candidates.append(Candidate("reference-vectors", args.reference_vectors, str(args.reference_vectors)))
    else:
        add_gap(gaps, "vectors.reference_model", "M3", f"missing {args.reference_vectors}")
    sweep_manifest, shard_paths, sweep_archive_ready = validate_sweep(args.sweep_dir, gaps)
    if sweep_manifest is not None:
        candidates.append(Candidate("sweep-manifest", sweep_manifest, str(sweep_manifest)))
    if shard_paths and sweep_archive_ready:
        sweep_archive = archive_dir / "mpfr-sweep.tar.xz"
        deterministic_tar_xz(args.sweep_dir, sweep_archive, epoch)
        candidates.append(Candidate("mpfr-sweep", sweep_archive, str(args.sweep_dir)))

    supply_chain_summary: dict[str, Any] | None = None
    if args.supply_chain_result != "passed":
        add_gap(gaps, "supply_chain", "B8", f"supply-chain gate outcome is {args.supply_chain_result}")
    if args.supply_chain_summary.is_file():
        try:
            supply_chain_summary = load_json(args.supply_chain_summary)
            for error in validate_supply_chain_summary(supply_chain_summary):
                add_gap(gaps, "supply_chain.summary", "B8", error)
            candidates.append(Candidate("supply-chain-summary", args.supply_chain_summary, str(args.supply_chain_summary)))
        except (OSError, ValueError, json.JSONDecodeError) as error:
            add_gap(gaps, "supply_chain.summary", "B8", f"invalid summary: {error}")
    else:
        add_gap(gaps, "supply_chain.summary", "B8", f"missing {args.supply_chain_summary}")

    artifact_entries: list[dict[str, Any]] = []
    for candidate in candidates:
        digest = sha256_file(candidate.path)
        destination = dist_dir / f"{digest}-{candidate.path.name}"
        if not destination.exists():
            shutil.copyfile(candidate.path, destination)
        artifact_entries.append(
            {
                "kind": candidate.kind,
                "path": str(destination.relative_to(args.output_dir)),
                "sha256": digest,
                "size": candidate.path.stat().st_size,
                "source": candidate.source,
            }
        )
    artifact_entries.sort(key=lambda item: (item["kind"], item["path"]))
    gaps.sort(key=lambda item: (item["owner"], item["id"], item["reason"]))
    corruptions.sort(key=lambda item: (item["id"], item["reason"]))
    ready = not gaps and not corruptions
    tag = args.tag or git_value(root, "describe", "--tags", "--exact-match", default="untagged")
    manifest = {
        "schema": "bleavit.release.v1",
        "git_commit": commit,
        "tag": tag,
        "toolchain": build_info.get("toolchain", "unknown"),
        "source_date_epoch": epoch,
        "artifacts": artifact_entries,
        "readiness": {
            "publishable": ready,
            "mode": "allow-missing" if args.allow_missing else "strict",
            "missing": gaps,
            "corruption": corruptions,
        },
        "supply_chain": {
            "outcome": args.supply_chain_result,
            "evidence": args.supply_chain_evidence,
            "summary": supply_chain_summary,
        },
        "content_addressing": "dist/<sha256>-<basename>",
        "mirror": {"required": True, "status": "pending", "evidence": None},
    }
    manifest_path = args.output_dir / "release-manifest.json"
    report_path = args.output_dir / "readiness-report.md"
    write_json(manifest_path, manifest)
    report_path.write_text(
        readiness_markdown(ready, args.allow_missing, gaps, corruptions, artifact_entries),
        encoding="utf-8",
    )
    for path in (manifest_path, report_path):
        shutil.copyfile(path, dist_dir / path.name)
        shutil.copyfile(path, dist_dir / f"{sha256_file(path)}-{path.name}")

    if corruptions:
        print(f"release assembly failed with {len(corruptions)} integrity errors", file=sys.stderr)
        return 1
    if gaps and not args.allow_missing:
        print(f"strict release assembly failed with {len(gaps)} readiness gaps", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

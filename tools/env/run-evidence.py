#!/usr/bin/env python3
"""Run B7 environments and produce release-bound SQ-139 evidence."""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import os
import re
import shutil
import signal
import socket
import subprocess
import sys
import time
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
from types import ModuleType
from typing import Any

import yaml


EVIDENCE_SCHEMA = "bleavit.env-evidence.v1"
MANIFEST_SCHEMA = "bleavit.env-suites.v1"
REPORT_SCHEMA = "bleavit.env-run-report.v1"
KINDS = ("zombienet", "chopsticks")
RAW_CODE_KEY = "0x3a636f6465"
RAW_HEAPPAGES_KEY = "0x3a686561707061676573"
CHOPSTICKS_GENESIS = "zombienet/specs/out/bleavit-drill-raw.json"
DEFAULT_ZOMBIENET_BINARY = Path("zombienet/bin/zombienet")
DEFAULT_NODE_BINARY = Path("target/release/bleavit-node")
TERMINATE_GRACE_SECONDS = 5.0
SCRIPT_ROOT = Path(__file__).resolve().parents[2]
# 15 §1 requires this closing check in every environment job; SQ-152 lands it.
TRY_STATE_CHECK = "try-state"


class EvidenceError(RuntimeError):
    """A fail-closed environment runner error with a user-facing message."""


@dataclass(frozen=True)
class Suite:
    identifier: str
    kind: str
    path: Path
    tier: str
    gated_on: tuple[str, ...]
    timeout_seconds: int
    spec: str


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Run the committed B7 suites and produce release-bound evidence."
    )
    parser.add_argument(
        "--root",
        type=Path,
        default=SCRIPT_ROOT,
        help="repository root (defaults to the runner's repository)",
    )
    parser.add_argument(
        "--wasm",
        type=Path,
        help="release runtime.wasm (required unless --no-evidence)",
    )
    parser.add_argument("--commit", help="release commit (defaults to git HEAD)")
    parser.add_argument(
        "--tier", choices=("release", "g1", "all"), default="release"
    )
    parser.add_argument(
        "--kind", choices=("zombienet", "chopsticks", "all"), default="all"
    )
    parser.add_argument(
        "--suites",
        help="comma-separated explicit suite ids; forces report-only mode",
    )
    parser.add_argument("--include-gated", action="store_true")
    parser.add_argument("--no-evidence", action="store_true")
    parser.add_argument(
        "--log-dir", type=Path, default=Path("target/env/evidence-logs")
    )
    parser.add_argument("--report-out", type=Path)
    parser.add_argument(
        "--zombienet-binary",
        type=Path,
        default=None,
    )
    parser.add_argument(
        "--chopsticks-command",
        nargs="+",
        help="command prefix used to start Chopsticks (the runner appends --config PATH)",
    )
    parser.add_argument(
        "--node-binary",
        type=Path,
        default=None,
    )
    return parser.parse_args()


def rooted(root: Path, path: Path) -> Path:
    return path if path.is_absolute() else root / path


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def git_output(root: Path, *arguments: str) -> str:
    completed = subprocess.run(
        ["git", *arguments],
        cwd=root,
        text=True,
        capture_output=True,
        check=False,
    )
    if completed.returncode != 0:
        detail = (completed.stderr or completed.stdout).strip()
        raise EvidenceError(
            f"15 §5: git {' '.join(arguments)} failed: {detail or 'nonzero exit'}"
        )
    return completed.stdout.strip()


def load_assemble_release() -> ModuleType:
    path = SCRIPT_ROOT / "tools" / "release" / "assemble-release.py"
    module_name = "bleavit_assemble_release_for_env_evidence"
    existing = sys.modules.get(module_name)
    if existing is not None:
        return existing
    spec = importlib.util.spec_from_file_location(module_name, path)
    if spec is None or spec.loader is None:
        raise EvidenceError(f"15 §5: cannot import release evidence consumer from {path}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[module_name] = module
    release_tools = str(path.parent)
    inserted = release_tools not in sys.path
    if inserted:
        sys.path.insert(0, release_tools)
    try:
        spec.loader.exec_module(module)
    except Exception:
        sys.modules.pop(module_name, None)
        raise
    finally:
        if inserted:
            sys.path.remove(release_tools)
    return module


def load_manifest(root: Path) -> list[Suite]:
    path = root / "tools" / "env" / "suites.json"
    try:
        document = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
        raise EvidenceError(f"15 §4.7; 02 §11: cannot read suites.json: {error}") from error
    if not isinstance(document, dict) or document.get("schema") != MANIFEST_SCHEMA:
        raise EvidenceError(
            f"15 §4.7; 02 §11: suites.json schema must be {MANIFEST_SCHEMA}"
        )
    rows = document.get("suites")
    if not isinstance(rows, list) or not rows:
        raise EvidenceError("15 §4.7; 02 §11: suites.json suites must be non-empty")

    result: list[Suite] = []
    identifiers: set[str] = set()
    paths: set[str] = set()
    for index, row in enumerate(rows):
        label = f"suites.json suites[{index}]"
        if not isinstance(row, dict):
            raise EvidenceError(f"15 §4.7; 02 §11: {label} must be an object")
        identifier = row.get("id")
        kind = row.get("kind")
        relative = row.get("path")
        tier = row.get("tier")
        gated_on = row.get("gated_on")
        timeout = row.get("timeout_seconds")
        citation = row.get("spec")
        if not isinstance(identifier, str) or not identifier:
            raise EvidenceError(f"15 §4.7; 02 §11: {label}.id must be non-empty")
        if identifier in identifiers:
            raise EvidenceError(f"15 §4.7; 02 §11: duplicate suite id {identifier!r}")
        identifiers.add(identifier)
        if kind not in KINDS:
            raise EvidenceError(f"15 §4.7; 02 §11: {label}.kind is invalid")
        if tier not in ("release", "g1"):
            raise EvidenceError(f"15 §4.7; 02 §11: {label}.tier is invalid")
        if not isinstance(relative, str) or not relative:
            raise EvidenceError(f"15 §4.7; 02 §11: {label}.path must be non-empty")
        pure = PurePosixPath(relative)
        if pure.is_absolute() or ".." in pure.parts or "\\" in relative:
            raise EvidenceError(
                f"15 §4.7; 02 §11: {label}.path must be repository-relative"
            )
        if relative in paths:
            raise EvidenceError(
                f"15 §4.7; 02 §11: duplicate suite path {relative!r}"
            )
        paths.add(relative)
        suite_path = root / Path(*pure.parts)
        if not suite_path.is_file():
            raise EvidenceError(
                f"15 §4.7; 02 §11: {label}.path does not exist: {relative}"
            )
        if (
            not isinstance(gated_on, list)
            or any(not isinstance(gate, str) or not gate for gate in gated_on)
        ):
            raise EvidenceError(f"15 §4.7; 02 §11: {label}.gated_on is invalid")
        if type(timeout) is not int or timeout <= 0:
            raise EvidenceError(
                f"15 §4.7; 02 §11: {label}.timeout_seconds must be positive"
            )
        if not isinstance(citation, str) or not citation:
            raise EvidenceError(f"15 §4.7; 02 §11: {label}.spec must be non-empty")
        result.append(
            Suite(
                identifier=identifier,
                kind=kind,
                path=Path(*pure.parts),
                tier=tier,
                gated_on=tuple(gated_on),
                timeout_seconds=timeout,
                spec=citation,
            )
        )
    return result


def parse_requested_ids(value: str | None, suites: list[Suite]) -> set[str] | None:
    if value is None:
        return None
    requested = {item.strip() for item in value.split(",") if item.strip()}
    if not requested:
        raise EvidenceError("--suites must name at least one suite id")
    known = {suite.identifier for suite in suites}
    unknown = sorted(requested - known)
    if unknown:
        raise EvidenceError("unknown --suites id(s): " + ", ".join(unknown))
    return requested


def select_suites(
    suites: list[Suite],
    kind: str,
    tier: str,
    requested: set[str] | None,
    include_gated: bool,
    log_dir: Path,
) -> tuple[list[Suite], list[dict[str, Any]]]:
    selected: list[Suite] = []
    rows: list[dict[str, Any]] = []
    for suite in suites:
        if kind != "all" and suite.kind != kind:
            continue
        if requested is not None and suite.identifier not in requested:
            continue
        log = str(log_dir / f"{suite.identifier}.log")
        if tier != "all" and suite.tier != tier:
            rows.append(
                {
                    "id": suite.identifier,
                    "kind": suite.kind,
                    "result": "excluded-tier",
                    "duration_seconds": 0.0,
                    "log": log,
                    "gated_on": list(suite.gated_on),
                }
            )
        elif suite.gated_on and not include_gated:
            rows.append(
                {
                    "id": suite.identifier,
                    "kind": suite.kind,
                    "result": "skipped-gated",
                    "duration_seconds": 0.0,
                    "log": log,
                    "gated_on": list(suite.gated_on),
                }
            )
        else:
            selected.append(suite)
    if requested is not None and not selected and not rows:
        raise EvidenceError("the --kind filter excludes every explicitly requested suite")
    return selected, rows


def require_executable(path: Path, label: str) -> None:
    if not path.is_file():
        raise EvidenceError(f"15 §4.7: required {label} is missing: {path}")
    if not os.access(path, os.X_OK):
        raise EvidenceError(f"15 §4.7: required {label} is not executable: {path}")


def require_file(path: Path, label: str) -> None:
    if not path.is_file():
        raise EvidenceError(f"15 §4.7; 02 §11: required {label} is missing: {path}")


def uses_xcm_topology(root: Path, suite: Suite) -> bool:
    try:
        return "bleavit-xcm.toml" in (root / suite.path).read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError) as error:
        raise EvidenceError(f"15 §4.7: cannot inspect {suite.path}: {error}") from error


def validate_node_version(root: Path) -> None:
    try:
        completed = subprocess.run(
            ["node", "--version"],
            cwd=root,
            text=True,
            capture_output=True,
            check=False,
            timeout=10,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        raise EvidenceError(f"15 §4.7: Node.js 22 or newer is required: {error}") from error
    version = (completed.stdout or completed.stderr).strip()
    match = re.fullmatch(r"v?(\d+)(?:\.\d+){0,2}", version)
    if completed.returncode != 0 or match is None:
        raise EvidenceError(
            f"15 §4.7: cannot determine Node.js version (got {version!r})"
        )
    if int(match.group(1)) < 22:
        raise EvidenceError(
            f"15 §4.7: Chopsticks requires Node.js 22 or newer (found {version})"
        )


def validate_prerequisites(
    root: Path,
    selected: list[Suite],
    zombienet_binary: Path,
    node_binary: Path,
) -> None:
    zombienet_suites = [suite for suite in selected if suite.kind == "zombienet"]
    chopsticks_suites = [suite for suite in selected if suite.kind == "chopsticks"]
    specs = root / "zombienet" / "specs" / "out"
    if selected:
        for name in (
            "paseo-local.json",
            "bleavit-drill.json",
            "bleavit-drill-raw.json",
        ):
            require_file(specs / name, f"generated chain spec {name}")
    if zombienet_suites:
        require_executable(zombienet_binary, "Zombienet binary")
        for name in (
            "polkadot",
            "polkadot-prepare-worker",
            "polkadot-execute-worker",
            "polkadot-parachain",
        ):
            require_executable(root / "zombienet" / "bin" / name, f"relay binary {name}")
        require_executable(node_binary, "Bleavit node binary")
        if any(uses_xcm_topology(root, suite) for suite in zombienet_suites):
            for name in ("asset-hub-paseo-local.json", "coretime-paseo-local.json"):
                require_file(specs / name, f"generated XCM chain spec {name}")
    if chopsticks_suites:
        validate_node_version(root)


def validate_artifact_binding(root: Path, wasm: Path) -> str:
    if not wasm.is_file():
        raise EvidenceError(f"15 §5: release runtime.wasm is missing: {wasm}")
    wasm_hash = sha256_file(wasm)
    consumer = load_assemble_release()
    for name in ("bleavit-drill.json", "bleavit-drill-raw.json"):
        path = root / "zombienet" / "specs" / "out" / name
        require_file(path, f"generated chain spec {name}")
        try:
            document = json.loads(path.read_text(encoding="utf-8"))
            if not isinstance(document, dict):
                raise ValueError("top level must be an object")
            actual = consumer.chain_spec_wasm_sha256(document)
        except (OSError, UnicodeDecodeError, json.JSONDecodeError, ValueError) as error:
            raise EvidenceError(f"15 §5: cannot read runtime code from {path}: {error}") from error
        if actual != wasm_hash:
            source = (
                'genesis.raw.top["0x3a636f6465"]'
                if name.endswith("-raw.json")
                else "genesis.runtimeGenesis.code"
            )
            raise EvidenceError(
                f"15 §5: {name} {source} sha256 {actual} does not match "
                f"release runtime.wasm sha256 {wasm_hash}"
            )
    return wasm_hash


def terminate_process_group(process: subprocess.Popen[Any]) -> None:
    process_group = process.pid
    # A PID/PGID could theoretically be reused between probes; cgroup-level
    # process isolation would close that residual race but is out of scope here.
    try:
        os.killpg(process_group, signal.SIGTERM)
    except ProcessLookupError:
        process.poll()
        return
    deadline = time.monotonic() + TERMINATE_GRACE_SECONDS
    while time.monotonic() < deadline:
        process.poll()  # Reap an exited leader so it doesn't keep the PGID visible.
        try:
            # The session leader can exit before descendants that ignored
            # SIGTERM. Probe the group itself rather than only wait on it.
            os.killpg(process_group, 0)
        except ProcessLookupError:
            return
        time.sleep(0.05)
    try:
        os.killpg(process_group, signal.SIGKILL)
    except ProcessLookupError:
        process.poll()
        return
    try:
        process.wait(timeout=TERMINATE_GRACE_SECONDS)
    except subprocess.TimeoutExpired:
        pass


def append_runner_log(path: Path, message: str) -> None:
    with path.open("a", encoding="utf-8") as handle:
        handle.write(f"\n[run-evidence] {message}\n")


def run_zombienet(
    root: Path, suite: Suite, binary: Path, log_path: Path
) -> tuple[bool, str | None]:
    command = [str(binary), "-p", "native", "test", suite.path.as_posix()]
    process: subprocess.Popen[Any] | None = None
    with log_path.open("wb") as log:
        try:
            try:
                process = subprocess.Popen(
                    command,
                    cwd=root,
                    stdout=log,
                    stderr=subprocess.STDOUT,
                    start_new_session=True,
                )
            except OSError as error:
                return False, f"could not start Zombienet: {error}"
            try:
                returncode = process.wait(timeout=suite.timeout_seconds)
            except subprocess.TimeoutExpired:
                detail = (
                    f"timed out after {suite.timeout_seconds} seconds; process group killed"
                )
                append_runner_log(log_path, detail)
                return False, detail
        finally:
            if process is not None:
                terminate_process_group(process)
    if returncode != 0:
        return False, f"Zombienet exited with status {returncode}"
    return True, None


def rpc_call(connection: Any, method: str, params: list[Any], deadline: float) -> Any:
    remaining = deadline - time.monotonic()
    if remaining <= 0:
        raise EvidenceError(f"Chopsticks timeout before RPC {method}")
    request_id = rpc_call.next_id
    rpc_call.next_id += 1
    connection.send(
        json.dumps(
            {"jsonrpc": "2.0", "id": request_id, "method": method, "params": params}
        )
    )
    response = json.loads(connection.recv(timeout=remaining))
    if not isinstance(response, dict) or response.get("id") != request_id:
        raise EvidenceError(f"invalid Chopsticks JSON-RPC response for {method}")
    if response.get("error") is not None:
        raise EvidenceError(f"Chopsticks RPC {method} failed: {response['error']!r}")
    if "result" not in response:
        raise EvidenceError(f"Chopsticks RPC {method} returned no result")
    return response["result"]


rpc_call.next_id = 1


def storage_bytes(value: Any, label: str) -> bytes | None:
    if value is None:
        return None
    if not isinstance(value, str) or re.fullmatch(r"0x(?:[0-9a-fA-F]{2})*", value) is None:
        raise EvidenceError(f"{label} is not null or an even-length 0x hex value")
    return bytes.fromhex(value[2:])


def header_number(header: Any) -> int:
    if not isinstance(header, dict) or not isinstance(header.get("number"), str):
        raise EvidenceError("chain_getHeader returned no hex block number")
    try:
        return int(header["number"], 16)
    except ValueError as error:
        raise EvidenceError(
            f"chain_getHeader returned invalid block number {header['number']!r}"
        ) from error


def connect_chopsticks(uri: str, process: subprocess.Popen[Any], deadline: float) -> Any:
    try:
        from websockets.sync.client import connect
    except ImportError as error:
        raise EvidenceError(
            "15 §4.7: Chopsticks execution requires websockets 15.x"
        ) from error

    last_error: Exception | None = None
    while time.monotonic() < deadline:
        returncode = process.poll()
        if returncode is not None:
            raise EvidenceError(
                f"Chopsticks exited with status {returncode} before RPC readiness"
            )
        connection = None
        try:
            remaining = max(0.1, deadline - time.monotonic())
            connection = connect(
                uri,
                open_timeout=min(1.0, remaining),
                close_timeout=1.0,
            )
            rpc_call(connection, "system_health", [], deadline)
            return connection
        except Exception as error:  # readiness transports fail in several library-specific forms.
            last_error = error
            if connection is not None:
                try:
                    connection.close()
                except Exception:
                    pass
            time.sleep(min(0.1, max(0.0, deadline - time.monotonic())))
    raise EvidenceError(
        f"Chopsticks RPC at {uri} was not ready before timeout: {last_error}"
    )


def repository_relative_chopsticks_db(root: Path, path: Path, database: Any) -> Path:
    if not isinstance(database, str) or not database:
        raise EvidenceError(f"Chopsticks config {path} has an invalid db path")
    pure = PurePosixPath(database)
    if (
        pure.is_absolute()
        or ".." in pure.parts
        or "\\" in database
        or pure.parts[:2] != ("chopsticks", ".state")
        or len(pure.parts) < 3
    ):
        raise EvidenceError(
            f"Chopsticks config {path} db must be repository-relative under "
            "chopsticks/.state/"
        )
    relative = Path(*pure.parts)
    state_root = (root / "chopsticks" / ".state").resolve()
    resolved = (root / relative).resolve()
    try:
        resolved.relative_to(state_root)
    except ValueError as error:
        raise EvidenceError(
            f"Chopsticks config {path} db must be repository-relative under "
            "chopsticks/.state/"
        ) from error
    if resolved == state_root:
        raise EvidenceError(
            f"Chopsticks config {path} db must name a file under chopsticks/.state/"
        )
    return resolved


def load_chopsticks_config(
    root: Path, path: Path
) -> tuple[int, Path, list[list[Any]]]:
    try:
        document = yaml.safe_load(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, yaml.YAMLError) as error:
        raise EvidenceError(f"cannot parse Chopsticks config {path}: {error}") from error
    if not isinstance(document, dict):
        raise EvidenceError(f"Chopsticks config {path} must contain an object")
    port = document.get("port")
    genesis = document.get("genesis")
    storage = document.get("import-storage")
    if genesis != CHOPSTICKS_GENESIS:
        raise EvidenceError(
            f"Chopsticks config {path} genesis must be {CHOPSTICKS_GENESIS!r}, "
            f"found {genesis!r}"
        )
    if "wasm-override" in document:
        raise EvidenceError(
            f"Chopsticks config {path} must not define active wasm-override"
        )
    if type(port) is not int or not 1 <= port <= 65535:
        raise EvidenceError(f"Chopsticks config {path} has an invalid port")
    database = repository_relative_chopsticks_db(root, path, document.get("db"))
    if not isinstance(storage, list):
        raise EvidenceError(f"Chopsticks config {path} has invalid import-storage")
    for index, row in enumerate(storage):
        if not isinstance(row, list) or len(row) != 2:
            raise EvidenceError(
                f"Chopsticks config {path} import-storage[{index}] must be [key, value]"
            )
        storage_bytes(row[0], f"import-storage[{index}] key")
        storage_bytes(row[1], f"import-storage[{index}] value")
        if isinstance(row[0], str) and row[0].casefold() in {
            RAW_CODE_KEY,
            RAW_HEAPPAGES_KEY,
        }:
            raise EvidenceError(
                f"Chopsticks config {path} import-storage[{index}] must not inject "
                f"reserved runtime key {row[0]}"
            )
    return port, database, storage


def validate_chopsticks_databases(root: Path, suites: list[Suite]) -> None:
    databases: dict[Path, str] = {}
    for suite in suites:
        if suite.kind != "chopsticks":
            continue
        _port, database, _storage = load_chopsticks_config(root, root / suite.path)
        previous = databases.get(database)
        if previous is not None:
            raise EvidenceError(
                "15 §4.7; 02 §11: Chopsticks db path must be unique across suites; "
                f"{previous!r} and {suite.identifier!r} both use "
                f"{database.relative_to(root)}"
            )
        databases[database] = suite.identifier


def require_free_chopsticks_port(port: int) -> None:
    try:
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as probe:
            probe.bind(("127.0.0.1", port))
    except OSError as error:
        raise EvidenceError(
            f"Chopsticks port 127.0.0.1:{port} is already occupied: {error}"
        ) from error


def cleanup_chopsticks_database(database: Path) -> None:
    for path in database.parent.glob(database.name + "*"):
        if path.is_symlink() or path.is_file():
            path.unlink()
        elif path.is_dir():
            shutil.rmtree(path)


def validate_live_runtime_code(
    connection: Any, expected_wasm_sha256: str, deadline: float
) -> None:
    value = rpc_call(connection, "state_getStorage", [RAW_CODE_KEY], deadline)
    code = storage_bytes(value, f"state_getStorage({RAW_CODE_KEY})")
    if code is None:
        raise EvidenceError(
            f"state_getStorage({RAW_CODE_KEY}) returned null; live runtime code is unbound"
        )
    actual = hashlib.sha256(code).hexdigest()
    if actual != expected_wasm_sha256:
        raise EvidenceError(
            f"live {RAW_CODE_KEY} sha256 {actual} does not match release runtime.wasm "
            f"sha256 {expected_wasm_sha256}"
        )


def run_chopsticks(
    root: Path,
    suite: Suite,
    command_prefix: list[str],
    log_path: Path,
    expected_wasm_sha256: str | None,
) -> tuple[bool, str | None]:
    config_path = root / suite.path
    try:
        port, database, storage = load_chopsticks_config(root, config_path)
        require_free_chopsticks_port(port)
        cleanup_chopsticks_database(database)
    except EvidenceError as error:
        append_runner_log(log_path, str(error))
        return False, str(error)
    command = [*command_prefix, "--config", suite.path.as_posix()]
    process: subprocess.Popen[Any] | None = None
    connection = None
    deadline = time.monotonic() + suite.timeout_seconds
    try:
        with log_path.open("wb") as log:
            try:
                process = subprocess.Popen(
                    command,
                    cwd=root,
                    stdout=log,
                    stderr=subprocess.STDOUT,
                    start_new_session=True,
                )
            except OSError as error:
                return False, f"could not start Chopsticks: {error}"
            connection = connect_chopsticks(f"ws://127.0.0.1:{port}", process, deadline)
            if expected_wasm_sha256 is not None:
                validate_live_runtime_code(connection, expected_wasm_sha256, deadline)
            for index, (key, expected) in enumerate(storage):
                actual = rpc_call(connection, "state_getStorage", [key], deadline)
                if storage_bytes(actual, f"state_getStorage({key})") != storage_bytes(
                    expected, f"import-storage[{index}] value"
                ):
                    raise EvidenceError(
                        f"state_getStorage({key}) does not match "
                        f"import-storage[{index}] byte-for-byte"
                    )
            previous = header_number(rpc_call(connection, "chain_getHeader", [], deadline))
            for index in range(2):
                rpc_call(connection, "dev_newBlock", [{"count": 1}], deadline)
                current = header_number(
                    rpc_call(connection, "chain_getHeader", [], deadline)
                )
                if current <= previous:
                    raise EvidenceError(
                        f"dev_newBlock #{index + 1} did not advance chain_getHeader "
                        f"({previous} -> {current})"
                    )
                previous = current
            if expected_wasm_sha256 is not None:
                validate_live_runtime_code(connection, expected_wasm_sha256, deadline)
            if process.poll() is not None:
                raise EvidenceError(
                    "Chopsticks process exited before the final RPC checks completed"
                )
        return True, None
    except Exception as error:
        detail = str(error) or error.__class__.__name__
        append_runner_log(log_path, detail)
        return False, detail
    finally:
        if connection is not None:
            try:
                connection.close()
            except Exception:
                pass
        if process is not None:
            terminate_process_group(process)


def parse_pins(root: Path) -> dict[str, str]:
    path = root / "tools" / "env" / "pins.env"
    try:
        text = path.read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError) as error:
        raise EvidenceError(f"15 §4.7: cannot read tools/env/pins.env: {error}") from error
    pins: dict[str, str] = {}
    for line_number, raw in enumerate(text.splitlines(), 1):
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        if "=" not in line:
            raise EvidenceError(f"pins.env:{line_number}: expected KEY=VALUE")
        key, value = line.split("=", 1)
        if not key or not value:
            raise EvidenceError(f"pins.env:{line_number}: invalid pin assignment")
        pins[key] = value
    return pins


def default_chopsticks_command(root: Path) -> list[str]:
    version = parse_pins(root).get("CHOPSTICKS_VERSION")
    if not version:
        raise EvidenceError("15 §4.7: pins.env has no CHOPSTICKS_VERSION")
    return ["npx", f"@acala-network/chopsticks@{version}"]


def validate_zombienet_binary_pin(root: Path, binary: Path) -> None:
    expected = parse_pins(root).get("ZOMBIENET_SHA256")
    if expected is None:
        raise EvidenceError("15 §4.7: pins.env has no ZOMBIENET_SHA256")
    actual = sha256_file(binary)
    if actual != expected:
        raise EvidenceError(
            f"15 §4.7: Zombienet binary sha256 {actual} does not match "
            f"tools/env/pins.env ZOMBIENET_SHA256 {expected}"
        )


def write_json(path: Path, document: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(path.name + ".tmp")
    try:
        temporary.write_text(
            json.dumps(document, indent=2, sort_keys=True) + "\n", encoding="utf-8"
        )
        temporary.replace(path)
    except Exception:
        try:
            temporary.unlink()
        except FileNotFoundError:
            pass
        raise


def cleanup_chopsticks_state(root: Path) -> None:
    state = root / "chopsticks" / ".state"
    if state.is_symlink() or state.is_file():
        state.unlink()
    elif state.is_dir():
        shutil.rmtree(state)


def cleanup_generated_state(root: Path) -> None:
    for relative in (Path("zombienet/bin"), Path("zombienet/specs/out")):
        base = root / relative
        if not base.exists():
            continue
        paths = sorted(base.rglob("*"), key=lambda item: len(item.parts), reverse=True)
        for path in paths:
            if path.is_symlink():
                path.unlink()
            elif path.is_file():
                if path.name != ".gitignore":
                    path.unlink()
            elif path.is_dir():
                try:
                    path.rmdir()
                except OSError:
                    pass
    cleanup_chopsticks_state(root)


def remove_prior_evidence(root: Path) -> None:
    for kind in KINDS:
        path = root / kind / "run-evidence.json"
        try:
            path.unlink()
        except FileNotFoundError:
            pass


def validate_clean_checkout(root: Path, release_commit: str) -> None:
    status = git_output(
        root,
        "status",
        "--porcelain",
        "--ignored=matching",
        "--",
        "zombienet",
        "chopsticks",
    )
    if status:
        raise EvidenceError(
            "15 §5: refusing environment evidence from a dirty definitions tree; "
            "git status reports:\n" + status
        )
    tools_status = git_output(root, "status", "--porcelain", "--", "tools/env")
    if tools_status:
        raise EvidenceError(
            "15 §5: refusing environment evidence from a dirty producer tree; "
            "git status reports:\n" + tools_status
        )
    head = git_output(root, "rev-parse", "HEAD")
    if head != release_commit:
        raise EvidenceError(
            f"15 §5: --commit {release_commit} does not equal git HEAD {head}; "
            "evidence was not emitted"
        )


def inventory(directory: Path) -> dict[str, str]:
    evidence_path = directory / "run-evidence.json"
    result: dict[str, str] = {}
    for path in sorted(directory.rglob("*")):
        if path.is_symlink():
            raise EvidenceError(
                f"15 §5: symlink is not permitted in environment evidence: {path}"
            )
        if path.is_file() and path != evidence_path:
            result[path.relative_to(directory).as_posix()] = sha256_file(path)
    return result


def skipped_for_kind(rows: list[dict[str, Any]], kind: str) -> list[dict[str, Any]]:
    result: list[dict[str, Any]] = []
    for row in rows:
        if row["kind"] != kind or row["result"] not in (
            "skipped-gated",
            "excluded-tier",
        ):
            continue
        reason = "gated" if row["result"] == "skipped-gated" else "tier"
        result.append(
            {
                "name": row["id"],
                "reason": reason,
                "gated_on": row.get("gated_on", []),
            }
        )
    return result


def validate_release_evidence_completeness(
    suites: list[Suite], rows: list[dict[str, Any]], kind: str
) -> None:
    required = [
        suite for suite in suites if suite.kind == kind and suite.tier == "release"
    ]
    by_identifier = {row["id"]: row for row in rows if row["kind"] == kind}
    gated = [
        suite
        for suite in required
        if by_identifier.get(suite.identifier, {}).get("result") == "skipped-gated"
    ]
    if gated:
        detail = ", ".join(
            f"{suite.identifier} [{', '.join(suite.gated_on)}]" for suite in gated
        )
        raise EvidenceError(
            f"15 §5: evidence for {kind} requires every release-tier suite to be "
            "attempted and passed; gated release-tier suite(s) were not attempted: "
            f"{detail}; rerun with --include-gated"
        )
    incomplete = [
        suite.identifier
        for suite in required
        if by_identifier.get(suite.identifier, {}).get("result") != "pass"
    ]
    if incomplete:
        raise EvidenceError(
            f"15 §5: evidence for {kind} requires every release-tier suite to be "
            "attempted and passed; missing or non-passing suite(s): "
            + ", ".join(incomplete)
        )
    missing_try_state = [
        row["id"]
        for row in rows
        if row["kind"] == kind
        and row["result"] == "pass"
        and TRY_STATE_CHECK not in row.get("checks", [])
    ]
    if missing_try_state:
        raise EvidenceError(
            f"15 §1: evidence for {kind} requires the closing try-state check on "
            "every passing suite; no driver executes it yet (SQ-152) — evidence "
            "emission is blocked until the try-runtime-cli leg lands: "
            + ", ".join(missing_try_state)
        )


def emit_evidence(
    root: Path,
    suites: list[Suite],
    rows: list[dict[str, Any]],
    wasm: Path,
    runtime_wasm_sha256: str,
    release_commit: str,
    tier: str,
) -> list[str]:
    target_paths = [root / kind / "run-evidence.json" for kind in KINDS]
    try:
        targets: dict[str, tuple[Path, dict[str, str], list[dict[str, Any]]]] = {}
        for kind in KINDS:
            kind_rows = [row for row in rows if row["kind"] == kind]
            if not kind_rows:
                continue
            validate_release_evidence_completeness(suites, rows, kind)
            passing = [
                {
                    "name": row["id"],
                    "result": "pass",
                    "duration_seconds": row["duration_seconds"],
                    "checks": row["checks"],
                }
                for row in kind_rows
                if row["result"] == "pass"
            ]
            if not passing:
                continue
            directory = root / kind
            targets[kind] = (directory, {}, passing)

        reverified_hash = validate_artifact_binding(root, wasm)
        if reverified_hash != runtime_wasm_sha256:
            raise EvidenceError(
                "15 §5: release runtime.wasm changed after the environment suites ran"
            )
        cleanup_generated_state(root)
        validate_clean_checkout(root, release_commit)
        suites_manifest_sha256 = sha256_file(root / "tools" / "env" / "suites.json")
        pins_env_sha256 = sha256_file(root / "tools" / "env" / "pins.env")

        for kind, (directory, _hashes, passing) in list(targets.items()):
            targets[kind] = (directory, inventory(directory), passing)

        for kind, (directory, hashes, passing) in targets.items():
            evidence = {
                "schema": EVIDENCE_SCHEMA,
                "suite": kind,
                "runtime_wasm_sha256": runtime_wasm_sha256,
                "artifact_hashes": hashes,
                "suites_run": passing,
                "recorded_at_commit": release_commit,
                "tier": tier,
                "suites_skipped": skipped_for_kind(rows, kind),
                "produced_by": "tools/env/run-evidence.py",
                "suites_manifest_sha256": suites_manifest_sha256,
                "pins_env_sha256": pins_env_sha256,
            }
            path = directory / "run-evidence.json"
            write_json(path, evidence)

        consumer = load_assemble_release()
        errors: list[str] = []
        for kind, (directory, _hashes, _passing) in targets.items():
            errors.extend(
                f"{kind}: {error}"
                for error in consumer.validate_run_evidence(
                    directory, kind, runtime_wasm_sha256, release_commit
                )
            )
        if errors:
            raise EvidenceError(
                "15 §5: release consumer rejected generated evidence: "
                + "; ".join(errors)
            )
    except BaseException:
        for path in target_paths:
            try:
                path.unlink()
            except FileNotFoundError:
                pass
        raise
    return sorted(targets)


def print_summary(rows: list[dict[str, Any]]) -> None:
    counts = {result: 0 for result in ("pass", "fail", "skipped-gated", "excluded-tier")}
    for row in rows:
        counts[row["result"]] += 1
        detail = ""
        if row["result"] == "skipped-gated":
            detail = " (" + ", ".join(row.get("gated_on", [])) + ")"
        print(
            f"{row['kind']}/{row['id']}: {row['result']} "
            f"({row['duration_seconds']:.3f}s){detail}"
        )
    print(
        "summary: "
        + ", ".join(f"{name}={count}" for name, count in counts.items())
    )


def run() -> int:
    args = parse_args()
    root = args.root.resolve()
    # Every invocation invalidates earlier evidence before even parsing the
    # manifest, including report-only and malformed-manifest runs.
    remove_prior_evidence(root)
    log_dir = rooted(root, args.log_dir).resolve()
    report_out = rooted(root, args.report_out).resolve() if args.report_out else None
    custom_zombienet_binary = args.zombienet_binary is not None
    custom_chopsticks_command = args.chopsticks_command is not None
    custom_node_binary = args.node_binary is not None
    zombienet_binary = rooted(
        root, args.zombienet_binary or DEFAULT_ZOMBIENET_BINARY
    ).resolve()
    node_binary = rooted(root, args.node_binary or DEFAULT_NODE_BINARY).resolve()
    wasm = rooted(root, args.wasm).resolve() if args.wasm else None

    if not args.no_evidence and wasm is None:
        raise EvidenceError("--wasm is required unless --no-evidence is used")
    release_commit = args.commit or git_output(root, "rev-parse", "HEAD")
    suites = load_manifest(root)
    validate_chopsticks_databases(root, suites)
    requested = parse_requested_ids(args.suites, suites)
    selected, rows = select_suites(
        suites,
        args.kind,
        args.tier,
        requested,
        args.include_gated,
        log_dir,
    )
    if not selected and not rows:
        raise EvidenceError("suite selection is empty")

    report_only_reasons: list[str] = []
    if args.no_evidence:
        report_only_reasons.append("--no-evidence")
    if requested is not None:
        report_only_reasons.append("--suites cherry-pick")
    if args.tier != "release":
        report_only_reasons.append(f"--tier {args.tier}")
    if custom_zombienet_binary:
        report_only_reasons.append("--zombienet-binary")
    if custom_chopsticks_command:
        report_only_reasons.append("--chopsticks-command")
    if custom_node_binary:
        report_only_reasons.append("--node-binary")
    evidence_enabled = not report_only_reasons

    runtime_wasm_sha256: str | None = None
    if not args.no_evidence:
        assert wasm is not None
        runtime_wasm_sha256 = validate_artifact_binding(root, wasm)
    elif wasm is not None:
        if not wasm.is_file():
            raise EvidenceError(f"release runtime.wasm is missing: {wasm}")
        runtime_wasm_sha256 = sha256_file(wasm)

    validate_prerequisites(root, selected, zombienet_binary, node_binary)
    if evidence_enabled:
        require_executable(zombienet_binary, "Zombienet binary")
        validate_zombienet_binary_pin(root, zombienet_binary)
    if any(suite.kind == "chopsticks" for suite in selected):
        # Chopsticks persists its fork database. A failed or interrupted prior
        # run must never turn the next release check into a continuation from
        # stale state rather than a fresh import from the bound raw chain spec.
        cleanup_chopsticks_state(root)
    log_dir.mkdir(parents=True, exist_ok=True)
    chopsticks_command: list[str] = []
    if any(suite.kind == "chopsticks" for suite in selected):
        chopsticks_command = (
            args.chopsticks_command
            if args.chopsticks_command is not None
            else default_chopsticks_command(root)
        )
    for suite in selected:
        log_path = log_dir / f"{suite.identifier}.log"
        started = time.monotonic()
        if suite.kind == "zombienet":
            passed, detail = run_zombienet(root, suite, zombienet_binary, log_path)
        else:
            passed, detail = run_chopsticks(
                root,
                suite,
                chopsticks_command,
                log_path,
                runtime_wasm_sha256,
            )
        row: dict[str, Any] = {
            "id": suite.identifier,
            "kind": suite.kind,
            "result": "pass" if passed else "fail",
            "duration_seconds": round(time.monotonic() - started, 3),
            "log": str(log_path),
            "gated_on": list(suite.gated_on),
            "checks": (
                ["zndsl"]
                if suite.kind == "zombienet"
                # The live :code binding only runs when a wasm hash is known;
                # a --no-evidence run without --wasm must not claim it.
                else ["boot", "injected-state", "blocks"]
                + (["code-binding"] if runtime_wasm_sha256 is not None else [])
            ),
        }
        if detail is not None:
            row["detail"] = detail
        rows.append(row)

    order = {suite.identifier: index for index, suite in enumerate(suites)}
    rows.sort(key=lambda row: order[row["id"]])
    report = {
        "schema": REPORT_SCHEMA,
        "commit": release_commit,
        "runtime_wasm_sha256": runtime_wasm_sha256,
        "tier": args.tier,
        "rows": rows,
    }
    if report_out is not None:
        write_json(report_out, report)
    print_summary(rows)

    if any(row["result"] == "fail" for row in rows):
        return 1

    if report_only_reasons:
        print(
            "evidence not emitted: "
            + ", ".join(report_only_reasons)
            + " forces report-only mode"
        )
        return 0

    assert wasm is not None and runtime_wasm_sha256 is not None
    produced = emit_evidence(
        root,
        suites,
        rows,
        wasm,
        runtime_wasm_sha256,
        release_commit,
        args.tier,
    )
    if produced:
        print("evidence produced for: " + ", ".join(produced))
    else:
        print("evidence not emitted: no suite actually ran for a selected kind")
    return 0


def main() -> int:
    try:
        return run()
    except EvidenceError as error:
        print(f"ERROR: {error}", file=sys.stderr)
        return 2
    except Exception as error:
        print(
            f"ERROR: environment evidence runner failed closed: {error}",
            file=sys.stderr,
        )
        return 2
    except KeyboardInterrupt:
        print("ERROR: interrupted", file=sys.stderr)
        return 130


if __name__ == "__main__":
    raise SystemExit(main())

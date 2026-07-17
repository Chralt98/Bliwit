#!/usr/bin/env python3
"""Validate B7 release environments (15 §4.7; 02 §11; 06 §6.2; 09 §7.1)."""

from __future__ import annotations

import argparse
import ast
import json
import re
import sys
from pathlib import Path, PurePosixPath
from typing import Any

import yaml

try:
    import tomllib
except ModuleNotFoundError:  # Python 3.10 compatibility for the local quality gate.
    tomllib = None  # type: ignore[assignment]


PIN_KEYS = {
    "ZOMBIENET_VERSION",
    "ZOMBIENET_SHA256",
    "CHOPSTICKS_VERSION",
    "POLKADOT_SDK_TAG",
    "PASEO_CSG_TAG",
    "PASEO_CSG_COMMIT",
}

# Normative inventories: 15 §4.7 + 09 §7.1.
REQUIRED_DRILLS = {
    "01-smoke.zndsl": "15 §4.7; 02 §11",
    "02-collator-loss.zndsl": "15 §4.7; 09 §7.1",
    "03-keeper-loss.zndsl": "15 §4.7; 09 §7.1",
    "04-dead-man.zndsl": "15 §4.7; 09 §7.1",
    "05-coretime-renewal-under-dead-man.zndsl": "09 §4/§7.1",
    "06-pb-migration.zndsl": "09 §3.2/§7.1",
    "07-xcm-reserve-transfer.zndsl": "15 §4.7; 09 §6.1",
    "08-expedited-code-under-freeze.zndsl": "09 §2.1/§3.2/§7.1",
    "09-three-unattended-epochs.zndsl": "15 §4.7; 09 §7.1; 13 §1",
}

# 02 §11 forked-state rows plus the exhaustive 06 §6.2 playbook registry.
REQUIRED_SCENARIOS = {
    "upgrade-transition.yml": "15 §4.7; 02 §11",
    "stale-queue.yml": "15 §4.7; 02 §11",
    "void-epoch.yml": "15 §4.7; 02 §11",
    "precondition-failures.yml": "15 §4.7; 02 §11",
    "pb-depeg.yml": "06 §6.2",
    "pb-migration.yml": "06 §6.2; 09 §3.2",
    "pb-oracle-void.yml": "06 §6.2",
    "pb-halt-intake.yml": "06 §6.2",
    "pb-reserve.yml": "06 §6.2",
    "pb-ledger-freeze.yml": "06 §6.2/§6.3; 09 §4",
}

NETWORKS = {
    "bleavit-local.toml": {4242},
    "bleavit-xcm.toml": {4242, 1000, 1005},
}
RELAY_SPEC_PATH = "zombienet/specs/out/paseo-local.json"
CHOPSTICKS_GENESIS = "zombienet/specs/out/bleavit-drill-raw.json"
G1_DRILLS = {
    "04-dead-man.zndsl",
    "05-coretime-renewal-under-dead-man.zndsl",
    "08-expedited-code-under-freeze.zndsl",
    "09-three-unattended-epochs.zndsl",
}
# The two well-known raw runtime override keys: `:code` and `:heappages`.
FORBIDDEN_IMPORT_STORAGE_KEYS = {
    "0x3a636f6465",
    "0x3a686561707061676573",
}
NETWORK_HEADER = re.compile(r"^Network:\s*(\S+)\s*$", re.MULTILINE)
JS_REFERENCE = re.compile(r"\bjs-script\s+(\S+)")
ENDPOINT = re.compile(r"\b(?:https?|wss?)://[^\s\"'<>`)]+", re.IGNORECASE)
LOCAL_HOSTS = {"localhost", "127.0.0.1", "[::1]"}
TEXT_NAMES = {".gitignore"}
TEXT_SUFFIXES = {
    ".env",
    ".json",
    ".js",
    ".md",
    ".mjs",
    ".py",
    ".sh",
    ".toml",
    ".yaml",
    ".yml",
    ".zndsl",
}


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Validate B7 Zombienet and Chopsticks release artifacts."
    )
    parser.add_argument(
        "--root",
        type=Path,
        default=Path(__file__).resolve().parents[2],
        help="repository root (defaults to the validator's repository)",
    )
    return parser.parse_args()


def read_text(path: Path, failures: list[str]) -> str | None:
    try:
        return path.read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError) as error:
        failures.append(f"cannot read {path}: {error}")
        return None


def parse_toml_compat(text: str) -> dict[str, Any]:
    """Parse the small TOML subset used by the two committed topology files."""
    root: dict[str, Any] = {}
    current: dict[str, Any] = root

    def descend(parts: list[str]) -> dict[str, Any]:
        node: dict[str, Any] = root
        for part in parts:
            child = node.setdefault(part, {})
            if isinstance(child, list):
                if not child or not isinstance(child[-1], dict):
                    raise ValueError(f"table path {'.'.join(parts)!r} has no active entry")
                node = child[-1]
            elif isinstance(child, dict):
                node = child
            else:
                raise ValueError(f"table path {'.'.join(parts)!r} is not a table")
        return node

    for line_number, raw in enumerate(text.splitlines(), 1):
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        if line.startswith("[[") and line.endswith("]]"):
            parts = line[2:-2].strip().split(".")
            parent = descend(parts[:-1])
            entries = parent.setdefault(parts[-1], [])
            if not isinstance(entries, list):
                raise ValueError(f"line {line_number}: array table conflicts with a value")
            entry: dict[str, Any] = {}
            entries.append(entry)
            current = entry
            continue
        if line.startswith("[") and line.endswith("]"):
            current = descend(line[1:-1].strip().split("."))
            continue
        if "=" not in line:
            raise ValueError(f"line {line_number}: expected key = value")
        key, raw_value = (part.strip() for part in line.split("=", 1))
        if not re.fullmatch(r"[A-Za-z0-9_-]+", key):
            raise ValueError(f"line {line_number}: unsupported key {key!r}")
        if raw_value in {"true", "false"}:
            value: Any = raw_value == "true"
        else:
            try:
                value = ast.literal_eval(raw_value)
            except (SyntaxError, ValueError) as error:
                raise ValueError(
                    f"line {line_number}: unsupported value {raw_value!r}"
                ) from error
        if not isinstance(value, (str, int, bool, list)):
            raise ValueError(f"line {line_number}: unsupported value type")
        current[key] = value
    return root


def load_toml(path: Path, failures: list[str]) -> dict[str, Any] | None:
    try:
        if tomllib is None:
            value = parse_toml_compat(path.read_text(encoding="utf-8"))
        else:
            with path.open("rb") as handle:
                value = tomllib.load(handle)
    except (OSError, UnicodeDecodeError, ValueError) as error:
        failures.append(f"15 §4.7: invalid Zombienet TOML {path}: {error}")
        return None
    if not isinstance(value, dict):
        failures.append(f"15 §4.7: Zombienet TOML {path} must be an object")
        return None
    return value


def load_yaml(path: Path, failures: list[str]) -> dict[str, Any] | None:
    text = read_text(path, failures)
    if text is None:
        return None
    try:
        value = yaml.safe_load(text)
    except yaml.YAMLError as error:
        failures.append(f"15 §4.7: invalid Chopsticks YAML {path}: {error}")
        return None
    if not isinstance(value, dict):
        failures.append(f"15 §4.7: Chopsticks YAML {path} must be an object")
        return None
    return value


def normalize_root_reference(value: str) -> Path:
    return Path(value.removeprefix("./"))


def validate_inventory(root: Path, failures: list[str]) -> None:
    drills = root / "zombienet" / "drills"
    scenarios = root / "chopsticks" / "scenarios"
    for name, citation in REQUIRED_DRILLS.items():
        if not (drills / name).is_file():
            failures.append(f"{citation}: required Zombienet drill is missing: {name}")
    for name, citation in REQUIRED_SCENARIOS.items():
        path = scenarios / name
        if not path.is_file():
            failures.append(f"{citation}: required Chopsticks scenario is missing: {name}")
        if not path.with_suffix(".md").is_file():
            failures.append(f"{citation}: scenario step card is missing: {path.with_suffix('.md').name}")


def validate_suites_manifest(root: Path, failures: list[str]) -> None:
    path = root / "tools" / "env" / "suites.json"
    try:
        document = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
        failures.append(f"15 §4.7; 02 §11: cannot parse suites.json: {error}")
        return
    if not isinstance(document, dict):
        failures.append("15 §4.7; 02 §11: suites.json must contain an object")
        return
    if document.get("schema") != "bleavit.env-suites.v1":
        failures.append(
            "15 §4.7; 02 §11: suites.json schema must be bleavit.env-suites.v1"
        )
    suites = document.get("suites")
    if not isinstance(suites, list):
        failures.append("15 §4.7; 02 §11: suites.json suites must be an array")
        return

    expected = {
        **{
            f"zombienet/drills/{name}": "zombienet"
            for name in REQUIRED_DRILLS
        },
        **{
            f"chopsticks/scenarios/{name}": "chopsticks"
            for name in REQUIRED_SCENARIOS
        },
        "chopsticks/bleavit.yml": "chopsticks",
    }
    identifiers: set[str] = set()
    manifest_paths: set[str] = set()
    for index, row in enumerate(suites):
        label = f"suites.json suites[{index}]"
        if not isinstance(row, dict):
            failures.append(f"15 §4.7; 02 §11: {label} must be an object")
            continue
        identifier = row.get("id")
        relative = row.get("path")
        kind = row.get("kind")
        tier = row.get("tier")
        timeout = row.get("timeout_seconds")
        if not isinstance(identifier, str) or not identifier:
            failures.append(f"15 §4.7; 02 §11: {label}.id must be a non-empty string")
        elif identifier in identifiers:
            failures.append(
                f"15 §4.7; 02 §11: duplicate suites.json id {identifier!r}"
            )
        else:
            identifiers.add(identifier)
        if kind not in {"zombienet", "chopsticks"}:
            failures.append(f"15 §4.7; 02 §11: {label}.kind is invalid")
        if tier not in {"release", "g1"}:
            failures.append(f"15 §4.7; 02 §11: {label}.tier is invalid")
        if type(timeout) is not int or timeout <= 0:
            failures.append(
                f"15 §4.7; 02 §11: {label}.timeout_seconds must be a positive integer"
            )
        if not isinstance(relative, str) or not relative:
            failures.append(f"15 §4.7; 02 §11: {label}.path must be a non-empty string")
            continue
        if relative in manifest_paths:
            failures.append(
                f"15 §4.7; 02 §11: suites.json has a duplicate path: {relative}"
            )
        manifest_paths.add(relative)
        suite_path = root / relative
        if not suite_path.is_file():
            failures.append(
                f"15 §4.7; 02 §11: suites.json path does not exist: {relative}"
            )
        expected_kind = expected.get(relative)
        if expected_kind is None:
            failures.append(
                f"15 §4.7; 02 §11: suites.json points outside the required inventory: {relative}"
            )
        elif kind != expected_kind:
            failures.append(
                f"15 §4.7; 02 §11: suites.json path {relative} must have kind {expected_kind}"
            )
        if expected_kind is not None:
            expected_tier = (
                "g1"
                if relative.startswith("zombienet/drills/")
                and Path(relative).name in G1_DRILLS
                else "release"
            )
            if tier != expected_tier:
                failures.append(
                    f"15 §4.7; 02 §11: suites.json path {relative} must have tier "
                    f"{expected_tier}, found {tier!r}"
                )

    missing = sorted(set(expected) - manifest_paths)
    extra = sorted(manifest_paths - set(expected))
    if missing:
        failures.append(
            "15 §4.7; 02 §11: suites.json is missing required path(s): "
            + ", ".join(missing)
        )
    if extra:
        failures.append(
            "15 §4.7; 02 §11: suites.json has extra path(s): "
            + ", ".join(extra)
        )


def validate_networks(root: Path, failures: list[str]) -> None:
    directory = root / "zombienet" / "networks"
    parsed: dict[str, dict[str, Any]] = {}
    for name, expected_ids in NETWORKS.items():
        path = directory / name
        if not path.is_file():
            failures.append(f"15 §4.7: required Zombienet network is missing: {name}")
            continue
        config = load_toml(path, failures)
        if config is None:
            continue
        parsed[name] = config
        relay = config.get("relaychain")
        if not isinstance(relay, dict):
            failures.append(f"15 §4.7: {name} must define [relaychain]")
        else:
            if relay.get("chain_spec_path") != RELAY_SPEC_PATH:
                failures.append(
                    f"02 §11: {name} relay chain_spec_path must be {RELAY_SPEC_PATH!r}, "
                    f"found {relay.get('chain_spec_path')!r}"
                )
            nodes = relay.get("nodes")
            validator_count = (
                sum(node.get("validator") is True for node in nodes if isinstance(node, dict))
                if isinstance(nodes, list)
                else 0
            )
            if validator_count != 4:
                failures.append(
                    f"09 §7.1: {name} must define exactly four relay validators, "
                    f"found {validator_count}"
                )

        parachains = config.get("parachains")
        if not isinstance(parachains, list):
            failures.append(f"02 §8: {name} must define [[parachains]]")
            continue
        ids = {para.get("id") for para in parachains if isinstance(para, dict)}
        if ids != expected_ids:
            failures.append(
                f"02 §8: {name} parachain ids must be {sorted(expected_ids)}, "
                f"found {sorted(ids, key=str)}"
            )
        bleavit = next(
            (para for para in parachains if isinstance(para, dict) and para.get("id") == 4242),
            None,
        )
        collators = bleavit.get("collators") if isinstance(bleavit, dict) else None
        if not isinstance(collators, list) or len(collators) != 3:
            failures.append(f"15 §4.7: {name} must define three Bleavit collators")

    xcm = parsed.get("bleavit-xcm.toml")
    if xcm is not None:
        channels = xcm.get("hrmp_channels")
        directions = {
            (channel.get("sender"), channel.get("recipient"))
            for channel in channels
            if isinstance(channel, dict)
        } if isinstance(channels, list) else set()
        required = {(4242, 1000), (1000, 4242)}
        missing = required - directions
        if missing:
            failures.append(
                "15 §4.7; 09 §6.1: bleavit-xcm.toml is missing HRMP direction(s): "
                + ", ".join(f"{sender}->{recipient}" for sender, recipient in sorted(missing))
            )


def validate_drills(root: Path, failures: list[str]) -> None:
    for name in REQUIRED_DRILLS:
        path = root / "zombienet" / "drills" / name
        if not path.is_file():
            continue
        text = read_text(path, failures)
        if text is None:
            continue
        match = NETWORK_HEADER.search(text)
        if match is None:
            failures.append(f"15 §4.7: {name} has no Network header")
        else:
            network = root / normalize_root_reference(match.group(1))
            if not network.is_file():
                failures.append(
                    f"15 §4.7: {name} references missing network {match.group(1)!r}"
                )
        for raw in JS_REFERENCE.findall(text):
            # The pinned Zombienet resolves js-script paths from the directory
            # containing the .zndsl test file, not from the process cwd.
            helper = path.parent / raw.strip('"\'')
            if not helper.is_file():
                failures.append(
                    f"15 §4.7: {name} references missing js helper {raw!r}"
                )


def validate_chopsticks(root: Path, failures: list[str]) -> None:
    paths = [root / "chopsticks" / "bleavit.yml"] + [
        root / "chopsticks" / "scenarios" / name for name in REQUIRED_SCENARIOS
    ]
    databases: dict[Path, Path] = {}
    for path in paths:
        if not path.is_file():
            continue
        config = load_yaml(path, failures)
        if config is None:
            continue
        genesis = config.get("genesis")
        if genesis != CHOPSTICKS_GENESIS:
            failures.append(
                f"02 §11: {path.relative_to(root)} genesis must be "
                f"{CHOPSTICKS_GENESIS!r}, found {genesis!r}"
            )
        if "wasm-override" in config:
            failures.append(
                f"02 §11: {path.relative_to(root)} must not define active wasm-override"
            )
        database = config.get("db")
        valid_database = False
        resolved_database: Path | None = None
        if isinstance(database, str) and database:
            pure = PurePosixPath(database)
            if (
                not pure.is_absolute()
                and ".." not in pure.parts
                and "\\" not in database
                and pure.parts[:2] == ("chopsticks", ".state")
                and len(pure.parts) >= 3
            ):
                state_root = (root / "chopsticks" / ".state").resolve()
                resolved_database = (root / Path(*pure.parts)).resolve()
                try:
                    resolved_database.relative_to(state_root)
                    valid_database = resolved_database != state_root
                except ValueError:
                    pass
        if not valid_database or resolved_database is None:
            failures.append(
                f"02 §11: {path.relative_to(root)} db must be repository-relative "
                "under chopsticks/.state/"
            )
        else:
            previous = databases.get(resolved_database)
            if previous is not None:
                failures.append(
                    "02 §11: Chopsticks db paths must be unique; "
                    f"{previous.relative_to(root)} and {path.relative_to(root)} both use "
                    f"{database}"
                )
            else:
                databases[resolved_database] = path
        if config.get("mock-signature-host") is not True:
            failures.append(
                f"15 §4.7: {path.relative_to(root)} must enable mock-signature-host"
            )
        storage = config.get("import-storage")
        if not isinstance(storage, list):
            failures.append(
                f"02 §11: {path.relative_to(root)} import-storage must be raw key/value pairs"
            )
            continue
        for index, row in enumerate(storage):
            if not isinstance(row, list) or len(row) != 2:
                failures.append(
                    f"02 §11: {path.relative_to(root)} import-storage[{index}] "
                    "must be [hex key, hex value]"
                )
                continue
            key, value = row
            if not isinstance(key, str) or re.fullmatch(r"0x(?:[0-9a-fA-F]{2})+", key) is None:
                failures.append(
                    f"02 §11: {path.relative_to(root)} import-storage[{index}] has an invalid key"
                )
            elif key.casefold() in FORBIDDEN_IMPORT_STORAGE_KEYS:
                failures.append(
                    f"02 §11: {path.relative_to(root)} import-storage[{index}] must not "
                    f"inject reserved runtime key {key}"
                )
            if value is not None and (
                not isinstance(value, str)
                or re.fullmatch(r"0x(?:[0-9a-fA-F]{2})*", value) is None
            ):
                failures.append(
                    f"02 §11: {path.relative_to(root)} import-storage[{index}] has an invalid value"
                )


def iter_environment_text(root: Path) -> list[Path]:
    result: list[Path] = []
    for relative in (Path("zombienet"), Path("chopsticks"), Path("tools/env")):
        base = root / relative
        if not base.exists():
            continue
        for path in base.rglob("*"):
            if not path.is_file() or "__pycache__" in path.parts:
                continue
            if path.name in TEXT_NAMES or path.suffix in TEXT_SUFFIXES:
                result.append(path)
    return result


def parse_pins(root: Path, failures: list[str]) -> dict[str, str]:
    path = root / "tools" / "env" / "pins.env"
    text = read_text(path, failures)
    if text is None:
        return {}
    pins: dict[str, str] = {}
    for line_number, raw in enumerate(text.splitlines(), 1):
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        if "=" not in line:
            failures.append(f"pins.env:{line_number}: expected KEY=VALUE")
            continue
        key, value = line.split("=", 1)
        if not re.fullmatch(r"[A-Z][A-Z0-9_]*", key) or not value:
            failures.append(f"pins.env:{line_number}: invalid shell pin assignment")
            continue
        pins[key] = value
    if set(pins) != PIN_KEYS:
        failures.append(
            "15 §4.7: pins.env keys must be exactly "
            f"{sorted(PIN_KEYS)}, found {sorted(pins)}"
        )
    commit = pins.get("PASEO_CSG_COMMIT")
    if commit is not None and re.fullmatch(r"[0-9a-f]{40}", commit) is None:
        failures.append("15 §4.7: PASEO_CSG_COMMIT must be a lowercase 40-hex commit")
    return pins


def validate_pin_single_home(root: Path, failures: list[str]) -> None:
    pins_path = (root / "tools" / "env" / "pins.env").resolve()
    pins = parse_pins(root, failures)
    for path in iter_environment_text(root):
        if path.resolve() == pins_path:
            continue
        text = read_text(path, failures)
        if text is None:
            continue
        for key, value in pins.items():
            if value in text:
                failures.append(
                    f"15 §4.7: {path.relative_to(root)} copies {key}'s pinned value; "
                    "reference tools/env/pins.env instead"
                )


def endpoint_host(endpoint: str) -> str:
    without_scheme = endpoint.split("://", 1)[1]
    authority = without_scheme.split("/", 1)[0]
    if authority.startswith("["):
        return authority.split("]", 1)[0] + "]"
    return authority.rsplit(":", 1)[0] if ":" in authority else authority


def validate_local_endpoints(root: Path, failures: list[str]) -> None:
    for base_name in ("zombienet", "chopsticks"):
        base = root / base_name
        if not base.exists():
            continue
        for path in base.rglob("*"):
            if not path.is_file() or path.suffix not in TEXT_SUFFIXES:
                continue
            text = read_text(path, failures)
            if text is None:
                continue
            for endpoint in ENDPOINT.findall(text):
                host = endpoint_host(endpoint).casefold()
                if host not in LOCAL_HOSTS:
                    failures.append(
                        f"02 §11: non-localhost endpoint in {path.relative_to(root)}: {endpoint}"
                    )


def validate(root: Path) -> list[str]:
    failures: list[str] = []
    root = root.resolve()
    validate_inventory(root, failures)
    validate_suites_manifest(root, failures)
    validate_networks(root, failures)
    validate_drills(root, failures)
    validate_chopsticks(root, failures)
    validate_pin_single_home(root, failures)
    validate_local_endpoints(root, failures)
    return failures


def main() -> int:
    args = parse_args()
    failures = validate(args.root)
    if failures:
        for failure in failures:
            print(f"ERROR: {failure}", file=sys.stderr)
        return 1
    print("B7 environment definitions are structurally valid")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

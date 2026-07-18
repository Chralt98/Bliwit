#!/usr/bin/env python3
"""Out-of-band Bleavit release attestation monitor (12 section 5.2).

``release.json`` field names are O1-owned and not frozen.  The live fetcher
therefore consumes the documented provisional ``bleavit.release.provisional.v1``
schema, while all comparison and signature functions accept already-extracted
files, SHA-256 maps, signature blobs, keyrings, and frozen ReleaseChannel bytes.
O1 can re-key the network adapter without changing the verdict core.

The production module is verify-only.  Tests contain their own deterministic
signer solely to produce RFC/minisign-format fixtures.
"""

from __future__ import annotations

import argparse
import base64
import collections
import hashlib
import json
import logging
import re
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Iterable, Mapping, Sequence

try:
    import tomllib
except ModuleNotFoundError:  # Local gate compatibility; production is Python 3.12.
    import toml_compat as tomllib  # type: ignore[no-redef]

sys.path.insert(0, str(Path(__file__).resolve().parent))

from common import (  # noqa: E402
    MetricStore,
    MonitoringError,
    RELEASE_CHANNEL_KEY,
    ReleaseChannel,
    SeriesDefinition,
    WsRpc,
    decode_release_channel,
    header_number,
    hex_bytes,
    serve_metrics,
)


LOG = logging.getLogger("bleavit-attestation-monitor")
PROVISIONAL_SCHEMA = "bleavit.release.provisional.v1"
TXID = re.compile(r"^[A-Za-z0-9_-]{43}$")
SHA256_HEX = re.compile(r"^[0-9a-f]{64}$")


def _series(name: str, kind: str, help_text: str) -> SeriesDefinition:
    return SeriesDefinition(name, kind, help_text)


SERIES: dict[str, SeriesDefinition] = {
    item.name: item
    for item in (
        _series("bleavit_release_monitor_up", "gauge", "Whether the last complete out-of-band check ran."),
        _series("bleavit_release_monitor_checks_total", "counter", "Completed out-of-band checks."),
        _series("bleavit_release_monitor_errors_total", "counter", "Operational/configuration check failures."),
        _series("bleavit_release_monitor_last_check_timestamp_seconds", "gauge", "Unix time of the last check attempt."),
        _series("bleavit_release_monitor_integrity_ok", "gauge", "Whether every release-integrity predicate passed."),
        _series("bleavit_release_monitor_bundle_byte_mismatches", "gauge", "Files/routes whose bytes differ from the signed map."),
        _series("bleavit_release_monitor_resolver_divergent_gateways", "gauge", "Gateway resolutions differing from ReleaseChannel manifest_txid."),
        _series("bleavit_release_monitor_valid_release_signatures", "gauge", "Valid non-revoked release signatures."),
        _series("bleavit_release_monitor_valid_attestations", "gauge", "Distinct valid non-revoked attestor signatures."),
        _series("bleavit_release_monitor_keyring_generation", "gauge", "Verified release keyring generation."),
        _series("bleavit_release_monitor_manifest_matches_channel", "gauge", "Whether release.json and resolver targets match ReleaseChannel."),
        _series("bleavit_release_monitor_covering_release", "gauge", "Whether the canonical release covers ReleaseChannel spec_version."),
        _series("bleavit_release_monitor_repoint_channel_lag_blocks", "gauge", "Observed finalized blocks since an ArNS target first differed from ReleaseChannel."),
        _series("bleavit_release_monitor_ant_record_changes_total", "counter", "Observed majority ArNS target changes."),
        _series("bleavit_release_monitor_webhook_failures_total", "counter", "Failed release-integrity webhook POSTs."),
    )
}


# RFC 8032 Ed25519 verification -------------------------------------------------
Q = 2**255 - 19
L = 2**252 + 27742317777372353535851937790883648493
D = (-121665 * pow(121666, Q - 2, Q)) % Q
I = pow(2, (Q - 1) // 4, Q)
B_Y = (4 * pow(5, Q - 2, Q)) % Q


def _recover_x(y: int, sign: int) -> int:
    if y >= Q:
        raise ValueError("Ed25519 y coordinate is out of range")
    xx = ((y * y - 1) * pow(D * y * y + 1, Q - 2, Q)) % Q
    x = pow(xx, (Q + 3) // 8, Q)
    if (x * x - xx) % Q:
        x = (x * I) % Q
    if (x * x - xx) % Q:
        raise ValueError("Ed25519 point is not on the curve")
    if x & 1 != sign:
        x = Q - x
    return x


B_X = _recover_x(B_Y, 0)
B = (B_X, B_Y, 1, (B_X * B_Y) % Q)
IDENTITY = (0, 1, 1, 0)


def _point_add(p: tuple[int, int, int, int], q: tuple[int, int, int, int]) -> tuple[int, int, int, int]:
    x1, y1, z1, t1 = p
    x2, y2, z2, t2 = q
    a = ((y1 - x1) * (y2 - x2)) % Q
    b = ((y1 + x1) * (y2 + x2)) % Q
    c = (2 * D * t1 * t2) % Q
    d = (2 * z1 * z2) % Q
    e = (b - a) % Q
    f = (d - c) % Q
    g = (d + c) % Q
    h = (b + a) % Q
    return (e * f % Q, g * h % Q, f * g % Q, e * h % Q)


def _scalar_mult(point: tuple[int, int, int, int], scalar: int) -> tuple[int, int, int, int]:
    result = IDENTITY
    addend = point
    while scalar:
        if scalar & 1:
            result = _point_add(result, addend)
        addend = _point_add(addend, addend)
        scalar >>= 1
    return result


def _decode_point(encoded: bytes) -> tuple[int, int, int, int]:
    if len(encoded) != 32:
        raise ValueError("Ed25519 point must be 32 bytes")
    value = int.from_bytes(encoded, "little")
    y = value & ((1 << 255) - 1)
    x = _recover_x(y, value >> 255)
    point = (x, y, 1, x * y % Q)
    # Reject small-order points; they cannot represent a signing identity.
    if _scalar_mult(point, 8) == IDENTITY:
        raise ValueError("Ed25519 small-order point is rejected")
    return point


def _points_equal(p: tuple[int, int, int, int], q: tuple[int, int, int, int]) -> bool:
    return (p[0] * q[2] - q[0] * p[2]) % Q == 0 and (
        p[1] * q[2] - q[1] * p[2]
    ) % Q == 0


def ed25519_verify(public_key: bytes, message: bytes, signature: bytes) -> bool:
    """Strict RFC 8032 Ed25519 verification using Python integers only."""
    if len(public_key) != 32 or len(signature) != 64:
        return False
    try:
        a = _decode_point(public_key)
        r = _decode_point(signature[:32])
    except ValueError:
        return False
    s = int.from_bytes(signature[32:], "little")
    if s >= L:
        return False
    h = int.from_bytes(
        hashlib.sha512(signature[:32] + public_key + message).digest(), "little"
    ) % L
    return _points_equal(_scalar_mult(B, s), _point_add(r, _scalar_mult(a, h)))


# Minisign parsing -------------------------------------------------------------
@dataclass(frozen=True)
class MinisignPublicKey:
    key_id: bytes
    public_key: bytes


@dataclass(frozen=True)
class MinisignSignature:
    algorithm: bytes
    key_id: bytes
    signature: bytes
    trusted_comment: str
    global_signature: bytes


def _b64(line: str, label: str) -> bytes:
    try:
        return base64.b64decode(line, validate=True)
    except ValueError as error:
        raise ValueError(f"invalid base64 in minisign {label}") from error


def parse_minisign_public_key(text: str) -> MinisignPublicKey:
    lines = [line.strip() for line in text.strip().splitlines() if line.strip()]
    if len(lines) == 1:
        encoded = lines[0]
    elif len(lines) == 2 and lines[0].startswith("untrusted comment:"):
        encoded = lines[1]
    else:
        raise ValueError("minisign public key must contain comment + base64 packet")
    packet = _b64(encoded, "public key")
    if len(packet) != 42 or packet[:2] != b"Ed":
        raise ValueError("minisign public key packet must be Ed + 8-byte id + 32-byte key")
    return MinisignPublicKey(packet[2:10], packet[10:42])


def parse_minisign_signature(text: str) -> MinisignSignature:
    lines = [line.strip() for line in text.strip().splitlines() if line.strip()]
    if len(lines) != 4 or not lines[0].startswith("untrusted comment:"):
        raise ValueError("minisign signature must contain exactly four non-empty lines")
    prefixes = ("trusted comment:", "trusted_comment:")
    prefix = next((candidate for candidate in prefixes if lines[2].startswith(candidate)), None)
    if prefix is None:
        raise ValueError("minisign signature has no trusted comment line")
    packet = _b64(lines[1], "signature")
    global_signature = _b64(lines[3], "global signature")
    if len(packet) != 74 or packet[:2] not in (b"Ed", b"ED"):
        raise ValueError("minisign packet must be Ed/ED + 8-byte id + 64-byte signature")
    if len(global_signature) != 64:
        raise ValueError("minisign global signature must be 64 bytes")
    return MinisignSignature(
        algorithm=packet[:2],
        key_id=packet[2:10],
        signature=packet[10:74],
        trusted_comment=lines[2][len(prefix) :].lstrip(),
        global_signature=global_signature,
    )


def verify_minisign(message: bytes, signature_text: str, public: MinisignPublicKey) -> bool:
    try:
        signature = parse_minisign_signature(signature_text)
    except ValueError:
        return False
    if signature.key_id != public.key_id:
        return False
    signed_message = (
        message
        if signature.algorithm == b"Ed"
        else hashlib.blake2b(message, digest_size=64).digest()
    )
    if not ed25519_verify(public.public_key, signed_message, signature.signature):
        return False
    # Minisign's global signature binds the 64-byte primary signature to the
    # trusted-comment payload (the algorithm/id are already bound by key lookup).
    return ed25519_verify(
        public.public_key,
        signature.signature + signature.trusted_comment.encode("utf-8"),
        signature.global_signature,
    )


@dataclass(frozen=True)
class KeyRecord:
    key_id: bytes
    public_key: MinisignPublicKey
    role: str
    revocation_index: int


@dataclass(frozen=True)
class Keyring:
    generation: int
    keys: Mapping[bytes, KeyRecord]


def load_keyring(path: Path) -> Keyring:
    try:
        with path.open("rb") as handle:
            document = tomllib.load(handle)
    except (OSError, tomllib.TOMLDecodeError) as error:
        raise MonitoringError(f"cannot load keyring {path}: {error}") from error
    generation = document.get("generation")
    rows = document.get("key")
    if not isinstance(generation, int) or generation < 0:
        raise MonitoringError("keyring generation must be a non-negative integer")
    if not isinstance(rows, list) or not rows:
        raise MonitoringError("keyring must contain at least one [[key]]")
    keys: dict[bytes, KeyRecord] = {}
    indexes: set[int] = set()
    for index, row in enumerate(rows, 1):
        if not isinstance(row, dict):
            raise MonitoringError(f"keyring entry {index} must be a table")
        role = row.get("role")
        revocation_index = row.get("revocation_index")
        encoded = row.get("public_key")
        if role not in {"release", "attestor"}:
            raise MonitoringError(f"keyring entry {index} role must be release or attestor")
        if not isinstance(revocation_index, int) or not 0 <= revocation_index < 64:
            raise MonitoringError(f"keyring entry {index} revocation_index must be 0..63")
        if revocation_index in indexes:
            raise MonitoringError(f"keyring revocation_index {revocation_index} is duplicated")
        if not isinstance(encoded, str):
            raise MonitoringError(f"keyring entry {index} public_key must be a string")
        try:
            public = parse_minisign_public_key(encoded)
        except ValueError as error:
            raise MonitoringError(f"keyring entry {index}: {error}") from error
        if public.key_id in keys:
            raise MonitoringError(f"keyring key id {public.key_id.hex()} is duplicated")
        indexes.add(revocation_index)
        keys[public.key_id] = KeyRecord(
            public.key_id, public, role, revocation_index
        )
    return Keyring(generation, keys)


@dataclass(frozen=True)
class IntegrityVerdict:
    ok: bool
    errors: tuple[str, ...]
    byte_mismatches: int
    resolver_divergent_gateways: int
    valid_release_signatures: int
    valid_attestations: int
    manifest_matches_channel: bool
    covering_release: bool


def _valid_signers(
    message: bytes,
    blobs: Sequence[str],
    keyring: Keyring,
    channel: ReleaseChannel,
    role: str,
) -> set[bytes]:
    valid: set[bytes] = set()
    for blob in blobs:
        try:
            signature = parse_minisign_signature(blob)
        except ValueError:
            continue
        record = keyring.keys.get(signature.key_id)
        if record is None or record.role != role:
            continue
        if channel.revoked_key_bits & (1 << record.revocation_index):
            continue
        if verify_minisign(message, blob, record.public_key):
            valid.add(record.key_id)
    return valid


def evaluate_integrity(
    *,
    files: Mapping[str, bytes],
    expected_hashes: Mapping[str, str],
    release_json_bytes: bytes,
    release_document: Mapping[str, Any],
    release_signatures: Sequence[str],
    attestations: Sequence[str],
    keyring: Keyring,
    release_channel_bytes: bytes,
    resolved_txids: Sequence[str],
    minimum_release_signatures: int,
) -> IntegrityVerdict:
    """Format-agnostic comparison core used by live fetching and tamper tests."""
    channel = decode_release_channel(release_channel_bytes)
    errors: list[str] = []
    mismatches = 0
    for path, expected in sorted(expected_hashes.items()):
        value = files.get(path)
        if value is None or SHA256_HEX.fullmatch(expected) is None or hashlib.sha256(value).hexdigest() != expected:
            mismatches += 1
            errors.append(f"byte/hash mismatch: {path}")
    unexpected = sorted(set(files) - set(expected_hashes))
    if unexpected:
        mismatches += len(unexpected)
        errors.append("unlisted served files: " + ", ".join(unexpected))

    manifest_txid = release_document.get("manifest_txid")
    manifest_matches = isinstance(manifest_txid, str) and manifest_txid == channel.manifest_txid
    if not manifest_matches:
        errors.append("release.json manifest_txid differs from ReleaseChannel")
    if hashlib.sha256(release_json_bytes).digest() != channel.release_json_hash:
        errors.append("release.json SHA-256 differs from ReleaseChannel")

    divergent = sum(txid != channel.manifest_txid for txid in resolved_txids)
    if divergent >= 2:
        errors.append(f"{divergent}-of-{len(resolved_txids)} gateway resolvers diverge")

    generation = release_document.get("keyring_generation")
    if generation != channel.keyring_generation or generation != keyring.generation:
        errors.append("release/keyring/ReleaseChannel generation mismatch")

    # 12 section 2.1 says release keys sign release.json's SHA-256 hash.
    signed_message = hashlib.sha256(release_json_bytes).digest()
    release_keys = _valid_signers(
        signed_message, release_signatures, keyring, channel, "release"
    )
    attestor_keys = _valid_signers(
        signed_message, attestations, keyring, channel, "attestor"
    )
    if len(release_keys) < minimum_release_signatures:
        errors.append(
            f"valid release signatures {len(release_keys)} < operator minimum {minimum_release_signatures}"
        )
    if len(attestor_keys) < 2:
        errors.append(f"valid independent attestations {len(attestor_keys)} < 2")

    supported = release_document.get("supported_spec_version")
    covering = (
        isinstance(supported, dict)
        and isinstance(supported.get("min"), int)
        and isinstance(supported.get("max"), int)
        and supported["min"] <= channel.spec_version <= supported["max"]
    )
    if not covering:
        errors.append("canonical release does not cover ReleaseChannel spec_version")
    return IntegrityVerdict(
        not errors,
        tuple(errors),
        mismatches,
        divergent,
        len(release_keys),
        len(attestor_keys),
        manifest_matches,
        covering,
    )


# Configuration and live fetching ---------------------------------------------
@dataclass(frozen=True)
class Gateway:
    name: str
    resolve_url: str
    raw_url: str
    tx_url: str
    name_url: str


@dataclass(frozen=True)
class Config:
    gateways: tuple[Gateway, ...]
    node_urls: tuple[str, ...]
    arns_name: str
    keyring_file: Path
    bind: str
    check_interval_seconds: int
    minimum_release_signatures: int
    max_file_bytes: int
    max_bundle_bytes: int
    webhooks: Mapping[str, tuple[str, ...]]


def _template(value: Any, label: str, fields: Iterable[str]) -> str:
    if not isinstance(value, str) or not value.startswith("https://"):
        raise MonitoringError(f"{label} must be an https:// URL template")
    missing = [field for field in fields if "{" + field + "}" not in value]
    if missing:
        raise MonitoringError(f"{label} is missing placeholders: {', '.join(missing)}")
    return value


def load_config(path: Path) -> Config:
    try:
        with path.open("rb") as handle:
            document = tomllib.load(handle)
    except (OSError, tomllib.TOMLDecodeError) as error:
        raise MonitoringError(f"cannot load config {path}: {error}") from error
    monitor = document.get("monitor")
    gateway_rows = document.get("gateway")
    webhooks = document.get("webhooks")
    if not isinstance(monitor, dict):
        raise MonitoringError("config requires [monitor]")
    if not isinstance(gateway_rows, list) or len(gateway_rows) < 3:
        raise MonitoringError("config requires at least three [[gateway]] entries")
    names: set[str] = set()
    gateways: list[Gateway] = []
    for index, row in enumerate(gateway_rows, 1):
        if not isinstance(row, dict) or not isinstance(row.get("name"), str):
            raise MonitoringError(f"gateway {index} needs a string name")
        name = row["name"]
        if name in names:
            raise MonitoringError(f"gateway name {name!r} is duplicated")
        names.add(name)
        gateways.append(
            Gateway(
                name,
                _template(row.get("resolve_url"), f"gateway {name} resolve_url", ("name",)),
                _template(row.get("raw_url"), f"gateway {name} raw_url", ("txid",)),
                _template(row.get("tx_url"), f"gateway {name} tx_url", ("txid", "path")),
                _template(row.get("name_url"), f"gateway {name} name_url", ("name", "path")),
            )
        )
    node_urls = monitor.get("node_urls")
    if not isinstance(node_urls, list) or not node_urls or not all(
        isinstance(url, str) and url.startswith(("ws://", "wss://")) for url in node_urls
    ):
        raise MonitoringError("monitor.node_urls must be a non-empty ws:// or wss:// list")
    arns_name = monitor.get("arns_name")
    if not isinstance(arns_name, str) or not re.fullmatch(r"[a-z0-9_-]+", arns_name):
        raise MonitoringError("monitor.arns_name must contain lowercase ArNS name characters")
    bind = monitor.get("bind")
    interval = monitor.get("check_interval_seconds")
    minimum = monitor.get("minimum_release_signatures")
    max_file = monitor.get("max_file_bytes")
    max_bundle = monitor.get("max_bundle_bytes")
    if not isinstance(bind, str):
        raise MonitoringError("monitor.bind must be HOST:PORT")
    if not isinstance(interval, int) or not 1 <= interval <= 3600:
        raise MonitoringError("monitor.check_interval_seconds must be 1..3600 (hourly floor)")
    if not isinstance(minimum, int) or minimum < 1:
        raise MonitoringError("monitor.minimum_release_signatures must be operator-supplied and >= 1")
    if not isinstance(max_file, int) or max_file <= 0:
        raise MonitoringError("monitor.max_file_bytes must be operator-supplied and positive")
    if not isinstance(max_bundle, int) or max_bundle < max_file:
        raise MonitoringError("monitor.max_bundle_bytes must be >= max_file_bytes")
    keyring_value = monitor.get("keyring_file")
    if not isinstance(keyring_value, str) or not keyring_value:
        raise MonitoringError("monitor.keyring_file must be a path")
    keyring_file = Path(keyring_value)
    if not keyring_file.is_absolute():
        keyring_file = path.parent / keyring_file
    if not isinstance(webhooks, dict):
        raise MonitoringError("config requires [webhooks]")
    parsed_webhooks: dict[str, tuple[str, ...]] = {}
    for channel in ("paging", "status_page", "community"):
        values = webhooks.get(channel)
        if not isinstance(values, list) or not values or not all(
            isinstance(value, str) and value.startswith("https://") for value in values
        ):
            raise MonitoringError(f"webhooks.{channel} must be a non-empty https:// URL list")
        parsed_webhooks[channel] = tuple(values)
    return Config(
        tuple(gateways),
        tuple(node_urls),
        arns_name,
        keyring_file,
        bind,
        interval,
        minimum,
        max_file,
        max_bundle,
        parsed_webhooks,
    )


class Fetcher:
    def __init__(self, config: Config):
        self.config = config
        self.total = 0

    def get(self, url: str, *, json_value: bool = False) -> Any:
        request = urllib.request.Request(
            url,
            headers={"Cache-Control": "no-cache, no-store", "Pragma": "no-cache", "User-Agent": "bleavit-attestation-monitor/1"},
        )
        try:
            with urllib.request.urlopen(request, timeout=30) as response:
                length = response.headers.get("Content-Length")
                if length is not None and int(length) > self.config.max_file_bytes:
                    raise MonitoringError(f"response exceeds operator max_file_bytes: {url}")
                value = response.read(self.config.max_file_bytes + 1)
        except (OSError, urllib.error.URLError, ValueError) as error:
            raise MonitoringError(f"fetch failed for {url}: {error}") from error
        if len(value) > self.config.max_file_bytes:
            raise MonitoringError(f"response exceeds operator max_file_bytes: {url}")
        self.total += len(value)
        if self.total > self.config.max_bundle_bytes:
            raise MonitoringError("fetch exceeds operator max_bundle_bytes")
        if not json_value:
            return value
        try:
            return json.loads(value)
        except (UnicodeDecodeError, json.JSONDecodeError) as error:
            raise MonitoringError(f"non-JSON response from {url}") from error


def _format_url(template: str, **values: str) -> str:
    return template.format(
        **{
            key: urllib.parse.quote(value, safe="/" if key == "path" else "")
            for key, value in values.items()
        }
    )


def resolve_arns(config: Config, fetcher: Fetcher | None = None) -> list[str]:
    """Resolve the configured name independently through every gateway."""
    client = fetcher or Fetcher(config)
    resolved: list[str] = []
    for gateway in config.gateways:
        resolution = client.get(
            _format_url(gateway.resolve_url, name=config.arns_name), json_value=True
        )
        txid = resolution.get("txId") if isinstance(resolution, dict) else None
        if not isinstance(txid, str) or TXID.fullmatch(txid) is None:
            raise MonitoringError(f"gateway {gateway.name} returned no valid resolver txId")
        resolved.append(txid)
    return resolved


def resolver_consensus(resolved: Sequence[str]) -> str | None:
    """Return a strict-majority TXID, or None when gateways have no consensus."""
    if not resolved:
        return None
    txid, count = collections.Counter(resolved).most_common(1)[0]
    return txid if count * 2 > len(resolved) else None


def fetch_release(config: Config, channel: ReleaseChannel) -> tuple[
    dict[str, bytes], dict[str, Any], bytes, list[str], list[str], list[str]
]:
    fetcher = Fetcher(config)
    resolved = resolve_arns(config, fetcher)
    manifests: list[Mapping[str, Any]] = []
    for gateway, txid in zip(config.gateways, resolved):
        raw_manifest = fetcher.get(
            _format_url(gateway.raw_url, txid=txid),
            json_value=True,
        )
        if not isinstance(raw_manifest, dict):
            raise MonitoringError(f"gateway {gateway.name} manifest is not an object")
        manifests.append(raw_manifest)
    canonical_txid = channel.manifest_txid
    canonical = next(
        (manifest for txid, manifest in zip(resolved, manifests) if txid == canonical_txid),
        manifests[0],
    )
    paths = canonical.get("paths")
    if not isinstance(paths, dict) or not paths:
        raise MonitoringError("Arweave path manifest has no paths object")
    path_names = sorted(paths)
    if not all(isinstance(path, str) and path and ".." not in Path(path).parts for path in path_names):
        raise MonitoringError("Arweave path manifest contains an unsafe/non-string path")

    route_values: dict[str, list[bytes]] = {path: [] for path in path_names}
    for gateway, resolved_txid in zip(config.gateways, resolved):
        for path in path_names:
            route_values[path].append(
                fetcher.get(_format_url(gateway.tx_url, txid=resolved_txid, path=path))
            )
            route_values[path].append(
                fetcher.get(_format_url(gateway.name_url, name=config.arns_name, path=path))
            )
    representative = {path: values[0] for path, values in route_values.items()}
    release_raw = representative.get("release.json")
    if release_raw is None:
        raise MonitoringError("served bundle has no release.json")
    try:
        release_document = json.loads(release_raw)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise MonitoringError("served release.json is invalid JSON") from error
    if not isinstance(release_document, dict) or release_document.get("schema") != PROVISIONAL_SCHEMA:
        raise MonitoringError(f"release.json must use provisional schema {PROVISIONAL_SCHEMA}")
    hashes = release_document.get("files")
    if not isinstance(hashes, dict) or not all(isinstance(k, str) and isinstance(v, str) for k, v in hashes.items()):
        raise MonitoringError("release.json files must map paths to SHA-256 hex")
    expected_paths = set(hashes)
    if set(path_names) != expected_paths | {"release.json"}:
        raise MonitoringError("manifest paths must equal release.json files plus release.json")

    # Route disagreement is represented as extra mismatch pseudo-paths, keeping
    # the pure comparison core unaware of gateway/network shape.
    compared_files = {path: representative[path] for path in expected_paths}
    for path, values in route_values.items():
        if path == "release.json":
            if any(value != release_raw for value in values):
                compared_files[f"__route_mismatch__/{path}"] = b"mismatch"
        elif any(value != representative[path] for value in values):
            compared_files[f"__route_mismatch__/{path}"] = b"mismatch"

    def signature_transactions(field: str) -> list[str]:
        rows = release_document.get(field)
        if not isinstance(rows, list) or not all(isinstance(row, dict) for row in rows):
            raise MonitoringError(f"release.json {field} must be a list of objects")
        blobs: list[str] = []
        for row in rows:
            txid = row.get("txid")
            if not isinstance(txid, str) or TXID.fullmatch(txid) is None:
                raise MonitoringError(f"release.json {field} contains an invalid txid")
            copies = [
                fetcher.get(_format_url(gateway.raw_url, txid=txid))
                for gateway in config.gateways
            ]
            if any(copy != copies[0] for copy in copies):
                raise MonitoringError(f"gateway bytes diverge for {field} transaction {txid}")
            try:
                blobs.append(copies[0].decode("utf-8"))
            except UnicodeDecodeError as error:
                raise MonitoringError(f"{field} transaction {txid} is not UTF-8 minisign") from error
        return blobs

    return (
        compared_files,
        release_document,
        release_raw,
        signature_transactions("release_signatures"),
        signature_transactions("attestations"),
        resolved,
    )


def post_webhooks(config: Config, payload: Mapping[str, Any], store: MetricStore) -> None:
    body = json.dumps(payload, sort_keys=True).encode("utf-8")
    for channel, urls in config.webhooks.items():
        for url in urls:
            request = urllib.request.Request(
                url,
                data=body,
                method="POST",
                headers={"Content-Type": "application/json", "User-Agent": "bleavit-attestation-monitor/1"},
            )
            try:
                with urllib.request.urlopen(request, timeout=10) as response:
                    if not 200 <= response.status < 300:
                        raise MonitoringError(f"HTTP {response.status}")
            except Exception as error:
                store.inc("bleavit_release_monitor_webhook_failures_total")
                LOG.error("%s webhook failed for %s: %s", channel, url, error)


class AttestationMonitor:
    def __init__(self, config: Config, store: MetricStore | None = None):
        self.config = config
        self.store = store or MetricStore(SERIES)
        self.last_majority_txid: str | None = None
        self.last_resolved_txids: tuple[str, ...] | None = None
        self.repoint_mismatch_since: int | None = None
        for counter in (
            "bleavit_release_monitor_checks_total",
            "bleavit_release_monitor_errors_total",
            "bleavit_release_monitor_ant_record_changes_total",
            "bleavit_release_monitor_webhook_failures_total",
        ):
            self.store.set(counter, 0)

    def check(self, release_channel_bytes: bytes, block: int) -> IntegrityVerdict:
        channel = decode_release_channel(release_channel_bytes)
        files, document, release_raw, signatures, attestations, resolved = fetch_release(
            self.config, channel
        )
        hashes = document.get("files", {})
        verdict = evaluate_integrity(
            files=files,
            expected_hashes=hashes,
            release_json_bytes=release_raw,
            release_document=document,
            release_signatures=signatures,
            attestations=attestations,
            keyring=load_keyring(self.config.keyring_file),
            release_channel_bytes=release_channel_bytes,
            resolved_txids=resolved,
            minimum_release_signatures=self.config.minimum_release_signatures,
        )
        majority = resolver_consensus(resolved)
        if self.last_majority_txid is not None and majority != self.last_majority_txid:
            self.store.inc("bleavit_release_monitor_ant_record_changes_total")
        self.last_majority_txid = majority
        self.last_resolved_txids = tuple(resolved)
        if majority != channel.manifest_txid:
            if self.repoint_mismatch_since is None:
                self.repoint_mismatch_since = block
            lag = max(0, block - self.repoint_mismatch_since)
        else:
            self.repoint_mismatch_since = None
            lag = 0
        values = {
            "bleavit_release_monitor_up": 1,
            "bleavit_release_monitor_integrity_ok": int(verdict.ok),
            "bleavit_release_monitor_bundle_byte_mismatches": verdict.byte_mismatches,
            "bleavit_release_monitor_resolver_divergent_gateways": verdict.resolver_divergent_gateways,
            "bleavit_release_monitor_valid_release_signatures": verdict.valid_release_signatures,
            "bleavit_release_monitor_valid_attestations": verdict.valid_attestations,
            "bleavit_release_monitor_keyring_generation": channel.keyring_generation,
            "bleavit_release_monitor_manifest_matches_channel": int(verdict.manifest_matches_channel),
            "bleavit_release_monitor_covering_release": int(verdict.covering_release),
            "bleavit_release_monitor_repoint_channel_lag_blocks": lag,
            "bleavit_release_monitor_last_check_timestamp_seconds": time.time(),
        }
        for name, value in values.items():
            self.store.set(name, value)
        self.store.inc("bleavit_release_monitor_checks_total")
        if not verdict.ok:
            post_webhooks(
                self.config,
                {
                    "alert": "BleavitReleaseIntegrity",
                    "runbook": "RB-RELEASE",
                    "block": block,
                    "manifest_txid": channel.manifest_txid,
                    "errors": list(verdict.errors),
                },
                self.store,
            )
        return verdict

    def resolver_state_changed(self) -> bool:
        """Poll resolver records between full, hourly bundle checks."""
        resolved = tuple(resolve_arns(self.config))
        return self.last_resolved_txids is None or resolved != self.last_resolved_txids

    def note_finalized_head(self, block: int) -> None:
        """Advance the repoint lag gauge without refetching an unchanged bundle."""
        lag = 0
        if self.repoint_mismatch_since is not None:
            lag = max(0, block - self.repoint_mismatch_since)
        self.store.set("bleavit_release_monitor_repoint_channel_lag_blocks", lag)


def read_channel(rpc: WsRpc, block_hash: str) -> bytes:
    raw = hex_bytes(
        rpc.call("state_getStorage", [RELEASE_CHANNEL_KEY, block_hash]),
        "ReleaseChannel storage",
    )
    assert raw is not None
    return raw


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Verify canonical Bleavit releases out of band.")
    parser.add_argument("--config", type=Path, required=True, help="operator TOML configuration")
    parser.add_argument("--bind", help="override monitor.bind for the Prometheus endpoint")
    parser.add_argument("--once", action="store_true", help="check once; exit 0 healthy, 1 mismatch, 2 operational error")
    return parser.parse_args(argv)


def run(args: argparse.Namespace) -> int:
    try:
        config = load_config(args.config)
        if args.bind:
            config = Config(**{**config.__dict__, "bind": args.bind})
    except MonitoringError as error:
        LOG.error("configuration error: %s", error)
        return 2
    store = MetricStore(SERIES)
    monitor = AttestationMonitor(config, store)
    if not args.once:
        try:
            serve_metrics(store, config.bind)
        except (OSError, MonitoringError) as error:
            LOG.error("metrics bind failed: %s", error)
            return 2
    endpoint = 0
    backoff = 1.0
    last_channel: bytes | None = None
    last_check = 0.0
    while True:
        rpc: WsRpc | None = None
        try:
            rpc = WsRpc(config.node_urls[endpoint % len(config.node_urls)])
            block_hash = rpc.call("chain_getFinalizedHead")
            block = header_number(rpc.call("chain_getHeader", [block_hash]))
            channel_bytes = read_channel(rpc, block_hash)
            verdict = monitor.check(channel_bytes, block)
            last_check = time.monotonic()
            last_channel = channel_bytes
            if args.once:
                sys.stdout.write(store.render())
                return 0 if verdict.ok else 1
            subscription = rpc.subscribe_finalized()
            backoff = 1.0
            while True:
                remaining = max(0.1, config.check_interval_seconds - (time.monotonic() - last_check))
                header = rpc.next_finalized(subscription, timeout=remaining)
                if header is None:
                    block_hash = rpc.call("chain_getFinalizedHead")
                    block = header_number(rpc.call("chain_getHeader", [block_hash]))
                else:
                    block_hash = header.get("hash")
                    if not isinstance(block_hash, str):
                        block_hash = rpc.call("chain_getFinalizedHead")
                        block = header_number(rpc.call("chain_getHeader", [block_hash]))
                    else:
                        block = header_number(header)
                channel_bytes = read_channel(rpc, block_hash)
                monitor.note_finalized_head(block)
                channel_changed = channel_bytes != last_channel
                hourly_due = time.monotonic() - last_check >= config.check_interval_seconds
                resolver_changed = False
                if not channel_changed and not hourly_due:
                    resolver_changed = monitor.resolver_state_changed()
                if channel_changed or resolver_changed or hourly_due:
                    monitor.check(channel_bytes, block)
                    last_channel = channel_bytes
                    last_check = time.monotonic()
        except KeyboardInterrupt:
            return 0
        except Exception as error:  # urllib/websocket transports expose varied subclasses.
            store.set("bleavit_release_monitor_up", 0)
            store.set("bleavit_release_monitor_integrity_ok", 0)
            store.set("bleavit_release_monitor_last_check_timestamp_seconds", time.time())
            store.inc("bleavit_release_monitor_errors_total")
            LOG.error("monitor check failed: %s", error)
            post_webhooks(
                config,
                {"alert": "BleavitReleaseIntegrity", "runbook": "RB-RELEASE", "errors": [str(error)]},
                store,
            )
            if args.once:
                sys.stdout.write(store.render())
                return 2
            endpoint += 1
            time.sleep(backoff)
            backoff = min(backoff * 2, 60.0)
        finally:
            if rpc is not None:
                try:
                    rpc.close()
                except Exception:
                    pass


def main(argv: list[str] | None = None) -> int:
    logging.basicConfig(level=logging.INFO, format="%(asctime)s %(levelname)s %(message)s")
    return run(parse_args(argv))


if __name__ == "__main__":
    raise SystemExit(main())

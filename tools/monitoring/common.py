#!/usr/bin/env python3
"""Shared, stdlib-only monitoring primitives for milestone O5.

Network clients import ``websockets`` only when a live connection is opened so
the offline unit suite never needs that optional dependency.
"""

from __future__ import annotations

import json
import math
import re
import threading
from dataclasses import dataclass
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any, Iterable, Mapping

import sys

RELEASE_TOOLS = Path(__file__).resolve().parents[1] / "release"
if str(RELEASE_TOOLS) not in sys.path:
    sys.path.insert(0, str(RELEASE_TOOLS))

from release_common import storage_prefix  # noqa: E402
from scale_metadata import MetadataDecodeError, Reader  # noqa: E402


HEX = re.compile(r"^0x(?:[0-9a-fA-F]{2})*$")
LABEL = re.compile(r"^[a-zA-Z_][a-zA-Z0-9_]*$")
METRIC = re.compile(r"^[a-zA-Z_:][a-zA-Z0-9_:]*$")
RELEASE_CHANNEL_LENGTH = 168
RELEASE_CHANNEL_KEY = storage_prefix("Constitution", "ReleaseChannel")


class MonitoringError(RuntimeError):
    """Crisp live/configuration failure surfaced by both O5 daemons."""


class ScaleValueError(ValueError):
    """Portable-registry SCALE value decoding failure."""


@dataclass(frozen=True)
class ReleaseChannel:
    schema: int
    version: str
    manifest_txid: str
    release_json_hash: bytes
    updated_at: int
    spec_version: int
    pending_authorized_at: int
    min_supported_version: str
    keyring_generation: int
    revoked_key_bits: int
    flags: int

    @property
    def security(self) -> bool:
        return bool(self.flags & 1)

    @property
    def expedited(self) -> bool:
        return bool(self.flags & 2)

    @property
    def urgent_upgrade(self) -> bool:
        return bool(self.flags & 4)


def _zero_padded_text(value: bytes, field: str) -> str:
    raw = value.rstrip(b"\0")
    if b"\0" in raw:
        raise MonitoringError(f"ReleaseChannel {field} contains embedded NUL")
    try:
        return raw.decode("utf-8")
    except UnicodeDecodeError as error:
        raise MonitoringError(f"ReleaseChannel {field} is not UTF-8") from error


def decode_release_channel(value: bytes) -> ReleaseChannel:
    """Decode the frozen 02 section 12 prefix by fixed byte offset."""
    if len(value) < RELEASE_CHANNEL_LENGTH:
        raise MonitoringError(
            f"ReleaseChannel is {len(value)} bytes; frozen prefix needs 168"
        )
    prefix = value[:RELEASE_CHANNEL_LENGTH]
    flags = int.from_bytes(prefix[164:168], "little")
    if flags & ~0b111:
        raise MonitoringError("ReleaseChannel reserved flag bits are non-zero")
    return ReleaseChannel(
        schema=prefix[0],
        version=_zero_padded_text(prefix[1:33], "version"),
        manifest_txid=_zero_padded_text(prefix[33:76], "manifest_txid"),
        release_json_hash=prefix[76:108],
        updated_at=int.from_bytes(prefix[108:112], "little"),
        spec_version=int.from_bytes(prefix[112:116], "little"),
        pending_authorized_at=int.from_bytes(prefix[116:120], "little"),
        min_supported_version=_zero_padded_text(
            prefix[120:152], "min_supported_version"
        ),
        keyring_generation=int.from_bytes(prefix[152:156], "little"),
        revoked_key_bits=int.from_bytes(prefix[156:164], "little"),
        flags=flags,
    )


def hex_bytes(value: Any, label: str, *, optional: bool = False) -> bytes | None:
    if value is None and optional:
        return None
    if not isinstance(value, str) or HEX.fullmatch(value) is None:
        raise MonitoringError(f"{label} is not an even-length 0x hex value")
    return bytes.fromhex(value[2:])


def _primitive(reader: Reader, name: str) -> Any:
    widths = {
        "u8": 1,
        "u16": 2,
        "u32": 4,
        "u64": 8,
        "u128": 16,
        "u256": 32,
        "i8": 1,
        "i16": 2,
        "i32": 4,
        "i64": 8,
        "i128": 16,
        "i256": 32,
    }
    if name == "bool":
        value = reader.u8()
        if value not in (0, 1):
            raise ScaleValueError(f"invalid SCALE bool discriminant {value}")
        return bool(value)
    if name == "char":
        value = int.from_bytes(reader.take(4), "little")
        try:
            return chr(value)
        except ValueError as error:
            raise ScaleValueError(f"invalid SCALE char {value}") from error
    if name == "str":
        try:
            return reader.bytes().decode("utf-8")
        except UnicodeDecodeError as error:
            raise ScaleValueError("invalid UTF-8 SCALE string") from error
    width = widths.get(name)
    if width is None:
        raise ScaleValueError(f"unsupported SCALE primitive {name}")
    return int.from_bytes(reader.take(width), "little", signed=name.startswith("i"))


def decode_scale_value(
    reader: Reader,
    type_id: int,
    types: Mapping[int, Mapping[str, Any]],
    *,
    depth: int = 0,
) -> Any:
    """Decode a value generically from the v14/v15 portable type registry.

    Transparent one-field tuple structs are unwrapped. Variants retain their
    portable name and index so mapping code never depends on numeric ordering.
    """
    if depth > 128:
        raise ScaleValueError("portable SCALE value exceeds 128 nested types")
    item = types.get(type_id)
    if item is None:
        raise ScaleValueError(f"portable registry has no type id {type_id}")
    definition = item["definition"]
    kind = definition["kind"]
    nested = depth + 1
    if kind == "primitive":
        return _primitive(reader, definition["primitive"])
    if kind == "compact":
        return reader.compact()
    if kind == "sequence":
        length = reader.compact()
        return [
            decode_scale_value(reader, definition["type_id"], types, depth=nested)
            for _ in range(length)
        ]
    if kind == "array":
        return [
            decode_scale_value(reader, definition["type_id"], types, depth=nested)
            for _ in range(definition["length"])
        ]
    if kind == "tuple":
        return [
            decode_scale_value(reader, child, types, depth=nested)
            for child in definition["type_ids"]
        ]
    if kind == "composite":
        fields = definition["fields"]
        values = [
            decode_scale_value(reader, field["type_id"], types, depth=nested)
            for field in fields
        ]
        if all(field["name"] is not None for field in fields):
            return {field["name"]: value for field, value in zip(fields, values)}
        if len(values) == 0:
            return None
        if len(values) == 1:
            return values[0]
        return values
    if kind == "variant":
        index = reader.u8()
        matches = [variant for variant in definition["variants"] if variant["index"] == index]
        if len(matches) != 1:
            raise ScaleValueError(
                f"type {type_id} has no unique variant at SCALE index {index}"
            )
        variant = matches[0]
        fields = variant["fields"]
        values = [
            decode_scale_value(reader, field["type_id"], types, depth=nested)
            for field in fields
        ]
        if all(field["name"] is not None for field in fields):
            payload: Any = {
                field["name"]: value for field, value in zip(fields, values)
            }
        elif len(values) == 0:
            payload = None
        elif len(values) == 1:
            payload = values[0]
        else:
            payload = values
        return {"variant": variant["name"], "index": index, "fields": payload}
    if kind == "bit_sequence":
        raise ScaleValueError("portable bit-sequence values are not used by O5")
    raise ScaleValueError(f"unsupported portable SCALE definition {kind}")


def decode_typed_bytes(data: bytes, type_id: int, metadata: Mapping[str, Any]) -> Any:
    reader = Reader(data)
    try:
        value = decode_scale_value(reader, type_id, metadata["types"])
    except MetadataDecodeError as error:
        raise ScaleValueError(str(error)) from error
    if reader.offset != len(data):
        raise ScaleValueError(
            f"SCALE value left {len(data) - reader.offset} trailing bytes"
        )
    return value


def variant_name(value: Any) -> str | None:
    return value.get("variant") if isinstance(value, dict) else None


def nested_field(value: Any, *path: str) -> Any:
    cursor = value
    for item in path:
        if not isinstance(cursor, dict) or item not in cursor:
            return None
        cursor = cursor[item]
    return cursor


@dataclass(frozen=True)
class SeriesDefinition:
    name: str
    kind: str
    help: str
    labels: tuple[str, ...] = ()

    def __post_init__(self) -> None:
        if METRIC.fullmatch(self.name) is None:
            raise ValueError(f"invalid Prometheus metric name {self.name!r}")
        if self.kind not in {"gauge", "counter"}:
            raise ValueError(f"invalid Prometheus metric kind {self.kind!r}")
        if any(LABEL.fullmatch(label) is None for label in self.labels):
            raise ValueError(f"invalid labels for {self.name}")


class MetricStore:
    def __init__(self, definitions: Mapping[str, SeriesDefinition]):
        self.definitions = dict(definitions)
        self.values: dict[tuple[str, tuple[tuple[str, str], ...]], float] = {}
        self._lock = threading.Lock()

    def _key(self, name: str, labels: Mapping[str, str] | None) -> tuple[str, tuple[tuple[str, str], ...]]:
        definition = self.definitions[name]
        supplied = labels or {}
        if set(supplied) != set(definition.labels):
            raise ValueError(
                f"{name} labels {sorted(supplied)} do not match {list(definition.labels)}"
            )
        return name, tuple((label, str(supplied[label])) for label in definition.labels)

    def set(self, name: str, value: float | int, labels: Mapping[str, str] | None = None) -> None:
        numeric = float(value)
        if not math.isfinite(numeric):
            raise ValueError(f"{name} value must be finite")
        with self._lock:
            self.values[self._key(name, labels)] = numeric

    def inc(self, name: str, amount: float | int = 1, labels: Mapping[str, str] | None = None) -> None:
        numeric = float(amount)
        if numeric < 0 or not math.isfinite(numeric):
            raise ValueError(f"counter increment for {name} must be finite and non-negative")
        with self._lock:
            key = self._key(name, labels)
            self.values[key] = self.values.get(key, 0.0) + numeric

    def clear_family(self, name: str) -> None:
        with self._lock:
            self.values = {key: value for key, value in self.values.items() if key[0] != name}

    def render(self) -> str:
        lines: list[str] = []
        with self._lock:
            snapshot = dict(self.values)
        for name, definition in sorted(self.definitions.items()):
            lines.append(f"# HELP {name} {definition.help}")
            lines.append(f"# TYPE {name} {definition.kind}")
            rows = sorted((key, value) for key, value in snapshot.items() if key[0] == name)
            for (_, labels), value in rows:
                suffix = ""
                if labels:
                    encoded = ",".join(
                        f'{key}="{_escape_label(label)}"' for key, label in labels
                    )
                    suffix = "{" + encoded + "}"
                rendered = str(int(value)) if value.is_integer() else format(value, ".17g")
                lines.append(f"{name}{suffix} {rendered}")
        return "\n".join(lines) + "\n"


def _escape_label(value: str) -> str:
    return value.replace("\\", "\\\\").replace("\n", "\\n").replace('"', '\\"')


def serve_metrics(store: MetricStore, bind: str) -> ThreadingHTTPServer:
    host, port = parse_bind(bind)

    class Handler(BaseHTTPRequestHandler):
        def do_GET(self) -> None:  # noqa: N802 - BaseHTTPRequestHandler API.
            if self.path not in ("/", "/metrics"):
                self.send_error(404)
                return
            payload = store.render().encode("utf-8")
            self.send_response(200)
            self.send_header("Content-Type", "text/plain; version=0.0.4; charset=utf-8")
            self.send_header("Content-Length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)

        def log_message(self, _format: str, *_args: Any) -> None:
            return

    server = ThreadingHTTPServer((host, port), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server


def parse_bind(value: str) -> tuple[str, int]:
    if value.count(":") != 1:
        raise MonitoringError("--bind must be HOST:PORT (IPv4/hostname form)")
    host, raw_port = value.rsplit(":", 1)
    if not host:
        raise MonitoringError("--bind host must not be empty")
    try:
        port = int(raw_port)
    except ValueError as error:
        raise MonitoringError("--bind port must be an integer") from error
    if not 1 <= port <= 65535:
        raise MonitoringError("--bind port must be between 1 and 65535")
    return host, port


class WsRpc:
    """Small synchronous JSON-RPC client with notification buffering."""

    def __init__(self, url: str, timeout: float = 20.0):
        if not url.startswith(("ws://", "wss://")):
            raise MonitoringError("node URL must start with ws:// or wss://")
        try:
            from websockets.sync.client import connect
        except ImportError as error:
            raise MonitoringError(
                "live monitoring requires websockets==15.0.1"
            ) from error
        self.connection = connect(url, open_timeout=timeout, close_timeout=2, max_size=None)
        self.timeout = timeout
        self.next_id = 1
        self.notifications: list[dict[str, Any]] = []

    def close(self) -> None:
        self.connection.close()

    def call(self, method: str, params: Iterable[Any] = ()) -> Any:
        request_id = self.next_id
        self.next_id += 1
        self.connection.send(
            json.dumps(
                {"jsonrpc": "2.0", "id": request_id, "method": method, "params": list(params)},
                separators=(",", ":"),
            )
        )
        while True:
            raw = self.connection.recv(timeout=self.timeout)
            response = json.loads(raw)
            if not isinstance(response, dict):
                raise MonitoringError(f"{method} returned a non-object JSON-RPC message")
            if response.get("id") != request_id:
                if "method" in response:
                    self.notifications.append(response)
                continue
            if response.get("error") is not None:
                raise MonitoringError(f"{method} failed: {response['error']!r}")
            if "result" not in response:
                raise MonitoringError(f"{method} returned no result")
            return response["result"]

    def subscribe_finalized(self) -> str:
        result = self.call("chain_subscribeFinalizedHeads")
        if not isinstance(result, str):
            raise MonitoringError("chain_subscribeFinalizedHeads returned no subscription id")
        return result

    def next_finalized(self, subscription: str, timeout: float | None = None) -> dict[str, Any] | None:
        while True:
            if self.notifications:
                message = self.notifications.pop(0)
            else:
                try:
                    raw = self.connection.recv(timeout=timeout or self.timeout)
                except TimeoutError:
                    return None
                message = json.loads(raw)
            if not isinstance(message, dict):
                continue
            if message.get("method") != "chain_finalizedHead":
                continue
            params = message.get("params", {})
            if params.get("subscription") != subscription:
                continue
            result = params.get("result")
            return result if isinstance(result, dict) else None


def header_number(value: Any) -> int:
    if not isinstance(value, dict) or not isinstance(value.get("number"), str):
        raise MonitoringError("finalized header has no hex number")
    try:
        return int(value["number"], 16)
    except ValueError as error:
        raise MonitoringError(f"invalid finalized header number {value['number']!r}") from error


def compact_encode(value: int) -> bytes:
    if value < 0:
        raise ValueError("compact value must be non-negative")
    if value < 1 << 6:
        return bytes([value << 2])
    if value < 1 << 14:
        return ((value << 2) | 1).to_bytes(2, "little")
    if value < 1 << 30:
        return ((value << 2) | 2).to_bytes(4, "little")
    width = max(4, (value.bit_length() + 7) // 8)
    return bytes([((width - 4) << 2) | 3]) + value.to_bytes(width, "little")


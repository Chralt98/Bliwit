#!/usr/bin/env python3
"""Pure helpers shared by the Bleavit release tooling."""

from __future__ import annotations

import hashlib
import json
import os
import re
import subprocess
from pathlib import Path
from typing import Any


MASK64 = (1 << 64) - 1
XXH64_PRIME1 = 11400714785074694791
XXH64_PRIME2 = 14029467366897019727
XXH64_PRIME3 = 1609587929392839161
XXH64_PRIME4 = 9650029242287828579
XXH64_PRIME5 = 2870177450012600261


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def write_json(path: Path, value: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        json.dumps(value, indent=2, sort_keys=True, ensure_ascii=False) + "\n",
        encoding="utf-8",
    )


def repo_root() -> Path:
    return Path(__file__).resolve().parents[2]


def git_value(root: Path, *args: str, default: str = "unknown") -> str:
    try:
        return subprocess.run(
            ["git", *args],
            cwd=root,
            check=True,
            capture_output=True,
            text=True,
        ).stdout.strip()
    except (OSError, subprocess.CalledProcessError):
        return default


def source_date_epoch(root: Path) -> int:
    raw = os.environ.get("SOURCE_DATE_EPOCH")
    if raw is None:
        raw = git_value(root, "show", "-s", "--format=%ct", "HEAD", default="0")
    try:
        value = int(raw)
    except ValueError as error:
        raise ValueError("SOURCE_DATE_EPOCH must be an integer") from error
    if value < 0:
        raise ValueError("SOURCE_DATE_EPOCH must be non-negative")
    return value


def _rotl64(value: int, count: int) -> int:
    return ((value << count) | (value >> (64 - count))) & MASK64


def _round64(accumulator: int, lane: int) -> int:
    accumulator = (accumulator + lane * XXH64_PRIME2) & MASK64
    accumulator = _rotl64(accumulator, 31)
    return (accumulator * XXH64_PRIME1) & MASK64


def xxh64(data: bytes, seed: int = 0) -> int:
    """Return XXH64 using the canonical little-endian algorithm."""
    length = len(data)
    offset = 0
    if length >= 32:
        v1 = (seed + XXH64_PRIME1 + XXH64_PRIME2) & MASK64
        v2 = (seed + XXH64_PRIME2) & MASK64
        v3 = seed & MASK64
        v4 = (seed - XXH64_PRIME1) & MASK64
        limit = length - 32
        while offset <= limit:
            lanes = [
                int.from_bytes(data[offset + index : offset + index + 8], "little")
                for index in (0, 8, 16, 24)
            ]
            v1 = _round64(v1, lanes[0])
            v2 = _round64(v2, lanes[1])
            v3 = _round64(v3, lanes[2])
            v4 = _round64(v4, lanes[3])
            offset += 32
        result = (
            _rotl64(v1, 1)
            + _rotl64(v2, 7)
            + _rotl64(v3, 12)
            + _rotl64(v4, 18)
        ) & MASK64
        for lane in (v1, v2, v3, v4):
            lane = _round64(0, lane)
            result ^= lane
            result = (result * XXH64_PRIME1 + XXH64_PRIME4) & MASK64
    else:
        result = (seed + XXH64_PRIME5) & MASK64

    result = (result + length) & MASK64
    while offset + 8 <= length:
        lane = int.from_bytes(data[offset : offset + 8], "little")
        result ^= _round64(0, lane)
        result = (_rotl64(result, 27) * XXH64_PRIME1 + XXH64_PRIME4) & MASK64
        offset += 8
    if offset + 4 <= length:
        lane = int.from_bytes(data[offset : offset + 4], "little")
        result ^= (lane * XXH64_PRIME1) & MASK64
        result = (_rotl64(result, 23) * XXH64_PRIME2 + XXH64_PRIME3) & MASK64
        offset += 4
    while offset < length:
        result ^= (data[offset] * XXH64_PRIME5) & MASK64
        result = (_rotl64(result, 11) * XXH64_PRIME1) & MASK64
        offset += 1

    result ^= result >> 33
    result = (result * XXH64_PRIME2) & MASK64
    result ^= result >> 29
    result = (result * XXH64_PRIME3) & MASK64
    result ^= result >> 32
    return result & MASK64


def twox128(text: str) -> bytes:
    encoded = text.encode("utf-8")
    return xxh64(encoded, 0).to_bytes(8, "little") + xxh64(encoded, 1).to_bytes(
        8, "little"
    )


def storage_prefix(pallet: str, item: str) -> str:
    return "0x" + (twox128(pallet) + twox128(item)).hex()


def safe_filename(identifier: str) -> str:
    return re.sub(r"[^A-Za-z0-9._-]+", "-", identifier).strip("-") or "surface"


"""Tiny Python-3.10 fallback for O5's deliberately small TOML documents.

CI and the supported runtime use Python 3.12/tomllib.  This parser exists only
so repository-wide developer gates on older Python can still exercise O5.
"""

from __future__ import annotations

import ast
from typing import Any, BinaryIO


class TOMLDecodeError(ValueError):
    pass


def loads(text: str) -> dict[str, Any]:
    root: dict[str, Any] = {}
    current = root
    for line_number, raw in enumerate(text.splitlines(), 1):
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        if line.startswith("[[") and line.endswith("]]" ):
            name = line[2:-2].strip()
            if not name or "." in name:
                raise TOMLDecodeError(f"line {line_number}: unsupported array table")
            rows = root.setdefault(name, [])
            if not isinstance(rows, list):
                raise TOMLDecodeError(f"line {line_number}: table kind collision")
            current = {}
            rows.append(current)
            continue
        if line.startswith("[") and line.endswith("]"):
            name = line[1:-1].strip()
            if not name or "." in name:
                raise TOMLDecodeError(f"line {line_number}: unsupported table")
            value = root.setdefault(name, {})
            if not isinstance(value, dict):
                raise TOMLDecodeError(f"line {line_number}: table kind collision")
            current = value
            continue
        if "=" not in line:
            raise TOMLDecodeError(f"line {line_number}: expected key = value")
        key, raw_value = (part.strip() for part in line.split("=", 1))
        if not key.replace("_", "").replace("-", "").isalnum():
            raise TOMLDecodeError(f"line {line_number}: unsupported key {key!r}")
        if raw_value in {"true", "false"}:
            value: Any = raw_value == "true"
        else:
            try:
                value = ast.literal_eval(raw_value)
            except (SyntaxError, ValueError) as error:
                raise TOMLDecodeError(
                    f"line {line_number}: unsupported TOML value"
                ) from error
        if not isinstance(value, (str, int, bool, list)):
            raise TOMLDecodeError(f"line {line_number}: unsupported value type")
        current[key] = value
    return root


def load(handle: BinaryIO) -> dict[str, Any]:
    try:
        return loads(handle.read().decode("utf-8"))
    except UnicodeDecodeError as error:
        raise TOMLDecodeError("TOML input is not UTF-8") from error


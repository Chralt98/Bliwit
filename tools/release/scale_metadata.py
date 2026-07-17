#!/usr/bin/env python3
"""Small stdlib-only decoder for the FRAME metadata surface used by B8.

The decoder intentionally stops after the v15 runtime-API section, but it keeps
the complete portable type definitions needed to render storage, constant,
event, and runtime-API layouts.  Renderings contain no registry IDs, so they are
stable when unrelated types are inserted into the metadata registry.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any


class MetadataDecodeError(ValueError):
    pass


HASHER_NAMES = (
    "Blake2_128",
    "Blake2_256",
    "Blake2_128Concat",
    "Twox128",
    "Twox256",
    "Twox64Concat",
    "Identity",
)
PRIMITIVE_NAMES = (
    "bool",
    "char",
    "str",
    "u8",
    "u16",
    "u32",
    "u64",
    "u128",
    "u256",
    "i8",
    "i16",
    "i32",
    "i64",
    "i128",
    "i256",
)


@dataclass
class Reader:
    data: bytes
    offset: int = 0

    def take(self, count: int) -> bytes:
        end = self.offset + count
        if count < 0 or end > len(self.data):
            raise MetadataDecodeError(
                f"metadata truncated at offset {self.offset}, need {count} bytes"
            )
        value = self.data[self.offset : end]
        self.offset = end
        return value

    def u8(self) -> int:
        return self.take(1)[0]

    def u32(self) -> int:
        return int.from_bytes(self.take(4), "little")

    def compact(self) -> int:
        first = self.u8()
        mode = first & 0b11
        if mode == 0:
            return first >> 2
        if mode == 1:
            return int.from_bytes(bytes([first]) + self.take(1), "little") >> 2
        if mode == 2:
            return int.from_bytes(bytes([first]) + self.take(3), "little") >> 2
        byte_count = (first >> 2) + 4
        return int.from_bytes(self.take(byte_count), "little")

    def vec(self, decoder):
        return [decoder() for _ in range(self.compact())]

    def bytes(self) -> bytes:
        return self.take(self.compact())

    def string(self) -> str:
        try:
            return self.bytes().decode("utf-8")
        except UnicodeDecodeError as error:
            raise MetadataDecodeError("invalid UTF-8 metadata string") from error

    def option(self, decoder):
        discriminant = self.u8()
        if discriminant == 0:
            return None
        if discriminant == 1:
            return decoder()
        raise MetadataDecodeError(f"invalid Option discriminant {discriminant}")


def _skip_docs(reader: Reader) -> None:
    reader.vec(reader.string)


def _type_id(reader: Reader) -> int:
    return reader.compact()


def _decode_field(reader: Reader) -> dict[str, Any]:
    field = {
        "name": reader.option(reader.string),
        "type_id": _type_id(reader),
        "type_name": reader.option(reader.string),
    }
    _skip_docs(reader)
    return field


def _decode_variant(reader: Reader) -> dict[str, Any]:
    variant = {
        "name": reader.string(),
        "fields": reader.vec(lambda: _decode_field(reader)),
        "index": reader.u8(),
    }
    _skip_docs(reader)
    return variant


def _decode_type(reader: Reader) -> dict[str, Any]:
    type_id = reader.compact()
    path = reader.vec(reader.string)

    def type_parameter() -> dict[str, Any]:
        return {"name": reader.string(), "type_id": reader.option(lambda: _type_id(reader))}

    type_parameters = reader.vec(type_parameter)
    definition_tag = reader.u8()
    if definition_tag == 0:  # Composite
        definition = {"kind": "composite", "fields": reader.vec(lambda: _decode_field(reader))}
    elif definition_tag == 1:  # Variant
        definition = {"kind": "variant", "variants": reader.vec(lambda: _decode_variant(reader))}
    elif definition_tag == 2:  # Sequence
        definition = {"kind": "sequence", "type_id": _type_id(reader)}
    elif definition_tag == 3:  # Array
        definition = {
            "kind": "array",
            "length": reader.u32(),
            "type_id": _type_id(reader),
        }
    elif definition_tag == 4:  # Tuple
        definition = {"kind": "tuple", "type_ids": reader.vec(lambda: _type_id(reader))}
    elif definition_tag == 5:  # Primitive
        primitive = reader.u8()
        if primitive >= len(PRIMITIVE_NAMES):
            raise MetadataDecodeError(f"unsupported primitive type {primitive}")
        definition = {"kind": "primitive", "primitive": PRIMITIVE_NAMES[primitive]}
    elif definition_tag == 6:  # Compact
        definition = {"kind": "compact", "type_id": _type_id(reader)}
    elif definition_tag == 7:  # BitSequence
        definition = {
            "kind": "bit_sequence",
            "store_type_id": _type_id(reader),
            "order_type_id": _type_id(reader),
        }
    else:
        raise MetadataDecodeError(f"unsupported scale-info TypeDef {definition_tag}")
    _skip_docs(reader)
    return {
        "id": type_id,
        "path": path,
        "type_parameters": type_parameters,
        "definition": definition,
    }


def _decode_storage(reader: Reader) -> dict[str, Any]:
    prefix = reader.string()

    def entry() -> dict[str, Any]:
        name = reader.string()
        modifier = reader.u8()
        entry_type = reader.u8()
        if entry_type == 0:
            value_type = _type_id(reader)
            kind = "plain"
            key_type = None
            hashers: list[int] = []
        elif entry_type == 1:
            hashers = reader.vec(reader.u8)
            if any(hasher >= len(HASHER_NAMES) for hasher in hashers):
                raise MetadataDecodeError(f"invalid storage hasher in {name}")
            key_type = _type_id(reader)
            value_type = _type_id(reader)
            kind = "map"
        else:
            raise MetadataDecodeError(f"invalid storage entry type {entry_type}")
        default = reader.bytes()
        _skip_docs(reader)
        return {
            "name": name,
            "modifier": modifier,
            "kind": kind,
            "hashers": [HASHER_NAMES[item] for item in hashers],
            "key_type": key_type,
            "value_type": value_type,
            "default": default,
        }

    entries = reader.vec(entry)
    return {"prefix": prefix, "entries": {entry["name"]: entry for entry in entries}}


def _decode_pallet(reader: Reader, version: int) -> dict[str, Any]:
    name = reader.string()
    storage = reader.option(lambda: _decode_storage(reader))
    calls = reader.option(lambda: _type_id(reader))
    event_type = reader.option(lambda: _type_id(reader))

    def constant() -> dict[str, Any]:
        constant_name = reader.string()
        type_id = _type_id(reader)
        value = reader.bytes()
        _skip_docs(reader)
        return {"name": constant_name, "type_id": type_id, "value": value}

    constants_list = reader.vec(constant)
    error_type = reader.option(lambda: _type_id(reader))
    index = reader.u8()
    if version == 15:
        _skip_docs(reader)
    return {
        "name": name,
        "storage": storage,
        "calls_type": calls,
        "event_type": event_type,
        "constants": {constant["name"]: constant for constant in constants_list},
        "error_type": error_type,
        "index": index,
    }


def _skip_extrinsic(reader: Reader, version: int) -> None:
    if version == 14:
        _type_id(reader)
        reader.u8()
    else:
        reader.u8()
        for _ in range(4):
            _type_id(reader)

    def signed_extension() -> None:
        reader.string()
        _type_id(reader)
        _type_id(reader)

    reader.vec(signed_extension)


def _decode_runtime_api(reader: Reader) -> dict[str, Any]:
    name = reader.string()

    def method() -> dict[str, Any]:
        method_name = reader.string()

        def parameter() -> dict[str, Any]:
            return {"name": reader.string(), "type_id": _type_id(reader)}

        inputs = reader.vec(parameter)
        output = _type_id(reader)
        _skip_docs(reader)
        return {"name": method_name, "inputs": inputs, "output_type": output}

    methods = reader.vec(method)
    _skip_docs(reader)
    return {"name": name, "methods": {item["name"]: item for item in methods}}


def decode_metadata(data: bytes) -> dict[str, Any]:
    reader = Reader(data)
    magic = reader.u32()
    if magic != 0x6174656D:
        raise MetadataDecodeError(f"invalid metadata magic 0x{magic:08x}")
    version = reader.u8()
    if version not in (14, 15):
        raise MetadataDecodeError(
            f"metadata v{version} is unsupported; expected v14 or v15"
        )
    type_list = reader.vec(lambda: _decode_type(reader))
    types = {item["id"]: item for item in type_list}
    pallet_list = reader.vec(lambda: _decode_pallet(reader, version))
    pallets = {item["name"]: item for item in pallet_list}
    _skip_extrinsic(reader, version)
    _type_id(reader)  # Runtime type.
    api_list = reader.vec(lambda: _decode_runtime_api(reader)) if version == 15 else []
    apis = {item["name"]: item for item in api_list}
    for pallet in pallets.values():
        event_type = types.get(pallet["event_type"], {})
        variants = event_type.get("definition", {}).get("variants", [])
        pallet["events"] = {variant["name"]: variant for variant in variants}
    return {"version": version, "types": types, "pallets": pallets, "apis": apis}


def _render_fields(
    fields: list[dict[str, Any]], types: dict[int, dict[str, Any]], stack: tuple[int, ...]
) -> str:
    rendered = []
    named = any(field["name"] is not None for field in fields)
    for field in fields:
        value = render_type(field["type_id"], types, stack)
        if named:
            rendered.append(f"{field['name'] or '_'}:{value}")
        else:
            rendered.append(value)
    return ",".join(rendered)


def render_type(
    type_id: int, types: dict[int, dict[str, Any]], stack: tuple[int, ...] = ()
) -> str:
    """Render a portable type recursively without unstable registry IDs.

    Known limitation: const-generic bounds are not present in the portable
    registry (`BoundedVec<T, ConstU32<N>>` renders identically for every N),
    so two layouts differing only in a bound render the same. Bounds are
    covered separately by the paired metadata constants in the surface
    manifest, never by this rendering.
    """
    item = types.get(type_id)
    if item is None:
        return "<missing-type>"
    path = "::".join(item["path"])
    if type_id in stack:
        return path or "<recursive>"
    nested = (*stack, type_id)
    definition = item["definition"]
    kind = definition["kind"]
    if kind == "primitive":
        body = definition["primitive"]
    elif kind == "composite":
        fields = definition["fields"]
        if not fields:
            body = "unit"
        elif any(field["name"] is not None for field in fields):
            body = "{" + _render_fields(fields, types, nested) + "}"
        else:
            body = "(" + _render_fields(fields, types, nested) + ")"
    elif kind == "variant":
        variants = []
        for variant in definition["variants"]:
            fields = variant["fields"]
            if not fields:
                suffix = ""
            elif any(field["name"] is not None for field in fields):
                suffix = "{" + _render_fields(fields, types, nested) + "}"
            else:
                suffix = "(" + _render_fields(fields, types, nested) + ")"
            variants.append(f"{variant['name']}={variant['index']}{suffix}")
        body = "enum[" + "|".join(variants) + "]"
    elif kind == "sequence":
        body = f"Vec<{render_type(definition['type_id'], types, nested)}>"
    elif kind == "array":
        body = f"[{render_type(definition['type_id'], types, nested)};{definition['length']}]"
    elif kind == "tuple":
        values = [render_type(value, types, nested) for value in definition["type_ids"]]
        body = "(" + ",".join(values) + ("," if len(values) == 1 else "") + ")"
    elif kind == "compact":
        body = f"Compact<{render_type(definition['type_id'], types, nested)}>"
    elif kind == "bit_sequence":
        store = render_type(definition["store_type_id"], types, nested)
        order = render_type(definition["order_type_id"], types, nested)
        body = f"BitSequence<{store},{order}>"
    else:
        raise MetadataDecodeError(f"cannot render type definition {kind}")
    if path and body != path and kind != "primitive":
        return f"{path}{body}"
    return body


def surface_layout(metadata: dict[str, Any], entry: dict[str, Any]) -> dict[str, Any] | None:
    """Return the resolved layout for a present manifest entry."""
    kind = entry["kind"]
    types = metadata["types"]
    if kind == "storage":
        pallet = metadata["pallets"].get(entry["pallet"])
        storage = pallet.get("storage") if pallet else None
        item = storage["entries"].get(entry["item"]) if storage else None
        if item is None:
            return None
        return {
            "hashers": item["hashers"],
            "key": render_type(item["key_type"], types) if item["key_type"] is not None else None,
            "value": render_type(item["value_type"], types),
        }
    if kind == "constant":
        pallet = metadata["pallets"].get(entry["pallet"])
        item = pallet.get("constants", {}).get(entry["constant"]) if pallet else None
        if item is None:
            return None
        return {"type": render_type(item["type_id"], types), "value": "0x" + item["value"].hex()}
    if kind == "event":
        pallet = metadata["pallets"].get(entry["pallet"])
        item = pallet.get("events", {}).get(entry["event"]) if pallet else None
        if item is None:
            return None
        return {
            "fields": [
                {"name": field["name"], "type": render_type(field["type_id"], types)}
                for field in item["fields"]
            ]
        }
    if kind == "runtime_api":
        api = metadata.get("apis", {}).get(entry["api"])
        method = api.get("methods", {}).get(entry["method"]) if api else None
        if method is None:
            return None
        return {
            "params": [
                {"name": item["name"], "type": render_type(item["type_id"], types)}
                for item in method["inputs"]
            ],
            "return": render_type(method["output_type"], types),
        }
    return None


def compare_layout(actual: dict[str, Any] | None, expected: dict[str, Any]) -> tuple[bool, str]:
    if actual == expected:
        return True, "layout matches"
    return False, "layout mismatch"


def surface_presence(metadata: dict[str, Any], entry: dict[str, Any]) -> tuple[bool, str]:
    kind = entry["kind"]
    if kind == "runtime_api":
        api = metadata.get("apis", {}).get(entry["api"])
        if api is None:
            return False, f"runtime API {entry['api']} absent from metadata"
        if entry["method"] not in api["methods"]:
            return False, f"runtime API method {entry['api']}.{entry['method']} absent from metadata"
        return True, "present"
    if kind == "storage":
        pallet = metadata["pallets"].get(entry["pallet"])
        if pallet is None:
            return False, f"pallet {entry['pallet']} absent from metadata"
        storage = pallet.get("storage")
        if storage is None or entry["item"] not in storage["entries"]:
            return False, f"storage {entry['pallet']}.{entry['item']} absent from metadata"
        return True, "present"
    if kind == "constant":
        pallet = metadata["pallets"].get(entry["pallet"])
        if pallet is None:
            return False, f"pallet {entry['pallet']} absent from metadata"
        if entry["constant"] not in pallet["constants"]:
            return False, f"constant {entry['pallet']}.{entry['constant']} absent from metadata"
        return True, "present"
    if kind == "event":
        pallet = metadata["pallets"].get(entry["pallet"])
        if pallet is None or entry["event"] not in pallet["events"]:
            return False, f"event variant absent: {entry['pallet']}.{entry['event']}"
        return True, "present"
    return False, f"unknown metadata surface kind {kind}"

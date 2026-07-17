#!/usr/bin/env python3
"""Boot a release node and extract its SCALE metadata and runtime identity."""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from node_boot import JsonRpcHttp, NodeProcess
from release_common import write_json
from scale_metadata import MetadataDecodeError, decode_metadata


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--node", type=Path, default=Path("target/release/bleavit-node"))
    parser.add_argument(
        "--chain-spec",
        type=Path,
        default=Path("deploy/chain-specs/out/bleavit-dev.json"),
    )
    parser.add_argument(
        "--wasm", type=Path, default=Path("release-work/runtime/runtime.wasm")
    )
    parser.add_argument("--out-dir", type=Path, default=Path("release-work/runtime"))
    parser.add_argument("--boot-timeout", type=float, default=120.0)
    return parser.parse_args()


def decode_hex(raw: str, label: str) -> bytes:
    if not isinstance(raw, str) or not raw.startswith("0x"):
        raise ValueError(f"{label} was not a 0x-prefixed hex string")
    try:
        return bytes.fromhex(raw[2:])
    except ValueError as error:
        raise ValueError(f"{label} contained invalid hex") from error


def bound_wasm_hashes(wasm_bytes: bytes, on_chain_hex: str) -> tuple[str, str]:
    """Return matching file/on-chain hashes or fail closed on the boot binding."""
    on_chain_wasm = decode_hex(on_chain_hex, "state_getStorage(:code)")
    file_hash = hashlib.sha256(wasm_bytes).hexdigest()
    on_chain_hash = hashlib.sha256(on_chain_wasm).hexdigest()
    if file_hash != on_chain_hash:
        raise RuntimeError(
            "booted runtime :code does not match --wasm: "
            f"file sha256={file_hash}, on-chain sha256={on_chain_hash}"
        )
    return file_hash, on_chain_hash


def main() -> int:
    args = parse_args()
    if not args.wasm.is_file():
        raise FileNotFoundError(f"runtime wasm not found: {args.wasm}")
    wasm_bytes = args.wasm.read_bytes()
    args.out_dir.mkdir(parents=True, exist_ok=True)

    with NodeProcess(
        args.node, args.chain_spec, boot_timeout=args.boot_timeout
    ) as node:
        rpc = JsonRpcHttp(node.http_url)
        metadata = decode_hex(rpc.call("state_getMetadata"), "state_getMetadata")
        runtime_version = rpc.call("state_getRuntimeVersion")
        properties = rpc.call("system_properties")
        on_chain_code = rpc.call("state_getStorage", ["0x3a636f6465"])

    wasm_file_sha256, on_chain_wasm_sha256 = bound_wasm_hashes(
        wasm_bytes, on_chain_code
    )

    metadata_path = args.out_dir / "metadata.scale"
    metadata_path.write_bytes(metadata)
    metadata_sha = hashlib.sha256(metadata).hexdigest()

    contract_version = None
    contract_status = "not found in decoded metadata constants"
    metadata_version = None
    try:
        decoded = decode_metadata(metadata)
        metadata_version = decoded["version"]
        constant = (
            decoded["pallets"]
            .get("Constitution", {})
            .get("constants", {})
            .get("INTEGRATION_CONTRACT_VERSION")
        )
        if constant is not None and len(constant["value"]) == 4:
            contract_version = int.from_bytes(constant["value"], "little")
            contract_status = "decoded from Constitution metadata constant"
    except MetadataDecodeError as error:
        contract_status = f"metadata decoder could not inspect constant: {error}"

    info = {
        "schema": "bleavit.runtime-info.v1",
        "spec_name": runtime_version.get("specName"),
        "spec_version": runtime_version.get("specVersion"),
        "impl_name": runtime_version.get("implName"),
        "impl_version": runtime_version.get("implVersion"),
        "authoring_version": runtime_version.get("authoringVersion"),
        "transaction_version": runtime_version.get("transactionVersion"),
        "state_version": runtime_version.get("stateVersion"),
        "integration_contract_version": contract_version,
        "integration_contract_version_status": contract_status,
        "metadata_version": metadata_version,
        "metadata_sha256": metadata_sha,
        "metadata_hash_kind": "sha256-of-raw-scale-metadata",
        "rfc78_merkleized_metadata_hash": None,
        "rfc78_status": "not enabled by the runtime build recipe",
        "wasm_sha256": wasm_file_sha256,
        "wasm_file_sha256": wasm_file_sha256,
        "on_chain_wasm_sha256": on_chain_wasm_sha256,
        "system_properties": properties,
    }
    write_json(args.out_dir / "runtime-info.json", info)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

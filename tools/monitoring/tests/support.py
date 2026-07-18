from __future__ import annotations

import base64
import hashlib
import json
import sys
from pathlib import Path
from typing import Any

MONITORING = Path(__file__).resolve().parents[1]
if str(MONITORING) not in sys.path:
    sys.path.insert(0, str(MONITORING))

import attestation_monitor as am


def encode_point(point: tuple[int, int, int, int]) -> bytes:
    x, y, z, _ = point
    inverse = pow(z, am.Q - 2, am.Q)
    affine_x = x * inverse % am.Q
    affine_y = y * inverse % am.Q
    return (affine_y | ((affine_x & 1) << 255)).to_bytes(32, "little")


def keypair(seed_byte: int, key_id_byte: int) -> tuple[bytes, bytes, bytes]:
    seed = bytes([seed_byte]) * 32
    digest = hashlib.sha512(seed).digest()
    scalar = int.from_bytes(digest[:32], "little")
    scalar &= (1 << 254) - 8
    scalar |= 1 << 254
    public = encode_point(am._scalar_mult(am.B, scalar))
    return seed, public, bytes([key_id_byte]) * 8


def sign(seed: bytes, public: bytes, message: bytes) -> bytes:
    digest = hashlib.sha512(seed).digest()
    scalar = int.from_bytes(digest[:32], "little")
    scalar &= (1 << 254) - 8
    scalar |= 1 << 254
    nonce = int.from_bytes(hashlib.sha512(digest[32:] + message).digest(), "little") % am.L
    encoded_r = encode_point(am._scalar_mult(am.B, nonce))
    challenge = int.from_bytes(
        hashlib.sha512(encoded_r + public + message).digest(), "little"
    ) % am.L
    s = (nonce + challenge * scalar) % am.L
    return encoded_r + s.to_bytes(32, "little")


def public_text(public: bytes, key_id: bytes) -> str:
    packet = b"Ed" + key_id + public
    return "untrusted comment: test public key\n" + base64.b64encode(packet).decode() + "\n"


def minisign_text(
    seed: bytes,
    public: bytes,
    key_id: bytes,
    message: bytes,
    algorithm: bytes = b"ED",
    trusted: str = "timestamp:1 file:fixture",
) -> str:
    signed = message if algorithm == b"Ed" else hashlib.blake2b(message, digest_size=64).digest()
    signature = sign(seed, public, signed)
    global_signature = sign(seed, public, signature + trusted.encode())
    packet = algorithm + key_id + signature
    return (
        "untrusted comment: test signature\n"
        + base64.b64encode(packet).decode()
        + "\ntrusted comment: "
        + trusted
        + "\n"
        + base64.b64encode(global_signature).decode()
        + "\n"
    )


def release_channel_bytes(
    *,
    manifest_txid: str = "A" * 43,
    release_json_hash: bytes = b"R" * 32,
    generation: int = 7,
    revoked: int = 0,
    flags: int = 0,
    spec_version: int = 42,
    updated_at: int = 100,
    pending: int = 0,
) -> bytes:
    value = bytearray(168)
    value[0] = 1
    value[1:33] = b"1.2.3".ljust(32, b"\0")
    value[33:76] = manifest_txid.encode().ljust(43, b"\0")
    value[76:108] = release_json_hash
    value[108:112] = updated_at.to_bytes(4, "little")
    value[112:116] = spec_version.to_bytes(4, "little")
    value[116:120] = pending.to_bytes(4, "little")
    value[120:152] = b"1.0.0".ljust(32, b"\0")
    value[152:156] = generation.to_bytes(4, "little")
    value[156:164] = revoked.to_bytes(8, "little")
    value[164:168] = flags.to_bytes(4, "little")
    return bytes(value)


def integrity_fixture() -> dict[str, Any]:
    files = {"index.html": b"<h1>Bleavit</h1>", "app.js": b"console.log('ok')"}
    document = {
        "schema": am.PROVISIONAL_SCHEMA,
        "manifest_txid": "A" * 43,
        "keyring_generation": 7,
        "supported_spec_version": {"min": 40, "max": 50},
        "files": {name: hashlib.sha256(value).hexdigest() for name, value in files.items()},
        "release_signatures": [],
        "attestations": [],
    }
    release_raw = json.dumps(document, sort_keys=True, separators=(",", ":")).encode()
    key_specs = [
        (11, 21, "release", 0),
        (12, 22, "attestor", 1),
        (13, 23, "attestor", 2),
    ]
    records: dict[bytes, am.KeyRecord] = {}
    signatures: list[str] = []
    attestations: list[str] = []
    message = hashlib.sha256(release_raw).digest()
    for seed_byte, id_byte, role, revocation_index in key_specs:
        seed, public, key_id = keypair(seed_byte, id_byte)
        parsed = am.parse_minisign_public_key(public_text(public, key_id))
        records[key_id] = am.KeyRecord(key_id, parsed, role, revocation_index)
        blob = minisign_text(seed, public, key_id, message)
        (signatures if role == "release" else attestations).append(blob)
    return {
        "files": files,
        "hashes": document["files"],
        "document": document,
        "release_raw": release_raw,
        "keyring": am.Keyring(7, records),
        "signatures": signatures,
        "attestations": attestations,
        "channel": release_channel_bytes(release_json_hash=hashlib.sha256(release_raw).digest()),
        "resolved": ["A" * 43] * 3,
    }


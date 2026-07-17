#!/usr/bin/env python3
"""Record deterministic chainHead v1 fixtures for the frozen critical surface."""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
import time
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent))

from node_boot import JsonRpcError, JsonRpcHttp, NodeProcess
from release_common import safe_filename, storage_prefix, write_json
from scale_metadata import (
    MetadataDecodeError,
    compare_layout,
    decode_metadata,
    surface_layout,
    surface_presence,
)
from surface_checks import (
    check_expected_value,
    nonempty_hex,
    properties_match,
    storage_value_from_chainhead,
    validate_release_channel,
)
from transcript import deterministic_json, normalized_transcript


class ChainHeadError(RuntimeError):
    pass


class ChainHeadTimeout(ChainHeadError):
    pass


class DeadlineBudget:
    def __init__(self, total_seconds: float, clock=time.monotonic):
        if total_seconds <= 0:
            raise ValueError("recording deadline must be positive")
        self.clock = clock
        self.deadline = clock() + total_seconds

    def operation(self, seconds: float) -> float:
        if seconds <= 0:
            raise ValueError("operation deadline must be positive")
        return min(self.deadline, self.clock() + seconds)

    def remaining(self, deadline: float) -> float:
        remaining = min(deadline, self.deadline) - self.clock()
        if remaining <= 0:
            raise ChainHeadTimeout("chainHead deadline exceeded")
        return remaining


class ChainHeadSession:
    def __init__(
        self,
        ws_url: str,
        operation_timeout: float = 120.0,
        recording_timeout: float = 1800.0,
        clock=time.monotonic,
        budget: DeadlineBudget | None = None,
    ):
        from websockets.sync.client import connect

        self.connection = connect(ws_url, open_timeout=15, max_size=None)
        self.operation_timeout = operation_timeout
        self.budget = budget or DeadlineBudget(recording_timeout, clock=clock)
        self.next_id = 1
        self.notifications: list[dict[str, Any]] = []
        self.subscription_id: str | None = None
        self.block_hash: str | None = None

    def close(self) -> None:
        self.connection.close()

    def _send(self, method: str, params: list[Any]) -> int:
        # The recording budget is charged before every send so an exhausted
        # deadline fails here rather than after another blocking write. A
        # loopback send on these payload sizes cannot meaningfully block in
        # the kernel buffer; the receive side carries the real timeouts.
        self.budget.remaining(self.budget.operation(self.operation_timeout))
        request_id = self.next_id
        self.next_id += 1
        self.connection.send(
            json.dumps(
                {
                    "jsonrpc": "2.0",
                    "id": request_id,
                    "method": method,
                    "params": params,
                },
                separators=(",", ":"),
            )
        )
        return request_id

    def _receive(self, deadline: float) -> dict[str, Any]:
        try:
            raw = self.connection.recv(timeout=self.budget.remaining(deadline))
        except TimeoutError as error:
            raise ChainHeadTimeout("chainHead websocket receive timed out") from error
        payload = json.loads(raw)
        if not isinstance(payload, dict):
            raise ChainHeadError("websocket returned a non-object JSON-RPC message")
        return payload

    def _response(self, request_id: int, deadline: float) -> dict[str, Any]:
        while True:
            self.budget.remaining(deadline)
            payload = self._receive(deadline)
            if payload.get("id") == request_id:
                return payload
            if payload.get("method") == "chainHead_v1_followEvent":
                self.notifications.append(payload)

    def start(self) -> None:
        deadline = self.budget.operation(self.operation_timeout)
        request_id = self._send("chainHead_v1_follow", [True])
        response = self._response(request_id, deadline)
        if "error" in response:
            raise ChainHeadError(f"chainHead_v1_follow failed: {response['error']}")
        self.subscription_id = response.get("result")
        if not isinstance(self.subscription_id, str):
            raise ChainHeadError("chainHead_v1_follow returned no subscription id")

        initialized = False
        while self.block_hash is None:
            event = self._next_follow_event(deadline)
            if event.get("event") == "initialized":
                hashes = event.get("finalizedBlockHashes", [])
                if not hashes:
                    raise ChainHeadError("follow initialized without a finalized block")
                initialized = True
            elif initialized and event.get("event") == "finalized":
                hashes = event.get("finalizedBlockHashes", [])
                if hashes:
                    # Pin the first freshly finalized block rather than genesis:
                    # System.Events has real SCALE bytes only after a block has
                    # executed, which the frozen event fixtures must publish.
                    self.block_hash = hashes[-1]
            elif event.get("event") == "stop":
                raise ChainHeadError("follow subscription stopped before initialization")

    def _next_follow_event(self, deadline: float) -> dict[str, Any]:
        while True:
            self.budget.remaining(deadline)
            if self.notifications:
                payload = self.notifications.pop(0)
            else:
                payload = self._receive(deadline)
            if payload.get("method") != "chainHead_v1_followEvent":
                continue
            params = payload.get("params", {})
            if params.get("subscription") != self.subscription_id:
                continue
            result = params.get("result")
            if isinstance(result, dict):
                return result

    def call(self, method: str, trailing_params: list[Any]) -> tuple[dict[str, Any], bool]:
        if self.subscription_id is None or self.block_hash is None:
            raise ChainHeadError("follow session has not been initialized")
        params = [self.subscription_id, self.block_hash, *trailing_params]
        deadline = self.budget.operation(self.operation_timeout)
        request_id = self._send(method, params)
        try:
            direct = self._response(request_id, deadline)
        except ChainHeadTimeout as error:
            return {"params": params, "response": {"timeout": str(error)}}, False
        response: dict[str, Any] = {"direct": direct}
        if "error" in direct:
            return {"params": params, "response": response}, False
        if method == "chainHead_v1_header":
            return {"params": params, "response": response}, direct.get("result") is not None

        started = direct.get("result")
        if not isinstance(started, dict) or started.get("result") != "started":
            return {"params": params, "response": response}, False
        operation_id = started.get("operationId")
        if not isinstance(operation_id, str):
            return {"params": params, "response": response}, False

        events: list[dict[str, Any]] = []
        success = False
        while True:
            try:
                event = self._next_follow_event(deadline)
            except ChainHeadTimeout as error:
                response["events"] = events
                response["timeout"] = str(error)
                return {"params": params, "response": response}, False
            if event.get("operationId") != operation_id:
                # New-block/finality notifications are intentionally omitted: they
                # aren't responses to this operation and make fixtures timing-based.
                continue
            events.append(event)
            event_name = event.get("event")
            if event_name == "operationWaitingForContinue":
                continue_id = self._send(
                    "chainHead_v1_continue", [self.subscription_id, operation_id]
                )
                try:
                    response["continue"] = self._response(continue_id, deadline)
                except ChainHeadTimeout as error:
                    response["events"] = events
                    response["timeout"] = str(error)
                    return {"params": params, "response": response}, False
            elif event_name in ("operationCallDone", "operationStorageDone"):
                success = True
                break
            elif event_name in (
                "operationError",
                "operationInaccessible",
                "stop",
            ):
                break
        response["events"] = events
        return {"params": params, "response": response}, success


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--node", type=Path, default=Path("target/release/bleavit-node"))
    parser.add_argument(
        "--chain-spec",
        type=Path,
        default=Path("deploy/chain-specs/out/bleavit-dev.json"),
    )
    parser.add_argument(
        "--manifest",
        type=Path,
        default=Path(__file__).resolve().with_name("surface-manifest.json"),
    )
    parser.add_argument(
        "--metadata", type=Path, default=Path("release-work/runtime/metadata.scale")
    )
    parser.add_argument("--out-dir", type=Path, default=Path("release-work/chainhead"))
    parser.add_argument("--boot-timeout", type=float, default=120.0)
    parser.add_argument("--operation-timeout", type=float, default=120.0)
    parser.add_argument("--recording-timeout", type=float, default=1800.0)
    parser.add_argument("--allow-missing", action="store_true")
    parser.add_argument(
        "--classic-only",
        action="store_true",
        help="exercise the explicit no-websockets degradation path",
    )
    return parser.parse_args()


def metadata_request(
    entry: dict[str, Any],
    present: bool,
    detail: str,
    rendered_layout: dict[str, Any] | None,
    layout_matches: bool | None,
) -> dict[str, Any]:
    return {
        "method": "metadata_presence",
        "params": {"kind": entry["kind"], "surface": entry["id"]},
        "response": {
            "present": present,
            "detail": detail,
            "layout": rendered_layout,
            "expected_layout": entry.get("layout"),
            "layout_matches": layout_matches,
        },
    }


def metadata_status(
    metadata: dict[str, Any], entry: dict[str, Any]
) -> tuple[bool, str, dict[str, Any] | None, bool | None]:
    present, detail = surface_presence(metadata, entry)
    rendered = surface_layout(metadata, entry) if present else None
    expected = entry.get("layout")
    layout_matches = None
    if present and expected is not None:
        layout_matches, detail = compare_layout(rendered, expected)
    return present, detail, rendered, layout_matches


def call_failure_reason(result: dict[str, Any], fallback: str) -> str:
    response = result.get("response", {})
    timeout = response.get("timeout")
    return f"timeout: {timeout}" if isinstance(timeout, str) else fallback


def classic_call(
    rpc: JsonRpcHttp,
    budget: DeadlineBudget,
    method: str,
    params: list[Any],
) -> tuple[Any, bool]:
    """Classic JSON-RPC read charged against the shared recording budget."""
    try:
        timeout = budget.remaining(budget.operation(rpc.timeout))
    except ChainHeadTimeout as error:
        return {"error": str(error)}, False
    try:
        return rpc.call(method, params, timeout=timeout), True
    except JsonRpcError as error:
        return {"error": str(error)}, False


def record_surface(
    entry: dict[str, Any],
    session: ChainHeadSession,
    rpc: JsonRpcHttp,
    metadata: dict[str, Any],
    budget: DeadlineBudget,
) -> tuple[list[dict[str, Any]], bool, str]:
    requests: list[dict[str, Any]] = []
    kind = entry["kind"]

    if kind == "raw_storage":
        # 02 §12 fixes the raw key and offset layout precisely so readers need
        # no metadata; a metadata rename must never gate this surface.
        raw_key = entry["raw_key"]
        chainhead, chainhead_ok = session.call(
            "chainHead_v1_storage", [[{"key": raw_key, "type": "value"}], None]
        )
        requests.append(
            {
                "method": "chainHead_v1_storage",
                "params": chainhead["params"],
                "response": chainhead["response"],
            }
        )
        classic, classic_ok = classic_call(
            rpc, budget, "state_getStorage", [raw_key, session.block_hash]
        )
        requests.append(
            {
                "method": "state_getStorage",
                "params": [raw_key, session.block_hash],
                "response": classic,
            }
        )
        if not (chainhead_ok and classic_ok):
            return (
                requests,
                False,
                call_failure_reason(chainhead, "raw storage read failed"),
            )
        chainhead_value = storage_value_from_chainhead(chainhead["response"])
        classic_value = classic if isinstance(classic, str) else None
        if chainhead_value != classic_value:
            return (
                requests,
                False,
                "chainHead and classic reads disagree on the raw value",
            )
        ok, reason = validate_release_channel(
            classic_value, bool(entry.get("value_optional"))
        )
        return requests, ok, reason

    if kind == "properties":
        properties, properties_ok = classic_call(rpc, budget, "system_properties", [])
        requests.append(
            {
                "method": "system_properties",
                "params": [],
                "response": properties,
            }
        )
        if not properties_ok:
            return requests, False, "system_properties request failed"
        ok, reason = properties_match(properties, entry["expected"])
        return requests, ok, reason

    present, presence_detail, rendered_layout, layout_matches = metadata_status(
        metadata, entry
    )
    metadata_ok = present and layout_matches is not False

    if kind == "runtime_api":
        result, success = session.call(
            "chainHead_v1_call", [entry["state_call"], entry["params_hex"]]
        )
        requests.append(
            {
                "method": "chainHead_v1_call",
                "params": result["params"],
                "response": result["response"],
            }
        )
        requests.append(
            metadata_request(
                entry, present, presence_detail, rendered_layout, layout_matches
            )
        )
        success = success and metadata_ok
        if success:
            detail = "recorded"
        elif layout_matches is False:
            detail = "layout mismatch"
        else:
            detail = call_failure_reason(result, "runtime call failed") if present else presence_detail
        return requests, success, detail

    if kind == "storage":
        raw_key = entry.get("raw_key")
        if raw_key is None:
            storage = metadata.get("pallets", {}).get(entry["pallet"], {}).get(
                "storage"
            )
            prefix = storage["prefix"] if storage is not None else entry["pallet"]
            raw_key = storage_prefix(prefix, entry["item"])
        query_type = "descendantsValues" if entry["query"] == "prefix" else "value"
        result, rpc_success = session.call(
            "chainHead_v1_storage", [[{"key": raw_key, "type": query_type}], None]
        )
        requests.append(
            {
                "method": "chainHead_v1_storage",
                "params": result["params"],
                "response": result["response"],
            }
        )
        requests.append(
            metadata_request(
                entry, present, presence_detail, rendered_layout, layout_matches
            )
        )
        fallback_success = True
        if entry.get("classic_rpc_fallback"):
            fallback_response, fallback_success = classic_call(
                rpc, budget, "state_getStorage", [raw_key]
            )
            requests.append(
                {
                    "method": "state_getStorage",
                    "params": [raw_key],
                    "response": fallback_response,
                }
            )
        exact_success = True
        exact_detail = "recorded"
        if "exact_key" in entry:
            # The verdict for a value-frozen identity row (02 §8) comes from an
            # exact-key read decoded against the frozen value, never from the
            # prefix scan: an empty map or unrelated rows must not pass.
            exact_key = entry["exact_key"]
            exact, exact_rpc_ok = session.call(
                "chainHead_v1_storage", [[{"key": exact_key, "type": "value"}], None]
            )
            requests.append(
                {
                    "method": "chainHead_v1_storage",
                    "params": exact["params"],
                    "response": exact["response"],
                }
            )
            classic, classic_ok = classic_call(
                rpc, budget, "state_getStorage", [exact_key, session.block_hash]
            )
            requests.append(
                {
                    "method": "state_getStorage",
                    "params": [exact_key, session.block_hash],
                    "response": classic,
                }
            )
            if not (exact_rpc_ok and classic_ok):
                exact_success = False
                exact_detail = call_failure_reason(exact, "exact-key read failed")
            else:
                value = classic if isinstance(classic, str) else None
                exact_success, exact_detail = check_expected_value(entry, value)
        success = rpc_success and metadata_ok and fallback_success and exact_success
        if success:
            detail = "recorded"
        elif layout_matches is False:
            detail = "layout mismatch"
        elif not exact_success:
            detail = exact_detail
        else:
            detail = presence_detail if not present else call_failure_reason(result, "storage query failed")
        return requests, success, detail

    if kind == "constant":
        header, header_success = session.call("chainHead_v1_header", [])
        requests.append(
            {
                "method": "chainHead_v1_header",
                "params": header["params"],
                "response": header["response"],
            }
        )
        requests.append(
            metadata_request(
                entry, present, presence_detail, rendered_layout, layout_matches
            )
        )
        success = header_success and metadata_ok
        reason = (
            "recorded"
            if success
            else "layout mismatch"
            if layout_matches is False
            else call_failure_reason(header, presence_detail)
        )
        return requests, success, reason

    if kind == "event":
        system_storage = metadata.get("pallets", {}).get("System", {}).get("storage")
        system_prefix = system_storage["prefix"] if system_storage else "System"
        events_key = storage_prefix(system_prefix, "Events")
        chainhead, chainhead_success = session.call(
            "chainHead_v1_storage", [[{"key": events_key, "type": "value"}], None]
        )
        requests.append(
            {
                "method": "chainHead_v1_storage",
                "params": chainhead["params"],
                "response": chainhead["response"],
            }
        )
        classic, classic_success = classic_call(
            rpc, budget, "state_getStorage", [events_key, session.block_hash]
        )
        requests.append(
            {
                "method": "state_getStorage",
                "params": [events_key, session.block_hash],
                "response": classic,
            }
        )
        requests.append(
            metadata_request(
                entry, present, presence_detail, rendered_layout, layout_matches
            )
        )
        # A live chain always has System.Events bytes at a finalized block;
        # both transports must deliver them or the fixture certifies nothing.
        chainhead_value = storage_value_from_chainhead(chainhead["response"])
        bytes_ok = nonempty_hex(chainhead_value) and nonempty_hex(classic)
        success = chainhead_success and classic_success and metadata_ok and bytes_ok
        if success:
            reason = "recorded"
        elif layout_matches is False:
            reason = "layout mismatch"
        elif not present:
            reason = presence_detail
        elif chainhead_success and classic_success and not bytes_ok:
            reason = "System.Events returned no bytes on at least one transport"
        else:
            reason = call_failure_reason(
                chainhead, "System.Events storage query failed"
            )
        return requests, success, reason

    return requests, False, f"unknown manifest kind {kind}"


def write_transcript(
    out_dir: Path,
    entry: dict[str, Any],
    block_hash: str | None,
    requests: list[dict[str, Any]],
) -> None:
    transcript = normalized_transcript(entry["id"], block_hash, requests)
    path = out_dir / f"{safe_filename(entry['id'])}.json"
    path.write_text(deterministic_json(transcript), encoding="utf-8")


def main() -> int:
    args = parse_args()
    manifest = json.loads(args.manifest.read_text(encoding="utf-8"))
    entries = manifest["entries"]
    args.out_dir.mkdir(parents=True, exist_ok=True)

    recorded: list[str] = []
    missing: list[dict[str, Any]] = []
    mode = "chainHead-v1"
    block_hash: str | None = None

    with NodeProcess(
        args.node, args.chain_spec, boot_timeout=args.boot_timeout
    ) as node:
        rpc = JsonRpcHttp(node.http_url)
        metadata_bytes = (
            args.metadata.read_bytes()
            if args.metadata.is_file()
            else bytes.fromhex(rpc.call("state_getMetadata")[2:])
        )
        try:
            metadata = decode_metadata(metadata_bytes)
            metadata_error = None
        except MetadataDecodeError as error:
            metadata = {"version": None, "types": {}, "pallets": {}}
            metadata_error = str(error)

        budget = DeadlineBudget(args.recording_timeout)
        session: ChainHeadSession | None = None
        websocket_error: str | None = None
        if not args.classic_only:
            try:
                session = ChainHeadSession(
                    node.ws_url,
                    operation_timeout=args.operation_timeout,
                    recording_timeout=args.recording_timeout,
                    budget=budget,
                )
                session.start()
                block_hash = session.block_hash
            except (ImportError, OSError, RuntimeError, ChainHeadError) as error:
                websocket_error = str(error)
                mode = "classic-rpc-only"
                if session is not None:
                    session.close()
                session = None
        else:
            websocket_error = "classic-only mode explicitly requested"
            mode = "classic-rpc-only"

        try:
            for entry in entries:
                if session is None:
                    requests: list[dict[str, Any]] = []
                    if entry.get("classic_rpc_fallback"):
                        raw_key = entry["raw_key"]
                        response, _ = classic_call(
                            rpc, budget, "state_getStorage", [raw_key]
                        )
                        requests.append(
                            {
                                "method": "state_getStorage",
                                "params": [raw_key],
                                "response": response,
                            }
                        )
                    if entry["kind"] == "properties":
                        response, _ = classic_call(rpc, budget, "system_properties", [])
                        requests.append(
                            {
                                "method": "system_properties",
                                "params": [],
                                "response": response,
                            }
                        )
                    if entry["kind"] not in ("raw_storage", "properties"):
                        present, detail, rendered_layout, layout_matches = (
                            metadata_status(metadata, entry)
                        )
                        requests.append(
                            metadata_request(
                                entry,
                                present,
                                detail,
                                rendered_layout,
                                layout_matches,
                            )
                        )
                    write_transcript(args.out_dir, entry, None, requests)
                    success = False
                    reason = "chainHead unavailable; classic-RPC-only degradation"
                else:
                    requests, success, reason = record_surface(
                        entry, session, rpc, metadata, budget
                    )
                    write_transcript(args.out_dir, entry, block_hash, requests)
                if success:
                    recorded.append(entry["id"])
                else:
                    missing.append(
                        {
                            "surface": entry["id"],
                            "kind": entry["kind"],
                            "required": entry["required"],
                            "reason": reason,
                            "blocked_by": entry.get("blocked_by"),
                            "rendered_layout": surface_layout(metadata, entry),
                            "expected_layout": entry.get("layout"),
                        }
                    )
        finally:
            if session is not None:
                session.close()

    blocked_by: dict[str, list[str]] = {}
    for item in missing:
        blocker = item.get("blocked_by") or "unassigned"
        blocked_by.setdefault(blocker, []).append(item["surface"])
    report = {
        "schema": "bleavit.chainhead-fixtures-report.v1",
        "mode": mode,
        "metadata_sha256": hashlib.sha256(metadata_bytes).hexdigest(),
        "metadata_version": metadata.get("version"),
        "metadata_error": metadata_error,
        "pinned_block": block_hash,
        "recorded": sorted(recorded),
        "missing": sorted(missing, key=lambda item: item["surface"]),
        "blocked_by": {key: sorted(value) for key, value in sorted(blocked_by.items())},
        "websocket_error": websocket_error,
        "strict_ready": not any(item["required"] for item in missing),
    }
    write_json(args.out_dir / "fixtures-report.json", report)
    if not args.allow_missing and not report["strict_ready"]:
        print(
            f"strict fixture recording failed: {sum(item['required'] for item in missing)} required surface items missing",
            file=sys.stderr,
        )
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

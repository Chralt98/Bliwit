from __future__ import annotations

import importlib.util
import tempfile
import unittest
from pathlib import Path


TOOLS = Path(__file__).resolve().parents[1]


def load_script(name: str):
    path = TOOLS / name
    spec = importlib.util.spec_from_file_location(name.replace("-", "_"), path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


RECORDER = load_script("record-chainhead-fixtures.py")
EXTRACT = load_script("extract-metadata.py")


def _event_fixture():
    class FakeSession:
        block_hash = "0x" + "ab" * 32

        def __init__(self, chainhead_value):
            self.chainhead_value = chainhead_value

        def call(self, method, params):
            events = []
            if self.chainhead_value is not None:
                events.append(
                    {
                        "event": "operationStorageItems",
                        "items": [
                            {"key": params[0][0]["key"], "value": self.chainhead_value}
                        ],
                    }
                )
            events.append({"event": "operationStorageDone"})
            return {
                "params": ["subscription", self.block_hash, *params],
                "response": {
                    "direct": {"result": {"result": "started"}},
                    "events": events,
                },
            }, True

    class FakeRpc:
        timeout = 15.0

        def __init__(self, classic_value):
            self.classic_value = classic_value

        def call(self, method, params, timeout=None):
            return self.classic_value

    metadata = {
        "types": {0: {"path": [], "definition": {"kind": "primitive", "primitive": "u32"}}},
        "apis": {},
        "pallets": {
            "Mini": {
                "events": {"Changed": {"fields": [{"name": "value", "type_id": 0}]}},
            },
            "System": {"storage": {"prefix": "System", "entries": {}}},
        },
    }
    entry = {
        "id": "event.mini.changed",
        "kind": "event",
        "pallet": "Mini",
        "event": "Changed",
        "layout": {"fields": [{"name": "value", "type": "u32"}]},
    }
    return FakeSession, FakeRpc, metadata, entry


class DeadlineTests(unittest.TestCase):
    def test_recovery_metadata_coverage_checks_every_metadata_surface(self) -> None:
        _, _, metadata, event = _event_fixture()
        event["required"] = True
        raw = {
            "id": "storage.release_channel",
            "kind": "raw_storage",
            "required": True,
        }
        recorded, missing = RECORDER.metadata_surface_coverage(
            metadata, [event, raw]
        )
        self.assertEqual(recorded, ["event.mini.changed"])
        self.assertEqual(missing, [])

        drifted = {
            **event,
            "layout": {"fields": [{"name": "wrong", "type": "u32"}]},
        }
        recorded, missing = RECORDER.metadata_surface_coverage(
            metadata, [drifted, raw]
        )
        self.assertEqual(recorded, [])
        self.assertEqual(
            [(item["surface"], item["reason"]) for item in missing],
            [("event.mini.changed", "layout mismatch")],
        )

    def test_unrelated_notifications_cannot_extend_operation_deadline(self) -> None:
        now = [0.0]

        def clock() -> float:
            return now[0]

        class FakeConnection:
            def recv(self, timeout: float) -> str:
                self.assert_timeout = timeout
                now[0] += 0.4
                return '{"jsonrpc":"2.0","method":"chainHead_v1_followEvent","params":{}}'

        session = RECORDER.ChainHeadSession.__new__(RECORDER.ChainHeadSession)
        session.connection = FakeConnection()
        session.budget = RECORDER.DeadlineBudget(10.0, clock=clock)
        session.notifications = []
        with self.assertRaises(RECORDER.ChainHeadTimeout):
            session._response(7, session.budget.operation(1.0))

    def test_event_recording_captures_system_events_bytes_on_both_transports(self) -> None:
        FakeSession, FakeRpc, metadata, entry = _event_fixture()
        budget = RECORDER.DeadlineBudget(60.0)
        requests, success, reason = RECORDER.record_surface(
            entry, FakeSession("0x010203"), FakeRpc("0x010203"), metadata, budget
        )
        self.assertTrue(success, reason)
        self.assertEqual(
            [request["method"] for request in requests],
            ["chainHead_v1_storage", "state_getStorage", "metadata_presence"],
        )
        self.assertEqual(requests[1]["response"], "0x010203")

    def test_event_recording_fails_when_either_transport_returns_no_bytes(self) -> None:
        FakeSession, FakeRpc, metadata, entry = _event_fixture()
        budget = RECORDER.DeadlineBudget(60.0)
        for chainhead_value, classic_value in ((None, "0x0102"), ("0x0102", None), ("0x", "0x")):
            _, success, reason = RECORDER.record_surface(
                entry,
                FakeSession(chainhead_value),
                FakeRpc(classic_value),
                metadata,
                budget,
            )
            self.assertFalse(success)
            self.assertIn("no bytes", reason)


class WasmBindingTests(unittest.TestCase):
    def test_matching_wasm_hashes_are_returned(self) -> None:
        first, second = EXTRACT.bound_wasm_hashes(b"wasm", "0x7761736d")
        self.assertEqual(first, second)

    def test_empty_or_mismatched_wasm_fails(self) -> None:
        with self.assertRaises(RuntimeError):
            EXTRACT.bound_wasm_hashes(b"", "0x7761736d")
        with self.assertRaises(RuntimeError):
            EXTRACT.bound_wasm_hashes(b"wasm-a", "0x7761736d2d62")


class NodeRetryTests(unittest.TestCase):
    def test_exit_before_ready_re_reserves_port_and_relaunches(self) -> None:
        ports = iter((31001, 31002))
        launches = []

        class FakeProcess:
            def __init__(self, exited: bool):
                self.returncode = 98 if exited else None

            def poll(self):
                return self.returncode

            def terminate(self):
                self.returncode = 0

            def wait(self, timeout=None):
                return self.returncode

            def kill(self):
                self.returncode = -9

        def launcher(command, stdout, stderr):
            launches.append(command)
            return FakeProcess(exited=len(launches) == 1)

        class FakeRpc:
            def __init__(self, url, timeout):
                self.url = url

            def call(self, method):
                return {"specVersion": 1}

        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            binary = root / "node"
            chain_spec = root / "spec.json"
            binary.write_bytes(b"binary")
            chain_spec.write_text("{}", encoding="utf-8")
            with RECORDER.NodeProcess(
                binary,
                chain_spec,
                port_reserver=lambda: next(ports),
                launcher=launcher,
                rpc_factory=FakeRpc,
                sleeper=lambda _: None,
            ) as node:
                self.assertEqual(node.port, 31002)
                self.assertEqual(len(launches), 2)
                self.assertIn("31001", launches[0])
                self.assertIn("31002", launches[1])


if __name__ == "__main__":
    unittest.main()

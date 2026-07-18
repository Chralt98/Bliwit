from __future__ import annotations

import unittest
from unittest import mock

import support  # noqa: F401 - inserts tools/monitoring on sys.path.

import chain_alerts_exporter as exporter_module
from common import MetricStore


class CatchUpRpc:
    def __init__(self) -> None:
        self.hash_calls: list[int] = []

    def call(self, method: str, params: list[object] | None = None) -> object:
        if method == "state_getRuntimeVersion":
            return {"specVersion": 1}
        if method == "chain_getBlockHash":
            assert params is not None and len(params) == 1
            block = params[0]
            assert isinstance(block, int)
            self.hash_calls.append(block)
            return block_hash(block)
        raise AssertionError(f"unexpected RPC call {method} {params}")


def block_hash(block: int) -> str:
    return f"0x{block:064x}"


def metric_value(store: MetricStore, name: str) -> float:
    values = [value for (series, _labels), value in store.values.items() if series == name]
    if len(values) != 1:
        raise AssertionError(f"expected one {name} sample, found {values}")
    return values[0]


class FinalizedCatchUpTests(unittest.TestCase):
    def new_exporter(
        self,
    ) -> tuple[exporter_module.ChainExporter, CatchUpRpc, list[tuple[int, str]]]:
        rpc = CatchUpRpc()
        exporter = exporter_module.ChainExporter(
            rpc, MetricStore(exporter_module.SERIES)  # type: ignore[arg-type]
        )
        exporter._load_metadata = lambda _block_hash, force=False: {}  # type: ignore[method-assign]
        exporter._release_channel = lambda *_args: None  # type: ignore[method-assign]
        observed: list[tuple[int, str]] = []

        def record_events(current_hash: str, current_block: int) -> None:
            if (
                exporter.last_event_block is not None
                and current_block <= exporter.last_event_block
            ):
                return
            observed.append((current_block, current_hash))
            exporter.last_event_block = current_block

        exporter._events = record_events  # type: ignore[method-assign]
        return exporter, rpc, observed

    def test_gap_catch_up_processes_every_intermediate_block_once(self) -> None:
        exporter, rpc, observed = self.new_exporter()
        exporter.last_event_block = 10

        self.assertTrue(exporter.process_finalized(block_hash(15), 15, full=False))
        self.assertTrue(exporter.process_finalized(block_hash(15), 15, full=False))

        self.assertEqual(
            observed,
            [(block, block_hash(block)) for block in range(11, 16)],
        )
        self.assertEqual(rpc.hash_calls, [11, 12, 13, 14])

    def test_over_cap_gap_counts_and_logs_skipped_blocks(self) -> None:
        exporter, _rpc, observed = self.new_exporter()
        exporter.last_event_block = 10

        with mock.patch.object(exporter_module, "MAX_EVENT_CATCH_UP_BLOCKS", 3):
            with self.assertLogs("bleavit-chain-alerts", level="ERROR") as logs:
                self.assertTrue(
                    exporter.process_finalized(block_hash(15), 15, full=False)
                )

        self.assertEqual(
            observed,
            [(block, block_hash(block)) for block in range(13, 16)],
        )
        self.assertEqual(
            metric_value(exporter.store, "bleavit_chain_scrape_errors_total"), 2
        )
        self.assertIn("skipping 2 oldest blocks", "\n".join(logs.output))

    def test_repeated_same_head_notification_is_event_idempotent(self) -> None:
        exporter, rpc, observed = self.new_exporter()

        self.assertTrue(exporter.process_finalized(block_hash(20), 20, full=False))
        self.assertTrue(exporter.process_finalized(block_hash(20), 20, full=False))

        self.assertEqual(observed, [(20, block_hash(20))])
        self.assertEqual(rpc.hash_calls, [])


if __name__ == "__main__":
    unittest.main()

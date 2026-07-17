from __future__ import annotations

import sys
import unittest
from pathlib import Path


sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from transcript import normalized_transcript


class TranscriptNormalizerTests(unittest.TestCase):
    def test_server_ids_and_timestamps_do_not_affect_fixture(self) -> None:
        def sample(subscription: str, operation: str, timestamp: str):
            return [
                {
                    "method": "chainHead_v1_storage",
                    "params": [subscription, "0x" + "ab" * 32],
                    "response": {
                        "direct": {
                            "result": {"result": "started", "operationId": operation}
                        },
                        "events": [
                            {
                                "event": "operationStorageDone",
                                "operationId": operation,
                                "subscription": subscription,
                                "timestamp": timestamp,
                            }
                        ],
                    },
                }
            ]

        first = normalized_transcript(
            "storage.constitution.phase_flags",
            "0x" + "ab" * 32,
            sample("follow-91", "random-op-a", "2026-01-01T00:00:00Z"),
        )
        second = normalized_transcript(
            "storage.constitution.phase_flags",
            "0x" + "ab" * 32,
            sample("follow-7", "random-op-z", "2030-01-01T00:00:00Z"),
        )
        self.assertEqual(first, second)
        serialized = repr(first)
        self.assertIn("subscription-1", serialized)
        self.assertIn("operation-1", serialized)
        self.assertNotIn("timestamp", serialized)


if __name__ == "__main__":
    unittest.main()


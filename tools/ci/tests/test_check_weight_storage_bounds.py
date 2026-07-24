import importlib.util
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[3]
SCRIPT = ROOT / "tools" / "ci" / "check-weight-storage-bounds.py"
SPEC = importlib.util.spec_from_file_location("check_weight_storage_bounds", SCRIPT)
MODULE = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(MODULE)


class WeightStorageBoundsTests(unittest.TestCase):
    def test_normative_recent_summary_bound_is_present(self):
        self.assertEqual(MODULE.recent_summary_bound(), 5057)

    def test_generated_weights_cover_recent_summary_bound(self):
        rows = MODULE.storage_annotations()["Epoch::RecentCohortSummaries"]
        self.assertTrue(rows)
        self.assertTrue(all(size >= 5057 for _, size in rows))


if __name__ == "__main__":
    unittest.main()

from __future__ import annotations

import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[3]


class GenerateChainSpecsProfileTests(unittest.TestCase):
    def test_generator_uses_canonical_explicit_runtime_profile(self) -> None:
        script = (ROOT / "tools/deploy/generate-chain-specs.sh").read_text(
            encoding="utf-8"
        )
        self.assertIn("tools/release/runtime_profiles.py", script)
        self.assertIn("RUNTIME_PROFILE", script)
        self.assertIn("--field features", script)
        self.assertIn("--no-default-features", script)
        self.assertIn('--features "$runtime_features"', script)
        self.assertNotIn(
            "--release --features substrate-wasm-builder --locked", script
        )


if __name__ == "__main__":
    unittest.main()

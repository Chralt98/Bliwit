from __future__ import annotations

import hashlib
import importlib.util
import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from decimal import Decimal, localcontext
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[3]
GENERATOR_PATH = REPO_ROOT / "tools/reference-model/generate-vectors.py"


def load_generator():
    spec = importlib.util.spec_from_file_location(
        "bleavit_generate_vectors", GENERATOR_PATH
    )
    if spec is None or spec.loader is None:
        raise RuntimeError(f"cannot import {GENERATOR_PATH}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


GENERATOR = load_generator()


class SweepGeneratorTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.temporary = tempfile.TemporaryDirectory()
        cls.root = Path(cls.temporary.name)
        cls.one_worker = cls.root / "one-worker"
        cls.two_workers = cls.root / "two-workers"
        for output, workers in (
            (cls.one_worker, 1),
            (cls.two_workers, 2),
        ):
            command = [
                sys.executable,
                str(GENERATOR_PATH),
                "--sweep-out",
                str(output),
                "--sweep-points",
                "400",
                "--sweep-shards",
                "2",
                "--sweep-workers",
                str(workers),
            ]
            completed = subprocess.run(
                command,
                cwd=REPO_ROOT,
                text=True,
                capture_output=True,
                check=False,
            )
            if completed.returncode != 0:
                raise RuntimeError(
                    f"{' '.join(command)} failed\n"
                    f"stdout:\n{completed.stdout}\n"
                    f"stderr:\n{completed.stderr}"
                )
        cls.manifest = json.loads(
            (cls.one_worker / "sweep-manifest.json").read_text()
        )

    @classmethod
    def tearDownClass(cls):
        cls.temporary.cleanup()

    def test_shards_are_deterministic_and_worker_count_independent(self):
        files = ["sweep-manifest.json"] + [
            shard["file"] for shard in self.manifest["shards"]
        ]
        for relative in files:
            self.assertEqual(
                (self.one_worker / relative).read_bytes(),
                (self.two_workers / relative).read_bytes(),
                relative,
            )

    def test_manifest_hashes_exact_shard_bytes(self):
        for shard in self.manifest["shards"]:
            contents = (self.one_worker / shard["file"]).read_bytes()
            self.assertEqual(hashlib.sha256(contents).hexdigest(), shard["sha256"])

    def test_rows_are_standalone_replayable(self):
        references = {
            "exp2": GENERATOR.ref_exp2,
            "log2": GENERATOR.ref_log2,
            "ln": GENERATOR.ref_ln,
        }
        families = set()
        for shard in self.manifest["shards"]:
            document = json.loads(
                (self.one_worker / shard["file"]).read_text()
            )
            self.assertEqual(document["schema"], GENERATOR.SWEEP_SCHEMA)
            for row in document["rows"]:
                families.add(row["f"])
                self.assertIsInstance(row["out"], int)
                with localcontext() as context:
                    context.prec = GENERATOR.WORK_PREC
                    if row["f"] == "cost":
                        self.assertEqual(
                            list(row), ["f", "q_l", "q_s", "b", "out"]
                        )
                        for key in ("q_l", "q_s", "b"):
                            self.assertIsInstance(row[key], int)
                        q64 = Decimal(1 << 64)
                        expected = GENERATOR.raw_64x64_nearest(
                            GENERATOR.cost(
                                Decimal(row["b"]) / q64,
                                Decimal(row["q_l"]) / q64,
                                Decimal(row["q_s"]) / q64,
                            )
                        )
                    else:
                        self.assertEqual(list(row), ["f", "in", "out"])
                        self.assertIn(row["f"], references)
                        self.assertIsInstance(row["in"], int)
                        value = Decimal(row["in"]) / Decimal(1 << 64)
                        expected = GENERATOR.raw_64x64_nearest(
                            references[row["f"]](value)
                        )
                self.assertEqual(row["out"], expected)
        self.assertEqual(families, {"exp2", "log2", "ln", "cost"})

    def test_point_and_shard_accounting(self):
        self.assertEqual(self.manifest["points"], 400)
        self.assertEqual(len(self.manifest["shards"]), 2)
        self.assertEqual(
            sum(shard["rows"] for shard in self.manifest["shards"]),
            self.manifest["points"],
        )
        for index, shard in enumerate(self.manifest["shards"]):
            self.assertEqual(shard["file"], f"shards/sweep-{index:03d}.json")
            document = json.loads(
                (self.one_worker / shard["file"]).read_text()
            )
            self.assertEqual(document["shard"], index)
            self.assertEqual(len(document["rows"]), shard["rows"])
        self.assertEqual(
            self.manifest["distribution"]["random_rows"],
            {
                "exp2_frac": "55%",
                "exp2_wide": "15%",
                "log2": "10%",
                "ln": "10%",
                "cost": "10% (including integer remainder)",
            },
        )

    def test_stale_shard_is_rejected(self):
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary)
            (output / "shards").mkdir()
            (output / "shards/sweep-999.json").write_text("stale\n")
            command = [
                sys.executable,
                str(GENERATOR_PATH),
                "--sweep-out",
                str(output),
                "--sweep-points",
                "40",
                "--sweep-shards",
                "2",
                "--sweep-workers",
                "1",
            ]
            completed = subprocess.run(
                command,
                cwd=REPO_ROOT,
                text=True,
                capture_output=True,
                check=False,
            )
            self.assertNotEqual(completed.returncode, 0)
            self.assertIn("stale shard files", completed.stderr)

    def test_tiny_sweep_passes_rust_checker(self):
        self._run_rust_checker(str(self.one_worker))

    def test_tiny_sweep_passes_rust_checker_with_workspace_relative_dir(self):
        # Regression: the CI workflows pass a workspace-relative
        # BLEAVIT_SWEEP_DIR, but cargo runs test binaries from the package
        # root — the checker must resolve relative paths against the
        # workspace root, or every sweep.yml/release.yml run fails.
        relative = Path("target") / f"tiny-sweep-relative-{os.getpid()}"
        destination = REPO_ROOT / relative
        shutil.copytree(self.one_worker, destination)
        try:
            self._run_rust_checker(str(relative))
        finally:
            shutil.rmtree(destination, ignore_errors=True)

    def _run_rust_checker(self, sweep_dir: str) -> None:
        cargo = shutil.which("cargo")
        if cargo is None:
            self.skipTest("cargo is not available")
        environment = os.environ.copy()
        environment["BLEAVIT_SWEEP_DIR"] = sweep_dir
        environment.pop("BLEAVIT_SWEEP_REQUIRE_FULL", None)
        command = [
            cargo,
            "test",
            "-p",
            "futarchy-fixed",
            "--locked",
            "--test",
            "sweep",
            "--",
            "--ignored",
        ]
        completed = subprocess.run(
            command,
            cwd=REPO_ROOT,
            env=environment,
            text=True,
            capture_output=True,
            check=False,
            timeout=300,
        )
        self.assertEqual(
            completed.returncode,
            0,
            f"{' '.join(command)} failed\n"
            f"stdout:\n{completed.stdout}\n"
            f"stderr:\n{completed.stderr}",
        )


if __name__ == "__main__":
    unittest.main()

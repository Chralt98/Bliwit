from __future__ import annotations

import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[3]


class WorkflowContractTests(unittest.TestCase):
    def test_release_publication_is_draft_verified_and_prerelease(self) -> None:
        workflow = (ROOT / ".github/workflows/release.yml").read_text(encoding="utf-8")
        create = workflow.index('gh release create "$GITHUB_REF_NAME"')
        upload = workflow.index('gh release upload "$GITHUB_REF_NAME"')
        verify = workflow.index('gh release view "$GITHUB_REF_NAME" --json assets')
        publish = workflow.index('gh release edit "$GITHUB_REF_NAME" --draft=false')
        self.assertLess(create, upload)
        self.assertLess(upload, verify)
        self.assertLess(verify, publish)
        self.assertIn("--draft", workflow[create:upload])
        self.assertIn("--prerelease", workflow[create:upload])
        self.assertNotIn("--clobber", workflow)
        self.assertIn("remote != local", workflow)
        self.assertIn("not canonical", workflow)

    def test_publish_job_has_repository_context_and_bundle_handoff(self) -> None:
        workflow = (ROOT / ".github/workflows/release.yml").read_text(encoding="utf-8")
        # gh runs in an empty workspace: without GH_REPO every call fails.
        self.assertIn("GH_REPO: ${{ github.repository }}", workflow)
        # The existence probe must distinguish "not found" from API failure.
        self.assertIn("release not found", workflow)
        self.assertIn("cannot determine release state", workflow)
        # The publish job consumes the exact artifact the artifacts job built.
        self.assertEqual(workflow.count("bleavit-release-${{ github.run_id }}"), 2)
        # The assembler binds every artifact to the release commit.
        self.assertIn('--commit "$GITHUB_SHA"', workflow)

    def test_cargo_heavy_jobs_free_runner_disk_space(self) -> None:
        # The workspace build writes ~35 GB; stock runners have ~14 GB free.
        # Every job that runs the full workspace build must first drop the
        # preinstalled runner bloat, or it dies with "No space left on device".
        ci = (ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        release = (ROOT / ".github/workflows/release.yml").read_text(encoding="utf-8")
        self.assertEqual(ci.count("Free runner disk space"), 1)
        self.assertEqual(release.count("Free runner disk space"), 2)
        for workflow in (ci, release):
            self.assertIn("/usr/share/dotnet", workflow)
            self.assertIn("CARGO_INCREMENTAL: 0", workflow)

    def test_tag_gates_run_all_tooling_suites(self) -> None:
        workflow = (ROOT / ".github/workflows/release.yml").read_text(encoding="utf-8")
        gates = workflow[workflow.index("  gates:"):workflow.index("  artifacts:")]
        self.assertIn("actions/setup-node@v4", gates)
        self.assertIn("node-version: '22'", gates)
        for suite in (
            "tools/deploy/tests",
            "tools/reference-model/tests",
            "tools/release/tests",
            "tools/env/tests",
        ):
            self.assertIn(suite, workflow)
        install_step = workflow.index(
            "python3 -m pip install pyyaml==6.0.2 websockets==15.0.1"
        )
        compile_step = workflow.index(
            "python3 -m py_compile tools/env/*.py tools/env/tests/*.py"
        )
        tooling_step = workflow.index("python3 -m unittest discover -s \"$suite\"")
        validate_step = workflow.index("python3 tools/env/validate-environments.py")
        self.assertLess(install_step, compile_step)
        self.assertLess(compile_step, tooling_step)
        self.assertLess(tooling_step, validate_step)

    def test_release_runs_environment_evidence_before_strict_assembly(self) -> None:
        workflow = (ROOT / ".github/workflows/release.yml").read_text(encoding="utf-8")
        build_node = workflow.index("Build the release node")
        fetch = workflow.index("tools/env/fetch-binaries.sh")
        generate = workflow.index("tools/env/generate-relay-specs.sh")
        prewarm = workflow.index(
            'npx --yes "@acala-network/chopsticks@${CHOPSTICKS_VERSION}" --help >/dev/null'
        )
        produce = workflow.index("python3 tools/env/run-evidence.py")
        assemble = workflow.index("python3 tools/release/assemble-release.py")
        self.assertLess(build_node, fetch)
        self.assertLess(fetch, generate)
        self.assertLess(generate, prewarm)
        self.assertLess(prewarm, produce)
        self.assertLess(produce, assemble)
        self.assertIn("actions/setup-node@v4", workflow)
        self.assertIn("node-version: '22'", workflow)
        self.assertIn("pyyaml==6.0.2 websockets==15.0.1", workflow)
        producer = workflow[produce:assemble]
        for argument in (
            '--wasm "$RELEASE_WORK/runtime/runtime.wasm"',
            '--commit "$GITHUB_SHA"',
            "--tier release",
            '--log-dir "$RELEASE_WORK/env-evidence"',
            '--report-out "$RELEASE_WORK/env-evidence/run-report.json"',
        ):
            self.assertIn(argument, producer)
        self.assertIn(
            "environment run evidence not produced; strict assembly attributes the B7 gap",
            producer,
        )
        self.assertIn("if: always()", workflow)
        self.assertIn("if-no-files-found: ignore", workflow)
        self.assertIn("path: release-work/env-evidence/**", workflow)

    def test_environment_ci_compiles_and_tests_the_evidence_driver(self) -> None:
        workflow = (ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        self.assertIn("pyyaml==6.0.2 websockets==15.0.1", workflow)
        self.assertIn(
            "python3 -m py_compile tools/env/*.py tools/env/tests/*.py",
            workflow,
        )
        compile_step = workflow.index("python3 -m py_compile tools/env/*.py")
        test_step = workflow.index("python3 -m unittest discover -s tools/env/tests")
        validate_step = workflow.index("python3 tools/env/validate-environments.py")
        self.assertLess(compile_step, test_step)
        self.assertLess(test_step, validate_step)

    def test_kernel_sweep_workflow_has_normative_change_paths(self) -> None:
        workflow = (ROOT / ".github/workflows/sweep.yml").read_text(encoding="utf-8")
        for change_path in (
            "crates/futarchy-fixed/**",
            "crates/futarchy-primitives/**",
            "reference-model/src/**",
            "tools/reference-model/generate-vectors.py",
            ".github/workflows/sweep.yml",
        ):
            self.assertIn(change_path, workflow)
        self.assertIn("BLEAVIT_SWEEP_REQUIRE_FULL", workflow)
        self.assertNotIn("--sweep-points", workflow)


if __name__ == "__main__":
    unittest.main()

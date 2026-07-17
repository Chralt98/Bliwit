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

    def test_tag_gates_run_all_tooling_suites(self) -> None:
        workflow = (ROOT / ".github/workflows/release.yml").read_text(encoding="utf-8")
        for suite in (
            "tools/deploy/tests",
            "tools/reference-model/tests",
            "tools/release/tests",
        ):
            self.assertIn(suite, workflow)

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

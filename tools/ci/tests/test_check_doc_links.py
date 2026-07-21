"""Gates for the Markdown link checker.

The checker walks the filesystem, not the git index. That is deliberate — a doc
you have written but not yet committed must still have its links verified — but
it means the walk will happily descend into anything sitting in the tree,
including the nested git worktrees this project keeps under `.claude/worktrees/`.

Each such worktree carries a full `docs/` copy, and those copies also defeat the
verbatim special case in `link_base`, which is anchored at *this* checkout's
`VERBATIM_DIR`. The result was ~300 reported failures that CI could never
reproduce, because CI checks out a clean tree with no worktrees in it.

These tests pin the resulting rule from both sides: git-excluded files are
skipped, and everything else — including untracked files — is still checked.
The script is driven as a subprocess because it does its work at module scope;
importing it would run it against the real repository.
"""

from __future__ import annotations

import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).resolve().parents[1] / "check-doc-links.py"


class DocLinkCheckerTests(unittest.TestCase):
    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.root = Path(self._tmp.name)
        (self.root / "tools" / "ci").mkdir(parents=True)
        shutil.copy(SCRIPT, self.root / "tools" / "ci" / "check-doc-links.py")
        (self.root / "docs").mkdir()
        subprocess.run(["git", "init", "-q"], cwd=self.root, check=True)

    def tearDown(self) -> None:
        self._tmp.cleanup()

    def run_checker(self) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            ["python3", "tools/ci/check-doc-links.py"],
            cwd=self.root,
            capture_output=True,
            text=True,
            check=False,
        )

    def write(self, rel: str, text: str) -> None:
        path = self.root / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")

    def test_a_broken_link_still_fails(self) -> None:
        """The checker must keep its teeth — everything else here is about scope."""
        self.write("docs/a.md", "see [b](./b.md)\n")
        result = self.run_checker()
        self.assertEqual(result.returncode, 1)
        self.assertIn("missing link target: ./b.md", result.stderr)

    def test_a_resolving_link_passes(self) -> None:
        self.write("docs/a.md", "see [b](./b.md)\n")
        self.write("docs/b.md", "hi\n")
        self.assertEqual(self.run_checker().returncode, 0)

    def test_broken_links_inside_a_git_excluded_path_are_skipped(self) -> None:
        """The defect: a nested worktree's stale copies failed a clean tree.

        Without the exclusion check this run reports a broken link that exists
        only in a directory git was told to ignore, and that CI never sees.
        """
        self.write(".gitignore", ".claude/worktrees/\n")
        self.write(".claude/worktrees/wt/docs/a.md", "see [gone](./gone.md)\n")
        result = self.run_checker()
        self.assertEqual(
            result.returncode,
            0,
            f"git-excluded file was still checked; stderr={result.stderr}",
        )
        self.assertIn("git-excluded file(s) skipped", result.stdout)

    def test_untracked_but_not_ignored_files_are_still_checked(self) -> None:
        """The exclusion must not become 'only check committed files'.

        A doc written and not yet committed is exactly when a broken link is
        most likely, so narrowing the walk to the index would gut the gate.
        """
        self.write("docs/brand-new.md", "see [nope](./nope.md)\n")
        result = self.run_checker()
        self.assertEqual(result.returncode, 1)
        self.assertIn("missing link target: ./nope.md", result.stderr)

    def test_skipped_count_is_reported(self) -> None:
        """No silent caps: a shrinking denominator must stay visible."""
        self.write(".gitignore", "vendor/\n")
        self.write("vendor/x.md", "[a](./a.md)\n")
        self.write("docs/ok.md", "no links\n")
        result = self.run_checker()
        self.assertEqual(result.returncode, 0)
        self.assertIn("1 git-excluded file(s) skipped", result.stdout)

    def test_without_git_every_file_is_checked(self) -> None:
        """Fallback must over-report, never under-report.

        `check-ignore` fails outside a repository. The checker treats that as
        'exclude nothing' so a source tarball still gets a real check.
        """
        shutil.rmtree(self.root / ".git")
        self.write("docs/a.md", "see [gone](./gone.md)\n")
        result = self.run_checker()
        self.assertEqual(result.returncode, 1)
        self.assertIn("missing link target: ./gone.md", result.stderr)


if __name__ == "__main__":
    unittest.main()

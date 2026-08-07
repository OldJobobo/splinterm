from __future__ import annotations

from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
WORKFLOW = ROOT / ".github/workflows/edge-release.yml"
INSTALLER = ROOT / "install.sh"


class EdgeWorkflowTests(unittest.TestCase):
    def test_git_bootstrap_precedes_checkout_and_channel_advance(self) -> None:
        workflow = WORKFLOW.read_text(encoding="utf-8")
        bootstrap = workflow.index("name: Bootstrap Git for checkout")
        checkout = workflow.index("uses: actions/checkout@v4")
        publish = workflow.index("name: Publish the immutable commit release")
        advance = workflow.index("name: Advance the edge channel Git ref")
        self.assertLess(bootstrap, checkout)
        self.assertLess(checkout, publish)
        self.assertLess(publish, advance)
        self.assertIn("git commit-tree", workflow[advance:])
        self.assertIn("refs/heads/edge-channel", workflow[advance:])
        self.assertNotIn("cancel-in-progress: true", workflow)
        self.assertIn("build-local-package.sh --no-check --skip-system-dependency-check", workflow)

    def test_channel_branch_does_not_trigger_source_ci(self) -> None:
        ci_workflow = (ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        self.assertIn("branches-ignore: [edge-channel]", ci_workflow)

    def test_channel_manifest_selects_an_immutable_release(self) -> None:
        workflow = WORKFLOW.read_text(encoding="utf-8")
        installer = INSTALLER.read_text(encoding="utf-8")
        self.assertIn("RELEASE_TAG=edge-%s", workflow)
        self.assertIn('"release": f"edge-{commit}"', (ROOT / "tools/package/edge-manifest.py").read_text(encoding="utf-8"))
        self.assertIn("contents/edge-manifest.json?ref=$channel_branch", installer)
        self.assertIn('download_asset "$release" "$main_asset"', installer)
        self.assertNotIn("gh release upload edge", workflow)

    def test_installer_keeps_mcp_opt_in_and_failure_restart_guard(self) -> None:
        installer = INSTALLER.read_text(encoding="utf-8")
        mcp_probe = installer.index("pacman -Q splinterm-mcp")
        mcp_download = installer.index('download_asset "$release" "$mcp_asset"')
        self.assertLess(mcp_probe, mcp_download)
        self.assertIn("trap cleanup_prebuilt_install EXIT", installer)
        self.assertIn("systemctl --user start splinterd.service || true", installer)
        self.assertIn("Emergency snapshot:", installer)


if __name__ == "__main__":
    unittest.main()

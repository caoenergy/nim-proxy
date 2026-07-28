#!/usr/bin/env python3
"""Guard the release workflow's Cosign CLI and artifact contract."""

from pathlib import Path
import unittest


WORKFLOW = Path(__file__).parents[1] / ".github" / "workflows" / "release.yml"


class ReleaseContractTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.workflow = WORKFLOW.read_text()

    def test_cosign_cli_is_explicitly_pinned(self) -> None:
        self.assertEqual(self.workflow.count("cosign-release: v3.1.2"), 2)

    def test_blob_signing_uses_sigstore_bundles(self) -> None:
        self.assertIn('--bundle "$f.sigstore.json"', self.workflow)
        self.assertNotIn("--output-signature", self.workflow)
        self.assertNotIn("--output-certificate", self.workflow)

    def test_release_publishes_and_verifies_bundles(self) -> None:
        self.assertIn("nim-proxy-*.tar.gz.sigstore.json", self.workflow)
        self.assertIn("nim-proxy-sbom.spdx.json.sigstore.json", self.workflow)
        self.assertIn(
            "--bundle "
            "nim-proxy-${{ needs.prepare.outputs.version }}"
            "-linux-amd64.tar.gz.sigstore.json",
            self.workflow,
        )


if __name__ == "__main__":
    unittest.main()

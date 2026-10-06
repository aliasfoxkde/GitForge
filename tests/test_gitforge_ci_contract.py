#!/usr/bin/env python3
"""Guard the GitHub-to-GitForge bridge against endpoint/auth drift."""

from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parents[1]


class GitForgeCiContractTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.workflow = (ROOT / ".github/workflows/gitforge-ci.yml").read_text()
        cls.setup = (ROOT / ".github/GITFORGE_CI_SETUP.md").read_text()

    def test_trigger_uses_ci_trigger_secret_and_pinned_endpoint(self) -> None:
        self.assertIn('"$GITFORGE_SCHEDULER_URL/pipelines/trigger"', self.workflow)
        self.assertIn('Authorization: Bearer $GITFORGE_CI_TRIGGER_TOKEN', self.workflow)
        self.assertIn("GITFORGE_CI_TRIGGER_TOKEN: ${{ secrets.GITFORGE_CI_TRIGGER_TOKEN }}", self.workflow)

    def test_poll_uses_scheduler_run_route_and_operator_secret(self) -> None:
        self.assertIn('STATUS_URL="$GITFORGE_SCHEDULER_URL/pipelines/runs/$RUN_ID"', self.workflow)
        self.assertIn('Authorization: Bearer $GITFORGE_SCHEDULER_OPERATOR_TOKEN', self.workflow)
        self.assertIn(
            "GITFORGE_SCHEDULER_OPERATOR_TOKEN: ${{ secrets.GITFORGE_SCHEDULER_OPERATOR_TOKEN }}",
            self.workflow,
        )

    def test_bridge_remains_opt_in_and_setup_docs_match_credentials(self) -> None:
        self.assertIn("if: vars.GITFORGE_ENABLED == 'true'", self.workflow)
        self.assertIn("`GITFORGE_CI_TRIGGER_TOKEN` | secret", self.setup)
        self.assertIn("`GITFORGE_SCHEDULER_OPERATOR_TOKEN` | secret", self.setup)
        self.assertNotIn("GITFORGE_API_URL", self.workflow)
        self.assertNotIn("GITFORGE_API_TOKEN", self.workflow)


if __name__ == "__main__":
    unittest.main()

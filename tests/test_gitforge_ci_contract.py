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

    def test_pull_request_uses_mirrored_head_branch_and_commit_range(self) -> None:
        self.assertIn('if [[ "$EVENT_NAME" == "pull_request" ]]; then', self.workflow)
        self.assertIn('REF_NAME="$PR_HEAD_REF"', self.workflow)
        self.assertIn('OLD_HASH="$PR_BASE_SHA"', self.workflow)
        self.assertIn('NEW_HASH="$PR_HEAD_SHA"', self.workflow)
        self.assertIn('REF_NAME="$PUSH_REF"', self.workflow)
        self.assertNotIn('--arg ref "${{ github.ref_name }}"', self.workflow)
        self.assertNotIn('--arg old "${{ github.event.before }}"', self.workflow)

    def test_fork_pull_requests_skip_mirror_only_bridge(self) -> None:
        fork_gate = "github.event.pull_request.head.repo.full_name == github.repository"
        self.assertGreaterEqual(self.workflow.count(fork_gate), 2)
        self.assertIn("github.event_name != 'pull_request'", self.workflow)
        self.assertIn("same-repository pull requests", self.setup)

    def test_bridge_remains_opt_in_and_setup_docs_match_credentials(self) -> None:
        self.assertGreaterEqual(self.workflow.count("vars.GITFORGE_ENABLED == 'true'"), 2)
        self.assertIn("`GITFORGE_CI_TRIGGER_TOKEN` | secret", self.setup)
        self.assertIn("`GITFORGE_SCHEDULER_OPERATOR_TOKEN` | secret", self.setup)
        self.assertNotIn("GITFORGE_API_URL", self.workflow)
        self.assertNotIn("GITFORGE_API_TOKEN", self.workflow)


if __name__ == "__main__":
    unittest.main()

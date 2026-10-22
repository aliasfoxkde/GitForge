import subprocess
import importlib.util
import json
import re
import tempfile
import threading
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

spec = importlib.util.spec_from_file_location(
    "gitforge_ci", Path(__file__).resolve().parents[1] / "gitforge_ci.py"
)
gitforge_ci = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gitforge_ci)
candidate_ref = gitforge_ci.candidate_ref
enqueue_and_wait = gitforge_ci.enqueue_and_wait
publish_candidate = gitforge_ci.publish_candidate


class TriggerServer(BaseHTTPRequestHandler):
    get_count = 0
    authorization = None
    payload = None

    def do_POST(self):  # noqa: N802
        type(self).authorization = self.headers.get("Authorization")
        type(self).payload = self.rfile.read(int(self.headers.get("Content-Length", "0")))
        self.send_response(202)
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        self.wfile.write(
            b'{"status":"queued","trigger_id":"trigger-123",'
            b'"pipeline_run_id":null}'
        )

    def do_GET(self):  # noqa: N802
        type(self).get_count += 1
        type(self).authorization = self.headers.get("Authorization")
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        if type(self).get_count == 1:
            self.wfile.write(b'{"status":"pending","pipeline_run_id":null}')
        else:
            self.wfile.write(
                b'{"status":"completed","pipeline_run_id":"run-456",'
                b'"pipeline_status":"queued"}'
            )

    def log_message(self, _format, *_args):
        pass


class GitForgeWorkflowContractTests(unittest.TestCase):
    @staticmethod
    def stop_server(server, worker):
        server.shutdown()
        worker.join(timeout=2)
        server.server_close()

    def test_pull_request_candidate_ref_is_scoped_to_pr_and_attempt(self):
        self.assertEqual(
            candidate_ref("pull_request", "327", "1001", "2"),
            "refs/heads/github-ci/pull/327/run/1001-2",
        )

    def test_push_candidate_ref_is_scoped_to_run(self):
        self.assertEqual(
            candidate_ref("push", None, "1002", "1"),
            "refs/heads/github-ci/push/run/1002-1",
        )

    def test_candidate_ref_rejects_invalid_event_identity(self):
        with self.assertRaises(ValueError):
            candidate_ref("pull_request", "../main", "1001", "1")

    def test_candidate_checkout_fetches_full_history_for_gitforge_push(self):
        workflow = (
            Path(__file__).resolve().parents[2]
            / ".github"
            / "workflows"
            / "gitforge-ci.yml"
        ).read_text(encoding="utf-8")
        candidate_checkout = re.search(
            r"(?ms)^      - name: Check out candidate commit\n(?P<step>.*?)(?=^      - name:|\Z)",
            workflow,
        )

        self.assertIsNotNone(candidate_checkout)
        self.assertRegex(candidate_checkout.group("step"), r"(?m)^          fetch-depth: 0$")

    def test_publish_candidate_transfers_commit_and_verifies_remote_ref(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            bare = root / "remote.git"
            work = root / "work"
            work.mkdir()
            subprocess.run(["git", "init", "--bare", str(bare)], check=True, capture_output=True)
            subprocess.run(["git", "init", "-b", "main"], cwd=work, check=True, capture_output=True)
            subprocess.run(["git", "config", "user.name", "Workflow Test"], cwd=work, check=True)
            subprocess.run(["git", "config", "user.email", "workflow@example.invalid"], cwd=work, check=True)
            (work / "source.txt").write_text("candidate commit\n", encoding="utf-8")
            subprocess.run(["git", "add", "source.txt"], cwd=work, check=True)
            subprocess.run(["git", "commit", "-m", "candidate"], cwd=work, check=True, capture_output=True)
            sha = subprocess.run(
                ["git", "rev-parse", "HEAD"], cwd=work, check=True, capture_output=True, text=True
            ).stdout.strip()

            publish_candidate(str(bare), "refs/heads/github-ci/pull/327/run/1001-1", sha, cwd=work)

            remote_sha = subprocess.run(
                ["git", "--git-dir", str(bare), "rev-parse", "refs/heads/github-ci/pull/327/run/1001-1"],
                check=True,
                capture_output=True,
                text=True,
            ).stdout.strip()
            self.assertEqual(remote_sha, sha)

    def test_enqueue_polls_valid_accepted_receipt_until_run_is_assigned(self):
        TriggerServer.get_count = 0
        server = ThreadingHTTPServer(("127.0.0.1", 0), TriggerServer)
        worker = threading.Thread(target=server.serve_forever, daemon=True)
        worker.start()
        self.addCleanup(self.stop_server, server, worker)

        run_id = enqueue_and_wait(
            f"http://127.0.0.1:{server.server_port}",
            "repo-uuid",
            "github-ci/pull/327/run/1001-1",
            "a" * 40,
            "test-trigger-token",
            old_sha="b" * 40,
            timeout_seconds=2,
            interval_seconds=0.01,
        )

        self.assertEqual(run_id, "run-456")
        self.assertGreaterEqual(TriggerServer.get_count, 2)
        self.assertEqual(TriggerServer.authorization, "Bearer test-trigger-token")
        self.assertEqual(
            json.loads(TriggerServer.payload),
            {
                "repo_id": "repo-uuid",
                "ref_name": "github-ci/pull/327/run/1001-1",
                "old_hash": "b" * 40,
                "new_hash": "a" * 40,
                "working_dir": None,
            },
        )


if __name__ == "__main__":
    unittest.main()

#!/usr/bin/env python3
"""Contract tests for qualityctl's manager compatibility and output modes."""

import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
QUALITYCTL = REPO_ROOT / "scripts" / "qualityctl"


class QualityctlContractTests(unittest.TestCase):
    def run_qualityctl(self, supports_show_output: bool, *args: str):
        with tempfile.TemporaryDirectory(prefix="gitforge-qualityctl.") as temp:
            root = Path(temp)
            cli = root / "gitforge-build"
            cli.write_text(
                """#!/usr/bin/env python3
import json, os, sys
args = sys.argv[1:]
if args == ["--help"]:
    print("Options: --show-output" if os.environ["MOCK_SHOW_OUTPUT"] == "1" else "Options: --socket --dir")
    raise SystemExit(0)
with open(os.environ["MOCK_CALLS"], "a", encoding="utf-8") as stream:
    stream.write(json.dumps(args) + "\\n")
raise SystemExit(0)
""",
                encoding="utf-8",
            )
            cli.chmod(0o755)

            daemon = root / "gitforge-buildd"
            daemon.write_text(
                """#!/usr/bin/env python3
import os, socket
server = socket.socket(socket.AF_UNIX)
server.bind(os.environ["GITFORGE_BUILD_SOCKET"])
server.close()
""",
                encoding="utf-8",
            )
            daemon.chmod(0o755)

            report_dir = root / "reports"
            calls = root / "calls.jsonl"
            env = os.environ.copy()
            env.update(
                {
                    "GITFORGE_BUILD_CLI": str(cli),
                    "GITFORGE_BUILD_DAEMON": str(daemon),
                    "GITFORGE_BUILD_SOCKET": str(root / "manager.sock"),
                    "QUALITY_REPORT_DIR": str(report_dir),
                    "MOCK_CALLS": str(calls),
                    "MOCK_SHOW_OUTPUT": "1" if supports_show_output else "0",
                }
            )
            result = subprocess.run(
                [str(QUALITYCTL), *args],
                cwd=REPO_ROOT,
                env=env,
                text=True,
                capture_output=True,
                check=False,
                timeout=30,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            call_args = [json.loads(line) for line in calls.read_text().splitlines()]
            stages = [call for call in call_args if "--" in call and "--shutdown" not in call]
            self.assertEqual(len(stages), 3)
            self.assertTrue(
                all(("--show-output" in call) == supports_show_output for call in stages),
                call_args,
            )
            manifest = json.loads((report_dir / "quality-manifest.json").read_text())
            self.assertEqual(manifest["status"], "passed")
            self.assertEqual(len(manifest["stages"]), 3)
            return result, manifest

    def test_json_flag_emits_manifest_with_old_manager(self):
        result, manifest = self.run_qualityctl(False, "--fast", "--json")
        self.assertEqual(json.loads(result.stdout), manifest)

    def test_json_flag_emits_manifest_and_enables_output_with_new_manager(self):
        result, manifest = self.run_qualityctl(True, "--fast", "--json")
        self.assertEqual(json.loads(result.stdout), manifest)

    def test_default_mode_is_human_readable(self):
        result, manifest = self.run_qualityctl(False, "--fast")
        self.assertIn("qualityctl: passed (3 stages)", result.stdout)
        self.assertIn("manifest:", result.stdout)
        self.assertNotIn('"contract_version"', result.stdout)
        self.assertEqual(manifest["status"], "passed")


if __name__ == "__main__":
    unittest.main()

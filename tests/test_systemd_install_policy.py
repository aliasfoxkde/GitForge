#!/usr/bin/env python3
"""Contract tests for the NAS GitForge systemd installer policy."""

import os
import subprocess
import tempfile
import unittest
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
INSTALLER = REPO_ROOT / "systemd" / "gitforge-install.sh"


class SystemdInstallPolicyTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.source = INSTALLER.read_text()

    def test_nas_default_service_loop_excludes_local_runner(self):
        self.assertIn("for svc in api git-server ci; do", self.source)

    def test_local_runner_requires_explicit_opt_in(self):
        self.assertIn('ENABLE_LOCAL_RUNNER="${GITFORGE_ENABLE_LOCAL_RUNNER:-0}"', self.source)
        self.assertIn('if [[ "$ENABLE_LOCAL_RUNNER" == "1" ]]; then', self.source)
        opt_in = self.source.split(
            'if [[ "$ENABLE_LOCAL_RUNNER" == "1" ]]; then', 1
        )[1].split("\nelse\n", 1)[0]
        self.assertIn("systemctl unmask gitforge@runner.service", opt_in)
        self.assertIn("systemctl enable gitforge@runner.service", opt_in)
        self.assertIn("systemctl start gitforge@runner.service", opt_in)

    def test_default_path_disables_and_masks_local_runner(self):
        default = self.source.split("\nelse\n", 1)[1].split("\nfi\n", 1)[0]
        self.assertIn("systemctl disable gitforge@runner.service", default)
        self.assertIn("systemctl mask gitforge@runner.service", default)

    def test_default_install_refuses_an_active_runner_before_mutation(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            stub_bin = root / "bin"
            stub_bin.mkdir()
            log = root / "calls.log"
            systemctl = stub_bin / "systemctl"
            systemctl.write_text(
                "#!/bin/sh\n"
                "printf 'systemctl %s\\n' \"$*\" >> \"$GITFORGE_TEST_LOG\"\n"
                "if [ \"${1:-}\" = is-active ]; then\n"
                "  printf '%s\\n' \"$GITFORGE_TEST_RUNNER_STATE\"\n"
                "  exit 0\n"
                "fi\n"
                "exit 0\n",
                encoding="utf-8",
            )
            systemctl.chmod(0o755)
            for command in (
                "cp",
                "mkdir",
                "chown",
                "install",
                "systemd-tmpfiles",
                "useradd",
            ):
                stub = stub_bin / command
                stub.write_text(
                    "#!/bin/sh\n"
                    f"printf '{command} %s\\n' \"$*\" >> \"$GITFORGE_TEST_LOG\"\n"
                    "exit 0\n",
                    encoding="utf-8",
                )
                stub.chmod(0o755)

            env = os.environ.copy()
            env.update(
                {
                    "PATH": f"{stub_bin}{os.pathsep}{env.get('PATH', '')}",
                    "GITFORGE_TEST_LOG": str(log),
                    "GITFORGE_TEST_RUNNER_STATE": "active",
                    "GITFORGE_ENABLE_LOCAL_RUNNER": "0",
                }
            )
            result = subprocess.run(
                ["/bin/bash", str(INSTALLER)],
                cwd=REPO_ROOT,
                env=env,
                capture_output=True,
                text=True,
                check=False,
                timeout=5,
            )

            self.assertEqual(result.returncode, 1, result.stderr)
            self.assertIn("Refusing to mask active GitForge runner", result.stderr)
            self.assertEqual(
                log.read_text(encoding="utf-8").splitlines(),
                ["systemctl is-active gitforge@runner.service"],
            )

    def test_default_install_fails_closed_on_unrecognized_runner_state(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            stub_bin = root / "bin"
            stub_bin.mkdir()
            log = root / "calls.log"
            systemctl = stub_bin / "systemctl"
            systemctl.write_text(
                "#!/bin/sh\n"
                "printf 'systemctl %s\\n' \"$*\" >> \"$GITFORGE_TEST_LOG\"\n"
                "printf 'indeterminate\\n'\n"
                "exit 1\n",
                encoding="utf-8",
            )
            systemctl.chmod(0o755)
            for command in (
                "cp",
                "mkdir",
                "chown",
                "install",
                "systemd-tmpfiles",
                "useradd",
            ):
                stub = stub_bin / command
                stub.write_text(
                    "#!/bin/sh\n"
                    f"printf '{command} %s\\n' \"$*\" >> \"$GITFORGE_TEST_LOG\"\n"
                    "exit 0\n",
                    encoding="utf-8",
                )
                stub.chmod(0o755)
            env = os.environ.copy()
            env.update(
                {
                    "PATH": f"{stub_bin}{os.pathsep}{env.get('PATH', '')}",
                    "GITFORGE_TEST_LOG": str(log),
                    "GITFORGE_ENABLE_LOCAL_RUNNER": "0",
                }
            )
            result = subprocess.run(
                ["/bin/bash", str(INSTALLER)],
                cwd=REPO_ROOT,
                env=env,
                capture_output=True,
                text=True,
                check=False,
                timeout=5,
            )

            self.assertEqual(result.returncode, 1, result.stderr)
            self.assertIn("Unable to determine GitForge runner state", result.stderr)
            self.assertEqual(
                log.read_text(encoding="utf-8").splitlines(),
                ["systemctl is-active gitforge@runner.service"],
            )

    def test_invalid_local_runner_setting_fails_closed(self):
        self.assertIn(
            'if [[ "$ENABLE_LOCAL_RUNNER" != "0" && "$ENABLE_LOCAL_RUNNER" != "1" ]]; then',
            self.source,
        )
        self.assertIn('echo "GITFORGE_ENABLE_LOCAL_RUNNER must be 0 or 1" >&2', self.source)
        self.assertIn("exit 2", self.source)


if __name__ == "__main__":
    unittest.main()

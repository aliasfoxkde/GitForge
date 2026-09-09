#!/usr/bin/env python3
"""Contract tests for the release bundle and verification scripts.

Exercises scripts/gitforge-release-bundle and scripts/gitforge-release-verify
end to end against a throwaway fixture assembled by the test itself:

1. a valid bundle contains the required binaries, metadata, READY marker,
   checksum manifest, unit examples, and resource policies;
2. the verifier accepts the valid bundle and its manifest;
3. tampering with a bundled binary fails verification;
4. malformed or incomplete bundles fail closed with useful diagnostics.

The tests are deterministic: no network, no real build artifacts, and every
test assembles its own bundle from a fresh temporary fixture. Run with either
runner:

    python3 tests/test_release_bundle.py -v
    python3 -m pytest tests/test_release_bundle.py -v
"""

import hashlib
import os
import subprocess
import tempfile
import unittest
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
BUNDLE_SCRIPT = REPO_ROOT / "scripts" / "gitforge-release-bundle"
VERIFY_SCRIPT = REPO_ROOT / "scripts" / "gitforge-release-verify"

REQUIRED_BINARIES = ("api", "ci", "git-server", "runner")
UNIT_EXAMPLES = (
    "gitforge-ci.service.example",
    "gitforge-runner.service.example",
)
RESOURCE_POLICIES = (
    "gitforge-resource-limits.conf",
    "gitforge-ci-resource-limits.conf",
    "gitforge-runner-resource-limits.conf",
)

# The bundler only requires a 40-hex source commit; this synthetic value keeps
# the test independent of whatever the checkout HEAD happens to be.
SOURCE_COMMIT = "4a1f2b3c4d5e6f708192a3b4c5d6e7f8091a2b3c"
TOOLCHAIN = "test-rustc 1.0.0 (canary)"
RELEASE_ID = "20260909-canary"

MANIFEST_FILES = frozenset(
    ("RELEASE_METADATA.txt",)
    + tuple(f"bin/{name}" for name in REQUIRED_BINARIES)
    + ("scripts/gitforge-status",)
    + tuple(f"systemd/user/{name}" for name in UNIT_EXAMPLES)
    + tuple(f"systemd/user/{name}" for name in RESOURCE_POLICIES)
)


class ReleaseBundleContractTests(unittest.TestCase):
    maxDiff = None

    def setUp(self):
        tmp = tempfile.TemporaryDirectory(prefix="gitforge-release-contract.")
        self.addCleanup(tmp.cleanup)
        self.workdir = Path(tmp.name)
        self.source_root = self.workdir / "source"
        self.bundle_root = self.workdir / "bundle"
        self.release_dir = self.bundle_root / RELEASE_ID
        self.source_root.mkdir()
        self.bundle_root.mkdir()
        for name in REQUIRED_BINARIES:
            self._write_executable(
                self.source_root / name, f"#!/bin/sh\n# test stub: {name}\nexit 0\n"
            )
        # The bundler copies the status helper from the candidate source root,
        # not from this repository, so the fixture must provide one.
        self._write_executable(
            self.source_root / "scripts" / "gitforge-status",
            "#!/bin/sh\n# test stub: gitforge-status\nexit 0\n",
        )
        self.bundle()

    # ─── fixture and invocation helpers ──────────────────────────────────────

    @staticmethod
    def _write_executable(path, content):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content)
        path.chmod(0o755)

    def _env(self):
        env = os.environ.copy()
        env["GITFORGE_SOURCE_COMMIT"] = SOURCE_COMMIT
        env["GITFORGE_TOOLCHAIN"] = TOOLCHAIN
        return env

    def _run(self, script, *args):
        return subprocess.run(
            [str(script), *args],
            capture_output=True,
            text=True,
            env=self._env(),
            timeout=120,
            check=False,
        )

    def bundle(self):
        result = self._run(
            BUNDLE_SCRIPT, str(self.source_root), str(self.bundle_root), RELEASE_ID
        )
        self.assertEqual(
            result.returncode, 0, f"bundle failed:\n{result.stdout}\n{result.stderr}"
        )
        self.assertIn("release preflight: all candidate binaries present", result.stdout)
        self.assertIn("release bundle: ready", result.stdout)
        return result

    def verify(self):
        return self._run(VERIFY_SCRIPT, str(self.release_dir))

    @staticmethod
    def _read_kv(path):
        values = {}
        for line in path.read_text().splitlines():
            key, sep, value = line.partition("=")
            if sep:
                values[key] = value
        return values

    def _manifest_entries(self):
        entries = {}
        for line in (self.release_dir / "MANIFEST.sha256").read_text().splitlines():
            digest, sep, rel_path = line.partition("  ")
            self.assertTrue(sep, f"malformed manifest line: {line!r}")
            self.assertRegex(digest, r"^[0-9a-f]{64}$", f"bad digest: {line!r}")
            self.assertNotIn(rel_path, entries, f"duplicate manifest entry: {rel_path}")
            entries[rel_path] = digest
        return entries

    def _assert_executable(self, rel_path):
        path = self.release_dir / rel_path
        self.assertTrue(path.is_file(), f"missing bundled file: {rel_path}")
        self.assertTrue(os.access(path, os.X_OK), f"not executable: {rel_path}")

    # ─── 1. valid bundle payload ─────────────────────────────────────────────

    def test_valid_bundle_contains_required_payload(self):
        for name in REQUIRED_BINARIES:
            self._assert_executable(f"bin/{name}")
        self._assert_executable("scripts/gitforge-status")

        for name in UNIT_EXAMPLES + RESOURCE_POLICIES:
            self.assertTrue(
                (self.release_dir / "systemd" / "user" / name).is_file(),
                f"missing systemd payload: {name}",
            )

        ready = self._read_kv(self.release_dir / "READY")
        self.assertEqual(ready.get("release_id"), RELEASE_ID)
        self.assertEqual(ready.get("source_commit"), SOURCE_COMMIT)
        self.assertEqual(ready.get("manifest"), "MANIFEST.sha256")

        metadata = self._read_kv(self.release_dir / "RELEASE_METADATA.txt")
        self.assertEqual(metadata.get("release_id"), RELEASE_ID)
        self.assertEqual(metadata.get("source_commit"), SOURCE_COMMIT)
        self.assertEqual(metadata.get("toolchain"), TOOLCHAIN)
        self.assertRegex(
            metadata.get("build_timestamp_utc", ""),
            r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$",
        )

        manifest = self._manifest_entries()
        self.assertEqual(set(manifest), set(MANIFEST_FILES))
        for rel_path, digest in manifest.items():
            actual = hashlib.sha256((self.release_dir / rel_path).read_bytes()).hexdigest()
            self.assertEqual(actual, digest, f"manifest digest mismatch: {rel_path}")

        ci_unit = (self.release_dir / "systemd/user/gitforge-ci.service.example").read_text()
        self.assertIn("ExecStart=%h/work/gitforge-current/bin/ci", ci_unit)
        self.assertIn("Environment=SCHEDULER_PORT=42781", ci_unit)
        runner_unit = (
            self.release_dir / "systemd/user/gitforge-runner.service.example"
        ).read_text()
        self.assertIn("Requires=gitforge-ci.service", runner_unit)
        self.assertNotIn("gitforge-scheduler.service", runner_unit)

    # ─── 2. verifier accepts the valid bundle ────────────────────────────────

    def test_verifier_accepts_valid_bundle(self):
        result = self.verify()
        self.assertEqual(
            result.returncode, 0, f"verify failed:\n{result.stdout}\n{result.stderr}"
        )
        self.assertIn("release verify: valid bundle=", result.stdout)
        self.assertIn(f"source={SOURCE_COMMIT}", result.stdout)
        self.assertIn("binaries=4", result.stdout)

    # ─── 3. tampering is detected ────────────────────────────────────────────

    def test_tampered_binary_fails_verification(self):
        target = self.release_dir / "bin" / "ci"
        with open(target, "r+b") as handle:
            handle.write(b"# tampered\n")

        result = self.verify()
        self.assertNotEqual(result.returncode, 0, "verify accepted a tampered binary")
        self.assertIn("FAILED", result.stdout + result.stderr)
        # READY and metadata still agree, so the failure is the checksum gate,
        # not the metadata consistency check.
        self.assertNotIn("valid bundle=", result.stdout)

    # ─── 4. malformed or incomplete bundles fail closed ─────────────────────

    def test_missing_ready_marker_fails_closed(self):
        (self.release_dir / "READY").unlink()
        result = self.verify()
        self.assertEqual(result.returncode, 1)
        self.assertIn(
            "non-empty READY marker, metadata, and checksum manifest are required",
            result.stderr,
        )

    def test_missing_binary_fails_closed(self):
        (self.release_dir / "bin" / "runner").unlink()
        result = self.verify()
        self.assertEqual(result.returncode, 1)
        self.assertIn("missing or non-executable binary: runner", result.stderr)

    def test_missing_resource_policy_fails_closed(self):
        # The manifest checksum gate runs first, so a policy that is merely
        # deleted from disk fails with a manifest diagnostic. The dedicated
        # "missing resource policy" gate fires when the bundler omitted the
        # payload entirely: absent from both disk and manifest.
        policy = self.release_dir / "systemd/user/gitforge-ci-resource-limits.conf"
        policy.unlink()
        manifest = self.release_dir / "MANIFEST.sha256"
        kept = [
            line
            for line in manifest.read_text().splitlines()
            if not line.endswith("systemd/user/gitforge-ci-resource-limits.conf")
        ]
        manifest.write_text("\n".join(kept) + "\n")
        result = self.verify()
        self.assertEqual(result.returncode, 1)
        self.assertIn("missing resource policy:", result.stderr)

    def test_inconsistent_source_commit_fails_closed(self):
        ready = self.release_dir / "READY"
        lines = ready.read_text().splitlines()
        ready.write_text(
            "\n".join(
                f"source_commit={'b' * 40}" if line.startswith("source_commit=") else line
                for line in lines
            )
            + "\n"
        )
        result = self.verify()
        self.assertEqual(result.returncode, 1)
        self.assertIn(
            "metadata and READY source commits are missing or inconsistent", result.stderr
        )

    def test_verify_rejects_missing_directory(self):
        result = self._run(VERIFY_SCRIPT, str(self.workdir / "does-not-exist"))
        self.assertEqual(result.returncode, 1)
        self.assertIn("release verify: directory does not exist:", result.stderr)


if __name__ == "__main__":
    unittest.main()

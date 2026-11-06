#!/usr/bin/env python3
"""Contract tests for the release bundle and verification scripts.

Exercises scripts/gitforge-release-bundle and scripts/gitforge-release-verify
end to end against a throwaway fixture assembled by the test itself:

1. a valid bundle contains the required binaries, metadata, READY marker,
   checksum manifest, unit examples, and resource policies;
2. the verifier accepts the valid bundle and its manifest;
3. tampering with a bundled binary fails verification;
4. malformed or incomplete bundles fail closed with useful diagnostics;
5. the bundle's provenance claim is enforced: binaries that were not built
   from the claimed commit are refused, by the bundler and by the verifier.

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
STAMPS_SCRIPT = REPO_ROOT / "scripts" / "gitforge-release-stamps"

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
OTHER_COMMIT = "0f38a46345ac82efec9493310aa5c556c31c1ac5"
# The toolchain and the build time the BINARIES carry. A toolchain line
# carries spaces, exactly as `rustc --version` reports it.
BUILD_TOOLCHAIN = "rustc 1.99.1 (58cf8f9c3 2026-10-13)"
BUILD_TIME = "2026-11-06T08:15:00Z"

# The toolchain of the host that runs the CUT. It is deliberately different
# from the one the binaries were built with: a bundle whose metadata names
# this toolchain describes the cutting host instead of the artifact it
# ships (GF-15, 2026-11-06).
CUTTING_HOST_TOOLCHAIN = "rustc 1.99.0 (b940084d7 2026-09-28)"

RELEASE_ID = "20260909-canary"

MANIFEST_FILES = frozenset(
    ("RELEASE_METADATA.txt", "READY")
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
            self._write_executable(self.source_root / name, self._stub(name))
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

    @staticmethod
    def _stub(
        name,
        commit=SOURCE_COMMIT,
        dirty="false",
        stamped=True,
        toolchain=BUILD_TOOLCHAIN,
        built=BUILD_TIME,
    ):
        """A stand-in for a release binary.

        Real release binaries answer `--version` with the commit they were
        built from, the toolchain that compiled them and the moment they
        were compiled; the release chain refuses a bundle whose binaries do
        not agree with the commit the bundle claims, and describes the
        bundle from those same fields. The stub reproduces that contract so
        the contract can be tested without a real build.
        """
        if not stamped:
            return f"#!/bin/sh\n# test stub: {name}\nexit 0\n"
        return (
            f"#!/bin/sh\n"
            f"# test stub: {name}\n"
            f'case "$1" in\n'
            f"  --version|-V)\n"
            f'    echo "{name} 0.0.0-test commit={commit} dirty={dirty} '
            f"toolchain={toolchain} built={built}\"\n"
            f"    exit 0\n"
            f"    ;;\n"
            f"esac\n"
            f"exit 0\n"
        )

    def _env(self):
        env = os.environ.copy()
        env["GITFORGE_SOURCE_COMMIT"] = SOURCE_COMMIT
        # The host that runs the cut states its own toolchain. The bundler
        # must not use it: the bundle describes the binaries it ships.
        env["GITFORGE_TOOLCHAIN"] = CUTTING_HOST_TOOLCHAIN
        # The bundler consults the live-instance release gate, which no
        # test fixture can satisfy; the gate itself is covered by
        # tests/test_release_gate.py against a purpose-built database.
        env["GITFORGE_RELEASE_SKIP_GATE"] = "1"
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
        # The metadata describes the binaries, not the cutting host.
        self.assertEqual(metadata.get("toolchain"), BUILD_TOOLCHAIN)
        self.assertEqual(metadata.get("build_timestamp_utc"), BUILD_TIME)
        self.assertRegex(
            metadata.get("bundle_cut_utc", ""),
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

    def test_tampered_ready_marker_fails_verification(self):
        ready = self.release_dir / "READY"
        ready.write_text(ready.read_text() + "tampered=yes\n")

        result = self.verify()
        self.assertNotEqual(result.returncode, 0, "verify accepted a tampered READY marker")
        self.assertIn("FAILED", result.stdout + result.stderr)
        self.assertNotIn("valid bundle=", result.stdout)

    def test_manifest_missing_ready_entry_fails_verification(self):
        manifest = self.release_dir / "MANIFEST.sha256"
        manifest.write_text(
            "".join(
                line
                for line in manifest.read_text().splitlines(keepends=True)
                if not line.endswith("  READY\n")
            )
        )

        result = self.verify()
        self.assertEqual(result.returncode, 1)
        self.assertIn(
            "checksum manifest must contain exactly one READY entry", result.stderr
        )

    def test_manifest_duplicate_ready_entry_fails_verification(self):
        manifest = self.release_dir / "MANIFEST.sha256"
        ready_entry = next(
            line
            for line in manifest.read_text().splitlines(keepends=True)
            if line.endswith("  READY\n")
        )
        manifest.write_text(manifest.read_text() + ready_entry)

        result = self.verify()
        self.assertEqual(result.returncode, 1)
        self.assertIn(
            "checksum manifest must contain exactly one READY entry", result.stderr
        )

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
        # READY is manifest-covered, so the tamper trips the checksum gate
        # before the consistency gate; either diagnostic is a fail-closed
        # catch of the same tamper.
        combined = result.stdout + result.stderr
        self.assertTrue(
            "metadata and READY source commits are missing or inconsistent" in combined
            or "computed checksum did NOT match" in combined,
            combined,
        )

    def test_verify_rejects_missing_directory(self):
        result = self._run(VERIFY_SCRIPT, str(self.workdir / "does-not-exist"))
        self.assertEqual(result.returncode, 1)
        self.assertIn("release verify: directory does not exist:", result.stderr)

    # ─── 5. the bundle's provenance claim is enforced ────────────────────────
    #
    # GF-14 (2026-11-05): the bundler copied whatever binaries sat in the
    # candidate source root and stamped them with the commit the root was
    # on. The release binaries live in gitignored paths, so a bundle of
    # stale leftovers passed every other gate while claiming a commit it was
    # never built from. These cases pin the fix: the claim now has to be
    # carried by the binaries, and the tooling refuses when it is not.

    def _refused_cut(self):
        """Run the bundler against a release id that was never used.

        The fixture already published one valid bundle, so a refusal has to
        be judged on its own release id: the bundler refuses to overwrite an
        existing release, and that refusal would otherwise mask the
        provenance diagnostic.
        """
        release_id = RELEASE_ID + "-refused"
        result = self._run(
            BUNDLE_SCRIPT, str(self.source_root), str(self.bundle_root), release_id
        )
        self.assertNotEqual(result.returncode, 0, "the bundler accepted the bundle")
        self.assertFalse(
            (self.bundle_root / release_id).exists(),
            "a refused cut published a bundle anyway",
        )
        self.assertEqual(
            sorted(p.name for p in self.bundle_root.iterdir()),
            [RELEASE_ID],
            "a refused cut left a staging tree behind",
        )
        return result

    def _rewrite_manifest(self):
        """Recompute the checksum manifest.

        Used by the cases below so that the checksum gate cannot be the
        reason a verification fails: they target the provenance gate.
        """
        entries = ["READY", "RELEASE_METADATA.txt"]
        entries += [f"bin/{name}" for name in REQUIRED_BINARIES]
        entries += ["scripts/gitforge-status"]
        entries += [f"systemd/user/{name}" for name in UNIT_EXAMPLES]
        entries += [f"systemd/user/{name}" for name in RESOURCE_POLICIES]
        lines = []
        for rel_path in entries:
            digest = hashlib.sha256((self.release_dir / rel_path).read_bytes()).hexdigest()
            lines.append(f"{digest}  {rel_path}\n")
        lines.sort()
        (self.release_dir / "MANIFEST.sha256").write_text("".join(lines))

    def test_bundler_refuses_binaries_built_from_another_commit(self):
        self._write_executable(
            self.source_root / "ci", self._stub("ci", commit=OTHER_COMMIT)
        )
        result = self._refused_cut()
        self.assertIn("was built from", result.stderr)
        self.assertIn(OTHER_COMMIT, result.stderr)
        self.assertIn(SOURCE_COMMIT, result.stderr)

    def test_bundler_refuses_binaries_without_a_build_stamp(self):
        self._write_executable(
            self.source_root / "runner", self._stub("runner", stamped=False)
        )
        result = self._refused_cut()
        self.assertIn("does not report a build commit", result.stderr)

    def test_bundler_refuses_binaries_built_from_a_modified_tree(self):
        self._write_executable(self.source_root / "api", self._stub("api", dirty="true"))
        result = self._refused_cut()
        self.assertIn("was built from a tree that was not clean", result.stderr)

    def test_verifier_refuses_a_bundle_whose_binary_disagrees_with_metadata(self):
        # A bundle cut by the pre-fix bundler: the manifest is internally
        # consistent, the metadata claims SOURCE_COMMIT, and the binary says
        # otherwise. Only the provenance gate can catch this.
        self._write_executable(
            self.release_dir / "bin" / "ci", self._stub("ci", commit=OTHER_COMMIT)
        )
        self._rewrite_manifest()

        result = self.verify()
        self.assertNotEqual(
            result.returncode, 0, "verifier accepted a bundle whose binary lies about its commit"
        )
        self.assertIn("was built from", result.stderr)
        self.assertIn(OTHER_COMMIT, result.stderr)
        self.assertNotIn("valid bundle=", result.stdout)

    def test_verifier_refuses_a_bundle_built_from_a_modified_tree(self):
        self._write_executable(
            self.release_dir / "bin" / "api", self._stub("api", dirty="true")
        )
        self._rewrite_manifest()

        result = self.verify()
        self.assertNotEqual(
            result.returncode, 0, "verifier accepted a bundle from a modified tree"
        )
        self.assertIn("was built from a tree that was not clean", result.stderr)
        self.assertNotIn("valid bundle=", result.stdout)

    def test_stamps_check_accepts_matching_binaries_and_rejects_others(self):
        accepted = self._run(STAMPS_SCRIPT, str(self.source_root), SOURCE_COMMIT)
        self.assertEqual(
            accepted.returncode, 0, f"stamps refused matching binaries:\n{accepted.stderr}"
        )
        self.assertIn(SOURCE_COMMIT, accepted.stdout)

        rejected = self._run(STAMPS_SCRIPT, str(self.source_root), OTHER_COMMIT)
        self.assertEqual(rejected.returncode, 1)
        self.assertIn("was built from", rejected.stderr)


    # ─── 6. the metadata describes the artifact it ships (GF-15) ────────────
    #
    # GF-15 (2026-11-06): the bundle's metadata named the CUTTING host - its
    # `rustc --version` and the time of the cut - so a bundle of binaries
    # compiled on one host claimed the toolchain of the host that assembled
    # it. The metadata has to be read out of the binaries instead.

    def test_metadata_describes_the_binaries_not_the_cutting_host(self):
        metadata = self._read_kv(self.release_dir / "RELEASE_METADATA.txt")
        self.assertEqual(metadata.get("toolchain"), BUILD_TOOLCHAIN)
        self.assertNotEqual(metadata.get("toolchain"), CUTTING_HOST_TOOLCHAIN)
        self.assertEqual(metadata.get("build_timestamp_utc"), BUILD_TIME)
        # The cut itself is still recorded - as its own field.
        self.assertIn("bundle_cut_utc", metadata)

    def test_verifier_refuses_metadata_that_names_another_toolchain(self):
        metadata_path = self.release_dir / "RELEASE_METADATA.txt"
        lines = metadata_path.read_text().splitlines()
        metadata_path.write_text(
            "\n".join(
                f"toolchain={CUTTING_HOST_TOOLCHAIN}"
                if line.startswith("toolchain=")
                else line
                for line in lines
            )
            + "\n"
        )
        self._rewrite_manifest()

        result = self.verify()
        self.assertNotEqual(
            result.returncode, 0, "verifier accepted metadata that describes another build"
        )
        self.assertIn("does not describe the shipped binaries", result.stderr)
        self.assertNotIn("valid bundle=", result.stdout)

    def test_verifier_refuses_a_metadata_build_time_the_binaries_do_not_carry(self):
        metadata_path = self.release_dir / "RELEASE_METADATA.txt"
        lines = metadata_path.read_text().splitlines()
        metadata_path.write_text(
            "\n".join(
                "build_timestamp_utc=2020-01-01T00:00:00Z"
                if line.startswith("build_timestamp_utc=")
                else line
                for line in lines
            )
            + "\n"
        )
        self._rewrite_manifest()

        result = self.verify()
        self.assertNotEqual(
            result.returncode, 0, "verifier accepted a metadata build time the binaries do not carry"
        )
        self.assertIn("does not describe the shipped binaries", result.stderr)
        self.assertNotIn("valid bundle=", result.stdout)

    def test_bundler_refuses_binaries_that_disagree_about_the_toolchain(self):
        self._write_executable(
            self.source_root / "runner", self._stub("runner", toolchain="rustc 0.0.1 (other)")
        )
        result = self._refused_cut()
        self.assertIn("disagree about toolchain", result.stderr)

    def test_bundler_refuses_binaries_that_cannot_say_when_they_were_built(self):
        self._write_executable(self.source_root / "api", self._stub("api", built="unknown"))
        result = self._refused_cut()
        self.assertIn("does not report when it was built", result.stderr)

    def test_stamps_field_mode_reports_the_value_the_binaries_agree_on(self):
        for field, expected in (("toolchain", BUILD_TOOLCHAIN), ("built", BUILD_TIME)):
            result = self._run(STAMPS_SCRIPT, "--field", field, str(self.source_root))
            self.assertEqual(
                result.returncode, 0, f"field mode refused:\n{result.stdout}\n{result.stderr}"
            )
            self.assertEqual(result.stdout.strip(), expected)

    def test_stamps_field_mode_refuses_binaries_that_disagree(self):
        self._write_executable(
            self.source_root / "ci", self._stub("ci", toolchain="rustc 0.0.1 (other)")
        )
        result = self._run(STAMPS_SCRIPT, "--field", "toolchain", str(self.source_root))
        self.assertNotEqual(result.returncode, 0, "field mode accepted disagreeing binaries")
        self.assertIn("disagree about toolchain", result.stderr)


if __name__ == "__main__":
    unittest.main()

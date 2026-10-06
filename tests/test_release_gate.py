#!/usr/bin/env python3
"""Contract tests for scripts/gitforge-release-gate.

Builds a throwaway SQLite fixture holding the pipelines / pipeline_runs /
jobs tables the gate reads, then exercises the gate end to end under both
evidence readers:

- the sqlite3 CLI path, via a faithful PATH shim of the one invocation the
  gate is allowed to make: `-batch -readonly <db> ".timeout 15000" <sql>`
  in list mode. The shim refuses any other argv, so every CLI-path test
  transitively pins the read-only and busy-timeout wiring;
- the python3 sqlite3 fallback path, forced by hiding the CLI behind a
  non-executable PATH stub (the natural mode on hosts like Fedora that
  ship the sqlite3 module without the CLI).

Every fail-closed gate is asserted on both paths — exact 40-hex commit
argument, run existence, a succeeded run, run-id shape, readable persisted
definition, durable-row coverage of the definition, durable count parity,
and all-jobs-succeeded — and the two readers must agree byte-for-byte on
stdout and exit status. The green path additionally asserts the database
file is not modified, proving the readers stay read-only.

The tests are deterministic: no network, no real build artifacts, no
cargo. Run with either runner:

    python3 tests/test_release_gate.py -v
    python3 -m pytest tests/test_release_gate.py -v
"""

import hashlib
import json
import os
import shutil
import sqlite3
import subprocess
import tempfile
import unittest
from pathlib import Path
from uuid import uuid4

REPO_ROOT = Path(__file__).resolve().parents[1]
GATE_SCRIPT = REPO_ROOT / "scripts" / "gitforge-release-gate"

# Synthetic values keep the fixture independent of any real checkout or
# database (same convention as tests/test_release_bundle.py).
SOURCE_COMMIT = "4a1f2b3c4d5e6f708192a3b4c5d6e7f8091a2b3c"
PIPELINE_ID = "pl-fixture-0001"
RUN_ID = "8f2a6c1e-5b47-4a90-9c31-d2e8f0a6b7c4"

DEF_JOBS = (
    {
        "name": "fmt",
        "steps": ({"name": "fmt", "run": "cargo fmt --all -- --check"},),
    },
    {
        "name": "clippy",
        "steps": (
            {
                "name": "clippy",
                "run": "cargo clippy --workspace --all-targets -- -D warnings",
            },
        ),
    },
    {
        "name": "test",
        "steps": (
            {"name": "test", "run": "cargo test --workspace -- --test-threads=2"},
        ),
    },
)
DEF_COMMANDS = {job["name"]: [step["run"] for step in job["steps"]] for job in DEF_JOBS}

SCHEMA = """
CREATE TABLE pipelines (
  id TEXT PRIMARY KEY, repo_id TEXT, name TEXT, trigger_type TEXT,
  config TEXT, created_at TEXT, active INTEGER
);
CREATE TABLE pipeline_runs (
  id TEXT PRIMARY KEY, pipeline_id TEXT, repo_id TEXT, status TEXT,
  triggered_by TEXT, commit_hash TEXT, started_at TEXT, finished_at TEXT,
  created_at TEXT, rerun_key TEXT
);
CREATE TABLE jobs (
  id TEXT PRIMARY KEY, pipeline_run_id TEXT, name TEXT, status TEXT,
  runner_id TEXT, started_at TEXT, finished_at TEXT, retry_count INTEGER,
  created_at TEXT, commands TEXT, image TEXT, working_dir TEXT,
  result_json TEXT, lease_token TEXT, lease_generation INTEGER,
  timeout_secs INTEGER
);
"""

# Stands in for the sqlite3 CLI. Emulates exactly the invocation the gate
# must use — batch, read-only, 15s busy timeout, list-mode rows with NULL
# as empty — and refuses anything else with a loud exit code, so a gate
# regression in the CLI wiring fails every CLI-path test.
SQLITE3_SHIM = '''#!/usr/bin/env python3
"""Test shim emulating the sqlite3 CLI invocation the release gate uses."""
import os
import sqlite3
import sys
from urllib.parse import quote

args = sys.argv[1:]
if (
    len(args) != 5
    or args[0] != "-batch"
    or args[1] != "-readonly"
    or args[3] != ".timeout 15000"
):
    sys.stderr.write("sqlite3 shim: unexpected invocation: %r\\n" % (args,))
    sys.exit(3)
try:
    connection = sqlite3.connect(
        "file:" + quote(os.path.abspath(args[2])) + "?mode=ro",
        uri=True,
        timeout=15.0,
    )
    try:
        rows = connection.execute(args[4]).fetchall()
    finally:
        connection.close()
except sqlite3.Error as error:
    sys.stderr.write("sqlite3 shim: %s\\n" % error)
    sys.exit(1)
for row in rows:
    print("|".join("" if value is None else str(value) for value in row))
'''


class ReleaseGateContractTests(unittest.TestCase):
    maxDiff = None

    def setUp(self):
        tmp = tempfile.TemporaryDirectory(prefix="gitforge-release-gate.")
        self.addCleanup(tmp.cleanup)
        self.workdir = Path(tmp.name)

    # ─── fixture helpers ─────────────────────────────────────────────────────

    def _hermetic_tools(self, with_sqlite3_shim):
        """Fresh tools dir with a hermetic PATH.

        with_sqlite3_shim=True provides the executable CLI shim (the gate's
        CLI path). Otherwise no sqlite3 exists on PATH at all — the hermetic
        PATH excludes system directories — so `command -v sqlite3` fails and
        the gate must take the python3 fallback on any host.
        """
        tools_dir = self.workdir / f"tools-{uuid4().hex[:8]}"
        tools_dir.mkdir()
        for tool in ("bash", "awk", "grep", "dirname", "python3"):
            resolved = shutil.which(tool)
            self.assertIsNotNone(resolved, f"required tool not on PATH: {tool}")
            os.symlink(resolved, tools_dir / tool)
        if with_sqlite3_shim:
            sqlite3_path = tools_dir / "sqlite3"
            sqlite3_path.write_text(SQLITE3_SHIM)
            sqlite3_path.chmod(0o755)
        return tools_dir

    def _pipeline_row(self, config=json.dumps({"version": 1, "jobs": list(DEF_JOBS)})):
        return (PIPELINE_ID, "repo-1", "ci", "push", config,
                "2026-10-01T00:00:00Z", 1)

    def _run_row(self, run_id=RUN_ID, status="succeeded",
                 commit=SOURCE_COMMIT, pipeline_id=PIPELINE_ID):
        return (run_id, pipeline_id, "repo-1", status, "push", commit,
                "2026-10-06T10:00:00Z", "2026-10-06T10:06:00Z",
                "2026-10-06T10:00:00Z", None)

    def _job_row(self, run_id=RUN_ID, commands=(), status="succeeded"):
        return (f"job-{uuid4().hex}", run_id, f"job-{uuid4().hex}", status,
                "runner-1", "2026-10-06T10:00:00Z", "2026-10-06T10:05:00Z", 0,
                "2026-10-06T10:00:00Z", json.dumps(list(commands)), "dsc-ci-rust:7",
                "/workspace", None, None, None, 900)

    def _build_db(self, *, pipelines, runs, jobs=()):
        db_path = self.workdir / "gitforge.db"
        connection = sqlite3.connect(db_path)
        try:
            connection.executescript(SCHEMA)
            connection.executemany("INSERT INTO pipelines VALUES (?,?,?,?,?,?,?)",
                                   pipelines)
            connection.executemany(
                "INSERT INTO pipeline_runs VALUES (?,?,?,?,?,?,?,?,?,?)", runs)
            connection.executemany(
                "INSERT INTO jobs VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)", jobs)
            connection.commit()
        finally:
            connection.close()
        return db_path

    def _green_db(self):
        return self._build_db(
            pipelines=[self._pipeline_row()],
            runs=[self._run_row()],
            jobs=[self._job_row(commands=DEF_COMMANDS["fmt"]),
                  self._job_row(commands=DEF_COMMANDS["clippy"]),
                  self._job_row(commands=DEF_COMMANDS["test"])],
        )

    # ─── invocation helpers ──────────────────────────────────────────────────

    def _env(self, backend):
        env = os.environ.copy()
        env["PATH"] = str(self._hermetic_tools(backend == "cli"))
        return env

    def _run_gate(self, db_path, backend, commit=SOURCE_COMMIT, extra_env=None):
        env = self._env(backend)
        if extra_env:
            env.update(extra_env)
        args = [str(GATE_SCRIPT)]
        if commit is not None:
            args.append(commit)
        return subprocess.run(
            args,
            capture_output=True,
            text=True,
            env={**env, "GITFORGE_RELEASE_DB": str(db_path)},
            timeout=120,
            check=False,
        )

    def _run_both_backends(self, db_path, **kwargs):
        results = {}
        for backend in ("cli", "python"):
            with self.subTest(backend=backend):
                results[backend] = self._run_gate(db_path, backend, **kwargs)
        self.assertEqual(
            results["cli"].returncode, results["python"].returncode,
            "readers disagree on exit status:\n"
            f"cli:    {results['cli'].stdout!r} / {results['cli'].stderr!r}\n"
            f"python: {results['python'].stdout!r} / {results['python'].stderr!r}",
        )
        self.assertEqual(
            results["cli"].stdout, results["python"].stdout,
            "readers disagree on stdout",
        )
        return results

    def _sha256(self, path):
        return hashlib.sha256(path.read_bytes()).hexdigest()

    # ─── 1. the green path passes on both readers ────────────────────────────

    def test_green_run_passes_without_touching_the_database(self):
        db_path = self._green_db()
        digest_before = self._sha256(db_path)

        results = self._run_both_backends(db_path)

        for backend, result in results.items():
            with self.subTest(backend=backend):
                self.assertEqual(result.returncode, 0,
                                 f"gate refused a green run:\n{result.stderr}")
                self.assertEqual(
                    result.stdout,
                    "release gate: PASSED — run 8f2a6c1e green, 3/3 jobs "
                    "covering the persisted definition\n",
                )
        self.assertEqual(self._sha256(db_path), digest_before,
                         "the gate wrote to the evidence database")

        self.assertIn("python3 sqlite3 fallback", results["python"].stderr)
        self.assertNotIn("fallback", results["cli"].stderr)

    def test_succeeded_run_selection_prefers_the_newest_run(self):
        # An older failed run and a newer succeeded run for the same commit:
        # the gate must grade the succeeded one, not the latest by accident.
        db_path = self._build_db(
            pipelines=[self._pipeline_row()],
            runs=[self._run_row(run_id="0aaaaaaa-5b47-4a90-9c31-d2e8f0a6b7c4",
                                status="failed"),
                  self._run_row()],
            jobs=[self._job_row(commands=DEF_COMMANDS["fmt"]),
                  self._job_row(commands=DEF_COMMANDS["clippy"]),
                  self._job_row(commands=DEF_COMMANDS["test"])],
        )
        results = self._run_both_backends(db_path)
        for result in results.values():
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("PASSED — run 8f2a6c1e green", result.stdout)

    # ─── 2. argument and database validation ─────────────────────────────────

    def test_missing_commit_argument_is_rejected(self):
        db_path = self._green_db()
        result = self._run_gate(db_path, "python", commit=None)
        self.assertEqual(result.returncode, 64)
        self.assertIn("exact 40-hex source commit is required", result.stderr)

    def test_non_hex_commit_argument_is_rejected(self):
        db_path = self._green_db()
        for bad in ("19daeea", "Z" * 40, "4a1f2b3c4d5e6f708192a3b4c5d6e7f8091a2b3x"):
            with self.subTest(commit=bad):
                result = self._run_gate(db_path, "python", commit=bad)
                self.assertEqual(result.returncode, 64)
                self.assertIn("exact 40-hex source commit is required", result.stderr)

    def test_missing_database_fails_closed(self):
        result = self._run_gate(self.workdir / "absent.db", "python")
        self.assertEqual(result.returncode, 1)
        self.assertIn("live database not found", result.stderr)

    def test_skip_gate_short_circuits_before_the_database(self):
        result = self._run_gate(
            self.workdir / "absent.db", "python",
            extra_env={"GITFORGE_RELEASE_SKIP_GATE": "1"},
        )
        self.assertEqual(result.returncode, 0)
        self.assertIn("SKIPPED via GITFORGE_RELEASE_SKIP_GATE=1", result.stderr)
        self.assertIn("NOT gated on a green run", result.stderr)

    # ─── 3. fail-closed grading gates, on both readers ───────────────────────

    def test_no_run_for_commit_fails_closed(self):
        db_path = self._build_db(
            pipelines=[self._pipeline_row()],
            runs=[self._run_row(commit="b" * 40)],
            jobs=[self._job_row(commands=DEF_COMMANDS["fmt"])],
        )
        results = self._run_both_backends(db_path)
        for result in results.values():
            self.assertEqual(result.returncode, 1)
            self.assertIn(
                f"REFUSED — no pipeline run exists on this instance for commit "
                f"{SOURCE_COMMIT}",
                result.stderr,
            )

    def test_no_succeeded_run_fails_closed_and_lists_the_runs(self):
        db_path = self._build_db(
            pipelines=[self._pipeline_row()],
            runs=[self._run_row(status="failed")],
            jobs=[self._job_row(commands=DEF_COMMANDS["fmt"], status="failed")],
        )
        results = self._run_both_backends(db_path)
        for result in results.values():
            self.assertEqual(result.returncode, 1)
            self.assertIn(f"REFUSED — no succeeded run for {SOURCE_COMMIT}",
                          result.stderr)
            self.assertIn("  8f2a6c1e=failed", result.stderr)

    def test_malformed_run_id_fails_closed(self):
        # The shape guard runs before the run id is ever interpolated into
        # the later queries.
        db_path = self._build_db(
            pipelines=[self._pipeline_row()],
            runs=[self._run_row(run_id="not-a-uuid")],
            jobs=[self._job_row(commands=DEF_COMMANDS["fmt"])],
        )
        results = self._run_both_backends(db_path)
        for result in results.values():
            self.assertEqual(result.returncode, 1)
            self.assertIn("has an unexpected shape: not-a-uuid", result.stderr)

    def test_unreadable_definition_fails_closed(self):
        # NULL config exercises NULL rendering (empty first field) through
        # the counts row on both readers; the gate must refuse coverage.
        db_path = self._build_db(
            pipelines=[self._pipeline_row(config=None)],
            runs=[self._run_row()],
            jobs=[self._job_row(commands=DEF_COMMANDS["fmt"]),
                  self._job_row(commands=DEF_COMMANDS["clippy"])],
        )
        results = self._run_both_backends(db_path)
        for result in results.values():
            self.assertEqual(result.returncode, 1)
            self.assertIn(
                "persisted pipeline definition is unreadable or has no jobs; "
                "coverage cannot be proven",
                result.stderr,
            )

    def test_definition_without_jobs_fails_closed(self):
        db_path = self._build_db(
            pipelines=[self._pipeline_row(config=json.dumps({"jobs": []}))],
            runs=[self._run_row()],
        )
        results = self._run_both_backends(db_path)
        for result in results.values():
            self.assertEqual(result.returncode, 1)
            self.assertIn("unreadable or has no jobs", result.stderr)

    def test_missing_durable_row_fails_closed(self):
        # The lazy-enqueue false green: a succeeded run whose durable rows
        # do not cover the definition, graded by the step-command lists.
        db_path = self._build_db(
            pipelines=[self._pipeline_row()],
            runs=[self._run_row()],
            jobs=[self._job_row(commands=DEF_COMMANDS["fmt"]),
                  self._job_row(commands=DEF_COMMANDS["clippy"])],
        )
        results = self._run_both_backends(db_path)
        for result in results.values():
            self.assertEqual(result.returncode, 1)
            self.assertIn("never got a durable row", result.stderr)
            self.assertIn("\ntest\n", result.stderr)

    def test_extra_durable_row_fails_closed(self):
        db_path = self._build_db(
            pipelines=[self._pipeline_row()],
            runs=[self._run_row()],
            jobs=[self._job_row(commands=DEF_COMMANDS["fmt"]),
                  self._job_row(commands=DEF_COMMANDS["clippy"]),
                  self._job_row(commands=DEF_COMMANDS["test"]),
                  self._job_row(commands=DEF_COMMANDS["fmt"])],
        )
        results = self._run_both_backends(db_path)
        for result in results.values():
            self.assertEqual(result.returncode, 1)
            self.assertIn("4 durable job rows for 3 definition jobs", result.stderr)

    def test_failing_job_fails_closed(self):
        db_path = self._build_db(
            pipelines=[self._pipeline_row()],
            runs=[self._run_row()],
            jobs=[self._job_row(commands=DEF_COMMANDS["fmt"]),
                  self._job_row(commands=DEF_COMMANDS["clippy"]),
                  self._job_row(commands=DEF_COMMANDS["test"], status="failed")],
        )
        results = self._run_both_backends(db_path)
        for result in results.values():
            self.assertEqual(result.returncode, 1)
            self.assertIn("2 of 3 jobs succeeded", result.stderr)
            self.assertIn("=failed", result.stderr)

    # ─── 4. reading while a writer holds the database ────────────────────────

    def test_gate_reads_while_a_write_transaction_is_open(self):
        # The live services hold write locks on the evidence database; the
        # readers must wait on their busy timeout instead of failing
        # instantly. WAL lets this read proceed without waiting, but an
        # instant-fail reader (no busy timeout) would still lose this race.
        db_path = self._green_db()
        connection = sqlite3.connect(db_path)
        connection.execute("PRAGMA journal_mode=wal")
        connection.execute("PRAGMA busy_timeout=15000")
        connection.execute("BEGIN IMMEDIATE")
        connection.execute(
            "UPDATE pipeline_runs SET status='running' WHERE id=?", (RUN_ID,))
        self.addCleanup(connection.close)
        self.addCleanup(connection.rollback)

        results = self._run_both_backends(db_path)
        for result in results.values():
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("PASSED", result.stdout)


if __name__ == "__main__":
    unittest.main()

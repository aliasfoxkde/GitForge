#!/usr/bin/env bash
# Contract test for the release channel's busy gate (GF-16).
#
# Why this needs a test: the gate used to count the runner's
# `gitforce-job-*` containers. The runner keeps a warm pool of containers
# that have NO job row at all, so the count was permanently non-zero and
# every cutover was deferred forever - the release channel could never
# promote anything, and it reported the condition as a normal "deferred",
# which looks like ordinary operation.
#
# The gate now reads the authoritative store: a job is in flight when its
# row is `assigned` or `running` AND its proof of life is still inside the
# fence grace window (the same liveness term the scheduler uses to decide
# whether a row is alive). The cases below pin the three properties that
# matter:
#   - the busy signal is the store, not the container names: a `docker`
#     stub on PATH fails the test if it is ever called, and the script's
#     source must not mention the container-name filter at all,
#   - a stale row cannot wedge the gate: a `running` row whose heartbeat is
#     older than the grace window does not count as in flight,
#   - an unreadable store fails closed: the cutover is refused, never
#     started on an unknown busy state.
#
# Both readers of the store are exercised: the sqlite3 CLI when the host
# has one, and the python3 fallback that the CI image (no sqlite3 CLI)
# forces.
#
# Usage: bash tests/release-auto-busy-gate-selftest.sh
set -u -o pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
busy_script="$repo_root/scripts/gitforge-release-busy"
auto_script="$repo_root/scripts/gitforge-release-auto"

[[ -f "$busy_script" ]] || { printf 'selftest: missing %s\n' "$busy_script" >&2; exit 1; }
[[ -f "$auto_script" ]] || { printf 'selftest: missing %s\n' "$auto_script" >&2; exit 1; }

tmp=$(mktemp -d)
trap 'find "$tmp" -depth -delete 2>/dev/null || true' EXIT

fail=0
check() {
    if [[ "$3" == "$2" ]]; then
        printf 'ok   %s\n' "$1"
    else
        printf 'FAIL %s: expected [%s], got [%s]\n' "$1" "$2" "$3" >&2
        fail=1
    fi
}
check_contains() {
    if printf '%s\n' "$3" | grep -qF -- "$2"; then
        printf 'ok   %s\n' "$1"
    else
        printf 'FAIL %s: expected to find: %s\n' "$1" "$2" >&2
        printf '%s\n' "$3" >&2
        fail=1
    fi
}
check_absent() {
    if printf '%s\n' "$3" | grep -qF -- "$2"; then
        printf 'FAIL %s: did not expect to find: %s\n' "$1" "$2" >&2
        printf '%s\n' "$3" >&2
        fail=1
    else
        printf 'ok   %s\n' "$1"
    fi
}

# ── The fixture builder ───────────────────────────────────────────────────────
# One database per case, with the schema the live store uses (jobs and
# runners, the columns the gate reads) and timestamps in the store's own
# RFC3339-with-offset format.
cat >"$tmp/mkdb.py" <<'PY'
import datetime
import os
import sqlite3
import sys

SCHEMA = """
CREATE TABLE runners (
    id TEXT PRIMARY KEY, name TEXT NOT NULL, runner_type TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'offline', capacity INTEGER NOT NULL DEFAULT 1,
    labels TEXT NOT NULL DEFAULT '[]', last_heartbeat TEXT,
    created_at TEXT NOT NULL, updated_at TEXT NOT NULL);
CREATE TABLE jobs (
    id TEXT PRIMARY KEY, pipeline_run_id TEXT NOT NULL, name TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending', runner_id TEXT, started_at TEXT,
    finished_at TEXT, retry_count INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL, commands TEXT NOT NULL DEFAULT '[]',
    image TEXT NOT NULL DEFAULT 'rust:latest', working_dir TEXT,
    result_json TEXT, lease_token TEXT, lease_generation INTEGER NOT NULL DEFAULT 0,
    timeout_secs INTEGER NOT NULL DEFAULT 300, heartbeat_at TEXT,
    lane TEXT NOT NULL DEFAULT 'standard', deferred_reason TEXT);
"""

# job_hb / runner_hb: seconds ago, or None for "no such timestamp".
# runner: "live" (a runner whose heartbeat is 10s old), "dead" (a runner
# whose heartbeat is an hour old), or "missing" (no runner row at all).
CASES = {
    "empty": [],
    "fresh-running": [{"id": "j1", "status": "running", "job_hb": 10, "runner": "live"}],
    "stale-running": [{"id": "j1", "status": "running", "job_hb": 3600, "runner": "dead"}],
    "assigned-live-runner": [{"id": "j1", "status": "assigned", "job_hb": None, "runner": "live"}],
    "assigned-dead-runner": [{"id": "j1", "status": "assigned", "job_hb": None, "runner": "dead"}],
    "queued": [{"id": "j1", "status": "queued", "job_hb": 10, "runner": "live"}],
    "pending": [{"id": "j1", "status": "pending", "job_hb": 10, "runner": "live"}],
    "succeeded": [{"id": "j1", "status": "succeeded", "job_hb": 10, "runner": "live"}],
    "mixed": [
        {"id": "j1", "status": "running", "job_hb": 10, "runner": "live"},
        {"id": "j2", "status": "running", "job_hb": 3600, "runner": "dead"},
        {"id": "j3", "status": "queued", "job_hb": 10, "runner": "live"},
        {"id": "j4", "status": "assigned", "job_hb": None, "runner": "live"},
    ],
    "runner-heartbeat-only": [{"id": "j1", "status": "running", "job_hb": None, "runner": "live"}],
    "orphan-running": [{"id": "j1", "status": "running", "job_hb": 10, "runner": "missing"}],
    "orphan-no-proof": [{"id": "j1", "status": "running", "job_hb": None, "runner": "missing",
                          "started": None}],
}


def stamp(moment):
    if moment is None:
        return None
    return moment.strftime("%Y-%m-%dT%H:%M:%S.%f") + "+00:00"


def main():
    db_path, case = sys.argv[1], sys.argv[2]
    if os.path.exists(db_path):
        os.unlink(db_path)
    now = datetime.datetime.now(datetime.timezone.utc)
    ago = lambda seconds: now - datetime.timedelta(seconds=seconds)  # noqa: E731
    connection = sqlite3.connect(db_path)
    connection.executescript(SCHEMA)
    for spec in CASES[case]:
        job_id = spec["id"]
        started = spec.get("started", 600)
        runner_id = None
        if spec["runner"] == "live":
            runner_id = "runner-" + job_id
            connection.execute(
                "INSERT INTO runners VALUES (?,?,?,?,?,?,?,?,?)",
                (runner_id, "runner-" + job_id, "docker", "online", 1, "[]",
                 stamp(ago(10)), stamp(ago(600)), stamp(ago(600))))
        elif spec["runner"] == "dead":
            runner_id = "runner-" + job_id
            connection.execute(
                "INSERT INTO runners VALUES (?,?,?,?,?,?,?,?,?)",
                (runner_id, "runner-" + job_id, "docker", "offline", 1, "[]",
                 stamp(ago(3600)), stamp(ago(600)), stamp(ago(600))))
        connection.execute(
            "INSERT INTO jobs (id,pipeline_run_id,name,status,runner_id,started_at,"
            "created_at,heartbeat_at) VALUES (?,?,?,?,?,?,?,?)",
            (job_id, "run-1", job_id, spec["status"], runner_id,
             stamp(ago(started)) if started is not None else None,
             stamp(ago(600)),
             stamp(ago(spec["job_hb"])) if spec["job_hb"] is not None else None))
    connection.commit()
    connection.close()


main()
PY

# ── A PATH with no sqlite3 CLI, to force the python3 fallback ────────────────
nosql="$tmp/nosql"
mkdir -p "$nosql"
for tool in bash env sed dirname tail python3 cat; do
    resolved="$(command -v "$tool" 2>/dev/null || true)"
    [[ -n "$resolved" ]] && ln -sf "$resolved" "$nosql/$tool"
done

digest() {
    python3 - "$1" <<'PY'
import hashlib
import sys
print(hashlib.sha256(open(sys.argv[1], "rb").read()).hexdigest())
PY
}

# ── Case set 1: the store reader ─────────────────────────────────────────────
declare -A EXPECTED=(
    [empty]=0
    [fresh-running]=1
    [stale-running]=0
    [assigned-live-runner]=1
    [assigned-dead-runner]=0
    [queued]=0
    [pending]=0
    [succeeded]=0
    [mixed]=2
    [runner-heartbeat-only]=1
    [orphan-running]=1
    [orphan-no-proof]=0
)

for case in empty fresh-running stale-running assigned-live-runner assigned-dead-runner \
            queued pending succeeded mixed runner-heartbeat-only orphan-running orphan-no-proof; do
    db="$tmp/db-$case.sqlite"
    python3 "$tmp/mkdb.py" "$db" "$case" || { fail=1; continue; }
    before="$(digest "$db")"

    # The reader this host would use (sqlite3 CLI when it has one).
    out="$(GITFORGE_RELEASE_DB="$db" bash "$busy_script" 2>/dev/null)"
    check "reader/$case" "${EXPECTED[$case]}" "$out"

    # The reader the CI image is forced onto (no sqlite3 CLI on PATH).
    out="$(PATH="$nosql" GITFORGE_RELEASE_DB="$db" bash "$busy_script" 2>/dev/null)"
    check "reader-fallback/$case" "${EXPECTED[$case]}" "$out"

    # The reader must not modify the store it reads.
    check "read-only/$case" "$before" "$(digest "$db")"
done

# A store that cannot be read fails closed, with a message that names the
# problem - a silently unreadable store would stall the channel forever.
out="$(GITFORGE_RELEASE_DB="$tmp/does-not-exist.sqlite" bash "$busy_script" 2>&1)"
status=$?
check "missing-store/exit" "1" "$status"
check_contains "missing-store/message" "live database not found" "$out"

# ── Case set 2: the release channel's wiring ─────────────────────────────────
# The scripts under test live in a directory of their own, because the
# release script calls its siblings by path.
wire="$tmp/wire"
mkdir -p "$wire/scripts" "$wire/repo/releases"
cp "$auto_script" "$busy_script" "$wire/scripts/"
chmod +x "$wire/scripts/gitforge-release-auto" "$wire/scripts/gitforge-release-busy"
# The verification step is not what this test is about.
printf '#!/usr/bin/env bash\nexit 0\n' >"$wire/scripts/gitforge-release-verify"
chmod +x "$wire/scripts/gitforge-release-verify"

promoted="$wire/repo/releases/gitforge-aaaaaaa-20260101"
candidate="$wire/repo/releases/gitforge-bbbbbbb-20260102"
mkdir -p "$promoted" "$candidate"
printf 'ready\n' >"$promoted/READY"
printf 'ready\n' >"$candidate/READY"
ln -sfn "$promoted" "$wire/repo/releases/gitforge-current"

stubs="$wire/bin"
mkdir -p "$stubs"
cat >"$stubs/systemctl" <<'STUB'
#!/usr/bin/env bash
# Every unit this host runs is enabled; the retired-unit cases are covered
# by tests/release-auto-restart-plan-selftest.sh.
for arg in "$@"; do
    if [[ "$arg" == "LoadState" ]]; then
        printf 'enabled\n'
        exit 0
    fi
done
exit 0
STUB
cat >"$stubs/docker" <<'STUB'
#!/usr/bin/env bash
# The busy gate must never consult the container names.
printf 'docker was called\n' >>"${GF_DOCKER_CALLED:?}"
exit 1
STUB
chmod +x "$stubs/systemctl" "$stubs/docker"

run_auto() {
    local db="$1"
    PATH="$stubs:$PATH" \
    GF_DOCKER_CALLED="$wire/docker-called" \
    GITFORGE_AUTO_REPO="$wire/repo" \
    GITFORGE_RELEASE_DB="$db" \
        bash "$wire/scripts/gitforge-release-auto" 2>&1
}

db_busy="$tmp/db-busy.sqlite"
python3 "$tmp/mkdb.py" "$db_busy" fresh-running
db_idle="$tmp/db-idle.sqlite"
python3 "$tmp/mkdb.py" "$db_idle" empty

# A job in flight defers the cutover, and the channel says so.
out="$(run_auto "$db_busy")"
status=$?
check "busy/exit" "0" "$status"
check_contains "busy/deferred" "deferred: 1 job(s) in flight" "$out"
check_contains "busy/inflight-reported" "inflight=1" "$out"
check "busy/no-docker" "" "$(cat "$wire/docker-called" 2>/dev/null || true)"

# An idle store lets the pass continue to the dry-run report.
out="$(run_auto "$db_idle")"
status=$?
check "idle/exit" "0" "$status"
check_contains "idle/dry-run" "dry-run: would promote gitforge-bbbbbbb-20260102" "$out"
check "idle/no-docker" "" "$(cat "$wire/docker-called" 2>/dev/null || true)"

# An unreadable store refuses the cutover instead of guessing.
out="$(run_auto "$tmp/does-not-exist.sqlite")"
status=$?
check "unreadable/exit" "1" "$status"
check_contains "unreadable/message" "cannot read the in-flight job count" "$out"
check_absent "unreadable/no-promotion" "would promote" "$out"
check "unreadable/link-untouched" "gitforge-aaaaaaa-20260101" \
    "$(basename "$(readlink -- "$wire/repo/releases/gitforge-current")")"

# The source must not go back to counting containers.
check_absent "source/no-container-filter" "gitforce-job-" "$(cat "$auto_script")"

# - Case set 3: the operator cutover's drain gate ----------------------------
# The sibling cutover script drains on the same signal. It must not count a
# row left behind by a dead runner as work in flight (that would wait for
# ever), and it must wait for a job that really is running.
cutover="$repo_root/scripts/gitforge-remote-cutover"
if [[ -f "$cutover" ]]; then
    cp "$cutover" "$wire/scripts/"
    chmod +x "$wire/scripts/gitforge-remote-cutover"
    bundle="$wire/bundle"
    mkdir -p "$bundle/bin"
    install -m 0755 /dev/null "$bundle/bin/ci"
    ln -sfn "$promoted" "$wire/link"

    db_stale="$tmp/db-stale.sqlite"
    python3 "$tmp/mkdb.py" "$db_stale" stale-running

    run_cutover() {
        PATH="$stubs:$PATH" timeout 30 bash "$wire/scripts/gitforge-remote-cutover" \
            --units user --db "$1" --bundle "$bundle" --link "$wire/link" 2>&1
    }

    # An idle store drains.
    out="$(run_cutover "$db_idle")"
    check_contains "cutover/idle-drains" "drained; cutting over" "$out"

    # A store whose only running row is stale drains too - the row is dead
    # work, and the scheduler fences it on its own.
    out="$(run_cutover "$db_stale")"
    check_contains "cutover/stale-drains" "drained; cutting over" "$out"

    # A job that really is running makes the cutover wait.
    out="$(PATH="$stubs:$PATH" timeout 8 bash "$wire/scripts/gitforge-remote-cutover" \
        --units user --db "$db_busy" --bundle "$bundle" --link "$wire/link" 2>&1)"
    check_contains "cutover/busy-waits" "in-flight=1; waiting" "$out"
else
    printf 'ok   cutover drain cases (script absent)\n'
fi

if ((fail)); then
    printf 'selftest: FAILED\n' >&2
    exit 1
fi
printf 'selftest: OK\n'

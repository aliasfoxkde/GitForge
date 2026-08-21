# CI Audit: gitforge-ci.yml & rust-ci.yml

**Auditor:** Hermes (zai/glm-5-turbo)
**Date:** 2026-08-21
**Scope:** `.github/workflows/gitforge-ci.yml`, `.github/workflows/rust-ci.yml`

---

## Findings Summary

| # | Severity | Workflow | Finding |
|---|----------|----------|---------|
| 1 | **HIGH** | gitforge-ci.yml | Poll loop silently swallows transient curl failures via `--fail-with-body` under `set -euo pipefail` -- no retry, no grace period |  |
| 2 | MEDIUM | gitforge-ci.yml | No concurrency control; overlapping pushes enqueue duplicate builds with no dedup |  |
| 3 | MEDIUM | rust-ci.yml | `test-race` job title says "Race Detection" but uses `--test-threads=1` with a static-linking RUSTFLAG -- no actual TSan/loom race detection is performed |  |
| 4 | LOW | gitforge-ci.yml | `GITFORGE_POLL_INTERVAL_SECONDS` default of 15s is not validated against `GITFORGE_POLL_TIMEOUT_SECONDS`; a user could set interval >= timeout and the loop never polls |  |
| 5 | LOW | rust-ci.yml | `rust-ci.yml` path filter on line 12 only watches itself, not `gitforge-ci.yml`; changes to the GitForge workflow trigger no Rust checks |  |

---

## Prioritized Reliability Finding: #1 -- Poll Loop Has No Retry Tolerance

### Evidence

**File:** `.github/workflows/gitforge-ci.yml`, lines 105-109

```yaml
RESPONSE=$(curl --fail-with-body --silent --show-error \
  --max-time 30 \
  -H "Authorization: Bearer $GITFORGE_API_TOKEN" \
  -H "Accept: application/json" \
  "$STATUS_URL")
```

The poll loop (lines 104-130) runs inside `set -euo pipefail` (line 93). When `curl --fail-with-body` encounters any HTTP 4xx/5xx or a network timeout, it returns a non-zero exit code. Because of `set -e`, the script **immediately terminates** the entire poll-build job -- there is zero retry logic for transient failures.

### Why It Matters

- GitForge's own API or the network path to it will experience ephemeral 502s, DNS hiccups, or TCP resets. A single blip kills the entire CI run for that push/PR.
- The GitHub Actions runner is billed for the full poll-build job duration even though it exited after one failed request.
- The `report-status` job (line 136, `if: always()`) will correctly flag failure, but developers get no signal whether the failure was a real build problem or a transient network glitch, leading to wasted re-runs and lost trust in the pipeline.

### Concrete Failure Scenario

1. A PR is pushed. `enqueue-build` succeeds and returns `job_id=abc123`.
2. `poll-build` starts polling. On the 3rd iteration (45s in), GitForge's API returns `502 Bad Gateway` due to a rolling restart.
3. `curl --fail-with-body` exits with code 22. `set -e` catches it. Poll-build terminates immediately.
4. `report-status` prints "GitForge CI did not complete successfully" and exits 1.
5. The PR goes red. The developer re-runs the workflow. If the blip lasts 2 minutes, they retry 3-4 times, each burning a runner and minutes of wall-clock time.

### Recommended Fix

Wrap the curl call in a small retry loop with exponential backoff (e.g., 3 attempts, 1s/2s/4s delays) before letting `set -e` propagate the failure. Example sketch:

```bash
attempts=0
max_attempts=3
while [ $attempts -lt $max_attempts ]; do
  if RESPONSE=$(curl --fail-with-body --silent --show-error \
       --max-time 30 \
       -H "Authorization: Bearer $GITFORGE_API_TOKEN" \
       -H "Accept: application/json" \
       "$STATUS_URL" 2>&1); then
    break
  fi
  attempts=$((attempts + 1))
  if [ $attempts -lt $max_attempts ]; then
    sleep $((2 ** attempts))
  fi
done
if [ $attempts -eq $max_attempts ]; then
  echo "Poll request failed after $max_attempts attempts" >&2
  exit 1
fi
```

This gives the poll loop resilience against ephemeral network/API issues while still failing fast on persistent outages.

---

## Additional Notes

- Both workflows trigger on identical events (push to main/stable, tags, PRs with Rust/workflow paths). The `gitforge-ci.yml` workflow is clearly intended to *replace* direct runner execution by delegating to GitForge's build queue, while `rust-ci.yml` runs natively on GitHub Actions. Having both active on the same triggers means PRs run CI twice unless one is disabled -- this should be confirmed as intentional.
- The `gitforge-ci.yml` workflow does not have a `concurrency` group (unlike `rust-ci.yml` line 14-16), so force-pushes to a PR branch will enqueue a new GitForge job *without cancelling* the previous one, risking wasted build capacity.

# Production validation campaign — 2026-11-02 (202 instance)

Scope: optimization-plan step 7 ("production validation: stress testing,
failure recovery, deployment and rollback") against the live Fedora 202
instance (audit.db at `~/work/gitforge-audit-20260822/audit.db`, operator
JWT refreshed via fedora `operator-credentials.env`).

## 1. Stress — concurrent trigger burst (PASSED)

Fixture: fresh repo `stress-canary-20261102`
(`30821259-c610-4706-bf55-96638e173ae3`), 2-job pipeline (ping-a → ping-b,
busybox). Smoke run first (`db091d0b`, succeeded, needs-order held).

- Same-commit burst: 10 concurrent `POST /api/pipelines/{id}/runs` for one
  commit → 9×202-null + 1×run id. All 10 trigger requests settled
  `completed`; the 202-null client responses settled to run `ff1de511`
  (durable-202-null contract). Zero lost, zero duplicated.
- Distinct-commit burst: 10 commits pushed, 10 concurrent triggers →
  **10/10 distinct runs, all `succeeded`, 20/20 jobs succeeded** (DB query
  `q_burst4.py` output recorded in transcript).
- Control plane under load: 30 sequential `/api/repos` GETs during/after —
  all 200, p95 = **3 ms**, max 4 ms.

## 2. Failure recovery (PASSED)

Fixture: repo `fail-canary-20261102`, pipeline with `will-fail` (exit 7)
→ `dependent` (needs: [will-fail]).

Run `ab18e9ff-1ecc-4c27-a415-dbcae11a09b1`: run graded **`failed`**,
`will-fail` = `failed`, `dependent` = `cancelled` — honest grading and
correct needs-graph cascade. Subsequent triggers on the instance continued
to execute and grade normally (post-failure liveness).

## 3. Findings (defects to ledger)

**G1 — `POST /api/pipelines/{id}/runs` drops the pipeline id in the
delegation.** `trigger_pipeline_run` (crates/gitforge-api/src/routes/ci.rs
~426) calls `client.trigger(pipeline.repo_id, &revision, None, &commit)`
— no pipeline id. Downstream, the run binds to the repo's currently-active
pipeline. Two different pipelines on the same (repo, commit) cannot both
run: the second trigger dedups onto the first's trigger request
(`create_or_existing` keys on (repo_id, new_hash) only,
crates/gitforge-db/src/queries.rs:3367) and/or executes the wrong config.
Reproduced twice live (runs `e80ed74f`, `01f104df` executed ping-a/ping-b
when fail-canary was requested).

**G2 — API-uploaded pipeline config is bypassed when the revision carries
no committed `.gitforge.yml` (REVISED — not row duplication).** The
per-run new `pipelines` rows on 202 are `create_default_pipeline`
persisting the fallback (services/ci/src/main.rs:2382-2434): the committed
definition at the pushed revision is authoritative, and with none
committed the consumer falls back to cache → persisted-any → default. My
canary repo had no `.gitforge.yml`, so the API-uploaded config never
governed. User-visible consequence: `POST /api/pipelines/{id}/runs` on a
repo without a committed definition runs the *default* pipeline, not the
uploaded config. G1 (pipeline id dropped in the delegation) is the
upstream cause; fixing G1 to pass the pipeline definition (or its id)
through the trigger is the complete fix.

**G3 — CI-side `/metrics` (Gate-1 observability route,
services/ci/src/main.rs:244) is not deployed on 202**: 404 with the
operator scheduler token, 401 anonymous, against port 42781. The API-side
`/metrics` counters (jobs_total etc.) read 0 because they live in the API
process which never sees job events. Checklist item "resource and queue
metrics available" is NOT satisfied in production.

## 4. Deploy + rollback (dry-run evidence COMPLETE — no traffic cut)

- Current channel: `gitforge-current` → `gitforge-69245e2b-20261029`
  (source_commit 69245e2b, 2026-10-29).
- Clean worktree `/nas/Temp/tmp/gf-release-ce06089e` at `ce06089e`;
  release build finished in 9m47s (target: `/nas/Temp/tmp/gf-release-target/`,
  binaries hashed at 18:01–18:03 local).
- Release gate: **PASSED** — run `820c6a8b` green, 8/8 jobs satisfied
  covering the persisted definition (DB snapshot of 202 audit.db copied to
  `/var/tmp/audit-snapshot-20261102.db`, 5.4G, fedora /tmp same).
- Bundle `gitforge-ce06089e-20261102` assembled (12 manifest files) and
  verify-passed locally AND on 202 after scp to
  `/home/mkinney/work/validation/releases/`.
- Promote dry-run on 202 (real symlink, no flip): current=
  `gitforge-69245e2b-20261029`, next=`gitforge-ce06089e-20261102`.
- Drain-gated cutover dry-run on 202 (`scripts/gitforge-remote-cutover
  --units user --dry-run`): drain gate on gitforge-ci/gitforge-runner,
  stateless gitforge-api/gitforge-git-server, no units touched.
- **Awaiting operator approval for `--apply`** (promote + cutover). Rollback
  path is the promote script's previous pointer: `gitforge-69245e2b-20261029`.

## Evidence anchors

- Trigger-response artifacts: `/var/tmp/burst-*.json`, `/var/tmp/burst2-*.json`
- Run/trigger verification queries: `/var/tmp/q_burst*.py`, `/var/tmp/q_fail*.py`
- Release-gate evidence for the chain: run `cdd70c1c` (push) and `820c6a8b`
  (PR-enqueued), both succeeded for exact SHA ce06089e; GitHub PR #365
  25/25 checks green.

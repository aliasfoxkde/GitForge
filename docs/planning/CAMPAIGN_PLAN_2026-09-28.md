# GitForge Quality & Durability Campaign — 2026-09-28

Status: Active
Branch: `r6-platform-durability` (PR #236)
Supersedes nothing; complements [MASTER_PLAN_2026-09-20.md](MASTER_PLAN_2026-09-20.md)
(the findings ledger) and [IMPROVEMENTS.md](IMPROVEMENTS.md) (the standing
backlog). This document is the phase plan for the 2026-09-28 campaign.

## Honest assessment (start of campaign)

Verified at campaign start, on the pre-merge tree:

- **Merge debt**: the branch (PR #236, pipeline create/run/delete API) had
  drifted 3 own-commits behind `main`; #237–#239 (F33/F34, F36, deploy
  ledger) landed first. Merged cleanly as e2dcb48a; the aegis baseline
  needed no regeneration (only this branch had touched it since its
  7f62e0e8 re-anchor).
- **Repo hygiene**: four stripped release binaries (`api`, `ci`,
  `git-server`, `runner`, ~42 MB) and a 0-byte stray `gitforge.db` sat
  untracked in the repo root — release-bundling leftovers from 2026-09-25.
  Removed; nothing held them open (lsof-verified).
- **Ledgered defects**: F21/F22/F23/F24/F29/F32/F33/F34/F36 verified
  RESOLVED in code with pinned tests. The one open engineering item was
  F31's residual: `reconcile_expired` only reaps `running` rows with
  `started_at`, so a job row carrying terminal evidence
  (`finished_at` + `result_json`) with a lost status write stranded its
  run non-terminal forever (the bd5c8664 shape, repaired by hand twice).
- **Docs rot**: the project `CLAUDE.md` described a Go repository — gofmt,
  `go test -p 1`, GoReleaser, sentinel errors, per-layer 95/90/85 coverage
  targets — none of which apply to this Rust workspace. Any agent or
  contributor following it would run the wrong toolchain on every gate.
- **GitHub check debt**: PR #236's `Test` and `Coverage` GitHub checks are
  red. Per the standing directive these are not authoritative (billing-
  blocked account, NAS-contended runners — the same run's Clippy, Build,
  integration, e2e, and all six Security jobs are green); the branch is
  validated through this repo's own GitForge pipeline instead.
- **Coverage**: workspace 86.22% lines / 87.06% regions (2026-09-20
  in-sandbox calibration; CI gate 82/84). The largest non-Docker-gated
  gaps are api route handlers (`routes/ci.rs` 62.42%, `routes/webhook.rs`
  54.91%, `routes/runners.rs` 61.54%).
- **Not applicable, deliberately**: WGCA 2.1 AAA and browser e2e — GitForge
  has no web frontend (CLI + Git transport + HTTP API). Recorded here so
  the goal is not silently dropped but consciously dispositioned.

## Phase 1 — Reconcile and repair (this branch, in flight)

1. ✅ Merge `main` into `r6-platform-durability` (e2dcb48a).
2. ✅ Remove stray root artifacts (4 binaries + empty db).
3. ✅ Fix the F31 residual: `JobQueries::reconcile_evidence_rows` grades
   any pre-terminal row carrying terminal evidence from its own receipt
   (recognized verdict wins; unparseable evidence fails closed to
   `failed`; evidence columns never rewritten). Wired into the CI
   watchdog tick and scheduler recovery. Pinned by
   `test_reconcile_evidence_rows_grades_stranded_rows`.
4. ✅ Verify F21/F23 durable-write fixes survived the merge
   (`persist_with_retry` at assigner.rs:37; insert/status/definition/
   lease-start/completion all wrapped; unit probe test at assigner.rs).
5. ⏳ Full gates: fmt → clippy `-D warnings` → `cargo test --workspace
   -- --test-threads=2` → aegis.
6. ✅ Fix F37 (found validating this branch, live repro run 666b3fa8):
   the watchdog reaped a hung `test` job but the run never finalized —
   the failure→cancel cascade existed only in engine memory, the
   rebuilt engine never fired it, and the periodic reconciler deferred
   to the live engine forever (custody deadlock). The doom cascade is
   now durable (`cancel_doomed_rows` over the persisted definition's
   `needs`), registry custody no longer shields all-terminal runs, and
   the periodic loop reclaims pass-finalized workspaces. See F37 in
   the master-plan ledger.
7. ⏳ Push; validate through the self-hosted GitForge pipeline
   (`.gitforge.yml`: fmt → clippy → test → coverage on `dsc-ci-rust:7`).
8. ⏳ Merge PR #236 (GitHub main requires the ruleset-bypass procedure:
   temporary bypass grant + `gh pr merge --admin`).

## Phase 2 — Documentation currency

1. ✅ Rewrite `CLAUDE.md` for the real Rust workspace: Makefile targets,
   the `.gitforge.yml` chain as the authoritative gate, active workflows,
   durable-write and BEGIN IMMEDIATE conventions, the calibrated 82/84
   coverage gate, the ledger-append rule.
2. Mark `docs/HANDOFF.md` as historical (it still headlines a
   long-fixed "Runner Not Executing Jobs" critical issue and
   August-era evidence) so no reader mistakes it for live state.
3. Sweep remaining `docs/*.md` top-level files for Go-era commands and
   wrong toolchains (HOOKS.md, BRANCH_STRATEGY.md, PLAN.md and friends
   are the oldest by last-touch date).
4. Record this campaign's outcomes in the master-plan ledger
   (F31 residual entry updated) and CHANGELOG_RECENT.md.

## Phase 3 — Coverage where it pays (next sessions)

Ordered by measured gap, each item independent:

1. `gitforge-api` route handlers: `webhook.rs` (54.91%), `runners.rs`
   (61.54%), `ci.rs` (62.42%) — the API is the product surface; the
   spawned-binary harness pattern from `services/ci/tests/
   ci_trigger_flow.rs` is the proven template.
2. `services/git-server/main.rs` (91.77% → the remaining edge paths).
3. `gitforge-runner/executor` stays Docker-gated: keep `#[include-
   ignored]` host sweeps as the measurement, CI floor stays honest.

Exit criterion: workspace ≥ 88% lines in-sandbox without weakening the
gate's calibration honesty (raise the 82/84 gate only with a re-measure).

## Phase 4 — Supply chain and lint strictness

1. cargo-vet: incremental `suggest` → `import` → `prune` cycle against
   the six pinned peer registries; inspect+certify only diffs a human
   actually reviewed (mass-certification stays rejected as dishonest —
   2026-09-08 decision, reaffirmed).
2. Aegis: re-scan after every structural refactor; regenerate the
   baseline only after triage, per the 748fb89 precedent.
3. Clippy stays at `-D warnings`; no lints-downgrade commits.

## Phase 5 — Release

1. ✅ Cut after Phase 1 landed: bundle with the full 40-char
   `GITFORGE_SOURCE_COMMIT`, `promote --apply`, verified via
   `systemctl show -p ExecStart`, honoring the contended drop-in pin
   (promoted first; the pin now converges everyone onto the symlink).
2. ✅ GitHub release/tag followed the GitForge release; GitForge-first
   push order per the standing directive.
3. ✅ Drain gate honored: waited out the one live tenant container;
   the pending-only runs queued behind the scheduler are not executing
   work and re-dispatch under the new binary. Executed record below.

## Deferred / operator-owned (not this campaign)

- F35 open question: ~6.5G deleted-but-open tmpfs (likely jellyfin
  transcode) — a service restart reclaims it; operator call.
- cargo-vet's RustCrypto/russh audit desert: no peer registry carries
  audits yet; revisit after upstream publishes.
- 99% workspace coverage: not honestly attainable — the executor is
  Docker-gated and `main.rs` binaries need live infra; the documented
  ceiling with current harnesses is ~88-90% workspace-wide. Chasing the
  number by de-testing infrastructure paths would be dishonest metrics.

## Live observations during validation (2026-09-29, host at load 45–78)

Observed while this branch's own pipeline ran on the saturated instance:

1. **Run 666b3fa8 stayed non-terminal after its `test` job was reaped** —
   initially read as watchdog starvation, but the reap HAD succeeded
   (`timed_out`, correct evidence); the run stayed `running` because of
   F37 (custody deadlock between the periodic reconciler and a
   never-converging engine), which is now fixed. The lesson stands: read
   the job rows before blaming the write plane.
2. **Cancellation is also a durable write**: `POST /jobs/{id}/cancel`
   for a superseded job returned 500 `database_error` at load 60+ —
   control-plane actions fail closed under saturation.
3. **Read-plane starvation**: direct sqlite reads of the live DB time
   out even with busy timeouts; gateway queries exceeded 35–80s at load
   60–78; the scheduler API is the only plane that stayed responsive.
4. **The api→ci trigger is latency-coupled to run creation** (observed
   post-deploy, 2026-09-29): `POST /api/pipelines/{id}/runs` returned
   502 at exactly 10.0s — the api's reqwest timeout in
   `CiTriggerClient` — because ci's `/pipelines/trigger` handler is
   synchronous end-to-end: it publishes to the event bus and blocks on
   a oneshot until the full durable run creation (config load + run +
   job + definition rows in one `BEGIN IMMEDIATE`) commits. Under a
   dispatch storm that transaction waits on the write lock. Same
   fail-closed-under-saturation class as item 2; candidate fix (not
   scheduled): make the trigger handler enqueue-only (outbox insert)
   and let the api poll the returned run id.

Item 1 was a code defect (F37, fixed). Items 2–4 are not new defects;
they are the documented cost of running the platform on a shared host at
4x oversubscription. Candidate mitigations (not scheduled): per-service
CPUQuota already exists (400%); a queue-depth-aware admission gate for
new runs, an enqueue-only trigger handler, and a read-replica or WAL
checkpoint tuning for the gateway are the next levers if this recurs.

## Phase 5 — Release (executed 2026-09-29)

1. ✅ Validation re-run for the release commit: the merge-commit run
   2d7d6c3d died at exactly 60m07s against the `test` fence while the
   content-identical tree passed the same job in 9m25s (environmental
   deadline-miss under load 77–169, not a code failure). The release
   commit 1b9a67caf7b26a098cc557614acf97ab4836ac1c validated clean as
   run 240be08c — fmt 21s, clippy 2m50s, test 9m30s, coverage 12m37s.
2. ✅ Gate → bundle (`gitforge-1b9a67ca-20260929`) → `promote --apply`
   (first `releases/gitforge-current` link) → drop-in ExecStart
   repointed to the symlink → single drain (one live tenant container
   waited out; the other 9 "running" runs were pending-only, queued
   behind the exclusive-class scheduler) → restart of all four units.
3. ✅ Post-deploy verification: `systemctl show -p ExecStart` resolves
   to `releases/gitforge-current/bin/%i` on all units; health green on
   :42780/:42781/:42782; F37 self-heal observed — startup
   reconciliation graded run 666b3fa8 `failed` and durably cancelled
   its doomed `coverage` row (fmt/clippy succeeded, test timed_out,
   coverage cancelled), and four other wedge-shaped backlog runs
   graded `failed` the same way.
4. ✅ GitHub sync: PR #244 merged via the ruleset bypass (restored to
   `[]` after), tag `v0.6.11` pushed GitForge-first, release published.
   Mirror synced: local, GitForge, and GitHub main all at c9a0c096.
5. Note: the changelog's 0.6.11 section intentionally omits the
   "Deployed release …" line (the 0.6.6 style) — the annotated tag
   message carries the deploy facts, matching the 0.6.10 precedent, so
   the validated release commit did not need a post-deploy doc delta.

## Phase 5 — Release v0.6.12 (executed 2026-10-01)

1. ✅ Validation, twice per the exact-SHA rule: run `531e1f23` green
   (4/4, 46 min) on the release tree `383f94b4`, then after the PR
   merge run `5d55c6d9` green (4/4, ~40 min) on the exact merge commit
   `74f2226d` — the tree is identical but the gate requires the run on
   the tagged SHA, matching the v0.6.11 precedent.
2. ✅ PR #246 merged via `gh pr merge --merge --admin` under a
   temporary ruleset bypass. Procedure notes: rulesets update by full
   -body `PUT` (a `PATCH` 404s); backup taken first, `bypass_actors`
   restored to `[]` immediately after the merge — the window was open
   only for the merge call.
3. ✅ Gate → bundle: `gitforge-release-gate` passed mechanically
   ("run 5d55c6d9 green, 4/4 jobs covering the persisted definition"),
   bundle `gitforge-74f2226d-20261001` assembled, `promote --apply`
   switched `releases/gitforge-current` atomically (previous:
   `gitforge-1b9a67ca-20260929`).
4. ⚠️ Drain: the co-tenant queue refilled continuously (2→4→3 live
   containers over 30 min, never zero), so the master plan's documented
   "accept the recovery" path was taken for ci+runner (api/git-server
   are stateless and restarted any time). Recovery behaved better than
   fencing: in-flight `build` completed and graded `succeeded` through
   its persisted receipt; `test-py310` was requeued and re-delivered
   (fresh assignment at 20:12:51Z) — no job was lost or falsely failed.
5. ✅ Post-deploy verification: all four units run from
   `gitforge-74f2226d-20261001/bin/*`; health 200 on :42780/:42781/
   :42782; `overall: healthy`. Gotcha recorded: `gitforge-status`
   defaults `GITFORGE_RELEASE_ROOT` to `/home/gitforge/work/
   gitforge-current` and reports false drift/degraded on this instance
   unless pointed at `/nas/Temp/repos/GitForge/releases/gitforge-current`.
6. ✅ GitHub sync: tag `v0.6.12` pushed GitForge-first then origin,
   release published. Local main, origin/main, and gitforge-ci main all
   at `74f2226d`.
7. Incident records from this cycle: a lock-storm trigger produced a
   run-without-jobs ghost (run row committed, planned-jobs write lost);
   the jobless-run reconciler graded it `cancelled` at horizon+2 min,
   validating the 1h `RECONCILE_EMPTY_RUN_HORIZON_SECS` design. Post-
   storm trigger delivery can lag minutes (dispatcher marks `delivered`
   on ci's honest 202 before the consumer creates the run) — re-query
   before diagnosing.

## Phase 6 — Release v0.6.13 (executed 2026-10-02)

1. ✅ Content: owner-prefixed repo-create fix (`43833379`, the kubix
   activation blocker — API rejects `/` in repo names, CLI sends the
   bare name) plus the co-tenant's pipeline-list active-filter
   (`752ea428`, absorbed rather than racing it). Release SHA
   `929bb967` converged all three mirrors (local main, origin/main,
   gitforge-ci main) via PRs #249/#250 — a co-tenant had promoted a
   bundle from their unmerged branch mid-cycle; absorbing the branch
   and revalidating on the merge beat pinning to unmerged code.
2. ✅ Validation on the exact release SHA: run `ed9f52d6` green (4/4:
   fmt, clippy, test, coverage). Gate passed mechanically, bundle
   `gitforge-929bb967-20261003` assembled and verified (11 files).
3. ✅ Promote: `promote --apply` switched `releases/gitforge-current`
   atomically (previous: `gitforge-752ea428-20261002` — the co-tenant
   branch release this cut supersedes). All four units restarted via
   `sudo -n systemctl restart gitforge@{api,git-server,ci,runner}`.
4. ✅ Zero-cost cutover: DB check confirmed no job was in flight at
   the kill moment — the drain window plus queue timing meant no
   requeue, no casualty. Post-restart dispatch verified (26 jobs
   `succeeded` in the first 10 minutes; kubix-ci's latest run
   `succeeded`).
5. ✅ Post-deploy: health 200 on :42780/:42781/:42782; all four
   binaries running from the new bundle; `gitforge-status` healthy on
   release/runners (drift-free with `GITFORGE_RELEASE_ROOT` pointed at
   this instance, per the v0.6.12 gotcha). Residual `overall:
   degraded` is host /tmp pressure (co-tenant gate workspace at
   ~14G/16G tmpfs), not the release.
6. ✅ GitHub sync: tag `v0.6.13` pushed GitForge-first then origin;
   release published with changelog notes.
7. Noted, not fixed here: 4 consecutive ~11s `infrastructure_failure`
   jobs on a co-tenant pipeline (container-start failures coinciding
   with /tmp at 100%) — environment, not cutover; owner-side retry
   applies. CLI-token TTL (24h, no refresh) and job_log_chunks
   retention remain open product follow-ups from the same audit.

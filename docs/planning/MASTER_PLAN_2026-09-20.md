# GitForge Master Plan — 2026-09-20

Status: Active
Supersedes: COMPREHENSIVE_EXECUTION_PLAN_2026-08-28.md (completes it; do not
delete — it records how the current state was reached)
Inputs: baseline audit (2026-09-20), coverage campaign, strict-lint/code-smell
audit (`docs/audits/CODE_SMELLS_2026-09-20.md`), documentation audit, and a
**live instance validation** against the production-local GitForge deployment
(2026-09-20/21) — trigger → run → job DAG → queue → execution.

---

## 1. Honest assessment of where the project stands

### What is genuinely strong

- **Test mass**: 1 517 workspace tests, all green under
  `cargo test --workspace` with `-p 1`; the standalone `tests/integration`
  harness (its own workspace) is at 94/94 after its 2026-09-20 repair
  (PR #183).
- **Lint strictness**: clippy `-D warnings` workspace-wide plus four pedantic
  lints promoted to `deny` in `[workspace.lints.clippy]` with all 19 members
  opted in. The remaining pedantic backlog is inventoried, not hidden
  (`docs/audits/CODE_SMELLS_2026-09-20.md`).
- **Coverage**: 86.22% lines / 87.06% regions against a 79.9% CI floor;
  per-crate weak spots were attacked directly (git-server 91.8% lines,
  review 99.4%, storage/job_logs 92.9%).
- **Security posture**: cargo-vet + cargo-audit (with an honest, justified
  rsa/russh exemption) + Aegis pattern scanning with a triaged baseline that
  fails the build on new findings. The Aegis sweep found zero true secrets.
- **Documentation**: API.md, RUNBOOK, ARCHITECTURE, CONTRIBUTING, README all
  rewritten against the real code in 2026-09-20; wire formats are pinned by
  contract tests, not prose.
- **The platform runs real work**: the live instance carries five other
  projects' CI (dsc, VIVERE, gods-eye-view, the-craft-coven-website, BigData)
  with 1 422 succeeded jobs in the durability ledger.

### What is genuinely weak (evidence from live validation)

The deployed instance diverges from `main` in ways that matter, and several
multi-tenant behaviors have no owner yet. Each finding below is actionable
and carries its evidence. None of these are hypothetical: all were observed
against the live deployment on 2026-09-20/21.

### Standards mappings (honest, not aspirational)

| Standard | Verdict |
|---|---|
| WGCA / WCAG 2.1 AAA | **N/A** — GitForge ships no web frontend. The dashboard route is a JSON endpoint, not UI. Template-parts are scaffolding for downstream projects. Revisit only if a real UI lands. |
| Strictest linting | **Achieved for the default+pedantic tier**; `restriction`-tier lints (e.g. `unwrap_used`) are deliberately not enabled globally because the codebase's accepted-unwrap classes (documented in CODE_SMELLS) would flood it. Crate-by-crate `unwrap_used` denial in pure-library crates is a Phase 3 item. |
| 99% coverage | **Not honest as a blanket target.** The remaining uncovered mass is Docker-gated execution paths and process-bootstrap code that cannot be meaningfully unit-tested. The plan targets ≥90% lines per non-Docker-gated crate and keeps the CI floor at 79.9% until Phase 3 raises it deliberately. |

---

## 2. Findings ledger (live validation, 2026-09-20/21)

Each finding: what was observed, why it matters, where the fix lands.

- **F1 — Deployment drift.** The running service binaries were built
  2026-09-16/17 while `main` had advanced through 2026-09-20 (three merged
  PRs). The stale gateway 404'd `GET /api/pipelines/{id}`, served a pipeline
  listing that duplicates rows, and made the CLI's `pipeline --list`
  misleading. **Fix lands in**: Phase 0 (refresh procedure below) + a
  build-stamp line in `scripts/gitforge-status` output so drift is visible
  without digging through `/proc`.
- **F2 — Runner registry pollution.** The runners table held **29 rows for
  the single runner name `swarmone-docker`**; every runner start inserts a
  new row instead of adopting its durable identity. 28 rows were stale (one
  had a heartbeat <2 min old). Retirement endpoints exist
  (`DELETE /api/runners/{id}` refuses busy runners) but nothing reclaims
  duplicates automatically. **Fix lands in**: Phase 1 (runner upsert-by-name
  on registration) + Phase 0 ops (retire the 28 stale rows once).
- **F3 — Online status lies.** 20 of the 29 runner rows reported `status =
  'online'` with heartbeats hours to weeks old; the status column is only
  ever written by the runner itself. Consumers (scheduler placement, the
  CLI listing) cannot trust it. **Fix lands in**: Phase 1 — derive display/
  placement status from `last_heartbeat` age (single source of truth), or a
  janitor that flips rows offline on heartbeat timeout.
- **F4 — Pipeline version-per-push growth.** The `pipelines` table holds
  **226 rows for dsc, 69 for VIVERE** — one new version row per push, old
  versions left at `active = 0`, no pruning, no `UNIQUE` constraint. This is
  the active `codex/push-pipeline-version-retire-20260911` feature area;
  do not collide with it. **Fix lands in**: Phase 2 — retention policy
  (keep N versions), a uniqueness boundary, and a decision on whether
  inactive versions should remain queryable.
- **F5 — CLI auth UX.** `~/.config/gitforge/config.toml` carries no token;
  every command 401s until the user discovers `--token`. Login itself works
  (`POST /auth/login` — note: mounted at the router root, **not** under
  `/api`; API.md documents this correctly). **Fix lands in**: Phase 0/1 —
  persist the token on `auth --login` (chmod 600), print a hint on 401 that
  names the command.
- **F6 — JWT rotation is session-destructive.** All issued tokens 401 after
  any service restart that picks up a new `JWT_SECRET` (observed: restart at
  04:14 on 2026-09-20 invalidated tokens minted hours earlier). **Fix lands
  in**: Phase 4 — deployment contract: `JWT_SECRET` must come from a
  persistent secret file/env the operator owns; document that rotation
  invalidates sessions (or add a key-id ring later).
- **F7 — Trigger/run correlation race.** `POST /pipelines/trigger` answered
  `{"status":"queued","pipeline_run_id":null}` while the run row materialized
  within seconds. The waiter window is 15 s in current code (comment
  documents an earlier 3 s that was too short); the deployed build predates
  it. **Action**: re-test on the refreshed deployment (Phase 0 exit
  criterion); if it still races, instrument the waiter resolution path.
- **F8 — `.env` is tracked in git.** A secret-shaped file (placeholders
  today, real values for the live stack) has no business in the index.
  Untracking naively would delete operators' local `.env` on pull, and
  compose depends on it. **Fix lands in**: Phase 4 — move to
  `.env.example` (exists) + `data/`-scoped real env, `git rm --cached` with
  a release note warning operators to back the file up first.
- **F9 — Rate limiter built, never mounted.** The gateway middleware exists
  but no layer is applied to the router. **Fix lands in**: Phase 4 — mount
  on the public auth routes at minimum (login brute-force is the obvious
  surface), with its own tests.
- **F10 — Compose fallback secret.** docker-compose falls back to a
  `'changeme-in-production'` JWT secret when unset. **Fix lands in**: Phase 4
  — fail fast at startup when `JWT_SECRET` is missing or is the known-bad
  default (mirrors the scheduler token's fail-closed treatment).
- **F11 — Pedantic backlog.** 548 advisory findings, dominated by
  `cast_possible_truncation` (45), `doc_markdown` (26), `must_use_candidate`
  (19), `missing_errors_doc` (17), `too_many_lines` (13). Campaign plan in
  CODE_SMELLS; scheduled Phase 3.
- **F12 — Queue observability.** Jobs are stored with `name = "job-<uuid>"`
  (no step name), queue status requires scheduler-token auth at the CI
  service, and the CLI has no `queue`/`runs` view — during a live saturation
  episode (both runner slots busy, three projects queued) there is no
  user-visible answer to "when will my job run". **Fix lands in**: Phase 5 —
  store the step name at enqueue, expose `GET /api/queue` through the
  gateway with repo-scoped authorization, add `gitforge pipeline --watch`
  output for queue position.
- **F13 — Unmanaged process reality.** The Makefile refuses unmanaged
  startup and points at "Fedora systemd", `scripts/gitforge-status` reports
  on `gitforge-*.service` user units — and while `.example` unit templates
  and a full release-bundle/promote toolchain exist
  (`scripts/gitforge-release-*`, `systemd/user/`), **no active units are
  installed** and the services actually run as ad-hoc processes with a
  hand-built env block (recreated verbatim in §5). Docs describe governance
  that is not applied to the live host. **Fix lands in**: Phase 0 — install
  the units from the bundle (the tooling already ships them) or reword the
  docs to describe the actual procedure.
  **RESOLVED 2026-09-22, reframed**: the services were *not* ad-hoc — they
  are supervised by **system template instances**
  `gitforge@{api,ci,git-server,runner}.service` (`Restart=always`, resource
  limits enforced, env via a `platform-runtime.conf` drop-in). The audit's
  user-unit/scope assumption was wrong; the status tool looked for
  `gitforge-*.service` user units only. Fixed by PR #205: the script probes
  both scopes and reports the unit that actually owns the process. The
  2026-09-22 cutover additionally pinned the units' `ExecStart` to the
  promoted release bundle, so deployment governance is now explicit (§4).
- **F14 — Sandbox outlives a decided outcome.** Observed during the
  2026-09-21 live cutover: when the CI service died mid-execution and its
  replacement durably failed the in-flight job, the runner (which survived)
  neither aborted the job's container nor stopped its compile; the container
  kept burning CPU until the runner's abandoned-container reconcile grace
  (default `GITFORGE_RECONCILE_GRACE_SECS=3600`) reaps it. The cancellation
  probe correctly suppresses doomed completions, but it should also **stop
  the sandbox** when the probe reports a terminal durable status.
  **Fix lands in**: Phase 1, alongside the runner upsert work.
  **RESOLVED 2026-09-22**: commit 68476c75 made the scheduler's
  `is_cancelled` probe report *every* terminal durable status (not only
  operator cancellation), so the runner's existing probe→cancel path stops
  the sandbox as soon as restart recovery decides the outcome; PR #204
  extracted the probe into `run_cancellation_watch` and added the runner-side
  tests (decided-outcome stop, transient-failure recovery, repeated-failure
  stop without orphaning).
- **F15 — Rebuild-while-running defeats the ownership check.** The
  lifecycle contract matches services by exact `/proc/<pid>/exe` path; a
  `cargo build --release` into the same `target/` replaces the inode, the
  exe link gains ` (deleted)`, and `scripts/stop-services.sh` finds nothing
  to stop. Rebuilding into a checkout that is currently serving traffic
  therefore silently disarms the stop tool. **Fix lands in**: Phase 0 —
  stop-services before rebuilding into a live `target/`, and/or teach the
  ownership check to accept the ` (deleted)` suffix of the exact path.
  **RESOLVED 2026-09-22**: `scripts/stop-services.sh` now matches the
  ` (deleted)` suffix of the exact path, and the units no longer execute from
  a checkout's `target/` at all — `ExecStart` pins the promoted release
  bundle, so rebuilding a checkout never touches serving binaries.
- **F16 — Restart publishes false greens.** Chained jobs are enqueued lazily
  by the in-memory engine; a `gitforge@ci` restart mid-pipeline left only
  the head job as a durable row, scheduler recovery re-ran just that job,
  nothing re-advanced the chain, and orphaned-run reconciliation graded the
  run `succeeded` with the rest of the pipeline never executed. Observed
  2026-09-22: two gitforge-ci runs finalized `succeeded` with 1 of 3 jobs
  (`44d2f87f`, `a117f789`). **Fix lands in**: reconciliation comparing
  durable rows against the run's persisted pipeline definition.
  **RESOLVED 2026-09-22**: PR #212 grades a shortfall `failed`
  (`incomplete_chain=true` in the finalize log); shipped in v0.6.6
  (`gitforge-d821d44-20260922`). Operating rule: never trust a green run
  whose durable job count does not cover its pipeline definition. Full
  resume of interrupted chains (rebuilding the engine from definition +
  rows) remains open if restart-resilient pipelines are wanted.
- **F17 — CI image cache staleness.** The `dsc-ci-rust` image bakes a warm
  cargo registry; a changed `Cargo.lock` against a stale cached image fails
  with "candidate versions didn't match" in ways unrelated to the code
  under test. **Fix lands in**: an in-repo image recipe plus an explicit
  bump contract. **RESOLVED 2026-09-22**: PR #208 added
  `infrastructure/docker/ci-rust.Dockerfile`; contract: any `Cargo.lock`
  change ⇒ rebuild the image and bump its tag (deployed at `dsc-ci-rust:6`).
- **F18 — CI-only latent failures hide behind false greens.** The first
  workspace test job that actually ran to completion on the refreshed image
  failed in two unrelated places: the build daemon invoked
  `rustup run stable cargo` where the image ships only a dated toolchain
  (PR #213 — the daemon now resolves the real cargo via `rustup which
  cargo`, bypass env still wins), and nine agent tests constructed
  `RunnerAgent` without the module's docker-availability guard
  (PR #214 — guard added; injecting the sandbox backend is the follow-up
  that removes the need). Lesson: a job class only falsifies under the
  environment it runs in — the false-green defect (F16) had been masking
  both.
- **F19 — The orphan reconciler races the lazy enqueue.** The periodic
  sweep grades a jobless run `cancelled` once it is older than the 600 s
  grace window, on the assumption that no engine will ever enqueue work
  for it. Under database write-lock contention the lazy enqueue lags the
  run row by far more than that: on 2026-09-23 the docs-push run
  `556dd836` had its head job enqueued at minute 29, but the sweep had
  already graded the run `cancelled` at minute 12 — stranding the job
  (repaired by hand) and making the new release gate refuse the commit,
  exactly as designed. The engine registers itself in the live-run
  registry only at first job execution, so an enqueue-starved run is
  invisible to the sweep's live filter. **Fix lands in**: an enqueue
  horizon for the jobless-run verdict (1 h, `RECONCILE_EMPTY_RUN_HORIZON_
  SECS`), plus the structural follow-up of registering the run with the
  engine at trigger time rather than first job. **RESOLVED 2026-09-23**:
  PR #220 (shipped in v0.6.7) adds the 1 h `RECONCILE_EMPTY_RUN_HORIZON_SECS`.
- **F20 — A dropped trigger INSERT silently eats a push's CI.** The
  git-server accepted a push and then inserted its `ci.trigger.pending`
  event in a single shot; under write-lock contention that INSERT could
  fail with the push already reported to the pusher — no run, no error,
  nothing in the durable redelivery ledger. **Fix lands in**: bounded
  retry inside `enqueue_ci_event`. **RESOLVED 2026-09-23**: PR #221
  (shipped in v0.6.7) retries 5 times with 3 s backoff and surfaces the
  error after the final attempt.
- **F21 — Job persist failure leaves the enqueue in-memory-only.**
  The scheduler assigner enqueues each job into its in-memory queue first
  and then persists a durable row
  (`crates/gitforge-scheduler/src/assigner.rs`,
  `enqueue_with_definition_and_image_and_timeout`); when that INSERT fails
  under SQLite write contention (`database is locked` after the 15 s busy
  window), the failure path is a lone `tracing::error!` — the job proceeds
  with **no durable row**, so completion persistence, watchdog
  reconciliation, and restart recovery never see it. Observed live on
  2026-09-23: run `8f63bfd8`'s head-job INSERT failed at 17:29:39Z after
  slogging through a gauntlet of 19–40 s COMMIT stalls, 75 s after the
  orphan sweep had already graded the jobless run `cancelled` (F19's
  window — three consecutive branch runs were lost this way that day). This
  is the durable half of the queue-idempotence work. **Fix lands in**:
  Phase 1 (retry the durable write with bounded backoff, and do not
  dispatch a job that has no durable row — coordinate with the
  queue-idempotence lane).
- **F22 — `.env` with the live `JWT_SECRET` tracked in a public repo.**
  `.env` is listed in `.gitignore`, but it was tracked before the ignore
  rule existed, so every commit since kept publishing it — secret value
  included — to the public GitHub mirror. Confirmed 2026-09-23
  (`aliasfoxkde/GitForge` is publicly readable; `JWT_SECRET` carries a real
  40-character value in history). Untracked going forward in the same
  session; the exposed secret is rotated at the next service restart, which
  invalidates outstanding tokens (re-login via `gitforge auth --login`).
  History rewrite is deliberately not used. Verified while triaging this:
  the API already fails fast on a missing `JWT_SECRET`
  (`services/api/src/main.rs` — "no dev fallback in production"), so
  rotation carries no code risk. **Fix lands in**: Phase 0 ops (rotation at
  the v0.6.7 cutover) + untrack commit.
>>>>>>> edc6cece (docs(plan): record F22 tracked-.env secret exposure)

---

## 3. Phased plan

### Phase 0 — Instance hygiene and honest deployment (days)

1. **Refresh the deployment** to current `main` using the procedure in §5,
   during a queue-quiet window (no `queued`/`running` jobs; poll the DB).
   Exit criterion: `GET /api/pipelines/{id}` answers 200; `pipeline --list`
   shows no duplicate ids; F7 re-test passes.
2. **Retire the 28 stale runner rows** via the authenticated retirement
   endpoint (it refuses busy runners — safe by construction).
3. **CLI login persistence** (F5): store token at
   `~/.config/gitforge/config.toml` `[auth] token` with 600 perms; 401 hint.
4. **Resolve F13**: ship `systemd/user/gitforge-*.service` units generated
   from the §5 env contract + an installer, or correct the docs.
   *(Done 2026-09-22, by correction: supervision already existed as system
   template units — F13 resolution; units now pin the promoted release.)*
5. `gitforge-status` gains a build-stamp field (binary mtime/commit vs
   origin/main) so F1 can never hide again.
   *(Done: PRs #198 and #205 — BUILD verdict per service plus release-vs-main
   comparison.)*

### Phase 1 — Scheduler/runner truthfulness (1–2 weeks)

1. **Runner registration = upsert by name** (F2): `RunnerQueries::create`
   becomes create-or-adopt; add the concurrency test (two simultaneous
   registrations of one name → one row).
   *(Done: PR #202, including the healing migration for existing duplicates.)*
2. **Heartbeat-derived status** (F3): runner listing and scheduler placement
   both treat `last_heartbeat` older than N (default 90 s) as offline;
   property test for the boundary.
   *(Done: PR #203 — one shared `RUNNER_HEARTBEAT_OFFLINE_AFTER_SECS`.)*
3. **Trigger correlation** (F7): integration test asserting the synchronous
   response carries the run id when creation completes within the window.
4. **Job names** (F12, part 1): carry the pipeline step name onto the job
   row at enqueue; surface it in run/queue responses.

### Phase 2 — Pipeline versioning maturity (coordinate with the
push-pipeline-version-retire branch)

1. Retention: keep the newest N versions per (repo, name); prune older
   inactive rows (configurable, default 10).
2. Uniqueness: `UNIQUE(repo_id, name, version)` or equivalent once version
   semantics settle.
3. Document the version model in ARCHITECTURE.md (what `active` means, what
   a historical version answers).
4. Backfill cleanup for the live instance (226-row dsc table → bounded).

### Phase 3 — Strictness and testability campaign

1. `PipelineQueries::update_config` so webhook-conflict paths are testable
   without DB surgery (unblocks the `too_many_lines` handler splits).
2. Injectable `CiTriggerClient` endpoint (already constructed with URL —
   allow override in tests beyond the loopback pin).
3. Cast campaign: enable `cast_possible_truncation` crate-by-crate starting
   with `gitforge-common`, `gitforge-events`; every cast gets an explicit
   range guard.
4. `OnceLock` for the review engine's literal regexes.
5. `unwrap_used` denial for pure-library crates (not services, not tests).
6. Raise the coverage floor 79.9% → 85% once Phase 3.1 unblocks handler
   tests.

### Phase 4 — Hardening

1. Untrack `.env` with the operator-safe procedure (F8).
2. Mount the rate limiter on `/auth/login` + public runner registration
   (F9).
3. Fail fast on missing/default `JWT_SECRET` in compose and binary boots
   (F10, F6).
4. Secret-file support (`JWT_SECRET_FILE`) so operators stop pasting secrets
   into process env blocks.

### Phase 5 — Platform maturation (GitForge eating its own dog food)

1. Queue visibility through the gateway (F12): repo-scoped
   `GET /api/repos/{id}/queue`, CLI `pipeline --watch` with position.
2. Artifact GC: artifacts root grows unbounded; add a retention sweep with
   receipts (mirrors the workspace sweep pattern).
3. GitForge's own CI (the `gitforge-ci` pipeline in the repo) runs on the
   live instance on every push to `main` — make this the release gate
   instead of GitHub Actions (which stays a red/billing-blocked mirror).
   *(Practice established 2026-09-22/23: the v0.6.3→v0.6.6 cutover chain
   was each gated on a live-instance true green; run `c68cd7a8` on
   `d821d44e` is the first complete honest green on `dsc-ci-rust:6` —
   fmt ✓ clippy ✓ test ✓, 3/3 jobs. Remaining: codify the gate so the
   release tooling refuses a cut without a green run id covering the
   definition.)*
4. Multi-runner soak: two runners, one saturated queue, cancel storms —
   the load shapes F2/F3 will be exercised under.

---

## 4. Deployment procedure (as of the 2026-09-22 cutover)

Lifecycle is owned by the **system template units**
`gitforge@{api,ci,git-server,runner}.service` (enabled, `Restart=always`,
MemoryMax/CPUQuota/TasksMax enforced). The drop-in
`/etc/systemd/system/gitforge@.service.d/platform-runtime.conf` carries the
env block (operator secrets live there and in the secret store — never in the
repo) and pins `ExecStart` to the promoted release bundle. A release cutover:

```
# 0. Build from a clean main-matching checkout (separate from any checkout
#    whose target/ serves traffic):
cargo build --release -p api -p ci -p git-server -p runner

# 1. Stage the four binaries plus scripts/gitforge-status, then:
GITFORGE_SOURCE_COMMIT=<sha> scripts/gitforge-release-bundle <stage> <releases-root> gitforge-<short>-<date>
scripts/gitforge-release-verify <releases-root>/gitforge-<short>-<date>
sudo scripts/gitforge-release-promote --apply <bundle> /home/gitforge/work/gitforge-current

# 2. Pin the units to the new bundle (drop-in ExecStart):
#    ExecStart=/nas/Temp/repos/GitForge/releases/<bundle-id>/bin/%i
sudo systemctl daemon-reload

# 3. Drain check — restarting ci fails in-flight rows (recovery), so wait for
#    a quiet queue or accept the recovery:
#    SELECT COUNT(*) FROM jobs WHERE status IN ('pending','assigned','running')
sudo systemctl restart gitforge@api.service gitforge@git-server.service   # stateless, any time
sudo systemctl restart gitforge@ci.service gitforge@runner.service        # after drain

# 4. Verify
./scripts/gitforge-status    # expect BUILD=current, overall: healthy
```

Restarting anything with a changed `JWT_SECRET` invalidates every issued
token (F6): re-login after cutover. `data/start-release-services.sh` is now a
thin `systemctl restart` wrapper; the units are the single source of truth.

## 5. Live E2E validation evidence (this session)

- Trigger accepted: `POST /pipelines/trigger` (trigger-token auth) → run row
  `e2db3c38` created; job DAG materialized (`fmt` → `clippy` → `test`, the
  repo's `needs` graph) with the first job enqueued on image `dsc-ci-rust:4`.
- Saturation behavior verified live: with both runner slots held by real
  work (a 570%-CPU python:3.10-slim VIVERE job; a dsc-ci-node job), the new
  job stayed `queued` with no runner — the scheduler did **not** misplace it
  and the runner did **not** over-subscribe. Queue-under-load behaves
  correctly; the gap is observability (F12), not placement.
- Full DAG execution: `fmt` succeeded end-to-end, `clippy` succeeded
  end-to-end (both with streaming log receipts persisted durably).
- Unplanned restart drill: the service cutover of 2026-09-21 landed
  mid-`test`-job. The in-flight job was durably failed, its full log
  receipt (327 chunks, ending mid-compilation) survived the restart and
  remained retrievable via `GET /api/jobs/{id}/logs`, and post-restart the
  scheduler/runner resumed processing new pushes within a minute — no
  zombie runs, no lost history. The failure was restart-induced, not a
  test failure (the workspace suite passes locally: 1 517 tests).
  Post-cutover freshness probe: `GET /api/pipelines/{id}` answers 200
  (the stale binary 404'd this route), and the pipeline listing now shows
  the true per-version rows (F4) instead of the old binary's duplicated
  ids.
- Companion merges during this session: PR #183 (E2E harness repair,
  94/94), PR #184 (this plan), PR #185 (0.5.0 changelog).
- Deployment state after the cutover: api/ci/git-server serve from the
  rebuilt `target/release` (binaries identical to the release bundle —
  same commit inputs), the runner serves from the verified bundle via
  `/home/gitforge/work/gitforge-current`. An auto-restart actor on the
  host races manual cutovers (it re-launched the three killed services
  within seconds from `target/release`); the Phase 0 systemd cutover is
  what removes this race properly.

## 6. Release checklist delta (extends IMPROVEMENTS.md §Release Checklist)

- [x] Phase 0 deploy refresh executed; `gitforge-status` shows build-stamp
      *(PRs #198, #205; deployment pinned to the promoted bundle — §4)*
- [x] Stale runner rows retired; registry row count == live runners
      *(PR #202 upsert-by-name registration incl. the healing migration)*
- [x] Release gated on a live-instance true green
      *(v0.6.6 = tag `gitforge-d821d44-20260922`; GitHub release published
      with notes covering the v0.6.2–v0.6.5 gap — v0.6.2 shipped tag-only,
      v0.6.3–v0.6.5 bundles were never GitHub-released. Recorded in
      `docs/CHANGELOG_RECENT.md`.)*
- [x] Codify the gate mechanically: the release tooling refuses a cut
      without a green run id whose job count covers the pipeline
      definition *(scripts/gitforge-release-gate, invoked by
      gitforge-release-bundle before assembly)*

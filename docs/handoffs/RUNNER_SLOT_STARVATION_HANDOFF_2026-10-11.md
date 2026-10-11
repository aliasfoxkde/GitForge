# Handoff: runner-slot starvation on the `gitforge-github` fleet and the CI merge-gate polling waiter

**Date:** 2026-10-11
**Scope:** repo-local handoff; documentation only — no code, workflow, or service changes made while producing it
**Base tree:** `main` @ `8431f11` (worktree
`/home/mkinney/worktrees/gitforge-slot-deadlock-handoff-20261011`; shallow clone, depth 1 — git
archaeology unavailable, see Blockers)
**Status:** OPEN — the historical claim as stated is NOT supported by the current tree; a related,
verifiable structural gap IS present and merits this handoff.

---

## 1. Verdict on the historical claim

Prior reports described "two try-merge runs holding both runner slots while polling jobs queued
behind them". Checked against the current tree, that composition does not exist:

- `try-merge` (`.github/workflows/auto-merge.yml`, job `try-merge`) is a fast single-shot gate: one
  non-waiting `gh pr checks` call, one `gh pr view --json mergeStateStatus` call, then
  `gh pr merge --squash --delete-branch --auto`. It contains no polling loop and cannot hold a
  runner slot for more than seconds.
- GitForge itself has no merge gate: `.gitforge.yml` contains no polling steps, and the API surface
  has no merge endpoints (the only `merge` hits in `crates/gitforge-api/src/server.rs` are axum
  `Router::merge` calls).
- The GitForge-side slot story is healthy by current evidence: head-of-line stall (F27) is fixed via
  the per-tick skipped set in `Scheduler::process_queue`
  (`crates/gitforge-scheduler/src/assigner.rs`), repo-fair admission via `QueueState::peek_fair`
  (`crates/gitforge-scheduler/src/queue.rs`), and queue telemetry via `GET /queue/status`
  (`crates/gitforge-scheduler/src/server.rs`, `get_queue_status`; documented at `docs/API.md`,
  "Scheduler queue status"). "Two slots busy, others queued" is by-design queueing, observed and
  recorded as correct behavior in `docs/planning/MASTER_PLAN_2026-09-20.md` (F12 evidence, "both
  runner slots held by real work ... the scheduler did not misplace it").

**Treat the literal claim as historical/incorrect.** The remainder of this document covers the
related gap the current tree *does* exhibit.

## 2. The verified current-tree gap

The self-hosted GitHub Actions fleet at label `[self-hosted, linux, x64, gitforge-github]` is a
single shared lane for every workflow job in this repo — including the merge gate and a
long-polling waiter — and the waiter's deadline is shorter than the work it awaits.

Verified facts, each grounded in the current tree:

1. **One shared lane.** Every job across all workflows in `.github/workflows/` uses exactly
   `[self-hosted, linux, x64, gitforge-github]`: `auto-merge.yml` (`try-merge`),
   `gitforge-ci.yml` (`enqueue-build`, `poll-build`, `report-status`), `rust-ci.yml` (fmt, clippy),
   `release.yml` (`build-linux`, which runs `cargo test --workspace` plus a release build),
   `security.yml` (5 jobs), `codeql.yml`, `integration.yml` (2 jobs), `always.yml`, `ai-review.yml`,
   `wiki.yml`. Workflow-level `concurrency` groups are per-ref
   (`${{ github.workflow }}-${{ github.ref }}`) and do not bound cross-workflow or cross-ref
   contention on the fleet.
2. **A long-polling job parks on that lane.** `gitforge-ci.yml` job `poll-build` holds its runner
   for the full poll: a `while :; do ... done` loop that curls
   `GET $GITFORGE_API_URL/api/pipeline-runs/$RUN_ID` and sleeps
   `GITFORGE_POLL_INTERVAL_SECONDS` (default 15, line-level env at the `env:` block) until
   `GITFORGE_POLL_TIMEOUT_SECONDS` (default 1800). It cannot simply move to GitHub-hosted runners:
   the in-file comment states "GitHub-hosted runners cannot reach a private Fedora LAN address".
   The workflow is additionally gated by `if: vars.GITFORGE_ENABLED == 'true'` — whether it is live
   right now is runtime state and must be probed (§3).
3. **The awaited work outlasts the waiter.** `poll-build` awaits a GitForge pipeline run of this
   repo's own chain in `.gitforge.yml`: `lint` (10m) → `fmt` (15m) → `clippy` (60m) → `test` (90m)
   → `coverage` (90m) → `fuzz` (60m), worst-case ≈ 5h25m of job time — before any queueing. On the
   GitForge runner, capacity defaults to 2 (`RunnerConfig::default` /
   `RunnerConfig::parse_from_iter` → `capacity.unwrap_or(2)` in
   `crates/gitforge-runner/src/agent.rs`; `GITFORGE_RUNNER_CAPACITY`; examples recommend 4 in
   `.env.example` and `config.toml.example`), and two concurrent runs can therefore hold both slots
   for hours while further runs queue. The 30-minute default poll deadline is far below both, so
   under load `poll-build` (a) occupies a scarce fleet slot the whole time and (b) exits red on
   `timeout` while the GitForge run is still healthy — a false merge-gate failure, not just
   latency. `docs/planning/CAMPAIGN_PLAN_2026-10-03.md` records the shape of this pressure
   live: four co-tenant 20-job runs saturating a runner's queue at 121 pending (2026-10-10
   addendum).
4. **Host contention is real, per the tree's own comments.** `release.yml` `build-linux` sets
   `GITFORGE_DOCKER_TIMEOUT_SECS: "600"` with the comment that concurrent CI sandboxes hold the
   shared Docker daemon past the 120s client default — independent in-tree evidence that concurrent
   lanes on this host already contend.

Consequence: when `gitforge-ci.yml` is enabled and multiple refs push concurrently, `poll-build`
jobs are the slots, and the merge gate `try-merge` — plus `enqueue-build`, `report-status`, and
every other lane — queues behind them. That is the current-tree mechanism matching the spirit of
the historical report, with the roles inverted: the *pollers* starve the gate, not the other way
around. The GitForge scheduler itself needs no change for this; the defect lives in the GitHub-side
lane topology and the waiter's design.

## 3. Reproduce / probe FIRST (before any design work)

The fleet size, `vars.GITFORGE_ENABLED`, and branch-protection check requirements are runtime state
outside this repo. Probe before changing anything; record outputs in the execution receipt.

1. **Fleet capacity.** Count runner registrations serving the label:
   `gh api repos/<owner>/<repo>/actions/runners --jq '.runners[] | {name, labels, busy}'`
   (host-side: inspect the `actions-runner` service config on the Fedora host). A fleet of N
   registrations executes at most N jobs concurrently; the rest queue.
2. **Is the polling lane live?** `gh variable list | grep GITFORGE_ENABLED` and
   `gh workflow view "GitForge CI"`. If `GITFORGE_ENABLED != 'true'`, `poll-build` never runs and
   the acute starvation path is dormant (record this; §4 Option A/B still removes the tripwire).
3. **Starvation signature.** For recent runs of both workflows, compare job `created_at` vs
   `started_at` (`gh run list --workflow=<wf>` → `gh api repos/<owner>/<repo>/actions/jobs/<id>`).
   A `try-merge` (or `enqueue-build`) job with material queue time while a `poll-build` job of
   another run is `in_progress` on the same fleet is the starvation signature. Also grep Actions
   history for `poll-build` conclusions of `timeout` whose GitForge run
   (`GET /queue/status` on the CI orchestrator, port 42781, scheduler-token auth; run row via
   `pipeline_runs`) later succeeded — the false-red signature.
4. **GitForge-side timeline.** During any deliberate probe push, correlate
   `GET /queue/status` (`durable_pending`, `in_memory_queued`, `assigned_jobs`, `online_runners`)
   with the Actions job states above. Do not fabricate these observations; if no live episode is
   observable, say so.

**Decision gate:** proceed to §4 if (a) `try-merge` p95 queue time under concurrent load exceeds
~60s, or (b) any `poll-build` false-red timeout is confirmed, or (c) `GITFORGE_ENABLED=true` with
fleet size ≤ 3 and observed gate queueing. Otherwise record the probe receipt and close this
handoff as no-action; do not add capacity speculatively.

## 4. Smallest maintainable design (in preference order)

**Option A — event-driven status (target state; releases the slot by never parking).**
GitForge publishes the pipeline verdict to GitHub instead of GitHub polling GitForge. The
`gitforge-events` bus and the repo commit-status surface
(`GET /repos/{owner}/{repo}/commits/{sha}/status`; `gitforge repo --commit-status`, shipped with
the #255 policy work per `docs/CHANGELOG_RECENT.md`) already exist; the missing piece is an
outbound poster in the `ci` service that, on run terminal state, sets the corresponding GitHub
commit status/check via the GitHub API. Then `poll-build` is deleted outright; `try-merge`
continues to work off GitHub checks; `enqueue-build` keeps returning in seconds. Costs: an egress
path plus a fine-scoped GitHub token managed per `AGENTS.md` secret rules ("Keep secrets in
service-managed environment files"); a service change ⇒ full release discipline (§8). Choose this
when probes confirm sustained starvation.

**Option B — probe-don't-park (minimal workflow-only change; no service change).**
Restructure `gitforge-ci.yml` so no job sleeps on the fleet: `enqueue-build` stays as-is; the wait
becomes repeated short probes driven by GitHub scheduling (a `schedule`/`workflow_dispatch`-fed
single-probe job that reads the run status once and exits, re-armed until terminal), each holding a
slot for seconds. Constraint to design for explicitly: the required-check name that branch
protection expects must remain stable across the re-arms (use a fixed check name / `checks:` API
update rather than a per-attempt job name), otherwise the gate regresses into "pending forever".
This is the fallback if Option A's egress/token prerequisites are blocked.

**Option C — lane separation via an additional runner registration (only if evidence supports).**
A second self-hosted registration on a dedicated label (e.g. `gitforge-github-gate`) for the short
control jobs (`try-merge`, `enqueue-build`, `report-status`), leaving long lanes on the existing
label. This is host-side fleet growth — take it only if §3 probes show gate queueing persisting
after Option A or B, and record the queue-time evidence that justified it.

**Independent must-fix either way — deadline semantics.** If any polling survives, the poll
deadline must exceed the awaited work: either raise `GITFORGE_POLL_TIMEOUT_SECONDS` above the
measured p95 of the full chain (measure; the `.gitforge.yml` per-job timeouts are ceilings, not
predictions), or grade a poll timeout as an explicit "unknown, still waiting" outcome (re-arm)
rather than `exit 1` red. Today's default of 1800s against a chain whose `test` job alone may run
90m is a false-red by construction under load. This fix is safe to land alone and unblocks the
gate even before any lane-topology work.

## 5. Bounded retries and timeouts

- Any new outbound status poster (Option A) uses bounded, jittered retries and fails observable —
  mirror the established durable-write discipline: `persist_with_retry`
  (`crates/gitforge-scheduler/src/assigner.rs`) is the in-tree precedent for retry wrappers, and
  `CLAUDE.md` names the F21/F23 one-shot-write class. Never retry unbounded; never swallow the
  final failure.
- Polling intervals stay constant-and-bounded (`GITFORGE_POLL_INTERVAL_SECONDS`), each iteration
  with `curl --max-time` as today; no exponential catch-up that can stampede the API after an
  outage.
- Every new GitHub-side job carries an explicit job timeout where the workflow supports it, honoring
  `AGENTS.md`: "Keep jobs bounded by their declared timeout and resource policy."

## 6. Fencing / lease / heartbeat interactions

- `poll-build` is a read-only observer of `GET /api/pipeline-runs/{id}` through the gateway; it
  holds no GitForge job lease, so no fence in
  `crates/gitforge-db/src/queries.rs` (`assign_with_lease`, per-job `heartbeat_at` UPDATE,
  `requeue_inflight`, `reconcile_expired`) is affected by it. Do not introduce any path where the
  GitHub side claims or requeues GitForge work — GitForge remains the sole expiry authority per
  `docs/architecture/adr-20261010-runner-heartbeat-plus-durable-job-timeouts.md` (runner
  liveness = `mark_stale_runners_offline` in the 5s `process_queue` tick,
  `RUNNER_HEARTBEAT_OFFLINE_AFTER_SECS = 90` in `crates/gitforge-db/src/models/runner.rs`; job
  liveness = durable timeout rows reaped by `reconcile_expired` in the
  `JOB_TIMEOUT_SWEEP_SECS = 60` watchdog, `MissedTickBehavior::Skip`, in `services/ci/src/main.rs`).
- Keep the two timeout vocabularies distinct in code and docs: a GitHub poll timeout means "the
  observer gave up", never "the job timed out"; a `timed_out` terminal status from GitForge means
  the watchdog reaped a durable row. The existing terminal-status mapping in `poll-build`
  (`cancelled|canceled|timed_out|timeout|error` → red) is correct for GitForge-decided outcomes and
  must not be extended to cover observer-side timeouts.
- If Option A lands, the poster must be idempotent per run+SHA (re-posting a terminal status is a
  no-op with the same conclusion) and must never fence or cancel GitForge work — it observes the
  same terminal truth the engines already converged on.

## 7. Automated regression tests

- **Workflow lane lint (new, repo-local, cheap).** A static checker in the spirit of
  `scripts/check-route-docs.py` (exits 1 on drift; wired head-of-chain in `.gitforge.yml`) that
  scans `.github/workflows/*.yml` and fails when (a) any job on the shared `gitforge-github` label
  contains an unbounded in-job `while`/`sleep` polling loop, or (b) a poll-style job's deadline is
  smaller than a documented floor. This pins the actual defect class so it cannot quietly return.
  Run it from the `lint` job's step list in `.gitforge.yml` next to the existing shellcheck step.
- **GitHub-side contract (manual-verify in receipt, not unit-testable in-tree):** the probe commands
  of §3 rerun as the post-change acceptance check (gate queue time ≈ 0; no `poll-build` timeouts).
- **GitForge-side guardrails (already in tree; do not duplicate, keep green):**
  `test_exclusive_class_job_does_not_block_the_batch` and
  `test_workspace_cargo_tests_are_admitted_exclusively_per_runner`
  (`crates/gitforge-scheduler/src/assigner.rs` test module) pin dispatch fairness and the
  `workspace-cargo-test` exclusive class; `crates/gitforge-runner/src/agent.rs` tests pin capacity
  parsing. No scheduler or runner code change is implied by this handoff, so no new Rust tests are
  required unless an implementer touches those crates.
- If Option A lands: unit tests for the status poster — terminal-status → GitHub conclusion
  mapping (including `cancelled` and `timed_out`), idempotent re-post, bounded-retry exhaustion is
  an error not a hang, and no secret material in logs (aegis-scanned).

## 8. GitForge lane validation criteria

Per `AGENTS.md` and `CLAUDE.md`, any merged change validates through the self-hosted lane:

- `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
  `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps` locally first.
- The `.gitforge.yml` chain green (`lint → fmt → clippy → test → coverage → fuzz`) on the branch;
  coverage gate stays ≥ 82 hard / 84 advisory (thresholds are sandbox-calibrated — do not touch).
- Option A only (service change): release evidence via `scripts/gitforge-release-gate` — run status
  `succeeded`, every durable job row `succeeded`, durable job rows cover the persisted definition
  name-for-name — then the standard bundle/promote/restart-single-time/`/health` + `gitforge-status`
  sequence, previous release retained for rollback.
- Workflow-only changes (Options B/C, deadline fix) need no service restart; validation is the
  workflow lint (§7), shellcheck (`scripts/*.sh` unchanged here, but the lint job covers the
  workflow YAML only via this new checker), and one live PR exercising the gate end-to-end.

## 9. Cancellation and recovery

- Canceling or timing out the GitHub-side waiter must not touch the GitForge run: today a dead
  `poll-build` leaves the run to finish under GitForge's own liveness planes (§6) — this is
  intended; document it in the workflow comment when editing so nobody "fixes" it into a cancel
  cascade.
- A canceled GitHub run whose `enqueue-build` already triggered a GitForge run leaves that run
  durably executing and observable via `GET /queue/status` and the run API; that is the designed
  recovery path (GitForge owns truth). Re-push re-triggers via `trigger_on: push` (`.gitforge.yml`)
  and the scheduler dedups by run identity, not by GitHub state.
- Option A failure mode: if the poster is down, GitHub shows a stale/pending status while GitForge
  is terminal. Recovery is the poster's redrive (`ENGINE_REDRIVE_INTERVAL_SECS = 300` engine
  redrive in `services/ci/src/main.rs` is the in-tree pattern for catch-up work), not a manual
  merge. Branch protection keeps the gate closed until the status lands — fail closed.

## 10. Metrics

- Ongoing: `try-merge` job queue time (created→started) sampled per §3 commands; count of
  `poll-build` timeout conclusions; fleet busy-ratio from the runners API.
- GitForge side already exposes admission telemetry (`GET /queue/status`: `durable_pending`,
  `in_memory_queued`, `assigned_jobs`, `online_runners`, per-job `waited_secs` in the `queued`
  array) — correlate, don't rebuild.
- Define the success metric before rollout: p95 `try-merge` queue time under concurrent load < 60s
  and zero poller-induced false reds in the observation window.

## 11. Rollout and rollback

- Workflow-only changes: land on a branch, validate per §8, merge, and observe one real PR cycle;
  rollback is `git revert` of the workflow file (vars/secrets untouched). No service restart.
- Option A: full release discipline (§8) because the `ci` binary changes; rollback is the retained
  previous release bundle + `gitforge-current` symlink flip + single restart, per
  `docs/RUNBOOK.md` cutover procedure and `AGENTS.md` change workflow.
- Roll back immediately if the gate regresses to pending-forever (check-name mismatch, §4 Option B
  constraint) or if the poster fails open/closed incorrectly (§9).

## 12. Explicit non-goals

- No change to `auto-merge` security semantics: the same-owner gate
  (`github.event.pull_request.user.login == github.event.repository.owner.login`), `mergeStateStatus`
  allowlist (`CLEAN|BLOCKED`), and squash-only merge stay exactly as they are.
- No scheduler/runner capacity bump without §3 evidence; no third runner without the Option C
  evidence bar.
- No redesign of GitForge admission (fairness, resource classes, watchdog) — those are fixed and
  pinned by tests.
- No move of long build lanes to GitHub-hosted runners (network-unreachable per the in-file
  constraint) and no new network plumbing beyond what the chosen option requires.
- No changes to Amortyx or any co-tenant pipeline behavior (the 121-pending episode is context,
  not scope).

## 13. Dependencies and open unknowns

- **Fleet size and `vars.GITFORGE_ENABLED`** — host/repo-settings state, not in-tree; §3 probe 1–2.
- **Branch protection's required check names** — governs whether `poll-build`'s check can be
  renamed/re-armed safely (Option B constraint) and what Option A must post. Repository settings,
  not in-tree.
- **GitForge → api.github.com egress and a fine-scoped token** — prerequisites for Option A;
  secrets handling per `AGENTS.md` (service-managed env files, never committed).
- **Shallow worktree** — this tree is a depth-1 clone, so commit archaeology (when the
  "two try-merge" report originated, whether a since-reverted change ever introduced a polling
  `try-merge`) is impossible here; re-verify from a full clone if provenance matters.

## 14. Evidence index (current tree, verified 2026-10-11)

| Fact | Anchor |
|---|---|
| Merge-gate job, single-shot, shared label | `.github/workflows/auto-merge.yml` — job `try-merge`, `gh pr checks` gate, `Enable auto-merge` step |
| Polling waiter on shared label | `.github/workflows/gitforge-ci.yml` — job `poll-build`, `while :;` loop, `GITFORGE_POLL_TIMEOUT_SECONDS` (1800), `GITFORGE_POLL_INTERVAL_SECONDS` (15), LAN comment |
| Per-ref-only concurrency groups | `concurrency:` blocks of `gitforge-ci.yml`, `rust-ci.yml`, `auto-merge.yml` |
| Long/heavy jobs on the same fleet | `.github/workflows/release.yml` `build-linux` (`cargo test --workspace` + Docker-daemon contention comment); `rust-ci.yml` `fmt`/`clippy`; `security.yml`; `integration.yml` |
| This repo's awaited chain and ceilings | `.gitforge.yml` — `trigger_on: push`; jobs `lint`/`fmt`/`clippy`/`test`/`coverage`/`fuzz` with 10m/15m/60m/90m/90m/60m timeouts |
| Runner slot model | `crates/gitforge-runner/src/agent.rs` — `RunnerConfig.capacity` (default 2, `GITFORGE_RUNNER_CAPACITY`), fetch loop over `/jobs/pending`, `JobChannel` per-job heartbeat |
| Capacity-bounded dispatch, fairness, skipped set | `crates/gitforge-scheduler/src/assigner.rs` — `process_queue` (`max_jobs_per_batch = available_capacity.max(1)`), `peek_fair` + `skipped` (F27), `resource_class` → `workspace-cargo-test` |
| Scheduler serves only assigned jobs | `crates/gitforge-scheduler/src/server.rs` — route `/jobs/pending`, `get_pending_jobs`, `get_queue_status` (`/queue/status`) |
| Liveness planes | `services/ci/src/main.rs` — 5s `process_queue` tick, `JOB_TIMEOUT_SWEEP_SECS`, `MissedTickBehavior::Skip`; `crates/gitforge-db/src/queries.rs` — `assign_with_lease`, `requeue_inflight`, `reconcile_expired`; `crates/gitforge-db/src/models/runner.rs` — `RUNNER_HEARTBEAT_OFFLINE_AFTER_SECS = 90` |
| Queue telemetry contract | `docs/API.md` "Scheduler queue status"; `docs/planning/MASTER_PLAN_2026-09-20.md` F12/F27; `docs/planning/CAMPAIGN_PLAN_2026-10-03.md` 2026-10-10 addendum (121 pending) |
| Static-gate precedent for §7 lint | `scripts/check-route-docs.py`, wired in `.gitforge.yml` `fmt` job |
| Prior art for slot-free waiting | `crates/gitforge-cli/src/main.rs` — `watch_pipeline_run` (`WATCH_POLL_SECS = 2`, `WATCH_MAX_POLLS = 900`) is CLI-side and holds no fleet slot |

## 15. Execution receipt requirements (for the implementing worker)

Return: probe outputs of §3 (fleet census, `GITFORGE_ENABLED`, queue-time table, any false-red
evidence), the chosen option with the rejected alternatives' reasons, changed-file list, the §7
lint output, one end-to-end gate validation on a real PR, and an explicit statement separating
observed results from assumptions. No tokens, credentials, or invented run IDs. A prose "done"
without these artifacts is a rejection (same bar as `docs/handoffs/CONTAINER_RECONCILER_HANDOFF.md`).
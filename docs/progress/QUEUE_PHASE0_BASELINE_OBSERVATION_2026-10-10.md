# GitForge runner/queue Phase 0 baseline observation — 2026-10-10

Phase 0 (observation-only) of handoff
`GITFORGE_RUNNER_QUEUE_RELIABILITY_AND_TEMP_ISOLATION_2026-10-10.md`
(platform-handoffs worktree). No production service was touched, no database
was written, no process was restarted or killed, no temp path was cleaned,
and no repo-wide test was run. Every host operation was read-only
(`ssh -o BatchMode=yes` plus SQLite `mode=ro` / `PRAGMA query_only=1`).

## Revisions resolved (handoff currency fence satisfied)

| What | Revision | Evidence |
|---|---|---|
| Analysis base (this repo) | `c4e3816bf19dd3a4ca06b9861257e4a925ad3c71` = GitHub `main` HEAD, branch `codex/gitforge-queue-baseline-20261010` | `git ls-remote https://github.com/aliasfoxkde/GitForge.git` |
| Handoff baseline | `b2d6a3c5` — ancestor of `c4e3816`; delta is 1 commit (merge of PR #290) | `git merge-base --is-ancestor` after unshallowing |
| **Deployed binaries** | `4d72efc2bd9cf2e9ed009cdf7d4c3ecca894e959`, release `gitforge-4d72efc2-20261010`, built `2026-10-10T17:19:16Z` in `dsc-ci-rust:9` | `readlink -f ~/work/gitforge-current`; `RELEASE_METADATA.txt` on the Fedora host |
| Landed engine work cited by handoff | `c71a87c` (custody-stall redrive) is an ancestor of `c4e3816` | `git merge-base --is-ancestor` |

`4d72efc2..c4e3816` touches only `Makefile`, `docs/CHANGELOG_RECENT.md`, and
release scripts — **zero changes** in `crates/gitforge-scheduler`,
`crates/gitforge-runner`, `services/ci`, `services/runner`,
`crates/gitforge-db`. The deployed queue behavior is exactly the code
analyzed here.

Line-number drift of the handoff's cited symbols at `c4e3816`: **none**.

| Handoff citation | Resolved at `c4e3816` |
|---|---|
| `agent.rs:1294` `RunnerAgent::claim_job` | unchanged — `crates/gitforge-runner/src/agent.rs:1294` |
| `agent.rs:1322` `execute_job` | unchanged — `crates/gitforge-runner/src/agent.rs:1322` |
| `server.rs:593` scheduler `claim_job` | unchanged — `crates/gitforge-scheduler/src/server.rs:593` |

Discrepancy noted, not chased (read-only mandate): the deploy-root checkout
`/nas/Temp/repos/GitForge` is now at `e1c9d5a8` (2026-09-05, an ancestor of
main, with `origin`-only remotes) and does not contain `b2d6a3c5`. That
checkout is **not** the deployment source — deployment is a release bundle
switched at `~/work/gitforge-current` — but the handoff's same-day
observation of `b2d6a3c5` at that path is no longer reproducible; the
checkout state evidently changed between handoff writing and this phase.

## Exact queue/run terminal transitions (code at `c4e3816`)

Job rows (`jobs.status`), `JobStatus` at `crates/gitforge-common/src/ids.rs:291`:
`pending` → `queued` (scheduler queue admission, `assigner.rs:479`) →
`assigned` (assigner binds job→runner) → `running` (lease-gated claim,
`server.rs:593`) → terminal ∈ {`succeeded`, `failed`, `cancelled`,
`timed_out`, `infrastructure_failure`} (`is_terminal`, `ids.rs:308`).
Completion is persisted durably under lease guard in the scheduler's
`complete_job` (`server.rs:945`) before the engine absorbs the event.

Run rows (`pipeline_runs.status`): created → `running` → terminal ∈
{`succeeded`, `failed`, `cancelled`, `timed_out`}. Finalizers:

1. Completion consumer / timeout watchdog → `finalize_run_if_terminal`
   (`services/ci/src/main.rs:2556`); a failed durable write keeps the engine
   registered for watchdog retry (run `bccaa1be` lesson). Non-success
   verdicts additionally sweep the run's never-dispatched rows
   (`sweep_unclaimed_jobs`, `main.rs:2645` →
   `JobQueries::cancel_unclaimed_for_run`, `queries.rs:1536`).
2. Orphan reconciler every 60 s (`RECONCILE_INTERVAL_SECS`,
   `main.rs:1329`): grades runs with no live engine; 600 s creation grace
   (`RECONCILE_MIN_RUN_AGE_SECS`), 3600 s jobless horizon
   (`RECONCILE_EMPTY_RUN_HORIZON_SECS`), skips on unreadable job lists
   (2026-09-16 lesson), grades short chains `failed` against the persisted
   definition (2026-09-22 lesson), doom-cascades un-dispatchable rows.
3. Operator cancellation → `Scheduler::finalize_pipeline_if_terminal`
   (`crates/gitforge-scheduler/src/assigner.rs:630`) — finalizes only when
   **all** job rows are terminal.

Guards: the F24 terminal-verdict guard
(`PipelineRunQueries::update_status_with_error`, `queries.rs:948`) makes a
terminal verdict immutable via one conditional UPDATE (no grader rewrite
races); `sweep_ghost_jobs` runs **every 60 s tick** (`main.rs:1638`) canelling
`pending`/`queued` rows under any terminal run
(`cancel_unclaimed_in_terminal_runs`, `queries.rs:1561`); the custody-stall
redrive (`c71a87c`, `rebuild_live_engines`, 300 s) re-enqueues queued rows
under held engines when nothing is actively executing.

## Is the handoff's queued-but-terminal class still possible?

**Row-level (queued/pending rows outliving a terminal run): defended in
depth, and empirically absent.** Three independent landed defenses (per-run
sweep at non-success finalization, 60 s global ghost sweep, orphan
reconciler + redrive). Live snapshot: **zero** pending/queued rows under
terminal runs.

**Verdict-level (falsely-green): still possible in principle, silent by
design, and it has real historical instances.** A stranded row under an
already-`succeeded` run is flipped `cancelled` by the ghost sweep, but the F24
guard forbids rewriting the run verdict — so a chain can stay green with a
job that never ran, and nothing detects it. The live DB has 12 `succeeded`
runs containing non-succeeded job rows, but 10 of those are legacy
`completed` rows (Aug 22–24, inert). Scoped to **`succeeded` runs containing
a `failed` job row**, exactly two exist: `ffcfd7ba` (2026-09-10) and
`527e7ca5` (2026-09-15) — each graded `succeeded` with one `failed` row
among 2 and 6 total jobs respectively. None since (25 days; 1,032 runs
since 2026-09-15). This is the concrete target for the Phase 1 detector.

Phase 2 hypothesis (no per-job temp isolation): **verified in code** —
`grep -rn TMPDIR crates/ services/` at `c4e3816` returns nothing outside
tests; `execute_job` passes `assignment.env` verbatim (`agent.rs:1363`), so
job processes inherit the container default temp dir. The `tempfile::tempdir`
hits the handoff cites are inside `#[cfg(test)] mod reconciler_loop_tests`
(`agent.rs:1968`) — the handoff's framing holds.

## Live snapshot (bounded, read-only)

Method: `ssh -o BatchMode=yes 192.168.1.202` + `python3` stdlib `sqlite3`
with `file:…?mode=ro` URI and `PRAGMA query_only=1` against the live DB
confirmed by unit env: `DATABASE_URL=sqlite:%h/work/gitforge-audit-20260822/audit.db`
(`systemctl --user cat gitforge-ci.service`). Observation window:
2026-10-10 17:49:50–17:53:03 CDT (22:49–22:53 UTC).

- Jobs total 10,978. By status: succeeded 9,426 / failed 1,267 /
  cancelled 183 / **pending 40** / **queued 38** / running 1 /
  timed_out 10 / legacy `completed` 13 (Aug 22–24 only, inert).
- Queue depth (handoff law): 78 waiting rows, oldest created
  21:11:57 UTC (≈1 h 38 m at first read), all under **non-terminal** runs.
  Queue moved between reads (40+38+1 → 38+37), heartbeats fresh — draining,
  not wedged.
- Zero pending/queued rows under terminal runs (ghost sweep holding).
- Runner scoping: exactly **one** registered scheduler runner,
  `remote-podman-runner` (`d28711cc…`, capacity **1**, online, identity
  `fedora-podman-runner-20260915`, podman user-socket); ≈3.8k jobs in a
  trailing 7-day window (window-definition sensitive: ~3,792–3,795 for
  natural 7-day cuts, ~4,102–4,111 for day-cut/trailing-now — see the
  independent-validation note below; exact method and cut time not pinned).
  10 concurrent `running` runs — control-center ×7, amortyx ×3 — serialized
  on the one slot; observed single-job durations were short (current
  samples 12–18 s; audited all-time max for each named job is 1 min,
  11 instances each — the earlier "19 s–36 min / `dep-audit` 27 m /
  `doc-check` 36 m" figures are unsupported and retracted).
  The 78-row backlog belongs to control-center + amortyx chains — the
  gitforge repo's own chain is not the only tenant; **no global-queue
  inference from one process**.
- Suspected stall checked before flagging: amortyx run `2bcbe799`
  (`running` 11 h, 376 job rows) is a deliberate long-lived pipeline on a
  5-minute stage cadence, actively progressing (jobs finishing during the
  read) — not a custody stall.
- End-to-end trace without REST: run `b61fcdbd` (`succeeded`, commit
  `ffdfa47d`) → job `9eb68a3e` (`gate`) → `result_json` 91,104 bytes
  (`contract_version: harness.job.v1`) → 3,249 `job_log_chunks`
  (209,559 bytes). Per-job verdicts fully derivable from the audit DB.
- Independent CI on the same host: four GitHub `Runner.Listener` processes
  alive during the window (PIDs 1972, 745270, 896302 on `amortyx`, 982717
  on `amortyx-cpu2`) — excluded from all conclusions above.

`/tmp` (read-only `df`/`du`): tmpfs 16 G, 13 G used, 3.2 G free (**81 %**).
11.93 GB (11.11 GiB) of it is eight `tmp.XXXXXXXXXX` dirs of ≈1.4–1.5 GB
each, owned by `mkinney`, mtimes 08:01–09:11 CDT today; contents are
Rust-workspace + `ci-rust.Dockerfile` build contexts and `repo/` checkouts —
CI image-build / checkout scratch that no job-finalization path reclaims.
Four additional smaller scratch dirs were present (0.91 GB, 0.29 GB, 3 MB,
3 MB). Attribution is partial (contents identified, owning process not —
still unresolved). **Not touched** — cleanup is deny-listed; this is Phase
2's per-job scratch lifecycle evidence.

## Uncertainty

- Deployed binaries (`4d72efc2`) vs analysis base (`c4e3816`): no scheduler/
  runner/ci/db delta (verified by diff), so no behavioral skew for this
  subsystem.
- The two falsely-green rows are evidence the class occurred; the precise
  historical code path that produced them was not reconstructed (pre-dates
  several landed guards).
- `/tmp` consumer attribution is by content shape, not by process.
- REST was not exercised (recorded 401 law); DB-only derivability was
  demonstrated instead.
- Job timestamps in the DB are UTC RFC3339; host shell times are CDT.

## Independent validation (2026-10-10 23:05Z)

A second independent read-only audit (same `mode=ro` DB access; no writes,
no cleanup, no production change) re-derived the figures above. Verified as
first reported: revision/source facts, the zero queued-under-terminal
snapshot, the 78-row backlog and its draining behavior, the single capacity-1
runner, the `2bcbe799` and `b61fcdbd` traces, and "no falsely-green instance
since 2026-09-15". Corrections applied to this receipt:

- `Runner.Listener` count two → **four** alive in the window (adds PIDs
  896302 on `amortyx`, 982717 on `amortyx-cpu2`).
- Concurrent-run split control-center ×8 / amortyx ×2+ → **×7 / ×3**
  (10 total unchanged).
- Runtime examples `dep-audit` 27 m / `doc-check` 36 m **retracted**: no
  27/36-minute instances exist in the audited DB or the localhost stack;
  all-time max for each named job is 1 min (11 instances each; current
  samples 12–18 s).
- "3,843 jobs in 7 d" not reproducible as an exact figure → reported as
  **≈3.8k** with window uncertainty (natural 7-day cuts ≈3,792–3,795;
  day-cut/trailing-now ≈4,102–4,111).
- Eight large `/tmp` dirs total **11.93 GB (11.11 GiB)**, not ≈10.9 GB;
  four additional smaller scratch dirs (0.91 GB, 0.29 GB, 3 MB, 3 MB).
  Owner/process attribution remains unresolved; nothing was deleted.
- Falsely-green claim scoped: 12 `succeeded` runs contain non-succeeded
  rows, but 10 are legacy `completed`; exactly **two** succeeded runs
  contain a `failed` job row (`ffcfd7ba` — 2 jobs, `527e7ca5` — 6 jobs).
- Runs since 2026-09-15: ~500 → **1,032**.

## Ranked next implementation task

1. **Phase 1.1 — falsely-terminal/falsely-green detector** (handoff order
   confirmed by evidence): at reconciliation tick, flag terminal runs whose
   job-status composition contradicts the verdict (`succeeded` run
   containing any non-succeeded job row; non-terminal run whose queued rows
   exceed a configurable age). Pin validation against the two historical
   rows `ffcfd7ba` / `527e7ca5` (they must be flagged) plus the existing
   redrive/reconcile tests; current live population is zero, so historical
   rows are the only in-repo fixtures that exercise the detector.
2. **Phase 1.2 — queue-visibility surface**: expose queue depth +
   oldest-queued age per repo/runner as a status endpoint or DB view; the
   exact scoped SQL from this receipt is the specification
   (jobs `pending`/`queued` joined to non-terminal runs, grouped by
   `repositories.name`).
3. **Phase 2** entry point confirmed: `execute_job` (`agent.rs:1322`),
   env assembly at `agent.rs:1363` — per-job scratch root + `TMPDIR`
   insertion lands there; nothing else in the workspace sets `TMPDIR`.

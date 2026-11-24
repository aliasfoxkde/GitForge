# Progress: Dark Factory Enhancement

> **Abandoned.** This log tracked the Dark Factory Go-template effort, not GitForge. Current progress lives in [planning/IMPROVEMENTS.md](planning/IMPROVEMENTS.md).

## Execution Log

### 2026-07-01

| Time | Action | Status |
|------|--------|--------|
| Initial | Plan created and approved | ✅ |
| | `template-parts/common/` created | ✅ |
| | `./AGENTS.md` created | ✅ |
| | `template-parts/atheon-enhanced/` integrated | ✅ |
| | `template-parts/scaffolding/repository-ruleset/rules/` added | ✅ |
| | `.claude/settings.json` for codebase-memory-mcp | ✅ |
| | `.github/wiki/` expanded (4 new pages) | ✅ |
| | Documentation files created for hook compliance | ✅ |
| | E2E framework completed (session.go, reporter.go, debugger.go) | ✅ |
| | Go template enhanced (api/, logger/, db/, _fixtures/) | ✅ |
| | code-library/snippets/common/ created (git_hooks.sh, ci_validation.sh) | ✅ |
| | Root `.claude/settings.json` created | ✅ |
| | `.gitignore` line 35 fixed | ✅ |
| | `README.md` template parts list updated | ✅ |
| | `template-parts/README.md` updated to reflect actual structure | ✅ |
| | `template-parts/go/` completed (go.mod, Makefile, .golangci.yml, pkg/) | ✅ |
| | `.dockerignore` template created | ✅ |
| | `template-parts/common/` expanded (Dockerfile, docker-compose.yml) | ✅ |

## 2026-11-16 — storage lease deferral candidate (unvalidated)

Added a source-only runner/scheduler deferral path on branch
`fix/ci-container-io-weight-20261111`: the runner carries the scheduler lease
token into its assignment and claim request; when the local host-storage lock
is busy, it can request an authenticated, conditional defer; the database
requeues only the matching `assigned` job and persists
`host_storage_lease_busy`. Focused DB, scheduler, HTTP, and runner tests were
added but not run. System I/O PSI is elevated and GitForge builds are active on
the coordinator. The candidate is not deployed; a same-host immediate
redispatch loop still needs resource-domain backoff/availability signaling.

## 2026-11-16 19:11 UTC — runner-scoped retry cooldown added (unvalidated)

The scheduler candidate now suppresses storage-sensitive reassignment to a
runner for 10 seconds after a host lease conflict; a focused scheduler test
was added. This avoids immediate same-runner assign/defer churn but is only an
in-memory runner-ID cooldown, not durable host/device admission. At 19:11 UTC
I/O PSI some/full avg10 was 46.79/38.86% (avg300 37.14/27.79%) with three
GitForge containers active. No build/test ran; the candidate remains
uncompiled and undeployed.

## 2026-11-16 21:17 UTC — formatting validated; Rust suite deferred

The first managed `gitforge-fmt` check reported formatting differences in the
dirty candidate. Ran `cargo fmt --all`; the managed check then passed at exit
0 (job `c72dd862-7a57-4d92-80d6-21b30ec3e3dc`). This validates formatting only.
The active `firecraft-ci-rust:2` job still runs with its workspace and target
cache on SWARMONE `/nas`; the host admission service reports its `xtask`, Cargo,
and rustc processes and denies heavy/normal work. Rust compilation/tests remain
deferred until that live workload drains and admission clears.

## 2026-11-17 04:53 UTC — candidate runner now gates storage-heavy claims on local host pressure

The runner-side host-storage lease now checks local I/O full PSI, CPU some PSI,
and active Cargo/Rust/build processes after acquiring the lease and before
claiming an assignment. Unknown/malformed PSI, invalid thresholds, unreadable
process state, or an active build defers the job; a pressure rejection drops
the lock before returning. The defer reason is now
`host_resource_admission_busy`; the scheduler continues accepting the prior
`host_storage_lease_busy` spelling during rollout. Added unit contracts for
high/low/unknown pressure, live versus zombie build processes, and releasing
the lock on denial. Updated the runner runbook and job contract.
`rustfmt` on the new module and targeted `git diff --check` pass. No Rust tests
or build ran because live CI jobs are still consuming storage I/O. This is an
uncompiled, undeployed candidate change.

## 2026-11-17 05:03 UTC — rollout compatibility for pressure deferrals

Added a scheduler contract that accepts both the new
`host_resource_admission_busy` and prior `host_storage_lease_busy` defer
reasons while rejecting arbitrary values. This keeps a rolling candidate from
rejecting a runner that still emits the old reason. The test is source-added
but not executed because active CI jobs remain. No service was restarted.

## 2026-11-17 05:06 UTC — runner deferral logs identify the local admission blocker

The runner-side admission check now emits a structured debug reason for high
I/O PSI, high CPU PSI, missing/malformed PSI, invalid thresholds, active
compiler processes, or an unavailable process probe. This keeps the queue/API
deferral contract bounded while making the runner journal diagnostically
useful. Added reason assertions to the source contracts. New-module rustfmt and
targeted diff checks pass; package tests and compile remain deferred while
active CI builds continue.

## 2026-11-17 05:10 UTC — admission denial contract protects claim ordering

Added a runner contract proving that a pressure denial never calls the
scheduler claim closure, drops the acquired host lock, and leaves the lease
available to the next admission attempt. The module is formatted and targeted
diff checks pass. This contract has not been executed; SWARMONE is still
running three GitForge jobs including Rust link/codegen.
## 2026-11-17 05:34 UTC — installer refuses to mask an active local runner

The NAS installer’s default runner-retirement path previously disabled and
masked the service without checking whether its process was active. It now
checks status before any side effects and fails closed for active, transitioning,
or indeterminate state; the error requires supported pause/drain, terminal
receipts, zero owned containers, and an explicit stop. `bash -n` passed and the
focused installer-policy contract passed 6/6, including no-side-effect checks
for active/unknown state. The live service remains active and masked on the
Nov. 12 release; no production service or job was changed.

## 2026-11-17 05:42 UTC — live preflight refused the still-active runner

Executed the installer in default mode against the real systemd state. It
exited 1 with `Refusing to mask active GitForge runner (active)` before install
operations. Follow-up showed the same runner PID 3652512 active/masked with
four active job containers. This validates the active-runner refusal in the
live environment; it does not drain or change production. Candidate remains
undeployed.

## 2026-11-17 06:42 UTC — bound cleanup after failed sandbox acquisition

Live release job `7f9f153e-e6bf-428e-85e9-63098b8ed0f5` was accepted at
06:32:27 UTC; executor-start was logged, but no container-created event or
matching Docker container was observed, and SQLite remained `running` with no
result receipt at 06:41 UTC. Source tracing found that acquisition has a
load-aware timeout, but failed-acquisition container reaping had no deadline.
Added a 30-second bound using the existing force-removal budget and a focused
pending-future contract test so a stalled Docker daemon cannot indefinitely
block the executor from returning its acquisition failure. Rust parsing and
diff checks pass; test/build/deployment remain pending because active host
and CI builds continue. No live job or service was changed.

## 2026-11-17 06:46 UTC — corrected the apparent containerless release job

Follow-up runner logs show job `83953691-849a-498c-81eb-044102d1ed5a`
completed and its container was destroyed at 06:42:37.464, followed by
creation of job `7f9f153e-e6bf-428e-85e9-63098b8ed0f5` at 06:42:37.780.
Both `release` jobs use the same image and the pipeline's writable
`{cache_root}/cargo/{image}-release` target. The earlier job held the target
mount lock for its lifetime; the second job remained assigned and heartbeating
while waiting before container creation. This was scheduler admission and
queue-state waste, not an orphaned job or proven reaper hang. The candidate's
storage-class admission should leave conflicting compile jobs queued. The
bounded failed-acquisition cleanup change is separate hardening. No test,
build, or production action was performed.

## 2026-11-17 06:58 UTC — fail-closed admission during runner-domain recovery

Candidate scheduler review found a restart race: an in-flight storage-sensitive
job can be re-adopted before its runner reconnects and registers its
`storage_domain`. Admission previously treated the unknown domain as idle.
Changed the candidate to hold storage-sensitive work while an active sensitive
assignment has missing/blank runner-domain metadata, and added
`unknown_domain_for_readopted_cargo_job_blocks_storage_admission`. Rust parser
and `git diff --check` pass. No compile/test ran because SWARMONE has active
release jobs and Fedora has active Amortyx Cargo jobs; this candidate remains
uncompiled and undeployed.

## 2026-11-17 07:05 UTC — focused recovery-domain admission test passes

Built the exact dirty candidate snapshot on Fedora `/nas` using an isolated
`CARGO_TARGET_DIR`, `--offline --locked`, two workers, and idle I/O priority.
The first compile identified a missing `sqlx::Row` import for
`SqliteRow::try_get` in `gitforge-db/src/connection.rs`; the candidate was
fixed, copied, and hash-verified. The scheduler crate then compiled and
`unknown_domain_for_readopted_cargo_job_blocks_storage_admission` passed 1/1.
Fedora CPU PSI some avg10 later reached 50.52% with Rust linkers active, so the
full scheduler suite was not started. This candidate remains undeployed.

## 2026-11-17 07:03 UTC — release lock wait directly measured

Runner logs show release `686582dd-6936-4f89-b096-adb9e56fa70f` accepted at
06:53:07 and container created at 07:03:37.651, 294 ms after the previous
release completed and released the same writable Cargo target lock. The
scheduler-visible `running` period therefore included a ~10m30s runner-side
wait. This confirms why locking only inside the executor wastes assigned
capacity; storage-sensitive admission must occur before durable assignment.
The second release is executing now; no production mutation was made.

## 2026-11-17 07:16 UTC — scheduler suite passes 180/180

On Fedora, full `cargo test --offline --locked -p gitforge-scheduler --lib --
--test-threads=1` passed 180/180 against the staged dirty candidate under the
host build lease, with two workers and isolated output on Fedora `/nas`. This
includes the restart/unknown-storage-domain admission regression. The focused
Platform cleanup suite passed 44/44 after hermetic test fixtures were updated;
no production cleanup logic changed. `git diff --check` passes. Next validate
runner, database, and sandbox package tests before broader gates.

## 2026-11-17 07:21 UTC — delayed release reached terminal success

SQLite records release `686582dd-6936-4f89-b096-adb9e56fa70f` succeeded at
07:21:24. The runner accepted it at 06:53:07, created its container at
07:03:37.651 after the predecessor released the shared Cargo target lock, and
ran for about 17m47s. The successful terminal result confirms a valid workload
while preserving the measured ~10m30s scheduler-visible wait. No runtime
mutation. Candidate admission changes are not deployed.

## 2026-11-17 07:30 UTC — runner control API and executor suites validated

On the staged dirty candidate on Fedora `/nas`, `gitforge-db`,
`gitforge-sandbox`, and `gitforge-runner` library tests all exited successfully
(runner: 126 passed / 4 ignored). A compile failure exposed an extra
`step_working_directories` field in the `ExecutableJob` initializer; per-step
cws already travel on `JobStep`, so removed the redundant initializer. The
`gitforge-api --test runners_routes` suite passed 13/13, including registration
capability and pause/drain lifecycle. Scheduler is 180/180; Platform cleanup
contracts 44/44. Full repository gates, immutable release, and production
canary remain outstanding.

## 2026-11-17 07:35 UTC — candidate format gate remains red

`cargo fmt --all -- --check` reports rustfmt drift in changed DB and scheduler
Rust source. Did not apply formatter changes under elevated host pressure:
SWARMONE I/O full avg10 9.18%; Fedora I/O some/full avg10 14.75/6.90%, CPU
some avg10 29.75% with Cargo/Java active. Focused package checks pass, but the
full formatting gate and broader release gates remain open; candidate is not
promotion-ready.

## 2026-11-17 07:40 UTC — candidate formatting passes; full release gate pending

Formatted the candidate after SWARMONE I/O pressure returned to zero. Synced
the seven changed Rust files to Fedora snapshot
`/nas/Temp/validation/gitforge-candidate-20261117-0701`; each SHA-256 digest
matches the source, and `cargo fmt --all -- --check` passes. Earlier 07:35
format failure is superseded by this receipt. Focused scheduler (180), runner
(126 passed / 4 ignored), DB/sandbox, and API runner-route (13) suites passed
before formatting; formatting was nonsemantic. Fedora currently has CodeQL and
a workspace Cargo test active, so no additional broad build was stacked. The
candidate remains dirty/uncommitted and the validation snapshot has no Git
metadata: no full repository gate, exact-SHA hosted checks, immutable release,
or isolated canary has completed. Production remains the Nov. 12 release.
Resume after the active jobs finish by freezing and hashing one candidate tree,
then running full gates serially before hosted checks and isolated canary.

## 2026-11-17 07:44 UTC — root Cargo cache is generated JBOD state

The GitForge checkout's untracked root `cache/` contains 13 GiB across roughly
25,700 generated Cargo files. `findmnt -T cache` resolves it to `/nas` on
`/dev/sda1` Btrfs, not the 444 GiB NVMe. None of its files are tracked. Added
`/cache/` to `.gitignore` so Git metadata operations do not enumerate the
generated runtime tree. Preserved its contents; this change does not free NVMe
space or alter the runner cache. `git check-ignore` and bounded `git status`
confirm the path is ignored. Current worktree registries report 88 GitForge
entries and 104 Platform-Architecture entries; those are registered counts,
not a stale-worktree finding.

## 2026-11-17 08:08 UTC — dirty candidate source freeze captured

Captured the GitForge candidate source against base commit
`ed0b27567043aa6d86aac49bb1ef79b84d2bf1f4` under
`/nas/Temp/validation/gitforge-freeze-20261117-0808/`. It contains 44 modified
tracked paths and 6 untracked source files. The tracked patch SHA-256 is
`6c7d725e378014d2785667bfb6053d70b2cd49c2aa37af17814de94168a0daf1`; the
untracked-source tar SHA-256 is
`b3f8f4c798a8268342c6393ebe507386afcd8eb8fb1a961d0079958b279d22da`; manifest
SHA-256 is `450f125dfa902eebe47c8a61c87a3a157944a524c87c838b48334f2af2b9a712`.
`git apply --reverse --check` succeeds against the live source, and root
`cache/` is excluded. This freeze identifies the exact dirty tree but does not
create a commit SHA or hosted-check identity. Fedora still has two Amortyx
Cargo test processes; do not stack candidate builds. Full exact-tree gates and
hosted CI/Aegis remain pending.

# GitForge abandoned-container reconciler handoff

**Priority:** P0  
**Repository:** `/nas/Temp/repos/GitForge`  
**Execution host:** Fedora `mkinney@192.168.0.201`  
**Base observed:** `origin/main` = `235292df`  
**Candidate branch:** `codex/container-reconciler-20260914`  
**Status:** implementation exists in a preserved worktree but is not accepted

## Objective

Safely discover and, only under explicit policy, remove GitForge-owned Docker containers left behind after a runner crash, cancellation, OOM, or host restart. The reconciler must be fail-closed, bounded, idempotent, observable, and unable to affect unrelated containers or active jobs.

## Required invariants

- Ownership requires exact label `com.gitforce.managed=true` and a parseable UUID in `com.gitforce.job_id`; names, image, age, and exit code are never ownership evidence.
- Running containers and containers correlated to an active/claimed job are always retained.
- Eligibility requires authoritative runtime exit time older than the configured grace period. Missing or invalid exit time retains the container.
- Default mode is read-only census. Deletion requires explicit configuration, a bounded maximum per pass, and a safe rollout receipt.
- Docker 404/409 races are idempotent outcomes; other errors are visible failures and do not trigger unbounded retries.
- Every Docker list/inspect/remove operation has an enforced timeout. A timeout cannot block runner admission or shutdown.
- The background task is owned by the runner lifecycle and is cancelled/joined on shutdown.
- Receipts contain counts, IDs or stable hashes, decisions, and outcomes only—never secrets, environment values, logs, or tokens. Receipt write failures are observable.

## Implementation sequence

1. Start from a clean isolated worktree based on the current GitForge `origin/main`; do not alter the canonical dirty checkout.
2. Inspect the existing sandbox label creation and runner active-job/lease state. Reuse the authoritative interfaces rather than introducing a second job identity source.
3. Implement a pure classifier with explicit states for unowned, malformed, active, running, grace, policy-disabled, and eligible.
4. Implement Docker listing plus per-container inspection sufficient to obtain authoritative `FinishedAt`; fail closed when unavailable. Do not infer exit time from `Created`.
5. Wrap list/inspect/remove futures in the configured timeout. Test a pending future and assert bounded return.
6. Implement a bounded reconcile pass with deterministic ordering, `max_removals`, race handling, and structured receipt.
7. Own the periodic task through a cancellation token or equivalent; prove startup pass, stop, and restart without a leaked task.
8. Add unit tests for all classifier boundaries and integration tests against a disposable Docker container. Never use global prune.
9. Run formatting, strict lint, focused tests, full workspace tests, Aegis, and the GitForge lane on Fedora with explicit timeout/resource limits.
10. Obtain independent review against both this specification and GitForge standards. Commit only after all gates pass; open a PR with exact evidence. Do not merge or enable deletion automatically from the worker.

## Acceptance matrix

| Gate | Evidence required |
|---|---|
| Ownership safety | tests prove wrong/missing labels and name-only matches are retained |
| State safety | tests prove running and active-job containers are retained |
| Exit-time correctness | test with old `Created` and recent `FinishedAt` is retained; old finish is eligible |
| Grace boundary | exact boundary and missing timestamp behavior proven |
| Timeout | pending Docker source returns within configured bound |
| Idempotence | repeated pass and 404/409 race remove at most once |
| Blast radius | max-removals and unrelated-container integration proof |
| Lifecycle | shutdown cancels and joins reconciler; restart has no duplicate task |
| Receipt safety | JSON schema/path/error tests and secret scan |
| Full quality | Fedora GitForge lane, strict lint, Aegis, and independent review green |

## Known candidate defects

The preserved candidate initially failed compilation and had a mutex deadlock in its mock; those were repaired for review. It still substitutes creation time for exit time, declares but does not enforce `call_timeout`, lacks clearly owned task cancellation, and suppresses receipt write errors. These are acceptance blockers, not tasks to waive.

## Resource and rollback policy

Run on Fedora only, one GitForge heavy lane at a time, with a hard outer timeout and bounded cgroup. Keep deletion disabled through qualification. Rollback is branch/commit revert plus service restart; do not delete historical containers until an operator-approved retention policy identifies them as eligible.

## Final worker report

The worker must return the branch, commit SHA, changed-file list, exact commands and exit codes, runtime/container evidence, test counts, scan results, unresolved findings, and a statement distinguishing proven behavior from assumptions. A prose “done” message without these artifacts is a rejection.

# GitForge Improvement Plan

Date: 2026-08-28
Updated: 2026-09-08
Status: Active

## Summary

Repository state after audit:
- **Tests**: All passing (300+ tests across workspace)
- **Linting**: Clippy passes with `-D warnings`; ShellCheck clean on all
  repo scripts; actionlint clean on all workflows; both gates added to
  Rust CI and the Makefile
- **Formatting**: `cargo fmt --check` passes
- **Dependency vetting**: cargo-vet initialized (`supply-chain/`); `make
  lint` no longer fails on a missing `cargo vet`
- **Race Detection**: Fixed storage durability issue with `sync_all()` calls
- **Coverage**: 82.90% lines (`cargo llvm-cov --all`; CI floor: 79.9%)
- **Aegis**: Integrated into CI (already present in security.yml)
- **E2E**: Template framework exists in template-parts; GitForge has no web frontend

## Resolved Security Gaps (2026-09-08)

1. **Scheduler completion without lease proof** — `POST /jobs/{id}/complete`
   accepted anonymous completions for any known job. Completion now requires
   the assigning runner identity and lease token; unknown jobs return 404.
2. **`/jobs/{id}/assign` no-op stub removed** — the route acknowledged
   assignments without performing any; a client-selectable assignment path
   would also have bypassed scheduler policy.
3. **Runner registration fail-open** — `RunnerAgent::register()` now fails
   closed by default (`GITFORGE_RUNNER_STANDALONE=deny`); legacy standalone
   fallback requires an explicit `allow`.
4. **Compose credentials** — `docker-compose.yml` requires
   `GITFORGE_SCHEDULER_TOKEN` for CI and runners, matching the fail-closed
   scheduler auth that was already enforced at the HTTP boundary.
5. **ai-review.yml env bug** — review summary/critical findings were passed
   as action inputs instead of step env vars, so `process.env` lookups in
   the comment script always hit their fallback.

## Honest Assessment: What's Achievable

### Achieved Previously
1. **Storage Durability Fix**: `sync_all()` calls prevent race conditions
2. **MockAiProvider**: Full mock implementation for testing AI providers
3. **Executor Unit Tests**: 7 new tests for JobResult, ExecutableJob
4. **Coverage**: 79.80% → 80.12%

### Blocked on Infrastructure (Cannot Be Done Without Docker/Daemons)
The following require a running integration environment:

| Item | Blocked By | Workaround |
|------|------------|------------|
| gitforge-runner/executor coverage | Live Docker daemon required | `#[ignore]` integration tests pass against a live daemon (see below) |
| 99% coverage on main.rs | Full infra required | Not achievable; the spawned-binary harnesses for git-server and ci lifted main.rs to 69%/83.5% lines respectively |

### Previously Blocked, Now Passing (2026-09-08)

| Item | Status |
|------|--------|
| Docker sandbox integration tests | 3/3 `#[ignore]` tests pass against live Docker 26.1.5 (`cargo test -- --ignored`) |
| Runner executor timeout reaping | Real-container test passes: hung job reaped at timeout |
| Git-server protocol tests | NEW `services/git-server/tests/git_http_protocol.rs`: real `git push`, `git clone`, `fetch`, and `ls-remote` against the spawned binary + SQLite DB |
| Compose-stack queued-job smoke | Disposable api+ci+runner+git-server run: push → trigger → pipeline → lease-fenced execution → durable completion → logs via API; cross-process durable queue with idempotency; restart requeue + reconciliation observed (see HANDOFF) |

### Not Applicable
| Item | Reason |
|------|--------|
| WGCA 2.1 AAA | GitForge is a CLI/Git server, not a web app |
| Browser e2e tests | No frontend - template-parts is a template, not GitForge UI |

## Coverage Analysis (Current State)

### Well-Covered Crates (>85%)
| Crate | Lines | Functions |
|-------|-------|-----------|
| gitforge-common | 100% | 100% |
| gitforge-events | 95%+ | 93%+ |
| gitforge-db/models | 90%+ | 95%+ |
| gitforge-scheduler | 88%+ | 87%+ |
| gitforge-process | 86%+ | 93%+ |

### Moderate Coverage (70-85%)
| Crate | Lines | Issue |
|-------|-------|-------|
| gitforge-api | ~80% | API routes need error path tests |
| gitforge-cli | ~81% | CLI integration tests |
| gitforge-build | ~67% | Daemon mode hard to unit test |

### Low Coverage (<70%) - Entry Points
| Crate | Lines | Issue |
|-------|-------|-------|
| gitforge-runner/executor | 32.45% | Container execution requires Docker (`#[ignore]` tests) |
| gitforge-build/daemon | 21.82% | Integration-only code |
| gitforge-ai | 7-58% | API mocking needed |

(2026-09-08 re-measurement, `cargo llvm-cov --all`: workspace total
82.90% lines. services/git-server left this table's sub-30% bucket after
its protocol harnesses were made coverage-visible: main.rs 69.43%,
ssh_server.rs 85.64%; services/ci followed at 83.54% once its
spawned-binary trigger harness landed.)

## Remaining Gaps and Next Steps (2026-09-08)

Ordered by value; each item states the concrete blocker.

1. **cargo-vet audits** — the exemption backlog stands at 362 crates (79
   fully audited), down from 377 after importing the zcash peer registry
   and recording three publisher trusts our existing imports already vouch
   for (dtolnay for proc-macro2 via isrg/mozilla/bytecode-alliance;
   Manishearth for potential_utf and icu_normalizer_data via mozilla). The
   remainder is dominated by the russh/RustCrypto tree from the SSH
   transport rewrite; the RustCrypto 0.9/0.10-rc and russh 0.63 versions
   have no audits in any peer registry yet because they are too new.
   Incremental path: re-run `cargo vet suggest` (it names both
   small-diff audits and trust candidates grounded in existing imports),
   re-run `import` + `prune` as peer registries pick the new versions up,
   and `cargo vet inspect` + `certify` only for diffs a human actually
   reviewed. Six peer registries are registered and pinned in
   `imports.lock`, so pruning is automatic once coverage exists.

## Resolved from the Remaining-Gaps Ledger (2026-09-08)

1. **Runner registration retry/backoff** — done. `RunnerAgent::register`
   retries an unreachable or 503-answering scheduler with bounded
   exponential backoff (`GITFORGE_REGISTER_ATTEMPTS`, default 6;
   `GITFORGE_REGISTER_BACKOFF_SECS`, default 1s, doubling to a 30s cap)
   before honoring the fail-closed exit. Auth rejections (401/403) are
   fatal on the first attempt and other non-503 statuses are never retried;
   standalone fallback still requires `GITFORGE_RUNNER_STANDALONE=allow`.
   Covered by real-socket tests: 503/503/201 retry-then-succeed with
   connection counting, single-attempt auth rejection, exhausted transport
   errors, and the policy-rejection allow/deny matrix.
2. **cargo-vet: registries + CI enforcement** — partially done. The vet
   gate had gone red when the SSH rewrite landed 80+ new dependencies with
   no audit coverage: the five applicable public audit registries (isrg,
   google, mozilla, bytecode-alliance, embark-studios) are now registered
   and pinned in `supply-chain/imports.lock`, the new tree is recorded as
   tracked exemptions via `cargo vet regenerate exemptions`
   (`cargo vet` is green again: 64 fully audited, 2 partially audited, 377
   exempted), and a `supply-chain` job now enforces `cargo vet` in
   rust-ci.yml so future dependency changes that lose coverage fail CI
   instead of silently drifting. Mass-certifying the backlog was rejected
   as dishonest: an audit entry asserts a human reviewed the source.
3. **Per-user SSH key authorization** — done. SSH no longer authenticates
   any key on possession. Public keys are registered to accounts through
   `POST /api/ssh-keys` (name + OpenSSH public-key line, parsed and
   validated at registration, stored with the `SHA256:` fingerprint),
   listed with `GET /api/ssh-keys`, and removed with
   `DELETE /api/ssh-keys/{id}` (ownership-fenced; 409 on a fingerprint
   already registered to any account, enforced by a UNIQUE column and a
   pre-check). `GitSshSession::auth_publickey` resolves the presented
   key's fingerprint against the `ssh_keys` table, logs accepted
   fingerprints with the owning account, and fails closed on all three
   failure modes: no database, unregistered key, and registry lookup
   error. This makes SSH strictly stronger than the unauthenticated Smart
   HTTP transport. Covered by `test_ssh_unregistered_key_is_rejected`
   (a real second keypair is denied with `Permission denied`),
   `test_database_ssh_key_registry` (fingerprint resolution, scoping,
   duplicate rejection, ownership-fenced deletion), and the API parsing
   tests (valid ed25519, comment-insensitive fingerprints, garbage
   rejection).
4. **Service entry-point coverage, measurement side** — done for
   git-server. The HTTP and SSH protocol harnesses spawn the real
   `git-server` binary, but they stopped it with `start_kill()`, so the
   child never flushed its LLVM profile and `cargo llvm-cov` reported the
   entry point at ~20-26% despite the suites driving push/clone/fetch and
   full SSH handshakes through it. Both suites now stop the server with
   SIGTERM via `tests/common/mod.rs::shutdown_gracefully` — the real
   graceful-shutdown path, with a SIGKILL fallback after 10s so tests
   never hang — which counts the child's coverage: main.rs 23.83% →
   69.43% lines, ssh_server.rs 22.07% → 85.64%. Lesson recorded: a
   spawned-instrumented binary only reports coverage on a clean exit.
5. **Service entry-point coverage, ci** — done. A new spawned-binary
   harness (`services/ci/tests/ci_trigger_flow.rs`) boots the real `ci`
   service against a temporary SQLite database, bare git repository with
   a committed `.gitforce.yml`, workspace root, and artifact root, then
   drives the same HTTP trigger endpoint the git-server calls after a
   push: it asserts the trigger token is required (401 without), the run
   is created and reported synchronously (202 `accepted` with the run
   id), the run and job rows are durable in the database the service
   wrote, the job's commands and image come from the committed
   definition rather than a substituted default, and the workspace is a
   real clone checked out at the pushed commit. The service is stopped
   with the same SIGTERM graceful-shutdown helper, so the consumer loops
   and startup path are counted. services/ci `main.rs` went from
   64.08% to 83.54% lines, and the workspace from 79.63% to 82.90%
   lines. What remains below the CI floor's reach is inherent:
   gitforge-runner/executor (32.45% lines) executes containers and is
   covered by the `#[ignore]` tests that pass against a live Docker
   daemon.

## Resolved from the Compose Smoke (2026-09-08)

1. **Periodic orphaned-run reconciliation** — done. `reconcile_orphaned_runs`
   (services/ci/src/main.rs) now runs on a 60s loop with a 120s run-age grace
   window and a live-engine guard; startup still sweeps once with no guards.
2. **Post-restart requeued completions lack lease proof** — done. Root cause
   was not lease propagation: `requeue_inflight` deliberately clears leases
   (receipts cannot be trusted across a restart), so a still-running
   execution is orphaned by design. The fix makes orphaning explicit and
   prompt end to end:
   - `Scheduler::is_cancelled` reports every terminal durable status, so the
     runner's cancellation probe stops a sandbox whose row was failed or
     requeued by restart recovery, not only operator cancellations.
   - The runner skips log, artifact, and completion reporting once its probe
     says the outcome was decided mid-execution; no doomed 409s.
   - A credentialed completion for an unassigned job is rejected 409 with
     "job is no longer assigned to a runner; its durable outcome was decided
     without this completion" instead of the malformed-request message.
   - Covered by `test_job_cancelled_probe_reports_terminal_durable_status`
     and `test_complete_job_unassigned_reports_orphaned_outcome`.
3. **git-server SSH protocol** — done, and the finding was bigger than a
   missing test: the ssh2/libssh2 listener could never complete a handshake
   (libssh2 is a client-side library and the socket was never attached to
   the session), and the `SshGitHandler` behind it was advertisement-only
   with a receive-pack that never moved refs. The transport is now a russh
   server (`services/git-server/src/ssh_server.rs`) that pipes
   authenticated channels to real `git upload-pack`/`git receive-pack`
   child processes, with a persisted ed25519 host key and required
   public-key auth. `tests/git_ssh_protocol.rs` drives real `push`, `clone`,
   `fetch`, and `ls-remote` over `ssh://` with generated keypairs and
   host-key pinning, and asserts key-less clients and unknown repositories
   are rejected.

### Not Applicable
- Browser/WCAG e2e: GitForge has no web frontend; template-parts are
  scaffolding templates, not GitForge UI.

## Release Checklist

- [x] All tests pass (`cargo test --workspace`)
- [x] Clippy clean (`cargo clippy --workspace -- -D warnings`)
- [x] Format check (`cargo fmt -- --check`)
- [x] ShellCheck clean (scripts/ + systemd/)
- [x] actionlint clean (.github/workflows/)
- [x] `cargo vet` gate initialized and passing
- [x] Coverage ≥80% (CI floor: 79.9%)
- [x] Docker-gated tests validated against a live daemon (4/4)
- [x] Git Smart HTTP protocol validated end-to-end (push/clone/fetch)
- [x] Compose-stack queued-job smoke validated (push → pipeline → durable completion → restart recovery)
- [x] CHANGELOG updated
- [x] GitHub release created (v0.3.3)

## References

- [COMPREHENSIVE_EXECUTION_PLAN_2026-08-28.md](./COMPREHENSIVE_EXECUTION_PLAN_2026-08-28.md) - Full 8-phase roadmap
- [SCALING_RESEARCH.md](./SCALING_RESEARCH.md) - Git at scale research

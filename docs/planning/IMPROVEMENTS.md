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
- **Coverage**: ~80% lines (CI floor: 79.9%)
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
| Service entry point coverage | TCP listeners, DB pools | Integration test suite |
| 99% coverage on main.rs | Full infra required | Not achievable in unit tests |

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
| services/ci | ~35% | main() entry point requires infra |
| services/git-server | ~20% | Git protocol requires Docker |
| gitforge-runner/executor | ~35% | Container execution requires Docker |
|-------|-------|-------|
| gitforge-runner/executor | 5.53% | Requires Docker integration |
| services/git-server | 20.48% | Git protocol integration tests |
| gitforge-ai | 7-58% | API mocking needed |
| gitforge-build/daemon | 21.82% | Integration-only code |

## Remaining Gaps and Next Steps (2026-09-08)

Ordered by value; each item states the concrete blocker.

1. **git-server SSH protocol test** — the protocol test covers Smart
   HTTP only. The SSH path needs host-key/authorized-key fixtures for
   `run_ssh_server`; candidate extension of `tests/git_http_protocol.rs`.
2. **Runner registration retry/backoff** — fail-closed currently exits the
   process; bounded retry with backoff before exiting would tolerate a
   scheduler that is briefly unavailable at runner start.
3. **cargo-vet audits** — `supply-chain/` currently exempts 364 transitive
   crates. Run `cargo vet suggest`/`cargo vet fetch` incrementally to move
   high-risk deps from exemption to audited.
4. **Service entry-point coverage** — `main()` functions require TCP
   listeners, DB pools, and daemon connections; realistic aggregate ceiling
   with integration harnesses is ~85-90%, not 99%.

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

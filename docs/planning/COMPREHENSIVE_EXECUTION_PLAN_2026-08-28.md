# GitForge and Harness Comprehensive Execution Plan

Date: 2026-08-28

Status: active; this document is the controlling task ledger for the next
implementation cycles. Observed facts are marked as evidence, planned work as
tasks, and unverified work is never counted as complete.

## Current evidence baseline

| Area | Evidence | State |
| --- | --- | --- |
| Workspace | Rust crates for API, CI, scheduler, runner, sandbox, storage, Git, CLI, and services | Indexed and structurally mapped |
| Code graph | codebase-memory-mcp: 8,135 nodes and 26,535 edges | Ready; use before text search |
| Tests | Full workspace `cargo test` and managed GitForge builds pass in prior checkpoints | Passing baseline |
| Coverage | `cargo llvm-cov --workspace --all-features`: 79.98% lines, 81.47% regions, 81.36% functions | Measured; below 99% target |
| Manager | Bounded queue, concurrent pipe drains, process groups, cancellation, coordinated shutdown, stale socket cleanup | Active-job shutdown smoke-tested |
| Distributed jobs | Durable leases, requeue, cancellation, log chunks, artifact transfer, authenticated API boundary | Multi-process restart and retry semantics incomplete |
| CI | Immutable action pins, least-privilege defaults, locked release test, SBOM, provenance, LLVM coverage ratchet | YAML validated; hosted run still required |
| Harness | `harnessctl doctor` passes; nightly memory and analysis timers active | Opencode absence is the only doctor warning |
| Pattern scanning | Aegis available locally; changed-line gate added to GitForge CI. Atheon MCP scan endpoint failed internally; binary scan is noisy | Needs bounded, reproducible scanner policy |
| Accessibility | W3C WCAG 2.1 defines A/AA/AAA; no complete browser audit has been executed | Not complete |

## Architecture principles

1. Keep the API, scheduler, runner, sandbox, storage, Git server, and local
   build manager as separate failure domains with explicit contracts.
2. Every long-running operation must have a bounded wait, cancellation path,
   observable state, and a recovery story after client or process failure.
3. Durable state is authoritative; in-memory queues and process registries are
   rebuildable indexes, never the only record of work.
4. CI is fail-closed for tests, lint, dependency review, security analysis,
   artifacts, and release provenance. Optional reporting may be non-blocking
   only when the required gate is independent and recorded.
5. Security scanners must distinguish newly introduced findings from reviewed
   historical fixtures, preserve reports, and never print sensitive matches.
6. Accessibility is tested at the page and interaction level. AAA is an
   aspirational target for applicable criteria; the W3C notes that blanket AAA
   requirements are not appropriate for every type of content.

## Phase 0 — Control plane and reproducibility

Goal: make every subsequent result attributable, bounded, and recoverable.

- [x] P0-01 Maintain clean branch/push checkpoints in GitForge and the
  canonical harness repository.
- [x] P0-02 Keep codebase-memory indexing current and use graph discovery for
  symbol-level work.
- [x] P0-03 Run `scripts/harnessctl doctor` and preserve failures as evidence.
- [x] P0-04 Add a single `scripts/qualityctl` entry point that runs the exact
  local equivalents of format, lint, tests, coverage, scanner, docs, and E2E
-  gates with per-stage timeouts and machine-readable results. The implemented
  first tranche covers frontend template checks, format, strict Clippy, locked
  workspace tests, and optional LLVM coverage; scanner/docs stages remain
  follow-up additions.
- [x] P0-05 Add a versioned quality manifest containing tool versions, Rust
  channel, coverage floors, scanner commit, and required environment checks.
  `scripts/qualityctl` now records the Git commit/branch, Cargo/Rust versions,
  LLVM coverage availability, Docker/Aegis availability, and coverage floor.
- [x] P0-06 Make generated reports land under ignored `target/quality/` and
  upload or archive them by run ID; prevent root-level artifacts such as
  `codecov.json` from appearing as untracked changes. `qualityctl` now emits
  per-stage logs and a JSON manifest there.

Exit criteria: a clean checkout can run one command and produce a complete,
timestamped pass/fail manifest without relying on a developer terminal.

## Phase 1 — Manager reliability and operational control

Goal: no hung manager request, orphaned descendant, or operator lockout.

- [x] P1-01 Queue submissions without holding the control request open.
- [x] P1-02 Drain stdout and stderr concurrently and detach stdin.
- [x] P1-03 Use process groups, SIGCONT/SIGTERM/SIGKILL, and child reaping.
- [x] P1-04 Add status, list, cancel, stats, and graceful shutdown commands.
- [x] P1-05 Bound socket message allocation and forward cargo flags naturally.
- [ ] P1-06 Replace polling-only client waits with a bounded event/notification
  protocol while retaining polling as a recovery fallback.
- [ ] P1-07 Persist manager jobs and leases so a daemon restart reconstructs
  queued/running/terminal state and marks uncertain children for reconciliation.
- [ ] P1-08 Add an orphan supervisor registry keyed by process-group ID, start
  time, workspace, and job ID; reconcile it on startup and shutdown.
- [ ] P1-09 Add stress tests for eight concurrent noisy dual-stream children,
  cancellation during spawn, SIGSTOP/SIGCONT, daemon crash, stale socket, and
  client disconnect.
- [ ] P1-10 Add operator safety controls: queue depth limit, per-job resource
  budget, timeout override policy, structured logs, and a health endpoint.

Exit criteria: kill -9 of the client or daemon leaves no unowned process and
restart recovery produces one authoritative terminal outcome per job.

## Phase 2 — Distributed scheduler/runner contract

Goal: exactly-once state transitions and at-least-once observability without
duplicate side effects.

- [x] P2-01 Use durable lease tokens/generations and reject stale runner writes.
- [x] P2-02 Requeue jobs after heartbeat expiry and persist cancellation.
- [x] P2-03 Upload bounded logs and artifacts with checksum/path validation.
- [x] P2-04 Expose authenticated API downloads and an HTTP boundary test.
- [ ] P2-05 Add durable, idempotent log delivery with sequence numbers,
  retry/backoff, acknowledgement, and final reconciliation. Current live sink
  can lose a suffix after a partial delivery failure.
- [ ] P2-06 Add a true multi-process E2E harness for API, CI, scheduler, runner,
  storage, and sandbox, including restart at every state transition.
- [ ] P2-07 Bind every job to repository, commit SHA, pipeline/run, lease, and
  runner generation; reject stale approvals and mismatched artifacts.
- [ ] P2-08 Add runner drain/quarantine modes and explicit operator cancellation
  propagation with bounded acknowledgement.

Exit criteria: restart, duplicate delivery, stale lease, runner loss, and
network retry tests pass without duplicate artifacts or invalid state changes.

## Phase 3 — Sandbox and resource safety

Goal: untrusted jobs cannot exhaust or escape the execution host.

- [x] P3-01 Keep Docker/stub sandbox modes explicit and testable.
- [ ] P3-02 Enforce CPU, memory, PID, disk, network, wall-clock, and output
  budgets at the sandbox boundary, with recorded termination reasons.
- [ ] P3-03 Add seccomp/rootless/read-only filesystem/network policy tests for
  the deployment mode actually used in production.
- [ ] P3-04 Add large-output, binary-output, signal, timeout, and cleanup tests.
- [ ] P3-05 Verify artifacts are copied only from declared paths and are bound
  to the completed job receipt.

Exit criteria: resource exhaustion and cancellation tests are deterministic and
the runner cannot retain capabilities after job completion.

## Phase 4 — CI/CD and supply-chain quality

Goal: CI says what actually passed and releases are verifiable.

- [x] P4-01 Pin active actions to reviewed full commit SHAs.
- [x] P4-02 Set read-only default permissions and narrow write permissions.
- [x] P4-03 Make CodeQL compilation and dependency review fail-closed.
- [x] P4-04 Run locked workspace tests before Linux release packaging.
- [x] P4-05 Publish CycloneDX SBOM and artifact provenance attestations.
- [x] P4-06 Replace mutable/ unavailable secret scanning references with local
  deterministic checks where necessary.
- [x] P4-07 Add pinned Aegis changed-line scanning for new high/critical
  findings.
- [ ] P4-08 Execute hosted GitHub workflow canaries and verify the SBOM,
  attestation, checksum, and release assets from a clean machine.
- [x] P4-09 Add dependency lockfile policy (`--locked`, cargo-deny/license and
  advisory policy) to every Rust build path, including templates. The active
  build/test/lint/release workflows now use `--locked`, root `cargo deny check`
  is enforced by `scripts/qualityctl` and the active Security workflow, and the
  RSA/h2 findings were fixed. Both frontend templates now have committed pnpm
  lockfiles and frozen-install validation.
- [ ] P4-10 Add workflow linting, action pin drift detection, shellcheck, and
  script-injection checks as required CI jobs. The dependency-free immutable
  action-ref check now runs in `scripts/qualityctl`, and the active Security
  workflow contains actionlint and ShellCheck jobs; local availability and
  full hosted evidence remain open.
- [ ] P4-11 Add rollback/release promotion gates and a deployment canary with
  health, migration, and artifact verification checks.

Exit criteria: a failed required analysis cannot report green; release
consumers can verify checksum, SBOM, and provenance independently.

## Phase 5 — Coverage, tests, and code quality

Goal: increase meaningful coverage, not test-count inflation.

- [x] P5-01 Replace the weak tarpaulin gate with LLVM source coverage.
- [x] P5-02 Establish the measured 79.98% line / 81.36% function baseline.
- [ ] P5-03 Add per-package and changed-code coverage floors so low-covered
  service entry points cannot hide behind aggregate coverage.
- [ ] P5-04 Raise the aggregate floor in small verified increments: 80, 85,
  90, 95, then 99 percent, with a reviewed exception process.
- [ ] P5-05 Prioritize runner executor, build daemon, CI service, Git server,
  and API error paths identified by the coverage report.
- [ ] P5-06 Add mutation testing for scheduler transitions, authorization,
  cancellation, receipts, and artifact validation.
- [ ] P5-07 Enable strict Rust lint policy (`clippy::pedantic` selectively,
  missing docs for public APIs, unsafe-code review, deny warnings) and resolve
  all justified warnings rather than blanket allowances.
- [ ] P5-08 Add documentation coverage: public API docs, runbook command tests,
  architecture decision records, examples compiled or executed in CI, and a
  link checker.

Exit criteria: every floor is measured from CI artifacts, critical state
machines have mutation/property tests, and documentation checks are automated.

## Phase 6 — Web accessibility and frontend validation

Goal: applicable web surfaces meet WCAG 2.1 A/AA and maximize AAA criteria.

- [ ] P6-01 Inventory API dashboard HTML and all `template-parts` frontend
  surfaces; identify ownership and supported browsers. Vite React PWA and SSR
  surfaces are now inventoried with real Playwright web-server configs and an
  immutable-pinned hosted matrix workflow; API dashboard inventory remains
  open.
- [ ] P6-02 Add Playwright keyboard, focus order, reduced motion, reflow/zoom,
  form-label, status-message, contrast, and screen-reader-oriented checks.
- [ ] P6-03 Run axe or equivalent automated checks and manually review all
  applicable AAA criteria; document justified exceptions per component.
- [ ] P6-04 Add accessible error, loading, cancellation, and live-log states.
- [ ] P6-05 Make accessibility artifacts part of release readiness and prevent
  regressions on changed templates.

Exit criteria: browser tests pass on every supported surface and the report
maps each applicable WCAG criterion to evidence or an explicitly reviewed gap.

## Phase 7 — Memory, harness, and agent operations

Goal: improve context quality without allowing stale or speculative memory to
degrade model behavior.

- [x] P7-01 Keep durable decisions, findings, and handoffs in the context
  ledger; nightly cleanup and analysis timers are enabled.
- [ ] P7-02 Separate memory into durable facts/decisions, temporary session
  notes, unresolved hypotheses, and archived superseded material.
- [ ] P7-03 Add nightly “dream” processing that clusters duplicates, extracts
  validated durable facts, expires stale temporary notes, and emits a review
  report; it must never silently rewrite authoritative decisions.
- [ ] P7-04 Add confidence/source/observed-at/last-validated metadata and
  conflict detection before promotion to durable memory.
- [ ] P7-05 Add retrieval budgets, relevance thresholds, recency weighting,
  and provider-specific context adapters to avoid prompt overload.
- [ ] P7-06 Record tool failures, timeout causes, and recovery outcomes as
  structured operational memory rather than raw transcripts.
- [ ] P7-07 Add memory doctor tests for duplicate, stale, contradictory, and
  oversized entries.

Exit criteria: memory cleanup is idempotent, reviewable, bounded, and cannot
delete a durable decision without a preserved replacement/history record.

## Phase 8 — Release, deployment, and operations

- [ ] P8-01 Validate Docker Compose and systemd deployments from clean hosts.
- [ ] P8-02 Add health/readiness/liveness checks for every service and runner.
- [ ] P8-03 Add metrics for queue depth, wait time, cancellation latency,
  stale leases, log delivery failures, artifact failures, and memory jobs.
- [ ] P8-04 Rehearse backup/restore, migration rollback, release rollback, and
  runner quarantine with recorded evidence.
- [ ] P8-05 Create a release only after hosted CI and verification gates pass;
  do not create a release from local-only evidence.

## Current risks and honest gaps

1. The 99% target is not met: aggregate line coverage is 79.98%.
2. “Documentation coverage” has no agreed denominator or automated metric yet;
   P5-08 must define it before a percentage can be reported.
3. The HTTP API/scheduler boundary test is in one process, not a restart-safe
   production E2E test.
4. Live log delivery is bounded but lacks durable retry/reconciliation after a
   partial network failure.
5. Aegis full-tree output is currently dominated by intentional fixtures and
   heuristic findings; changed-line blocking is the defensible interim policy.
6. The Atheon MCP scanner endpoint failed during this audit. The binary
   fallback is available but must be time-bounded and scoped away from build
   artifacts.
7. No hosted GitHub Actions run or production deployment was performed in this
   local audit; release readiness therefore remains unverified.

## Research basis

- [GitHub Actions security](https://docs.github.com/en/actions/how-tos/secure-your-work)
- [GitHub secure-use guidance](https://docs.github.com/en/actions/reference/security/secure-use)
- [GitHub workflow artifacts](https://docs.github.com/en/actions/concepts/workflows-and-actions/workflow-artifacts)
- [GitHub artifact attestations](https://github.com/actions/attest)
- [Cargo continuous integration](https://doc.rust-lang.org/stable/cargo/guide/continuous-integration.html)
- [cargo-llvm-cov](https://github.com/taiki-e/cargo-llvm-cov)
- [W3C WCAG 2.1](https://www.w3.org/TR/WCAG21/)

## Working rule

Each implementation cycle must select the next smallest unblocked task,
preserve a dated evidence entry, run the narrowest relevant tests first, run
broader gates before promotion, commit and push both repositories, and update
this ledger with failures as well as successes.

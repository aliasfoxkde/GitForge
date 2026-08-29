# Testing Strategy

This repository is a Rust Git/CI platform with service, scheduler, runner,
sandbox, storage, CLI, and template surfaces. The current source of truth for
coverage and outstanding test work is [the comprehensive execution plan](planning/COMPREHENSIVE_EXECUTION_PLAN_2026-08-28.md).

## Required local gates

Run the narrowest relevant gate first, then the complete workspace gate:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --no-fail-fast
cargo llvm-cov --workspace --all-features --locked --no-fail-fast \
  --cobertura --output-path target/quality/cobertura.xml
```

The local build manager is part of the reliability surface:

```bash
gitforge-buildd
gitforge-build test --workspace
gitforge-build --stats
gitforge-build --list
gitforge-build --cancel JOB_ID
gitforge-build --shutdown
```

For the repeatable quality sequence, use `scripts/qualityctl --fast` for
format, strict Clippy, and locked workspace tests, or omit `--fast` to add the
LLVM coverage gate. It writes a JSON manifest and per-stage logs under the
ignored `target/quality/` directory. Set `QUALITY_TIMEOUT_SECONDS` to bound a
stage in constrained environments.

For manager changes, also run the active-job shutdown smoke test described in
`docs/AUDIT.md`; it must verify socket removal and no surviving cargo/rustc
descendants.

## Test layers

### Unit tests

Use unit tests for deterministic state transitions, parsing, validation,
authorization decisions, protocol framing, resource limits, and error mapping.
Tests must not depend on external services or shared mutable state.

### Integration tests

Use real in-process HTTP routers, SQLite migrations, file storage, and sandbox
stubs where those boundaries are the subject under test. Every test must state
which boundary is real and which dependency is replaced. Do not call an
in-process router a multi-process E2E test.

### End-to-end tests

The required E2E path is:

1. API accepts a repository/pipeline request.
2. CI creates a durable run and scheduler job.
3. Scheduler leases the job to a runner.
4. Runner executes in the selected sandbox.
5. Logs stream with sequence/lease integrity.
6. Artifacts and receipt are checksum-verified and downloadable through the
   authenticated API.
7. Restart at each state transition preserves one authoritative outcome.

Docker, database, and multi-process tests must be explicitly marked and have
bounded timeouts with cleanup on success and failure.

## Coverage policy

Coverage is measured with LLVM source instrumentation, not test-count proxies.
The measured 2026-08-25 workspace baseline is:

| Metric | Baseline | Current gate | Target |
| --- | ---: | ---: | ---: |
| Lines | 79.98% | 79.9% | 99% |
| Regions | 81.47% | report-only | 99% |
| Functions | 81.36% | report-only | 99% |

Raise the line floor only after new tests pass in CI. Add per-package and
changed-code floors before raising the aggregate floor, so service entry points
cannot disappear inside an aggregate number. Critical scheduler, lease,
authorization, cancellation, artifact, and receipt paths require branch or
mutation testing in addition to line coverage.

## Security and quality gates

- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo audit` and the repository dependency/license policy
- Aegis changed-line scan for new high/critical findings
- workflow YAML parsing and immutable action-pin checks
- shell lint and script-injection checks
- generated documentation/link checks
- browser accessibility tests for every supported web surface

Scanner findings are evidence, not automatic permission to suppress. Intentional
fixtures require a documented baseline or changed-line policy; raw secret
matches must never be printed in logs.

## Failure handling

Every long-running test must have a timeout, process-group cleanup, and an
artifact containing enough structured information to reproduce the failure.
Record failures in the dated audit and task ledger. A skipped, timed-out, or
non-blocking security job is not a passing result.

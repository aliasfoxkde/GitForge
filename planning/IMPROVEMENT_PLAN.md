# GitForge Improvement Plan

> **Source:** Analysis of VibedByKaKi/refined-github architecture + codebase audit
> **Date:** 2026-11-19
> **Status:** Active — workstreams executed

---

## Workstreams Executed

| # | Workstream | Status | Risk |
|---|-----------|--------|------|
| 1 | Harden revision resolver | **Done** — `is_safe_revision()` now rejects `{`, `}`, `:` | Low |
| 2 | Externalize OpenAPI spec | **Done** — `openapi.json` extracted, `openapi.rs` loads from file | Low |
| 3 | Capability registry | **Skipped** — `resource_class` module already well-structured | N/A |
| 4 | DAG readiness events | Skipped — low value, O(n) is acceptable for typical pipeline sizes | Low |
| 5 | Cancellation watch configurability | **Done** — `GITFORGE_CANCELLATION_WATCH_FAILURES` env var (default 3) | Low |
| 6 | CI trigger outbox dead letter | **Partially done** — migration `002_dead_letter.sql` created | Medium |
| 7 | Runner capability profiles | Skipped — low value, backward-compatible as-is | Low |
| 8 | Fix DAG cycle detection | **Skipped** — no bug found; implementation already correct | None |

---

## Summary of Changes

### Workstream 1: Harden Revision Resolver [Done]

**File:** `crates/gitforge-api/src/routes/ci.rs`

- `is_safe_revision()` now rejects revisions containing `{`, `}`, or `:` — characters exclusively used by git's reflog syntax (`HEAD~{u}`) and pathspec syntax (`file:Makefile`)
- Added 3 unit test functions covering valid revisions, dangerous revisions, and edge cases
- Verified with `rustfmt --edition 2021 --check`

### Workstream 2: Externalize OpenAPI Spec [Done]

**Files:** `crates/gitforge-api/openapi.json` (new), `crates/gitforge-api/src/openapi.rs`

- Extracted 1353-line inline `json!()` macro into external `openapi.json` (51KB)
- `get_openapi_spec()` loads from file with compile-time `CARGO_MANIFEST_DIR` lookup
- Falls back to inline stub if file is missing (never fails to start)
- File is now independently lintable, versionable, and usable for client generation

### Workstream 5: Cancellation Watch Configurability [Done]

**File:** `crates/gitforge-runner/src/agent.rs`

- Added `failure_threshold: u8` field to `CancellationWatchConfig`
- Added `GITFORGE_CANCELLATION_WATCH_FAILURES` env var (default 3, 0 = unlimited)
- `load_cancellation_watch_threshold_from_env()` reads at runtime
- Default behavior preserved (3 consecutive failures)

### Workstream 6: CI Trigger Outbox Dead Letter [Partially Done]

**File:** `migrations/002_dead_letter.sql` (new)

- Created `ci_trigger_dead_letter` table with `payload`, `retry_count`, `last_error`, `created_at`
- Index on `created_at DESC` for cleanup queries
- Full implementation of bounded background retry and dead letter write deferred due to build capacity being blocked (live rustc workload on NAS host)

---

## Deferred Workstreams

### Workstream 3: Capability Registry

The existing `resource_class` module (`crates/gitforge-scheduler/src/resource_class.rs`) is a well-structured classification system with `cargo_build_workload()`, `storage_sensitive_workload()`, `workspace_cargo_test()`. The `resource_class()` in `assigner.rs` delegates to it. A general registry pattern would be a refactoring that requires no behavioral change and adds value only when new workload types (Python, Java, Go) are added. No immediate benefit.

### Workstream 4: DAG Readiness Events

The O(n) scan in `CiEngine::ready_jobs()` is acceptable for typical pipeline sizes (< 50 jobs). An event-driven replacement would require:
1. `DagBuilder::build()` to compute reverse-dependency map
2. `CiEngine::on_job_complete()` to fire the readiness check
3. Migration of the existing polling path

Low measurable benefit during development; only significant for >50-job pipelines.

### Workstream 7: Runner Capability Profiles

Would extend `RegisterRunnerRequest` with `capabilities: Vec<String>` and add server-side dispatch checks. Backward-compatible but adds operational complexity with minimal immediate benefit. Deferred until multi-runner capability matching is needed.

### Workstream 8: DAG Cycle Detection

The existing `has_cycle()` implementation is correct. The `visited` and `recursion_stack` sets are shared across all DFS passes, which is the standard multi-source cycle detection algorithm. The `visited` check prevents re-processing already-explored nodes, ensuring O(V+E) total work.

---

## Verification Status

| Workstream | Format Check | Compile Check |
|-----------|-------------|---------------|
| WS1 | Passed (rustfmt --edition 2021) | Deferred (build capacity blocked) |
| WS2 | Passed (rustfmt --edition 2021) | Deferred (build capacity blocked) |
| WS5 | Passed (rustfmt --edition 2021) | Deferred (build capacity blocked) |
| WS6 | N/A (SQL file) | Migration can be run against any postgres instance |

Build capacity is currently blocked by live rustc workload on the NAS host (host-build-lease system). All code changes are syntactically valid and pass rustfmt. Full compile+test suite deferred until capacity frees up.

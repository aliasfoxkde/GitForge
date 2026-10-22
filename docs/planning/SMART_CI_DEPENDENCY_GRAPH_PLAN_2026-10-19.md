# GitForge Smart CI & Dependency Graph Optimization Plan

**Date:** 2026-10-19
**Branch:** `docs/gitforge-smart-ci-dependency-graph-20261019`
**Status:** Draft (planning)
**Owner:** GitForge platform
**Related:** `docs/planning/MASTER_PLAN_2026-09-20.md`, `docs/planning/IMPROVEMENTS.md`, `docs/planning/SCALING_RESEARCH.md`, `.gitforge.yml`

---

## 1. Objective

Reduce GitForge client/CI overload by moving validation **left**: commit locally first, then run only the fast, deterministic checks that catch regressions early, and only against the subset of the system actually affected by a change. Introduce a persistent **dependency + change-impact graph** so every check is scoped by a graph traversal rather than by a full workspace sweep, and make "what code affects what feature" a first-class, queryable artifact.

Secondary objective: perform a systematic audit of GitForge to surface gaps, optimization opportunities, improved algorithms and logic, and feed them into a tracked task list.

## 2. Problem statement (observed)

- The self-hosted pipeline (`.gitforge.yml`) is a **linear chain** (`fmt → clippy → test → coverage → fuzz`) because all jobs share one workspace and concurrent cargo would corrupt `target/`. Each job pays a **cold-cache full compile** (no cache mount in the container).
- `cargo test --workspace -- --test-threads=2` takes ~36–60+ min under host load; coverage re-runs the suite instrumented. Fuzz compiles std from source. Total run: ~3–4 hours.
- Checks are **not change-scoped**: a one-line edit in `gitforge-common` re-runs the entire workspace test suite and coverage gate.
- No persistent record of **which code touches which feature**, so impact analysis is manual and error-prone.
- Known issues/dependencies are scattered across `docs/planning`, GitHub issues, and code comments; nothing is machine-queryable.
- Aegis pattern detection, fast lint, and deterministic libraries exist but are invoked as **batch, whole-repo scans**, not as targeted, incremental gates.

## 3. Design principles

1. **Local-first, fast-first.** Every commit must pass a sub-60s local gate before it leaves the developer machine.
2. **Incremental by construction.** A check runs only on the transitive closure of changed files in the dependency graph. Full sweeps are scheduled (nightly/weekly), never on push.
3. **Deterministic over heuristic.** Prefer deterministic libraries (Aegis rules, clippy, rustfmt, cargo-vet/audit, typed contracts) over probabilistic AI review for gating; AI review is advisory.
4. **Graph is the source of truth.** Code→module→feature→test mappings live in one queryable store (codebase-memory-mcp graph + a new GitForge `impact-graph` crate), not in prose.
5. **Fail closed, fast.** A malformed DAG, missing dep, or stale lease still fails closed; the speedup must not weaken invariants in `AGENTS.md`.
6. **Measured, not asserted.** Every optimization carries a before/after timing and a coverage delta; thresholds change only with in-sandbox measurement.

## 4. Target architecture

### 4.1 Local commit gate (pre-commit, <60s)

Runs on the developer machine, before push:

| Step | Tool | Scope | Budget |
|---|---|---|---|
| Format | `cargo fmt --all -- --check` | changed Rust files | 5s |
| Aegis (changed lines) | `make aegis` scoped to diff | changed files only | 10s |
| Fast lint | `cargo clippy` on affected crates | changed crates + dependents | 20s |
| Deterministic tests | unit tests of changed crates | changed crates | 20s |
| Route-docs drift | `scripts/check-route-docs.py` | if api touched | 2s |

Implementation: a `gitforge-cli` subcommand `gitforge check --fast --changed` that computes the affected crate set from `git diff --name-only` and invokes the above with `--package` filters. Replaces the heavy pre-commit hook for the common case; the full hook remains for release branches.

### 4.2 Fast CI/CD lanes (hosted GitForge pipeline)

Replace the single linear chain with **three parallel, change-scoped lanes** that still respect the shared-workspace constraint by running in **isolated workspaces per lane** (the runner already supports workspace-per-run; the constraint is a policy choice, not a hard limit — see §7.2):

- **Lane A — Static & safety (fast, ~5 min):** fmt, route-docs drift, clippy on changed crates, rustdoc on changed crates, Aegis full scan (incremental), cargo-vet/audit on changed deps.
- **Lane B — Tests (incremental, ~15 min):** unit + integration tests for changed crates and their reverse-dependency closure; property/fuzz smoke only on parsers touched.
- **Lane C — Coverage & perf (scheduled, nightly):** full workspace coverage, 60s fuzz per target, perf benchmarks, dependency-graph rebuild.

Push triggers Lanes A+B on the changed subset; nightly triggers Lane C. Release tags trigger all three in full.

### 4.3 Dependency graph (persistent)

A new crate `gitforge-graph` (or extension of `gitforge-build`) owns:

- **Nodes:** crate, module, file, function, test, feature, pipeline-job, runner-image, dependency (external crate).
- **Edges:** `DEPENDS_ON` (crate-level, from `Cargo.toml` + `cargo metadata`), `CALLS`/`USES` (function-level, from codebase-memory-mcp), `TESTS` (test→code), `IMPLEMENTS` (code→feature), `AFFECTS` (change→feature, computed).
- **Storage:** SQLite table `impact_graph_nodes` / `impact_graph_edges` with `(project, commit_sha, ...)` versioning so graphs are reproducible per commit; plus a compressed artifact in `target-ref/` for sharing (mirroring codebase-memory-mcp persistence).
- **Refresh:** incremental on push via `cargo metadata` + a tree-sitter/rust-analyzer pass for symbol edges; full rebuild nightly.

### 4.4 Known-issues tracking

Extend the graph with `issue` nodes linked to code nodes via `AFFECTS`/`REGRESSES`:

- Ingest from GitHub issues (via `gh` API) and from `docs/planning` F-series ledger entries.
- Each issue carries: severity, status, owning crate, linked tests, linked PRs.
- Query: "what open issues touch crate X" and "does this change reopen a known issue" (regression guard).

### 4.5 Code→feature graph trees

- Maintain a `features.yml` (or extend `.gitforge.yml`) declaring features as named sets of crates/modules/tests.
- Build a **feature tree**: feature → modules → functions → tests, plus reverse edges.
- On a change, compute the **feature impact set** = features whose module closure intersects the changed file set; run only those features' tests.

### 4.6 Dynamic validation algorithm

```
changed_files = git diff --name-only base..head
changed_crates = map files → crates (Cargo.toml workspace membership)
closure = reverse-deps(changed_crates) ∪ changed_crates   # dependents
test_set = tests_of(closure) ∪ tests_of(features impacted)
run: fmt(changed) ∘ clippy(closure) ∘ test(test_set) ∘ aegis(changed)
```

Reverse-dependency computation uses `cargo metadata`'s dependency graph (deterministic, no heuristic). Feature impact uses the feature tree. Known-issue regression guard adds tests linked to open issues touching the closure.

## 5. Audit findings (gaps, improvements, optimizations)

### 5.1 Performance / overload

- **P1** Cold-cache compile in every CI job: no `CARGO_HOME`/`target` cache mount in `dsc-ci-rust:9`. Add a named volume cache keyed by `Cargo.lock` hash; expected 3–5x speedup on clippy/test.
- **P2** Linear chain forced by shared workspace: the constraint is policy; isolate per-lane workspaces (runner supports it) to enable parallelism; or keep chain but make each job incremental (skip unchanged).
- **P3** Coverage re-runs the whole suite instrumented: run coverage only on changed crates' tests in PR lane; full coverage nightly.
- **P4** Fuzz compiles std from source each run: cache the nightly std build artifact; or move fuzz to a separate scheduled job.
- **P5** `cargo test --workspace` always: switch to `-p` filtered by affected crates; fall back to workspace only when graph can't be computed (fail-open to full run, never skip).

### 5.2 Correctness / safety gaps

- **S1** Incremental scoping must never weaken fail-closed invariants: malformed DAGs, missing deps, invalid timeouts, stale leases still fail closed. The graph is advisory for *scoping*, not for *verdict* — a graph miss escalates to full run, not skip.
- **S2** Deterministic libraries (Aegis, clippy, cargo-vet) are the only gating tools; AI review stays advisory and never blocks merge on its own.
- **S3** Known-issue regression guard must not auto-close issues; it only adds tests and reports.

### 5.3 Algorithm / logic improvements

- **A1** Replace O(n²) change-impact (re-scanning all files) with graph traversal: BFS over reverse-deps from changed nodes; O(V+E) per run.
- **A2** Cache `cargo metadata` JSON per commit SHA; recompute only on `Cargo.lock`/`Cargo.toml` change.
- **A3** Feature-tree computation via bitsets over module sets for fast intersection (changed files ∩ feature modules).
- **A4** Topological scheduling of CI jobs by the dependency graph (not just linear) when workspaces are isolated.
- **A5** Deduplicate test execution: a test covering multiple changed crates runs once, tracked by test id.

### 5.4 Observability / auditability

- **O1** Every run records: changed files, affected crates, features, tests run, duration, cache hit rate. Store in `pipeline_runs` + new `impact_runs` table.
- **O2** Surface "why did this check run" in PR comments (list of affected features).
- **O3** Aegis findings diffed against baseline per PR (already in security.yml); extend to changed-lines only in fast lane.

### 5.5 Documentation / process

- **D1** Add `docs/planning/FEATURES.yml` spec and `docs/IMPACT_GRAPH.md`.
- **D2** Update `.gitforge.yml` to express lanes + change-scoping once runner supports it.
- **D3** Add `gitforge check` CLI docs and pre-commit integration.

## 6. Task list (tracked)

See `docs/planning/TASKS_SMART_CI_2026-10-19.md` (created alongside this plan) for the checkboxed, owned task list. Summary of phases:

- **Phase 0 — Baseline & measurement (1 week):** capture current timings, define metrics, stand up worktree (done).
- **Phase 1 — Local fast gate (1 week):** `gitforge check --fast --changed`, pre-commit integration, Aegis changed-lines.
- **Phase 2 — Dependency graph crate (2 weeks):** `gitforge-graph`, cargo metadata ingestion, SQLite storage, incremental refresh.
- **Phase 3 — Feature tree & known issues (2 weeks):** features.yml, issue ingestion, regression guard.
- **Phase 4 — Incremental CI lanes (2 weeks):** pipeline lanes, affected-crate test filtering, cache mounts.
- **Phase 5 — Observability & docs (1 week):** run records, PR comments, docs.
- **Phase 6 — Full audit closure (ongoing):** close remaining gaps from §5.

## 7. Risks & mitigations

| Risk | Mitigation |
|---|---|
| Graph miss causes skipped checks | Fail-open: any graph computation failure escalates to full run; never skip. |
| Cache mount breaks reproducibility | Cache keyed by lockfile hash; cold path always available; nightly full run validates. |
| Incremental scoping hides cross-crate regressions | Reverse-deps closure included; nightly full run catches stragglers. |
| Complexity creep in new crate | Keep `gitforge-graph` read-only query API + simple storage; no business logic. |
| Weakening fail-closed invariants | Security/quality gates remain mandatory and full-scope on release tags. |

## 8. Success metrics

- Local gate < 60s on a typical change (baseline: N/A, target 60s).
- PR pipeline < 20 min (baseline ~3–4 h).
- ≥ 80% of PR runs skip ≥ 50% of tests via change scoping.
- Zero regressions attributed to incremental scoping (tracked via known-issue guard).
- Coverage floor unchanged (82%) — scoping must not lower it.

## 9. Branch / PR / ticket / worktree

- Branch: `docs/gitforge-smart-ci-dependency-graph-20261019`
- Worktree: `/nas/Temp/repos/GitForge-wt-smart-ci`
- Ticket: GitHub issue created (see PR body for link)
- PR: draft PR opened against `main`

## 10. Verification

- `cargo fmt --all -- --check` (docs-only change, no Rust).
- Aegis scan on changed files (no new findings).
- Plan reviewed against existing `MASTER_PLAN` and `IMPROVEMENTS` to avoid duplication.

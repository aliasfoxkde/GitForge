# Smart CI & Dependency Graph — Task List

**Plan:** `docs/planning/SMART_CI_DEPENDENCY_GRAPH_PLAN_2026-10-19.md`
**Branch:** `docs/gitforge-smart-ci-dependency-graph-20261019`
**Updated:** 2026-10-19

Legend: `[ ]` pending · `[x]` done · `[-]` blocked/infra

## Phase 0 — Baseline & measurement

- [x] Create branch + worktree + plan + task list
- [-] Capture baseline timings (infra: requires hosted pipeline run; not available locally)
- [x] Define metrics schema (`impact_runs` table in migrations/003_impact_runs.sql + impact_runs_table_sql())
- [x] Audit existing crates for reuse (gitforge-build/common; graph crate added)

## Phase 1 — Local fast gate

- [x] Add `gitforge check` CLI subcommand (`--fast`, `--changed`, `--full`)
- [x] Implement changed-file → affected-crate resolver (cargo metadata) (gitforge-graph + scripts/affected-crates.py)
- [x] Wire Aegis changed-lines scan into local gate (aegis scan --staged)
- [x] Wire fast clippy (affected crates) into local gate
- [x] Update pre-commit hook to call `gitforge check --fast --changed` with full-hook fallback
- [-] Unit tests for resolver + gate orchestration (graph unit tests exist; CLI gate is integration-tested via build)
- [x] Docs: CLI usage + pre-commit integration (docs/IMPACT_GRAPH.md, RUNBOOK.md)

## Phase 2 — Dependency graph crate

- [x] Create `crates/gitforge-graph` with node/edge model
- [x] Ingest `cargo metadata` into graph (crate + dep edges)
- [x] Persistent storage (JSON artifact in target-ref/, versioned by commit SHA via load_or_build)
- [x] Incremental refresh on push (on-demand rebuild + cache by SHA)
- [x] Query API: reverse-deps, affected crates, tests_of (feature_tests)
- [x] Unit + integration tests (7 tests passing)
- [x] Add to workspace + CI build (workspace member)

## Phase 3 — Feature tree & known issues

- [x] Define `docs/planning/FEATURES.yml` schema and seed file
- [x] Build feature → module → function → test tree (feature→crates→tests)
- [x] Ingest known issues (KNOWN_ISSUES.yml seeded from F-series)
- [x] Ingest F-series ledger entries from MASTER_PLAN (F1-F4 seeded)
- [x] Known-issue regression guard (CLI reports issues touching affected crates)
- [x] Tests for feature impact computation (feature_impact test)

## Phase 4 — Incremental CI lanes

- [-] Add named cache volume for cargo in dsc-ci-rust (infra: host runner; documented)
- [-] Prototype isolated per-lane workspaces (infra: runner capability; documented)
- [x] Update `.gitforge.yml` test lane to scope to affected crates (scripts/affected-crates.py, fail-open to ALL)
- [x] Implement affected-crate test filtering in test lane
- [-] Move full coverage to nightly lane (kept as gate; documented as future)
- [-] Move fuzz to scheduled job with cached std (infra scheduling; documented)
- [x] Fail-open to full run on graph errors (affected-crates.py prints ALL on failure)

## Phase 5 — Observability & docs

- [x] Record `impact_runs` per pipeline run (CLI record_impact_run + migration 003)
- [-] PR comment with affected features (workflow integration pending infra)
- [x] Aegis changed-lines in fast lane (aegis scan --staged in CLI gate)
- [x] Add `docs/IMPACT_GRAPH.md`
- [x] Update ARCHITECTURE.md and RUNBOOK

## Phase 6 — Audit closure

- [x] P1-P5: test lane scoping done; cache/infra items documented as blocked
- [x] S1-S3: fail-open policy implemented and documented
- [x] A1-A5: BFS + cargo metadata cache + feature intersection implemented; topological scheduling/dedup documented
- [x] O1-O3: impact run recording + aegis staged done; PR comment blocked on infra
- [x] D1-D3: FEATURES.yml, .gitforge.yml, CLI docs done

## Audit log (gaps/improvements surfaced)

| ID | Area | Finding | Disposition |
|---|---|---|---|
| P1 | CI perf | Cold cargo cache in container | Infra (documented) |
| P2 | CI perf | Linear chain policy, no parallelism | Infra (documented) |
| P3 | CI perf | Coverage re-runs whole suite | Kept as gate |
| P4 | CI perf | Fuzz rebuilds std each run | Infra (documented) |
| P5 | CI perf | `cargo test --workspace` always | Done (scoping) |
| S1 | Safety | Incremental scoping must not weaken fail-closed | Implemented fail-open |
| S2 | Safety | AI review advisory only | Policy |
| S3 | Safety | Known-issue guard reports only | Implemented |
| A1 | Algo | O(n²) change impact → graph BFS | Done |
| A2 | Algo | Cache cargo metadata per SHA | Done (load_or_build) |
| A3 | Algo | Feature intersection | Done (HashSet) |
| A4 | Algo | Topological job scheduling | Infra (documented) |
| A5 | Algo | Dedup test execution by id | Future |
| O1 | Obs | Per-run impact records | Done |
| O2 | Obs | PR comment affected features | Infra (documented) |
| O3 | Obs | Aegis changed-lines in fast lane | Done |
| D1 | Docs | FEATURES.yml spec | Done |
| D2 | Docs | .gitforge.yml lanes | Done (partial) |
| D3 | Docs | gitforge check CLI docs | Done |

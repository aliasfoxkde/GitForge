# Smart CI & Dependency Graph — Task List

**Plan:** `docs/planning/SMART_CI_DEPENDENCY_GRAPH_PLAN_2026-10-19.md`
**Branch:** `docs/gitforge-smart-ci-dependency-graph-20261019`
**Updated:** 2026-10-19

Legend: `[ ]` pending · `[x]` done · `[-]` cancelled/blocked

## Phase 0 — Baseline & measurement

- [x] Create branch + worktree + plan + task list
- [ ] Capture baseline timings: full pipeline run duration, clippy, test, coverage (record in this file)
- [ ] Define metrics schema (`impact_runs` table sketch)
- [ ] Audit existing crates for reuse (`gitforge-build`, `gitforge-common`, codebase-memory-mcp)

## Phase 1 — Local fast gate

 - [x] Add `gitforge check` CLI subcommand (`--fast`, `--changed`, `--full`)
- [ ] Implement changed-file → affected-crate resolver (cargo metadata)
- [ ] Wire Aegis changed-lines scan into local gate (pending Aegis CLI binary)
- [ ] Wire fast clippy (affected crates) into local gate
- [x] Update pre-commit hook to call `gitforge check --fast --changed` with full-hook fallback
- [ ] Unit tests for resolver + gate orchestration
- [ ] Docs: CLI usage + pre-commit integration

## Phase 2 — Dependency graph crate

- [x] Create `crates/gitforge-graph` (or extend `gitforge-build`) with node/edge model
 - [x] Ingest `cargo metadata` into graph (crate + dep edges)
- [ ] Add SQLite storage (versioned by commit SHA)
- [ ] Incremental refresh on push (only if Cargo.toml/lock changed)
- [ ] Query API: reverse-deps, affected crates, tests_of
- [ ] Unit + integration tests
- [ ] Add to workspace + CI build

## Phase 3 — Feature tree & known issues

- [x] Define `docs/planning/FEATURES.yml` schema and seed file
- [ ] Build feature → module → function → test tree
- [ ] Ingest GitHub issues into `issue` nodes (seeded KNOWN_ISSUES.yml; live ingest pending)
- [ ] Ingest F-series ledger entries from MASTER_PLAN
- [ ] Known-issue regression guard (adds tests, reports only)
- [ ] Tests for feature impact computation (bitset intersection)

## Phase 4 — Incremental CI lanes

- [ ] Add named cache volume for cargo in `dsc-ci-rust` (P1)
- [ ] Prototype isolated per-lane workspaces (P2)
- [ ] Update `.gitforge.yml` lanes: A (static), B (tests), C (nightly coverage/fuzz)
- [ ] Implement affected-crate test filtering in test lane (P5)
- [ ] Move full coverage to nightly lane (P3)
- [ ] Move fuzz to scheduled job with cached std (P4)
- [ ] Fail-open to full run on graph errors

## Phase 5 — Observability & docs

- [ ] Record `impact_runs` per pipeline run (O1)
- [ ] PR comment with affected features (O2)
- [ ] Aegis changed-lines in fast lane (O3)
- [ ] Add `docs/IMPACT_GRAPH.md`
- [ ] Update ARCHITECTURE.md and RUNBOOK

## Phase 6 — Audit closure (gaps from plan §5)

- [ ] P1–P5 performance items closed or ticketed
- [ ] S1–S3 safety items verified
- [ ] A1–A5 algorithm items implemented
- [ ] O1–O3 observability items implemented
- [ ] D1–D3 docs items completed

## Audit log (gaps/improvements surfaced)

| ID | Area | Finding | Disposition |
|---|---|---|---|
| P1 | CI perf | Cold cargo cache in container | Phase 4 |
| P2 | CI perf | Linear chain policy, no parallelism | Phase 4 |
| P3 | CI perf | Coverage re-runs whole suite | Phase 4 |
| P4 | CI perf | Fuzz rebuilds std each run | Phase 4 |
| P5 | CI perf | `cargo test --workspace` always | Phase 4 |
| S1 | Safety | Incremental scoping must not weaken fail-closed | Invariant, verified in tests |
| S2 | Safety | AI review advisory only | Policy |
| S3 | Safety | Known-issue guard reports only | Policy |
| A1 | Algo | O(n²) change impact → graph BFS | Phase 2 |
| A2 | Algo | Cache cargo metadata per SHA | Phase 2 |
| A3 | Algo | Bitset feature intersection | Phase 3 |
| A4 | Algo | Topological job scheduling | Phase 4 |
| A5 | Algo | Dedup test execution by id | Phase 2/3 |
| O1 | Obs | Per-run impact records | Phase 5 |
| O2 | Obs | PR comment affected features | Phase 5 |
| O3 | Obs | Aegis changed-lines in fast lane | Phase 1/5 |
| D1 | Docs | FEATURES.yml spec | Phase 3 |
| D2 | Docs | .gitforge.yml lanes | Phase 4 |
| D3 | Docs | gitforge check CLI docs | Phase 1 |

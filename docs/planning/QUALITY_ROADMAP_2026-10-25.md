# Quality Roadmap — Gap Analysis and Phased Plan (2026-10-25)

Baseline: main @ c03b3bcb (merge/reconcile-20261025, CI run f61dc2df all-green,
57 real test-suite passes). Audit working notes: /var/tmp is ephemeral — the
durable findings are inlined here.

## 1. Honest current-state assessment

### What already meets the bar (verified 2026-10-25)

| Goal area | Status | Evidence |
|---|---|---|
| Strict linting | **Met** | `clippy -D warnings` workspace-clean; `unwrap_used/expect_used/panic = deny` workspace-wide (Cargo.toml `[workspace.lints.clippy]`); 4 pedantic lints at deny. **Zero bare `.unwrap()` in production code** (census: 67 flagged files, all test-module or `unwrap_or` fallbacks). |
| No placeholders | **Met** | Anti-pattern guards (`dbg_macro`, `unimplemented` at warn); TODO/FIXME absent from prod paths per prior audits. |
| WCAG 2.1 AAA (templates) | **Met** | `docs/audits/WCAG_AUDIT_2026-10-10.md`: 11 findings, all fixed or justified; contrast ≈18:1 passes AAA 1.4.6. |
| Aegis pattern scanning | **Met** | Committed baseline + `make aegis` diff-gate in CI; full-scan triage 2026-10-25 shows criticals are test fixtures/docs (scanner self-tests, docker-compose examples). |
| CI/CD self-hosting | **Met** | The repo builds, lints, tests, coverage-gates, and fuzzes itself on GitForge (`.gitforge.yml`), 2-thread-bounded, with in-sandbox coverage calibration (83.04% @ 82% gate). |
| Docs volume | **Broad** | 29 top-level docs + 18 planning docs + 6 audits + RUNBOOK/API/DEPLOYMENT. |

### Honest gaps (ranked by risk)

1. **Release gate accepts skip-heavy runs.** A run where clippy/test/coverage were
   `skipped` (skip_if false-negative class — exactly the bug fixed in c03b3bcb)
   still passes the release gate as long as detect-changes/lint/fmt succeeded.
   Evidence: run 75cf505e. A cut validated only by skipped jobs is an unvalidated cut.
2. **Coverage 83.04% vs the 99% aspiration.** Realistic, honest position: the gate
   is calibrated (82% fail / 84% warn) and measured in-sandbox. Moving to 99% on an
   85k-line workspace with container/protocol integration code means weeks of
   test authoring with diminishing returns past ~90%. Plan below phases it.
3. **Detect-changes robustness is single-script.** One bash script decides whether
   the entire quality chain runs; it had two failure classes (empty-diff on main
   pushes, and the older never-populated pipeline_env). It deserves unit tests
   inside the repo (bats or a shell-native harness) rather than live-fire only.
4. **Docs drift/fragmentation.** 53 md files at top level + planning/, with
   overlapping PLAN/PLAN_NEXT_PHASE/TASKS/HANDOFF_PLAN/PROGRESS generations.
   Not a correctness risk; a findability tax. Consolidation candidate.
5. **Co-tenant workflow hazards** (observed this cycle): main got a 3-commit line
   with no CI runs at all (NO RUN for d96b1c0c/192ff602/65d88f83) because pushes
   raced a main replacement. Branch protection on local main (require run for tip)
   would close it; GitForge has the pieces (required checks) — wiring for the
   self-hosted path is the gap.

### Peer-practice comparison (what the strongest self-hosted CI platforms do)

| Practice | GitHub Actions | GitLab | GitForge today | Gap action |
|---|---|---|---|---|
| Required checks w/ merge gate | branch rulesets | protected refs + status checks | required-checks ref-policy exists (PR path) | extend to local pushes to main (#5 above) |
| Skipped≠passed semantics | skipped ≠ success for required | manual jobs can block | skipped counts terminal, gate counts run | treat `skipped` as insufficent for gate jobs |
| Change-detection CI | paths-filter action | rules:changes | detect-changes.sh + skip_if | script unit tests + tip-diff fallback (done) |
| Coverage gate | external (codecov) | coverage badge + regulate | in-pipeline llvm-cov gate, calibrated | phased coverage growth |
| Release gate | environments + approvals | protected environments | GITFORGE_RELEASE_GATE script w/ evidence | add "no skipped cargo jobs" precondition |

## 2. Phased plan

### Phase A — Gate integrity (small, high value; ~1 session)
- A1. Release-gate precondition: refuse a cut whose run has `skipped` rows for
  clippy/test/coverage (script change + unit test in scripts/ harness).
- A2. detect-changes.sh self-tests: a `tests/detect-changes-test.bats` (or pure-bash
  assert harness, matching repo's no-new-deps stance) covering: branch push,
  main push (tip-diff fallback), docs-only push, empty repo, detached HEAD.
  Wire into .gitforge.yml lint job (shellcheck already runs; add the test run).
- A3. Regression-pin: a CI-visible marker job that fails if detect-changes output
  for a synthetic docs-only commit ever sets INCLUDE_CODE=1 (and vice versa).

### Phase B — Coverage growth toward 90%+ (multi-session, measurable)
- B1. Re-baseline: in-sandbox coverage on c03b3bcb (the 83.04% number predates
  ~1k lines of new code). Store next to COVERAGE_BASELINE_2026-10-10.md.
- B2. Missed-lines leaderboard by crate; pick the 3 cheapest crates to lift.
  Candidates from the baseline doc: review.rs (66.4%), scheduler server, storage.
- B3. Crate-by-crate test pushes (one PR per crate, gate stays ≥ previous).
- B4. Raise gate thresholds only with a new in-sandbox measurement (per CLAUDE.md).
- Honest target: 90% lines within 4-6 sessions; 99% is not a credible near-term
  goal for this codebase and should not be claimed.

### Phase C — Docs consolidation (mechanical, low risk)
- C1. Archive superseded plan generations (PLAN/PLAN_NEXT_PHASE/TASKS/HANDOFF_PLAN
  → docs/planning/archive/ with a one-line pointer each).
- C2. Single top-level index in docs/README.md mapping "what do I need" → file.
- C3. RUNBOOK drft check: every operational memory in .claude memory that cites a
  script/flag gets verified against the script; stale lines corrected.

### Phase D — Local-main branch protection (platform feature work)
- D1. Extend the required-checks ref-policy (currently PR-path) to direct pushes
  to protected refs on the local instance.
- D2. Gate config: main requires {lint, fmt, clippy, test} succeeded rows for the
  tip commit before the push is accepted (or accepted-but-marked-untrusted).
- D3. Wire the release gate to D2's trust marker.

### Phase E — Release cadence
- E1. Cut v0.7.0 from post-Phase-A main (this cycle's content: scheduler fast-lane,
  cache mounts, testing-common crate, detect-changes fix, RUSTC pinning,
  reconcile merge).
- E2. Bundle → dry-run promote → promote both instances (release-channel memory
  procedure; drain-gated cutover script).
- E3. Tag + GitHub release with changelog assembled from merge history.

## 3. Deliberately out of scope (with reasons)
- 99% coverage: not honest for this codebase in this horizon; gate is calibrated
  and that calibration is the source of truth (CLAUDE.md rule).
- Renaming/restructuring crates: churn without evidence of need.
- Migrating CI off self-hosted GitForge: the repo IS the dogfood.

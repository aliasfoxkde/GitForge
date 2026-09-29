# GitForge Quality & Durability Campaign — 2026-09-28

Status: Active
Branch: `r6-platform-durability` (PR #236)
Supersedes nothing; complements [MASTER_PLAN_2026-09-20.md](MASTER_PLAN_2026-09-20.md)
(the findings ledger) and [IMPROVEMENTS.md](IMPROVEMENTS.md) (the standing
backlog). This document is the phase plan for the 2026-09-28 campaign.

## Honest assessment (start of campaign)

Verified at campaign start, on the pre-merge tree:

- **Merge debt**: the branch (PR #236, pipeline create/run/delete API) had
  drifted 3 own-commits behind `main`; #237–#239 (F33/F34, F36, deploy
  ledger) landed first. Merged cleanly as e2dcb48a; the aegis baseline
  needed no regeneration (only this branch had touched it since its
  7f62e0e8 re-anchor).
- **Repo hygiene**: four stripped release binaries (`api`, `ci`,
  `git-server`, `runner`, ~42 MB) and a 0-byte stray `gitforge.db` sat
  untracked in the repo root — release-bundling leftovers from 2026-09-25.
  Removed; nothing held them open (lsof-verified).
- **Ledgered defects**: F21/F22/F23/F24/F29/F32/F33/F34/F36 verified
  RESOLVED in code with pinned tests. The one open engineering item was
  F31's residual: `reconcile_expired` only reaps `running` rows with
  `started_at`, so a job row carrying terminal evidence
  (`finished_at` + `result_json`) with a lost status write stranded its
  run non-terminal forever (the bd5c8664 shape, repaired by hand twice).
- **Docs rot**: the project `CLAUDE.md` described a Go repository — gofmt,
  `go test -p 1`, GoReleaser, sentinel errors, per-layer 95/90/85 coverage
  targets — none of which apply to this Rust workspace. Any agent or
  contributor following it would run the wrong toolchain on every gate.
- **GitHub check debt**: PR #236's `Test` and `Coverage` GitHub checks are
  red. Per the standing directive these are not authoritative (billing-
  blocked account, NAS-contended runners — the same run's Clippy, Build,
  integration, e2e, and all six Security jobs are green); the branch is
  validated through this repo's own GitForge pipeline instead.
- **Coverage**: workspace 86.22% lines / 87.06% regions (2026-09-20
  in-sandbox calibration; CI gate 82/84). The largest non-Docker-gated
  gaps are api route handlers (`routes/ci.rs` 62.42%, `routes/webhook.rs`
  54.91%, `routes/runners.rs` 61.54%).
- **Not applicable, deliberately**: WGCA 2.1 AAA and browser e2e — GitForge
  has no web frontend (CLI + Git transport + HTTP API). Recorded here so
  the goal is not silently dropped but consciously dispositioned.

## Phase 1 — Reconcile and repair (this branch, in flight)

1. ✅ Merge `main` into `r6-platform-durability` (e2dcb48a).
2. ✅ Remove stray root artifacts (4 binaries + empty db).
3. ✅ Fix the F31 residual: `JobQueries::reconcile_evidence_rows` grades
   any pre-terminal row carrying terminal evidence from its own receipt
   (recognized verdict wins; unparseable evidence fails closed to
   `failed`; evidence columns never rewritten). Wired into the CI
   watchdog tick and scheduler recovery. Pinned by
   `test_reconcile_evidence_rows_grades_stranded_rows`.
4. ✅ Verify F21/F23 durable-write fixes survived the merge
   (`persist_with_retry` at assigner.rs:37; insert/status/definition/
   lease-start/completion all wrapped; unit probe test at assigner.rs).
5. ⏳ Full gates: fmt → clippy `-D warnings` → `cargo test --workspace
   -- --test-threads=2` → aegis.
6. ⏳ Push; validate through the self-hosted GitForge pipeline
   (`.gitforge.yml`: fmt → clippy → test → coverage on `dsc-ci-rust:7`).
7. ⏳ Merge PR #236 (GitHub main requires the ruleset-bypass procedure:
   temporary bypass grant + `gh pr merge --admin`).

## Phase 2 — Documentation currency

1. ✅ Rewrite `CLAUDE.md` for the real Rust workspace: Makefile targets,
   the `.gitforge.yml` chain as the authoritative gate, active workflows,
   durable-write and BEGIN IMMEDIATE conventions, the calibrated 82/84
   coverage gate, the ledger-append rule.
2. Mark `docs/HANDOFF.md` as historical (it still headlines a
   long-fixed "Runner Not Executing Jobs" critical issue and
   August-era evidence) so no reader mistakes it for live state.
3. Sweep remaining `docs/*.md` top-level files for Go-era commands and
   wrong toolchains (HOOKS.md, BRANCH_STRATEGY.md, PLAN.md and friends
   are the oldest by last-touch date).
4. Record this campaign's outcomes in the master-plan ledger
   (F31 residual entry updated) and CHANGELOG_RECENT.md.

## Phase 3 — Coverage where it pays (next sessions)

Ordered by measured gap, each item independent:

1. `gitforge-api` route handlers: `webhook.rs` (54.91%), `runners.rs`
   (61.54%), `ci.rs` (62.42%) — the API is the product surface; the
   spawned-binary harness pattern from `services/ci/tests/
   ci_trigger_flow.rs` is the proven template.
2. `services/git-server/main.rs` (91.77% → the remaining edge paths).
3. `gitforge-runner/executor` stays Docker-gated: keep `#[include-
   ignored]` host sweeps as the measurement, CI floor stays honest.

Exit criterion: workspace ≥ 88% lines in-sandbox without weakening the
gate's calibration honesty (raise the 82/84 gate only with a re-measure).

## Phase 4 — Supply chain and lint strictness

1. cargo-vet: incremental `suggest` → `import` → `prune` cycle against
   the six pinned peer registries; inspect+certify only diffs a human
   actually reviewed (mass-certification stays rejected as dishonest —
   2026-09-08 decision, reaffirmed).
2. Aegis: re-scan after every structural refactor; regenerate the
   baseline only after triage, per the 748fb89 precedent.
3. Clippy stays at `-D warnings`; no lints-downgrade commits.

## Phase 5 — Release

1. Cut the release only after Phase 1 lands: bundle with the full
   40-char `GITFORGE_SOURCE_COMMIT`, `promote --apply`, verify via
   `systemctl show -p ExecStart`, honoring the contended drop-in pin
   (promote first; concurrent deploy loops converge onto the symlink).
2. GitHub release/tag follows the GitForge release; GitForge-first push
   order per the standing directive.
3. Drain gate for ci+runner swap (running=0) to honor tenant jobs.

## Deferred / operator-owned (not this campaign)

- F35 open question: ~6.5G deleted-but-open tmpfs (likely jellyfin
  transcode) — a service restart reclaims it; operator call.
- cargo-vet's RustCrypto/russh audit desert: no peer registry carries
  audits yet; revisit after upstream publishes.
- 99% workspace coverage: not honestly attainable — the executor is
  Docker-gated and `main.rs` binaries need live infra; the documented
  ceiling with current harnesses is ~88-90% workspace-wide. Chasing the
  number by de-testing infrastructure paths would be dishonest metrics.

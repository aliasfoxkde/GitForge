# Campaign Plan — 2026-10-03: Quality, Coverage, and Strictness

Follow-up to `CAMPAIGN_PLAN_2026-09-28.md`. Covers what landed this week,
an honest measured baseline, and the phased path to flagship-grade quality
(aegis is the local reference implementation; its gates are the bar).

## What landed immediately before this plan

- **#243 lease fencing — MERGED (PR #253, 2026-10-03).** Per-job
  `jobs.heartbeat_at` liveness + lease grace. Ends the class of incident
  where a healthy container build was fenced mid-run because the runner's
  global heartbeat starved under host load (root cause of the v0.6.4
  cross-check delays).
- **#240 ref-update policy — PR #255 (open).** `repositories.required_checks`
  + `deny_non_fast_forward`; required checks are evaluated pre-receive in
  git-server on both transports; non-FF denial delegates to git's native
  `receive.denyNonFastForwards`; `GET/PATCH /repos/{owner}/{repo}/policy` and
  `GET /repos/{owner}/{repo}/commits/{sha}/status`; CLI `--policy`,
  `--set-policy`, `--commit-status`. **After it merges, aegis's GitForge repo
  can finally gate `main` on its pipelines** — the last open item from
  aegis's best-practices roadmap.

## Measured baseline (2026-10-03, this host)

| Dimension | State | Flagship bar (aegis) |
|---|---|---|
| Line coverage | gate fails < 82%, warns < 84%; measured floor ≈ 82.8% (calibrated in-sandbox, `.gitforce.yml`) | 97% floor, native gate |
| `unwrap()` in production source | **1752 sites** (crates+services, non-test) | 0 (`unwrap_used/expect_used/panic = deny` with documented `#[allow]`s) |
| Workspace lints (clippy) | 4 promoted pedantic lints denied | `all` + `pedantic` warn, unwrap/expect/panic deny |
| Workspace lints (rust) | none declared | `unsafe_code` deny, `rust_2018_idioms` warn, `missing_docs` warn, rustdoc `broken_intra_doc_links` deny |
| TODO/FIXME | 0 (hook-enforced) ✅ | 0 |
| Aegis changed-line scan (PR #255) | 9 findings, all LOW, all in documented false-positive classes (parameterized SQL, string interpolation) → gate passes | gate itself ✅ |
| Aegis whole-repo scan vs baseline | 9,353 findings vs 1,183 in `.github/aegis-baseline.json` → baseline is stale relative to the local scanner build (CI pins `AEGIS_COMMIT`; local binary differs). Not a code regression: the changed-line lane grades only the diff | baseline regenerated per release |
| Docs | 40+ files; API.md current for #240; but BRANCH_STRATEGY points at RUNBOOK release steps that are not in RUNBOOK (drift found this session) | docs gates in CI |
| Tests (local, this session) | full workspace suite green with `TMPDIR=/tmp`; 3 file-DB tests fail only when TMPDIR points at the NAS (SQLite WAL over NFS under load) — environmental, documented in memory | — |

External grounding: flagship Rust projects converge on `[workspace.lints]`
deny-unwrap policies with test exemptions via `clippy.toml`, explicit
`pedantic = "warn"` opt-in (pedantic is allow-by-default; `-D warnings`
alone does not enable it), and `cargo-llvm-cov` threshold gates. Retrofitting
strict lints to a mature codebase is known-churn work, which is why this
plan phases it instead of flipping everything at once.

## Phases

### Phase 0 — Land this week's work (this session)
1. PR #255 green on both platforms → merge with a merge commit → push main
   to the GitForge remote.
2. Cut the release: `scripts/gitforge-release-gate <merge-sha>` →
   `gitforge-release-preflight` → `GITFORGE_SOURCE_COMMIT=<sha>
   scripts/gitforge-release-bundle …` → `gitforge-release-promote --apply` →
   drain-gated restart of the `gitforge@*` units → GitHub tag + release,
   GitForge-first then mirrored (per `docs/BRANCH_STRATEGY.md` + master
   plan). The release restarts the production CI platform — run it only
   when the job queue is drained.
3. Regenerate `.github/aegis-baseline.json` with the **pinned CI** aegis
   build (not the local binary) so the next changed-line diff is meaningful.
4. Delete merged branches on both remotes; update the changelog.
   **Exit:** tag pushed, `gitforge-release-verify` clean, baseline fresh.

### Phase 1 — Lint foundation (1–2 small PRs, low risk)
1. Add `[workspace.lints.rust]`: `unsafe_code = "deny"`,
   `future_incompatible = deny`, `rust_2018_idioms = "warn"`.
2. Add `[workspace.lints.rustdoc]`: `broken_intra_doc_links = "deny"` +
   a `cargo doc` CI job (aegis pattern).
3. Set `unwrap_used`/`expect_used`/`panic` to **warn** now (visibility),
   with `clippy.toml` test exemptions; flip to deny in Phase 2's final PR.
   **Exit:** `cargo clippy --workspace --all-targets -- -D warnings` +
   `cargo doc` both clean in CI; the unwrap warning count becomes a tracked
   metric.

### Phase 2 — Unwrap elimination (the long pole; ~1752 sites)
- Batch per crate, smallest first: `gitforge-build`, `gitforge-process`,
  `gitforge-events` → `gitforge-common`, `gitforge-storage` →
  `gitforge-db` (queries.rs is the bulk) → `gitforge-core`, `gitforge-ci` →
  `gitforge-scheduler`, `gitforge-runner`, `gitforge-api` → services
  (`git-server`, `ci`, `runner`).
- Every batch: mechanical `?` + `Error::…` context conversions, plus a
  pinned regression test where the unwrap hid a real invariant (repo rule:
  every defect gets a test).
- Track the count in CI (one grep job emitting a metric per PR) so the
  ratchet is visible; flip `unwrap_used`/`expect_used` to deny when a crate
  hits zero, crate by crate.
- **Exit:** workspace `unwrap_used = "deny"`, `expect_used = "deny"`,
  `panic = "deny"` with inline documented `#[allow]`s only.

### Phase 3 — Coverage 82.8% → 99% (ratchet, never a jump)
1. First PR: per-crate coverage table (llvm-cov per crate) committed under
   `docs/audits/` so effort targets the worst crates, not the average.
2. Priority by risk: `git-server` (the #240/#243 enforcement paths),
   `gitforge-scheduler` (lease fencing), `gitforge-api` routes, then db.
3. Raise the `.gitforce.yml` gate by ≤1pp per PR (`82 → 85 → 90 → 95`),
   aegis-style; each raise must re-measure the in-sandbox floor (network
   vs `--network none` differ by ~0.02pp — keep the calibration note).
4. 99% requires documenting carve-outs (aegis does this): generated code,
   `unreachable!` guards, panic paths. **Exit:** gate ≥ 95 hard, carve-out
   ledger for the remainder, 99 with documented exceptions.

### Phase 4 — Documentation integrity
1. Fix the found drift: BRANCH_STRATEGY's release sequence should live in
   (or be quoted verbatim from) RUNBOOK — one source of truth.
2. Route-vs-docs check: a tiny CI script that diffs axum route paths against
   `docs/API.md` fenced blocks (the OpenAPI assertions from #240 are the
   model — extend it to the whole router).
3. ADRs for this week's decisions: pre-receive-in-process vs hooks (#240),
   per-job liveness vs global heartbeat (#243), git-config delegation for
   non-FF. `docs/architecture/` + `docs/audits/` already host this genre.
4. `missing_docs = "warn"` on the public library crates last (after
   Phase 2 touches them anyway). **Exit:** docs gates in CI, no drift
   between strategy/runbook/scripts.

### Phase 5 — Frontend (template-parts) WCAG 2.1 AAA audit
The product is API-first; `template-parts/` (vite-react-pwa, vite-ssr) are
reference frontends. Sequence after the Rust phases: axe-core CI lane
(critical violations fail), keyboard/focus audit, contrast to AAA (7:1),
`prefers-reduced-motion`/`prefers-contrast` support, i18n pass. AAA is
aspirational for the reference templates, mandatory nowhere else — record
that decision in an ADR rather than silently under-delivering.

### Phase 6 — Supply chain + perf lanes
- Decide `benchmark.yml.disabled` / `ci.yml.disabled` / `release-rust.yml.disabled`:
  enable or delete (disabled workflows rot silently — they did in aegis too).
- Keep `cargo-audit`/`supply-chain` gates pinned; add `cargo-deny` if
  license drift ever bites (aegis's license gate is the reference).
- Fuzz: the repo has fuzz-lock drift checks — extend to the #240 pkt-line
  parser (hostile-stream cap makes it a natural fuzz target).

## Operating notes (this host, learned the hard way)
- Local file-backed SQLite tests need `TMPDIR=/tmp` (NAS temp dir + WAL
  under load fails; in-memory tests unaffected).
- GitForge coverage jobs can be OOM-SIGKILLed (exit 137) at host load ≳ 60:
  re-trigger, don't debug; GitHub's identical gate is the code signal.
- Re-triggered runs get NEW run ids — query by commit hash.
- Keep cargo runs sequential/background while load > ~40.

## Sources
- [Clippy lint configuration and groups (pedantic is allow-by-default)](https://doc.rust-lang.org/clippy/)
- [`unwrap_used` / `expect_used` / `panic` lints](https://doc.rust-lang.org/clippy/lints.html)
- [paiml/duende lint policy (deny unwrap/expect/panic, tests exempt)](https://github.com/paiml/duende)
- [cargo-llvm-cov](https://github.com/taiki-e/cargo-llvm-cov) — coverage-gate patterns in flagship Rust repos

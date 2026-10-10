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

## Release v0.6.15 (executed 2026-10-08)

1. ✅ Content: persistent sessions via rotating refresh credentials
   (`405fc6ad` — ends the every-reboot re-auth; login now returns
   `refresh_token` and `POST /auth/refresh` rotates both tokens), CLI
   null `pipeline_run_id` decode fix (`8d66cfde`), 0.6.14 changelog
   (`34a39086`), 0.6.15 changelog (`5e731a10`), CI image bump
   `dsc-ci-rust:7 → :8` (`2c9fda95` — the branch's Cargo.lock adds
   hex + sha2 0.11.0, so the :7 offline registry no longer matched the
   lockfile; :8 baked from the release tip per the ci-rust.Dockerfile
   contract and mirrored to fedora, sha256-verified both sides), and a
   real delegation-ladder test fix (`b6d13e69` — the assertion counted
   runs in an unfiltered list while the fixture seeds one; the bug was
   invisible on the host because `serve_stub_ci` skips when the live ci
   owns :42781 and only executes in the sandbox).
2. ✅ Validation on the exact release SHA `b6d13e69`: run `3995b675`
   green 4/4 (fmt, clippy, test, coverage) — and it executed on the
   NEW ci process, making it the post-cutover smoke run as well. Gate
   passed mechanically: `release gate: PASSED — run 3995b675 green,
   4/4 jobs covering the persisted definition`.
3. ✅ Cutover (absorbed a mid-cycle collision, do not race): bundle
   `gitforge-b6d13e69-20261008` (built 08:47:01Z, RELEASE_METADATA
   source_commit exact-matches the gated SHA) was bundled and promoted
   by a co-tenant arm at 08:48Z before the green run existed; this arm
   verified the metadata, produced the gated green run on the promoted
   ci, and adopted the cutover instead of re-cutting. All four units
   have run the bundle binaries since the 10:19:32Z restart. The
   restart honestly fenced this arm's in-flight attempt-10 test job
   (`scheduler_restart_fenced_running_job`).
4. ✅ Post-cutover verification: `/health` 200 on :42780/:42781/:42782;
   boot journal shows migrations → event consumer (10:20:11Z) →
   workspace sweep (10:20:23Z, removed=3); `gitforge-status` clean with
   `GITFORGE_RELEASE_ROOT=…/releases/gitforge-current`; durable trigger
   queue proven in production — a trigger published at 10:33Z landed
   as a run row at 10:38Z while the consumer was busy with a co-tenant
   checkout (late, never lost — the pre-0.6.15 hollow-trigger loss is
   closed); auth rotation verified live (login → refresh rotates
   access+refresh); CLI rebuilt from the release tip and live via the
   existing `~/.local/bin/gitforge` symlink, `auth --status` works and
   leaves a copied config byte-identical (the old CLI's save-drops-
   tokens defect no longer reproduces on read paths).
5. Incident ledger for this release cycle — 11 trigger attempts, 10
   infra/environment kills, each root-caused with journal+DB evidence:
   runner_lost (heartbeat UPDATE stalled 140s under DB pressure →
   stale sweep fenced a healthy job), ghost runs ×2 (run row created,
   checkout never spawned — both formed while ci's sequential consumer
   was mid-clone on a CO-TENANT workspace; every idle-consumer creation
   succeeded), NAS rmeta EIO mid-clippy (btrfs read failure under
   load), sandbox acquisition timeout at load 66 (60s hard cap,
   Docker daemon saturated), one REAL test defect (fixed in the release
   SHA), and the restart fence above. Re-fire, don't debug: every
   infra kill re-triggered clean on the next window.
6. 0.6.16 candidates from this cycle: spawn the event consumer before
   the inline engine rebuild; grade a run failed when its checkout
   spawn errors (and move per-run checkouts off the sequential
   consumer); cascade manual job cancels to pending descendants (F37
   covers failure-grading only); load-aware or raised sandbox
   acquisition cap; checkpoint/requeue for jobs fenced by
   `scheduler_restart` (retry_count stayed 0); persist_with_retry for
   the runner heartbeat write.
7. Mirror convergence: gitforge-ci main fast-forwarded to the release
   tip in this cycle; GitHub mirror via PR after (branch
   `feat/v0614-auth-updates`, 30 commits).

   Convergence addendum (2026-10-08 ~13:45Z): gitforge-ci main, origin/main,
   and local remotes all converged on merge commit `54f93c3b` (GitHub PR
   #281 admin-merged under a temporary Safeguards bypass grant, restored
   after). Main's own pipeline run `d9c964c4` graded succeeded 4/4 on that
   exact SHA — the converged tree is green on the primary platform.
   Two further ledger findings from the convergence window: (a) run
   `99c463db` logged `persisted planned job rows planned=4` yet zero rows
   exist — the plan persist is a one-shot write outside persist_with_retry
   (F21/F23 class) and the run reconciler skipped on pool timeouts; (b) a
   ~29 min global SQLite write freeze (WAL mtime frozen; btrfs-transaction
   committing 11 MB/s under 84%-util spinner read load) stalled all
   heartbeats/completions; run `4d07b0a9`'s test container finished its
   work but its completion hit `cannot start a transaction within a
   transaction` (nested-BEGIN on the completion path; retries fail on the
   broken context) and the lease-dead fence graded it failed at 12:38:20,
   leaving a running-forever durable row for boot reconcile. Correct
   response to the freeze was to wait — a service restart clears nothing
   and fences live jobs.

   0.6.16 cycle addendum (2026-10-10): every candidate from item 6 has a
   verdict. Nested-BEGIN poisoning — two-layer defense landed
   (`begin_immediate` recovery passes 0b16dd76; pool-wide
   `before_acquire` heal 0dfc0f15), validated green by fedora pipeline
   run `be5c03fd` on the exact commit. Planned-row persist — wrapped in
   `persist_with_retry` (a4d52e44). Ghost-run class — checkout-spawn
   failure grading and per-repo trigger lanes were already in tree;
   the remaining boot-order fix landed (consumer before the inline
   workspace rebuild, 69e3c859). Manual cancel cascade — descendants
   cancelled and the pipeline finalized (09c91bfa); the live instance
   exhibited the pre-fix signature the same day (run `ee2df0ec`'s jobs
   all cancelled yet the run row stayed `running` on build 63427df).
   Sandbox acquisition cap — now load-aware, scaling to 3× under
   saturation with the env pin still winning (bdfc9a75). Runner
   heartbeat write — through `persist_with_retry` (same commit).
   Remaining candidates (checkpoint/requeue for restart-fenced jobs)
   are covered by the redrive pair (50fede66, c71a87c2). Release
   evidence discipline note: the 2026-10-10 operating directive routes
   all CI through the fedora instance; its ci unit was found stopped
   (clean SIGTERM, 02:41:32 CDT, no owning cron/timer) and restarted
   manually — attribution unknown, recorded here because an absent
   orchestrator silently turns every queued run into a ghost.

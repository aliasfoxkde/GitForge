# OpenCodifier Quality Audit — GitForge — 2026-11-03

Scope: procedural workarounds (docs compensating for tool defects), code smells, test quality, CLAUDE.md anti-patterns. Read-only audit.

## 1. Procedural workarounds

Discriminator applied: does skipping the procedure make the tool misbehave?

1. **HIGH — sqlx stale prepared-statement panic; procedure = "stop everything, then start together."**
   `docs/RUNBOOK.md:264-276`: a live migration (ALTER TABLE) on the shared SQLite DB while peers hold prepared `SELECT *` statements causes `SqliteRow::current index out of bounds` panics (ledger F40, 2026-10-10). The runbook prescribes a drain-gated stop-everything-then-start-together sequence. Underlying defect (peers don't handle schema-change reprojection) is fixable in code: version-check handshake at connection setup (fail fast with a clear error), or pool recycling on schema-version mismatch. Disposition: fix now — bounded change; keep the cutover procedure until shipped and mark it as debt with the defect reference in RUNBOOK.
2. **HIGH — masked-unit auto-update rollback (tool fixed; needs regression test).**
   `docs/RUNBOOK.md:283-288`: NAS runner unit is masked; pre-fix `gitforge-release-auto` made every release roll back forever. Scripts now skip masked units. Disposition: fix now — add a regression test for the masked-unit skip in scripts/gitforge-release-auto so the failure can't silently return.
3. **MED — GitHub auto-merge from CI fails; documented manual-merge workaround.**
   `docs/planning/PHASE2_PLAN.md:133`: third-party GITHUB_TOKEN can't set auto-merge (needs a GitHub App). Workaround: manual merge per PR. Fixable by minting an App token or merging host-side. Disposition: fix now (small script); record as debt if no App is wanted.
4. **MED — GitHub ruleset temporary-bypass dance, documented 4+ times.**
   `docs/planning/CAMPAIGN_PLAN_2026-09-28.md:72,208`, `CAMPAIGN_PLAN_2026-10-03.md:208`, `docs/BRANCH_STRATEGY.md:75`, `docs/MIRROR_POLICY.md:49`: back up rulesets → bypass grant → merge → restore. Branch protection itself is legitimate policy, but the manual grant/restore cycle is a tool gap. Disposition: record as debt with owner — a `scripts/` helper wrapping bypass+merge+restore atomically converts the procedure into a tool.
5. **MED — SELinux labeling gotcha for ad-hoc docker probes.**
   `docs/RUNBOOK.md:260-263`: ad-hoc bind-mount probes need `--security-opt label=disable` or writes fail despite correct modes; the runner's own job workspaces already handle labeling. The fix exists in the tool but ad-hoc scripts don't reuse it. Disposition: fix now — bake labeling handling into scripts that bind-mount host dirs.
6. **LOW — systemd CLI quirks (`XDG_RUNTIME_DIR`, `.service` suffix).**
   `docs/RUNBOOK.md:255-259`. Environment quirk, not a GitForge defect. Disposition: no action.
7. **LOW — `GITFORGE_SCHEDULER_TOKEN` "temporary compatibility fallback" with no owner or removal condition.**
   `docs/HARNESS_JOB_CONTRACT_V1.md:9`. Disposition: record as debt — add owner + removal milestone per Fix-the-Tool rule 2.
8. **HIGH (stale-doc risk) — runner registration fail-open.**
   `docs/planning/HANDOFF.md:138-146` lists it as a known issue, but `crates/gitforge-runner/src/agent.rs:57-60,124,329` shows `allow_standalone` now defaults to **false** (fail-closed, opt-in legacy mode). HANDOFF is a stale false record — the P0 was fixed but the doc still says fail-open. Disposition: fix now — update HANDOFF (or archive it with a status header); verify the config path that sets `allow_standalone` is denied in the trust policy.


1. **HIGH — `crates/gitforge-db/src/queries.rs` is 7,253 lines.** Confirmed. Single file holds Repo/Pipeline/Run/User queries with `#[cfg(test)]` modules inline. Disposition: record as debt; split by aggregate into `queries/{repo,pipeline,run,user}.rs` in a dedicated refactor milestone.
2. **MED — `services/ci/src/main.rs` is 6,493 lines.** A main.rs of this size means routing + handlers + state live in the binary crate rather than the `gitforge-ci` library (engine.rs is a separate 1,697). Disposition: record as debt; move handlers into `gitforge-ci` so tests exercise the library, not a spawned binary.
3. **MED — triplicated `error_response` helpers in gitforge-api.** `crates/gitforge-api/src/server.rs:334`, `src/routes/ssh_keys.rs:60`, `src/routes/review.rs:105` — three near-identical JSON error constructors with slightly different signatures (`code: &str` vs `error: &str`, `String` vs `&str` message). Classic copy-paste error handling. Disposition: fix now — small mechanical consolidation into one helper.
4. **LOW — unwrap/expect production sites: 0** per `scripts/unwrap-production-count.py` (2,144 raw grep hits are test contexts + `#[cfg(test)]` modules; spot check of queries.rs:4013 confirmed test code). CLAUDE.md rule honored. Disposition: none — the wrapper script is doing its job.
5. **LOW — no TODO/FIXME/HACK markers in crates/ or services/ non-test code; no versioned files (`*_v1`, `*.bak`); no hardcoded credential literals found.** Disposition: none.
6. **MED — `crates/gitforge-scheduler/src/assigner.rs` (4,435 lines) and `crates/gitforge-runner/src/agent.rs` (4,135 lines)** oversized single files. Disposition: record as debt alongside #1.
7. **Info — `services/ci` vs `crates/gitforge-api/routes/ci.rs` (1,583 lines)**: API crate re-implements CI route handlers adjacent to the service binary; boundary between thin gateway and owning service is blurred. Same disposition as #2.

## 3. Test quality

1. **MED — status-code-only assertions outnumber body checks in several route suites.** Counts of `assert_eq!(status, ...)` vs body-content asserts: `artifact_routes.rs` 18 vs 10, `runners_routes.rs` 23 vs 19, `ssh_key_routes.rs` 12 vs 7. Several artifact tests assert `StatusCode::OK` without reading the artifact payload (e.g. artifact_routes.rs:248, :295). A route that returns 200 with the wrong/empty body passes. Disposition: fix now — add payload assertions to the OK paths in these three files (bounded, ~15 sites).
2. **LOW — good counterexamples exist**: ci_routes.rs:190-202 checks ownership enforcement with body content AND forbidden body text; webhook_routes.rs has 35 body asserts vs 15 status. The pattern is established; suites in #1 just lag it.
3. **HIGH — `#[ignore]`-gated Docker integration tests are the ONLY coverage of runner executor timeout reaping and sandbox behavior.** `crates/gitforge-runner/src/executor.rs`, `crates/gitforge-sandbox/src/docker.rs` carry `#[ignore]` tests that pass only `cargo test -- --ignored` against a live daemon (docs/planning/IMPROVEMENTS.md:53-57). Nothing in the pre-push gate runs them; a release gate that never sets `--ignored` gives a false pass on the isolation/timeout path. Disposition: fix now — wire `--ignored` into the CI job that runs on the docker-capable fedora runner (one .gitforge.yml line), and keep the ignore for local runs.
4. **HIGH — coverage gate silently skips: 11 `return 0` escape hatches, zero Rust support.** `.githooks/pre-push.d/30-coverage-gate` only understands Go and Python coverage profiles; for this Rust workspace it always exits 0 without running anything — a false-pass gate wearing a gate's name. Same story for `10-complexity-gate` (6 skips, no Rust). This is the purest OpenCodifier class: the *procedure* (coverage gate) exists; the *tool* (the gate script) does not measure this repo's language. Disposition: fix now — replace both with the repo's actual Rust routes (cargo-tarpaulin/lcov via `scripts/qualityctl`, which exists) or delete the Dark Factory Go/Python scripts from this repo.
5. **LOW — pre-push smoke test is 2-thread bounded to dodge a known load race** (`.githooks/pre-push:9-12`: git_http_edges / ci_trigger_flow flake class). This is a documented workaround for test isolation (spawned-binary protocol tests racing under load). Underlying fixable defect: the tests share ports/dirs or lack backpressure. Disposition: record as debt with owner — fix the race, then unbound the parallelism.

## 4. CLAUDE.md anti-patterns (Rust standards)

Covered in §2 items 4-5: production unwrap/expect = 0 (verified via scripts/unwrap-production-count.py + spot checks), no TODO/FIXME/HACK, no versioned/backup files, no hardcoded credential literals in crates/ or services/. One caveat: `services/*/src/main.rs` binaries were included in the scan; repo-wide exclusions (fuzz/, target/) are not production code. Disposition: clean.

## Summary

GitForge's discipline is unusually good for the OpenCodifier failure classes: zero production unwraps, no TODO debt, fail-closed runner (though HANDOFF.md still claims otherwise). The four fix-now items: (1) pre-push coverage/complexity gates are Go/Python-only scripts that always exit 0 on this Rust workspace — false-pass gates, replace or delete; (2) #[ignore] Docker tests (executor timeout reaping, sandbox) never run in any gate; (3) HANDOFF.md stale fail-open claim contradicts agent.rs:124 fail-closed default; (4) ~15 status-only assertions in artifact/runners/ssh_key route suites. Highest-value debt: sqlx schema-change panic masked by the stop-everything cutover procedure (RUNBOOK F40), and the 7k-line queries.rs / 6.5k-line ci main.rs monoliths.

## 5. Phased plan derived from this audit (2026-11-03)

Item 3 of the Summary was fixed during this session: HANDOFF.md:140 now
points at the RESOLVED verdict under "Next Steps". The rest, ordered by
phase.

### Phase P0 — false-pass gates (fix first; they mask everything else)
1. DONE 2026-11-03 — both Dark Factory gate scripts removed from
   `.githooks/pre-push.d/`. The surviving `pre-push.d` member
   (`20-clean-branch`, advisory) is now invoked from `.githooks/pre-push`
   with a real commit range (upstream..HEAD) instead of git's
   ref-vs-itself arguments, so its commit-message check actually runs.
   The Rust routes stay where they were: `.gitforge.yml` coverage job
   and `scripts/gitforge-quick-check.sh`.
2. DONE 2026-11-03 — new `docker-integration` job in `.gitforge.yml`
   (test → docker-integration → coverage chain) runs
   `cargo test -p gitforge-sandbox -p gitforge-runner --lib --tests
   -- --ignored --test-threads=2`. The job mounts the daemon socket via
   the `{docker_socket}` volume placeholder (resolves to the socket of
   the daemon THIS runner drives — rootful docker on the NAS, rootless
   podman on fedora), and the sandbox runs socket-mounting containers
   with SELinux labeling disabled after live probes on both hosts
   confirmed the refusal without it. `#[ignore]` is kept for local
   runs. A `gitforge-ci` test pins the job's presence, socket mount,
   and the `--ignored` filter in its step.
3. DONE 2026-11-03 — `scripts/gitforge-release-auto` gained
   `--print-restart-plan`, and
   `tests/release-auto-restart-plan-selftest.sh` asserts with a fake
   `systemctl` that a masked unit is excluded from the restart plan
   (both directions pinned; the test fails when the skip is reverted).
   Wired into the `.gitforge.yml` lint job and the shellcheck glob.

### Phase P1 — correctness consolidation (bounded, mechanical)
4. Consolidate the three `error_response` helpers (server.rs:334,
   routes/ssh_keys.rs:60, routes/review.rs:105) into one.
5. Add payload assertions to ~15 status-only OK paths in
   artifact_routes.rs, runners_routes.rs, ssh_key_routes.rs.
6. Land the sandbox-timeout fix already staged in executor.rs (job
   timeout inherited, not a hardcoded 3600s ceiling), with a pinned test
   citing run 0d4b44c4.
7. Container hardening: `cap_drop`, `no-new-privileges:true`, explicit
   non-root `user` in `gitforge-sandbox/src/docker.rs` HostConfig.

### Phase P2 — debt with owners (one per defect class)
8. sqlx schema-change handshake: version-check at connection setup,
   fail-fast with a clear error (ledger F40); then shrink the RUNBOOK
   stop-everything cutover to the residual case, referencing the defect.
9. `GITFORGE_SCHEDULER_TOKEN` fallback: add owner + removal milestone in
   HARNESS_JOB_CONTRACT_V1.md.
10. Ruleset bypass helper: `scripts/` wrapper for grant→merge→restore.
11. Pre-push 2-thread bound: fix the git_http_edges/ci_trigger_flow
    port/dir race, then unbind.

### Phase P3 — structural (dedicated milestones, no behavior change)
12. Split `gitforge-db/src/queries.rs` (7,253 lines) into
    `queries/{repo,pipeline,run,user}.rs`.
13. Move `services/ci` handlers into the `gitforge-ci` library crate.
14. Split `scheduler/assigner.rs` (4,435) and `runner/agent.rs` (4,135).
15. Docs scaffolding: add `docs/PROJECT_STRUCTURE.md`/`CONVENTIONS.md`.

### Deferred / not planned
- GitHub App for auto-merge (only worth it if PR volume justifies it).
- SELinux labeling in ad-hoc probe scripts (operational, not product).

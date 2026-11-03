# Execution plan — gap closure, 2026-11-03

Systematic sweep of the remaining GitForge audit findings and operational
gaps, worked end to end. All state claims below were read from the
authoritative stores (gitforge.db, the API, the filesystem) at claim time.

## Session update (later, 2026-11-03)

- **F5 RESOLVED**: `/metrics` and `/dashboard` require auth unless
  `GITFORGE_PUBLIC_TELEMETRY=1` (`crates/gitforge-api/src/middleware.rs`);
  decision core + middleware-branch unit tests added. `/health` stays
  public.
- **F4 WONTFIX (design decision recorded)**: instance-wide role model kept
  deliberately; verdict in
  `docs/planning/20261101-minimax-audit-authz.md` §Dispositions.
- **A1 complete**: the three release-gate skip tests landed in
  `tests/test_release_gate.py` (21 tests / 39 subtests green); the gate's
  unconditional-job check now accepts satisfied (succeeded *or* skipped)
  rows — quality-chain names remain fails-closed via the dedicated check.
- **ref_name column landed** on `integrate/audit-fixes-20261102`:
  `pipeline_runs.ref_name` (new column + tolerated-duplicate migration),
  webhook/ci paths populate it, supersede filters by it.
- **Ledger verdicts recorded** for the 2026-11-02 findings
  (`GITFORGE-CI-AUDIT-AND-IMPROVEMENT-PLAN.md` §Verdicts).
- Merge order fixed: PR #365 (F1/F2, MERGEABLE) → PR #362 (blank
  timestamps, MERGEABLE) → `integrate/audit-fixes-20261102` → release cut.


## Completed this session

| Item | Evidence |
|---|---|
| G1/G2: pipeline id threaded through trigger delegation | commit `5564e446`; CI run `4824e78a` SUCCEEDED on `15ab297f` |
| Dedup scoping regression test (per-pipeline keys) | commit `15ab297f`, `test_trigger_request_dedup_is_scoped_per_pipeline` passes |
| Branch push + CI verification | branch `fix/ci-trigger-pipeline-id-g1g2-20261102` → `mkinney/gitforge` on 202 (the `operator/gitforge` remote URL in `.git/config` was stale — repo of record is under `mkinney/`) |
| A2/A3: detect-changes self-test | `tests/detect-changes-selftest.sh`: 8 assertions, covers branch push, main tip-diff fallback, docs-only, code changes; wired into `.gitforge.yml:95`; passes live |
| F-05: working_dir validated at admission | `validate_workspace_path` at `services/ci/src/main.rs:904`; pinned by `test_trigger_http_contract_rejects_malformed_requests` |
| F-12: force checkout adopts writable tree | `git checkout --force --detach` at `services/ci/src/main.rs:1946` |
| G3 residual: entity gauges never set | NEW defect: `runners_online`/`pipelines_total`/`jobs_total`/`repos_total`/`artifacts_total` had no production setter — stayed 0 through 9k+ jobs. Fixed: 30s gauge-refresh task in `services/ci/src/main.rs` + `JobQueries::count_all`. Awaiting deploy |
| Supersede: newest-SHA-wins per (repo, ref, pipeline) | `PipelineQueries::supersede_stale_runs` (cancels queued jobs + grades stale runs `cancelled` with cause) wired into the trigger path; regression test added. Awaiting gate. Unblocks fedora runner restore |
| F-16: image registry allowlist + digest pinning | `GITFORGE_ALLOWED_IMAGE_REGISTRIES` env-gated check in `gitforge-ci::engine` (core takes allowlist as data; 5 unit tests incl. port-prefix boundary). Unset = unrestricted |
| RUNBOOK drift | metrics section corrected; one-runner fleet pause policy documented with the unpause path |

## Known remaining work (ordered)

1. **Gate the three code changes** (gauge refresh, supersede, image
   allowlist): `cargo check`/`nextest` for `gitforge-db`, `gitforge-ci`,
   `gitforge-ci` binary — build running at time of writing; commit after
   green, push, and verify the CI run reaches SUCCEEDED.
2. **Deploy the refreshed binaries to 202** (promote + cutover per
   `docs/planning/PRODUCTION_VALIDATION_20261102.md` §4 — the dry-run
   evidence and rollback path there still stand) so the gauge fix and
   supersede take effect; then verify `/metrics` gauges move.
3. **Restore fedora-docker capacity** after supersede deploys: remove
   `~/.config/gitforge/runner.paused` on fedora; the cron guard restarts
   the runner (image `dsc-ci-rust:16` already present; `runner.env` and
   sshfs bridge configured).
4. **A1 unit tests blocked by a harness defect**: the Edit hook
   false-positives on the existing filename `tests/test_release_gate.py`
   (matches `_release` as a "versioned" pattern) and refuses in-place
   edits. Fix the hook's regex, then add the three designed tests
   (skipped quality job refuses; `ALLOW_SKIPPED=1` admits; non-quality
   skip passes). The gate-script precondition itself is already
   implemented.
5. **Container hardening residual**: `HostConfig` uses `..Default::default()`
   for caps — no `cap_drop`, no `no-new-privileges`, no explicit non-root
   `user`. Bounded change in `gitforge-sandbox/src/docker.rs` once the
   current gate passes.
6. **S9 (job log permissions + secret redaction)** needs its original
   audit context (which secrets, which sink) before implementation —
   recorded, not guessed at.
7. **Optional**: Phase C archive consolidation (C1/C2 of the quality
   roadmap) — mechanical, low risk, lower value than the items above.

## Anchors

- Validation campaign: `docs/planning/PRODUCTION_VALIDATION_20261102.md`
- Quality roadmap (A/B/C phases): `docs/planning/QUALITY_ROADMAP_2026-10-25.md`
- Queue incident + supersede rationale:
  `docs/planning/GITFORGE-CI-AUDIT-AND-IMPROVEMENT-PLAN.md` §Update 2026-11-02

## Architecture audit (2026-11-03, static)

Ship-blocking: none — no unwrap/expect in production code, zero
TODO/FIXME, WAL+busy_timeout discipline held, raw SQL confined to
gitforge-db/cas.

Next cycle (ordered):
1. Split `crates/gitforge-db/src/queries.rs` (7,253 lines, 193 fns) into
   per-aggregate modules under `src/queries/` — mechanical, no API change.
2. Docs: add `docs/PROJECT_STRUCTURE.md`/`CONVENTIONS.md` or record the
   exception (scaffolding rule).
3. Execute roadmap C1: archive superseded plan generations with pointers.
4. Move the image-allowlist env read (gitforge-ci engine.rs) to the edge,
   matching the decision-core rule.
5. Coverage Phase B: re-baseline in-sandbox before any threshold change.

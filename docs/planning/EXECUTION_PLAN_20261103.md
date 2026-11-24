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

## Runner image identity across a multi-host fleet (2026-11-05)

F-16 shipped the *gate* (allowlist + digest pinning required), but the
fleet cannot currently satisfy it: there is no shared registry the runners
can reach, so every repository still distributes its job images by hand.
This section records the measurements that define the durable fix.

**The failure.** A DSC pipeline pinned each job's image by Docker image
ID. The same content resolves to different IDs per storage driver —
swarmone's classic overlay2 reports the image's *config* digest
(`sha256:0402417a…`), fedora's overlayfs + containerd snapshotter reports
the *manifest-list* digest (`sha256:fede6b3c…`). `SimplePolicy::select_runner`
filters `status=="online" && capacity>0` and takes `max_by_key(capacity)`,
so `fedora-docker` (5) outranks `swarmone-docker` (4) and every dsc job
landed on fedora, where the pinned IDs did not exist — "No such image" at
sandbox creation, which cancels every job chained behind it. The pipeline's
correctness depended on which runner won a capacity race.

**What is and is not portable** (measured 2026-11-05, same image content
on both hosts):

| Identity                                  | Portable across drivers? | Value |
| ----------------------------------------- | ------------------------ | ----- |
| `.Id` (local image ID)                    | **no**                   | `0402417a…` (swarmone) vs `fede6b3c…` (fedora) |
| image **config** digest                   | yes                      | `sha256:0402417a…` on both |
| **diffID** fingerprint (sorted layer set) | yes                      | `sha256:393acaf2…` on both |
| registry **manifest** digest, pushed independently per host | **no**   | `6e8aeb89…` (swarmone) vs `71f39742…` (fedora) — each host re-compresses layers differently |

The last row matters: a registry manifest digest identifies *a particular
publication*, not the content. A shared registry only yields a portable
digest if it is the **single publisher** — otherwise two hosts that each
push their own copy mint different digests for identical content.

**The pull path already supports it.** `ensure_image`
(`crates/gitforge-sandbox/src/docker.rs:361`) passes the whole reference to
Docker's `create_image` `fromImage`. A combined `repo@sha256:…` reference
is accepted there — verified live against a temporary registry
(`POST /v1.44/images/create?fromImage=127.0.0.1:5002/dsc-ci-node@sha256:71f3974…`
→ HTTP 200, `Digest: sha256:71f3974…`). So F-16's contract is achievable
without further sandbox changes.

**Why the repo-side workaround is debt, not a fix.** The DSC repository
(pinned by the tag contract in `.gitforce.yml` + the baked lockfile stamp
check) makes a tag behave like a fixed reference: the image bakes the
sha256 of the lockfile it was warmed from, and `lint-node`/`lint-rust` fail
fast when the checkout's lockfile diverges from it; the diffID fingerprint
proves the image is byte-identical on every runner host. That is a
procedure compensating for the missing capability. It is recorded as
explicit debt, owned by the operator, with the following shape:

1. **One shared registry, reachable from every runner**, published from a
   single build host so the manifest digest is canonical. Today
   `gitforge-local-registry.service` publishes `127.0.0.1:5000` only —
   loopback-only and unreachable from fedora — and both daemons'`
   `insecure-registries` cover only `127.0.0.0/8`, so a LAN-bound registry
   additionally needs `insecure-registries` on both daemons (a
   `/etc/docker/daemon.json` edit plus a docker restart on the CI host).
   A temporary registry bound to `192.168.0.201:5002` was verified
   reachable from both hosts (HTTP 200) — the network path is not the
   blocker; daemon configuration is.
2. **`GITFORGE_ALLOWED_IMAGE_REGISTRIES`** set to that registry's prefix
   on the scheduler, so the F-16 gate becomes active and every job image
   must be a digest-pinned reference under it.
3. **Job images pinned as `registry/repo@sha256:<manifest-digest>`** in
   each repository's pipeline definition.
4. **Runner affinity** (or capacity tuning) so a job's image is present
   on whichever runner the scheduler selects. There is no affinity
   mechanism today: the `runners` table has no `repo_id` and `labels` is
   hardcoded `"[]"`. `select_runner` has no notion of "can this runner
   serve this image".

Item 4 is the substantive scheduler change: with a shared registry, a
runner can *pull* an image it does not have, so the cold-pull cost and the
60-second sandbox acquisition cap (`GITFORGE_SANDBOX_ACQUIRE_SECS`) become
the binding constraint instead of image presence. An affinity preference
plus a warm-pull strategy on the runner is the natural follow-on.

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

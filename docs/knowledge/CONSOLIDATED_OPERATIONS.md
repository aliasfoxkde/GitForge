# Consolidated Operations Knowledge

Consolidated from `docs/RUNBOOK.md`, `docs/DEPLOYMENT.md`, and the
operational-gotcha rules recorded outside the repo (agent memory rules:
skip_if false-greens, release-gate evidence, masked units, cache-mount
rollout). Every claim cites its source. Verified against the tree at
commit d476baf7 (branch `feat/m9-knowledge-consolidation`).

Nothing here is invented; where a documented claim contradicts the code
it cites, it is listed under [Contradictions](#contradictions) instead
of silently resolved.

## 1. Service topology and ports

Source: docs/RUNBOOK.md lines 9-18, 82-84; docs/DEPLOYMENT.md.

| Service | Ports | Notes |
|---|---|---|
| api (gateway) | 42780 | `/health`, `/metrics`, `/swagger-ui` public |
| ci (orchestrator + scheduler API) | 42781 | scheduler HTTP API lives inside the ci service (RUNBOOK line 109) |
| git-server | 42782 (HTTP), 42022 (SSH) | in-process russh SSH transport (RUNBOOK lines 86-97) |
| runner | dynamic | registers into the scheduler |

The API gateway serves the runner list; the scheduler on 42781 only
exposes `POST /runners` (registration/heartbeat) — query the gateway for
runner inspection (RUNBOOK lines 358-363).

## 2. Two-instance topology (2026-10-10)

Source: docs/RUNBOOK.md lines 276-292.

- **NAS (control plane)**: system-level template units
  `gitforge@{api,ci,git-server}.service`, repo of record, ports
  42780/42781/42782. The NAS `gitforge@runner` unit is masked — retired,
  not failed; `gitforge-release-auto` and `gitforge-status` skip masked
  units (RUNBOOK lines 281-284; scripts/gitforge-release-auto lines
  55-65 `unit_masked()`; scripts/gitforge-status lines 282-287).
- **Execution fleet**: remote docker runners (`fedora-docker` on
  192.168.1.202, `t5500-docker`) register into the NAS scheduler
  (RUNBOOK lines 285-290).
- **Fedora standalone instance**: user-level systemd
  `gitforge-{api,ci,git-server,runner}.service`, pinned via the
  `~/work/gitforge-current` symlink; release-validation and coverage
  instance; cut over with `scripts/gitforge-remote-cutover --units user`
  (RUNBOOK lines 228-252).

### Masked units are retired, not failures

A masked `gitforge@*` unit is a valid steady state (a component retired
on that host). Treat it as retired: promote/status skip it — do NOT
unmask to unstick a promote. `systemctl cat` returns rc=0 even for
masked units; `systemctl list-unit-files <unit>` showing `masked` is
the discriminator. Historical incident: because release-auto hardcoded
all four units, the masked NAS runner made every promote fail and roll
back forever (channel stuck at gitforge-54f93c3b-20261009).

Sources: scripts/gitforge-release-auto:55-65, scripts/gitforge-status:282-287,
docs/RUNBOOK.md:248-252, 281-284.

## 3. Release cycling (bundle cuts, not tag cuts)

Source: docs/RUNBOOK.md lines 184-226.

1. **Gate** — `scripts/gitforge-release-gate <40-hex-commit>` refuses to
   cut unless the live instance graded the exact source commit with a
   full honest run.
2. **Bundle** — `scripts/gitforge-release-bundle <stage> <bundle-root>
   <release-id>`: preflight, release binaries, MANIFEST.sha256, READY.
   Atomic as of 2026-10-10: assembles under
   `releases/.incomplete-<release-id>/` and renames into place only once
   `READY` exists (RUNBOOK lines 221-226).
3. **Promote** — `scripts/gitforge-release-auto` (flock on
   `releases/.auto-update.lock`; drain gate: defers while any
   `gitforge-job-*` container runs — pending-only runs are just queued;
   flips `gitforge-current` atomically; restarts `gitforge@*`; leaves
   the previous bundle for rollback) (RUNBOOK lines 206-214).

Verify after cutover: unit states, `/health` on 42780/42781/42782, and
`scripts/gitforge-status` with `GITFORGE_RELEASE_ROOT=<bundle link>` —
the default path reports false drift (RUNBOOK lines 211-214).

### Release-gate evidence requirements

`scripts/gitforge-release-gate` (mechanical form of ledger rule F16)
requires, as hardened 2026-10-21 (scripts/gitforge-release-gate, esp.
lines 18-52, 206-247):

- a run whose `commit_hash` is the EXACT source commit, status `succeeded`;
- durable job rows covering every job in the persisted pipeline
  definition (matched on step `run` strings — durable rows carry
  generated names, plan finding F12);
- `succeeded` OR `skipped` satisfies a job with a `skip_if`;
- **every definition job WITHOUT a `skip_if` must have a `succeeded`
  row** (gate SQL at scripts/gitforge-release-gate:206-214) — otherwise
  a run whose conditions all evaluated false grades green having
  executed nothing.

Operational notes: the gate needs the `sqlite3` CLI, absent on Fedora —
use python3's sqlite3 module there, or snapshot the DB and point
`GITFORGE_RELEASE_DB` at the snapshot (script lines 36-52).
`GITFORGE_RELEASE_SKIP_GATE=1` bypasses with a loud warning — bootstrap
only, for the cut that deploys the engine that produces the evidence
(script lines 37-42).

### skip_if change detection — the false-green defect class

The `skip_if` feature (commit 439942be) initially shipped half-wired:
`CiEngineState::pipeline_env` was never populated
(`extract_change_env` in crates/gitforge-ci/src/engine.rs:133 had no
production caller), so every skip_if-gated job was evaluated against an
empty environment and always skipped; the skip transition was
in-memory only, so durable rows stayed `pending` and the run never
finalized (redrove every ~100s). Fixed 2026-10-21 (commits
7151ad58/c4d52403): D1 `extract_change_env` reads `CI_CHANGE_*` from
the durable `jobs.result_json` on both live-completion and rebuild
paths; D2 `settle_status()` (engine.rs:71) plus a fixed-point release
loop so skip chains settle in any definition order; D3 durable
terminal `JobStatus::Skipped` rows (engine.rs:90-95, 454-495).

**Discipline**: a green GitForge CI run means the gates ran — but read
the per-job list, not just run status. A run whose gated jobs are
`pending` forever is the old engine, not a slow run; the engine must be
deployed before the push that triggers the run.

### Deploy skew

A runner/service binary older than a feature silently drops new
fields. Known instances: pre-cache runners ignore `volumes:` (serde
`#[serde(default)]` drops unknown fields — crates/gitforge-common/src/
volume.rs:35-40), and a pre-skip_if engine shows the pending-forever
signature above. Check the executing binary's commit before trusting a
run.

## 4. Cache mounts

Source: docs/RUNBOOK.md line 168 (`GITFORGE_CACHE_ROOT`); the
`.gitforge.yml` clippy/test/coverage jobs declare `cache:` blocks
(.gitforge.yml:50).

- The runner bind-mounts `<GITFORGE_CACHE_ROOT>/cargo/<image>` at the
  job's `mount_path`; default `~/.cache/gitforge`, fallback
  `/var/cache/gitforge` when `$HOME` is unset
  (crates/gitforge-runner/src/agent.rs:2756).
- A cache that cannot be prepared is skipped with a warning; jobs run
  cold (RUNBOOK line 168).
- **Rollout gate**: a runner binary older than the feature silently
  ignores the `volumes` field (serde drops unknown fields,
  crates/gitforge-common/src/volume.rs:35-40), so a green run proves
  nothing about the mount. Verify with `docker inspect` — the Binds
  array must contain more than the workspace bind — before trusting any
  warm-run measurement. Historical: run cab70f20 was misread as cache
  evidence on the pre-feature Fedora runner (corrected in commit
  24c5a6bf).

## 5. CI boot and trigger semantics

Source: docs/RUNBOOK.md lines 216-219.

The ci unit runs migrations, then the startup workspace sweep, then
spawns the event consumer. Triggers published while the consumer is
down are durable: a `202` with a null `pipeline_run_id` means queued,
not lost — re-query before re-triggering.

## 6. Shared-DB migration hazard (ledger F40)

Source: docs/RUNBOOK.md lines 266-274.

sqlx caches per-statement column metadata at prepare time. One service
`ALTER TABLE`ing the shared SQLite DB while another holds prepared
`SELECT *` statements makes SQLite's auto-reprepare pair a new column
count with stale metadata; the peer panics (`SqliteRow::current`:
`len is 16 but the index is 16`). `Restart=on-failure` bounds the
damage; the drain-gated stop-everything-then-start-together cutover
avoids the window. Never migrate under a running peer.

## 7. Workspace prep and runner env

Source: docs/RUNBOOK.md lines 125-137, 155-182.

- Each workspace-prep git command runs under
  `GITFORGE_WORKSPACE_PREP_TIMEOUT_SECS` (default 300); a hung child is
  killed and the run graded `failed` with the cause on the run row.
- A terminal (`failed`/`cancelled`) run sweeps its never-dispatched
  job rows to `cancelled` — no zombie rows.
- Runners need distinct `GITFORGE_RUNNER_NAME`s; registration retries
  up to `GITFORGE_REGISTER_ATTEMPTS` (6) with doubling backoff from
  `GITFORGE_REGISTER_BACKOFF_SECS`; 401/403 are never retried;
  `GITFORGE_SCHEDULER_URL` is mandatory (fail-fast).

## Contradictions

1. **Crate count.** CLAUDE.md and `.github/wiki/GETTING_STARTED.md`
   said "15 libraries/crates under `crates/`"; `ls crates/` and the
   `members` array in Cargo.toml (lines 3-20) show 16 crates
   (including `gitforge-testing-common`, a dev-support crate).
   Resolved in this change by updating both files to 16; noted here
   because `gitforge-testing-common` is not a production library, so a
   "15 production libraries" reading was defensible — the docs now say
   16 crates to match the manifest verbatim.

2. **RUNBOOK clippy command is weaker than the CI gate.**
   docs/RUNBOOK.md:413 shows `cargo clippy --workspace -- -D warnings`,
   but CLAUDE.md:39 and `.gitforge.yml` use
   `--workspace --all-targets`. The RUNBOOK form does not lint test
   targets; use the CLAUDE.md form locally. (Flagged, not silently
   changed — the RUNBOOK section is developer convenience, but the
   discrepancy can let a clippy failure reach CI.)

3. **RUNBOOK "Last Updated" staleness.** docs/RUNBOOK.md:3 says
   2026-08-29 while its body documents 2026-10-10 changes (atomic
   bundle cuts, two-instance topology) — the header predates the
   content. Not corrected here (owned by the runbook maintainer);
   trust the body's dated claims, which match scripts/ sources.

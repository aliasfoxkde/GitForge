# Changelog

All notable changes to GitForge will be documented in this file.

## [Unreleased]

### Fixed

- **CI trigger status is terminal and request-scoped**: callers can resolve a
  queued event to its pipeline run or observe a terminal publish/consumer
  failure; failed publication releases its waiter and workspace handoff, and
  concurrent triggers cannot overwrite each other's workspace. Trigger-status
  polling accepts the configured CI trigger credential as well as explicitly
  configured compatibility/operator credentials. The status journal remains
  best-effort correlation, not a durable outbox.
- **Manual CI runs retain their selected pipeline identity**: an explicit
  active pipeline ID now travels through the CI trigger event and is stored
  on the created run; inactive and cross-repository selections fail closed.
  Ordinary push events continue loading the pipeline definition committed at
  the pushed revision. A queued event ID is only a correlation handle, not a
  durable-delivery guarantee: the CI event bus is currently in-memory and a
  durable trigger outbox remains required for crash-safe acceptance.

## [0.6.14] - 2026-10-03

### Added

- **Ref-update policy with required status checks (#240)**: repositories
  can require named pipelines to be green on a commit before
  `refs/heads/*` advances to it, and can deny non-fast-forward branch
  updates. Required checks are evaluated pre-receive in the git server on
  both Smart HTTP and SSH — a violating push is declined with a standard
  receive-pack report (`! [remote rejected] <ref> (reason)`); non-FF
  denial delegates to git's native `receive.denyNonFastForwards`.
  Managed via `GET/PATCH /repos/{owner}/{repo}/policy` and
  `GET /repos/{owner}/{repo}/commits/{sha}/status`, plus
  `gitforge repo --policy / --set-policy / --commit-status`. Merged as
  PR #255 (cd5bb758).

- **Per-job lease liveness (#243)**: runners prove a running job is alive
  with a per-job `jobs.heartbeat_at` refresh, and fencing waits for the
  job to go quiet instead of judging from the runner's global heartbeat —
  long container builds are no longer killed because the runner's
  heartbeat starved under host load. Merged as PR #253 (ffbc21a5).

## [0.6.13] - 2026-10-02

### Fixed

- **Owner-prefixed repo names no longer create unresolvable repos**:
  `gitforge repo --create mkinney/kubix` stored the full path as the
  repository name, but git serving resolves repos by (owner username,
  bare name) — the repo row existed yet never resolved, 404ing every
  clone/push. The API now rejects `/` in repo names outright (the
  owner is derived from the auth token; no org handling exists to
  honor a prefix), and the CLI accepts `owner/name` per the
  documented syntax while sending only the bare name (43833379).

- **Pipeline listings only include active pipelines**: the pipeline
  query and its route-contract tests now filter inactive rows, ending
  the accumulation of per-push duplicate pipeline records in list
  output (752ea428, absorbed into this cut from
  `fix/pipeline-list-active-20261002`).

## [0.6.12] - 2026-10-01

### Fixed

- **Aegis baseline regenerated** after the trigger-budget and route-test
  commits shifted scanner line anchors, and a scanner-triggering
  `'../etc/passwd'` literal in the webhook repo-mismatch test was
  replaced with an equivalent non-UUID rejection value.

### Tests

- **Runner registry and webhook trigger routes are covered at the route
  level**: `runners_routes.rs` (8 tests) pins the registration contract
  (adopt-by-stable-name with 201/200 semantics, type aliasing, defaults,
  the `type`-renamed wire shape, admin-or-maintainer retire gate, and
  the `runner_busy` conflict clearing once active jobs complete), and
  `webhook_routes.rs` (6 tests) pins the non-delegating trigger path
  (pipeline/repo validation ordering, run + entry-job persistence,
  replay idempotency cancelling the duplicate run, and the three
  definition rejections) — all against the real router over an
  in-memory database.

### Fixed

- **Pipeline-run triggers no longer 502 under dispatch storms**: the
  gateway's trigger client allowed 10s while the orchestrator's trigger
  handler waits up to 15s to correlate the run id from its durable
  creation path — so the gateway died first and reported a failure for
  triggers that in fact succeeded (observed live 2026-09-29: `POST
  /api/pipelines/{id}/runs` 502ing at exactly 10.0s during a backlog
  storm). The correlation window is now a shared constant
  (`gitforge_common::CI_TRIGGER_CORRELATION_WINDOW`) and the client's
  budget is derived from it with an explicit margin, pinned by
  `trigger_client_timeout_exceeds_ci_correlation_window`; a window that
  elapses under write contention now surfaces as the orchestrator's own
  honest `queued` (202, null run id) instead of a manufactured 502.

## [0.6.11] - 2026-09-29

### Added

- **Pipeline management API + CLI**: stored pipelines can be created,
  listed, triggered, and deleted through the gateway (`POST
  /api/pipelines/{id}/runs` runs a stored pipeline at a given `ref`),
  and pipeline runs plus their job rows are queryable (`GET
  /api/pipeline-runs`, `GET /api/pipeline-runs/{id}`,
  `GET /api/pipeline-runs/{id}/jobs`). Mirrored in the `gitforge` CLI.

### Fixed

- **Git HTTP: gzip request bodies are decoded (F32)**: full clones of
  large repositories failed with HTTP 500 ("bad line length
  character") because git compresses upload-pack request bodies past a
  negotiation threshold and the server piped the still-compressed
  bytes into `git upload-pack`. `Content-Encoding: gzip`/`x-gzip`
  bodies are now inflated in both smart-HTTP POST handlers (with the
  same size cap as raw bodies), and receive-pack commands are parsed
  before the body is consumed.
- **Shallow clones over HTTP**: the hand-built upload-pack ref
  advertisement omitted the `shallow` capability, so every
  depth-limited fetch died with "Server does not support shallow
  clients". `shallow` and `deepen-relative` are now advertised (the
  spawned git child already implemented shallow), with an end-to-end
  depth-1 test.
- **Gateway boot no longer crash-loops on a no-op runner rename**
  (observed live 2026-09-25 as 11 consecutive restart failures): the
  startup duplicate-name migration ran an unconditional UPDATE whose
  lock escalation could return the WAL upgrade-to-write BUSY that
  ignores the busy handler. It now checks read-only first and takes
  `BEGIN IMMEDIATE` up front only when duplicates exist.
- **F36 — engine rebuild adopts existing workspaces**: a rebuilt CI
  engine treated a pre-existing `workspaces/<run_id>` as fatal and
  never registered itself, drifting every interrupted run to the
  orphan reconciler for a false `failed` grade with an intact
  workspace sitting on disk. Rebuild now adopts the workspace and
  resumes the run.

- **F37 — watchdog-reaped timeouts no longer strand runs (custody
  deadlock)**: when the timeout watchdog reaped a hung job, the
  failure→cancel cascade existed only in the engine's memory, and a
  rebuilt engine grafts the reap from the durable rows so it never fired
  it — the doomed downstream job stayed `pending`, the run stayed
  `running`, and neither finalizer would act (the periodic pass deferred
  to the live engine; the engine had already converged its own mirror).
  The doom cascade is now durable: `cancel_doomed_rows` grades every
  never-dispatched row that transitively depends on a failed,
  timed-out, or cancelled row as `cancelled`, using the persisted
  definition's `needs` edges (dispatched rows are left to the runner
  lifecycle; an unreadable definition cancels nothing). Registry custody
  no longer shields a run whose durable rows are all terminal, a genuine
  failure outranks cleanup cancellations in the verdict, and the
  periodic loop reclaims pass-finalized workspaces. Live repro: run
  666b3fa8. Pinned by
  `test_reconcile_cancels_doomed_descendants_of_reaped_job` and friends.
- **F31 residual — evidence-torn job rows now self-heal**: a job row that
  carries its completion receipt (`finished_at` + `result_json`) but lost
  the status/started-at write (the F21/F23 one-shot-write-loss class,
  observed live as bd5c8664 and repaired by hand twice) stranded its run
  non-terminal forever. `JobQueries::reconcile_evidence_rows` grades such
  rows from their own recorded receipt — a recognized verdict wins,
  unparseable evidence fails closed to `failed`, and the evidence columns
  are never rewritten. Runs on the CI watchdog tick and after scheduler
  recovery. Pinned by
  `test_reconcile_evidence_rows_grades_stranded_rows`.

### Changed

- The `pre-commit` hook (installed by `make setup`) ran Go gates
  (gofmt/goimports/go vet) against this Rust workspace — a template
  leftover that either no-op'd or failed spuriously. It now runs
  `cargo fmt --all -- --check` and strict clippy when Rust sources or the
  manifest are staged; the full suite stays in `pre-push`.
- Project `CLAUDE.md` rewritten for the actual Rust workspace (Makefile
  targets, the `.gitforge.yml` chain as the authoritative gate, real
  conventions and coverage calibration) — it previously documented a Go
  repository end to end.

## [0.6.10] - 2026-09-25

Durable DAG planning across control-plane restart, storm-scale CI
trigger outbox insert, container-backend failure classification
(infrastructure vs code), queue dispatch observability, and the
stranded-run reconciliation loop. Validated on the instance (runs
5710ff8f, e9a7d325 — 4/4 jobs each); deployed as
`gitforge-64da53b7-20260925`.

## [0.6.9] - 2026-09-24

`bounded_put` keeps the newest bytes behind a fixed-size marker (F30 —
run summaries and failure evidence survive unbounded-growth truncation);
ledger corrections (F27 resolution, in-sandbox coverage-gate
calibration).

## [0.6.8] - 2026-09-24

F21/F23/F24 durable-write fixes; F25 toolchain preflight; F26 durable
runner registry; F9 rate limiter mounted; `JWT_SECRET_FILE`; SQLite
`BEGIN IMMEDIATE` write discipline; F28 coverage gate calibrated
in-sandbox (82 hard / 84 warn).

## [0.6.7] - 2026-09-23

Interrupted pipeline chains are graded `failed` (`incomplete_chain`)
instead of silently `succeeded` (#212, F19/F20).

## [0.6.6] - 2026-09-23

Deployed release `gitforge-d821d44-20260922` (source `d821d44e`). Three
CI-correctness fixes found by making GitForge's own pipeline actually run
to completion on the refreshed CI image — each observed live before it was
fixed. Includes the previously unreleased history (v0.6.2 shipped
tag-only; v0.6.3–v0.6.5 bundles were never GitHub-released).

### Fixed

- **False-green reconciliation after a CI restart (#212)**: chained jobs
  are enqueued lazily, so a `gitforge@ci` restart mid-pipeline left only
  the head job as a durable row and orphaned-run reconciliation graded the
  run `succeeded` with the rest of the pipeline never executed (observed:
  two runs "succeeded" with 1 of 3 jobs). Reconciliation now compares
  durable job rows against the run's persisted pipeline definition and
  grades a shortfall `failed` (`incomplete_chain=true` in the finalize
  log); unreadable/legacy definitions keep the row-only grading
- **Build-daemon cargo resolution (#213)**: with `CARGO_REAL` unset every
  submitted job ran through `rustup run stable cargo`, which fails where
  the default toolchain is dated with no `stable` alias — including the
  `dsc-ci-rust:6` image. The daemon now resolves the real cargo binary
  once per lifetime via `rustup which cargo`; the bypass env still wins
- **Docker-dependent agent tests (#214)**: nine `agent` tests constructed
  `RunnerAgent` without the module's `docker_daemon_available()` guard and
  panicked inside the CI container. They exercise registration and
  lifecycle state, not containers; they still run wherever a docker
  daemon exists

### Added

- In-repo CI image recipe `infrastructure/docker/ci-rust.Dockerfile`
  (#208) with the cache contract: any `Cargo.lock` change ⇒ rebuild the
  image and bump its tag (deployed at `dsc-ci-rust:6`, which also bakes
  `openssh-client` for the hermetic ssh-protocol suite, #211)
- `GITFORGE_SANDBOX_MEMORY_MB` runner env override (#210); fixes OOM
  SIGKILL of debug codegen/linking under the hardcoded 4 GiB default
- Scheduler queue fairness (#209): fixes job starvation where one repo's
  queued job waited 30+ min while other repos' jobs took freed slots

### Validation

- `gitforge-ci` pipeline (fmt → clippy → test on `dsc-ci-rust:6`) fully
  green on the released commit under the fixed control plane — run
  `c68cd7a8`, the first complete honest green on the refreshed image
- `scripts/gitforge-status`: all four services `current`, overall healthy

## [0.5.0] - 2026-09-21

### Added

- Route-contract coverage for the CI, admin, and SSH-key API surfaces,
  including webhook replay idempotency (a replayed delivery maps to one
  durable job; the replay's own run row is cancelled) and the delegation
  fail-closed path when the CI trigger is unavailable (502, run row
  preserved)
- Exact wire-format contract tests for pipeline-run, job, and runner
  responses — pinning, among other things, that the runner payload
  serializes its type field as `"type"`
- The four mechanical pedantic lints
  (`uninlined_format_args`, `map_unwrap_or`,
  `redundant_closure_for_method_calls`, `ignored_unit_patterns`)
  promoted to `deny` in `[workspace.lints.clippy]`; all nineteen
  workspace members opt in
- Master plan (`docs/planning/MASTER_PLAN_2026-09-20.md`): phased
  roadmap with a findings ledger built from a live instance
  validation, plus the deployment procedure captured from the running
  stack
- Standalone E2E harness (`tests/integration`) repaired to 94/94
  after library API drift; daemon detection now keys on the build
  daemon's Unix socket instead of process-name matching

### Changed

- git-server response construction degrades to a bare 500 response
  (`finish_response`) instead of unwrapping a malformed builder chain
  inside a connection thread; 22 handler chains converted
- `docs/CONTRIBUTING.md` rewritten for the Rust workspace; API.md,
  RUNBOOK, ARCHITECTURE, and DEPLOYMENT corrected against the real
  code; superseded planning docs carry banners
- Code-smell audit recorded in `docs/audits/CODE_SMELLS_2026-09-20.md`
  (130 non-test unwrap sites surveyed with verdicts; 548 pedantic
  findings inventoried with a campaign plan)

### Removed

- Orphaned test suites `tests/service_tests.rs` and
  `tests/cli/cli_test.rs` (owned by no Cargo.toml, superseded by the
  integration harness)

## [0.4.0] - 2026-09-08

### Security

- Git over SSH now works and authenticates: the transport was rewritten
  from ssh2/libssh2 (client-only library — the old listener could never
  complete a handshake, so the port served nothing) to a russh server that
  pipes authenticated channels to real `git upload-pack`/`git receive-pack`
  child processes. Public-key auth is required; accepted fingerprints are
  logged. The ed25519 host key is generated on first boot, persisted under
  the ssh volume (override path with `GITFORGE_SSH_HOST_KEY`), and
  published as `.pub` for `known_hosts` pinning
- Scheduler job completion requires lease proof: anonymous completion of a
  known job is rejected (409), unknown jobs return 404
- Removed the `POST /jobs/{id}/assign` no-op stub that acknowledged
  assignments without performing any
- Runner registration is fail-closed by default
  (`GITFORGE_RUNNER_STANDALONE=deny`); standalone fallback requires explicit
  opt-in
- Git over SSH authenticates against a per-user key registry instead of
  accepting any key on possession: public keys are registered to accounts
  via `POST /api/ssh-keys` (validated OpenSSH parsing, `SHA256:`
  fingerprint, globally unique), the transport resolves presented keys
  against that registry and rejects unregistered ones, and a broken
  registry fails closed rather than letting connections through
- docker-compose requires `GITFORGE_SCHEDULER_TOKEN` for CI and runners,
  matching the fail-closed scheduler auth boundary

### Added

- Git Smart HTTP protocol integration tests: real `git push`, `clone`,
  `fetch`, and `ls-remote` against the spawned git-server binary
- API gateway flow integration test: boots the real `api` binary against
  a temporary database, logs in with a seeded bcrypt-hashed account,
  and drives authenticated repository creation and the SSH key registry
  over HTTP, asserting the rows in the service's own database
- CI trigger flow integration test: boots the real `ci` service against
  a temporary database and bare repository, requires the trigger token,
  and asserts the run, job commands, image, and workspace clone all come
  from the committed `.gitforce.yml` at the pushed revision
- AI provider HTTP boundary tests: each provider (OpenAI, Anthropic,
  Ollama) is pointed at a local scripted HTTP server and driven through
  health checks and reviews with realistic wire-format responses,
  asserting auth headers, status-to-error mapping (429/401/5xx), finding
  parsing with severity/category fallback, and cost/token accounting.
  gitforge-ai went from 57.86% to 90.16% lines
- Build daemon protocol tests: the request/response connection handler
  is driven over real unix socket pairs — invalid and unknown job ids,
  empty list/stats, the socket shutdown request raising the shared
  flag, and oversized or undecodable requests refused without a
  response — plus a round trip that submits a real `cargo --version`,
  polls it to completion across connections, and lists the finished
  job. daemon.rs went from 19.78% to 77.92% lines
- Protocol and trigger test harnesses stop their spawned services with
  SIGTERM (the real graceful-shutdown path), so `cargo llvm-cov` now
  counts the entry-point code they exercise; workspace line coverage
  rose from 79.63% to 83.40% with no production changes
- Job log store tests: `bounded_put` receipts are verified end to end —
  SHA-256 over the stored content, byte counts, the `gitforge://log/`
  URI, truncation of oversized logs (kept bytes are the head of the
  log and the receipt reflects the truncated size), exact-boundary
  non-truncation, and overwrite replacing the receipt — plus on-disk
  delete removing both log and metadata, listing that skips corrupt
  metadata files, and get returning None when only metadata remains.
  job_logs.rs went from 62.84% to 92.94% lines
- Code review crate tests: multi-file diff parsing with new, deleted,
  and binary file markers, single-line hunk headers (`@@ -3 +3 @@`),
  the ParsedDiff → FileChange bridge (change-type mapping and hunk-text
  round trip), diff stats and complexity flags, every vulnerability
  severity mapping, context-line scanning with deletion lines ignored,
  extension-scoped patterns skipping extensionless files, and findings
  aggregated across files. gitforge-review went from 83.87% to 99.37%
  lines
- Process crate tests: the SIGTERM shutdown handler is driven by
  delivering a real SIGTERM to the test process (with a guard stream
  armed first so the delivery can never kill the binary),
  `wait_for_shutdown` returns once its flag is set, and the process
  pool's `spawn` is exercised over real children — tracked until exit,
  reaped by the timeout arm against a hung `sleep`, and rejecting an
  unknown program without a ghost tracking entry. gitforge-process
  went from 86.11% to 93.02% lines; signal.rs reached 100%
- Runner scheduler-boundary tests: a request-recording HTTP harness
  drives the runner's reporting pipeline without Docker — job claims
  yield lease tokens or fail closed on rejection, malformed payloads,
  and unreachable schedulers; live log chunks carry `[stdout]`/
  `[stderr]` labels and split multibyte payloads at UTF-8 boundaries;
  final step output streams in bounded chunks per step; artifact
  uploads assert runner/lease/checksum headers, refuse checksum drift
  and path escapes, and no-op without a workspace; completion receipts
  stay bounded when a 3-byte character straddles the byte limit.
  agent.rs went from 75.55% to 86.72% lines (the remainder is the
  Docker-gated execution path)
- Git-server edge-path and CI-outbox tests: a spawned-binary suite
  drives the Smart HTTP handler branches real `git` never produces —
  legacy and path-suffixed routes, unknown and storage-less
  repositories (404), a database-less instance returning 503 on every
  git route while `/health` keeps serving, oversized bodies rejected
  as 413/400 under `GITFORGE_MAX_GIT_BODY_BYTES`, malformed packs as
  500s — plus the durable push → `events` outbox → CI trigger
  delivery: bearer-token and ref/hash payload assertions, lease
  reclaim of a stale `delivering` row, and requeue-on-failure with
  attempt counters when the trigger endpoint fails. services/git-server
  main.rs went from 67.83% to 91.77% lines
- Git over SSH protocol integration tests: real `ssh-keygen` client
  keypairs and host-key pinning; `push`, `clone`, `fetch`, and `ls-remote`
  over the `ssh://` transport, plus rejection of key-less clients,
  unregistered keys, and unknown repositories
- Periodic orphaned-run reconciliation in CI (60s loop, 120s run-age grace,
  live-engine guard); startup still sweeps once
- Restart recovery stops orphaned executions: the scheduler's cancellation
  probe reports every terminal durable status, the runner skips log,
  artifact, and completion reporting once its outcome was decided
  mid-execution, and credentialed completions for unassigned jobs are
  rejected 409 with an explicit orphaned-outcome message
- Runner registration retries an unreachable or not-ready scheduler with
  bounded exponential backoff (`GITFORGE_REGISTER_ATTEMPTS`, default 6;
  `GITFORGE_REGISTER_BACKOFF_SECS`, default 1s, doubling to a 30s cap) so a
  runner started beside a restarting control plane survives the compose race
  instead of exiting; auth rejections (401/403) are never retried
- ShellCheck and actionlint gates in Rust CI and `make lint`
- cargo-vet supply chain (`supply-chain/`) behind `make lint`
- cargo-vet enforcement in Rust CI: a `supply-chain` job runs `cargo vet`
  so dependency changes that lose audit coverage fail CI. Five public
  audit registries (isrg, google, mozilla, bytecode-alliance,
  embark-studios) are registered and pinned in `imports.lock`, and the
  dependencies introduced by the SSH transport rewrite are recorded as
  tracked exemptions so the gate is green without pretending they were
  audited

### Changed

- The git-server image no longer ships `openssh-server`: SSH is served
  in-process, so the container carries no sshd

### Fixed

- Anthropic reviews always failed to parse: the response struct expected
  a JSON key literally named `type_` while the API sends `"type"`, so
  every real `generate_review` call returned a parse error. The health
  check had masked it because it only reads the status code
- API list endpoints return 500 `database_error` instead of masking
  storage failures as empty 200 responses
- ai-review.yml passed review outputs as action inputs instead of step
  env vars, so the PR comment always used its fallback text
- ShellCheck SC2012/SC2034/SC2155 findings in scripts/
- Compose stack could not run a pipeline end to end (found by a live
  queued-job smoke, all fixed and validated):
  - api, ci, and git-server used separate SQLite volumes and git-server
    had no `DATABASE_URL`, so pushes and pipeline triggers could not
    resolve repositories; they now share one `gitforge-data` volume
  - `sqlite:` database URLs open an existing file only; compose now uses
    `?mode=rwc` so first boot creates the database
  - git-server port mappings pointed at ports the server does not bind
    (in-container ports are 42022/42782)
  - api provisioned repositories outside the shared git volume (no
    `GIT_ROOT`) and ci could not read them (no `/git` mount)
  - images lacked the volume mountpoints, so Docker seeded fresh volumes
    root-owned and the non-root services could not write; the Dockerfile
    now creates `/data` and `/git` with `gitforge` ownership
  - the ci image had no `git` binary, so loading `.gitforce.yml` from a
    pushed revision failed with ENOENT
  - the non-root runner could not open the mounted Docker socket; compose
    now maps the host docker group via `DOCKER_GID`
  - run workspaces are bind-mounted at a host-identical absolute path
    (`GITFORGE_WORKSPACE_HOST_DIR`): the runner passes workspace paths to
    the host Docker daemon as bind sources, so a container-only path made
    every job see an empty auto-created directory

## [0.3.2] - 2026-08-28

### Added

- MockAiProvider for AI testing with configurable behavior (success, failure, rate limiting)
- Executor unit tests (7 new tests for JobResult, ExecutableJob, compute_output_sha)
- Clone derive on AiError for mock support

### Changed

- Coverage improved from 79.80% to 80.12%
- 46 total tests in gitforge-runner (up from 39)

## [0.3.0] - 2026-08-28

### Fixed

- Storage race condition: Added `sync_all()` calls to ensure artifact and cache data is flushed to disk before returning
- Scheduler stale snapshots: Reject runner-loss snapshots that are older than current state
- Scheduler persistence errors: Fail closed on database errors to prevent state corruption
- Build daemon: Proper shutdown coordination and cargo flag forwarding
- Multi-repository recovery: Preserve repository context during job recovery

### Improved

- Workspace rustfmt applied consistently
- CI pipeline streaming with lease-fenced logs
- SBOM and artifact provenance attestations added
- LLVM coverage ratcheting in CI

### Documentation

- Git at Scale research: Analysis of Cursor's distributed Git architecture
- Comprehensive execution plan: 8-phase roadmap with evidence-based tracking

## [0.2.0] - 2026-07-14

### Added

#### Phase A - Security Hardening (COMPLETED)
- JWT authentication enforced on all API routes except /health and /metrics
- Auth middleware with token validation
- AuthenticatedUser extractor for route handlers
- Public paths: /health, /metrics, /swagger-ui, /api-docs
- Protected routes: /api/repos, /api/pipelines, /api/pipeline-runs, /api/jobs, /api/runners, /api/artifacts

#### Phase B - Runner-Scheduler Communication (COMPLETED)
- Real HTTP client implementation in runner agent using reqwest
- Runner registration via HTTP POST to scheduler
- Heartbeat loop sending POST to scheduler
- Job fetch loop polling GET /jobs/pending
- Scheduler HTTP server with routes for runners and jobs
- Graceful fallback when scheduler is unavailable

#### Phase C - Event Pipeline Triggering (COMPLETED)
- CI service event consumer subscribed to push events
- Pipeline triggered automatically on push received events
- Default pipeline definition generated per repository
- Jobs enqueued to scheduler on pipeline start

#### Phase D - Artifact Storage (COMPLETED)
- Routes wired with FileStorage integration
- Get artifact metadata from storage
- Delete artifact from storage
- Auth enforced on all artifact routes

#### Phase E - Docker Deployment (COMPLETED)
- Multi-stage Dockerfile for minimal production images
- Separate images for api, ci, runner, and git-server
- docker-compose.yml with all services configured
- config.toml.example with all configuration options
- Docker-in-Docker support for runner

#### Scheduler HTTP Endpoints (COMPLETED)
- POST /runners - Register new runner
- POST /runners/{id}/heartbeat - Runner heartbeat
- GET /jobs/pending - Get pending jobs for runner
- POST /jobs/{id}/assign - Assign job to runner
- POST /jobs/{id}/complete - Mark job complete

### Fixed

- Fixed unused imports across multiple crates
- Fixed serde_json missing dependencies
- Fixed EventFilter export from gitforce-events
- Fixed JobDefinition/StepDefinition exports from gitforce-ci
- Fixed auth middleware compatibility with Axum 0.7
- Fixed ArtifactId private field issue with From<Uuid> implementation
- Resolved all clippy warnings for strict linting (-D warnings)
- Fixed needless borrows, unnecessary map_or, if_same_then_else
- Fixed derivable_impls with #[derive(Default)]
- Fixed never_loop by replacing while with if in scheduler
- Fixed Priority enum ordering with proper Default

### Linting & Quality

- Zero clippy warnings with strict -D warnings
- All tests passing (200+ tests across workspace)
- Full workspace builds successfully
- Git hooks installed via make setup
- GitHub Actions Rust workflow with test, lint, build, coverage, security

## [0.1.0] - 2026-07-06

### Added

#### Core Infrastructure
- `gitforce-common` - Shared types, UUIDs, errors, time utilities
- `gitforce-db` - Database models and connection pool
- `gitforce-events` - Event bus and event type definitions

#### Git Server
- `gitforce-core` - Git protocol handlers and repository management
- Repository storage backend (FileStorageBackend)
- Git SSH and HTTP protocol handlers
- Hook execution system (pre-receive, post-receive)

#### CI/CD
- `gitforce-ci` - Pipeline orchestration and DAG execution
- Pipeline definition loader (YAML-based)
- Job state machine (Pending → Queued → Assigned → Running → Completed)
- `gitforce-scheduler` - Job queue and runner assignment
- Priority-based job scheduling
- Runner selection policies

#### Execution
- `gitforce-runner` - Job execution agent
- Runner registration and heartbeat
- `gitforce-sandbox` - Container isolation
- Docker sandbox implementation with bollard (real Docker integration)
- Resource limits support

#### Storage
- `gitforce-storage` - Artifact and cache storage
- Filesystem-based artifact store
- Cache store with key/retrieval

#### API
- `gitforce-api` - REST API gateway
- Repository endpoints (wired to SQLite)
- CI/CD endpoints (pipelines, jobs, logs - wired to SQLite)
- Runner management endpoints (wired to SQLite)
- Artifact endpoints
- JWT authentication
- OpenAPI 3.0 / Swagger UI documentation

#### Services
- `git-server` - Git SSH/HTTP server binary
- `ci` - CI orchestrator binary
- `runner` - Runner agent binary
- `api` - HTTP API server binary

### Technical Details

- **Language**: Rust (10 crates, 4 service binaries)
- **Async Runtime**: Tokio
- **Web Framework**: Axum 0.7
- **Database**: SQLite (MVP) via sqlx
- **Git Library**: git2
- **Container Runtime**: bollard (Docker client)

### Next Steps

- CLI tool for GitForge
- Cloud sync protocol

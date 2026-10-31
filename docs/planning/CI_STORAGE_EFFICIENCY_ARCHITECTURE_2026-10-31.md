# GitForge Execution Efficiency Architecture

**Date:** 2026-10-31  
**Status:** Proposed implementation architecture; no runtime behavior changes in this documentation PR  
**Umbrella:** #342  
**Workstreams:** #338, #339, #340, #341

## 1. Executive recommendation

GitForge should evolve into a resource-aware execution platform with three coordinated foundations:

1. **Admission-controlled execution:** separate control-plane traffic from compute, and schedule fast, standard, heavy and background work with explicit resource reservations and fairness.
2. **Immutable shared storage:** use a content-addressed store for reusable non-Git artifacts, with explicit manifests/references, per-trust-domain namespaces, managed workspaces and lease-aware garbage collection.
3. **Sound incremental validation:** map code changes to tests/features, reuse prior test results only when all relevant inputs are fingerprint-identical, and fall back to wider validation whenever the graph or cache cannot prove safety.

Do not start by replacing GitForge's scheduler, Git storage, Cargo, Docker, or every language toolchain. Preserve the current durable queue/lease/receipt model, build the missing contracts around it, benchmark in controlled stages, and land independently deployable changes.

The highest leverage is likely to come from queue admission, avoiding duplicate heavy validation, shared immutable dependency/compiler artifacts, and isolation of writable workspaces. Changing implementation languages or adopting an experimental compiler is a secondary, evidence-driven optimization.

## 2. Current-state audit

Repository baseline inspected: main at 5ceb106d60f3eb528392daac6ad258cc317edc18 (latest main commit visible during this audit).

### 2.1 Existing strengths to preserve

- Rust workspace with separate API, CI engine, scheduler, runner, sandbox, process, build, storage, database, Git protocol and CLI crates.
- Durable scheduler/database job state, leases, cancellation, runner heartbeat handling and restart recovery.
- Pipeline DAG with dependencies, environment propagation, skip conditions, and a cache block that expands into a host volume plus tool environment.
- Queue has Normal/High/Low priorities and repository-aware fairness based on currently running jobs. It also exposes fair ordering for queue-status reporting.
- A narrow resource class already prevents two workspace-wide Cargo test jobs from overlapping on one runner.
- Runner Docker integration supports host binds, tmpfs and named volumes, timeout/resource limits, sandbox cleanup and backend health checks.
- Storage traits exist for artifacts, caches, logs and receipts. The open #336 proposal adds atomic publication of individual file-backed records.
- The self-hosted pipeline already uses image-keyed Cargo target mounts, and a mount failure is designed to degrade to a cold build rather than change correctness.

These are valuable primitives. The proposed work should extend them rather than create duplicate job identity, object identity, lease, or receipt systems.

### 2.2 Confirmed gaps

| Area | Current state | Consequence |
|---|---|---|
| Queueing | Priority heap and repository fairness, but no general resource-vector admission or explicit fast/heavy lanes | Large jobs can compete with small validation and consume scarce host resources |
| Queue fairness | Priority is ordered before repo load; aging/weighted service guarantees are not a complete contract | Continuous higher-priority traffic can starve lower classes |
| Resource classes | One special workspace-Cargo-test class; resource use is not generalized across jobs | CPU, RAM, pids, scratch and I/O are not admitted as one budget |
| Cache identity | CacheKey components are concatenated and filesystem name uses 16 hex characters | Ambiguous component boundaries and truncated identifiers should be removed |
| Cache storage | Key-addressed data plus JSON metadata; no shared immutable blob layer or explicit global reference graph | Identical content is duplicated; retention and GC are difficult |
| CI cache mount | Cargo target path keyed by a sanitized image string | Different image names can normalize to the same path; cache correctness should use canonical digests |
| Workspaces | Native Git worktrees/manual lifecycle and runner workspaces | Ownership/cleanup and per-job I/O costs remain operator burdens |
| Concurrent builds | Current self-pipeline is intentionally a linear chain because the jobs share a mutable workspace/target | Parallelism is currently limited; simply fanning out jobs would be unsafe |
| Test selection | Changed-file / affected-crate filtering exists in prior work, but test-level change-impact and trusted result reuse are not on main | Full test/coverage work can still be repeated unnecessarily |
| Observability | Queue/job status, receipts, logs and metrics exist | Missing unified queue wait, host pressure, physical I/O, cache effectiveness and workspace-cost signals |

The October 10 worktree census documented 44 live Git worktrees: 25 clean, 17 with small dirty WIP, one additional dirty main worktree, and one 3,962-file build-artifact spill. The important finding is not the exact historical count; it is that Git cleanup must be owner/lease aware and distinguish build output from source changes.

### 2.3 Existing work to review rather than duplicate

- Issue #322 and branch docs/gitforge-smart-ci-dependency-graph-20261019 contain a gitforge-graph crate, cargo-metadata graph ingestion, affected-crate and feature queries, fast CLI checks, a changed-crate script, feature/known-issue data, and an impact-run migration. At audit time that branch was six commits ahead but 70 commits behind main. Rebase and independently review those commits; do not merge the stale branch wholesale.
- Issue #277 has actual evidence of extreme host pressure, D-state waits, high storage reads/writes and low free disk during heavy CI. This is the rationale for admission and resource telemetry.
- Issue #284 documents overlapping Rust CI, Integration Tests, GitForge CI and other Actions lanes competing for the same self-hosted pool.
- Issue #318 documents public fork PR code executing on persistent self-hosted infrastructure and required-check gaps. This is a security gate for any shared cache or runner design.
- PR #336 proposes atomic same-directory temporary publication for individual artifact/cache/receipt files. It does not provide a transaction spanning data and metadata, and does not yet supply the CAS/refcount/GC architecture.
- PR #337 and PR #258 are separate release-contract and SSH-trigger work. Do not mix their ownership with the storage/queue changes.

### 2.4 Current validation blockers observed

At main commit 5ceb106d, the October 30 workflows showed:

- Rust Build/Test/Clippy failed compiling services/git-server due to an incompatible russh auth_publickey callback signature in services/git-server/src/ssh_server.rs.
- Rust Security failed compiling services/api/tests/api_gateway_flow.rs because the test referenced an undefined harness_passphrase.
- Supply Chain failed cargo-vet because 15 dependency versions lacked safe-to-deploy audit attestations.
- CodeQL Rust analysis was blocked by the same git-server compilation error.
- The GitForge self-hosted poll step received HTTP 401 when querying the pipeline-runs API, causing the status adapter to time out/fail rather than read the durable run outcome.
- PR #336's inspected head had Build, Test, Clippy, Coverage, ordinary Security Audit and Integration green, but Supply Chain still failed on the 15 missing attestations and GitForge polling still returned 401. Its full required gate was not green.

These are release blockers. Cache optimization cannot turn an uncompiled or unauthenticated gate into valid evidence. Resolve them in Gate 0 and do not report a green release based on partial CI.

## 3. Target system architecture

### 3.1 Separate control-plane and compute-plane concerns

Keep API, auth, Git smart HTTP/SSH, webhooks, trigger/outbox delivery, runner heartbeats, cancellation, queue inspection and receipt writes responsive under compile/test load.

No expensive compilation, package installation, test selection scan, archive extraction, or large filesystem traversal should run synchronously on a request path. Persist the intent/outbox record, acknowledge it within a bounded durability budget, and let workers perform the compute asynchronously.

Measure the request path separately from job throughput. An idle API must not be penalized by a large number of queued jobs that have not been admitted to execute.

### 3.2 Work lanes

| Lane | Intended work | Scheduling objective |
|---|---|---|
| Control | Auth, Git refs/receive-pack response, webhook persistence, heartbeats, cancel/status, lightweight metadata | Predictable low tail latency |
| Fast | Format, manifests, route/schema checks, changed-file static checks, targeted small tests | Short feedback time; reserved capacity |
| Standard | Normal build and affected test suite | Fair throughput |
| Heavy | Full workspace tests, coverage, fuzz, mutation, release build, large integration suites | Bounded resource consumption and fair progress |
| Background | Cache prewarm, graph rebuild, GC, log compaction, scheduled full validation | Run only with spare capacity or explicit schedule |

These are scheduling classes, not five independent schedulers. Keep one durable job identity and one lease contract. Add lane and resource request fields to that contract.

Fast-lane capacity must be reserved but bounded. Use weighted service/aging so a constant stream of fast work cannot starve heavy validation. Heavy work must be deferred fairly, not silently lost. Queue status should state exactly why an item is waiting.

### 3.3 Resource-vector admission

Each job declares or receives a resource class with a request/limit vector:

- CPU quota / shares and maximum child process count.
- Memory reservation and hard limit.
- Scratch/workspace/cache capacity.
- I/O weight or concurrency budget where supported.
- Network policy and optional egress class.
- Expected wall time and timeout.
- Runner/image/platform requirements.

The scheduler admits work only when both runner capacity and host-level headroom allow it. Host metrics should include cgroup CPU/memory, PSI CPU/memory/I/O where available, disk free bytes/inodes, I/O throughput/latency, Docker storage headroom, active container count, process count and queue wait.

Use measured runner profiles, not hard-coded guesses. A 2-core/2-GB host should normally admit far fewer concurrent heavy compiles than a larger runner. A small fast lane may still run while one heavy task is executing, provided measured budgets allow it.

When there is not enough capacity, leave the job durably queued with a reason such as runner_unavailable, cpu_budget, memory_headroom, scratch_headroom, io_pressure, per_repo_limit, lane_limit or dependency_wait. Do not spawn an unbounded number of tasks that only wait for semaphores.

### 3.4 Fairness and overload behavior

- Weighted fair dispatch across tenant and repository, with per-repo in-flight limits.
- Priority aging so older normal/heavy jobs eventually advance.
- Per-lane concurrency and reserved fast-path slots.
- Admission caps on total pending work, with explicit policy response/backpressure to the trigger source when safe.
- Idempotent trigger coalescing only when semantics match (same repository, ref/commit, pipeline definition digest, actor/trust context and trigger class). Never merge a manual run or privileged run into an untrusted PR run.
- Queue reasons and age visible through CLI/API/metrics.
- Circuit breakers for degraded storage/runner state; keep health/status endpoints light.

### 3.5 Workspace and storage model

Keep Git's own object database canonical for commits, trees and blobs. Git worktrees already share Git object storage; the extra bytes to address are checked-out working files, Cargo target/build outputs, package/dependency stores, container layers, test outputs, logs and generated artifacts.

Introduce a content-addressed store (CAS) for immutable non-Git objects:

- Full digest (BLAKE3 or SHA-256) and size, verified on write/read.
- Canonical, length-delimited encoding for cache keys; do not concatenate ambiguous fields or use truncated digest prefixes as identity.
- Immutable blob paths and manifest/reference records.
- A durable ref/index that points from tenant/repository/job/commit/toolchain manifests to object digests.
- Namespace and trust policy on every reference.
- Per-object temporary write, file sync and atomic rename; recovery reconciles partial file/index publication.
- Optional chunked storage for large archives/logs only after measurement shows dedup savings exceed hashing, indexing and metadata overhead.

Cache identity must cover the complete reproducibility boundary: source tree digest, lockfile(s), toolchain version, target triple, OS/arch/ABI, runner image digest, features/profile/build flags, pipeline definition, relevant environment contract, test binary/args and graph schema. Never put secret material in the key; use a secret-version identifier only when a secret affects the output and the execution semantics can prove it.

The current sanitized image string used for Cargo cache paths should be replaced by a canonical image-reference digest. Sanitization can collide (different punctuation can become the same hyphen).

#### Workspace providers

Use a portable provider API with layered backends:

1. Native Git worktree and separate target/cache directory: portable baseline.
2. Reflink copy or OverlayFS lower/upper/work directories on supported Linux filesystems.
3. Btrfs read-only base subvolume plus per-job writable snapshot when the runner filesystem is Btrfs and all relevant paths reside on the same filesystem.
4. Safe ordinary copy fallback.

A workspace record owns its path, repository, commit, branch/ref, creator/agent/job, lease, state, filesystem provider, base digest, created/last-used time and cleanup policy.

Do not hard-link mutable files. A change in a hard-linked file can mutate the shared inode. Do not symlink writable target output to a shared location. Shared package/cache store entries should be immutable and read-only to consumers; only use same-filesystem hard links where the provider, trust model and mutation guarantees are proven.

Do not share a writable Cargo target directory between concurrent unrelated builds. Parallel CI lanes need separate workspaces and target outputs, or a cache layer that deduplicates compiler invocations without concurrent mutation. A shared Cargo registry/git source download cache is a different risk class from sharing target/.

Btrfs snapshots share copy-on-write extents and are useful for quick workspace branching, but they are not backups. Keep backup/restore independent. Check snapshot support and quota behavior on the host before relying on it; retain a non-Btrfs fallback.

#### CAS references and garbage collection

Use explicit references and leases. GC must:
1. Mark live objects from durable manifests, active jobs/leases, worktrees, pinned releases, retention rules and recovery records.
2. Produce a dry-run report with object IDs, logical/physical bytes, reason and owner.
3. Move eligible unreferenced objects through a quarantine/grace period.
4. Recheck references/leases and only then delete.
5. Reconcile orphan metadata and orphan blob files without guessing ownership.

Protect against the publish-then-reference race: a just-published object must be protected by an upload/transaction lease until the durable reference is committed. GC must never remove an object merely because its reference registration has not completed yet.

Expose logical bytes, physical bytes, dedup ratio, cache hit/miss, verified reads, corrupt objects, active pins, leased bytes, garbage candidates and bytes actually reclaimed.

### 3.6 Session, log and debug knowledge compaction

This is a distinct data lifecycle built on the immutable store, not a lossy replacement for raw logs.

Keep the raw source immutable and recoverable. Add derived layers:

- Normalize logs and conversation traces into structured events (timestamp, source, tool, command, result, file/path references, error, test outcome, token/resource usage).
- Compress repetitive streams and deduplicate byte-identical chunks.
- Store extracted decisions, constraints, fixes, test evidence and reusable knowledge as a versioned summary/knowledge artifact.
- Include source digest and exact byte/line/turn offsets for provenance.
- Provide search/retrieval back to the raw object.
- Make retention policy explicit per raw logs, traces, summaries and artifacts.

A 2% retained-size target is a benchmark hypothesis, not a promise. A summarizer must not discard critical decisions, error messages, security findings, commands, test evidence or provenance. Build a golden recall corpus; compare extracted knowledge against source and require exact retrieval of critical facts before raw retention can be reduced.

## 4. Sound incremental validation and test-result reuse

### 4.1 Reuse the existing graph work

Rebase issue #322's graph branch onto current main and inspect each commit. Retain the graph/node/edge model and CLI fast-check pieces where correct, but update storage, pipeline and scheduler integration contracts. Do not assume that crate-level reverse dependencies are sufficient for test-level correctness.

### 4.2 Build a test impact index

Collect:
- Static dependencies from Cargo metadata and language build/project manifests.
- Test-to-code dynamic coverage data, ideally file/function/symbol granularity.
- Test binary identity, filter/arguments, fixtures, environment and runner image.
- Feature-to-code and feature-to-test links.
- Known failing, flaky, ignored, global-state, integration and network-dependent tests.

For a change, compute the affected closure. A changed implementation file selects tests that executed that code and tests in dependent crates/modules. New/changed tests always execute. Failed/flaky tests are never reused from a prior pass. Global integration tests and tests with unknown dependencies remain included.

### 4.3 Safe test cache

A prior pass is reusable only if the complete test fingerprint matches:

- test identity/binary and test args/filter;
- source and transitive dependency digests;
- dependency lockfiles and generated inputs;
- compiler/runtime, target, OS/arch/ABI and runner image;
- build profile, features and relevant flags;
- fixtures, configuration and environment contract;
- graph/index schema and execution adapter version.

Tests that observe time, random sources, network, mutable external services, global state, host files or non-hermetic secrets are non-cacheable until those inputs are captured or isolated.

Record an explicit result class: executed_pass, executed_fail, flaky, timeout, skipped_by_selection, cache_reused, or not_eligible. Cache reuse must not be represented as a test that just ran. Receipts should show the cached result's source commit/fingerprint and age.

If graph construction, coverage, cache lookup, object verification or environment matching fails, fail open to the conservative test set (up to the full suite). Require periodic full-suite and full coverage/fuzz jobs, plus release full validation. During rollout, run selected and full suites on the same commit and compare; any missed failure blocks enabling skip decisions.

### 4.4 Shared test-runner interface

Use the existing OpenTestSuite prototype as an external test adapter project rather than creating a parallel test framework inside the scheduler. The adapter contract should support discovery, selection, execution, retries, per-test resource labels, test groups, output/trace/coverage collection, structured result, artifact publication and deterministic environment identity.

For Rust, evaluate cargo-nextest as the execution runner, preserving Cargo as build/metadata manager and cargo test as fallback. For Python, pytest-testmon is a useful reference for dynamic test dependencies based on Coverage.py; its own docs require an initial full test run with collection enabled. It is not a direct implementation for Rust. For JS/TS, normalize Biome/Oxlint diagnostics and test runner outputs behind language adapters.

## 5. Toolchain and runtime optimization research

The linked projects are valuable, but each should be treated according to maturity and benchmark scope.

| Project/tool | What it contributes | Proposed use | Limitation / guardrail |
|---|---|---|---|
| t3dotgg/lust-lang | Rust-subset compiler with daemon and incremental edit loop; prototype README reports very fast checks/builds for its benchmark | Experimental agent fast-check pilot for code within supported subset | Current README says no generics, traits or closures. Do not replace rustc, Clippy, Cargo or release tests |
| pingdotgg/ts-rust | Rust port of TypeScript 7 compiler/checker/LSP with conformance and performance work | Optional TypeScript checker pilot on real project fixtures | Early release pinned to a particular TypeScript revision; require exact diagnostics/build-mode/LSP comparison before it can gate |
| cargo-nextest | Process-per-test runner, test concurrency controls, groups, retries and structured reporting | Primary Rust test runner pilot | Cargo still builds; tune parallelism to allocated CPU/memory, not host core count |
| sccache | Local/remote compiler invocation cache | CI compiler cache for cacheable rustc invocations | Rust incremental compilation and some linked crate types are not cacheable; measure cache hit rate by crate |
| cargo-chef | Docker dependency-layer cache | Improve prebuilt CI image/dependency layer construction | Not a replacement for incremental builds inside a live workspace |
| mold / LLD | Faster native linkers | Controlled linker benchmark for Rust/C/C++ projects | Verify platform/toolchain compatibility and binary correctness |
| Biome / Oxlint | Fast JS/TS lint/format implementations | Use for rules that match the project's required ESLint/Prettier behavior | Do not silently drop plugins or rule coverage |
| uv / Ruff / pytest-xdist | Fast Python env/package operations, lint/format, parallel test execution | Python project adapter defaults after compatibility test | pytest remains the authority; cap concurrency to resource requests |
| pytest-testmon | Per-test coverage dependency tracking and changed-code test selection | Reference implementation for dynamic test impact | Initial full collection required; non-deterministic test inputs require conservative invalidation |

### Rust-specific strategy

- Keep Cargo as workspace/build orchestrator. Avoid describing nextest or sccache as replacements for Cargo.
- Benchmark cold/warm Cargo registry and Git source caches, target artifacts, sccache, cargo-chef image layers, cargo-nextest, and mold/LLD independently.
- Separate compilation concurrency from test execution concurrency.
- On dedicated self-hosted runner profiles, tune Cargo build jobs and nextest threads to the actual CPU/memory reservation. Do not set each to all host cores when multiple jobs can overlap.
- Consider building dependency layers once per toolchain/image/lockfile digest, then reuse through immutable artifacts.

### Service/API strategy

The server challenge repositories are useful because they make workload shape, correctness, database configuration and latency objectives explicit. Apply those principles to GitForge endpoints and queue paths, not their user counts:

- 1-core/2-GB/SQLite profile: control-plane behavior under severe memory and CPU constraints, with durable writes.
- 8-core/Postgres multicore profile: concurrency, database contention, scheduler throughput and p95/p99 under mixed load.
- 16-core service profile: connection count, memory, accept-loop/runtime behavior and database separation.
- Database configs: reproduce each database with same schema/workload before changing production config. Do not copy parameter values across hardware/DB versions.

Profile before replacing async frameworks, rewriting services, switching languages or hand-tuning database settings. The likely first wins are queue admission, prepared statements/pool bounds, request-path isolation, fewer repeated builds, cache correctness and avoiding I/O storms.

## 6. Benchmark and validation programme

### 6.1 Baseline workload

Create reproducible repositories representing:
- Rust workspace with several crates, tests, proc macros and build scripts.
- TypeScript monorepo with project references, test packages and lint plugins.
- Python workspace with local packages, pytest fixtures and integration tests.
- Large repository with generated assets, logs and artifacts.
- Multi-project burst with many small pushes plus a few heavy full builds.
- Failure cases: storage near full, cache corrupt/missing, runner outage, DB slow/locked, stale lease, duplicate webhook, scheduler restart and cancellation.

### 6.2 Profiles

Measure on 2-core/2-GB, 4-core, 8-core/16-GB and 16-core/32-GB classes where available. Include local NVMe and slower disk/storage profiles where possible. Use a separate load generator for request benchmarks.

### 6.3 Metrics

Capture:
- API endpoint p50/p95/p99 latency, throughput and error rate.
- Trigger-to-ack and trigger-to-run latency.
- Queue age and wait by lane, repo, reason and runner.
- Job runtime and resource usage (CPU-seconds, peak RSS, pids, scratch, I/O bytes and pressure).
- Workspace create/restore/cleanup duration and physical bytes allocated.
- Cache hit/miss, bytes reused/downloaded, checksum failures and compiler cache effectiveness.
- CAS logical/physical bytes, dedup ratio, object count, GC delay/reclaimed bytes.
- Selected-test count vs full count, selection reason, cached outcomes, and false-negative differential results.
- DB pool wait, transactions, lock/wal checkpoint time and write latency.

Run cold and warm cases, repeated trials and realistic mixed workloads. Save raw benchmark artifacts and exact software/input versions.

### 6.4 Rollout gates

1. Feature disabled: collect baseline only.
2. Shadow mode: calculate lanes, resource decisions and test impact but execute the existing complete suite.
3. Opt-in projects/runners: enable bounded queues and immutable cache reads; keep full test execution.
4. Differential test selection: run selected plus full validation; compare outcomes.
5. Per-project opt-in cache reuse after no missed failures over an agreed evidence window.
6. Default-on only after exact-head CI/GitForge gates, security review, recovery tests, performance evidence and rollback procedure.

## 7. Phased execution

### Gate 0 — correctness and trust (must precede optimization rollout)

- Fix the current russh callback signature compile error and API test fixture compile error on main (the open #336 branch contains fixes for both; still require exact-head validation).
- Complete cargo-vet review of all 15 unattested dependency versions using evidence; do not add blanket exemptions (#345).
- Fix and test GitForge API-token vs trigger-token contracts for both enqueue and status/poll operations; distinguish 401 auth failures from missing run rows/timeouts (#344).
- Close #318's public-fork isolation and required-check semantics before running untrusted code against persistent caches/runners.
- Validate PR #336 atomic publication semantics at exact head; resolve the Supply Chain and GitForge poll failures; do not call it fully green until both GitHub and GitForge evidence succeeds.
- Close or coordinate #337 and #258 independently; avoid overlapping stale branches on Git server files.

### Gate 1 — observability and bounded admission

- Add lane/resource/queue-reason fields and metrics without changing scheduling decisions.
- Measure host and runner pressure and produce queue deferral recommendations in shadow mode.
- Introduce global/per-runner/per-repo admission limits and priority aging with fault tests.
- Protect control-plane capacity and keep trigger persistence/ack independent of job execution.

### Gate 2 — storage/workspace prototype

- Define CAS/reference/namespace schema and object API.
- Implement atomic verified object publication and reference/lease registration.
- Build a workspace provider interface with native Git worktree baseline.
- Prototype OverlayFS/reflink and Btrfs only on appropriate filesystems.
- Add dry-run mark/sweep and recovery tests before destructive GC is enabled.

### Gate 3 — incremental tests

- Rebase and review #322 graph code.
- Add per-test dynamic mapping and exact execution fingerprints.
- Integrate OpenTestSuite adapter and cargo-nextest pilot.
- Start in shadow/differential mode; keep full suite as required check.

### Gate 4 — toolchain bake-off and measured rollout

- Benchmark sccache/nextest/cargo-chef/mold, JS and Python alternatives.
- Run end-to-end mixed-project load tests.
- Enable only the optimizations that produce verified improvements without reducing correctness/security.

## 8. Required failure-injection tests

- Two workers concurrently publish the same CAS key/object.
- Worker crashes between blob publication and reference commit.
- GC overlaps active publication, workspace lease renewal and job completion.
- Dirty worktree belongs to another live agent; GC must preserve it.
- Stale lock is old but still owned; GC must preserve it until fencing succeeds.
- Cache data is corrupted, truncated or metadata is missing.
- Cache namespace is accessed by a different tenant/trust class.
- Runner is out of scratch space or cgroup memory pressure is high.
- Fast lane is flooded while a heavy job waits; both must make progress.
- One repository floods the scheduler; other repositories still dispatch.
- Scheduler/CI restart while jobs are queued, assigned, running and terminal.
- Test graph misses an edge; selection falls back to full scope.
- Changed fixture, environment, compiler or lockfile invalidates cached test result.
- Flaky/failed/new tests are not reused from prior successful receipts.
- Poll API returns 401/403/404/5xx; the Actions adapter reports the actual cause and never reports false success.

## 9. Exit criteria

This initiative is complete only when:

- A mixed-project workload has bounded queues, explicit deferral reasons, fair progress and responsive control-plane p95/p99.
- Resource limits are enforced and verified at the runner boundary.
- Shared content is immutable, tenant-scoped, integrity-checked and recoverable.
- Workspace lifecycle and GC cannot delete live/dirty work.
- Repeated dependency/build/test inputs are demonstrably reused with measured I/O/storage savings.
- Incremental test selection has zero missed failures under differential full-suite validation and still has scheduled full/release gates.
- The system exposes enough telemetry to explain queue delay, storage use, cache behavior and per-job resource cost.
- Rollback remains possible by disabling lanes/cache/test selection and returning to current full validation behavior.

## References

### GitForge
- #342 umbrella epic; #338 resource scheduling; #339 CAS/workspaces/GC; #340 incremental tests; #341 toolchain benchmarks; #344 CI status-poll authentication; #345 cargo-vet attestations.
- #277 resource/storage headroom; #284 duplicate lanes; #318 runner trust; #322 smart CI; #288 push/outbox behavior; #335 Amortyx-Stack registration/evidence; #336 atomic publication; #337 release bundle tests; #258 SSH push trigger.

### Storage/workspace
- pnpm store design: https://github.com/pnpm/pnpm/blob/main/pnpm/docs/settings/store.md
- Btrfs subvolumes/snapshots: https://btrfs.readthedocs.io/en/latest/Subvolumes.html

### Build/test tooling
- lust-lang: https://github.com/t3dotgg/lust-lang
- ts-rust: https://github.com/pingdotgg/ts-rust
- sccache Rust caveats: https://github.com/mozilla/sccache/blob/main/docs/Rust.md
- cargo-chef: https://github.com/LukeMathWalker/cargo-chef
- cargo-nextest: https://nexte.st/docs/running/
- pytest-testmon: https://www.testmon.org/

### Service and database benchmarks
- 1-vCPU/2-GB server challenge: https://github.com/arjaythedev/twelve-dollar-server-challenge
- Multicore server challenge: https://github.com/arjaythedev/multicore-server-challenge
- Million-users challenge: https://github.com/arjaythedev/million-users-challenge
- Database benchmark configs: https://github.com/arjaythedev/database-benchmark-configs
- Software engineering resources: https://github.com/arjaythedev/software-engineering-resources

---
This document defines the intended architecture and execution order. It is not evidence that the runtime changes are implemented or that current CI failures are fixed.

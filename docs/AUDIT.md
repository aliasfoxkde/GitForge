# GitForge System Audit

## Overview
GitForge is a self-hosted Git platform with CI/CD capabilities. This document audits the current state of the system and identifies gaps and areas for improvement.

## System Architecture

```
┌─────────────┐     ┌─────────────┐     ┌─────────────┐
│   Git HTTP  │     │  Git SSH    │     │  Runner     │
│  (42782)    │     │  (42022)    │     │  Agent      │
└─────────────┘     └─────────────┘     └─────────────┘
       │                   │                   │
       └───────────────────┴───────────────────┘
                           │
                    ┌─────────────┐
                    │  CI/Scheduler│
                    │   (42781)   │
                    └─────────────┘
                           │
                    ┌─────────────┐
                    │  API Gateway │
                    │   (42780)   │
                    └─────────────┘
```

## Current Status

### Services Running ✅
| Service | Port | Status | Notes |
|---------|------|--------|-------|
| API Gateway | 42780 | ✅ Running | REST API, health check passing |
| CI/Scheduler | 42781 | ✅ Running | Hosts both CI orchestrator and scheduler HTTP API |
| Git HTTP | 42782 | ✅ Running | HTTP Git protocol implemented |
| Git SSH | 42022 | ⚠️ Placeholder | SSH daemon not yet implemented |

### Test Results
- `cargo clippy --workspace --all-targets -- -D warnings` ✅ Passes
- `cargo test --workspace` ✅ All tests pass
- `cargo fmt --all` ✅ Properly formatted

## Identified Gaps & Areas of Improvement

### 1. Git SSH Support (High Priority)
**Status:** Not implemented
**Issue:** SSH Git protocol requires russh library integration. Initial attempt with russh 0.46 revealed API compatibility issues with the `async_trait` lifetimes.
**Impact:** Users cannot use SSH-based Git operations
**Recommendation:**
- Investigate using a more stable SSH library (ssh2, tokio-ssh2) or
- Upgrade to latest russh version when API stabilizes
- Consider using openssh-sftp-server or similar for initial SSH support

### 2. Repository Lookup (Medium Priority)
**Status:** Placeholder implementation
**Issue:** `RepoId::new()` is used as a placeholder in git-server handlers instead of actual repository lookup
**Impact:** Git operations always fail with "Repository not found"
**Recommendation:**
- Implement repository registry/database lookup
- Wire up RepoService properly in git-server

### 3. Authentication & Authorization (Medium Priority)
**Status:** No auth implemented
**Issue:** All endpoints accept any connection (public key auth accepts all keys, HTTP endpoints have no auth)
**Impact:** Security vulnerability in production
**Recommendation:**
- Implement JWT validation on API endpoints
- Add public key storage and validation for SSH
- Add repository-level access controls

### 4. Runner Agent (Medium Priority)
**Status:** Built but not fully integrated
**Issue:** Runner agent exists but isn't running as part of the standard service stack
**Impact:** CI jobs cannot be executed
**Recommendation:**
- Add runner to docker-compose or service startup
- Implement job execution in Docker sandbox
- Wire up runner to scheduler properly

### 5. Event System (Medium Priority)
**Status:** In-memory only
**Issue:** InMemoryEventBus used; no persistence or cross-instance messaging
**Impact:** Events lost on restart, no horizontal scaling
**Recommendation:**
- Implement persistent event store (Redis, PostgreSQL)
- Add event bus abstraction layer for multiple backends

### 6. CI Pipeline Execution (Medium Priority)
**Status:** Basic pipeline definition exists
**Issue:** No actual job execution engine; CiEngine doesn't execute steps
**Impact:** Pipelines are defined but not run
**Recommendation:**
- Implement step execution in CI engine
- Integrate with runner agent for containerized execution
- Add status tracking and real-time updates

### 7. Database Migrations (Low Priority)
**Status:** Basic migration system exists
**Issue:** No version tracking or rollback capability
**Impact:** Schema changes difficult to manage
**Recommendation:**
- Add migration version tracking table
- Implement rollback support
- Add migration testing

### 8. Docker Build Issues (Low Priority)
**Status:** Dockerfile references non-existent targets
**Issue:** `api-prod`, `ci-prod`, etc. targets not defined in Dockerfile
**Impact:** Docker builds will fail
**Recommendation:**
- Add multi-stage build targets for each service
- Create proper build.sh or Makefile for container builds

### 9. Documentation (Ongoing)
**Status:** Partial documentation
**Issue:** Runbooks, API docs, and architecture docs may be outdated after recent changes
**Impact:** Hard to onboard new contributors
**Recommendation:**
- Update docs/ARCHITECTURE.md with current architecture
- Update docs/RUNBOOK.md with correct ports and startup commands
- Add API documentation for all endpoints

### 10. Monitoring & Observability (Low Priority)
**Status:** Basic tracing only
**Issue:** No metrics, alerting, or structured logging
**Impact:** Hard to debug issues in production
**Recommendation:**
- Add Prometheus metrics endpoint
- Implement structured logging with correlation IDs
- Add health check endpoints for all services
- Create dashboards for key metrics

## Quick Wins

1. **Add repository lookup** - Wire RepoService into git-server handlers
2. **Start runner agent** - Add to service startup for CI functionality
3. **Fix Dockerfile targets** - Add proper multi-stage builds
4. **Update documentation** - Ensure docs reflect current ports and architecture

## Future Enhancements

1. **Webhooks** - GitHub/GitLab compatible webhook receiver
2. **Pull Request UI** - Web interface for code review
3. **Artifact Storage** - Build artifact management
4. **Distributed Runners** - Multiple runner agents for parallel jobs
5. **Secret Management** - Vault integration for secrets
6. **Custom Hooks** - User-defined pipeline hooks
7. **Rate Limiting** - Protect API from abuse
8. **Caching** - Dependency caching for faster builds

## Testing Checklist

- [ ] API health check: `curl http://localhost:42780/health`
- [ ] Scheduler health check: `curl http://localhost:42781/health`
- [ ] Git HTTP health check: `curl http://localhost:42782/health`
- [ ] Runner registration with scheduler
- [ ] Pipeline trigger and execution
- [ ] Docker-based job execution
- [ ] SSH Git clone/push operations (when implemented)
- [ ] Authentication flow
- [ ] Webhook processing

## Verified continuation findings — 2026-08-26

## Verified continuation findings — 2026-08-27

- Added persisted user roles with a `developer` default for legacy accounts;
  login tokens now carry the database role instead of hard-coding `user`.
- Added user-facing API job submission (`POST /api/jobs`) with bounded input,
  per-user durable idempotency, owner/admin/maintainer authorization, and
  replay-safe responses.
- Added owner-aware job/run status and receipt access plus
  `POST /api/jobs/{id}/cancel`. API cancellation writes durable state so the
  separate scheduler and runner processes observe it without shared memory.
- Scheduler refreshes durable pending jobs on its bounded tick and reconciles
  API-written cancellations before assignment. Regression tests prove a
  canceled queued job is never handed to a runner.
- Protected the CI service's `/pipelines/trigger` LAN adapter with a dedicated
  `GITFORGE_TRIGGER_TOKEN`, falling back to operator/shared credentials only
  during migration. The pre-existing service edits remain preserved alongside
  this scoped security change.

## Verified continuation findings — 2026-08-27 auth boundary tranche

- Activated the existing shared JWT middleware and `AuthenticatedUser`
  extractor on the protected API route boundary. Token parsing/validation now
  runs before protected handlers; resource-level ownership checks remain in
  handlers where required.
- Split runner registration from runner administration. `POST /api/runners`
  remains an explicit unauthenticated bootstrap exception, while runner list
  and detail routes remain protected. This exception is now represented in the
  route structure instead of being an accidental consequence of missing
  middleware.
- Migrated webhook triggering, repository routes, artifact routes, runner
  administration, and user-facing job submit/get/log/cancel handlers to the
  shared claims context, removing their production route-local bearer-token
  parsers. Repository listing, lookup, and deletion now enforce owner access
  with admin/maintainer override; cross-user integration coverage proves
  private repositories are hidden. Artifact reads, downloads, deletion, and
  job listing now resolve the artifact's job through its pipeline run and
  repository owner before returning data; unauthorized artifacts are hidden
  with not-found responses. CI pipeline list/read handlers were subsequently
  migrated to the same shared claims context.
- Validation: GitForge API unit tests (190 before this continuation), 42
  integration tests including cross-user artifact scope, strict Clippy, and
  focused authorization tests pass. Whole-workspace validation remains a
  later managed gate after this tranche.

## Verified continuation findings — 2026-08-27 role and lease tranche

- Added administrator-only `PATCH /api/users/{id}/role` with a strict role
  allowlist and last-administrator protection. Protected middleware resolves
  the persisted role for each request, so demoted JWTs lose administrative
  access immediately rather than retaining stale claims until expiry.
- Added scheduler regressions for unknown heartbeats, stale-runner offline
  transitions, assignment requeue and lease cleanup, wrong-runner rejection,
  wrong-lease rejection, durable assignment races, and the HTTP
  pending/start/complete protocol. SQLite persists a lease token and monotonic
  generation; all three transitions use conditional updates so stale runners
  are fenced.
- Updated the OpenAPI specification and API/job-contract documentation for
  role management and the multi-scheduler boundary.
- Validation: 181 API unit tests, 43 API integration tests, 66 database unit
  tests plus 9 database integration tests, and 89 scheduler tests pass;
  workspace Clippy and scoped formatting pass. The managed full-workspace
  GitForge job `f2caae83-efde-40a6-a8da-35c53ef0da33` passed with exit 0 at
  `2026-08-26T03:46:56Z`; managed workspace Clippy also passed.

- Fixed a scheduler cancellation invariant: queue removal is lazy, so stale
  `BinaryHeap` entries are now discarded by both `peek` and `dequeue` before
  they can be assigned. Added queue- and scheduler-level regression tests.
- The scheduler control API is token-authenticated, but user-facing API
  cancellation/ownership remains open. Scheduler routes now separate runner
  and operator credentials (`GITFORGE_RUNNER_TOKEN` and
  `GITFORGE_SCHEDULER_OPERATOR_TOKEN`) with the old shared token as a
  compatibility fallback. Operator submission is durable and idempotent;
  user-facing JWT-to-scheduler ownership remains a follow-up.
- Existing service edits in `services/api/src/main.rs` and
  `services/ci/src/main.rs` were pre-existing and remain intentionally
  preserved; they are not part of this queue fix.

## Verified continuation findings — 2026-08-27 build-manager tranche

- Fixed a control-plane deadlock in `gitforge-build`: submissions no longer
  wait for a semaphore slot before returning, so the Unix control socket stays
  responsive when all build capacity is occupied.
- Fixed the high-risk child-process deadlock where stdout was drained before
  stderr. Both pipes are now drained concurrently, preventing a noisy
  `cargo test` from blocking on a full stderr pipe.
- Implemented the daemon `Cancel` request. Queued jobs are marked cancelled;
  running jobs are terminated through their process group, which also avoids
  orphaned descendants. The CLI now exposes `gitforge-build --cancel JOB_ID`.
  Timeout cleanup now uses async sleep and reaps the child instead of blocking
  a runtime worker thread.
- Validation: `cargo test -p gitforge-build` (53 library, 16 CLI, 4 daemon
  tests) and `cargo clippy -p gitforge-build --all-targets -- -D warnings`
  pass. The CLI now polls durable daemon status for synchronous submissions;
  full workspace managed validation also passed through the daemon:
  `2375c12a-75a7-4522-b3e4-1d5859e9033a` (`cargo test --workspace`, exit 0)
  and `6f22bbf8-12cd-4bf9-a92d-40f741b06e11` (workspace Clippy, exit 0).

## Verified continuation findings — 2026-08-28 log and artifact tranche

- Added a SQLite `job_log_chunks` ledger with per-chunk and per-job bounds.
  Appends require the active runner ID and lease token; stale runners receive
  a conflict and cannot publish late output. The API job-log response now
  includes ordered durable chunks alongside the terminal receipt.
- Added authenticated scheduler runner routes for log append and artifact
  upload. Artifact names are bounded and path-like values are rejected;
  content is stored under server-generated IDs with optional SHA-256
  verification and the shared API artifact root.
- Runner agents now publish UTF-8-safe output chunks before completion and
  upload workspace artifacts through the scheduler protocol. A scheduler HTTP
  regression covers stale-log fencing, accepted logs, artifact checksum,
  storage retrieval, and terminal completion.
- Focused database, scheduler, runner, API, and service compile gates pass.
  True live sandbox streaming and full multi-process API/scheduler/runner E2E
  remain explicitly open. The rebuilt manager also passed full workspace
  tests (`813e789e-807b-4d30-b1c5-0a9dbcda6905`) and Clippy
  (`bc4c1426-8b90-40ed-b7d1-58da3a56a5e2`), both exit 0.

## Verified continuation findings — 2026-08-25 process ownership tranche

- Audited all service entry points and found the process-wide
  `waitpid(-1, WNOHANG)` reaper was enabled even where Tokio owns child
  processes. That can consume an exit status before `Child::wait`, causing
  false failures or hangs.
- API, CI, runner, Git-server, and build-daemon entry points now use
  `init_without_sigchld_reaper`; child owners are responsible for waiting on
  children. The legacy reaper remains explicit-only until a registry-backed
  orphan supervisor replaces it.
- Added `gitforge-build --shutdown`, which is acknowledged over the private
  socket and verified to terminate the daemon and remove the socket.
- Validation: scoped service `cargo check` and strict Clippy passed; the full
  workspace test passed through the manager as job
  `353c5c30-dfbe-42f4-b286-52c23f6af36c` with exit 0.

## Verified continuation findings — 2026-08-25 CI integrity tranche

- Active workflows and reusable templates had mutable action tags; all
  resolvable action references are now pinned to full commit SHAs with release
  comments identifying the reviewed tag. The formerly unresolved
  `aws-actions/git-secrets-scan` template action was replaced with a local
  deterministic credential-pattern scan.
- Release Linux builds now run the locked workspace test suite before building,
  and the release job requests GitHub artifact provenance attestations for
  packaged archives. SBOM generation remains a separate follow-up because the
  repository has not yet selected and pinned a CycloneDX/SPDX generator.
- Corrected the wiki workflow's branch selector and added explicit default
  read permissions where workflows do not need write access. All 14 workflow
  and reusable-template YAML files parse successfully; no unpinned `uses:`
  references remain in those files.
- Release jobs now publish a pinned CycloneDX JSON SBOM alongside archives;
  CI coverage uses LLVM source instrumentation and enforces a 79.9% line gate,
  matching the 79.98% workspace baseline measured on 2026-08-25. The gate is
  intentionally a ratchet: it will rise with verified coverage improvements;
  the 99% objective is not yet met.

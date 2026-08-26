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

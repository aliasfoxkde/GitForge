# GitForge Next Phase Plan

## Executive Summary

Previous phases completed foundational work. This next phase addresses critical gaps: security, runner-scheduler communication, CI event integration, deployment, and test coverage.

---

## Critical Gaps Identified

### 1. Security Issues (CRITICAL)
| Issue | Impact | Priority |
|-------|--------|----------|
| JWT auth not enforced in routes | All API endpoints are unauthenticated | CRITICAL |
| CORS allows any origin | Security vulnerability | HIGH |
| No cargo-audit in CI | Vulnerabilities may go undetected | HIGH |
| Hardcoded JWT secret | Dev secret checked in | MEDIUM |

### 2. Unwired Routes (HIGH)
| Route | Current State | Priority |
|-------|---------------|----------|
| `/artifacts/*` | Returns empty/mock data | HIGH |
| `/jobs/{id}/logs` | Returns placeholder | MEDIUM |

### 3. Runner-Scheduler Communication (CRITICAL)
| Component | Status | Priority |
|-----------|--------|----------|
| Runner heartbeat loop | Empty stub | CRITICAL |
| Runner job fetch loop | Empty stub | CRITICAL |
| Scheduler job endpoint | Not implemented | CRITICAL |

### 4. Event Pipeline Triggering (HIGH)
| Component | Status | Priority |
|-----------|--------|----------|
| CI event consumer | Empty loop | HIGH |
| Push → Pipeline trigger | Not wired | HIGH |

### 5. Deployment Gaps (MEDIUM)
| Item | Status | Priority |
|------|--------|----------|
| Dockerfile | Missing | HIGH |
| docker-compose.yml | Missing | HIGH |
| Service implementations | Stubs | HIGH |

---

## Implementation Plan

### Phase A: Security Hardening (Week 1)

#### A.1 Enforce JWT Authentication
```
1. Create auth middleware tower layer
2. Apply to all routes except /health and /metrics
3. Update server.rs to use middleware
4. Add tests for auth middleware
```

#### A.2 Fix CORS Configuration
```
1. Make CORS origin configurable via env var
2. Default to restrictive in production
3. Add tests for CORS
```

#### A.3 Add Security Audit to CI
```
1. Add rustsec/audit-check to .github/workflows/rust.yml
2. Add cargo-audit to pre-commit hooks
3. Create SECURITY.md policy
```

### Phase B: Runner-Scheduler Communication (Week 1-2)

#### B.1 Implement HTTP Client in Runner
```
File: crates/gitforce-runner/src/agent.rs

1. Add HTTP client using reqwest
2. Implement register() to POST to scheduler
3. Implement heartbeat loop to POST /runners/{id}/heartbeat
4. Implement job fetch loop to GET /jobs/pending
```

#### B.2 Implement Job Endpoints in Scheduler
```
File: crates/gitforce-scheduler/src/

1. Add HTTP server routes for:
   - GET /runners/{id}/heartbeat (update heartbeat)
   - GET /jobs/pending (fetch pending jobs)
   - POST /jobs/{id}/assign (claim job)
   - POST /jobs/{id}/complete (submit result)
```

#### B.3 Wire Up Runner Service
```
File: services/runner/src/main.rs

1. Create runner with scheduler address
2. Start heartbeat loop
3. Start job fetch/execution loop
4. Add proper shutdown handling
```

### Phase C: Event Pipeline Triggering (Week 2)

#### C.1 Implement Event Consumer in CI Service
```
File: services/ci/src/main.rs

1. Subscribe to event bus
2. Handle PushReceived → trigger pipelines
3. Handle PR events → trigger PR pipelines
4. Persist pipeline runs to database
```

#### C.2 Add Pipeline Trigger Endpoint
```
File: crates/gitforce-api/src/routes/

1. POST /pipelines/{id}/trigger
2. Create pipeline run from push event
3. Queue jobs via scheduler
```

### Phase D: Artifact Storage Integration (Week 2)

#### D.1 Wire Artifact Routes to Storage
```
File: crates/gitforce-api/src/routes/artifacts.rs

1. Implement list_artifacts() using storage
2. Implement get_artifact() using storage
3. Implement get_job_artifacts() using storage
4. Add proper error handling
```

### Phase E: Deployment (Week 2-3)

#### E.1 Create Dockerfile
```
File: Dockerfile

Multi-stage build:
- Build stage with Rust
- Production stage with minimal runtime
- Non-root user for security
```

#### E.2 Create Docker Compose
```
File: docker-compose.yml

Services:
- api (REST gateway)
- git-server (Git SSH/HTTP)
- ci (Pipeline orchestrator)
- runner (Job executor)
- sqlite (Database - single instance)
```

#### E.3 Externalize Configuration
```
File: config.toml.example

[server]
host = "0.0.0.0"
port = 8080

[database]
url = "sqlite:/data/gitforge.db"

[runner]
scheduler_url = "http://ci:8081"
```

### Phase F: Test Coverage (Week 3)

#### F.1 Critical Missing Tests
| File | Coverage Target | Tests Needed |
|------|-----------------|--------------|
| gitforce-runner/src/agent.rs | 80% | register, heartbeat, job_fetch |
| gitforce-scheduler/src/assigner.rs | 90% | full assignment flow |
| gitforce-api/src/routes/*.rs | 80% | integration tests |
| services/ci/src/main.rs | 70% | event processing |

#### F.2 Integration Tests
```
tests/integration/
├── api_test.rs      # HTTP integration with TestServer
├── db_test.rs       # Database query tests
└── pipeline_test.rs # End-to-end pipeline execution
```

### Phase G: Documentation (Week 3)

#### G.1 Update Documents
```
1. ARCHITECTURE.md - Add service communication diagrams
2. RUNBOOK.md - Add operational procedures
3. API.md - Document auth requirements
4. DEPLOYMENT.md - Add Docker deployment guide
```

---

## File Changes Summary

### New Files
```
Dockerfile
docker-compose.yml
config.toml.example
tests/integration/api_test.rs
tests/integration/db_test.rs
SECURITY.md
DEPLOYMENT.md
```

### Modified Files
```
.github/workflows/rust.yml        # Add security audit
crates/gitforce-api/src/server.rs # Add auth middleware
crates/gitforce-api/src/routes/   # Apply auth to routes
crates/gitforce-runner/src/agent.rs # Real HTTP client
crates/gitforce-scheduler/src/   # Add HTTP endpoints
crates/gitforce-api/src/routes/artifacts.rs # Wire to storage
services/ci/src/main.rs           # Real event consumer
services/runner/src/main.rs      # Real runner loop
```

---

## Success Metrics

| Phase | Target | Verification |
|-------|--------|--------------|
| Security | All routes authenticated | Manual + tests |
| Runner-Scheduler | Job executes end-to-end | Integration test |
| Event Trigger | Pipeline runs on push | Manual test |
| Artifacts | Upload/download works | API test |
| Deployment | Docker compose up | Manual |
| Test Coverage | 80%+ workspace | cargo tarpaulin |

---

## Timeline

```
Week 1: Security (A) + Runner-Scheduler (B.1-B.2)
Week 2: Runner Service (B.3) + Event Trigger (C) + Artifacts (D)
Week 3: Deployment (E) + Tests (F) + Docs (G)
```

---

## Risk Assessment

| Risk | Likelihood | Impact | Mitigation |
|------|-------------|--------|------------|
| Runner-scheduler API design changes | Medium | High | Design first, implement after |
| Docker networking complexity | High | Medium | Use docker-compose networking |
| Test flakiness | Medium | Medium | Use proper async test utilities |

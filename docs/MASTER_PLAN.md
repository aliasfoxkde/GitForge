# GitForge Master Implementation Plan

**Last Updated**: 2026-07-22
**Goal**: Production-ready, 99% coverage, all CI passing

---

## Phase 1: Security Hardening
**Priority**: CRITICAL - Address security vulnerabilities

- [ ] 1.1 Remove hardcoded JWT secrets - fail if JWT_SECRET env not set
- [ ] 1.2 Fix CORS to use explicit allowed origins
- [ ] 1.3 Replace unwrap() calls in route handlers with proper error handling
- [ ] 1.4 Add RBAC to authentication middleware
- [ ] 1.5 Add cargo audit to CI
- [ ] 1.6 Add Docker container scanning (Trivy/Grype)
- [ ] 1.7 Fix auto-merge.yml to not merge when CI is red

## Phase 2: API Completeness
**Priority**: CRITICAL - Wire CLI to API

- [ ] 2.1 Wire auth login/logout/whoami CLI commands to API
- [ ] 2.2 Wire repo create/info/delete CLI commands to API
- [ ] 2.3 Wire pipeline show/run/create/delete CLI commands to API
- [ ] 2.4 Wire runner info/deregister/capacity CLI commands to API
- [ ] 2.5 Implement stubbed job logs endpoint
- [ ] 2.6 Implement webhook trigger endpoint properly
- [ ] 2.7 Add missing pipeline CRUD endpoints
- [ ] 2.8 Add missing runner DELETE/PATCH endpoints
- [ ] 2.9 Implement sync push/pull API endpoints

## Phase 3: Coverage Improvement
**Priority**: HIGH - Achieve 99% coverage

- [ ] 3.1 Run coverage analysis per crate to identify gaps
- [ ] 3.2 Add tests for lowest-covered crates
- [ ] 3.3 Add integration tests for API handlers
- [ ] 3.4 Add tests for error handling paths
- [ ] 3.5 Add tests for edge cases (empty inputs, boundary values)

## Phase 4: CI/CD Quality
**Priority**: HIGH - Ensure all checks pass

- [ ] 4.1 Add cargo test to release-rust.yml before build
- [ ] 4.2 Add cargo outdated check
- [ ] 4.3 Add Dependabot/Renovate for dependency updates
- [ ] 4.4 Add SBOM generation
- [ ] 4.5 Fix auto-merge.yml CI check condition
- [ ] 4.6 Run full CI locally to verify all passing

## Phase 5: Documentation
**Priority**: MEDIUM - Complete documentation

- [ ] 5.1 Create comprehensive config.md reference
- [ ] 5.2 Add CLI man page/docs/cli.md
- [ ] 5.3 Document webhook event payloads
- [ ] 5.4 Add database schema documentation
- [ ] 5.5 Update README with current features
- [ ] 5.6 Add monitoring/alerting guide

## Phase 6: Code Quality
**Priority**: MEDIUM - Polish and refine

- [ ] 6.1 Consolidate duplicate dependencies where possible
- [ ] 6.2 Update outdated dependencies (protobuf, git2)
- [ ] 6.3 Add input validation to all endpoints
- [ ] 6.4 Add rate limiting to API
- [ ] 6.5 Add request timeout configuration
- [ ] 6.6 Run clippy with strictest settings
- [ ] 6.7 Run fmt check on all code

## Phase 7: Production Readiness
**Priority**: HIGH - Final validation

- [ ] 7.1 Create production deployment guide
- [ ] 7.2 Add health check endpoints
- [ ] 7.3 Add graceful shutdown to all services
- [ ] 7.4 Add Prometheus metrics to all services
- [ ] 7.5 Verify Docker builds work
- [ ] 7.6 Final coverage report (target 99%)
- [ ] 7.7 Create release notes

---

## Current State (2026-07-22)

| Metric | Value | Target |
|--------|-------|--------|
| Coverage | 89.65% | 99% |
| Tests | 872+ | 950+ |
| API Endpoints | 20/23 | 23/23 |
| CLI Wired | ~20% | 100% |

## Key Issues

1. **CLI not wired to API** - Only sync commands work
2. **Hardcoded JWT secret** - Security risk
3. **CORS permissive** - Security risk
4. **unwrap() in handlers** - Could panic
5. **release-rust.yml no tests** - Could release broken builds
6. **auto-merge merges when CI red** - Could merge bad code

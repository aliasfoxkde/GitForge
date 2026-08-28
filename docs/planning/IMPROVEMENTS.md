# GitForge Improvement Plan

Date: 2026-08-28
Status: Active

## Summary

Repository state after audit:
- **Tests**: All passing (90+ tests in storage alone)
- **Linting**: Clippy passes with `-D warnings`
- **Formatting**: `cargo fmt --check` passes
- **Race Detection**: Fixed storage durability issue with `sync_all()` calls
- **Coverage**: 79.98% lines, 81.47% regions, 81.36% functions (target: 99%;
  CI floor: 79.9% LLVM lines)

## Completed This Session

### Fixes Applied
1. **Storage Durability Race Condition**: Added `sync_all()` calls after artifact and cache writes to ensure data is flushed to disk before returning. This fixes intermittent test failures in `test_artifact_metadata_after_put`.

### Documentation Added
1. **SCALING_RESEARCH.md**: Analysis of Cursor's "Git at Scale" architecture with insights for GitForge:
   - Object storage as source of truth
   - Repository as cache pattern
   - Rendezvous hashing for routing
   - WAL-first operations
   - Primary-only compaction

2. **COMPREHENSIVE_EXECUTION_PLAN_2026-08-28.md**: Phase 0-8 tracking with evidence-based progress

## Coverage Analysis (Current State)

### Well-Covered Crates (>85%)
| Crate | Lines | Functions |
|-------|-------|-----------|
| gitforge-common | 100% | 100% |
| gitforge-events | 95.78% | 93.18% |
| gitforge-db/models | 90%+ | 95%+ |
| gitforge-scheduler | 88%+ | 87%+ |
| gitforge-process | 86%+ | 93%+ |

### Moderate Coverage (70-85%)
| Crate | Lines | Issue |
|-------|-------|-------|
| gitforge-api | 79.75% | API routes need more error path tests |
| gitforge-cli | 81.13% | CLI integration tests |
| gitforge-build | 66.93% | Daemon mode hard to unit test |

### Low Coverage (<70%)
| Crate | Lines | Issue |
|-------|-------|-------|
| gitforge-runner/executor | 5.53% | Requires Docker integration |
| services/git-server | 20.48% | Git protocol integration tests |
| gitforge-ai | 7-58% | API mocking needed |
| gitforge-build/daemon | 21.82% | Integration-only code |

## Technical Debt Identified

### High Priority
1. **Storage**: Race condition fixed, needs stress testing
2. **Runner Executor**: 95% untested - needs Docker test harness
3. **Git Server**: 80% untested - needs integration test environment

### Medium Priority
4. **AI Provider mocking**: Anthropic/OpenAI/Ollama need test doubles
5. **CI Service**: 30% coverage - engine tests needed
6. **Build Daemon**: 22% coverage - coordinator tests needed

### Low Priority (Architectural)
7. **WCAG 2.1 AAA**: Frontend templates exist; browser-based coverage is not
   yet complete
8. **Mutation testing**: Not yet implemented

## Immediate Next Steps

### 1. Test Infrastructure
- [ ] Add Docker-based integration tests for runner executor
- [ ] Create AI provider mock implementations for testing
- [ ] Add git-server protocol integration tests

### 2. Code Quality
- [ ] Enable `clippy::pedantic` selectively
- [ ] Add `missing_docs` lint for public APIs
- [ ] Audit `unsafe` code usage

### 3. CI/CD Improvements
- [ ] Add workflow linting (actionlint, shellcheck)
- [ ] Add dependency lockfile policy
- [ ] Execute hosted GitHub Actions verification

### 4. Release Preparation
- [ ] Verify all tests pass in CI
- [ ] Run coverage report and verify >80% (79.98% measured; 79.9% CI floor)
- [ ] Create GitHub release with changelog

## Release Checklist

- [ ] All tests pass (`cargo test --workspace`)
- [ ] Clippy clean (`cargo clippy --workspace -- -D warnings`)
- [ ] Format check (`cargo fmt -- --check`)
- [ ] Coverage ≥80% (currently 79.98%; CI currently enforces 79.9% while
  package and changed-code floors are added)
- [ ] No race conditions (storage sync fix applied)
- [ ] CHANGELOG updated
- [ ] Version bumped in Cargo.toml
- [ ] GitHub release created
- [ ] Docker image built and pushed

## Open Questions

1. Should executor tests require Docker daemon running?
2. Should AI provider tests make real API calls or use mocks?
3. What's the acceptable coverage floor for integration-only code?

## References

- [COMPREHENSIVE_EXECUTION_PLAN_2026-08-28.md](./COMPREHENSIVE_EXECUTION_PLAN_2026-08-28.md) - Full 8-phase roadmap
- [SCALING_RESEARCH.md](./SCALING_RESEARCH.md) - Git at scale research

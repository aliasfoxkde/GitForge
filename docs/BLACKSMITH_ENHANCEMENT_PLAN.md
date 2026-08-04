# GitForge Enhancement Plan: Blacksmith.sh Insights

## Context

Blacksmith.sh is a drop-in GitHub Actions runner replacement that's 60% cheaper and significantly faster. This plan extracts actionable insights and features to enhance GitForge's usability, performance, and enterprise readiness.

---

## 1. Performance & Speed Enhancements

### 1.1 Instant Provisioning (Cold Start Elimination)
**Blacksmith Insight:** Instant microVM provisioning eliminates runner wait times.

**GitForge Implementation:**
- [ ] **Pre-warmed runners** - Maintain idle runner pool based on predicted demand
- [ ] **Runner caching** - Cache compiled runner binaries and dependencies
- [ ] **Docker layer optimization** - Persist and reuse Docker layers on NVMe storage
- [ ] **Connection pooling** - Keep runner agents connected and ready

**Files to modify:**
- `crates/gitforce-runner/src/agent.rs`
- `crates/gitforce-scheduler/src/assigner.rs`
- `services/runner/src/main.rs`

### 1.2 Accelerated Docker Builds
**Blacksmith Insight:** Persisted Docker layers on NVMe drives achieve 40x speedup.

**GitForge Implementation:**
- [ ] **Layer caching** - Store Docker layers in local cache with content-addressable storage
- [ ] **Remote build cache** - Share build cache between runners
- [ ] **NVMe-aware placement** - Schedule jobs on runners with fast storage

**Files to modify:**
- `crates/gitforce-sandbox/src/docker.rs`
- `crates/gitforce-build/src/builder.rs`

### 1.3 Hardware Optimization
**Blacksmith Insight:** Bare-metal gaming CPUs with top single-core performance.

**GitForge Implementation:**
- [ ] **CPU flags detection** - Detect and advertise CPU features (AVX2, AES-NI, etc.)
- [ ] **Job-to-runner matching** - Match CPU-intensive jobs to suitable runners
- [ ] **Resource classification** - Tag runners by hardware class

**Files to modify:**
- `crates/gitforce-runner/src/executor.rs`
- `crates/gitforce-scheduler/src/assigner.rs`

---

## 2. Developer Experience Enhancements

### 2.1 CI Observability Dashboard
**Blacksmith Insight:** CI dashboard to spot failing/slow jobs, search logs, view inline failures.

**GitForge Implementation:**
- [ ] **Web dashboard** - New `services/dashboard` crate with real-time CI view
- [ ] **Job timeline** - Visual pipeline with timing breakdown per step
- [ ] **Log aggregation** - Centralized log storage with full-text search
- [ ] **Failure annotations** - Inline failure markers on pipeline view

**New files:**
- `services/dashboard/Cargo.toml`
- `services/dashboard/src/main.rs`
- `services/dashboard/src/pages/pipelines.rs`
- `services/dashboard/src/pages/logs.rs`

### 2.2 Enhanced CLI
**GitForge Insight:** User noted GitForge CLI should integrate easily with IDEs.

**GitForge Implementation:**
- [ ] **IDE plugins** - VSCode extension, JetBrains plugin scaffolding
- [ ] **Interactive CLI** - TUI for pipeline watching (`gitforge watch`)
- [ ] **JSON output mode** - `gitforge pipeline list --json` for tooling
- [ ] **Shell completion** - Bash, Zsh, Fish completion scripts
- [ ] **Configuration wizard** - `gitforge init --wizard` for first-time setup

**Files to modify:**
- `crates/gitforce-cli/src/main.rs`
- `crates/gitforce-cli/src/tui.rs` (new)
- `docs/IDE_INTEGRATION.md` (new)

### 2.3 Better Error Messages
**Blacksmith Insight:** Clear, actionable error messages with fix suggestions.

**GitForge Implementation:**
- [ ] **Structured errors** - All errors include error codes and documentation links
- [ ] **Fix suggestions** - Error messages suggest actual commands to run
- [ ] **Context hints** - Show relevant config values in error messages

**Files to modify:**
- `crates/gitforce-common/src/errors.rs`
- All error enums across crates

---

## 3. Cost & Resource Management

### 3.1 Cost Tracking & Budgets
**Blacksmith Insight:** Per-minute billing with clear cost tracking.

**GitForge Implementation:**
- [ ] **Resource metering** - Track CPU, memory, storage per job
- [ ] **Cost estimation** - Pre-job cost estimates based on resource usage
- [ ] **Budget alerts** - Notify when org exceeds usage thresholds
- [ ] **Cost breakdown API** - `GET /api/orgs/{id}/costs`

**Files to modify:**
- `crates/gitforce-ci/src/metering.rs` (new)
- `crates/gitforce-api/src/routes/costs.rs` (new)
- `crates/gitforce-db/schema.sql`

### 3.2 Efficient Caching
**Blacksmith Insight:** Cache artifacts co-located with runners for 4x faster downloads.

**GitForge Implementation:**
- [ ] **Content-addressable cache** - Cache key based on file hash, not job ID
- [ ] **Tiered storage** - Fast NVMe cache + slow S3/GCS fallback
- [ ] **Cross-runner cache sharing** - Distributed cache using consistent hashing
- [ ] **Cache invalidation API** - `POST /api/cache/invalidate`

**Files to modify:**
- `crates/gitforce-storage/src/cache.rs`
- `crates/gitforce-runner/src/executor.rs`

---

## 4. Enterprise Readiness

### 4.1 Compliance & Security
**Blacksmith Insight:** SOC2 compliance is enterprise requirement.

**GitForge Implementation:**
- [ ] **Audit logging** - Immutable log of all API calls and actions
- [ ] **Role-based access control (RBAC)** - Finer permissions than admin/user
- [ ] **SSO/SAML support** - Enterprise identity provider integration
- [ ] **Compliance reports** - Generate audit trails for SOC2/HIPAA

**Files to modify:**
- `crates/gitforce-api/src/auth/rbac.rs` (new)
- `crates/gitforce-api/src/middleware/audit.rs` (new)
- `crates/gitforce-db/schema.sql`

### 4.2 High Availability
**Blacksmith Insight:** Managed service handles scaling automatically.

**GitForge Implementation:**
- [ ] **Multi-region replication** - Replicate repos across regions
- [ ] **Leader election** - HA for scheduler and API using Raft/consul
- [ ] **Health checks** - Automatic failover when runners go down
- [ ] **Connection draining** - Graceful shutdown with job migration

**Files to modify:**
- `crates/gitforce-scheduler/src/leader.rs` (new)
- `services/api/src/ha.rs` (new)

---

## 5. Integration Ecosystem

### 5.1 Notifications & Webhooks
**Blacksmith Insight:** Rich integrations with Slack, PagerDuty, etc.

**GitForge Implementation:**
- [ ] **Slack integration** - Pipeline status to Slack channels
- [ ] **PagerDuty alerts** - Escalate failures to on-call
- [ ] **Email digest** - Daily/weekly pipeline summary
- [ ] **Webhook filters** - Custom webhook rules by event type/branch

**Files to modify:**
- `crates/gitforce-events/src/webhook.rs`
- `crates/gitforce-events/src/channels/slack.rs` (new)

### 5.2 External Runner Support
**Blacksmith Insight:** Drop-in replacement for GitHub Actions runners.

**GitForge Implementation:**
- [ ] **GitHub Actions compatible** - Run GitHub Actions workflows on GitForge runners
- [ ] **GitLab CI compatibility** - Support GitLab CI job syntax
- [ ] **Universal runner protocol** - `gitforce-runner` as universal executor

**Files to create:**
- `crates/gitforce-compat/src/github_actions.rs` (new)
- `crates/gitforce-compat/src/gitlab_ci.rs` (new)

---

## 6. AI/ML Integration (Differentiator)

### 6.1 Intelligent Job Optimization
**Blacksmith Insight:** Fast CI enables more testing.

**GitForge Implementation:**
- [ ] **Flaky test detection** - ML-based identification of unreliable tests
- [ ] **Test prioritization** - Run likely-fail tests first
- [ ] **Cache prediction** - Predict which files affect which tests

**Files to create:**
- `crates/gitforce-ml/src/flaky_detection.rs` (new)
- `crates/gitforce-ml/src/test_prioritizer.rs` (new)

### 6.2 Enhanced AI Review
**Blacksmith Insight:** Fast feedback cycles enable better code review culture.

**GitForge Implementation:**
- [ ] **Real-time review** - Stream review comments as AI generates them
- [ ] **Review assignment** - AI suggests best reviewers based on code ownership
- [ ] **Review analytics** - Track review time, finding trends, improvement metrics

**Files to modify:**
- `crates/gitforce-ai/src/streaming.rs` (new)
- `crates/gitforce-review/src/analytics.rs` (new)

---

## Implementation Phases

### Phase 1: Quick Wins (1-2 weeks)
1. Structured error messages with fix suggestions
2. Shell completion scripts
3. JSON output mode for CLI
4. Log aggregation improvements

### Phase 2: Performance (2-4 weeks)
1. Docker layer caching
2. Pre-warmed runner pool
3. Content-addressable build cache
4. CPU-aware job scheduling

### Phase 3: Enterprise (4-8 weeks)
1. RBAC implementation
2. Audit logging
3. High availability setup
4. Compliance reports

### Phase 4: Ecosystem (8+ weeks)
1. Dashboard service
2. GitHub Actions compatibility
3. IDE plugins
4. ML-based optimizations

---

## Metrics to Track

| Enhancement | Metric | Target |
|-------------|--------|--------|
| Docker caching | Build time reduction | 50% |
| Pre-warmed runners | Job start latency | <500ms |
| Cost tracking | Accuracy | 95% |
| RBAC | Permission覆盖 | 100% |
| Dashboard | Adoption rate | 50% of users |
| AI review | Review completion time | <2 min |

---

## Priority Matrix

| | High Impact | Low Impact |
|---|-------------|------------|
| **High Effort** | HA, RBAC, Dashboard | IDE plugins, SSO |
| **Low Effort** | Error messages, CLI enhancements, Caching | Shell completion, JSON mode |

**Recommended Focus:** Start with High Impact + Low Effort items for maximum ROI.

---

## Open Questions

1. Should GitForge support GitHub Actions YAML syntax directly?
2. What's the target enterprise customer size? (Startup vs Enterprise pricing model)
3. Should we build a marketplace for custom runners/filters?
4. Interest in real-time collaboration features for code review?

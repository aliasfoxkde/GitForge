# GitForge Build Queue & CI/CD System - Implementation Plan

## Problem Statement

### Current Issues
1. **Zombie Processes**: 6 defunct rustc processes from crashed builds left unreapped
2. **Parallel Build Storms**: 8 rustc/cargo processes running simultaneously for unrelated projects (drone_processor, gitforce_events, oracle_mcp)
3. **No Central Queue**: Builds triggered independently without resource coordination
4. **External CI Dependency**: GitHub Actions handles builds externally rather than through GitForge

### User Requirements
- Route ALL CI/CD, builds, releases through GitForge (not external systems)
- Implement build queue to manage system resources better/smarter
- Prevent zombie processes and build storms

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────────┐
│                        GitForge                                  │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────────────────┐ │
│  │  Webhook    │  │   Build      │  │   Resource Manager      │ │
│  │  Receiver   │──▶│   Queue      │──▶│   (CPU/Memory Limits)   │ │
│  └─────────────┘  └─────────────┘  └─────────────────────────────┘ │
│                          │                       │                 │
│                          ▼                       ▼                 │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────────────────┐   │
│  │  Scheduler  │  │   Worker    │  │   Process Supervisor    │   │
│  │             │◀─│   Pool      │◀─│   (Zombie Prevention)    │   │
│  └─────────────┘  └─────────────┘  └─────────────────────────┘   │
│                          │                                        │
│                          ▼                                        │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────────────────┐   │
│  │  Runner     │  │   Build     │  │   Artifact              │   │
│  │  Agents     │──▶│   Jobs      │──▶│   Storage               │   │
│  └─────────────┘  └─────────────┘  └─────────────────────────┘   │
└─────────────────────────────────────────────────────────────────┘
```

## Phase 1: Process Supervision & Zombie Prevention

### 1.1 Subreaper Configuration
**Priority: CRITICAL** - Prevents zombie process accumulation

Based on Buildkite/ArgoCD/Tekton patterns: The agent process must become a subreaper to ensure orphaned children are reaped.

```rust
// crates/gitforce-process/src/subreaper.rs

pub fn become_subreaper() -> Result<(), std::io::Error> {
    // Set the process as a subreaper via prctl
    unsafe {
        libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0);
    }
    Ok(())
}
```

### 1.2 SIGCHLD Handler
**Priority: CRITICAL** - Ensures proper child reaping

Using SIG_DFL (default handler) is sufficient - kernel will reap children automatically when they exit. Using waitpid() in a loop is more robust.

```rust
// crates/gitforce-process/src/signal.rs

// Proper SIGCHLD handling with waitpid loop
pub fn setup_sigchld_handler() {
    std::thread::spawn(|| {
        loop {
            // Wait for any child process
            let mut status = 0;
            match waitpid(-1, Some(&mut status), wait::WNOHANG) {
                Ok(WaitStatus::Exited(_, code)) => {
                    tracing::debug!("child exited with code: {}", code);
                }
                Ok(WaitStatus::Signaled(_, sig, _)) => {
                    tracing::debug!("child killed by signal: {:?}", sig);
                }
                Ok(WaitStatus::StillAlive) => {
                    // No child exited, continue
                }
                Err(e) => {
                    tracing::error!("waitpid error: {}", e);
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    });
}
```

### 1.3 Process Pool Manager
**Priority: HIGH** - Controls concurrent build count

Based on Buildkite's "first available agent" model with priority queuing.

```rust
// crates/gitforce-process/src/pool.rs
pub struct ProcessPool {
    pub max_concurrent: usize,
    running: HashMap<u32, Child>,
    pending: Vec<Box<dyn Future<Output = ()>>>,
    semaphore: Arc<Semaphore>,
}

impl ProcessPool {
    pub async fn acquire_slot(&self) -> Permit<'_> {
        self.semaphore.acquire().await.unwrap()
    }
}
```

## Phase 2: Build Queue Architecture

### 2.1 Job Queue Database Schema

```sql
CREATE TABLE build_jobs (
    id TEXT PRIMARY KEY,
    job_type TEXT NOT NULL,  -- compile, test, coverage, build_release
    status TEXT NOT NULL DEFAULT 'pending',  -- pending, running, completed, failed, cancelled
    priority INTEGER NOT NULL DEFAULT 0,  -- higher = more priority
    repo_id TEXT,
    commit_hash TEXT,
    artifact_path TEXT,
    result_json TEXT,
    created_at TEXT NOT NULL,
    started_at TEXT,
    finished_at TEXT,
    error_message TEXT
);

CREATE INDEX idx_build_jobs_status ON build_jobs(status);
CREATE INDEX idx_build_jobs_priority ON build_jobs(priority DESC);
```

### 2.2 Job Types

| Type | Description | Resource Weight |
|------|-------------|-----------------|
| `compile` | Rust compilation | 2 |
| `test` | cargo test runs | 2 |
| `coverage` | llvm-cov runs | 4 |
| `build_release` | Cross-platform builds | 8 |
| `integration_test` | Full integration suites | 4 |

### 2.3 Queue API Endpoints

```
POST /api/queue/jobs          - Submit new job
GET  /api/queue/jobs          - List jobs (with pagination, filtering)
GET  /api/queue/jobs/{id}     - Get job status
POST /api/queue/jobs/{id}/cancel - Cancel a job
DELETE /api/queue/jobs/{id}   - Delete a job
GET  /api/queue/stats         - Queue statistics
POST /api/queue/workers/{id}/heartbeat - Worker heartbeat
```

## Phase 3: Resource Limits

### 3.1 Concurrency Control

| Limit Type | Max Concurrent | Rationale |
|------------|---------------|----------|
| Global | 4 | Prevent system overload |
| Per-repo | 2 | Isolate noisy neighbors |
| Per-user | 1 | Prevent single user垄断 |

### 3.2 Memory Limits

```rust
// Using cgroup v2
pub struct MemoryLimit {
    pub max_bytes: u64,
    pub swap_max_bytes: u64,
}

// Default: 4GB per build job
const DEFAULT_MEMORY_LIMIT: u64 = 4 * 1024 * 1024 * 1024;
```

### 3.3 CPU Limits

```rust
pub struct CpuLimit {
    pub cpu_time_secs: u64,  // Max CPU time
    pub cpus_allowed: u32,   // CPU affinity
}
```

## Phase 4: GitForge-Native CI/CD Integration

### 4.1 Webhook Triggers

| Event | Action |
|-------|--------|
| `push` | Queue compile + test job |
| `pull_request` | Queue test job |
| `tag` | Queue build_release job |
| `schedule` | Queue coverage job (nightly) |

### 4.2 Runner Registration Flow

```
Runner启动
    │
    ▼
POST /api/runners/register {name, capacity, labels}
    │
    ▼
GitForge 返回 runner_id + queue_url
    │
    ▼
轮询 GET /api/queue/jobs/next (带 runner_id)
    │
    ▼
接收 job specification
    │
    ▼
执行 job (编译/测试/构建)
    │
    ▼
POST /api/queue/jobs/{id}/complete {result}
```

### 4.3 Results Reporting

```json
{
  "job_id": "uuid",
  "status": "completed",
  "exit_code": 0,
  "duration_ms": 45000,
  "coverage_pct": 89.5,
  "artifacts": [
    {"name": "test-results.xml", "path": "/artifacts/..."},
    {"name": "coverage.lcov", "path": "/artifacts/..."}
  ]
}
```

## Implementation Order

| # | Phase | Task | Priority | Status |
|---|-------|------|----------|--------|
| 1 | 1 | Subreaper setup + SIGCHLD | Critical | ✅ DONE |
| 2 | 1 | Process cleanup on startup | Critical | ✅ DONE |
| 3 | 2 | SQLite job queue model | High | ✅ DONE |
| 4 | 2 | Queue API endpoints | High | ✅ DONE |
| 5 | 2 | Worker pool implementation | High | ✅ DONE |
| 6 | 3 | Concurrency limits | High | ✅ DONE |
| 7 | 3 | Memory/CPU limits | Medium | ⏳ PENDING |
| 8 | 4 | Runner queue integration | Medium | ✅ DONE |
| 9 | 4 | Webhook → job trigger | Medium | ✅ DONE |
| 10 | 4 | GitForge-native CI workflow | Medium | ✅ DONE |
| 11 | 4 | Disable legacy direct-build workflows | Medium | ✅ DONE |
| 12 | 4 | Results reporting | Medium | ⏳ PENDING |

## Files to Create

```
crates/gitforce-process/           # NEW: Process supervision
  src/lib.rs
  src/subreaper.rs
  src/signal.rs
  src/pool.rs
  src/limit.rs

crates/gitforce-queue/             # NEW: Build queue
  src/lib.rs
  src/job.rs
  src/queue.rs
  src/worker.rs
  src/api.rs

services/queue/                    # NEW: Queue service
  src/main.rs
```

## Files to Modify

```
crates/gitforce-runner/src/agent.rs    - Register with queue, poll jobs
crates/gitforce-events/src/webhook.rs  - Trigger jobs on events
services/ci/src/main.rs               - Use queue for scheduling
services/runner/src/main.rs            - Connect to build queue
```

## Testing Strategy

1. **Unit Tests**: Queue operations, job state transitions
2. **Integration Tests**: Worker pool with mock jobs
3. **Chaos Tests**: Kill workers mid-job, verify cleanup
4. **Load Tests**: 100 concurrent job submissions

## Rollout Plan

### Phase 1 (Day 1-2): Critical Foundation
- Deploy process supervisor
- Clean up existing zombies
- Validate no new zombies

### Phase 2 (Day 3-5): Queue Core
- Deploy queue service
- Migrate CI builds to use queue
- Validate build ordering

### Phase 3 (Day 6-10): Resource Control
- Implement concurrency limits
- Add memory/CPU tracking
- Dashboard for queue status

### Phase 4 (Day 11-15): Full Integration
- All runners use queue
- Webhook → job automation
- Coverage reports in GitForge

Below is a **systems-grade async execution design for GitForce in Rust**, centered on:

* full crate-level trait contracts
* cross-crate async boundaries
* Tokio-based task graph execution model
* deterministic CI orchestration semantics

This is intentionally **implementation-facing**, not conceptual.

---

# 1. CORE ASYNC ARCHITECTURE MODEL (TOKIO TASK GRAPH)

GitForce CI is modeled as:

> A distributed, persistent, resumable DAG execution system

## 1.1 Execution layers

```text id="t1"
Event → Pipeline → DAG → Job → Step → Sandbox Execution → Artifact/State Update
```

Each layer is async + independently recoverable.

---

## 1.2 Tokio execution topology

### Runtime structure

* 1 global async runtime (tokio multi-threaded)
* bounded job queue
* per-job task group
* per-step async execution task
* cancellation propagation tree

---

## 1.3 Core execution graph model

```rust id="coregraph"
JobGraph
 ├── Node (Job)
 │     ├── Step (async task)
 │     ├── Step
 │
 ├── Dependencies (edges)
 └── ExecutionContext (shared state handle)
```

---

# 2. SHARED CORE TYPES (gitforce-common)

## 2.1 Execution primitives

```rust id="common1"
pub type JobId = uuid::Uuid;
pub type PipelineId = uuid::Uuid;
pub type RepoId = uuid::Uuid;
pub type RunnerId = uuid::Uuid;
pub type StepId = uuid::Uuid;
```

---

## 2.2 Core async result model

```rust id="common2"
#[derive(Debug)]
pub enum ExecutionStatus {
    Pending,
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
    TimedOut,
}
```

---

## 2.3 Execution context (shared across tasks)

```rust id="common3"
#[derive(Clone)]
pub struct ExecutionContext {
    pub job_id: JobId,
    pub pipeline_id: PipelineId,
    pub repo_id: RepoId,
    pub commit: String,

    pub workspace_path: String,

    pub env: std::collections::HashMap<String, String>,

    pub cancellation_token: tokio_util::sync::CancellationToken,
}
```

---

# 3. GIT SERVER TRAITS (gitforce-core)

## 3.1 Core Git service abstraction

```rust id="coretrait1"
#[async_trait::async_trait]
pub trait GitService: Send + Sync {
    async fn receive_pack(&self, repo: RepoId, data: Vec<u8>) -> anyhow::Result<()>;
    async fn upload_pack(&self, repo: RepoId) -> anyhow::Result<Vec<u8>>;

    async fn create_repo(&self, name: &str, owner: RepoId) -> anyhow::Result<RepoId>;
    async fn delete_repo(&self, repo: RepoId) -> anyhow::Result<()>;

    async fn list_refs(&self, repo: RepoId) -> anyhow::Result<Vec<GitRef>>;
}
```

---

## 3.2 Hook system (critical CI trigger point)

```rust id="coretrait2"
#[async_trait::async_trait]
pub trait HookExecutor: Send + Sync {
    async fn pre_receive(&self, repo: RepoId, payload: PushEvent) -> anyhow::Result<()>;

    async fn post_receive(&self, repo: RepoId, payload: PushEvent) -> anyhow::Result<()>;
}
```

---

# 4. EVENT SYSTEM TRAITS (gitforce-events)

## 4.1 Event bus abstraction

```rust id="evt1"
#[async_trait::async_trait]
pub trait EventBus: Send + Sync {
    async fn publish(&self, event: EventEnvelope) -> anyhow::Result<()>;

    async fn subscribe(
        &self,
        filter: EventFilter,
    ) -> anyhow::Result<Box<dyn EventStream>>;
}
```

---

## 4.2 Event stream

```rust id="evt2"
#[async_trait::async_trait]
pub trait EventStream: Send + Sync {
    async fn next(&mut self) -> Option<EventEnvelope>;
}
```

---

# 5. CI ORCHESTRATION TRAITS (gitforce-ci)

## 5.1 Pipeline engine

```rust id="ci1"
#[async_trait::async_trait]
pub trait PipelineEngine: Send + Sync {
    async fn trigger_pipeline(
        &self,
        event: PipelineTriggerEvent,
    ) -> anyhow::Result<PipelineRunId>;

    async fn resolve_pipeline(
        &self,
        repo: RepoId,
        commit: &str,
    ) -> anyhow::Result<PipelineDefinition>;
}
```

---

## 5.2 DAG builder

```rust id="ci2"
#[async_trait::async_trait]
pub trait DagBuilder: Send + Sync {
    async fn build_dag(
        &self,
        pipeline: PipelineDefinition,
    ) -> anyhow::Result<JobGraph>;
}
```

---

## 5.3 Job tracker

```rust id="ci3"
#[async_trait::async_trait]
pub trait JobTracker: Send + Sync {
    async fn update_status(&self, job_id: JobId, status: ExecutionStatus) -> anyhow::Result<()>;

    async fn get_status(&self, job_id: JobId) -> anyhow::Result<ExecutionStatus>;
}
```

---

# 6. SCHEDULER TRAITS (gitforce-scheduler)

## 6.1 Job scheduler abstraction

```rust id="sched1"
#[async_trait::async_trait]
pub trait Scheduler: Send + Sync {
    async fn enqueue(&self, job: Job) -> anyhow::Result<()>;

    async fn dequeue(&self) -> anyhow::Result<Option<Job>>;

    async fn assign_runner(&self, job_id: JobId, runner: RunnerId) -> anyhow::Result<()>;
}
```

---

## 6.2 Worker selection policy

```rust id="sched2"
#[async_trait::async_trait]
pub trait SchedulingPolicy: Send + Sync {
    async fn select_runner(
        &self,
        job: &Job,
        runners: &[Runner],
    ) -> anyhow::Result<Option<RunnerId>>;
}
```

---

# 7. RUNNER TRAITS (gitforce-runner)

## 7.1 Runner agent interface

```rust id="runner1"
#[async_trait::async_trait]
pub trait RunnerAgent: Send + Sync {
    async fn register(&self, runner: RunnerInfo) -> anyhow::Result<RunnerId>;

    async fn heartbeat(&self, runner_id: RunnerId) -> anyhow::Result<()>;

    async fn fetch_job(&self, runner_id: RunnerId) -> anyhow::Result<Option<Job>>;
}
```

---

## 7.2 Execution runtime

```rust id="runner2"
#[async_trait::async_trait]
pub trait ExecutionRuntime: Send + Sync {
    async fn execute_job(
        &self,
        job: Job,
        ctx: ExecutionContext,
    ) -> anyhow::Result<JobResult>;
}
```

---

# 8. SANDBOX TRAITS (gitforce-sandbox)

## 8.1 Sandbox abstraction

```rust id="sandbox1"
#[async_trait::async_trait]
pub trait Sandbox: Send + Sync {
    async fn create(&self, ctx: &ExecutionContext) -> anyhow::Result<SandboxInstance>;

    async fn execute_step(
        &self,
        instance: &SandboxInstance,
        step: Step,
    ) -> anyhow::Result<StepResult>;

    async fn destroy(&self, instance: SandboxInstance) -> anyhow::Result<()>;
}
```

---

## 8.2 Resource control

```rust id="sandbox2"
pub struct SandboxLimits {
    pub cpu_ms: u64,
    pub memory_mb: u64,
    pub disk_mb: u64,
    pub timeout_ms: u64,
}
```

---

# 9. STORAGE TRAITS (gitforce-storage)

## 9.1 Artifact storage

```rust id="store1"
#[async_trait::async_trait]
pub trait ArtifactStore: Send + Sync {
    async fn put(&self, artifact: Artifact) -> anyhow::Result<()>;

    async fn get(&self, id: &ArtifactId) -> anyhow::Result<Artifact>;

    async fn delete(&self, id: &ArtifactId) -> anyhow::Result<()>;
}
```

---

## 9.2 Cache system

```rust id="store2"
#[async_trait::async_trait]
pub trait CacheStore: Send + Sync {
    async fn put_cache(&self, key: String, data: Vec<u8>) -> anyhow::Result<()>;

    async fn get_cache(&self, key: &str) -> anyhow::Result<Option<Vec<u8>>>;
}
```

---

# 10. MIRROR SYSTEM TRAITS (gitforce-mirror)

```rust id="mirror1"
#[async_trait::async_trait]
pub trait MirrorService: Send + Sync {
    async fn sync_repo(&self, repo: RepoId) -> anyhow::Result<()>;

    async fn sync_ref(&self, repo: RepoId, ref_name: &str) -> anyhow::Result<()>;
}
```

---

# 11. ASYNC EXECUTION MODEL (TOKIO TASK GRAPH)

This is the **core runtime engine behavior**.

---

## 11.1 Execution pipeline

```text id="exec1"
Event Bus
   ↓
Pipeline Trigger Task (Tokio)
   ↓
DAG Builder Task
   ↓
Scheduler Task Queue
   ↓
Runner Assignment Task
   ↓
Sandbox Execution Task
   ↓
Result Aggregation Task
   ↓
Artifact Upload Task
   ↓
Event Emission Task
```

---

## 11.2 Tokio task hierarchy model

### Root runtime

```rust id="exec2"
tokio::runtime::Builder::new_multi_thread()
    .worker_threads(N)
    .enable_all()
    .build()
```

---

### Task groups

Each pipeline run spawns a **task group scope**:

```rust id="exec3"
PipelineRunTaskGroup
 ├── JobTaskGroup
 │     ├── StepTask
 │     ├── StepTask
 │
 ├── ArtifactTask
 ├── MirrorTask
```

---

## 11.3 Structured concurrency pattern

Each job:

```rust id="exec4"
async fn run_job(job: Job, ctx: ExecutionContext) -> Result<()> {
    let cancellation = ctx.cancellation_token.clone();

    let steps = build_steps(job);

    for step in steps {
        tokio::select! {
            _ = cancellation.cancelled() => {
                return Err(anyhow!("cancelled"));
            }

            result = execute_step(step, &ctx) => {
                result?;
            }
        }
    }

    Ok(())
}
```

---

## 11.4 DAG execution model

Parallel jobs:

```text id="dag1"
Job A → Job C
Job B → Job C
```

Execution rule:

* Jobs A and B spawn concurrently
* Job C waits on both completion signals

Implementation:

```rust id="dag2"
tokio::spawn(job_a);
tokio::spawn(job_b);

join!(job_a, job_b);

tokio::spawn(job_c);
```

For scale:

* use semaphore-limited concurrency pool
* scheduler enforces dependency resolution

---

## 11.5 Backpressure model

```text id="bp1"
Runner capacity exhausted
        ↓
Scheduler delays assignment
        ↓
Queue grows
        ↓
Priority rebalancing
        ↓
Retry or timeout policies
```

---

## 11.6 Failure propagation model

* Step failure → Job failure (configurable retry)
* Job failure → Pipeline failure (or continue-on-failure flag)
* Pipeline failure → Event emitted + optional rollback hooks

---

# 12. SYSTEM-WIDE EXECUTION CONTRACT

All components MUST obey:

* async-only APIs
* cancellation propagation via `CancellationToken`
* idempotent execution
* retry-safe operations
* stateless workers (except execution context)
* persistent state in Postgres
* event-driven coordination only

---

# 13. KEY DESIGN OUTCOME

This architecture yields:

* GitHub-class CI/CD orchestration
* Fully self-hosted execution layer
* Replaceable Git backend
* Horizontal scaling of compute
* Deterministic pipeline execution
* AI-layer augmentation possible without coupling

---

If you want the next step, the logical continuation is:

* **MVP build order with exact Rust files + module scaffolding**
* or
* **Firecracker-based sandbox implementation design (production-grade isolation layer)**
* or
* **full distributed scaling model (multi-node scheduler + runner federation)**
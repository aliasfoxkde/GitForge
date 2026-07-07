Below is the **production-grade continuation design pack** for GitForce, split into:

1. MVP build order + exact Rust workspace scaffolding (file-level)
2. Firecracker sandbox design (secure execution layer)
3. Full distributed scaling model (multi-node scheduler + runner federation)

This is structured to be directly convertible into an implementation backlog.

---

# 1. MVP BUILD ORDER + RUST WORKSPACE SCAFFOLDING

## 1.1 MVP philosophy (important constraint)

MVP is NOT full GitHub parity.

MVP target:

* Git push works
* Repo stored locally
* Event emitted
* CI pipeline triggered
* 1 runner executes jobs in Docker
* artifacts stored
* status returned via API

Everything else is deferred.

---

# 1.2 FINAL MVP CRATE LAYOUT (SIMPLIFIED FIRST BUILD)

```text id="mvp1"
gitforce/
│
├── Cargo.toml
│
├── crates/
│   ├── gitforce-common/
│   ├── gitforce-core/
│   ├── gitforce-api/
│   ├── gitforce-events/
│   ├── gitforce-ci/
│   ├── gitforce-runner/
│   ├── gitforce-sandbox/
│   ├── gitforce-storage/
│   └── gitforce-db/
│
├── services/
│   ├── git-server/
│   ├── api/
│   ├── ci/
│   └── runner/
│
└── migrations/
```

---

# 1.3 FILE-BY-FILE IMPLEMENTATION ORDER

This is **strict build sequence for AI execution**

---

## PHASE 1 — COMMON FOUNDATION

### 1. `gitforce-common`

```
crates/gitforce-common/src/
```

Files:

```text id="f1"
mod.rs
ids.rs
error.rs
time.rs
result.rs
```

### Order:

1. ids.rs → UUID types
2. error.rs → unified error enum
3. result.rs → Result<T> alias
4. mod.rs wiring

---

## PHASE 2 — DATABASE LAYER

### 2. `gitforce-db`

```
crates/gitforce-db/src/
```

Files:

```text id="f2"
mod.rs
models/
    mod.rs
    repo.rs
    pipeline.rs
    job.rs
    runner.rs
    event.rs
connection.rs
queries.rs
```

### Order:

1. connection.rs (Postgres pool)
2. models/repo.rs
3. models/pipeline.rs
4. models/job.rs
5. queries.rs (CRUD only)

---

## PHASE 3 — EVENT SYSTEM

### 3. `gitforce-events`

```
crates/gitforce-events/src/
```

Files:

```text id="f3"
mod.rs
event.rs
types.rs
bus.rs
serializer.rs
```

### Order:

1. event.rs (EventEnvelope)
2. types.rs (enum events)
3. serializer.rs (serde JSON)
4. bus.rs (NATS or in-memory pubsub)

---

## PHASE 4 — GIT SERVER CORE

### 4. `gitforce-core`

```
crates/gitforce-core/src/
```

Files:

```text id="f4"
mod.rs
repo.rs
storage.rs
refs.rs
hooks.rs
auth.rs
git_protocol/
    mod.rs
    ssh.rs
    http.rs
```

### Order:

1. storage.rs (bare repo filesystem)
2. repo.rs (create/list/delete)
3. refs.rs
4. hooks.rs (post-receive trigger point)
5. git_protocol/ssh.rs (minimal push support)

---

## PHASE 5 — CI ORCHESTRATOR

### 5. `gitforce-ci`

```
crates/gitforce-ci/src/
```

Files:

```text id="f5"
mod.rs
pipeline.rs
dag.rs
engine.rs
executor.rs
state.rs
```

### Order:

1. pipeline.rs (definition loader)
2. dag.rs (dependency graph builder)
3. state.rs (job state machine)
4. engine.rs (trigger logic)
5. executor.rs (calls runner system)

---

## PHASE 6 — SCHEDULER

### 6. `gitforce-scheduler`

```
crates/gitforce-scheduler/src/
```

Files:

```text id="f6"
mod.rs
queue.rs
assigner.rs
policy.rs
```

### Order:

1. queue.rs (FIFO + priority)
2. policy.rs (runner selection rules)
3. assigner.rs (job → runner mapping)

---

## PHASE 7 — RUNNER

### 7. `gitforce-runner`

```
crates/gitforce-runner/src/
```

Files:

```text id="f7"
mod.rs
agent.rs
executor.rs
heartbeat.rs
protocol.rs
```

### Order:

1. agent.rs (runner registration)
2. heartbeat.rs
3. executor.rs (job execution entrypoint)
4. protocol.rs (scheduler communication)

---

## PHASE 8 — SANDBOX (MVP DOCKER ONLY)

### 8. `gitforce-sandbox`

```
crates/gitforce-sandbox/src/
```

Files:

```text id="f8"
mod.rs
docker.rs
limits.rs
context.rs
```

### Order:

1. docker.rs (container spawn/exec)
2. limits.rs (cpu/mem/time)
3. context.rs (workspace mounting)

---

## PHASE 9 — STORAGE

### 9. `gitforce-storage`

```
crates/gitforce-storage/src/
```

Files:

```text id="f9"
mod.rs
artifact.rs
cache.rs
filesystem.rs
```

### Order:

1. filesystem.rs
2. artifact.rs
3. cache.rs

---

## PHASE 10 — API

### 10. `gitforce-api`

```
crates/gitforce-api/src/
```

Files:

```text id="f10"
mod.rs
routes/
    mod.rs
    repo.rs
    ci.rs
    runner.rs
    artifacts.rs
auth.rs
server.rs
```

### Order:

1. server.rs (axum bootstrap)
2. routes/repo.rs
3. routes/ci.rs
4. auth.rs

---

## PHASE 11 — SERVICES WIRING

```
services/
```

### git-server/main.rs

* mounts gitforce-core
* starts SSH + HTTP

### ci/main.rs

* starts event listener
* starts scheduler loop

### runner/main.rs

* heartbeat loop
* job execution loop

### api/main.rs

* HTTP API server

---

# 1.4 MVP EXECUTION FLOW

```text id="mvpflow"
git push
   ↓
git-server (core)
   ↓
post-receive hook
   ↓
event emitted
   ↓
CI engine triggered
   ↓
job queued
   ↓
scheduler assigns runner
   ↓
runner executes docker sandbox
   ↓
artifact stored
   ↓
API updates status
```

---

# 2. FIRECRACKER SANDBOX DESIGN (PRODUCTION ISOLATION LAYER)

This replaces Docker in production.

---

## 2.1 architecture

```text id="fc1"
Runner
  ↓
Firecracker Manager
  ↓
MicroVM Pool
  ↓
Ephemeral VM per job
  ↓
Mounted workspace (virtio-fs)
  ↓
Execution container inside VM
```

---

## 2.2 core components

### crates/gitforce-sandbox/firecracker.rs

Responsibilities:

* VM lifecycle
* kernel boot
* rootfs assignment
* networking setup
* snapshot reuse

---

## 2.3 microVM lifecycle

### creation

1. allocate VM
2. assign kernel image
3. assign rootfs snapshot
4. configure CPU/memory limits
5. attach virtio devices
6. boot VM

---

### execution model

Inside VM:

* runner agent binary starts
* receives job over gRPC
* executes steps in isolated process namespace

---

### teardown

* snapshot filesystem
* destroy VM
* release resources

---

## 2.4 resource isolation

* CPU: cgroups v2
* memory: hard limit enforced by Firecracker
* disk: ephemeral rootfs overlay
* network: optional egress rules

---

## 2.5 security model

* no host filesystem access
* no shared kernel exploits surface (microVM isolation)
* read-only base image
* ephemeral writable layer only

---

## 2.6 performance optimization

* VM pooling (warm instances)
* snapshot restore instead of cold boot
* pre-warmed runner images

---

## 2.7 execution protocol

```text id="fc2"
Scheduler → Runner → Firecracker VM → Job execution → Result stream → Scheduler
```

---

# 3. FULL DISTRIBUTED SCALING MODEL

This is where GitForce becomes “GitHub-scale”.

---

# 3.1 system topology

```text id="scale1"
                ┌──────────────┐
                │   API Node   │
                └──────┬───────┘
                       │
               ┌───────▼────────┐
               │  Event Bus     │
               │ (NATS/Kafka)   │
               └───────┬────────┘
                       │
        ┌──────────────┼──────────────┐
        │              │              │
┌───────▼───────┐ ┌───▼────────┐ ┌───▼────────┐
│ CI Scheduler   │ │ CI Engine  │ │ MirrorSvc  │
└───────┬───────┘ └────┬───────┘ └────────────┘
        │              │
        ▼              ▼
   Runner Pool A   Runner Pool B
   (region 1)      (region 2)
```

---

# 3.2 scheduler federation

Each scheduler node:

* owns subset of queues
* subscribes to event partition
* uses consistent hashing for job routing

---

## 3.3 queue model

* priority queue per repo
* global fairness scheduler
* backpressure-aware admission control

---

## 3.4 runner federation

### runner types

* docker runners
* firecracker runners
* GPU runners
* ARM runners

---

### registration model

Runner heartbeat:

```text id="scale2"
Runner → Scheduler
  {
    cpu_capacity,
    memory_capacity,
    tags,
    region
  }
```

---

## 3.5 assignment algorithm

1. filter runners by tags
2. filter by capacity
3. score by:

   * latency
   * load
   * cache hit rate
   * region affinity
4. assign best match

---

## 3.6 job routing

```text id="scale3"
Event → Partition Key (repo_id)
      → Scheduler shard
      → Queue
      → Runner
```

---

## 3.7 state synchronization

All state stored in Postgres, but:

* scheduler caches active jobs in memory
* runners are stateless
* event log is source of truth for recovery

---

## 3.8 failure recovery

### scheduler crash

* rebuild from event log

### runner crash

* job re-queued

### VM crash

* retry with fresh sandbox

---

## 3.9 scaling limits

Horizontal scaling supported for:

* API nodes
* CI schedulers
* runners
* event consumers

Single points (initial MVP):

* Postgres
* event broker

---

# FINAL RESULT

You now have:

## MVP system

* file-by-file Rust implementation plan
* deterministic build order
* minimal CI system

## Production sandbox model

* Firecracker isolation layer
* microVM execution architecture

## Distributed system design

* multi-node CI scheduling
* runner federation
* partitioned queues
* event-driven scaling

---

If you want next-level execution planning, the logical next step is:

* **exact Tokio task orchestration code (real async scheduler loop implementation)**
* or
* **Firecracker Rust integration skeleton (actual API bindings + VM lifecycle code)**
* or
* **minimal 7-day build plan that gets a working Git push → CI → runner system online**

Just pick direction.

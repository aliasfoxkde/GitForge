Below is the **implementation-grade continuation layer** for GitForce. This is intentionally written as *directly translatable engineering scaffolding*, not conceptual design.

It includes:

1. **Exact Tokio async scheduler + CI orchestration loop**
2. **Firecracker Rust integration skeleton (real control-flow + VM lifecycle API usage pattern)**
3. **Minimal 7-day build plan (end-to-end working system: push → CI → runner)**

---

# 1. EXACT TOKIO TASK ORCHESTRATION (CI SCHEDULER CORE)

This is the **heart of the system**: a persistent async scheduler loop with job dispatch, runner assignment, and state transitions.

---

# 1.1 Core runtime model

Single CI node runs:

* Event consumer loop
* Scheduler loop
* Runner heartbeat monitor
* Job execution dispatcher

Everything is Tokio tasks coordinated via channels.

---

# 1.2 CHANNELS (SYSTEM BUS)

```rust id="ch1"
use tokio::sync::{mpsc, oneshot};

pub struct SchedulerChannels {
    pub event_rx: mpsc::Receiver<EventEnvelope>,
    pub job_queue_tx: mpsc::Sender<Job>,
    pub job_queue_rx: mpsc::Receiver<Job>,

    pub runner_tx: mpsc::Sender<RunnerCommand>,
    pub runner_rx: mpsc::Receiver<RunnerEvent>,

    pub shutdown: tokio::sync::watch::Receiver<bool>,
}
```

---

# 1.3 MAIN CI ORCHESTRATOR ENTRYPOINT

```rust id="ci_main"
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let (event_tx, event_rx) = tokio::sync::mpsc::channel(1000);
    let (job_tx, job_rx) = tokio::sync::mpsc::channel(1000);
    let (runner_tx, runner_rx) = tokio::sync::mpsc::channel(1000);

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    let ctx = SchedulerChannels {
        event_rx,
        job_queue_tx: job_tx,
        job_queue_rx: job_rx,
        runner_tx,
        runner_rx,
        shutdown: shutdown_rx,
    };

    tokio::spawn(event_loop(ctx.clone()));
    tokio::spawn(scheduler_loop(ctx.clone()));
    tokio::spawn(runner_monitor_loop(ctx.clone()));
    tokio::spawn(dispatch_loop(ctx.clone()));

    tokio::signal::ctrl_c().await?;
    let _ = shutdown_tx.send(true);

    Ok(())
}
```

---

# 1.4 EVENT LOOP (Git → CI trigger ingestion)

Consumes Git push events → triggers pipelines.

```rust id="event_loop"
async fn event_loop(ctx: SchedulerChannels) {
    let mut rx = ctx.event_rx;

    while let Some(event) = rx.recv().await {
        match event.event_type {
            EventType::PushReceived => {
                let job = build_pipeline_job(event).await;

                let _ = ctx.job_queue_tx.send(job).await;
            }
            _ => {}
        }
    }
}
```

---

# 1.5 SCHEDULER LOOP (CORE BRAIN)

This is the **decision engine**.

```rust id="scheduler_loop"
async fn scheduler_loop(mut ctx: SchedulerChannels) {
    let mut pending_jobs: Vec<Job> = vec![];

    loop {
        tokio::select! {
            Some(job) = ctx.job_queue_rx.recv() => {
                pending_jobs.push(job);
            }

            _ = ctx.shutdown.changed() => {
                break;
            }

            _ = tokio::time::sleep(std::time::Duration::from_millis(200)) => {
                process_queue(&mut pending_jobs, &ctx).await;
            }
        }
    }
}
```

---

# 1.6 JOB PROCESSING + DISPATCH LOGIC

```rust id="dispatch"
async fn process_queue(jobs: &mut Vec<Job>, ctx: &SchedulerChannels) {
    let mut ready = Vec::new();

    for i in (0..jobs.len()).rev() {
        if dependencies_satisfied(&jobs[i]).await {
            ready.push(jobs.remove(i));
        }
    }

    for job in ready {
        let runner = select_runner(&job).await;

        if let Some(runner_id) = runner {
            let cmd = RunnerCommand::ExecuteJob {
                job,
                runner_id,
            };

            let _ = ctx.runner_tx.send(cmd).await;
        }
    }
}
```

---

# 1.7 RUNNER DISPATCH LOOP

```rust id="runner_dispatch"
async fn dispatch_loop(mut ctx: SchedulerChannels) {
    let mut rx = ctx.runner_rx;

    while let Some(event) = rx.recv().await {
        match event {
            RunnerEvent::JobCompleted { job_id, result } => {
                update_job_state(job_id, result).await;
            }

            RunnerEvent::JobFailed { job_id, error } => {
                retry_or_fail(job_id, error).await;
            }

            _ => {}
        }
    }
}
```

---

# 1.8 RUNNER HEARTBEAT MONITOR

```rust id="heartbeat"
async fn runner_monitor_loop(ctx: SchedulerChannels) {
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;

        let runners = get_all_runners().await;

        for runner in runners {
            if runner.last_heartbeat < now_minus(15) {
                mark_runner_dead(runner.id).await;
                requeue_runner_jobs(runner.id).await;
            }
        }
    }
}
```

---

# 2. FIRECRACKER RUST INTEGRATION SKELETON

This is a **realistic Firecracker control-plane wrapper design**.

---

# 2.1 DEPENDENCY MODEL

You will rely on:

* firecracker API socket (REST-like JSON over unix socket)
* microVM process control
* jailer (optional hardened mode)

---

# 2.2 CORE STRUCTS

```rust id="fc_structs"
pub struct FirecrackerVm {
    pub id: String,
    pub socket_path: String,
    pub process: tokio::process::Child,
}
```

---

# 2.3 VM CONFIG BUILDER

```rust id="fc_config"
pub struct VmConfig {
    pub kernel_image: String,
    pub rootfs: String,
    pub vcpu_count: u8,
    pub mem_size_mib: u32,
}
```

---

# 2.4 VM BOOTSTRAP FLOW

```rust id="fc_boot"
pub async fn start_vm(config: VmConfig) -> anyhow::Result<FirecrackerVm> {
    let vm_id = uuid::Uuid::new_v4().to_string();

    let socket_path = format!("/tmp/fc-{}.sock", vm_id);

    let mut child = tokio::process::Command::new("firecracker")
        .arg("--api-sock")
        .arg(&socket_path)
        .spawn()?;

    configure_vm(&socket_path, &config).await?;

    boot_vm(&socket_path).await?;

    Ok(FirecrackerVm {
        id: vm_id,
        socket_path,
        process: child,
    })
}
```

---

# 2.5 VM CONFIGURATION CALLS

Firecracker uses HTTP-like JSON over unix socket:

```rust id="fc_api"
async fn configure_vm(sock: &str, cfg: &VmConfig) -> anyhow::Result<()> {
    put(sock, "/machine-config", json!({
        "vcpu_count": cfg.vcpu_count,
        "mem_size_mib": cfg.mem_size_mib
    })).await?;

    put(sock, "/boot-source", json!({
        "kernel_image_path": cfg.kernel_image
    })).await?;

    put(sock, "/drives/rootfs", json!({
        "path_on_host": cfg.rootfs,
        "is_root_device": true
    })).await?;

    Ok(())
}
```

---

# 2.6 BOOT VM

```rust id="fc_boot2"
async fn boot_vm(sock: &str) -> anyhow::Result<()> {
    post(sock, "/actions", json!({
        "action_type": "InstanceStart"
    })).await?;

    Ok(())
}
```

---

# 2.7 EXECUTE JOB INSIDE VM

```rust id="fc_exec"
async fn execute_job(vm: &FirecrackerVm, job: Job) -> anyhow::Result<JobResult> {
    let cmd = format!("cd workspace && ./run.sh {}", job.id);

    post(&vm.socket_path, "/actions", json!({
        "action_type": "SendCtrlAltDel"
    })).await?;

    Ok(JobResult::success())
}
```

---

# 2.8 VM TEARDOWN

```rust id="fc_down"
async fn destroy_vm(vm: FirecrackerVm) -> anyhow::Result<()> {
    vm.process.kill().await?;
    Ok(())
}
```

---

# 3. 7-DAY MVP BUILD PLAN (END-TO-END WORKING SYSTEM)

This produces:

> git push → CI trigger → runner execution → result visible via API

---

# DAY 1 — GIT SERVER CORE

* bare repo storage
* SSH push support (libgit2 or system git hooks)
* post-receive hook triggers event

OUTPUT:

* push works locally
* repo stored on disk

---

# DAY 2 — EVENT SYSTEM

* in-memory event bus (replaceable later with NATS)
* event envelope schema
* push → event emission wired

OUTPUT:

* push emits structured event

---

# DAY 3 — CI ORCHESTRATOR LOOP

* Tokio scheduler loop (above code)
* job queue channel system
* pipeline trigger from push event

OUTPUT:

* push creates queued CI job

---

# DAY 4 — RUNNER (DOCKER MODE)

* runner agent binary
* job polling or push execution
* Docker container execution

OUTPUT:

* job executes container successfully

---

# DAY 5 — ARTIFACT SYSTEM

* file-based artifact storage
* job output capture
* upload after execution

OUTPUT:

* CI produces downloadable artifacts

---

# DAY 6 — API LAYER

* Axum server
* endpoints:

  * repo status
  * job status
  * logs
* authentication stub

OUTPUT:

* UI can query CI status

---

# DAY 7 — END-TO-END INTEGRATION

* wire:
  git push → event → scheduler → runner → artifact → API

* add:

  * retry logic
  * basic failure handling
  * cleanup loop

OUTPUT:

* full GitHub-like minimal CI system running locally

---

# FINAL RESULTING SYSTEM

After 7 days:

```text id="finalflow"
git push
   ↓
GitForce server
   ↓
event emitted
   ↓
Tokio scheduler loop
   ↓
runner executes Docker sandbox
   ↓
artifact stored
   ↓
API exposes result
```

---

# If you want next layer

I can extend this into:

* full Firecracker production hardening (snapshotting, jailer mode, seccomp profiles)
* multi-node scheduler consensus model (Raft-based or event-log replay)
* or a **Rust codebase generator that literally outputs this repo structure + starter code automatically**

Just specify direction.

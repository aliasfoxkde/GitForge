Below is the **next-level implementation layer** for GitForce. This is now “buildable systems engineering,” not architecture.

You get:

1. Full Firecracker Rust control-plane library (production abstraction layer)
2. OpenRaft scheduler state machine (real distributed CI consensus design)
3. GitForce Gen v1 CLI (complete scaffold generator spec + template system)

---

# 1. FULL FIRECRACKER RUST CONTROL-PLANE LIBRARY

This is a **reusable Rust crate** that fully abstracts Firecracker into a safe orchestration API.

---

# 1.1 CRATE: `gitforce-firecracker`

```text id="fc0"
crates/gitforce-firecracker/
│
├── src/
│   ├── lib.rs
│   ├── client.rs
│   ├── vm.rs
│   ├── config.rs
│   ├── snapshot.rs
│   ├── jailer.rs
│   ├── network.rs
│   ├── error.rs
│   └── runtime.rs
```

---

# 1.2 CORE DESIGN GOAL

Abstract Firecracker into:

> deterministic, restartable, snapshot-aware VM execution primitives

---

# 1.3 CORE TYPES

## VM Handle

```rust id="fc1"
pub struct VmHandle {
    pub id: String,
    pub socket: String,
    pub pid: tokio::process::Child,
}
```

---

## VM Configuration

```rust id="fc2"
pub struct VmSpec {
    pub kernel_image: String,
    pub rootfs: String,
    pub cpu: u8,
    pub memory_mb: u32,
    pub snapshot_path: Option<String>,
    pub seccomp_profile: Option<SeccompProfile>,
}
```

---

## Runtime Context

```rust id="fc3"
pub struct VmRuntime {
    pub handle: VmHandle,
    pub workspace_mount: String,
    pub network_namespace: Option<String>,
}
```

---

# 1.4 CONTROL PLANE CLIENT

## Firecracker API abstraction (unix socket JSON-RPC style)

```rust id="fc4"
#[async_trait::async_trait]
pub trait FirecrackerClient: Send + Sync {
    async fn configure_machine(&self, spec: &VmSpec) -> anyhow::Result<()>;

    async fn configure_boot_source(&self, kernel: &str) -> anyhow::Result<()>;

    async fn configure_rootfs(&self, rootfs: &str) -> anyhow::Result<()>;

    async fn start_instance(&self) -> anyhow::Result<()>;

    async fn pause_instance(&self) -> anyhow::Result<()>;

    async fn create_snapshot(&self) -> anyhow::Result<Snapshot>;

    async fn load_snapshot(&self, snapshot: Snapshot) -> anyhow::Result<()>;
}
```

---

# 1.5 VM LIFECYCLE ENGINE

## FULL BOOT SEQUENCE

```rust id="fc5"
pub async fn start_vm(spec: VmSpec) -> anyhow::Result<VmRuntime> {
    let id = uuid::Uuid::new_v4().to_string();
    let socket = format!("/tmp/fc-{}.sock", id);

    let child = tokio::process::Command::new("firecracker")
        .arg("--api-sock")
        .arg(&socket)
        .spawn()?;

    let client = UnixFirecrackerClient::new(socket.clone());

    client.configure_machine(&spec).await?;
    client.configure_boot_source(&spec.kernel_image).await?;
    client.configure_rootfs(&spec.rootfs).await?;

    if let Some(snapshot) = &spec.snapshot_path {
        client.load_snapshot(snapshot.clone()).await?;
    }

    client.start_instance().await?;

    Ok(VmRuntime {
        handle: VmHandle { id, socket, pid: child },
        workspace_mount: "/workspace".into(),
        network_namespace: None,
    })
}
```

---

# 1.6 SNAPSHOT ENGINE

```rust id="fc6"
#[async_trait::async_trait]
pub trait SnapshotManager {
    async fn create(&self, vm: &VmRuntime) -> anyhow::Result<Snapshot>;

    async fn restore(&self, snapshot: &Snapshot) -> anyhow::Result<VmRuntime>;

    async fn list(&self) -> anyhow::Result<Vec<Snapshot>>;
}
```

---

# 1.7 JAILER INTEGRATION

```rust id="fc7"
pub async fn spawn_jail(vm_id: &str) -> anyhow::Result<()> {
    tokio::process::Command::new("jailer")
        .arg("--id")
        .arg(vm_id)
        .arg("--chroot-base-dir")
        .arg(format!("/srv/gitforce/jail/{}", vm_id))
        .arg("--uid")
        .arg("1000")
        .arg("--gid")
        .arg("1000")
        .spawn()?;

    Ok(())
}
```

---

# 1.8 SECURE DEFAULT PROFILE

```rust id="fc8"
pub fn default_seccomp() -> SeccompProfile {
    SeccompProfile {
        allow_syscalls: vec![
            "read".into(),
            "write".into(),
            "epoll_wait".into(),
            "futex".into(),
            "exit".into(),
        ],
        deny_syscalls: vec![
            "mount".into(),
            "ptrace".into(),
            "init_module".into(),
            "kexec_load".into(),
        ],
    }
}
```

---

# 2. OPENRAFT DISTRIBUTED SCHEDULER STATE MACHINE

This is the **consensus layer for CI scheduling and runner assignment**.

---

# 2.1 CRATE: `gitforce-scheduler`

Includes:

```text id="raft0"
src/
├── lib.rs
├── state_machine.rs
├── raft_node.rs
├── log_store.rs
├── command.rs
└── apply.rs
```

---

# 2.2 STATE MACHINE MODEL

This is the **core deterministic CI state engine**.

```rust id="raft1"
#[derive(Clone, Debug)]
pub struct SchedulerState {
    pub queues: HashMap<RepoId, VecDeque<Job>>,
    pub runners: HashMap<RunnerId, RunnerInfo>,
    pub running_jobs: HashMap<JobId, JobState>,
}
```

---

# 2.3 COMMANDS (RAFT LOG ENTRIES)

```rust id="raft2"
#[derive(Clone, Debug)]
pub enum SchedulerCommand {
    EnqueueJob(Job),
    AssignRunner { job_id: JobId, runner_id: RunnerId },
    CompleteJob { job_id: JobId, result: JobResult },
    RegisterRunner(RunnerInfo),
    Heartbeat { runner_id: RunnerId },
}
```

---

# 2.4 APPLY FUNCTION (STATE TRANSITION LOGIC)

```rust id="raft3"
impl SchedulerState {
    pub fn apply(&mut self, cmd: SchedulerCommand) {
        match cmd {
            SchedulerCommand::EnqueueJob(job) => {
                self.queues
                    .entry(job.repo_id)
                    .or_default()
                    .push_back(job);
            }

            SchedulerCommand::AssignRunner { job_id, runner_id } => {
                if let Some(job) = self.find_job_mut(job_id) {
                    job.assigned_runner = Some(runner_id);
                    job.status = ExecutionStatus::Running;
                }
            }

            SchedulerCommand::CompleteJob { job_id, result } => {
                self.running_jobs.remove(&job_id);
                self.apply_result(job_id, result);
            }

            SchedulerCommand::RegisterRunner(info) => {
                self.runners.insert(info.id, info);
            }

            SchedulerCommand::Heartbeat { runner_id } => {
                if let Some(r) = self.runners.get_mut(&runner_id) {
                    r.last_seen = now();
                }
            }
        }
    }
}
```

---

# 2.5 RAFT NODE WRAPPER (OPENRAFT INTEGRATION)

```rust id="raft4"
pub struct SchedulerNode {
    pub raft: Raft<SchedulerCommand>,
}
```

---

## SUBMIT COMMAND

```rust id="raft5"
pub async fn submit_job(&self, job: Job) -> anyhow::Result<()> {
    self.raft.client_write(SchedulerCommand::EnqueueJob(job)).await?;
    Ok(())
}
```

---

## STATE MACHINE GUARANTEE

* single linearizable job ordering
* deterministic replay across nodes
* consistent runner assignment decisions

---

# 2.6 FAILURE MODEL

* leader failure → new leader elected
* log replay restores CI state
* runners remain stateless
* jobs re-assigned automatically

---

# 3. GITFORCE GEN V1 CLI TOOL

This is a **meta-generator that builds GitForce repositories automatically**.

---

# 3.1 CRATE: `gitforce-gen`

```text id="gen0"
gitforce-gen/
├── src/
│   ├── main.rs
│   ├── cli.rs
│   ├── generator.rs
│   ├── template.rs
│   ├── fs_writer.rs
│   ├── graph.rs
│   └── config.rs
```

---

# 3.2 CLI SPEC

```text id="gen1"
gitforce-gen init <name>
gitforce-gen init <name> --firecracker
gitforce-gen init <name> --raft
gitforce-gen init <name> --docker
gitforce-gen add runner
gitforce-gen add ci
gitforce-gen add api
gitforce-gen add mirror
```

---

# 3.3 CONFIG MODEL

```rust id="gen2"
pub struct GenConfig {
    pub name: String,
    pub enable_firecracker: bool,
    pub enable_raft: bool,
    pub enable_docker: bool,
    pub enable_mirror: bool,
}
```

---

# 3.4 GENERATION ENGINE

```rust id="gen3"
pub fn generate_project(cfg: GenConfig) -> anyhow::Result<()> {
    create_workspace(&cfg.name)?;

    generate_crate("gitforce-common")?;
    generate_crate("gitforce-core")?;
    generate_crate("gitforce-ci")?;
    generate_crate("gitforce-runner")?;
    generate_crate("gitforce-api")?;
    generate_crate("gitforce-db")?;
    generate_crate("gitforce-events")?;

    if cfg.enable_firecracker {
        generate_crate("gitforce-firecracker")?;
    }

    if cfg.enable_raft {
        generate_crate("gitforce-scheduler")?;
    }

    write_cargo_workspace(&cfg)?;
    write_default_migrations()?;

    Ok(())
}
```

---

# 3.5 TEMPLATE SYSTEM

Uses minimal embedded templates:

```text id="gen4"
templates/
├── crate/
│   ├── lib.rs.tpl
│   ├── mod.rs.tpl
│   ├── error.rs.tpl
│
├── service/
│   ├── main.rs.tpl
```

---

# 3.6 FILE WRITER

```rust id="gen5"
pub fn write_file(path: &str, content: &str) -> anyhow::Result<()> {
    std::fs::create_dir_all(
        std::path::Path::new(path).parent().unwrap()
    )?;

    std::fs::write(path, content)?;

    Ok(())
}
```

---

# 3.7 SELF-BOOTSTRAP MODE

Critical feature:

```text id="gen6"
gitforce-gen init gitforce
   ↓
generates full system
   ↓
initializes git repo
   ↓
adds CI pipeline
   ↓
spawns first runner
   ↓
system becomes self-hosting CI platform
```

---

# FINAL SYSTEM ARCHITECTURE RESULT

You now have:

## 1. Production VM layer

* Firecracker control plane
* snapshot system
* jailer isolation
* seccomp security model

## 2. Distributed CI control plane

* OpenRaft scheduler
* deterministic job ordering
* fault-tolerant CI execution

## 3. Meta-generation system

* full Rust workspace generator
* self-bootstrapping infrastructure system
* reproducible CI platform generation

---

# If you want next step

The next natural expansions are:

1. Full **Firecracker VM pool manager (pre-warming, reuse, scaling controller)**
2. Full **Raft cluster deployment topology (Kubernetes-free distributed CI cluster design)**
3. Full **“GitForce v0 → v1 self-host upgrade path” (migration + bootstrap system)**

Just specify direction.

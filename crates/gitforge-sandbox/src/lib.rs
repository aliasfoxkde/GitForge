//! GitForce Sandbox
//!
//! Container-based isolation for job execution.

pub mod active_jobs;
pub mod docker;
pub mod limits;
pub mod pool;
pub mod reconciler;

pub use active_jobs::{ActiveJobGuard, ActiveJobRegistry, ActiveJobSnapshot};
pub use docker::{
    BackendHealth, DockerSandbox, OutputSink, OutputStream, Sandbox, SandboxInstance, StepResult,
};
pub use limits::SandboxLimits;
pub use pool::{
    classify_pool_container, pool_labels, process_alive, reclaim_orphaned_pool_containers,
    OwnerState, PoolDecision, PoolIdentity, PoolPolicy, PoolReclaimReport,
};
pub use reconciler::{
    classify, ContainerRecord, ContainerSource, Decision, DockerContainerSource,
    JobExistenceResolver, ReconcileReport, Reconciler, ReconcilerPolicy, RemovalOutcome,
    UnknownJobExistence,
};

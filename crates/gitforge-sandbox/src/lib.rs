//! GitForce Sandbox
//!
//! Container-based isolation for job execution.

pub mod active_jobs;
pub mod docker;
pub mod limits;
pub mod reconciler;

pub use active_jobs::{ActiveJobGuard, ActiveJobRegistry, ActiveJobSnapshot};
pub use docker::{
    BackendHealth, DockerSandbox, OutputSink, OutputStream, Sandbox, SandboxInstance, StepResult,
};
pub use limits::SandboxLimits;
pub use reconciler::{
    classify, ContainerRecord, ContainerSource, Decision, DockerContainerSource, ReconcileReport,
    Reconciler, ReconcilerPolicy, RemovalOutcome,
};

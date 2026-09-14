//! GitForce Sandbox
//!
//! Container-based isolation for job execution.

pub mod docker;
pub mod limits;
pub mod reconciler;

pub use docker::{DockerSandbox, OutputSink, OutputStream, Sandbox, SandboxInstance, StepResult};
pub use limits::SandboxLimits;
pub use reconciler::{
    classify, ContainerRecord, ContainerSource, Decision, DockerContainerSource, ReconcileReport,
    Reconciler, ReconcilerPolicy, RemovalOutcome,
};

//! GitForce Runner
//!
//! Job execution agent that runs in Docker containers.

pub mod agent;
pub mod executor;
pub mod fault;
pub mod mount_lock;

pub use agent::{RunnerAgent, RunnerConfig};
pub use executor::JobExecutor;

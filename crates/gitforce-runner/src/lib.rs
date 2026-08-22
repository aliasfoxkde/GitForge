//! GitForce Runner
//!
//! Job execution agent that runs in Docker containers.

pub mod agent;
pub mod executor;
pub mod outbox;

pub use agent::{RunnerAgent, RunnerConfig};
pub use executor::JobExecutor;
pub use outbox::{CompletionOutbox, OutboxEntry, OUTBOX_CAPACITY};

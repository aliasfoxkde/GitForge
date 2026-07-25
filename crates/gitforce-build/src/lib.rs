//! GitForge Build Coordinator
//!
//! Local build coordinator with semaphore-based concurrency limiting.
//! Routes cargo builds through GitForge while preventing system overload.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────┐     ┌──────────────────┐     ┌─────────────────┐
//! │ Claude Code /   │────▶│ gitforge-buildd  │────▶│  cargo process   │
//! │ Other Agents   │     │  (daemon)        │     │  (semaphore: 2) │
//! └─────────────────┘     │  - semaphore    │     └─────────────────┘
//!                         │  - job queue     │
//!                         │  - zombie reap   │     ┌─────────────────┐
//!                         └────────┬─────────┘────▶│ GitForge API    │
//!                                  │                │ (optional)      │
//!                                  │                └─────────────────┘
//!                                  ▼
//!                         ┌──────────────────┐
//!                         │ Unix Socket API  │
//!                         │ /tmp/gitforge-   │
//!                         │ build.sock       │
//!                         └──────────────────┘
//! ```

pub mod coordinator;
pub mod job;
pub mod protocol;

pub use coordinator::BuildCoordinator;
pub use job::{BuildJob, BuildResult, JobOutput, JobStatus, MAX_CONCURRENT_JOBS};
pub use protocol::*;

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

pub mod client;
pub mod coordinator;
pub mod job;
pub mod protocol;

pub use client::{JobSubmitter, MockClient, UnixSocketClient, DEFAULT_SOCKET};
pub use coordinator::{BuildCoordinator, BuildCoordinatorConfig};
pub use job::{BuildJob, BuildResult, JobOutput, JobStatus, MAX_CONCURRENT_JOBS};
pub use protocol::*;

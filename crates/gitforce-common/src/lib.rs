//! GitForce Common Library
//!
//! Shared primitives for all GitForce components including:
//! - UUID types for all entities
//! - Unified error handling
//! - Result type aliases
//! - Time utilities

pub mod error;
pub mod ids;
pub mod password;
pub mod result;
pub mod time;

pub use error::{Error, ErrorKind, Result};
pub use ids::{
    JobId, JobStatus, PipelineId, PipelineRunId, PipelineStatus, RepoId, RunnerId, StepId, UserId,
};
pub use time::DateTime;

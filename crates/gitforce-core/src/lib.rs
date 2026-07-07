//! GitForce Core Git Server
//!
//! Git protocol handling, repository management, and hooks.

pub mod git_protocol;
pub mod hooks;
pub mod repo;
pub mod storage;

pub use hooks::{HookExecutor, HookManager, HookPayload};
pub use repo::{GitRef, RepoService};
pub use storage::{FileStorageBackend, StorageBackend};

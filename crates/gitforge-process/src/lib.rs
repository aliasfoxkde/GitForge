//! GitForge Process Supervision
//!
//! This crate provides process supervision utilities for GitForge,
//! including subreaper setup, SIGCHLD handling, process pools,
//! and cgroup-based resource limits to prevent zombie processes
//! and manage concurrent builds.

pub mod limits;
pub mod pool;
pub mod signal;
pub mod subreaper;

pub use limits::{
    apply_limits, get_cgroup_path, is_in_cgroup_v2, CpuLimit, MemoryLimit, ResourceLimits,
};
pub use pool::{JobWeight, PoolConfig, ProcessPool};
pub use signal::{
    create_shutdown_flag, install_sigchld_handler, spawn_shutdown_handler, wait_for_shutdown,
};
pub use subreaper::become_subreaper;

/// Initialize process supervision
///
/// This should be called once at startup to configure the process
/// as a subreaper and install SIGCHLD handling.
pub fn init() -> std::io::Result<()> {
    become_subreaper()?;
    install_sigchld_handler();
    tracing::info!("process supervision initialized");
    Ok(())
}

/// Initialize subreaper support without installing the process-wide SIGCHLD
/// reaper.
///
/// Use this in processes that directly manage children through Tokio or
/// another runtime. A global `waitpid(-1, WNOHANG)` loop can otherwise reap a
/// child's status before its owner calls `Child::wait`, producing missing or
/// misleading exit results. Such processes must explicitly wait for every
/// child they start. Call [`init`] only in a process whose children are not
/// otherwise waited on and which intentionally needs orphan reaping.
pub fn init_without_sigchld_reaper() -> std::io::Result<()> {
    become_subreaper()?;
    tracing::info!("process supervision initialized without global SIGCHLD reaper");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_init() {
        init().expect("failed to initialize process supervision");
    }
}

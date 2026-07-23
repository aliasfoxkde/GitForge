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

pub use limits::{apply_limits, get_cgroup_path, is_in_cgroup_v2, CpuLimit, MemoryLimit, ResourceLimits};
pub use pool::{JobWeight, PoolConfig, ProcessPool};
pub use signal::install_sigchld_handler;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_init() {
        init().expect("failed to initialize process supervision");
    }
}

//! Sandbox resource limits

use serde::{Deserialize, Serialize};

/// Resource limits for sandbox execution
///
/// `cpus` and `pids_limit` are enforced by the container runtime's cgroup
/// controller at container creation (issue #277): `cpus` becomes a
/// `cpu_quota`/`cpu_period` pair and `pids_limit` caps concurrent processes
/// and threads inside the container. A `pids_limit` of `0` means unlimited
/// and is the operator escape hatch, not the normal path.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxLimits {
    /// CPU cores this sandbox may use (fractional values allowed)
    pub cpus: f64,
    /// Memory limit in megabytes
    pub memory_mb: u64,
    /// Maximum concurrent processes/threads in the container (0 = unlimited)
    pub pids_limit: i64,
    /// Disk limit in megabytes
    pub disk_mb: u64,
    /// Execution timeout in seconds
    pub timeout_secs: u64,
    /// Whether to allow network access
    pub network: bool,
}

impl Default for SandboxLimits {
    fn default() -> Self {
        Self {
            cpus: 2.0,
            memory_mb: 4096, // 4GB
            pids_limit: 512,
            disk_mb: 10240,     // 10GB
            timeout_secs: 3600, // 1 hour
            network: true,
        }
    }
}

impl SandboxLimits {
    /// Create limits for a specific tier
    pub fn small() -> Self {
        Self {
            cpus: 1.0,
            memory_mb: 512,
            pids_limit: 128,
            disk_mb: 1024,
            timeout_secs: 300,
            network: false,
        }
    }

    pub fn medium() -> Self {
        Self {
            cpus: 2.0,
            memory_mb: 2048,
            pids_limit: 512,
            disk_mb: 5120,
            timeout_secs: 1800,
            network: true,
        }
    }

    pub fn large() -> Self {
        Self {
            cpus: 4.0,
            memory_mb: 8192,
            pids_limit: 512,
            disk_mb: 20480,
            timeout_secs: 3600,
            network: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sandbox_limits_default() {
        let limits = SandboxLimits::default();
        assert_eq!(limits.cpus, 2.0);
        assert_eq!(limits.memory_mb, 4096);
        assert_eq!(limits.pids_limit, 512);
        assert!(limits.network);
    }

    #[test]
    fn test_sandbox_limits_small() {
        let limits = SandboxLimits::small();
        assert_eq!(limits.cpus, 1.0);
        assert_eq!(limits.memory_mb, 512);
        assert_eq!(limits.pids_limit, 128);
        assert!(!limits.network);
    }

    #[test]
    fn test_sandbox_limits_medium() {
        let limits = SandboxLimits::medium();
        assert_eq!(limits.cpus, 2.0);
        assert_eq!(limits.memory_mb, 2048);
        assert_eq!(limits.pids_limit, 512);
        assert!(limits.network);
    }

    #[test]
    fn test_sandbox_limits_large() {
        let limits = SandboxLimits::large();
        assert_eq!(limits.cpus, 4.0);
        assert_eq!(limits.memory_mb, 8192);
        assert_eq!(limits.pids_limit, 512);
        assert!(limits.network);
    }

    #[test]
    fn test_sandbox_limits_debug() {
        let limits = SandboxLimits::default();
        let debug_str = format!("{limits:?}");
        assert!(debug_str.contains("cpus"));
        assert!(debug_str.contains("memory_mb"));
        assert!(debug_str.contains("pids_limit"));
    }

    #[test]
    fn test_sandbox_limits_clone() {
        let limits = SandboxLimits::large();
        let cloned = limits.clone();
        assert_eq!(cloned.cpus, limits.cpus);
        assert_eq!(cloned.memory_mb, limits.memory_mb);
        assert_eq!(cloned.pids_limit, limits.pids_limit);
    }

    #[test]
    fn test_sandbox_limits_all_tiers() {
        let small = SandboxLimits::small();
        let medium = SandboxLimits::medium();
        let large = SandboxLimits::large();
        let default = SandboxLimits::default();

        // Verify tier ordering
        assert!(small.cpus < medium.cpus);
        assert!(medium.cpus < large.cpus);
        assert!(small.memory_mb < medium.memory_mb);
        assert!(medium.memory_mb < large.memory_mb);
        assert!(default.memory_mb <= large.memory_mb);
        assert!(small.pids_limit < medium.pids_limit);

        // Verify network settings
        assert!(!small.network); // Small has no network
        assert!(medium.network);
        assert!(large.network);
        assert!(default.network);
    }
}

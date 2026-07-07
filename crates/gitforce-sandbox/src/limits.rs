//! Sandbox resource limits

use serde::{Deserialize, Serialize};

/// Resource limits for sandbox execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxLimits {
    /// CPU time limit in milliseconds
    pub cpu_ms: u64,
    /// Memory limit in megabytes
    pub memory_mb: u64,
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
            cpu_ms: 3600000, // 1 hour
            memory_mb: 4096, // 4GB
            disk_mb: 10240,  // 10GB
            timeout_secs: 3600, // 1 hour
            network: true,
        }
    }
}

impl SandboxLimits {
    /// Create limits for a specific tier
    pub fn small() -> Self {
        Self {
            cpu_ms: 300000,  // 5 minutes
            memory_mb: 512,
            disk_mb: 1024,
            timeout_secs: 300,
            network: false,
        }
    }

    pub fn medium() -> Self {
        Self {
            cpu_ms: 1800000, // 30 minutes
            memory_mb: 2048,
            disk_mb: 5120,
            timeout_secs: 1800,
            network: true,
        }
    }

    pub fn large() -> Self {
        Self {
            cpu_ms: 3600000,
            memory_mb: 8192,
            disk_mb: 20480,
            timeout_secs: 3600,
            network: true,
        }
    }
}

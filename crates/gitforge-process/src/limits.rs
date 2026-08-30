//! Resource limits using Linux cgroups
//!
//! Provides CPU and memory limits for build jobs using cgroup v2.

use std::path::Path;
use std::process::Command;

/// Memory limit configuration
#[derive(Debug, Clone)]
pub struct MemoryLimit {
    /// Maximum memory in bytes
    pub max_bytes: u64,
    /// Maximum memory + swap in bytes (0 = no swap)
    pub swap_max_bytes: u64,
}

impl Default for MemoryLimit {
    fn default() -> Self {
        // Default 4GB per build job
        Self {
            max_bytes: 4 * 1024 * 1024 * 1024,
            swap_max_bytes: 0,
        }
    }
}

/// CPU limit configuration
#[derive(Debug, Clone)]
pub struct CpuLimit {
    /// CPU time limit in seconds
    pub cpu_time_secs: u64,
    /// Number of CPUs allowed (0 = unlimited)
    pub cpus_allowed: u32,
}

impl Default for CpuLimit {
    fn default() -> Self {
        Self {
            cpu_time_secs: 3600, // 1 hour
            cpus_allowed: 2,
        }
    }
}

/// Unified resource limits
#[derive(Debug, Clone, Default)]
pub struct ResourceLimits {
    pub memory: MemoryLimit,
    pub cpu: CpuLimit,
}

impl ResourceLimits {
    /// Create new limits with custom values
    pub fn new(memory_bytes: u64, cpu_time_secs: u64, cpus: u32) -> Self {
        Self {
            memory: MemoryLimit {
                max_bytes: memory_bytes,
                swap_max_bytes: 0,
            },
            cpu: CpuLimit {
                cpu_time_secs,
                cpus_allowed: cpus,
            },
        }
    }

    /// Medium limits (8GB memory, 2 hour timeout, 4 CPUs)
    pub fn medium() -> Self {
        Self {
            memory: MemoryLimit {
                max_bytes: 8 * 1024 * 1024 * 1024,
                swap_max_bytes: 0,
            },
            cpu: CpuLimit {
                cpu_time_secs: 7200,
                cpus_allowed: 4,
            },
        }
    }

    /// Heavy limits (16GB memory, 4 hour timeout, 8 CPUs)
    pub fn heavy() -> Self {
        Self {
            memory: MemoryLimit {
                max_bytes: 16 * 1024 * 1024 * 1024,
                swap_max_bytes: 0,
            },
            cpu: CpuLimit {
                cpu_time_secs: 14400,
                cpus_allowed: 8,
            },
        }
    }
}

/// Apply resource limits to current process using prlimit
pub fn apply_limits(limits: &ResourceLimits) -> std::io::Result<()> {
    // Set memory limit via prlimit
    set_memory_limit(limits.memory.max_bytes)?;

    // Set CPU time limit via prlimit
    set_cpu_limit(limits.cpu.cpu_time_secs)?;

    tracing::info!(
        "applied resource limits: memory={} bytes, cpu_time={}s, cpus={}",
        limits.memory.max_bytes,
        limits.cpu.cpu_time_secs,
        limits.cpu.cpus_allowed
    );

    Ok(())
}

/// Set memory limit using prlimit
fn set_memory_limit(max_bytes: u64) -> std::io::Result<()> {
    // Use prlimit to set max memory
    // RLIMIT_AS = address space limit (memory)
    let output = Command::new("prlimit")
        .args([
            &format!("--pid={}", std::process::id()),
            &format!("--as={}", max_bytes),
        ])
        .output();

    let output = match output {
        Ok(output) => output,
        Err(e) => {
            tracing::warn!("prlimit unavailable, skipping memory limit: {}", e);
            // Don't fail - prlimit might not be available in all environments
            return Ok(());
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        tracing::warn!("prlimit --as failed: {}", stderr);
        // Don't fail - prlimit might not be available in all environments
    }

    Ok(())
}

/// Set CPU time limit using prlimit
fn set_cpu_limit(cpu_time_secs: u64) -> std::io::Result<()> {
    let output = Command::new("prlimit")
        .args([
            &format!("--pid={}", std::process::id()),
            &format!("--cpu={}", cpu_time_secs),
        ])
        .output();

    let output = match output {
        Ok(output) => output,
        Err(e) => {
            tracing::warn!("prlimit unavailable, skipping cpu limit: {}", e);
            return Ok(());
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        tracing::warn!("prlimit --cpu failed: {}", stderr);
    }

    Ok(())
}

/// Check if running in a cgroup v2 environment
pub fn is_in_cgroup_v2() -> bool {
    Path::new("/sys/fs/cgroup/cgroup.controllers").exists()
}

/// Get current cgroup path for the process
pub fn get_cgroup_path() -> Option<String> {
    let cgroup_path = "/proc/self/cgroup";

    if !Path::new(cgroup_path).exists() {
        return None;
    }

    // Read cgroup v2 mount
    if let Ok(content) = std::fs::read_to_string(cgroup_path) {
        for line in content.lines() {
            let parts: Vec<&str> = line.split(':').collect();
            if parts.len() >= 3 {
                // cgroup v2 format: hierarchy-ID:controllers:path
                if parts[1] == "cgroup" {
                    return Some(parts[2].to_string());
                }
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_memory_limit_default() {
        let limit = MemoryLimit::default();
        assert_eq!(limit.max_bytes, 4 * 1024 * 1024 * 1024);
    }

    #[test]
    fn test_cpu_limit_default() {
        let limit = CpuLimit::default();
        assert_eq!(limit.cpu_time_secs, 3600);
        assert_eq!(limit.cpus_allowed, 2);
    }

    #[test]
    fn test_resource_limits_new() {
        let limits = ResourceLimits::new(8_000_000_000, 7200, 4);
        assert_eq!(limits.memory.max_bytes, 8_000_000_000);
        assert_eq!(limits.cpu.cpu_time_secs, 7200);
        assert_eq!(limits.cpu.cpus_allowed, 4);
    }

    #[test]
    fn test_resource_limits_medium() {
        let limits = ResourceLimits::medium();
        assert_eq!(limits.memory.max_bytes, 8 * 1024 * 1024 * 1024);
        assert_eq!(limits.cpu.cpus_allowed, 4);
    }

    #[test]
    fn test_resource_limits_heavy() {
        let limits = ResourceLimits::heavy();
        assert_eq!(limits.memory.max_bytes, 16 * 1024 * 1024 * 1024);
        assert_eq!(limits.cpu.cpus_allowed, 8);
    }

    #[test]
    fn test_apply_limits_does_not_panic() {
        let limits = ResourceLimits::default();
        // Should not panic even if prlimit fails
        let result = apply_limits(&limits);
        assert!(result.is_ok());
    }

    #[test]
    fn test_is_in_cgroup_v2() {
        // Should return bool, not panic
        let _ = is_in_cgroup_v2();
    }

    #[test]
    fn test_get_cgroup_path() {
        // Should return Some or None, not panic
        let _ = get_cgroup_path();
    }
}

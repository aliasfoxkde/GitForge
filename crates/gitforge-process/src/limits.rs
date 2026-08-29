//! Resource limits for build jobs.

use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone)]
pub struct MemoryLimit {
    pub max_bytes: u64,
    pub swap_max_bytes: u64,
}
impl Default for MemoryLimit {
    fn default() -> Self {
        Self {
            max_bytes: 4 * 1024 * 1024 * 1024,
            swap_max_bytes: 0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CpuLimit {
    pub cpu_time_secs: u64,
    pub cpus_allowed: u32,
}
impl Default for CpuLimit {
    fn default() -> Self {
        Self {
            cpu_time_secs: 3600,
            cpus_allowed: 2,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ResourceLimits {
    pub memory: MemoryLimit,
    pub cpu: CpuLimit,
}
impl ResourceLimits {
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
    pub fn medium() -> Self {
        Self::new(8 * 1024 * 1024 * 1024, 7200, 4)
    }
    pub fn heavy() -> Self {
        Self::new(16 * 1024 * 1024 * 1024, 14400, 8)
    }
}

pub fn apply_limits(limits: &ResourceLimits) -> std::io::Result<()> {
    set_memory_limit(limits.memory.max_bytes)?;
    set_cpu_limit(limits.cpu.cpu_time_secs)?;
    Ok(())
}

pub fn apply_limits_to_pid(pid: u32, limits: &ResourceLimits) -> std::io::Result<()> {
    run_prlimit(pid, "--as", limits.memory.max_bytes)?;
    run_prlimit(pid, "--cpu", limits.cpu.cpu_time_secs)
}

/// Apply CPU time only; RLIMIT_AS is unsafe for Node/browser virtual ranges.
pub fn apply_cpu_limit_to_pid(pid: u32, cpu_time_secs: u64) -> std::io::Result<()> {
    run_prlimit(pid, "--cpu", cpu_time_secs)
}

fn run_prlimit(pid: u32, resource: &str, value: u64) -> std::io::Result<()> {
    let output = Command::new("prlimit")
        .args([&format!("--pid={pid}"), &format!("{resource}={value}")])
        .output()?;
    if output.status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "prlimit {resource} for pid {pid} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )))
    }
}

fn set_memory_limit(max_bytes: u64) -> std::io::Result<()> {
    let output = Command::new("prlimit")
        .args([
            &format!("--pid={}", std::process::id()),
            &format!("--as={max_bytes}"),
        ])
        .output()?;
    if !output.status.success() {
        tracing::warn!(
            "prlimit --as failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}
fn set_cpu_limit(cpu_time_secs: u64) -> std::io::Result<()> {
    let output = Command::new("prlimit")
        .args([
            &format!("--pid={}", std::process::id()),
            &format!("--cpu={cpu_time_secs}"),
        ])
        .output()?;
    if !output.status.success() {
        tracing::warn!(
            "prlimit --cpu failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

pub fn is_in_cgroup_v2() -> bool {
    Path::new("/sys/fs/cgroup/cgroup.controllers").exists()
}
pub fn get_cgroup_path() -> Option<String> {
    std::fs::read_to_string("/proc/self/cgroup")
        .ok()
        .and_then(|content| {
            content.lines().find_map(|line| {
                let parts: Vec<_> = line.split(':').collect();
                (parts.len() >= 3 && parts[1] == "cgroup").then(|| parts[2].to_string())
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn defaults_are_stable() {
        assert_eq!(MemoryLimit::default().max_bytes, 4 * 1024 * 1024 * 1024);
        assert_eq!(CpuLimit::default().cpu_time_secs, 3600);
        assert_eq!(CpuLimit::default().cpus_allowed, 2);
    }
    #[test]
    fn constructors_set_values() {
        let l = ResourceLimits::new(8_000_000_000, 7200, 4);
        assert_eq!(l.memory.max_bytes, 8_000_000_000);
        assert_eq!(l.cpu.cpu_time_secs, 7200);
        assert_eq!(
            ResourceLimits::medium().memory.max_bytes,
            8 * 1024 * 1024 * 1024
        );
        assert_eq!(ResourceLimits::heavy().cpu.cpus_allowed, 8);
    }
    #[test]
    fn helpers_do_not_panic() {
        assert!(apply_limits(&ResourceLimits::default()).is_ok());
        let _ = is_in_cgroup_v2();
        let _ = get_cgroup_path();
    }
}

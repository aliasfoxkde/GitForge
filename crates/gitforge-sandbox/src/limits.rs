//! Sandbox resource limits

use gitforge_common::{Error, Result};
use serde::{Deserialize, Serialize};

/// CFS scheduling period in microseconds used for the derived Docker CPU
/// quota (100 ms, Docker's own default period).
pub const CPU_PERIOD_MICROS: i64 = 100_000;

/// Upper bound on `cpu_cores`. The derived quota is
/// `cores * CPU_PERIOD_MICROS`, so this keeps the value inside
/// kernel-accepted ranges and turns absurd configuration into a hard
/// error instead of a silently uncapped container.
pub const MAX_CPU_CORES: u32 = 1024;

/// Default CPU capacity in whole cores (matches the process-level
/// `CpuLimit` default `cpus_allowed` of 2).
fn default_cpu_cores() -> u32 {
    2
}

/// Resource limits for sandbox execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxLimits {
    /// Cumulative CPU-time budget in milliseconds across the sandbox
    /// lifetime. Declared policy only: nothing currently enforces this
    /// budget — no backend accounts consumption against it, and
    /// `SandboxLimits::timeout_secs` is not read at runtime (the enforced
    /// wall-clock deadline is the runner's `ExecutableJob::timeout_secs`,
    /// a separate field) — so treat it as informational. It must never be
    /// mapped to the Docker CFS quota: a quota caps the *rate* (cores),
    /// never the total. Capacity lives in `cpu_cores`.
    pub cpu_ms: u64,
    /// CPU capacity in whole cores, enforced by Docker's CFS bandwidth
    /// controller as `cpu_quota = cpu_cores * CPU_PERIOD_MICROS`.
    /// Fail-closed: `0` (which would disable the quota entirely) and
    /// values above `MAX_CPU_CORES` are rejected when a container is
    /// created.
    #[serde(default = "default_cpu_cores")]
    pub cpu_cores: u32,
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
            cpu_ms: 3600000,    // 1 hour
            memory_mb: 4096,    // 4GB
            disk_mb: 10240,     // 10GB
            timeout_secs: 3600, // 1 hour
            network: true,
            cpu_cores: default_cpu_cores(),
        }
    }
}

impl SandboxLimits {
    /// Docker CFS bandwidth quota in microseconds for this limit set.
    ///
    /// This is the *capacity* the container may use, derived from
    /// `cpu_cores` against a fixed `CPU_PERIOD_MICROS` period. Fails
    /// closed: `cpu_cores` of `0` (which would disable the quota
    /// entirely) or above `MAX_CPU_CORES` is a hard error rather than a
    /// silently uncapped container — the pre-fix mapping sent
    /// `cpu_ms * 1000` µs of quota per 100 ms period, which for the
    /// 1-hour default claimed 36,000 cores and capped nothing.
    pub fn validated_cpu_quota_micros(&self) -> Result<i64> {
        let cores = self.cpu_cores;
        if cores == 0 {
            return Err(Error::sandbox(
                "cpu_cores must be at least 1; 0 would disable the CFS quota entirely",
            ));
        }
        if cores > MAX_CPU_CORES {
            return Err(Error::sandbox(format!(
                "cpu_cores {cores} exceeds the maximum of {MAX_CPU_CORES}"
            )));
        }
        // Accepted values are bounded by MAX_CPU_CORES, so the product
        // (max 102_400_000) is far inside i64.
        Ok(i64::from(cores) * CPU_PERIOD_MICROS)
    }

    /// Create limits for a specific tier
    pub fn small() -> Self {
        Self {
            cpu_ms: 300000, // 5 minutes
            cpu_cores: 1,
            memory_mb: 512,
            disk_mb: 1024,
            timeout_secs: 300,
            network: false,
        }
    }

    pub fn medium() -> Self {
        Self {
            cpu_ms: 1800000, // 30 minutes
            cpu_cores: 2,
            memory_mb: 2048,
            disk_mb: 5120,
            timeout_secs: 1800,
            network: true,
        }
    }

    pub fn large() -> Self {
        Self {
            cpu_ms: 3600000,
            cpu_cores: 4,
            memory_mb: 8192,
            disk_mb: 20480,
            timeout_secs: 3600,
            network: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gitforge_common::ErrorKind;

    #[test]
    fn test_sandbox_limits_default() {
        let limits = SandboxLimits::default();
        assert_eq!(limits.cpu_ms, 3600000);
        assert_eq!(limits.cpu_cores, default_cpu_cores());
        assert_eq!(limits.memory_mb, 4096);
        assert!(limits.network);
    }

    #[test]
    fn test_sandbox_limits_small() {
        let limits = SandboxLimits::small();
        assert_eq!(limits.cpu_ms, 300000);
        assert_eq!(limits.cpu_cores, 1);
        assert_eq!(limits.memory_mb, 512);
        assert!(!limits.network);
    }

    #[test]
    fn test_sandbox_limits_medium() {
        let limits = SandboxLimits::medium();
        assert_eq!(limits.cpu_ms, 1800000);
        assert_eq!(limits.cpu_cores, 2);
        assert_eq!(limits.memory_mb, 2048);
        assert!(limits.network);
    }

    #[test]
    fn test_sandbox_limits_large() {
        let limits = SandboxLimits::large();
        assert_eq!(limits.cpu_ms, 3600000);
        assert_eq!(limits.cpu_cores, 4);
        assert_eq!(limits.memory_mb, 8192);
        assert!(limits.network);
    }

    #[test]
    fn test_sandbox_limits_debug() {
        let limits = SandboxLimits::default();
        let debug_str = format!("{limits:?}");
        assert!(debug_str.contains("cpu_ms"));
        assert!(debug_str.contains("memory_mb"));
    }

    #[test]
    fn test_sandbox_limits_clone() {
        let limits = SandboxLimits::large();
        let cloned = limits.clone();
        assert_eq!(cloned.cpu_ms, limits.cpu_ms);
        assert_eq!(cloned.memory_mb, limits.memory_mb);
    }

    #[test]
    fn test_sandbox_limits_all_tiers() {
        let small = SandboxLimits::small();
        let medium = SandboxLimits::medium();
        let large = SandboxLimits::large();
        let default = SandboxLimits::default();

        // Verify tier ordering
        assert!(small.memory_mb < medium.memory_mb);
        assert!(medium.memory_mb < large.memory_mb);
        assert!(default.memory_mb <= large.memory_mb);

        // Verify network settings
        assert!(!small.network); // Small has no network
        assert!(medium.network);
        assert!(large.network);
        assert!(default.network);
    }

    // =====================================================================
    // CPU capacity -> Docker CFS quota contract
    //
    // `cpu_cores` is the container's CPU *capacity*; `cpu_ms` is a
    // declared (currently unenforced) budget and must never become a
    // quota. These tests pin the exact values handed to Docker's
    // HostConfig and the fail-closed rejection of 0 / over-max.
    // =====================================================================

    #[test]
    fn cpu_period_is_the_docker_default_100ms() {
        assert_eq!(CPU_PERIOD_MICROS, 100_000);
    }

    /// Exact CFS quota for every tier and the default: whole cores times
    /// the 100 ms period, in microseconds.
    #[test]
    fn cpu_quota_is_cores_times_period_exactly() {
        assert_eq!(
            SandboxLimits::small().validated_cpu_quota_micros().unwrap(),
            100_000
        );
        assert_eq!(
            SandboxLimits::medium()
                .validated_cpu_quota_micros()
                .unwrap(),
            200_000
        );
        assert_eq!(
            SandboxLimits::large().validated_cpu_quota_micros().unwrap(),
            400_000
        );
        assert_eq!(
            SandboxLimits::default()
                .validated_cpu_quota_micros()
                .unwrap(),
            200_000
        );
    }

    /// Fail closed: `cpu_cores = 0` would disable the CFS quota entirely,
    /// leaving the container uncapped, so it is a hard error.
    #[test]
    fn zero_cpu_cores_is_rejected_fail_closed() {
        let limits = SandboxLimits {
            cpu_cores: 0,
            ..Default::default()
        };
        let error = limits
            .validated_cpu_quota_micros()
            .expect_err("zero capacity must be rejected");
        assert_eq!(error.kind, ErrorKind::Sandbox);
        assert!(
            error.to_string().contains("cpu_cores"),
            "error should name the offending field: {error}"
        );
    }

    #[test]
    fn cpu_cores_at_max_boundary_is_accepted_exactly() {
        let limits = SandboxLimits {
            cpu_cores: MAX_CPU_CORES,
            ..Default::default()
        };
        assert_eq!(
            limits.validated_cpu_quota_micros().unwrap(),
            i64::from(MAX_CPU_CORES) * CPU_PERIOD_MICROS
        );
        assert_eq!(limits.validated_cpu_quota_micros().unwrap(), 102_400_000);
    }

    /// Above the maximum must fail loudly rather than silently create an
    /// uncapped container — the exact defect this validation exists for.
    #[test]
    fn cpu_cores_above_max_is_rejected() {
        for cores in [MAX_CPU_CORES + 1, u32::MAX] {
            let limits = SandboxLimits {
                cpu_cores: cores,
                ..Default::default()
            };
            let error = limits
                .validated_cpu_quota_micros()
                .expect_err("oversized capacity must be rejected");
            assert_eq!(error.kind, ErrorKind::Sandbox);
            assert!(
                error.to_string().contains("cpu_cores"),
                "error should name the offending field: {error}"
            );
        }
    }

    /// Serialized-config compatibility: payloads written before `cpu_cores`
    /// existed must keep deserializing, picking up the default capacity.
    #[test]
    fn legacy_payload_without_cpu_cores_deserializes_to_default() {
        let legacy = serde_json::json!({
            "cpu_ms": 3_600_000u64,
            "memory_mb": 4096u64,
            "disk_mb": 10240u64,
            "timeout_secs": 3600u64,
            "network": true,
        });
        let limits: SandboxLimits = serde_json::from_value(legacy).expect("legacy payload");
        assert_eq!(limits.cpu_ms, 3_600_000);
        assert_eq!(limits.cpu_cores, default_cpu_cores());
        // The old payload still maps to the exact quota the default yields.
        assert_eq!(limits.validated_cpu_quota_micros().unwrap(), 200_000);
    }

    /// Serialize -> deserialize round trip keeps every field, including
    /// `cpu_cores`, so configs written by this version read back
    /// identically (JSON compatibility in both directions).
    #[test]
    fn serde_round_trip_preserves_all_fields() {
        let limits = SandboxLimits::large();
        let text = serde_json::to_string(&limits).expect("serialize");
        let back: SandboxLimits = serde_json::from_str(&text).expect("deserialize");
        assert_eq!(back.cpu_ms, limits.cpu_ms);
        assert_eq!(back.cpu_cores, limits.cpu_cores);
        assert_eq!(back.memory_mb, limits.memory_mb);
        assert_eq!(back.disk_mb, limits.disk_mb);
        assert_eq!(back.timeout_secs, limits.timeout_secs);
        assert_eq!(back.network, limits.network);
    }
}

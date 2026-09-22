//! Runner model

use chrono::{DateTime, Utc};
use gitforge_common::RunnerId;
use serde::{Deserialize, Serialize};

/// Runner type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunnerType {
    Docker,
    Firecracker,
    BareMetal,
}

impl RunnerType {
    pub fn as_str(&self) -> &'static str {
        match self {
            RunnerType::Docker => "docker",
            RunnerType::Firecracker => "firecracker",
            RunnerType::BareMetal => "bare_metal",
        }
    }
}

/// Runner status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunnerStatus {
    Online,
    Busy,
    Offline,
}

impl RunnerStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            RunnerStatus::Online => "online",
            RunnerStatus::Busy => "busy",
            RunnerStatus::Offline => "offline",
        }
    }
}

/// Heartbeat age after which a runner still marked `online` is reported and
/// scheduled as `offline`. One shared definition so API listings, scheduler
/// placement, and lease recovery agree on what "online" means.
pub const RUNNER_HEARTBEAT_OFFLINE_AFTER_SECS: i64 = 90;

/// Runner entity
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Runner {
    pub id: RunnerId,
    pub name: String,
    pub runner_type: String,
    pub status: String,
    pub last_heartbeat: Option<DateTime<Utc>>,
    pub capacity: i32,
    pub created_at: DateTime<Utc>,
}

impl Runner {
    /// Create a new runner
    pub fn new(name: String, runner_type: RunnerType, capacity: i32) -> Self {
        Self {
            id: RunnerId::new(),
            name,
            runner_type: runner_type.as_str().to_string(),
            status: RunnerStatus::Online.as_str().to_string(),
            last_heartbeat: Some(Utc::now()),
            capacity,
            created_at: Utc::now(),
        }
    }

    /// Update heartbeat
    pub fn heartbeat(&mut self) {
        self.last_heartbeat = Some(Utc::now());
    }

    /// Mark as busy
    pub fn set_busy(&mut self) {
        self.status = RunnerStatus::Busy.as_str().to_string();
    }

    /// Mark as online
    pub fn set_online(&mut self) {
        self.status = RunnerStatus::Online.as_str().to_string();
    }

    /// Check if runner is healthy (heartbeat within threshold)
    pub fn is_healthy(&self, threshold_secs: i64) -> bool {
        if let Some(heartbeat) = self.last_heartbeat {
            let age = Utc::now() - heartbeat;
            age.num_seconds() < threshold_secs
        } else {
            false
        }
    }

    /// Effective status with heartbeat liveness applied. A runner persisted
    /// as `online` whose last activity — the heartbeat, or the registration
    /// time when no heartbeat ever arrived — is older than the threshold
    /// reports `offline`. Registry rows left online by a scheduler restart
    /// therefore cannot lie to listings or placement.
    ///
    /// The comparison is strict: a last activity exactly at the threshold is
    /// still online, matching the tick-based recovery in the scheduler.
    pub fn effective_status(&self, now: DateTime<Utc>, offline_after_secs: i64) -> String {
        if self.status != RunnerStatus::Online.as_str() {
            return self.status.clone();
        }
        let last_activity = self.last_heartbeat.unwrap_or(self.created_at);
        if last_activity < now - chrono::Duration::seconds(offline_after_secs) {
            RunnerStatus::Offline.as_str().to_string()
        } else {
            RunnerStatus::Online.as_str().to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_runner_creation() {
        let runner = Runner::new("test-runner".to_string(), RunnerType::Docker, 4);
        assert_eq!(runner.name, "test-runner");
        assert_eq!(runner.runner_type, "docker");
        assert_eq!(runner.capacity, 4);
        assert_eq!(runner.status, "online");
    }

    #[test]
    fn test_runner_heartbeat() {
        let mut runner = Runner::new("test-runner".to_string(), RunnerType::Docker, 2);
        runner.heartbeat();
        assert!(runner.last_heartbeat.is_some());
    }

    #[test]
    fn test_runner_set_busy() {
        let mut runner = Runner::new("test-runner".to_string(), RunnerType::Firecracker, 2);
        runner.set_busy();
        assert_eq!(runner.status, "busy");
    }

    #[test]
    fn test_runner_set_online() {
        let mut runner = Runner::new("test-runner".to_string(), RunnerType::BareMetal, 1);
        runner.set_busy();
        runner.set_online();
        assert_eq!(runner.status, "online");
    }

    #[test]
    fn test_runner_is_healthy() {
        let mut runner = Runner::new("test-runner".to_string(), RunnerType::Docker, 2);
        runner.heartbeat();
        assert!(runner.is_healthy(60));
        assert!(!runner.is_healthy(0));
    }

    #[test]
    fn test_runner_is_not_healthy_without_heartbeat() {
        // Create a runner and manually set last_heartbeat to None
        let mut runner = Runner::new("test-runner".to_string(), RunnerType::Docker, 2);
        runner.last_heartbeat = None;
        assert!(!runner.is_healthy(60));
    }

    #[test]
    fn test_effective_status_boundary() {
        let now = Utc::now();
        let mut runner = Runner::new("hb-runner".to_string(), RunnerType::Docker, 1);
        assert_eq!(runner.effective_status(now, 90), "online");

        // Exactly at the threshold is still online (strict comparison, same
        // as the scheduler's tick-based recovery).
        runner.last_heartbeat = Some(now - chrono::Duration::seconds(90));
        assert_eq!(runner.effective_status(now, 90), "online");

        // One second past it is offline.
        runner.last_heartbeat = Some(now - chrono::Duration::seconds(91));
        assert_eq!(runner.effective_status(now, 90), "offline");
    }

    #[test]
    fn test_effective_status_passthrough_non_online() {
        let now = Utc::now();
        let mut runner = Runner::new("busy-runner".to_string(), RunnerType::Docker, 1);
        runner.set_busy();
        runner.last_heartbeat = Some(now - chrono::Duration::seconds(3600));
        assert_eq!(runner.effective_status(now, 90), "busy");

        runner.status = "offline".to_string();
        assert_eq!(runner.effective_status(now, 90), "offline");
    }

    #[test]
    fn test_effective_status_without_heartbeat_uses_registration_time() {
        let now = Utc::now();
        // No heartbeat ever arrived: a fresh registration is online, one
        // that registered longer ago than the threshold is offline.
        let mut fresh = Runner::new("fresh-runner".to_string(), RunnerType::Docker, 1);
        fresh.last_heartbeat = None;
        assert_eq!(fresh.effective_status(now, 90), "online");

        let mut ancient = Runner::new("ancient-runner".to_string(), RunnerType::Docker, 1);
        ancient.last_heartbeat = None;
        ancient.created_at = now - chrono::Duration::seconds(3600);
        assert_eq!(ancient.effective_status(now, 90), "offline");
    }

    #[test]
    fn test_runner_type_as_str() {
        assert_eq!(RunnerType::Docker.as_str(), "docker");
        assert_eq!(RunnerType::Firecracker.as_str(), "firecracker");
        assert_eq!(RunnerType::BareMetal.as_str(), "bare_metal");
    }

    #[test]
    fn test_runner_status_as_str() {
        assert_eq!(RunnerStatus::Online.as_str(), "online");
        assert_eq!(RunnerStatus::Busy.as_str(), "busy");
        assert_eq!(RunnerStatus::Offline.as_str(), "offline");
    }
}

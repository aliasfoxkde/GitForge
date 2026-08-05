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

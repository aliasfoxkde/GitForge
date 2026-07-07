//! Runner model

use chrono::{DateTime, Utc};
use gitforce_common::RunnerId;
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

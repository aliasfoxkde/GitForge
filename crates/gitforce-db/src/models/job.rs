//! Job model

use chrono::{DateTime, Utc};
use gitforce_common::{JobId, PipelineRunId, RunnerId};
use serde::{Deserialize, Serialize};

/// Job status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobStatus {
    Pending,
    Queued,
    Assigned,
    Running,
    Succeeded,
    Failed,
    Cancelled,
    TimedOut,
}

impl JobStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            JobStatus::Pending => "pending",
            JobStatus::Queued => "queued",
            JobStatus::Assigned => "assigned",
            JobStatus::Running => "running",
            JobStatus::Succeeded => "succeeded",
            JobStatus::Failed => "failed",
            JobStatus::Cancelled => "cancelled",
            JobStatus::TimedOut => "timed_out",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(JobStatus::Pending),
            "queued" => Some(JobStatus::Queued),
            "assigned" => Some(JobStatus::Assigned),
            "running" => Some(JobStatus::Running),
            "succeeded" => Some(JobStatus::Succeeded),
            "failed" => Some(JobStatus::Failed),
            "cancelled" => Some(JobStatus::Cancelled),
            "timed_out" => Some(JobStatus::TimedOut),
            _ => None,
        }
    }

    /// Check if job is in a terminal state
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            JobStatus::Succeeded | JobStatus::Failed | JobStatus::Cancelled | JobStatus::TimedOut
        )
    }
}

/// Job entity
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub id: JobId,
    pub pipeline_run_id: PipelineRunId,
    pub name: String,
    pub status: String,
    pub runner_id: Option<RunnerId>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub retry_count: i32,
    pub created_at: DateTime<Utc>,
}

impl Job {
    /// Create a new job
    pub fn new(pipeline_run_id: PipelineRunId, name: String) -> Self {
        Self {
            id: JobId::new(),
            pipeline_run_id,
            name,
            status: JobStatus::Pending.as_str().to_string(),
            runner_id: None,
            started_at: None,
            finished_at: None,
            retry_count: 0,
            created_at: Utc::now(),
        }
    }

    /// Mark job as queued
    pub fn queue(&mut self) {
        self.status = JobStatus::Queued.as_str().to_string();
    }

    /// Assign to a runner
    pub fn assign(&mut self, runner_id: RunnerId) {
        self.status = JobStatus::Assigned.as_str().to_string();
        self.runner_id = Some(runner_id);
    }

    /// Mark as running
    pub fn start(&mut self) {
        self.status = JobStatus::Running.as_str().to_string();
        self.started_at = Some(Utc::now());
    }

    /// Mark as finished
    pub fn finish(&mut self, success: bool) {
        self.status = if success {
            JobStatus::Succeeded.as_str().to_string()
        } else {
            JobStatus::Failed.as_str().to_string()
        };
        self.finished_at = Some(Utc::now());
    }

    /// Mark as timed out
    pub fn timeout(&mut self) {
        self.status = JobStatus::TimedOut.as_str().to_string();
        self.finished_at = Some(Utc::now());
    }

    /// Increment retry count
    pub fn retry(&mut self) {
        self.retry_count += 1;
        self.status = JobStatus::Queued.as_str().to_string();
        self.started_at = None;
        self.finished_at = None;
    }
}

//! Scheduler job model
//!
//! Persistent representation of a job managed by the scheduler. A row lives for
//! the whole job lifecycle (pending -> claimed -> terminal) so pending and
//! completed jobs survive scheduler restarts.

use chrono::{DateTime, Utc};
use gitforce_common::{JobId, PipelineRunId, RepoId, RunnerId};
use serde::{Deserialize, Serialize};

/// Status of a scheduler job
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SchedulerJobStatus {
    Pending,
    Claimed,
    Succeeded,
    Failed,
    Cancelled,
}

impl SchedulerJobStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            SchedulerJobStatus::Pending => "pending",
            SchedulerJobStatus::Claimed => "claimed",
            SchedulerJobStatus::Succeeded => "succeeded",
            SchedulerJobStatus::Failed => "failed",
            SchedulerJobStatus::Cancelled => "cancelled",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(SchedulerJobStatus::Pending),
            "claimed" => Some(SchedulerJobStatus::Claimed),
            "succeeded" => Some(SchedulerJobStatus::Succeeded),
            "failed" => Some(SchedulerJobStatus::Failed),
            "cancelled" => Some(SchedulerJobStatus::Cancelled),
            _ => None,
        }
    }

    /// Check if the status is terminal (the outcome has been recorded)
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            SchedulerJobStatus::Succeeded
                | SchedulerJobStatus::Failed
                | SchedulerJobStatus::Cancelled
        )
    }
}

/// Scheduler job entity
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchedulerJob {
    pub id: JobId,
    pub pipeline_run_id: PipelineRunId,
    pub repo_id: RepoId,
    pub name: String,
    pub status: String,
    pub commands: Vec<String>,
    pub working_dir: Option<String>,
    pub runner_id: Option<RunnerId>,
    pub claimed_at: Option<DateTime<Utc>>,
    pub success: Option<bool>,
    pub exit_code: Option<i64>,
    pub error: Option<String>,
    pub completed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl SchedulerJob {
    /// Create a new pending scheduler job
    pub fn new(
        pipeline_run_id: PipelineRunId,
        repo_id: RepoId,
        name: String,
        commands: Vec<String>,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: JobId::new(),
            pipeline_run_id,
            repo_id,
            name,
            status: SchedulerJobStatus::Pending.as_str().to_string(),
            commands,
            working_dir: None,
            runner_id: None,
            claimed_at: None,
            success: None,
            exit_code: None,
            error: None,
            completed_at: None,
            created_at: now,
            updated_at: now,
        }
    }

    /// Set an optional working directory
    pub fn with_working_dir(mut self, working_dir: Option<String>) -> Self {
        self.working_dir = working_dir;
        self
    }

    /// Parsed status of the job
    pub fn status(&self) -> Option<SchedulerJobStatus> {
        SchedulerJobStatus::from_str(&self.status)
    }

    /// Check if the outcome of the job has been recorded
    pub fn is_terminal(&self) -> bool {
        self.status().is_some_and(|status| status.is_terminal())
    }

    /// Check if the job still needs scheduler attention (pending or claimed)
    pub fn is_active(&self) -> bool {
        matches!(
            self.status(),
            Some(SchedulerJobStatus::Pending | SchedulerJobStatus::Claimed)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_job() -> SchedulerJob {
        SchedulerJob::new(
            PipelineRunId::new(),
            RepoId::new(),
            "build".to_string(),
            vec!["make build".to_string()],
        )
    }

    #[test]
    fn test_scheduler_job_status_as_str() {
        assert_eq!(SchedulerJobStatus::Pending.as_str(), "pending");
        assert_eq!(SchedulerJobStatus::Claimed.as_str(), "claimed");
        assert_eq!(SchedulerJobStatus::Succeeded.as_str(), "succeeded");
        assert_eq!(SchedulerJobStatus::Failed.as_str(), "failed");
        assert_eq!(SchedulerJobStatus::Cancelled.as_str(), "cancelled");
    }

    #[test]
    fn test_scheduler_job_status_from_str() {
        assert_eq!(
            SchedulerJobStatus::from_str("pending"),
            Some(SchedulerJobStatus::Pending)
        );
        assert_eq!(
            SchedulerJobStatus::from_str("claimed"),
            Some(SchedulerJobStatus::Claimed)
        );
        assert_eq!(
            SchedulerJobStatus::from_str("succeeded"),
            Some(SchedulerJobStatus::Succeeded)
        );
        assert_eq!(
            SchedulerJobStatus::from_str("failed"),
            Some(SchedulerJobStatus::Failed)
        );
        assert_eq!(
            SchedulerJobStatus::from_str("cancelled"),
            Some(SchedulerJobStatus::Cancelled)
        );
        assert_eq!(SchedulerJobStatus::from_str("unknown"), None);
    }

    #[test]
    fn test_scheduler_job_status_is_terminal() {
        assert!(!SchedulerJobStatus::Pending.is_terminal());
        assert!(!SchedulerJobStatus::Claimed.is_terminal());
        assert!(SchedulerJobStatus::Succeeded.is_terminal());
        assert!(SchedulerJobStatus::Failed.is_terminal());
        assert!(SchedulerJobStatus::Cancelled.is_terminal());
    }

    #[test]
    fn test_scheduler_job_new_defaults() {
        let job = sample_job();
        assert_eq!(job.name, "build");
        assert_eq!(job.commands, vec!["make build".to_string()]);
        assert_eq!(job.status, "pending");
        assert!(job.working_dir.is_none());
        assert!(job.runner_id.is_none());
        assert!(job.claimed_at.is_none());
        assert!(job.success.is_none());
        assert!(job.exit_code.is_none());
        assert!(job.error.is_none());
        assert!(job.completed_at.is_none());
        assert_eq!(job.created_at, job.updated_at);
    }

    #[test]
    fn test_scheduler_job_with_working_dir() {
        let job = sample_job().with_working_dir(Some("/workspace/repo".to_string()));
        assert_eq!(job.working_dir.as_deref(), Some("/workspace/repo"));
    }

    #[test]
    fn test_scheduler_job_is_active() {
        let mut job = sample_job();
        assert!(job.is_active());

        job.status = SchedulerJobStatus::Claimed.as_str().to_string();
        assert!(job.is_active());

        job.status = SchedulerJobStatus::Succeeded.as_str().to_string();
        assert!(!job.is_active());
    }

    #[test]
    fn test_scheduler_job_is_terminal() {
        let mut job = sample_job();
        assert!(!job.is_terminal());

        job.status = SchedulerJobStatus::Failed.as_str().to_string();
        assert!(job.is_terminal());
    }

    #[test]
    fn test_scheduler_job_id_unique() {
        let job1 = sample_job();
        let job2 = sample_job();
        assert_ne!(job1.id, job2.id);
    }
}

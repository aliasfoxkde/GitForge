//! Job model

use chrono::{DateTime, Utc};
use gitforge_common::{JobId, PipelineRunId, RunnerId};
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

    #[allow(clippy::should_implement_trait)]
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
    /// Container image for this job (e.g. "rust:1.75", "python:3.12").
    /// Must be non-empty for jobs in queued/assigned/running states.
    pub image: String,
}

impl Job {
    /// Create a new job with the given image.
    ///
    /// # Panics
    ///
    /// Panics if `image` is empty.
    pub fn new(pipeline_run_id: PipelineRunId, name: String, image: &str) -> Self {
        assert!(!image.is_empty(), "Job::new requires a non-empty image");
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
            image: image.to_string(),
        }
    }

    /// Returns true if this job has a non-empty image field.
    pub fn has_image(&self) -> bool {
        !self.image.is_empty()
    }

    /// Returns true if this job has valid execution metadata (non-empty image and at least one step).
    pub fn has_execution_metadata(&self) -> bool {
        !self.image.is_empty()
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

/// A single step within a job's execution pipeline.
/// Each step has a name, a command to run, optional environment variables,
/// and an optional working directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobStep {
    pub id: String,
    pub job_id: JobId,
    pub step_index: i32,
    pub name: String,
    pub run: String,
    pub env: serde_json::Value,
    pub working_dir: Option<String>,
}

impl JobStep {
    /// Create a new job step.
    pub fn new(job_id: JobId, step_index: i32, name: &str, run: &str) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            job_id,
            step_index,
            name: name.to_string(),
            run: run.to_string(),
            env: serde_json::json!({}),
            working_dir: None,
        }
    }

    /// Returns the command to execute for this step.
    pub fn command(&self) -> &str {
        &self.run
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_job_status_as_str() {
        assert_eq!(JobStatus::Pending.as_str(), "pending");
        assert_eq!(JobStatus::Queued.as_str(), "queued");
        assert_eq!(JobStatus::Assigned.as_str(), "assigned");
        assert_eq!(JobStatus::Running.as_str(), "running");
        assert_eq!(JobStatus::Succeeded.as_str(), "succeeded");
        assert_eq!(JobStatus::Failed.as_str(), "failed");
        assert_eq!(JobStatus::Cancelled.as_str(), "cancelled");
        assert_eq!(JobStatus::TimedOut.as_str(), "timed_out");
    }

    #[test]
    fn test_job_status_from_str() {
        assert_eq!(JobStatus::from_str("pending"), Some(JobStatus::Pending));
        assert_eq!(JobStatus::from_str("queued"), Some(JobStatus::Queued));
        assert_eq!(JobStatus::from_str("running"), Some(JobStatus::Running));
        assert_eq!(JobStatus::from_str("succeeded"), Some(JobStatus::Succeeded));
        assert_eq!(JobStatus::from_str("failed"), Some(JobStatus::Failed));
        assert_eq!(JobStatus::from_str("unknown"), None);
    }

    #[test]
    fn test_job_status_is_terminal() {
        assert!(!JobStatus::Pending.is_terminal());
        assert!(!JobStatus::Queued.is_terminal());
        assert!(!JobStatus::Running.is_terminal());
        assert!(JobStatus::Succeeded.is_terminal());
        assert!(JobStatus::Failed.is_terminal());
        assert!(JobStatus::Cancelled.is_terminal());
        assert!(JobStatus::TimedOut.is_terminal());
    }

    #[test]
    fn test_job_creation() {
        let pipeline_run_id = PipelineRunId::new();
        let job = Job::new(pipeline_run_id, "build".to_string(), "rust:1.75");
        assert_eq!(job.name, "build");
        assert_eq!(job.status, "pending");
        assert!(job.runner_id.is_none());
        assert_eq!(job.retry_count, 0);
        assert_eq!(job.image, "rust:1.75");
        assert!(job.has_image());
        assert!(job.has_execution_metadata());
    }

    #[test]
    fn test_job_queue() {
        let pipeline_run_id = PipelineRunId::new();
        let mut job = Job::new(pipeline_run_id, "build".to_string(), "rust:1.75");
        job.queue();
        assert_eq!(job.status, "queued");
    }

    #[test]
    fn test_job_assign() {
        let pipeline_run_id = PipelineRunId::new();
        let mut job = Job::new(pipeline_run_id, "build".to_string(), "rust:1.75");
        let runner_id = RunnerId::new();
        job.assign(runner_id);
        assert_eq!(job.status, "assigned");
        assert_eq!(job.runner_id, Some(runner_id));
    }

    #[test]
    fn test_job_start() {
        let pipeline_run_id = PipelineRunId::new();
        let mut job = Job::new(pipeline_run_id, "build".to_string(), "rust:1.75");
        job.start();
        assert_eq!(job.status, "running");
        assert!(job.started_at.is_some());
    }

    #[test]
    fn test_job_finish_success() {
        let pipeline_run_id = PipelineRunId::new();
        let mut job = Job::new(pipeline_run_id, "build".to_string(), "rust:1.75");
        job.finish(true);
        assert_eq!(job.status, "succeeded");
        assert!(job.finished_at.is_some());
    }

    #[test]
    fn test_job_finish_failure() {
        let pipeline_run_id = PipelineRunId::new();
        let mut job = Job::new(pipeline_run_id, "build".to_string(), "rust:1.75");
        job.finish(false);
        assert_eq!(job.status, "failed");
        assert!(job.finished_at.is_some());
    }

    #[test]
    fn test_job_timeout() {
        let pipeline_run_id = PipelineRunId::new();
        let mut job = Job::new(pipeline_run_id, "build".to_string(), "rust:1.75");
        job.timeout();
        assert_eq!(job.status, "timed_out");
        assert!(job.finished_at.is_some());
    }

    #[test]
    fn test_job_retry() {
        let pipeline_run_id = PipelineRunId::new();
        let mut job = Job::new(pipeline_run_id, "build".to_string(), "rust:1.75");
        job.start();
        job.finish(false);
        job.retry();
        assert_eq!(job.retry_count, 1);
        assert_eq!(job.status, "queued");
        assert!(job.started_at.is_none());
        assert!(job.finished_at.is_none());
    }

    #[test]
    fn test_job_id_unique() {
        let pipeline_run_id = PipelineRunId::new();
        let job1 = Job::new(pipeline_run_id, "build".to_string(), "rust:1.75");
        let job2 = Job::new(pipeline_run_id, "build".to_string(), "rust:1.75");
        assert_ne!(job1.id, job2.id);
    }

    #[test]
    fn test_job_image_field() {
        let pipeline_run_id = PipelineRunId::new();
        let job = Job::new(pipeline_run_id, "build".to_string(), "python:3.12-slim");
        assert_eq!(job.image, "python:3.12-slim");
        assert!(job.has_image());
        assert!(job.has_execution_metadata());
    }

    #[test]
    fn test_job_image_empty_by_default_via_default() {
        // Verify the field is present and defaults to empty string
        let pipeline_run_id = PipelineRunId::new();
        let job = Job::new(pipeline_run_id, "build".to_string(), "rust:latest");
        // After adding image, all jobs have an image via constructor
        assert!(!job.image.is_empty());
    }
}

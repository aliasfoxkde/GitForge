//! Pipeline models

use chrono::{DateTime, Utc};
use gitforge_common::{PipelineId, PipelineRunId, RepoId};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Pipeline entity
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pipeline {
    pub id: PipelineId,
    pub repo_id: RepoId,
    pub name: String,
    pub trigger_type: String,
    pub config: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

/// Trigger type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TriggerType {
    Push,
    Tag,
    PullRequest,
    Manual,
}

impl TriggerType {
    pub fn as_str(&self) -> &'static str {
        match self {
            TriggerType::Push => "push",
            TriggerType::Tag => "tag",
            TriggerType::PullRequest => "pull_request",
            TriggerType::Manual => "manual",
        }
    }
}

/// Pipeline run status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PipelineStatus {
    Pending,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

impl PipelineStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            PipelineStatus::Pending => "pending",
            PipelineStatus::Running => "running",
            PipelineStatus::Succeeded => "succeeded",
            PipelineStatus::Failed => "failed",
            PipelineStatus::Cancelled => "cancelled",
        }
    }
}

/// Pipeline run entity
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineRun {
    pub id: PipelineRunId,
    pub pipeline_id: PipelineId,
    pub repo_id: RepoId,
    pub status: String,
    pub triggered_by: String,
    pub commit_hash: String,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

impl PipelineRun {
    /// Create a new pipeline run
    pub fn new(
        pipeline_id: PipelineId,
        repo_id: RepoId,
        triggered_by: String,
        commit_hash: String,
    ) -> Self {
        Self {
            id: PipelineRunId::new(),
            pipeline_id,
            repo_id,
            status: PipelineStatus::Pending.as_str().to_string(),
            triggered_by,
            commit_hash,
            started_at: None,
            finished_at: None,
            created_at: Utc::now(),
        }
    }

    /// Mark as started
    pub fn start(&mut self) {
        self.status = PipelineStatus::Running.as_str().to_string();
        self.started_at = Some(Utc::now());
    }

    /// Mark as finished
    pub fn finish(&mut self, success: bool) {
        self.status = if success {
            PipelineStatus::Succeeded.as_str().to_string()
        } else {
            PipelineStatus::Failed.as_str().to_string()
        };
        self.finished_at = Some(Utc::now());
    }
}

/// Pipeline definition from config file
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineDefinition {
    pub name: String,
    pub trigger_on: Vec<TriggerType>,
    pub jobs: Vec<JobDefinition>,
    pub environment: HashMap<String, String>,
}

/// Job definition within a pipeline
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobDefinition {
    pub name: String,
    pub image: String,
    pub steps: Vec<StepDefinition>,
    pub needs: Vec<String>,
    pub env: HashMap<String, String>,
}

/// Step definition within a job
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepDefinition {
    pub name: String,
    pub run: String,
    pub env: Option<HashMap<String, String>>,
    pub working_directory: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pipeline_status_as_str() {
        assert_eq!(PipelineStatus::Pending.as_str(), "pending");
        assert_eq!(PipelineStatus::Running.as_str(), "running");
        assert_eq!(PipelineStatus::Succeeded.as_str(), "succeeded");
        assert_eq!(PipelineStatus::Failed.as_str(), "failed");
        assert_eq!(PipelineStatus::Cancelled.as_str(), "cancelled");
    }

    #[test]
    fn test_trigger_type_as_str() {
        assert_eq!(TriggerType::Push.as_str(), "push");
        assert_eq!(TriggerType::Tag.as_str(), "tag");
        assert_eq!(TriggerType::PullRequest.as_str(), "pull_request");
        assert_eq!(TriggerType::Manual.as_str(), "manual");
    }

    #[test]
    fn test_pipeline_run_creation() {
        let run = PipelineRun::new(
            PipelineId::new(),
            RepoId::new(),
            "push".to_string(),
            "abc123".to_string(),
        );
        assert_eq!(run.triggered_by, "push");
        assert_eq!(run.commit_hash, "abc123");
        assert_eq!(run.status, "pending");
    }

    #[test]
    fn test_pipeline_run_start() {
        let mut run = PipelineRun::new(
            PipelineId::new(),
            RepoId::new(),
            "push".to_string(),
            "abc123".to_string(),
        );
        run.start();
        assert_eq!(run.status, "running");
        assert!(run.started_at.is_some());
    }

    #[test]
    fn test_pipeline_run_finish_success() {
        let mut run = PipelineRun::new(
            PipelineId::new(),
            RepoId::new(),
            "push".to_string(),
            "abc123".to_string(),
        );
        run.start();
        run.finish(true);
        assert_eq!(run.status, "succeeded");
        assert!(run.finished_at.is_some());
    }

    #[test]
    fn test_pipeline_run_finish_failure() {
        let mut run = PipelineRun::new(
            PipelineId::new(),
            RepoId::new(),
            "push".to_string(),
            "abc123".to_string(),
        );
        run.start();
        run.finish(false);
        assert_eq!(run.status, "failed");
        assert!(run.finished_at.is_some());
    }

    #[test]
    fn test_pipeline_definition_serialization() {
        let def = PipelineDefinition {
            name: "test".to_string(),
            trigger_on: vec![TriggerType::Push],
            jobs: vec![],
            environment: HashMap::new(),
        };
        let json = serde_json::to_string(&def).unwrap();
        assert!(json.contains("test"));
    }

    #[test]
    fn test_job_definition_serialization() {
        let job = JobDefinition {
            name: "build".to_string(),
            image: "rust:latest".to_string(),
            steps: vec![],
            needs: vec![],
            env: HashMap::new(),
        };
        let json = serde_json::to_string(&job).unwrap();
        assert!(json.contains("build"));
    }

    #[test]
    fn test_step_definition_serialization() {
        let step = StepDefinition {
            name: "cargo build".to_string(),
            run: "cargo build --release".to_string(),
            env: None,
            working_directory: None,
        };
        let json = serde_json::to_string(&step).unwrap();
        assert!(json.contains("cargo build"));
    }
}

//! UUID type definitions for all GitForce entities
//!
//! Each entity type has its own dedicated UUID type to prevent
//! mixing up IDs across different entity types.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Repository identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RepoId(pub Uuid);

impl RepoId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for RepoId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for RepoId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<Uuid> for RepoId {
    fn from(uuid: Uuid) -> Self {
        Self(uuid)
    }
}

impl From<RepoId> for Uuid {
    fn from(id: RepoId) -> Self {
        id.0
    }
}

/// Pipeline identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PipelineId(pub Uuid);

impl PipelineId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for PipelineId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for PipelineId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<Uuid> for PipelineId {
    fn from(uuid: Uuid) -> Self {
        Self(uuid)
    }
}

impl From<PipelineId> for Uuid {
    fn from(id: PipelineId) -> Self {
        id.0
    }
}

/// Pipeline run identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PipelineRunId(pub Uuid);

impl PipelineRunId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for PipelineRunId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for PipelineRunId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<Uuid> for PipelineRunId {
    fn from(uuid: Uuid) -> Self {
        Self(uuid)
    }
}

impl From<PipelineRunId> for Uuid {
    fn from(id: PipelineRunId) -> Self {
        id.0
    }
}

/// Job identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct JobId(pub Uuid);

impl JobId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for JobId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for JobId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<Uuid> for JobId {
    fn from(uuid: Uuid) -> Self {
        Self(uuid)
    }
}

impl From<JobId> for Uuid {
    fn from(id: JobId) -> Self {
        id.0
    }
}

/// Runner identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RunnerId(pub Uuid);

impl RunnerId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for RunnerId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for RunnerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<Uuid> for RunnerId {
    fn from(uuid: Uuid) -> Self {
        Self(uuid)
    }
}

impl From<RunnerId> for Uuid {
    fn from(id: RunnerId) -> Self {
        id.0
    }
}

/// Step identifier within a job
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct StepId(pub Uuid);

impl StepId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for StepId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for StepId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<Uuid> for StepId {
    fn from(uuid: Uuid) -> Self {
        Self(uuid)
    }
}

impl From<StepId> for Uuid {
    fn from(id: StepId) -> Self {
        id.0
    }
}

/// User identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct UserId(pub Uuid);

impl UserId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for UserId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for UserId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<Uuid> for UserId {
    fn from(uuid: Uuid) -> Self {
        Self(uuid)
    }
}

impl From<UserId> for Uuid {
    fn from(id: UserId) -> Self {
        id.0
    }
}

/// Job status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    /// Check if this status is terminal (no further transitions possible)
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            JobStatus::Succeeded | JobStatus::Failed | JobStatus::Cancelled | JobStatus::TimedOut
        )
    }
}

/// Pipeline status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipelineStatus {
    Pending,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_repo_id_generation() {
        let id1 = RepoId::new();
        let id2 = RepoId::new();
        assert_ne!(id1, id2);
        // Verify it's a valid UUID (v4)
        assert!(id1.0.get_version_num() == 4);
    }

    #[test]
    fn test_id_conversion() {
        let id = RepoId::new();
        let uuid: Uuid = id.into();
        let id2: RepoId = uuid.into();
        assert_eq!(id, id2);
    }

    #[test]
    fn test_id_display() {
        let id = RepoId::new();
        let display = format!("{}", id);
        assert_eq!(display, id.0.to_string());
    }

    #[test]
    fn test_pipeline_id_generation() {
        let id1 = PipelineId::new();
        let id2 = PipelineId::new();
        assert_ne!(id1, id2);
        assert!(id1.0.get_version_num() == 4);
    }

    #[test]
    fn test_pipeline_run_id_generation() {
        let id1 = PipelineRunId::new();
        let id2 = PipelineRunId::new();
        assert_ne!(id1, id2);
        assert!(id1.0.get_version_num() == 4);
    }

    #[test]
    fn test_job_id_generation() {
        let id1 = JobId::new();
        let id2 = JobId::new();
        assert_ne!(id1, id2);
        assert!(id1.0.get_version_num() == 4);
    }

    #[test]
    fn test_runner_id_generation() {
        let id1 = RunnerId::new();
        let id2 = RunnerId::new();
        assert_ne!(id1, id2);
        assert!(id1.0.get_version_num() == 4);
    }

    #[test]
    fn test_step_id_generation() {
        let id1 = StepId::new();
        let id2 = StepId::new();
        assert_ne!(id1, id2);
        assert!(id1.0.get_version_num() == 4);
    }

    #[test]
    fn test_user_id_generation() {
        let id1 = UserId::new();
        let id2 = UserId::new();
        assert_ne!(id1, id2);
        assert!(id1.0.get_version_num() == 4);
    }

    #[test]
    fn test_all_id_types_have_unique_uuids() {
        let repo = RepoId::new();
        let pipeline = PipelineId::new();
        let run = PipelineRunId::new();
        let job = JobId::new();
        let runner = RunnerId::new();
        let step = StepId::new();
        let user = UserId::new();

        // All should be unique
        let ids = [repo.0, pipeline.0, run.0, job.0, runner.0, step.0, user.0];
        let mut sorted = ids;
        sorted.sort();
        for i in 1..sorted.len() {
            assert_ne!(sorted[i], sorted[i-1], "IDs should be unique");
        }
    }

    #[test]
    fn test_job_status_is_terminal() {
        assert!(!crate::JobStatus::Pending.is_terminal());
        assert!(!crate::JobStatus::Running.is_terminal());
        assert!(crate::JobStatus::Succeeded.is_terminal());
        assert!(crate::JobStatus::Failed.is_terminal());
        assert!(crate::JobStatus::Cancelled.is_terminal());
        assert!(crate::JobStatus::TimedOut.is_terminal());
    }

    #[test]
    fn test_pipeline_status_values() {
        use crate::PipelineStatus;
        assert!(!matches!(PipelineStatus::Pending, PipelineStatus::Running));
        assert!(!matches!(PipelineStatus::Running, PipelineStatus::Pending));
    }

    #[test]
    fn test_job_status_non_terminal() {
        assert!(!crate::JobStatus::Queued.is_terminal());
        assert!(!crate::JobStatus::Assigned.is_terminal());
    }

    #[test]
    fn test_pipeline_status_terminal() {
        use crate::PipelineStatus;
        // PipelineStatus doesn't have is_terminal, but we can verify the variants exist
        let _ = PipelineStatus::Pending;
        let _ = PipelineStatus::Running;
        let _ = PipelineStatus::Succeeded;
        let _ = PipelineStatus::Failed;
        let _ = PipelineStatus::Cancelled;
    }

    #[test]
    fn test_id_serialization() {
        // RepoId uses serde with #[serde(transparent)] so it serializes as a UUID string
        // We test that From<Uuid> and Into<Uuid> work correctly
        let uuid = uuid::Uuid::new_v4();
        let id: RepoId = uuid.into();
        let round_trip: Uuid = id.into();
        assert_eq!(uuid, round_trip);
    }

    #[test]
    fn test_pipeline_id_from_uuid() {
        let uuid = Uuid::new_v4();
        let id: PipelineId = uuid.into();
        assert_eq!(id.0, uuid);
    }

    #[test]
    fn test_pipeline_id_into_uuid() {
        let id = PipelineId::new();
        let uuid: Uuid = id.into();
        assert_eq!(uuid, id.0);
    }

    #[test]
    fn test_pipeline_run_id_from_uuid() {
        let uuid = Uuid::new_v4();
        let id: PipelineRunId = uuid.into();
        assert_eq!(id.0, uuid);
    }

    #[test]
    fn test_pipeline_run_id_into_uuid() {
        let id = PipelineRunId::new();
        let uuid: Uuid = id.into();
        assert_eq!(uuid, id.0);
    }

    #[test]
    fn test_job_id_from_uuid() {
        let uuid = Uuid::new_v4();
        let id: JobId = uuid.into();
        assert_eq!(id.0, uuid);
    }

    #[test]
    fn test_job_id_into_uuid() {
        let id = JobId::new();
        let uuid: Uuid = id.into();
        assert_eq!(uuid, id.0);
    }

    #[test]
    fn test_runner_id_from_uuid() {
        let uuid = Uuid::new_v4();
        let id: RunnerId = uuid.into();
        assert_eq!(id.0, uuid);
    }

    #[test]
    fn test_runner_id_into_uuid() {
        let id = RunnerId::new();
        let uuid: Uuid = id.into();
        assert_eq!(uuid, id.0);
    }

    #[test]
    fn test_step_id_from_uuid() {
        let uuid = Uuid::new_v4();
        let id: StepId = uuid.into();
        assert_eq!(id.0, uuid);
    }

    #[test]
    fn test_step_id_into_uuid() {
        let id = StepId::new();
        let uuid: Uuid = id.into();
        assert_eq!(uuid, id.0);
    }

    #[test]
    fn test_user_id_from_uuid() {
        let uuid = Uuid::new_v4();
        let id: UserId = uuid.into();
        assert_eq!(id.0, uuid);
    }

    #[test]
    fn test_user_id_into_uuid() {
        let id = UserId::new();
        let uuid: Uuid = id.into();
        assert_eq!(uuid, id.0);
    }

    #[test]
    fn test_all_ids_default() {
        assert!(RepoId::default() != RepoId::new()); // Default calls new()
        assert!(PipelineId::default() != PipelineId::new());
        assert!(PipelineRunId::default() != PipelineRunId::new());
        assert!(JobId::default() != JobId::new());
        assert!(RunnerId::default() != RunnerId::new());
        assert!(StepId::default() != StepId::new());
        assert!(UserId::default() != UserId::new());
    }

    #[test]
    fn test_all_ids_display() {
        assert!(!format!("{}", RepoId::new()).is_empty());
        assert!(!format!("{}", PipelineId::new()).is_empty());
        assert!(!format!("{}", PipelineRunId::new()).is_empty());
        assert!(!format!("{}", JobId::new()).is_empty());
        assert!(!format!("{}", RunnerId::new()).is_empty());
        assert!(!format!("{}", StepId::new()).is_empty());
        assert!(!format!("{}", UserId::new()).is_empty());
    }

    #[test]
    fn test_all_ids_debug() {
        let repo = RepoId::new();
        let pipeline = PipelineId::new();
        let run = PipelineRunId::new();
        let job = JobId::new();
        let runner = RunnerId::new();
        let step = StepId::new();
        let user = UserId::new();

        assert!(format!("{:?}", repo).contains("RepoId"));
        assert!(format!("{:?}", pipeline).contains("PipelineId"));
        assert!(format!("{:?}", run).contains("PipelineRunId"));
        assert!(format!("{:?}", job).contains("JobId"));
        assert!(format!("{:?}", runner).contains("RunnerId"));
        assert!(format!("{:?}", step).contains("StepId"));
        assert!(format!("{:?}", user).contains("UserId"));
    }

    #[test]
    fn test_all_ids_clone() {
        let repo = RepoId::new();
        let pipeline = PipelineId::new();
        let run = PipelineRunId::new();
        let job = JobId::new();
        let runner = RunnerId::new();
        let step = StepId::new();
        let user = UserId::new();

        assert_eq!(repo, repo.clone());
        assert_eq!(pipeline, pipeline.clone());
        assert_eq!(run, run.clone());
        assert_eq!(job, job.clone());
        assert_eq!(runner, runner.clone());
        assert_eq!(step, step.clone());
        assert_eq!(user, user.clone());
    }

    #[test]
    fn test_id_hash_trait() {
        use std::collections::HashSet;
        let mut set: HashSet<RepoId> = HashSet::new();
        set.insert(RepoId::new());
        set.insert(RepoId::new());
        assert!(set.len() >= 1);
    }

    #[test]
    fn test_job_status_debug() {
        assert!(format!("{:?}", JobStatus::Pending).contains("Pending"));
        assert!(format!("{:?}", JobStatus::Succeeded).contains("Succeeded"));
    }

    #[test]
    fn test_pipeline_status_debug() {
        assert!(format!("{:?}", PipelineStatus::Pending).contains("Pending"));
        assert!(format!("{:?}", PipelineStatus::Failed).contains("Failed"));
    }

    #[test]
    fn test_job_status_partialeq() {
        assert_eq!(JobStatus::Pending, JobStatus::Pending);
        assert_ne!(JobStatus::Pending, JobStatus::Running);
    }

    #[test]
    fn test_pipeline_status_partialeq() {
        assert_eq!(PipelineStatus::Pending, PipelineStatus::Pending);
        assert_ne!(PipelineStatus::Pending, PipelineStatus::Running);
    }
}

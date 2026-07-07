//! Event envelope and core event types

use chrono::{DateTime, Utc};
use gitforce_common::{JobId, PipelineId, PipelineRunId, RepoId, RunnerId, UserId};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Event envelope - all events are wrapped in this
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventEnvelope {
    /// Unique event identifier
    pub event_id: Uuid,
    /// Event type discriminator
    pub event_type: EventType,
    /// Event schema version
    pub event_version: u8,
    /// Event timestamp (Unix milliseconds)
    pub timestamp: i64,
    /// Associated repository (if applicable)
    pub repo_id: Option<RepoId>,
    /// Actor who triggered the event (if applicable)
    pub actor_id: Option<UserId>,
    /// Correlation ID for tracing related events
    pub correlation_id: Option<Uuid>,
    /// Event-specific payload
    pub payload: EventPayload,
}

impl EventEnvelope {
    /// Create a new event envelope
    pub fn new(
        event_type: EventType,
        payload: EventPayload,
        repo_id: Option<RepoId>,
        actor_id: Option<UserId>,
    ) -> Self {
        Self {
            event_id: Uuid::new_v4(),
            event_type,
            event_version: 1,
            timestamp: Utc::now().timestamp_millis(),
            repo_id,
            actor_id,
            correlation_id: None,
            payload,
        }
    }

    /// Create a new event with a correlation ID
    pub fn with_correlation(
        mut self,
        correlation_id: Uuid,
    ) -> Self {
        self.correlation_id = Some(correlation_id);
        self
    }

    /// Get the timestamp as a DateTime
    pub fn timestamp_datetime(&self) -> DateTime<Utc> {
        DateTime::from_timestamp_millis(self.timestamp).unwrap_or_default()
    }
}

/// Event types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventType {
    // Repository events
    RepoCreated,
    RepoDeleted,

    // Git events
    PushReceived,
    RefUpdated,

    // Pipeline events
    PipelineTriggered,
    PipelineStarted,
    PipelineFinished,

    // Job events
    JobQueued,
    JobStarted,
    JobFinished,

    // Artifact events
    ArtifactCreated,

    // Runner events
    RunnerRegistered,
    RunnerHeartbeat,
    RunnerOffline,

    // Mirror events
    MirrorSyncRequested,
    MirrorSyncCompleted,
}

impl EventType {
    /// Get the event type name as a string
    pub fn as_str(&self) -> &'static str {
        match self {
            EventType::RepoCreated => "repo.created",
            EventType::RepoDeleted => "repo.deleted",
            EventType::PushReceived => "push.received",
            EventType::RefUpdated => "ref.updated",
            EventType::PipelineTriggered => "pipeline.triggered",
            EventType::PipelineStarted => "pipeline.started",
            EventType::PipelineFinished => "pipeline.finished",
            EventType::JobQueued => "job.queued",
            EventType::JobStarted => "job.started",
            EventType::JobFinished => "job.finished",
            EventType::ArtifactCreated => "artifact.created",
            EventType::RunnerRegistered => "runner.registered",
            EventType::RunnerHeartbeat => "runner.heartbeat",
            EventType::RunnerOffline => "runner.offline",
            EventType::MirrorSyncRequested => "mirror.sync_requested",
            EventType::MirrorSyncCompleted => "mirror.sync_completed",
        }
    }
}

impl std::fmt::Display for EventType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Event payloads
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventPayload {
    // Repository payloads
    RepoCreated(RepoCreatedPayload),
    RepoDeleted(RepoDeletedPayload),

    // Git payloads
    PushReceived(PushReceivedPayload),
    RefUpdated(RefUpdatedPayload),

    // Pipeline payloads
    PipelineTriggered(PipelineTriggeredPayload),
    PipelineStarted(PipelineStartedPayload),
    PipelineFinished(PipelineFinishedPayload),

    // Job payloads
    JobQueued(JobQueuedPayload),
    JobStarted(JobStartedPayload),
    JobFinished(JobFinishedPayload),

    // Artifact payloads
    ArtifactCreated(ArtifactCreatedPayload),

    // Runner payloads
    RunnerRegistered(RunnerRegisteredPayload),
    RunnerHeartbeat(RunnerHeartbeatPayload),
    RunnerOffline(RunnerOfflinePayload),

    // Mirror payloads
    MirrorSyncRequested(MirrorSyncRequestedPayload),
    MirrorSyncCompleted(MirrorSyncCompletedPayload),
}

/// Repository created payload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoCreatedPayload {
    pub repo_id: RepoId,
    pub name: String,
    pub owner_id: UserId,
    pub visibility: String,
}

/// Repository deleted payload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoDeletedPayload {
    pub repo_id: RepoId,
    pub name: String,
}

/// Push received payload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PushReceivedPayload {
    pub repo_id: RepoId,
    pub ref_name: String,
    pub old_hash: String,
    pub new_hash: String,
    pub pusher_id: Option<UserId>,
}

/// Ref updated payload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefUpdatedPayload {
    pub repo_id: RepoId,
    pub ref_name: String,
    pub old_hash: String,
    pub new_hash: String,
}

/// Pipeline triggered payload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineTriggeredPayload {
    pub pipeline_id: PipelineId,
    pub pipeline_run_id: PipelineRunId,
    pub repo_id: RepoId,
    pub commit_hash: String,
    pub trigger_source: String,
}

/// Pipeline started payload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineStartedPayload {
    pub pipeline_run_id: PipelineRunId,
    pub started_at: i64,
}

/// Pipeline finished payload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineFinishedPayload {
    pub pipeline_run_id: PipelineRunId,
    pub status: String,
    pub duration_ms: u64,
}

/// Job queued payload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobQueuedPayload {
    pub job_id: JobId,
    pub pipeline_run_id: PipelineRunId,
    pub name: String,
}

/// Job started payload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobStartedPayload {
    pub job_id: JobId,
    pub runner_id: RunnerId,
    pub started_at: i64,
}

/// Job finished payload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobFinishedPayload {
    pub job_id: JobId,
    pub status: String,
    pub exit_code: i32,
    pub duration_ms: u64,
}

/// Artifact created payload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactCreatedPayload {
    pub artifact_id: Uuid,
    pub job_id: JobId,
    pub path: String,
    pub checksum: String,
    pub size_bytes: u64,
}

/// Runner registered payload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunnerRegisteredPayload {
    pub runner_id: RunnerId,
    pub name: String,
    pub runner_type: String,
    pub capacity: i32,
}

/// Runner heartbeat payload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunnerHeartbeatPayload {
    pub runner_id: RunnerId,
    pub capacity_used: u32,
    pub active_jobs: u32,
}

/// Runner offline payload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunnerOfflinePayload {
    pub runner_id: RunnerId,
    pub reason: String,
}

/// Mirror sync requested payload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MirrorSyncRequestedPayload {
    pub repo_id: RepoId,
    pub github_repo: String,
    pub branch: Option<String>,
}

/// Mirror sync completed payload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MirrorSyncCompletedPayload {
    pub repo_id: RepoId,
    pub github_repo: String,
    pub commit_hash: String,
    pub success: bool,
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_type_str() {
        assert_eq!(EventType::PushReceived.as_str(), "push.received");
        assert_eq!(EventType::PipelineTriggered.as_str(), "pipeline.triggered");
    }

    #[test]
    fn test_event_envelope_creation() {
        let payload = PushReceivedPayload {
            repo_id: RepoId::new(),
            ref_name: "refs/heads/main".to_string(),
            old_hash: "abc123".to_string(),
            new_hash: "def456".to_string(),
            pusher_id: None,
        };

        let event = EventEnvelope::new(
            EventType::PushReceived,
            EventPayload::PushReceived(payload),
            None,
            None,
        );

        assert_eq!(event.event_version, 1);
        assert!(event.timestamp > 0);
    }
}

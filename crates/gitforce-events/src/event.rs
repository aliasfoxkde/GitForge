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
        assert_eq!(EventType::JobQueued.as_str(), "job.queued");
        assert_eq!(EventType::JobStarted.as_str(), "job.started");
        assert_eq!(EventType::JobFinished.as_str(), "job.finished");
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

    #[test]
    fn test_job_queued_payload() {
        let payload = JobQueuedPayload {
            job_id: JobId::new(),
            pipeline_run_id: PipelineRunId::new(),
            name: "build".to_string(),
        };
        assert_eq!(payload.name, "build");
    }

    #[test]
    fn test_job_started_payload() {
        let payload = JobStartedPayload {
            job_id: JobId::new(),
            runner_id: RunnerId::new(),
            started_at: 1234567890,
        };
        assert_eq!(payload.started_at, 1234567890);
    }

    #[test]
    fn test_job_finished_payload() {
        let payload = JobFinishedPayload {
            job_id: JobId::new(),
            status: "succeeded".to_string(),
            exit_code: 0,
            duration_ms: 5000,
        };
        assert_eq!(payload.status, "succeeded");
        assert_eq!(payload.exit_code, 0);
    }

    #[test]
    fn test_artifact_created_payload() {
        let payload = ArtifactCreatedPayload {
            artifact_id: Uuid::new_v4(),
            job_id: JobId::new(),
            path: "/artifacts/test.zip".to_string(),
            checksum: "abc123".to_string(),
            size_bytes: 1024,
        };
        assert_eq!(payload.path, "/artifacts/test.zip");
        assert_eq!(payload.size_bytes, 1024);
    }

    #[test]
    fn test_runner_registered_payload() {
        let payload = RunnerRegisteredPayload {
            runner_id: RunnerId::new(),
            name: "runner-1".to_string(),
            runner_type: "docker".to_string(),
            capacity: 4,
        };
        assert_eq!(payload.name, "runner-1");
        assert_eq!(payload.capacity, 4);
    }

    #[test]
    fn test_runner_heartbeat_payload() {
        let payload = RunnerHeartbeatPayload {
            runner_id: RunnerId::new(),
            capacity_used: 2,
            active_jobs: 1,
        };
        assert_eq!(payload.capacity_used, 2);
        assert_eq!(payload.active_jobs, 1);
    }

    #[test]
    fn test_mirror_sync_completed_payload() {
        let payload = MirrorSyncCompletedPayload {
            repo_id: RepoId::new(),
            github_repo: "owner/repo".to_string(),
            commit_hash: "abc123".to_string(),
            success: true,
            error: None,
        };
        assert!(payload.success);
        assert!(payload.error.is_none());
    }

    #[test]
    fn test_pipeline_triggered_payload() {
        let payload = PipelineTriggeredPayload {
            pipeline_id: PipelineId::new(),
            pipeline_run_id: PipelineRunId::new(),
            repo_id: RepoId::new(),
            commit_hash: "abc123".to_string(),
            trigger_source: "push".to_string(),
        };
        assert_eq!(payload.trigger_source, "push");
    }

    #[test]
    fn test_pipeline_started_payload() {
        let payload = PipelineStartedPayload {
            pipeline_run_id: PipelineRunId::new(),
            started_at: 1234567890,
        };
        assert_eq!(payload.started_at, 1234567890);
    }

    #[test]
    fn test_pipeline_finished_payload() {
        let payload = PipelineFinishedPayload {
            pipeline_run_id: PipelineRunId::new(),
            status: "succeeded".to_string(),
            duration_ms: 60000,
        };
        assert_eq!(payload.status, "succeeded");
        assert_eq!(payload.duration_ms, 60000);
    }

    #[test]
    fn test_push_received_payload() {
        let payload = PushReceivedPayload {
            repo_id: RepoId::new(),
            ref_name: "refs/heads/main".to_string(),
            old_hash: "abc123".to_string(),
            new_hash: "def456".to_string(),
            pusher_id: Some(UserId::new()),
        };
        assert!(payload.pusher_id.is_some());
        assert_eq!(payload.old_hash, "abc123");
    }

    #[test]
    fn test_ref_updated_payload() {
        let payload = RefUpdatedPayload {
            repo_id: RepoId::new(),
            ref_name: "refs/tags/v1.0".to_string(),
            old_hash: "abc123".to_string(),
            new_hash: "def456".to_string(),
        };
        assert_eq!(payload.ref_name, "refs/tags/v1.0");
    }

    #[test]
    fn test_repo_created_payload() {
        let payload = RepoCreatedPayload {
            repo_id: RepoId::new(),
            name: "test-repo".to_string(),
            owner_id: UserId::new(),
            visibility: "public".to_string(),
        };
        assert_eq!(payload.visibility, "public");
    }

    #[test]
    fn test_repo_deleted_payload() {
        let payload = RepoDeletedPayload {
            repo_id: RepoId::new(),
            name: "test-repo".to_string(),
        };
        assert_eq!(payload.name, "test-repo");
    }

    #[test]
    fn test_runner_offline_payload() {
        let payload = RunnerOfflinePayload {
            runner_id: RunnerId::new(),
            reason: "heartbeat timeout".to_string(),
        };
        assert_eq!(payload.reason, "heartbeat timeout");
    }

    #[test]
    fn test_mirror_sync_requested_payload() {
        let payload = MirrorSyncRequestedPayload {
            repo_id: RepoId::new(),
            github_repo: "owner/repo".to_string(),
            branch: Some("main".to_string()),
        };
        assert_eq!(payload.github_repo, "owner/repo");
        assert_eq!(payload.branch, Some("main".to_string()));
    }

    #[test]
    fn test_event_envelope_with_correlation() {
        let payload = PushReceivedPayload {
            repo_id: RepoId::new(),
            ref_name: "refs/heads/main".to_string(),
            old_hash: "abc123".to_string(),
            new_hash: "def456".to_string(),
            pusher_id: None,
        };

        let correlation_id = Uuid::new_v4();
        let event = EventEnvelope::new(
            EventType::PushReceived,
            EventPayload::PushReceived(payload),
            None,
            None,
        ).with_correlation(correlation_id);

        assert_eq!(event.correlation_id, Some(correlation_id));
    }

    #[test]
    fn test_event_envelope_timestamp_datetime() {
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

        let dt = event.timestamp_datetime();
        assert_eq!(dt.timestamp_millis(), event.timestamp);
    }

    #[test]
    fn test_event_type_display() {
        assert_eq!(format!("{}", EventType::PushReceived), "push.received");
        assert_eq!(format!("{}", EventType::PipelineTriggered), "pipeline.triggered");
        assert_eq!(format!("{}", EventType::RunnerHeartbeat), "runner.heartbeat");
    }

    #[test]
    fn test_event_type_equality() {
        assert_eq!(EventType::PushReceived, EventType::PushReceived);
        assert_ne!(EventType::PushReceived, EventType::PipelineTriggered);
    }

    #[test]
    fn test_event_type_hash() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(EventType::PushReceived);
        set.insert(EventType::PipelineTriggered);
        assert_eq!(set.len(), 2);
        set.insert(EventType::PushReceived);
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn test_event_envelope_all_event_types() {
        let test_cases = vec![
            (EventType::RepoCreated, "repo.created"),
            (EventType::RepoDeleted, "repo.deleted"),
            (EventType::PushReceived, "push.received"),
            (EventType::RefUpdated, "ref.updated"),
            (EventType::PipelineTriggered, "pipeline.triggered"),
            (EventType::PipelineStarted, "pipeline.started"),
            (EventType::PipelineFinished, "pipeline.finished"),
            (EventType::JobQueued, "job.queued"),
            (EventType::JobStarted, "job.started"),
            (EventType::JobFinished, "job.finished"),
            (EventType::ArtifactCreated, "artifact.created"),
            (EventType::RunnerRegistered, "runner.registered"),
            (EventType::RunnerHeartbeat, "runner.heartbeat"),
            (EventType::RunnerOffline, "runner.offline"),
            (EventType::MirrorSyncRequested, "mirror.sync_requested"),
            (EventType::MirrorSyncCompleted, "mirror.sync_completed"),
        ];

        for (event_type, expected_str) in test_cases {
            assert_eq!(event_type.as_str(), expected_str);
            assert_eq!(format!("{}", event_type), expected_str);
        }
    }

    #[test]
    fn test_event_envelope_debug() {
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

        let debug_str = format!("{:?}", event);
        assert!(debug_str.contains("PushReceived"));
        assert!(debug_str.contains("event_id"));
    }

    #[test]
    fn test_event_envelope_serde_roundtrip() {
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

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("push_received"));

        let deserialized: EventEnvelope = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.event_type, EventType::PushReceived);
    }

    #[test]
    fn test_event_payload_serde() {
        let payloads = vec![
            EventPayload::RepoCreated(RepoCreatedPayload {
                repo_id: RepoId::new(),
                name: "test-repo".to_string(),
                owner_id: UserId::new(),
                visibility: "public".to_string(),
            }),
            EventPayload::RepoDeleted(RepoDeletedPayload {
                repo_id: RepoId::new(),
                name: "test-repo".to_string(),
            }),
        ];

        for payload in payloads {
            let json = serde_json::to_string(&payload).unwrap();
            assert!(!json.is_empty());
        }
    }

    #[test]
    fn test_event_type_serde() {
        let event_type = EventType::PushReceived;
        let json = serde_json::to_string(&event_type).unwrap();
        assert_eq!(json, "\"push_received\"");
        let deserialized: EventType = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, EventType::PushReceived);
    }

    #[test]
    fn test_event_envelope_with_both_ids() {
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
            Some(RepoId::new()),
            Some(UserId::new()),
        );

        assert!(event.repo_id.is_some());
        assert!(event.actor_id.is_some());
    }

    #[test]
    fn test_event_envelope_timestamp_valid() {
        let payload = PushReceivedPayload {
            repo_id: RepoId::new(),
            ref_name: "refs/heads/main".to_string(),
            old_hash: "abc123".to_string(),
            new_hash: "def456".to_string(),
            pusher_id: None,
        };

        let before = chrono::Utc::now().timestamp_millis();
        let event = EventEnvelope::new(
            EventType::PushReceived,
            EventPayload::PushReceived(payload),
            None,
            None,
        );
        let after = chrono::Utc::now().timestamp_millis();

        assert!(event.timestamp >= before);
        assert!(event.timestamp <= after);
    }
}

//! CI trigger event correlation models
//!
//! One durable row per accepted `POST /pipelines/trigger` request, keyed by
//! the `event_id` returned to the caller. The row is what lets a caller that
//! received a `queued` answer (the in-process correlation window elapsed
//! before the run was created) recover its `pipeline_run_id` after the fact
//! — across service restarts, because it lives in the shared SQLite store.

use chrono::{DateTime, Utc};
use gitforge_common::{PipelineRunId, RepoId};

/// The event was accepted and is waiting for the consumer to create its run.
pub const TRIGGER_EVENT_PENDING: &str = "pending";

/// The consumer created the run; `pipeline_run_id` is set.
pub const TRIGGER_EVENT_CORRELATED: &str = "correlated";

/// The consumer failed while handling the event; no run exists for it.
pub const TRIGGER_EVENT_FAILED: &str = "failed";

/// Durable `event_id -> pipeline_run_id` correlation for one trigger request.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CiTriggerEvent {
    pub event_id: uuid::Uuid,
    pub pipeline_run_id: Option<PipelineRunId>,
    /// One of the `TRIGGER_EVENT_*` constants.
    pub status: String,
    pub repo_id: RepoId,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ci_trigger_event_status_constants() {
        assert_eq!(TRIGGER_EVENT_PENDING, "pending");
        assert_eq!(TRIGGER_EVENT_CORRELATED, "correlated");
        assert_eq!(TRIGGER_EVENT_FAILED, "failed");
    }

    #[test]
    fn test_ci_trigger_event_serialization_round_trip() {
        let event = CiTriggerEvent {
            event_id: uuid::Uuid::new_v4(),
            pipeline_run_id: Some(PipelineRunId::new()),
            status: TRIGGER_EVENT_CORRELATED.to_string(),
            repo_id: RepoId::new(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: CiTriggerEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.event_id, event.event_id);
        assert_eq!(parsed.pipeline_run_id, event.pipeline_run_id);
        assert_eq!(parsed.status, event.status);
    }
}

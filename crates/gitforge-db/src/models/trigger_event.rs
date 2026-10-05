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
    /// Serialized push payload the event was accepted with (F2). `None` on
    /// rows written before recovery existed; such rows cannot be re-driven
    /// and are failed fail-closed by the recovery sweep.
    pub payload: Option<String>,
    /// Working directory the trigger request asked for, honored when the
    /// event is re-driven after a restart.
    pub working_dir: Option<String>,
    /// How many times a driver (live consumer or recovery sweep) has claimed
    /// this event. Bounds the recovery retry budget.
    pub attempts: i64,
    /// Last failure recorded by a driver, for operators reading the row.
    pub last_error: Option<String>,
}

/// A pending trigger event held under an active lease by one driver.
///
/// The lease is the whole coordination scheme: the live consumer claims an
/// event when it picks it up from the bus, the recovery sweep claims only
/// unclaimed or lease-expired rows, and both drivers settle the row through
/// claim-guarded writes. Two drivers can therefore never process the same
/// accepted event at the same time (barring a drive that outlives its lease,
/// where the run-idempotency index converges the outcome onto one run).
#[derive(Debug, Clone)]
pub struct TriggerEventClaim {
    /// The claimed correlation row, including its durable payload.
    pub event: CiTriggerEvent,
    /// Proof of ownership; settle writes are conditional on this token.
    pub claim_token: String,
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
            payload: None,
            working_dir: None,
            attempts: 0,
            last_error: None,
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: CiTriggerEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.event_id, event.event_id);
        assert_eq!(parsed.pipeline_run_id, event.pipeline_run_id);
        assert_eq!(parsed.status, event.status);
    }
}

//! Durable pipeline trigger request model

use chrono::{DateTime, Utc};
use gitforge_common::{PipelineRunId, RepoId};

/// A durably recorded request to run a pipeline for one pushed revision.
///
/// The CI trigger endpoint persists this row BEFORE publishing to the
/// in-memory event bus, and the event consumer marks it terminal only when
/// the run actually materialized (or failed for good). A process restart
/// between "git-server marked the webhook delivered" and "the bus consumer
/// created the run" used to lose the trigger entirely — 11% of all
/// deliveries on the live instance (177 of 1,614) never produced a run.
/// This row is the durable hand-off that makes the loss impossible.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PipelineTriggerRequest {
    /// Request identifier; also carried as the event envelope's
    /// correlation id so the consumer can find the row it is working off.
    pub id: uuid::Uuid,
    pub repo_id: RepoId,
    pub ref_name: String,
    pub old_hash: String,
    pub new_hash: String,
    /// Lifecycle: `pending` (accepted, not yet picked up), `processing`
    /// (a consumer is creating the run), `completed` (run materialized),
    /// `failed` (terminal failure; `error` carries the cause).
    pub status: String,
    /// The run this request produced, once known.
    pub run_id: Option<PipelineRunId>,
    /// How many times the consumer has attempted this request.
    pub attempts: i64,
    /// Terminal failure cause, when `status = failed`.
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl PipelineTriggerRequest {
    /// A fresh, not-yet-processed trigger request.
    pub fn new(repo_id: RepoId, ref_name: String, old_hash: String, new_hash: String) -> Self {
        let now = Utc::now();
        Self {
            id: uuid::Uuid::new_v4(),
            repo_id,
            ref_name,
            old_hash,
            new_hash,
            status: "pending".to_string(),
            run_id: None,
            attempts: 0,
            error: None,
            created_at: now,
            updated_at: now,
        }
    }
}

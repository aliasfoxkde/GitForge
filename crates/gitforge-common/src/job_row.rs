//! The authoritative store's view of a job id, carried as data.
//!
//! A container's `com.gitforce.job_id` label only means something if the
//! store has a row for that id. The runner's warm pool minted job-shaped
//! ids for containers that were never given to a job, and the difference
//! between such a placeholder and a real job is exactly the difference
//! between a row and no row.
//!
//! The store's answer is a value, not a lookup a decision performs: a
//! decision that reads the store itself inherits every concurrent
//! writer's timing, so the answer is resolved once at the edge and
//! handed to the decision as data.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// What the authoritative store says about a job id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobRowState {
    /// The store has a row for this job id: it is a real job.
    Present,
    /// The store has no row for this job id: it was never a job.
    Absent,
    /// The store could not be consulted. Never a licence to remove.
    Unknown,
}

impl JobRowState {
    /// The wire form of this state, for logs and receipts.
    pub fn as_str(&self) -> &'static str {
        match self {
            JobRowState::Present => "present",
            JobRowState::Absent => "absent",
            JobRowState::Unknown => "unknown",
        }
    }
}

/// Bulk job-existence request.
///
/// The runner asks about the job ids it stamped on the containers it
/// found, so the store can say which of them were ever jobs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobExistenceRequest {
    /// The job ids to answer for.
    pub job_ids: Vec<String>,
}

/// Bulk job-existence response: one answer per requested id.
///
/// A requested id that the store cannot speak for is answered `unknown`
/// rather than omitted, so an incomplete answer is visible as such and
/// never reads as "absent".
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobExistenceResponse {
    /// The store's view of each requested job id.
    pub states: HashMap<String, JobRowState>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wire_form_round_trips() {
        for state in [
            JobRowState::Present,
            JobRowState::Absent,
            JobRowState::Unknown,
        ] {
            let json = serde_json::to_string(&state).expect("serialize state");
            let parsed: JobRowState = serde_json::from_str(&json).expect("parse state");
            assert_eq!(parsed, state);
            assert_eq!(json, format!("\"{}\"", state.as_str()));
        }
    }

    #[test]
    fn a_response_round_trips() {
        let response = JobExistenceResponse {
            states: HashMap::from([
                ("a".to_string(), JobRowState::Present),
                ("b".to_string(), JobRowState::Absent),
                ("c".to_string(), JobRowState::Unknown),
            ]),
        };
        let json = serde_json::to_string(&response).expect("serialize response");
        let parsed: JobExistenceResponse = serde_json::from_str(&json).expect("parse response");
        assert_eq!(parsed.states.len(), 3);
        assert_eq!(parsed.states["a"], JobRowState::Present);
        assert_eq!(parsed.states["b"], JobRowState::Absent);
        assert_eq!(parsed.states["c"], JobRowState::Unknown);
    }
}

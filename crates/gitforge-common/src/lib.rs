//! GitForce Common Library
//!
//! Shared primitives for all GitForce components including:
//! - UUID types for all entities
//! - Unified error handling
//! - Result type aliases
//! - Time utilities

pub mod error;
pub mod ids;
pub mod password;
pub mod result;
pub mod time;

pub use error::{Error, ErrorKind, Result};
pub use ids::{
    JobId, JobStatus, PipelineId, PipelineRunId, PipelineStatus, RepoId, RunnerId, SshKeyId,
    StepId, UserId,
};
pub use time::DateTime;

/// True when `hash` is git's "ref does not exist" sentinel: empty or all
/// zero digits. `git receive-pack` sends an all-zero new hash on branch
/// deletion (and an all-zero old hash on branch creation); only the
/// deletion case must never reach CI, because there is no commit to build.
pub fn is_zero_hash(hash: &str) -> bool {
    hash.is_empty() || hash.bytes().all(|byte| byte == b'0')
}

/// How long the CI orchestrator's `POST /pipelines/trigger` handler waits
/// for the pipeline bus consumer to durably create the run and correlate
/// its id before answering `queued` (202 without a run id). The trigger is
/// latency-coupled to a durable `BEGIN IMMEDIATE` write, so under dispatch
/// storms the window can elapse honestly; the run is still created by the
/// consumer either way.
///
/// Clients of that endpoint MUST budget more than this window. The api
/// gateway's `CiTriggerClient` derives its request timeout from it (+ a
/// margin) so a slow-but-successful trigger surfaces as the orchestrator's
/// own `queued` response rather than a manufactured gateway 502 for a
/// trigger that in fact succeeded.
pub const CI_TRIGGER_CORRELATION_WINDOW: std::time::Duration = std::time::Duration::from_secs(15);

#[cfg(test)]
mod tests {
    use super::is_zero_hash;

    #[test]
    fn zero_hash_matches_deletion_and_creation_sentinels() {
        assert!(is_zero_hash("0000000000000000000000000000000000000000"));
        assert!(is_zero_hash("0"));
        assert!(is_zero_hash(""));
    }

    #[test]
    fn zero_hash_rejects_real_hashes() {
        assert!(!is_zero_hash("681fb4dfa3059321947bc3cfad93e11f0527f24a"));
        assert!(!is_zero_hash("000000000000000000000000000000000000000a"));
        assert!(!is_zero_hash("HEAD"));
    }
}

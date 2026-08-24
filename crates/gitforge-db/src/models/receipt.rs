//! Durable execution evidence for CI jobs.

use chrono::{DateTime, Utc};
use gitforge_common::{JobId, PipelineRunId};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// One bounded stdout/stderr record for a completed job.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobLog {
    pub id: Uuid,
    pub job_id: JobId,
    pub pipeline_run_id: PipelineRunId,
    pub stdout: String,
    pub stderr: String,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    pub created_at: DateTime<Utc>,
}

const MAX_LOG_BYTES: usize = 1_048_576;

fn bound(value: String) -> (String, bool) {
    if value.len() <= MAX_LOG_BYTES {
        return (value, false);
    }
    let mut end = MAX_LOG_BYTES.saturating_sub(32);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    (format!("{}\n[output truncated]", &value[..end]), true)
}

impl JobLog {
    pub fn new(
        job_id: JobId,
        pipeline_run_id: PipelineRunId,
        stdout: String,
        stderr: String,
    ) -> Self {
        let (stdout, stdout_truncated) = bound(stdout);
        let (stderr, stderr_truncated) = bound(stderr);
        Self {
            id: Uuid::new_v4(),
            job_id,
            pipeline_run_id,
            stdout,
            stderr,
            stdout_truncated,
            stderr_truncated,
            created_at: Utc::now(),
        }
    }
}

/// An append-only, correlated event receipt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventReceipt {
    pub id: Uuid,
    pub event_type: String,
    pub job_id: Option<JobId>,
    pub pipeline_run_id: Option<PipelineRunId>,
    pub correlation_id: String,
    pub payload: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

impl EventReceipt {
    pub fn new(
        event_type: impl Into<String>,
        job_id: Option<JobId>,
        pipeline_run_id: Option<PipelineRunId>,
        correlation_id: impl Into<String>,
        payload: serde_json::Value,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            event_type: event_type.into(),
            job_id,
            pipeline_run_id,
            correlation_id: correlation_id.into(),
            payload,
            created_at: Utc::now(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounds_large_output() {
        let log = JobLog::new(
            JobId::new(),
            PipelineRunId::new(),
            "x".repeat(2_000_000),
            String::new(),
        );
        assert!(log.stdout_truncated);
        assert!(log.stdout.len() <= MAX_LOG_BYTES);
    }
}

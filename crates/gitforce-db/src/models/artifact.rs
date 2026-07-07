//! Artifact model

use chrono::{DateTime, Utc};
use gitforce_common::JobId;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Artifact entity
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    pub id: Uuid,
    pub job_id: JobId,
    pub path: String,
    pub checksum: String,
    pub size_bytes: i64,
    pub created_at: DateTime<Utc>,
}

impl Artifact {
    /// Create a new artifact
    pub fn new(job_id: JobId, path: String, checksum: String, size_bytes: i64) -> Self {
        Self {
            id: Uuid::new_v4(),
            job_id,
            path,
            checksum,
            size_bytes,
            created_at: Utc::now(),
        }
    }
}

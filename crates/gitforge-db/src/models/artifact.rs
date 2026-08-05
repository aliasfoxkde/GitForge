//! Artifact model

use chrono::{DateTime, Utc};
use gitforge_common::JobId;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_artifact_creation() {
        let artifact = Artifact::new(
            JobId::new(),
            "/path/to/test.bin".to_string(),
            "abc123".to_string(),
            1024,
        );
        assert_eq!(artifact.path, "/path/to/test.bin");
        assert_eq!(artifact.size_bytes, 1024);
        assert_eq!(artifact.checksum, "abc123");
    }

    #[test]
    fn test_artifact_id_unique() {
        let artifact1 = Artifact::new(JobId::new(), "/path/1".to_string(), "abc".to_string(), 100);
        let artifact2 = Artifact::new(JobId::new(), "/path/2".to_string(), "def".to_string(), 200);
        assert_ne!(artifact1.id, artifact2.id);
    }

    #[test]
    fn test_artifact_size() {
        let artifact = Artifact::new(
            JobId::new(),
            "/path/to/artifact.zip".to_string(),
            "checksum123".to_string(),
            4096,
        );
        assert_eq!(artifact.size_bytes, 4096);
    }
}

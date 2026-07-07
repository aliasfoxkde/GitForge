//! Artifact storage

use async_trait::async_trait;
use gitforce_common::{Error, JobId, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use uuid::Uuid;

/// Artifact identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ArtifactId(Uuid);

impl ArtifactId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for ArtifactId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for ArtifactId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Artifact metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    pub id: ArtifactId,
    pub job_id: JobId,
    pub name: String,
    pub path: String,
    pub checksum: String,
    pub size_bytes: u64,
    pub content_type: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl Artifact {
    /// Create artifact metadata from a file
    pub async fn from_file(job_id: JobId, name: String, path: &PathBuf) -> Result<Self> {
        let mut file = File::open(path).await.map_err(|e| {
            Error::storage(format!("failed to open file for artifact: {}", e))
        })?;

        let mut hasher = Sha256::new();
        let mut size: u64 = 0;
        let mut buffer = vec![0u8; 8192];

        loop {
            let bytes_read = file.read(&mut buffer).await.map_err(|e| {
                Error::storage(format!("failed to read file for artifact: {}", e))
            })?;

            if bytes_read == 0 {
                break;
            }

            hasher.update(&buffer[..bytes_read]);
            size += bytes_read as u64;
        }

        let checksum = hex::encode(hasher.finalize());

        Ok(Self {
            id: ArtifactId::new(),
            job_id,
            name,
            path: path.to_string_lossy().to_string(),
            checksum,
            size_bytes: size,
            content_type: None,
            created_at: chrono::Utc::now(),
        })
    }
}

/// Artifact store trait
#[async_trait]
pub trait ArtifactStore: Send + Sync {
    /// Store an artifact
    async fn put(&self, artifact: &Artifact, data: &[u8]) -> Result<()>;

    /// Retrieve an artifact
    async fn get(&self, id: ArtifactId) -> Result<Vec<u8>>;

    /// Delete an artifact
    async fn delete(&self, id: ArtifactId) -> Result<()>;

    /// Get artifact metadata
    async fn get_metadata(&self, id: ArtifactId) -> Result<Artifact>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_artifact_id_generation() {
        let id1 = ArtifactId::new();
        let id2 = ArtifactId::new();
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_artifact_id_display() {
        let id = ArtifactId::new();
        let display = format!("{}", id);
        assert_eq!(display, id.0.to_string());
    }

    #[test]
    fn test_artifact_id_default() {
        let id = ArtifactId::default();
        let id2 = ArtifactId::new();
        assert_ne!(id, id2); // Default creates new unique ID
    }

    #[test]
    fn test_artifact_id_uuid_conversion() {
        // ArtifactId wraps Uuid internally
        let id = ArtifactId::new();
        let id_inner = id.0;
        assert_eq!(id.0, id_inner);
    }

    #[test]
    fn test_artifact_creation_fields() {
        let job_id = JobId::new();
        let artifact = Artifact {
            id: ArtifactId::new(),
            job_id,
            name: "test-artifact.zip".to_string(),
            path: "/tmp/artifact.zip".to_string(),
            checksum: "abc123".to_string(),
            size_bytes: 1024,
            content_type: Some("application/zip".to_string()),
            created_at: chrono::Utc::now(),
        };
        assert_eq!(artifact.name, "test-artifact.zip");
        assert_eq!(artifact.size_bytes, 1024);
        assert_eq!(artifact.content_type, Some("application/zip".to_string()));
    }
}

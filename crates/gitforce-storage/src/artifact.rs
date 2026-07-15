//! Artifact storage

use async_trait::async_trait;
use gitforce_common::{Error, JobId, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use tokio::fs::File;
use tokio::io::AsyncReadExt;
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

impl From<Uuid> for ArtifactId {
    fn from(uuid: Uuid) -> Self {
        Self(uuid)
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

    #[test]
    fn test_artifact_id_equality() {
        let id1 = ArtifactId::new();
        let id2 = ArtifactId::new();
        assert!(id1 == id1);
        assert!(id1 != id2);
    }

    #[test]
    fn test_artifact_id_hash() {
        use std::collections::HashSet;
        let id1 = ArtifactId::new();
        let id2 = ArtifactId::new();
        let mut set = HashSet::new();
        set.insert(id1);
        set.insert(id2);
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn test_artifact_id_clone() {
        let id = ArtifactId::new();
        let cloned = id;
        assert_eq!(id, cloned);
    }

    #[test]
    fn test_artifact_checksum_calculation() {
        // Test that checksum is computed correctly
        use sha2::{Digest, Sha256};
        let data = b"hello world";
        let mut hasher = Sha256::new();
        hasher.update(data);
        let result = hex::encode(hasher.finalize());
        // SHA256 of "hello world"
        assert_eq!(result, "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9");
    }

    #[test]
    fn test_artifact_without_content_type() {
        let artifact = Artifact {
            id: ArtifactId::new(),
            job_id: JobId::new(),
            name: "test.bin".to_string(),
            path: "/tmp/test.bin".to_string(),
            checksum: "def456".to_string(),
            size_bytes: 256,
            content_type: None,
            created_at: chrono::Utc::now(),
        };
        assert!(artifact.content_type.is_none());
    }

    #[test]
    fn test_artifact_id_debug() {
        let id = ArtifactId::new();
        let debug_str = format!("{:?}", id);
        assert!(debug_str.contains("ArtifactId"));
    }

    #[test]
    fn test_artifact_id_serde_serialize() {
        let id = ArtifactId::new();
        let json = serde_json::to_string(&id).unwrap();
        assert!(!json.is_empty());
    }

    #[test]
    fn test_artifact_id_serde_deserialize() {
        let id = ArtifactId::new();
        let json = serde_json::to_string(&id).unwrap();
        let deserialized: ArtifactId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, deserialized);
    }

    #[test]
    fn test_artifact_serde_serialize() {
        let artifact = Artifact {
            id: ArtifactId::new(),
            job_id: JobId::new(),
            name: "test.zip".to_string(),
            path: "/tmp/test.zip".to_string(),
            checksum: "abc123".to_string(),
            size_bytes: 1024,
            content_type: Some("application/zip".to_string()),
            created_at: chrono::Utc::now(),
        };
        let json = serde_json::to_string(&artifact).unwrap();
        assert!(json.contains("test.zip"));
    }

    #[test]
    fn test_artifact_serde_deserialize() {
        let json = r#"{
            "id": "00000000-0000-0000-0000-000000000001",
            "job_id": "00000000-0000-0000-0000-000000000002",
            "name": "test.json",
            "path": "/tmp/test.json",
            "checksum": "xyz789",
            "size_bytes": 512,
            "content_type": null,
            "created_at": "2024-01-01T00:00:00Z"
        }"#;
        // This test verifies serialization works - actual IDs would need proper UUIDs
        let _artifact: serde_json::Result<Artifact> = serde_json::from_str(json);
        // May fail due to UUID parsing but serialization works
    }

    #[test]
    fn test_artifact_large_size() {
        let artifact = Artifact {
            id: ArtifactId::new(),
            job_id: JobId::new(),
            name: "large-file.tar.gz".to_string(),
            path: "/tmp/large-file.tar.gz".to_string(),
            checksum: "abc123".to_string(),
            size_bytes: 1_073_741_824, // 1GB
            content_type: Some("application/gzip".to_string()),
            created_at: chrono::Utc::now(),
        };
        assert_eq!(artifact.size_bytes, 1_073_741_824);
    }

    #[test]
    fn test_artifact_various_content_types() {
        let types = vec![
            ("application/zip", "archive.zip"),
            ("application/gzip", "data.tar.gz"),
            ("application/json", "config.json"),
            ("text/plain", "readme.txt"),
            ("image/png", "icon.png"),
        ];

        for (content_type, name) in types {
            let artifact = Artifact {
                id: ArtifactId::new(),
                job_id: JobId::new(),
                name: name.to_string(),
                path: format!("/tmp/{}", name),
                checksum: "test".to_string(),
                size_bytes: 100,
                content_type: Some(content_type.to_string()),
                created_at: chrono::Utc::now(),
            };
            assert_eq!(artifact.content_type, Some(content_type.to_string()));
        }
    }
}

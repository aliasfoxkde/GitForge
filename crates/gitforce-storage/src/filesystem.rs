//! Filesystem-based storage implementation

use crate::artifact::{Artifact, ArtifactId, ArtifactStore};
use crate::cache::{CacheEntry, CacheKey, CacheStore};
use async_trait::async_trait;
use gitforce_common::{Error, Result};
use std::path::PathBuf;
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Filesystem-based storage for artifacts and cache
pub struct FileStorage {
    root: PathBuf,
    artifacts_dir: PathBuf,
    cache_dir: PathBuf,
}

impl FileStorage {
    /// Create a new filesystem storage
    pub async fn new(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        let artifacts_dir = root.join("artifacts");
        let cache_dir = root.join("cache");

        // Create directories
        fs::create_dir_all(&artifacts_dir).await.map_err(|e| {
            Error::storage(format!("failed to create artifacts directory: {}", e))
        })?;

        fs::create_dir_all(&cache_dir).await.map_err(|e| {
            Error::storage(format!("failed to create cache directory: {}", e))
        })?;

        Ok(Self {
            root,
            artifacts_dir,
            cache_dir,
        })
    }

    /// Get artifact path
    fn artifact_path(&self, id: ArtifactId) -> PathBuf {
        self.artifacts_dir.join(format!("{}.data", id))
    }

    /// Get artifact metadata path
    fn artifact_meta_path(&self, id: ArtifactId) -> PathBuf {
        self.artifacts_dir.join(format!("{}.meta.json", id))
    }

    /// Get cache path
    fn cache_path(&self, key: &CacheKey) -> PathBuf {
        self.cache_dir.join(format!("{}.data", key.hash()))
    }
}

#[async_trait]
impl ArtifactStore for FileStorage {
    async fn put(&self, artifact: &Artifact, data: &[u8]) -> Result<()> {
        let artifact_path = self.artifact_path(artifact.id);
        let meta_path = self.artifact_meta_path(artifact.id);

        // Write data
        let mut file = fs::File::create(&artifact_path).await.map_err(|e| {
            Error::storage(format!("failed to create artifact file: {}", e))
        })?;

        file.write_all(data).await.map_err(|e| {
            Error::storage(format!("failed to write artifact data: {}", e))
        })?;

        // Write metadata
        let meta_json = serde_json::to_string_pretty(artifact).map_err(|e| {
            Error::storage(format!("failed to serialize artifact metadata: {}", e))
        })?;

        let mut meta_file = fs::File::create(&meta_path).await.map_err(|e| {
            Error::storage(format!("failed to create artifact metadata file: {}", e))
        })?;

        meta_file.write_all(meta_json.as_bytes()).await.map_err(|e| {
            Error::storage(format!("failed to write artifact metadata: {}", e))
        })?;

        tracing::info!(
            "stored artifact {} ({} bytes)",
            artifact.id,
            artifact.size_bytes
        );
        Ok(())
    }

    async fn get(&self, id: ArtifactId) -> Result<Vec<u8>> {
        let path = self.artifact_path(id);

        let mut file = fs::File::open(&path).await.map_err(|e| {
            Error::storage(format!("failed to open artifact file: {}", e))
        })?;

        let mut data = Vec::new();
        file.read_to_end(&mut data).await.map_err(|e| {
            Error::storage(format!("failed to read artifact data: {}", e))
        })?;

        Ok(data)
    }

    async fn delete(&self, id: ArtifactId) -> Result<()> {
        let artifact_path = self.artifact_path(id);
        let meta_path = self.artifact_meta_path(id);

        if artifact_path.exists() {
            fs::remove_file(&artifact_path).await.map_err(|e| {
                Error::storage(format!("failed to delete artifact file: {}", e))
            })?;
        }

        if meta_path.exists() {
            fs::remove_file(&meta_path).await.map_err(|e| {
                Error::storage(format!("failed to delete artifact metadata file: {}", e))
            })?;
        }

        tracing::info!("deleted artifact {}", id);
        Ok(())
    }

    async fn get_metadata(&self, id: ArtifactId) -> Result<Artifact> {
        let meta_path = self.artifact_meta_path(id);

        let mut file = fs::File::open(&meta_path).await.map_err(|e| {
            Error::storage(format!("failed to open artifact metadata file: {}", e))
        })?;

        let mut contents = Vec::new();
        file.read_to_end(&mut contents).await.map_err(|e| {
            Error::storage(format!("failed to read artifact metadata: {}", e))
        })?;

        let artifact: Artifact = serde_json::from_slice(&contents).map_err(|e| {
            Error::storage(format!("failed to parse artifact metadata: {}", e))
        })?;

        Ok(artifact)
    }
}

#[async_trait]
impl CacheStore for FileStorage {
    async fn put(&self, key: CacheKey, data: Vec<u8>) -> Result<()> {
        let path = self.cache_path(&key);

        let mut file = fs::File::create(&path).await.map_err(|e| {
            Error::storage(format!("failed to create cache file: {}", e))
        })?;

        file.write_all(&data).await.map_err(|e| {
            Error::storage(format!("failed to write cache data: {}", e))
        })?;

        tracing::debug!("cached {} bytes at {:?}", data.len(), path);
        Ok(())
    }

    async fn get(&self, key: &CacheKey) -> Result<Option<Vec<u8>>> {
        let path = self.cache_path(key);

        if !path.exists() {
            return Ok(None);
        }

        let mut file = fs::File::open(&path).await.map_err(|e| {
            Error::storage(format!("failed to open cache file: {}", e))
        })?;

        let mut data = Vec::new();
        file.read_to_end(&mut data).await.map_err(|e| {
            Error::storage(format!("failed to read cache data: {}", e))
        })?;

        Ok(Some(data))
    }

    async fn delete(&self, key: &CacheKey) -> Result<()> {
        let path = self.cache_path(key);

        if path.exists() {
            fs::remove_file(&path).await.map_err(|e| {
                Error::storage(format!("failed to delete cache file: {}", e))
            })?;
        }

        Ok(())
    }

    async fn list(&self) -> Result<Vec<CacheEntry>> {
        // For filesystem, we'd need to scan the cache directory
        // This is a simplified implementation
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_file_storage_artifact() {
        let dir = tempdir().unwrap();
        let storage = FileStorage::new(dir.path()).await.unwrap();

        let artifact = Artifact {
            id: ArtifactId::new(),
            job_id: gitforce_common::JobId::new(),
            name: "test-artifact".to_string(),
            path: "/fake/path".to_string(),
            checksum: "abc123".to_string(),
            size_bytes: 1024,
            content_type: None,
            created_at: chrono::Utc::now(),
        };

        // Put using explicit trait method call to disambiguate
        ArtifactStore::put(&storage, &artifact, b"hello world").await.unwrap();

        // Get using explicit trait method call to disambiguate
        let data = ArtifactStore::get(&storage, artifact.id).await.unwrap();
        assert_eq!(data, b"hello world");

        // Get metadata
        let meta = ArtifactStore::get_metadata(&storage, artifact.id).await.unwrap();
        assert_eq!(meta.name, "test-artifact");

        // Delete using explicit trait method call
        ArtifactStore::delete(&storage, artifact.id).await.unwrap();
        let data = ArtifactStore::get(&storage, artifact.id).await;
        assert!(data.is_err());
    }
}

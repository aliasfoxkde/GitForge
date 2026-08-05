//! Filesystem-based storage implementation

use crate::artifact::{Artifact, ArtifactId, ArtifactStore};
use crate::cache::{CacheEntry, CacheKey, CacheStore};
use async_trait::async_trait;
use gitforge_common::{Error, Result};
use std::path::PathBuf;
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Filesystem-based storage for artifacts and cache
pub struct FileStorage {
    #[allow(dead_code)]
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
        fs::create_dir_all(&artifacts_dir)
            .await
            .map_err(|e| Error::storage(format!("failed to create artifacts directory: {}", e)))?;

        fs::create_dir_all(&cache_dir)
            .await
            .map_err(|e| Error::storage(format!("failed to create cache directory: {}", e)))?;

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

    /// Get cache metadata path
    fn cache_meta_path(&self, key: &CacheKey) -> PathBuf {
        self.cache_dir.join(format!("{}.meta.json", key.hash()))
    }
}

#[async_trait]
impl ArtifactStore for FileStorage {
    async fn put(&self, artifact: &Artifact, data: &[u8]) -> Result<()> {
        let artifact_path = self.artifact_path(artifact.id);
        let meta_path = self.artifact_meta_path(artifact.id);

        // Write data
        let mut file = fs::File::create(&artifact_path)
            .await
            .map_err(|e| Error::storage(format!("failed to create artifact file: {}", e)))?;

        file.write_all(data)
            .await
            .map_err(|e| Error::storage(format!("failed to write artifact data: {}", e)))?;

        // Write metadata
        let meta_json = serde_json::to_string_pretty(artifact)
            .map_err(|e| Error::storage(format!("failed to serialize artifact metadata: {}", e)))?;

        let mut meta_file = fs::File::create(&meta_path).await.map_err(|e| {
            Error::storage(format!("failed to create artifact metadata file: {}", e))
        })?;

        meta_file
            .write_all(meta_json.as_bytes())
            .await
            .map_err(|e| Error::storage(format!("failed to write artifact metadata: {}", e)))?;

        tracing::info!(
            "stored artifact {} ({} bytes)",
            artifact.id,
            artifact.size_bytes
        );
        Ok(())
    }

    async fn get(&self, id: ArtifactId) -> Result<Vec<u8>> {
        let path = self.artifact_path(id);

        let mut file = fs::File::open(&path)
            .await
            .map_err(|e| Error::storage(format!("failed to open artifact file: {}", e)))?;

        let mut data = Vec::new();
        file.read_to_end(&mut data)
            .await
            .map_err(|e| Error::storage(format!("failed to read artifact data: {}", e)))?;

        Ok(data)
    }

    async fn delete(&self, id: ArtifactId) -> Result<()> {
        let artifact_path = self.artifact_path(id);
        let meta_path = self.artifact_meta_path(id);

        if artifact_path.exists() {
            fs::remove_file(&artifact_path)
                .await
                .map_err(|e| Error::storage(format!("failed to delete artifact file: {}", e)))?;
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

        let mut file = fs::File::open(&meta_path)
            .await
            .map_err(|e| Error::storage(format!("failed to open artifact metadata file: {}", e)))?;

        let mut contents = Vec::new();
        file.read_to_end(&mut contents)
            .await
            .map_err(|e| Error::storage(format!("failed to read artifact metadata: {}", e)))?;

        let artifact: Artifact = serde_json::from_slice(&contents)
            .map_err(|e| Error::storage(format!("failed to parse artifact metadata: {}", e)))?;

        Ok(artifact)
    }

    async fn list(&self) -> Result<Vec<Artifact>> {
        let mut artifacts = Vec::new();

        let mut entries = fs::read_dir(&self.artifacts_dir)
            .await
            .map_err(|e| Error::storage(format!("failed to read artifacts directory: {}", e)))?;

        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| Error::storage(format!("failed to read artifact entry: {}", e)))?
        {
            let path = entry.path();
            // Check if filename ends with .meta.json
            if let Some(filename) = path.file_name().and_then(|n| n.to_str()) {
                if filename.ends_with(".meta.json") {
                    let mut contents = Vec::new();
                    if let Ok(mut file) = fs::File::open(&path).await {
                        if file.read_to_end(&mut contents).await.is_ok() {
                            if let Ok(artifact) = serde_json::from_slice::<Artifact>(&contents) {
                                artifacts.push(artifact);
                            }
                        }
                    }
                }
            }
        }

        Ok(artifacts)
    }

    async fn list_by_job(&self, job_id: gitforge_common::JobId) -> Result<Vec<Artifact>> {
        let all_artifacts = ArtifactStore::list(self).await?;
        Ok(all_artifacts
            .into_iter()
            .filter(|a| a.job_id == job_id)
            .collect())
    }
}

#[async_trait]
impl CacheStore for FileStorage {
    async fn put(&self, key: CacheKey, data: Vec<u8>) -> Result<()> {
        let path = self.cache_path(&key);
        let meta_path = self.cache_meta_path(&key);

        let mut file = fs::File::create(&path)
            .await
            .map_err(|e| Error::storage(format!("failed to create cache file: {}", e)))?;

        file.write_all(&data)
            .await
            .map_err(|e| Error::storage(format!("failed to write cache data: {}", e)))?;

        // Write metadata
        let now = chrono::Utc::now();
        let entry = crate::cache::CacheEntry {
            key: key.clone(),
            size_bytes: data.len() as u64,
            created_at: now,
            accessed_at: now,
        };

        let meta_json = serde_json::to_string(&entry)
            .map_err(|e| Error::storage(format!("failed to serialize cache metadata: {}", e)))?;

        let mut meta_file = fs::File::create(&meta_path)
            .await
            .map_err(|e| Error::storage(format!("failed to create cache metadata file: {}", e)))?;

        meta_file
            .write_all(meta_json.as_bytes())
            .await
            .map_err(|e| Error::storage(format!("failed to write cache metadata: {}", e)))?;

        tracing::debug!("cached {} bytes at {:?}", data.len(), path);
        Ok(())
    }

    async fn get(&self, key: &CacheKey) -> Result<Option<Vec<u8>>> {
        let path = self.cache_path(key);

        if !path.exists() {
            return Ok(None);
        }

        let mut file = fs::File::open(&path)
            .await
            .map_err(|e| Error::storage(format!("failed to open cache file: {}", e)))?;

        let mut data = Vec::new();
        file.read_to_end(&mut data)
            .await
            .map_err(|e| Error::storage(format!("failed to read cache data: {}", e)))?;

        Ok(Some(data))
    }

    async fn delete(&self, key: &CacheKey) -> Result<()> {
        let path = self.cache_path(key);
        let meta_path = self.cache_meta_path(key);

        if path.exists() {
            fs::remove_file(&path)
                .await
                .map_err(|e| Error::storage(format!("failed to delete cache file: {}", e)))?;
        }

        if meta_path.exists() {
            fs::remove_file(&meta_path).await.map_err(|e| {
                Error::storage(format!("failed to delete cache metadata file: {}", e))
            })?;
        }

        Ok(())
    }

    async fn list(&self) -> Result<Vec<CacheEntry>> {
        let mut entries = Vec::new();

        let mut dir_entries = fs::read_dir(&self.cache_dir)
            .await
            .map_err(|e| Error::storage(format!("failed to read cache directory: {}", e)))?;

        while let Some(entry) = dir_entries
            .next_entry()
            .await
            .map_err(|e| Error::storage(format!("failed to read cache entry: {}", e)))?
        {
            let path = entry.path();
            if let Some(filename) = path.file_name().and_then(|n| n.to_str()) {
                if filename.ends_with(".meta.json") {
                    let mut contents = Vec::new();
                    if let Ok(mut file) = fs::File::open(&path).await {
                        if file.read_to_end(&mut contents).await.is_ok() {
                            if let Ok(cache_entry) = serde_json::from_slice::<CacheEntry>(&contents)
                            {
                                entries.push(cache_entry);
                            }
                        }
                    }
                }
            }
        }

        Ok(entries)
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
            job_id: gitforge_common::JobId::new(),
            name: "test-artifact".to_string(),
            path: "/fake/path".to_string(),
            checksum: "abc123".to_string(),
            size_bytes: 1024,
            content_type: None,
            created_at: chrono::Utc::now(),
        };

        // Put using explicit trait method call to disambiguate
        ArtifactStore::put(&storage, &artifact, b"hello world")
            .await
            .unwrap();

        // Get using explicit trait method call to disambiguate
        let data = ArtifactStore::get(&storage, artifact.id).await.unwrap();
        assert_eq!(data, b"hello world");

        // Get metadata
        let meta = ArtifactStore::get_metadata(&storage, artifact.id)
            .await
            .unwrap();
        assert_eq!(meta.name, "test-artifact");

        // Delete using explicit trait method call
        ArtifactStore::delete(&storage, artifact.id).await.unwrap();
        let data = ArtifactStore::get(&storage, artifact.id).await;
        assert!(data.is_err());
    }

    #[tokio::test]
    async fn test_cache_put_and_get() {
        let dir = tempdir().unwrap();
        let storage = FileStorage::new(dir.path()).await.unwrap();

        let repo_id = gitforge_common::RepoId::new();
        let key = CacheKey::new(repo_id, "test-key", "main");
        let data = b"cache content";

        CacheStore::put(&storage, key.clone(), data.to_vec())
            .await
            .unwrap();

        let retrieved = CacheStore::get(&storage, &key).await.unwrap();
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap(), data);
    }

    #[tokio::test]
    async fn test_cache_get_nonexistent() {
        let dir = tempdir().unwrap();
        let storage = FileStorage::new(dir.path()).await.unwrap();

        let repo_id = gitforge_common::RepoId::new();
        let key = CacheKey::new(repo_id, "nonexistent", "main");
        let result = CacheStore::get(&storage, &key).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_cache_delete() {
        let dir = tempdir().unwrap();
        let storage = FileStorage::new(dir.path()).await.unwrap();

        let repo_id = gitforge_common::RepoId::new();
        let key = CacheKey::new(repo_id, "delete-me", "main");
        CacheStore::put(&storage, key.clone(), b"data".to_vec())
            .await
            .unwrap();

        // Verify it exists
        let result = CacheStore::get(&storage, &key).await.unwrap();
        assert!(result.is_some());

        // Delete and verify gone
        CacheStore::delete(&storage, &key).await.unwrap();

        let result = CacheStore::get(&storage, &key).await.unwrap();
        assert!(result.is_none());

        // List should also not include deleted entry
        let entries = CacheStore::list(&storage).await.unwrap();
        assert!(entries.is_empty());
    }

    #[tokio::test]
    async fn test_cache_list() {
        let dir = tempdir().unwrap();
        let storage = FileStorage::new(dir.path()).await.unwrap();

        let repo_id = gitforge_common::RepoId::new();
        let key1 = CacheKey::new(repo_id, "key1", "main");
        let key2 = CacheKey::new(repo_id, "key2", "main");

        // Initially empty
        let entries = CacheStore::list(&storage).await.unwrap();
        assert!(entries.is_empty());

        // Add entries
        CacheStore::put(&storage, key1.clone(), b"data1".to_vec())
            .await
            .unwrap();
        CacheStore::put(&storage, key2.clone(), b"data2".to_vec())
            .await
            .unwrap();

        // Now list should return entries
        let entries = CacheStore::list(&storage).await.unwrap();
        assert_eq!(entries.len(), 2);
    }

    #[tokio::test]
    async fn test_artifact_delete_nonexistent() {
        let dir = tempdir().unwrap();
        let storage = FileStorage::new(dir.path()).await.unwrap();

        // Deleting nonexistent artifact should not error
        let artifact_id = ArtifactId::new();
        let result = ArtifactStore::delete(&storage, artifact_id).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_artifact_get_nonexistent() {
        let dir = tempdir().unwrap();
        let storage = FileStorage::new(dir.path()).await.unwrap();

        let artifact_id = ArtifactId::new();
        let result = ArtifactStore::get(&storage, artifact_id).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_multiple_artifacts() {
        let dir = tempdir().unwrap();
        let storage = FileStorage::new(dir.path()).await.unwrap();

        for i in 0..5 {
            let artifact = Artifact {
                id: ArtifactId::new(),
                job_id: gitforge_common::JobId::new(),
                name: format!("artifact-{}", i),
                path: "/fake/path".to_string(),
                checksum: format!("checksum{}", i),
                size_bytes: 1024 + i as u64,
                content_type: None,
                created_at: chrono::Utc::now(),
            };

            let data = format!("content{}", i);
            ArtifactStore::put(&storage, &artifact, data.as_bytes())
                .await
                .unwrap();

            let retrieved = ArtifactStore::get(&storage, artifact.id).await.unwrap();
            assert_eq!(retrieved, data.as_bytes());
        }
    }

    #[tokio::test]
    async fn test_artifact_metadata_after_put() {
        let dir = tempdir().unwrap();
        let storage = FileStorage::new(dir.path()).await.unwrap();

        let artifact = Artifact {
            id: ArtifactId::new(),
            job_id: gitforge_common::JobId::new(),
            name: "metadata-test".to_string(),
            path: "/fake/path".to_string(),
            checksum: "abc123".to_string(),
            size_bytes: 42,
            content_type: Some("application/octet-stream".to_string()),
            created_at: chrono::Utc::now(),
        };

        ArtifactStore::put(&storage, &artifact, b"test data")
            .await
            .unwrap();

        // Get metadata
        let meta = ArtifactStore::get_metadata(&storage, artifact.id)
            .await
            .unwrap();
        assert_eq!(meta.name, "metadata-test");
        assert_eq!(meta.checksum, "abc123");
        assert_eq!(meta.size_bytes, 42);
        assert_eq!(
            meta.content_type,
            Some("application/octet-stream".to_string())
        );
    }

    #[tokio::test]
    async fn test_cache_put_get_multiple() {
        let dir = tempdir().unwrap();
        let storage = FileStorage::new(dir.path()).await.unwrap();

        let repo_id = gitforge_common::RepoId::new();

        // Put multiple cache entries
        for i in 0..3 {
            let key = CacheKey::new(repo_id, &format!("key-{}", i), "main");
            let data = format!("value{}", i);
            CacheStore::put(&storage, key.clone(), data.into_bytes())
                .await
                .unwrap();
        }

        // Get them back
        for i in 0..3 {
            let key = CacheKey::new(repo_id, &format!("key-{}", i), "main");
            let retrieved = CacheStore::get(&storage, &key).await.unwrap();
            assert!(retrieved.is_some());
            assert_eq!(retrieved.unwrap(), format!("value{}", i).into_bytes());
        }
    }

    #[tokio::test]
    async fn test_artifact_exactly_at_limit() {
        let dir = tempdir().unwrap();
        let storage = FileStorage::new(dir.path()).await.unwrap();

        let artifact = Artifact {
            id: ArtifactId::new(),
            job_id: gitforge_common::JobId::new(),
            name: "size-test".to_string(),
            path: "/fake/path".to_string(),
            checksum: "xyz".to_string(),
            size_bytes: 0,
            content_type: None,
            created_at: chrono::Utc::now(),
        };

        // Empty data
        ArtifactStore::put(&storage, &artifact, b"").await.unwrap();
        let data = ArtifactStore::get(&storage, artifact.id).await.unwrap();
        assert_eq!(data, b"");
    }

    #[tokio::test]
    async fn test_artifact_with_special_characters_in_name() {
        let dir = tempdir().unwrap();
        let storage = FileStorage::new(dir.path()).await.unwrap();

        let artifact = Artifact {
            id: ArtifactId::new(),
            job_id: gitforge_common::JobId::new(),
            name: "test-artifact-with-dashes_and_underscores".to_string(),
            path: "/fake/path".to_string(),
            checksum: "special".to_string(),
            size_bytes: 100,
            content_type: None,
            created_at: chrono::Utc::now(),
        };

        ArtifactStore::put(&storage, &artifact, b"special content")
            .await
            .unwrap();
        let retrieved = ArtifactStore::get(&storage, artifact.id).await.unwrap();
        assert_eq!(retrieved, b"special content");
    }

    #[tokio::test]
    async fn test_cache_overwrite() {
        let dir = tempdir().unwrap();
        let storage = FileStorage::new(dir.path()).await.unwrap();

        let repo_id = gitforge_common::RepoId::new();
        let key = CacheKey::new(repo_id, "overwrite-test", "main");

        // Put first value
        CacheStore::put(&storage, key.clone(), b"first".to_vec())
            .await
            .unwrap();
        let first = CacheStore::get(&storage, &key).await.unwrap();
        assert_eq!(first.unwrap(), b"first");

        // Overwrite with second value
        CacheStore::put(&storage, key.clone(), b"second".to_vec())
            .await
            .unwrap();
        let second = CacheStore::get(&storage, &key).await.unwrap();
        assert_eq!(second.unwrap(), b"second");
    }

    #[tokio::test]
    async fn test_artifact_list() {
        let dir = tempdir().unwrap();
        let storage = FileStorage::new(dir.path()).await.unwrap();

        // List should be empty initially
        let artifacts = ArtifactStore::list(&storage).await.unwrap();
        assert!(artifacts.is_empty());

        // Add an artifact
        let artifact = Artifact {
            id: ArtifactId::new(),
            job_id: gitforge_common::JobId::new(),
            name: "list-test".to_string(),
            path: "/fake/path".to_string(),
            checksum: "abc123".to_string(),
            size_bytes: 100,
            content_type: None,
            created_at: chrono::Utc::now(),
        };
        ArtifactStore::put(&storage, &artifact, b"test data")
            .await
            .unwrap();

        // List should have one artifact
        let artifacts = ArtifactStore::list(&storage).await.unwrap();
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].name, "list-test");
    }

    #[tokio::test]
    async fn test_artifact_list_by_job() {
        let dir = tempdir().unwrap();
        let storage = FileStorage::new(dir.path()).await.unwrap();

        let job_id = gitforge_common::JobId::new();
        let other_job_id = gitforge_common::JobId::new();

        // Add artifact for job_id
        let artifact1 = Artifact {
            id: ArtifactId::new(),
            job_id,
            name: "job-artifact".to_string(),
            path: "/fake/path1".to_string(),
            checksum: "abc123".to_string(),
            size_bytes: 100,
            content_type: None,
            created_at: chrono::Utc::now(),
        };
        ArtifactStore::put(&storage, &artifact1, b"data1")
            .await
            .unwrap();

        // Add artifact for other_job_id
        let artifact2 = Artifact {
            id: ArtifactId::new(),
            job_id: other_job_id,
            name: "other-job-artifact".to_string(),
            path: "/fake/path2".to_string(),
            checksum: "def456".to_string(),
            size_bytes: 200,
            content_type: None,
            created_at: chrono::Utc::now(),
        };
        ArtifactStore::put(&storage, &artifact2, b"data2")
            .await
            .unwrap();

        // List by job_id should return only artifact1
        let artifacts = ArtifactStore::list_by_job(&storage, job_id).await.unwrap();
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].name, "job-artifact");

        // List by other_job_id should return only artifact2
        let artifacts = ArtifactStore::list_by_job(&storage, other_job_id)
            .await
            .unwrap();
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].name, "other-job-artifact");
    }

    #[tokio::test]
    async fn test_artifact_list_by_job_none() {
        let dir = tempdir().unwrap();
        let storage = FileStorage::new(dir.path()).await.unwrap();

        let job_id = gitforge_common::JobId::new();

        // List by job_id with no artifacts should return empty
        let artifacts = ArtifactStore::list_by_job(&storage, job_id).await.unwrap();
        assert!(artifacts.is_empty());
    }

    #[tokio::test]
    async fn test_artifact_list_multiple() {
        let dir = tempdir().unwrap();
        let storage = FileStorage::new(dir.path()).await.unwrap();

        let job_id = gitforge_common::JobId::new();

        // Add multiple artifacts for same job
        for i in 0..5 {
            let artifact = Artifact {
                id: ArtifactId::new(),
                job_id,
                name: format!("artifact-{}", i),
                path: format!("/fake/path{}", i),
                checksum: format!("checksum{}", i),
                size_bytes: 100 + i as u64,
                content_type: None,
                created_at: chrono::Utc::now(),
            };
            ArtifactStore::put(&storage, &artifact, format!("data{}", i).as_bytes())
                .await
                .unwrap();
        }

        let artifacts = ArtifactStore::list_by_job(&storage, job_id).await.unwrap();
        assert_eq!(artifacts.len(), 5);
    }

    #[tokio::test]
    async fn test_artifact_list_all_jobs() {
        let dir = tempdir().unwrap();
        let storage = FileStorage::new(dir.path()).await.unwrap();

        let job1 = gitforge_common::JobId::new();
        let job2 = gitforge_common::JobId::new();

        // Add artifacts for different jobs
        for (i, job_id) in [job1, job1, job2].iter().enumerate() {
            let artifact = Artifact {
                id: ArtifactId::new(),
                job_id: *job_id,
                name: format!("artifact-{}", i),
                path: format!("/fake/path{}", i),
                checksum: format!("checksum{}", i),
                size_bytes: 100,
                content_type: None,
                created_at: chrono::Utc::now(),
            };
            ArtifactStore::put(&storage, &artifact, format!("data{}", i).as_bytes())
                .await
                .unwrap();
        }

        // list() should return all artifacts
        let all = ArtifactStore::list(&storage).await.unwrap();
        assert_eq!(all.len(), 3);

        // list_by_job should filter correctly
        let job1_artifacts = ArtifactStore::list_by_job(&storage, job1).await.unwrap();
        assert_eq!(job1_artifacts.len(), 2);
    }

    #[tokio::test]
    async fn test_artifact_list_empty_directory() {
        let dir = tempdir().unwrap();
        let storage = FileStorage::new(dir.path()).await.unwrap();

        // Empty artifacts directory
        let artifacts = ArtifactStore::list(&storage).await.unwrap();
        assert!(artifacts.is_empty());
    }

    #[tokio::test]
    async fn test_artifact_overwrite() {
        let dir = tempdir().unwrap();
        let storage = FileStorage::new(dir.path()).await.unwrap();

        let artifact_id = ArtifactId::new();
        let job_id = gitforge_common::JobId::new();

        let artifact = Artifact {
            id: artifact_id,
            job_id,
            name: "overwrite-test".to_string(),
            path: "/fake/path".to_string(),
            checksum: "original".to_string(),
            size_bytes: 100,
            content_type: None,
            created_at: chrono::Utc::now(),
        };

        // First write
        ArtifactStore::put(&storage, &artifact, b"original data")
            .await
            .unwrap();
        let data1 = ArtifactStore::get(&storage, artifact_id).await.unwrap();
        assert_eq!(data1, b"original data");

        // Overwrite with same ID
        let artifact2 = Artifact {
            id: artifact_id,
            job_id,
            name: "overwrite-test".to_string(),
            path: "/fake/path".to_string(),
            checksum: "updated".to_string(),
            size_bytes: 200,
            content_type: None,
            created_at: chrono::Utc::now(),
        };
        ArtifactStore::put(&storage, &artifact2, b"updated data")
            .await
            .unwrap();
        let data2 = ArtifactStore::get(&storage, artifact_id).await.unwrap();
        assert_eq!(data2, b"updated data");
    }
}

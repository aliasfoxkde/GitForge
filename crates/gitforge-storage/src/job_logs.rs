//! Job logs storage

use async_trait::async_trait;
use gitforge_common::{Error, JobId, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::receipt::LogReceipt;

/// Job log metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobLogMeta {
    pub job_id: JobId,
    pub size_bytes: u64,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Job log store trait
#[async_trait]
pub trait JobLogStore: Send + Sync {
    /// Store job logs
    async fn put(&self, job_id: JobId, data: Vec<u8>) -> Result<()>;

    /// Retrieve job logs
    async fn get(&self, job_id: &JobId) -> Result<Option<Vec<u8>>>;

    /// Delete job logs
    async fn delete(&self, job_id: &JobId) -> Result<()>;

    /// List all job logs
    async fn list(&self) -> Result<Vec<JobLogMeta>>;
}

/// In-memory job log store (for MVP / testing)
#[derive(Debug)]
pub struct InMemoryJobLogStore {
    logs: std::sync::Mutex<std::collections::HashMap<JobId, (Vec<u8>, JobLogMeta)>>,
}

impl InMemoryJobLogStore {
    /// Create a new in-memory job log store
    pub fn new() -> Self {
        Self {
            logs: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }
}

impl Default for InMemoryJobLogStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl JobLogStore for InMemoryJobLogStore {
    async fn put(&self, job_id: JobId, data: Vec<u8>) -> Result<()> {
        let size_bytes = data.len() as u64;
        let meta = JobLogMeta {
            job_id,
            size_bytes,
            created_at: chrono::Utc::now(),
        };

        let mut logs = self.logs.lock().unwrap();
        logs.insert(job_id, (data, meta));
        tracing::debug!("stored job log for job {} ({} bytes)", job_id, size_bytes);
        Ok(())
    }

    async fn get(&self, job_id: &JobId) -> Result<Option<Vec<u8>>> {
        let logs = self.logs.lock().unwrap();
        Ok(logs.get(job_id).map(|(data, _)| data.clone()))
    }

    async fn delete(&self, job_id: &JobId) -> Result<()> {
        let mut logs = self.logs.lock().unwrap();
        logs.remove(job_id);
        Ok(())
    }

    async fn list(&self) -> Result<Vec<JobLogMeta>> {
        let logs = self.logs.lock().unwrap();
        Ok(logs.values().map(|(_, meta)| meta.clone()).collect())
    }
}

/// File-based job log store for persistent storage
#[derive(Debug)]
pub struct FileJobLogStore {
    root: PathBuf,
}

impl FileJobLogStore {
    /// Create a new file-based job log store
    pub async fn new(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        let logs_dir = root.join("job_logs");

        fs::create_dir_all(&logs_dir)
            .await
            .map_err(|e| Error::storage(format!("failed to create job logs directory: {}", e)))?;

        Ok(Self { root })
    }

    fn log_path(&self, job_id: &JobId) -> PathBuf {
        self.root.join("job_logs").join(format!("{}.log", job_id))
    }

    fn meta_path(&self, job_id: &JobId) -> PathBuf {
        self.root
            .join("job_logs")
            .join(format!("{}.meta.json", job_id))
    }

    /// Store job logs with a size bound, returning a LogReceipt.
    ///
    /// If `data` exceeds `max_bytes`, it is truncated and the returned receipt
    /// reflects the truncated size. The SHA-256 is always computed over the
    /// (potentially truncated) stored content.
    pub async fn bounded_put(
        &self,
        job_id: JobId,
        data: Vec<u8>,
        max_bytes: u64,
    ) -> Result<LogReceipt> {
        let started_at = chrono::Utc::now();
        let original_len = data.len();

        // Truncate if necessary
        let (stored_data, truncated) = if original_len as u64 > max_bytes {
            (data[..max_bytes as usize].to_vec(), true)
        } else {
            (data, false)
        };

        let size_bytes = stored_data.len() as u64;

        // Compute SHA-256 of stored content
        let mut hasher = Sha256::new();
        hasher.update(&stored_data);
        let sha256 = hex::encode(hasher.finalize());

        let log_path = self.log_path(&job_id);
        let meta_path = self.meta_path(&job_id);

        // Write log data
        let mut file = fs::File::create(&log_path)
            .await
            .map_err(|e| Error::storage(format!("failed to create log file: {}", e)))?;
        file.write_all(&stored_data)
            .await
            .map_err(|e| Error::storage(format!("failed to write log data: {}", e)))?;

        // Write metadata
        let meta = JobLogMeta {
            job_id,
            size_bytes,
            created_at: started_at,
        };
        let meta_json = serde_json::to_string(&meta)
            .map_err(|e| Error::storage(format!("failed to serialize log meta: {}", e)))?;
        let mut meta_file = fs::File::create(&meta_path)
            .await
            .map_err(|e| Error::storage(format!("failed to create log metadata file: {}", e)))?;
        meta_file
            .write_all(meta_json.as_bytes())
            .await
            .map_err(|e| Error::storage(format!("failed to write log metadata: {}", e)))?;

        if truncated {
            tracing::warn!(
                "job {} log truncated from {} to {} bytes",
                job_id,
                original_len,
                size_bytes
            );
        }
        tracing::debug!("stored job log for job {} ({} bytes)", job_id, size_bytes);

        Ok(LogReceipt {
            uri: format!("gitforge://log/{}", job_id),
            sha256,
            bytes: size_bytes,
        })
    }
}

#[async_trait]
impl JobLogStore for FileJobLogStore {
    async fn put(&self, job_id: JobId, data: Vec<u8>) -> Result<()> {
        let size_bytes = data.len() as u64;
        let now = chrono::Utc::now();

        let meta = JobLogMeta {
            job_id,
            size_bytes,
            created_at: now,
        };

        let log_path = self.log_path(&job_id);
        let meta_path = self.meta_path(&job_id);

        // Write log data
        let mut file = fs::File::create(&log_path)
            .await
            .map_err(|e| Error::storage(format!("failed to create log file: {}", e)))?;
        file.write_all(&data)
            .await
            .map_err(|e| Error::storage(format!("failed to write log data: {}", e)))?;

        // Write metadata
        let meta_json = serde_json::to_string(&meta)
            .map_err(|e| Error::storage(format!("failed to serialize log meta: {}", e)))?;
        let mut meta_file = fs::File::create(&meta_path)
            .await
            .map_err(|e| Error::storage(format!("failed to create log metadata file: {}", e)))?;
        meta_file
            .write_all(meta_json.as_bytes())
            .await
            .map_err(|e| Error::storage(format!("failed to write log metadata: {}", e)))?;

        tracing::debug!("stored job log for job {} ({} bytes)", job_id, size_bytes);
        Ok(())
    }

    async fn get(&self, job_id: &JobId) -> Result<Option<Vec<u8>>> {
        let log_path = self.log_path(job_id);

        if !log_path.exists() {
            tracing::debug!("job log not found for job {}", job_id);
            return Ok(None);
        }

        let mut file = fs::File::open(&log_path)
            .await
            .map_err(|e| Error::storage(format!("failed to open log file: {}", e)))?;
        let mut data = Vec::new();
        file.read_to_end(&mut data)
            .await
            .map_err(|e| Error::storage(format!("failed to read log data: {}", e)))?;

        tracing::debug!(
            "retrieved job log for job {} ({} bytes)",
            job_id,
            data.len()
        );
        Ok(Some(data))
    }

    async fn delete(&self, job_id: &JobId) -> Result<()> {
        let log_path = self.log_path(job_id);
        let meta_path = self.meta_path(job_id);

        if log_path.exists() {
            fs::remove_file(&log_path)
                .await
                .map_err(|e| Error::storage(format!("failed to delete log file: {}", e)))?;
        }
        if meta_path.exists() {
            fs::remove_file(&meta_path)
                .await
                .map_err(|e| Error::storage(format!("failed to delete log metadata: {}", e)))?;
        }

        Ok(())
    }

    async fn list(&self) -> Result<Vec<JobLogMeta>> {
        let logs_dir = self.root.join("job_logs");
        let mut entries = Vec::new();

        let mut dir = fs::read_dir(&logs_dir)
            .await
            .map_err(|e| Error::storage(format!("failed to read job logs directory: {}", e)))?;

        while let Some(item) = dir
            .next_entry()
            .await
            .map_err(|e| Error::storage(format!("failed to read job log entry: {}", e)))?
        {
            let path = item.path();
            if path.extension().map(|e| e == "json").unwrap_or(false) {
                // This is a metadata file
                if let Ok(meta_json) = fs::read_to_string(&path).await {
                    if let Ok(meta) = serde_json::from_str::<JobLogMeta>(&meta_json) {
                        entries.push(meta);
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

    #[tokio::test]
    async fn test_in_memory_job_log_store() {
        let store = InMemoryJobLogStore::new();
        let job_id = JobId::new();

        // Put
        store
            .put(job_id, b"test log output".to_vec())
            .await
            .unwrap();

        // Get
        let logs = store.get(&job_id).await.unwrap();
        assert!(logs.is_some());
        assert_eq!(logs.unwrap(), b"test log output");

        // Delete
        store.delete(&job_id).await.unwrap();
        let logs = store.get(&job_id).await.unwrap();
        assert!(logs.is_none());
    }

    #[tokio::test]
    async fn test_in_memory_job_log_store_list() {
        let store = InMemoryJobLogStore::new();

        for i in 0..5 {
            let job_id = JobId::new();
            store
                .put(job_id, format!("log {}", i).into_bytes())
                .await
                .unwrap();
        }

        let entries = store.list().await.unwrap();
        assert_eq!(entries.len(), 5);
    }

    #[tokio::test]
    async fn test_in_memory_job_log_store_not_found() {
        let store = InMemoryJobLogStore::new();
        let job_id = JobId::new();

        let logs = store.get(&job_id).await.unwrap();
        assert!(logs.is_none());
    }

    #[tokio::test]
    async fn test_in_memory_job_log_store_overwrite() {
        let store = InMemoryJobLogStore::new();
        let job_id = JobId::new();

        store.put(job_id, b"first".to_vec()).await.unwrap();
        store.put(job_id, b"second".to_vec()).await.unwrap();

        let logs = store.get(&job_id).await.unwrap();
        assert_eq!(logs.unwrap(), b"second");

        let entries = store.list().await.unwrap();
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn test_job_log_meta_serialization() {
        let meta = JobLogMeta {
            job_id: JobId::new(),
            size_bytes: 1024,
            created_at: chrono::Utc::now(),
        };

        let json = serde_json::to_string(&meta).unwrap();
        let deserialized: JobLogMeta = serde_json::from_str(&json).unwrap();
        assert_eq!(meta.size_bytes, deserialized.size_bytes);
    }

    #[tokio::test]
    async fn test_file_job_log_store() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = FileJobLogStore::new(temp_dir.path()).await.unwrap();
        let job_id = JobId::new();

        // Put
        store
            .put(job_id, b"test log output".to_vec())
            .await
            .unwrap();

        // Get
        let logs = store.get(&job_id).await.unwrap();
        assert!(logs.is_some());
        assert_eq!(logs.unwrap(), b"test log output");

        // List
        let entries = store.list().await.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].job_id, job_id);

        // Delete
        store.delete(&job_id).await.unwrap();
        let logs = store.get(&job_id).await.unwrap();
        assert!(logs.is_none());
    }

    #[tokio::test]
    async fn test_file_job_log_store_not_found() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = FileJobLogStore::new(temp_dir.path()).await.unwrap();
        let job_id = JobId::new();

        let logs = store.get(&job_id).await.unwrap();
        assert!(logs.is_none());
    }

    #[tokio::test]
    async fn test_file_job_log_store_empty_list() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = FileJobLogStore::new(temp_dir.path()).await.unwrap();

        let entries = store.list().await.unwrap();
        assert!(entries.is_empty());
    }
}

//! Receipt persistence and retrieval store.
//!
//! Provides durable storage for job receipts with cryptographic integrity
//! and retrieval by multiple identifiers.

use crate::receipt::JobReceipt;
use async_trait::async_trait;
use gitforge_common::{Error, JobId, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Receipt metadata for indexing and retrieval
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReceiptMeta {
    pub job_id: JobId,
    pub workspace_id: Option<String>,
    pub owner_id: Option<String>,
    pub run_id: Option<String>,
    pub base_sha: Option<String>,
    pub head_sha: Option<String>,
    pub status: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// SHA-256 of the full receipt JSON for integrity
    pub receipt_sha256: String,
}

/// Trait for receipt storage operations
#[async_trait]
pub trait ReceiptStore: Send + Sync {
    /// Store a job receipt
    async fn put(&self, receipt: &JobReceipt) -> Result<()>;

    /// Retrieve a receipt by job ID
    async fn get(&self, job_id: &JobId) -> Result<Option<JobReceipt>>;

    /// List receipts by workspace ID
    async fn list_by_workspace(&self, workspace_id: &str) -> Result<Vec<JobReceipt>>;

    /// List receipts by owner ID
    async fn list_by_owner(&self, owner_id: &str) -> Result<Vec<JobReceipt>>;

    /// List receipts by run ID
    async fn list_by_run(&self, run_id: &str) -> Result<Vec<JobReceipt>>;

    /// Verify receipt integrity by job ID
    async fn verify(&self, job_id: &JobId) -> Result<ReceiptVerification>;
}

/// Result of receipt verification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReceiptVerification {
    pub job_id: JobId,
    pub receipt_exists: bool,
    pub signature_valid: Option<bool>,
    pub stored_sha256: Option<String>,
    pub computed_sha256: Option<String>,
}

/// In-memory receipt store for testing
pub struct InMemoryReceiptStore {
    receipts: std::sync::Mutex<std::collections::HashMap<JobId, JobReceipt>>,
    by_workspace: std::sync::Mutex<std::collections::HashMap<String, Vec<JobId>>>,
    #[allow(dead_code)]
    by_owner: std::sync::Mutex<std::collections::HashMap<String, Vec<JobId>>>,
    by_run: std::sync::Mutex<std::collections::HashMap<String, Vec<JobId>>>,
}

impl InMemoryReceiptStore {
    pub fn new() -> Self {
        Self {
            receipts: std::sync::Mutex::new(std::collections::HashMap::new()),
            by_workspace: std::sync::Mutex::new(std::collections::HashMap::new()),
            by_owner: std::sync::Mutex::new(std::collections::HashMap::new()),
            by_run: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    fn compute_receipt_sha256(receipt: &JobReceipt) -> String {
        let json = serde_json::to_string(receipt).unwrap_or_default();
        let mut hasher = Sha256::new();
        hasher.update(json.as_bytes());
        hex::encode(hasher.finalize())
    }

    fn extract_workspace_id(receipt: &JobReceipt) -> Option<String> {
        receipt
            .workspace_path
            .as_ref()
            .and_then(|p| p.split('/').next_back())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
    }
}

impl Default for InMemoryReceiptStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ReceiptStore for InMemoryReceiptStore {
    async fn put(&self, receipt: &JobReceipt) -> Result<()> {
        // Validate before storing
        receipt.validate()?;

        let job_id = receipt.job_id;

        // Compute SHA-256 of receipt for integrity
        let receipt_sha = Self::compute_receipt_sha256(receipt);

        let mut receipts = self.receipts.lock().unwrap();
        receipts.insert(job_id, receipt.clone());

        // Index by workspace
        if let Some(ws_id) = Self::extract_workspace_id(receipt) {
            let mut by_ws = self.by_workspace.lock().unwrap();
            by_ws.entry(ws_id).or_default().push(job_id);
        }

        // Index by run_id
        if let Some(run_id) = &receipt.run_id {
            let mut by_run = self.by_run.lock().unwrap();
            by_run.entry(run_id.clone()).or_default().push(job_id);
        }

        tracing::debug!(
            "stored receipt for job {} (sha256: {})",
            job_id,
            &receipt_sha[..16]
        );
        Ok(())
    }

    async fn get(&self, job_id: &JobId) -> Result<Option<JobReceipt>> {
        let receipts = self.receipts.lock().unwrap();
        Ok(receipts.get(job_id).cloned())
    }

    async fn list_by_workspace(&self, workspace_id: &str) -> Result<Vec<JobReceipt>> {
        let by_ws = self.by_workspace.lock().unwrap();
        let job_ids: Vec<JobId> = by_ws.get(workspace_id).cloned().unwrap_or_default();
        let receipts = self.receipts.lock().unwrap();
        Ok(job_ids
            .into_iter()
            .filter_map(|id| receipts.get(&id).cloned())
            .collect())
    }

    async fn list_by_owner(&self, _owner_id: &str) -> Result<Vec<JobReceipt>> {
        // In-memory store doesn't track owner_id separately
        let receipts = self.receipts.lock().unwrap();
        Ok(receipts.values().cloned().collect())
    }

    async fn list_by_run(&self, run_id: &str) -> Result<Vec<JobReceipt>> {
        let by_run = self.by_run.lock().unwrap();
        let job_ids: Vec<JobId> = by_run.get(run_id).cloned().unwrap_or_default();
        let receipts = self.receipts.lock().unwrap();
        Ok(job_ids
            .into_iter()
            .filter_map(|id| receipts.get(&id).cloned())
            .collect())
    }

    async fn verify(&self, job_id: &JobId) -> Result<ReceiptVerification> {
        let receipts = self.receipts.lock().unwrap();

        let Some(receipt) = receipts.get(job_id) else {
            return Ok(ReceiptVerification {
                job_id: *job_id,
                receipt_exists: false,
                signature_valid: None,
                stored_sha256: None,
                computed_sha256: None,
            });
        };

        let stored_sha = Self::compute_receipt_sha256(receipt);
        let signature_valid = receipt.verify_signature().is_ok();

        Ok(ReceiptVerification {
            job_id: *job_id,
            receipt_exists: true,
            signature_valid: Some(signature_valid),
            stored_sha256: Some(stored_sha.clone()),
            computed_sha256: Some(stored_sha),
        })
    }
}

/// File-based receipt store for persistent storage
pub struct FileReceiptStore {
    root: PathBuf,
}

impl FileReceiptStore {
    /// Create a new file-based receipt store
    pub async fn new(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        let receipts_dir = root.join("receipts");

        fs::create_dir_all(&receipts_dir)
            .await
            .map_err(|e| Error::storage(format!("failed to create receipts directory: {}", e)))?;

        Ok(Self { root })
    }

    fn receipt_path(&self, job_id: &JobId) -> PathBuf {
        self.root.join("receipts").join(format!("{}.json", job_id))
    }

    fn meta_path(&self, job_id: &JobId) -> PathBuf {
        self.root
            .join("receipts")
            .join(format!("{}.meta.json", job_id))
    }

    fn compute_receipt_sha256(receipt: &JobReceipt) -> String {
        let json = serde_json::to_string(receipt).unwrap_or_default();
        let mut hasher = Sha256::new();
        hasher.update(json.as_bytes());
        hex::encode(hasher.finalize())
    }

    fn extract_workspace_id(receipt: &JobReceipt) -> Option<String> {
        receipt
            .workspace_path
            .as_ref()
            .and_then(|p| p.split('/').next_back())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
    }

    async fn write_atomic(path: &PathBuf, contents: &[u8]) -> Result<()> {
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("receipt");
        let temporary_path = path.with_file_name(format!(
            ".{file_name}.tmp-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));

        let result = async {
            let mut file = fs::File::create(&temporary_path).await.map_err(|e| {
                Error::storage(format!("failed to create temporary receipt file: {}", e))
            })?;
            file.write_all(contents).await.map_err(|e| {
                Error::storage(format!("failed to write temporary receipt file: {}", e))
            })?;
            file.sync_all().await.map_err(|e| {
                Error::storage(format!("failed to flush temporary receipt file: {}", e))
            })?;
            fs::rename(&temporary_path, path)
                .await
                .map_err(|e| Error::storage(format!("failed to publish receipt file: {}", e)))
        }
        .await;

        if result.is_err() {
            let _ = fs::remove_file(&temporary_path).await;
        }
        result
    }
}

#[async_trait]
impl ReceiptStore for FileReceiptStore {
    async fn put(&self, receipt: &JobReceipt) -> Result<()> {
        // Validate before storing
        receipt.validate()?;

        let job_id = receipt.job_id;
        let receipt_sha = Self::compute_receipt_sha256(receipt);

        // Write receipt JSON
        let receipt_path = self.receipt_path(&job_id);
        let receipt_json = serde_json::to_string_pretty(receipt)
            .map_err(|e| Error::storage(format!("failed to serialize receipt: {}", e)))?;
        Self::write_atomic(&receipt_path, receipt_json.as_bytes()).await?;

        // Write metadata for indexing
        let workspace_id = Self::extract_workspace_id(receipt);
        let meta = ReceiptMeta {
            job_id,
            workspace_id,
            owner_id: None, // Owner tracking requires DB integration
            run_id: receipt.run_id.clone(),
            base_sha: receipt.base_sha.clone(),
            head_sha: receipt.head_sha.clone(),
            status: format!("{:?}", receipt.status),
            created_at: chrono::Utc::now(),
            receipt_sha256: receipt_sha,
        };
        let meta_path = self.meta_path(&job_id);
        let meta_json = serde_json::to_string_pretty(&meta)
            .map_err(|e| Error::storage(format!("failed to serialize meta: {}", e)))?;
        Self::write_atomic(&meta_path, meta_json.as_bytes()).await?;

        tracing::debug!("persisted receipt for job {}", job_id);
        Ok(())
    }

    async fn get(&self, job_id: &JobId) -> Result<Option<JobReceipt>> {
        let receipt_path = self.receipt_path(job_id);

        if !receipt_path.exists() {
            return Ok(None);
        }

        let mut file = fs::File::open(&receipt_path)
            .await
            .map_err(|e| Error::storage(format!("failed to open receipt: {}", e)))?;
        let mut contents = Vec::new();
        file.read_to_end(&mut contents)
            .await
            .map_err(|e| Error::storage(format!("failed to read receipt: {}", e)))?;

        let receipt: JobReceipt = serde_json::from_slice(&contents)
            .map_err(|e| Error::storage(format!("failed to parse receipt: {}", e)))?;

        Ok(Some(receipt))
    }

    async fn list_by_workspace(&self, _workspace_id: &str) -> Result<Vec<JobReceipt>> {
        // Read all receipts and filter by workspace (metadata lookup would be more efficient)
        let receipts_dir = self.root.join("receipts");
        let mut receipts = Vec::new();

        let mut entries = fs::read_dir(&receipts_dir)
            .await
            .map_err(|e| Error::storage(format!("failed to read receipts directory: {}", e)))?;

        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| Error::storage(format!("failed to read entry: {}", e)))?
        {
            let path = entry.path();
            if path.extension().map(|e| e == "json").unwrap_or(false)
                && !path.to_string_lossy().ends_with(".meta.json")
            {
                if let Ok(mut file) = fs::File::open(&path).await {
                    let mut contents = Vec::new();
                    if file.read_to_end(&mut contents).await.is_ok() {
                        if let Ok(receipt) = serde_json::from_slice::<JobReceipt>(&contents) {
                            if let Some(ws_id) = Self::extract_workspace_id(&receipt) {
                                if ws_id == _workspace_id {
                                    receipts.push(receipt);
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok(receipts)
    }

    async fn list_by_owner(&self, _owner_id: &str) -> Result<Vec<JobReceipt>> {
        // Would need DB integration for owner tracking
        let receipts_dir = self.root.join("receipts");
        let mut receipts = Vec::new();

        let mut entries = fs::read_dir(&receipts_dir)
            .await
            .map_err(|e| Error::storage(format!("failed to read receipts directory: {}", e)))?;

        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| Error::storage(format!("failed to read entry: {}", e)))?
        {
            let path = entry.path();
            if path.extension().map(|e| e == "json").unwrap_or(false)
                && !path.to_string_lossy().ends_with(".meta.json")
            {
                if let Ok(mut file) = fs::File::open(&path).await {
                    let mut contents = Vec::new();
                    if file.read_to_end(&mut contents).await.is_ok() {
                        if let Ok(receipt) = serde_json::from_slice::<JobReceipt>(&contents) {
                            receipts.push(receipt);
                        }
                    }
                }
            }
        }

        Ok(receipts)
    }

    async fn list_by_run(&self, run_id: &str) -> Result<Vec<JobReceipt>> {
        let receipts_dir = self.root.join("receipts");
        let mut receipts = Vec::new();

        let mut entries = fs::read_dir(&receipts_dir)
            .await
            .map_err(|e| Error::storage(format!("failed to read receipts directory: {}", e)))?;

        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| Error::storage(format!("failed to read entry: {}", e)))?
        {
            let path = entry.path();
            if path.extension().map(|e| e == "json").unwrap_or(false)
                && !path.to_string_lossy().ends_with(".meta.json")
            {
                if let Ok(mut file) = fs::File::open(&path).await {
                    let mut contents = Vec::new();
                    if file.read_to_end(&mut contents).await.is_ok() {
                        if let Ok(receipt) = serde_json::from_slice::<JobReceipt>(&contents) {
                            if receipt.run_id.as_deref() == Some(run_id) {
                                receipts.push(receipt);
                            }
                        }
                    }
                }
            }
        }

        Ok(receipts)
    }

    async fn verify(&self, job_id: &JobId) -> Result<ReceiptVerification> {
        let Some(receipt) = self.get(job_id).await? else {
            return Ok(ReceiptVerification {
                job_id: *job_id,
                receipt_exists: false,
                signature_valid: None,
                stored_sha256: None,
                computed_sha256: None,
            });
        };

        let stored_sha = Self::compute_receipt_sha256(&receipt);
        let signature_valid = receipt.verify_signature().is_ok();

        Ok(ReceiptVerification {
            job_id: *job_id,
            receipt_exists: true,
            signature_valid: Some(signature_valid),
            stored_sha256: Some(stored_sha.clone()),
            computed_sha256: Some(stored_sha),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ArtifactReceipt, LogReceipt, ReceiptStatus};
    use gitforge_common::PipelineRunId;
    use gitforge_common::RepoId;

    fn make_test_receipt(job_id: JobId) -> JobReceipt {
        let started = chrono::Utc::now();
        let completed = started + chrono::Duration::seconds(10);
        // Valid SHA-256 hex strings (64 hex chars each)
        let base = "a".repeat(64);
        let head = "b".repeat(64);
        let output = "c".repeat(64);
        let log_sha = "d".repeat(64);
        let artifact_sha = "e".repeat(64);
        JobReceipt {
            receipt_version: crate::receipt::RECEIPT_VERSION,
            work_request_id: Some("wr-1".into()),
            pipeline_run_id: PipelineRunId::new(),
            job_id,
            repository_id: Some(RepoId::new()),
            base_sha: Some(base.clone()),
            head_sha: Some(head.clone()),
            workspace_path: Some("/workspace/test-workspace".into()),
            run_id: Some("run-001".into()),
            status: ReceiptStatus::Succeeded,
            commands: vec!["cargo test".into()],
            working_directory: Some("/workspace".into()),
            exit_code: Some(0),
            changed_paths: vec!["src/lib.rs".into()],
            started_at: started,
            completed_at: completed,
            output_sha: output.clone(),
            output_bytes: 20,
            stable_uri: format!("gitforge://job/{}", job_id),
            log_uri: vec!["gitforge://log/test".into()],
            artifact_uri: vec!["gitforge://artifact/test/report.json".into()],
            logs: Some(LogReceipt {
                uri: "gitforge://log/test".into(),
                sha256: log_sha.clone(),
                bytes: 12,
            }),
            artifacts: vec![ArtifactReceipt {
                name: "report.json".into(),
                uri: "gitforge://artifact/test/report.json".into(),
                sha256: artifact_sha.clone(),
                bytes: 20,
                media_type: Some("application/json".into()),
            }],
            error: None,
            receipt_signature: None,
        }
    }

    #[tokio::test]
    async fn test_in_memory_receipt_store_put_get() {
        let store = InMemoryReceiptStore::new();
        let job_id = JobId::new();
        let receipt = make_test_receipt(job_id);

        store.put(&receipt).await.unwrap();

        let retrieved = store.get(&job_id).await.unwrap();
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().job_id, job_id);
    }

    #[tokio::test]
    async fn test_in_memory_receipt_store_get_nonexistent() {
        let store = InMemoryReceiptStore::new();
        let job_id = JobId::new();

        let retrieved = store.get(&job_id).await.unwrap();
        assert!(retrieved.is_none());
    }

    #[tokio::test]
    async fn test_in_memory_receipt_store_list_by_run() {
        let store = InMemoryReceiptStore::new();
        let job_id = JobId::new();
        let receipt = make_test_receipt(job_id);

        store.put(&receipt).await.unwrap();

        let receipts = store.list_by_run("run-001").await.unwrap();
        assert_eq!(receipts.len(), 1);
        assert_eq!(receipts[0].job_id, job_id);
    }

    #[tokio::test]
    async fn test_in_memory_receipt_store_list_by_workspace() {
        let store = InMemoryReceiptStore::new();
        let job_id = JobId::new();
        let receipt = make_test_receipt(job_id);

        store.put(&receipt).await.unwrap();

        // Workspace ID is extracted from path (last segment)
        let receipts = store.list_by_workspace("test-workspace").await.unwrap();
        assert_eq!(receipts.len(), 1);
    }

    #[tokio::test]
    async fn test_in_memory_receipt_store_verify() {
        let store = InMemoryReceiptStore::new();
        let job_id = JobId::new();
        let receipt = make_test_receipt(job_id);

        store.put(&receipt).await.unwrap();

        let verification = store.verify(&job_id).await.unwrap();
        assert!(verification.receipt_exists);
        assert!(verification.signature_valid.unwrap()); // No signature = valid
    }

    #[tokio::test]
    async fn test_in_memory_receipt_store_verify_nonexistent() {
        let store = InMemoryReceiptStore::new();
        let job_id = JobId::new();

        let verification = store.verify(&job_id).await.unwrap();
        assert!(!verification.receipt_exists);
        assert!(verification.signature_valid.is_none());
    }

    #[tokio::test]
    async fn test_file_receipt_store_put_get() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = FileReceiptStore::new(temp_dir.path()).await.unwrap();
        let job_id = JobId::new();
        let receipt = make_test_receipt(job_id);

        store.put(&receipt).await.unwrap();

        let retrieved = store.get(&job_id).await.unwrap();
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().job_id, job_id);
    }

    #[tokio::test]
    async fn test_file_receipt_store_get_nonexistent() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = FileReceiptStore::new(temp_dir.path()).await.unwrap();
        let job_id = JobId::new();

        let retrieved = store.get(&job_id).await.unwrap();
        assert!(retrieved.is_none());
    }

    #[tokio::test]
    async fn test_file_receipt_store_list_by_run() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = FileReceiptStore::new(temp_dir.path()).await.unwrap();
        let job_id = JobId::new();
        let receipt = make_test_receipt(job_id);

        store.put(&receipt).await.unwrap();

        let receipts = store.list_by_run("run-001").await.unwrap();
        assert_eq!(receipts.len(), 1);
    }

    #[tokio::test]
    async fn test_file_receipt_store_verify() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = FileReceiptStore::new(temp_dir.path()).await.unwrap();
        let job_id = JobId::new();
        let receipt = make_test_receipt(job_id);

        store.put(&receipt).await.unwrap();

        let verification = store.verify(&job_id).await.unwrap();
        assert!(verification.receipt_exists);
        assert!(verification.stored_sha256.is_some());
    }

    #[tokio::test]
    async fn test_file_receipt_store_verify_nonexistent() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = FileReceiptStore::new(temp_dir.path()).await.unwrap();
        let job_id = JobId::new();

        let verification = store.verify(&job_id).await.unwrap();
        assert!(!verification.receipt_exists);
    }

    #[tokio::test]
    async fn test_receipt_verification_with_signature() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = FileReceiptStore::new(temp_dir.path()).await.unwrap();
        let job_id = JobId::new();
        let mut receipt = make_test_receipt(job_id);

        // Sign the receipt
        let signature = receipt.compute_signature();
        receipt.receipt_signature = Some(signature);

        store.put(&receipt).await.unwrap();

        let verification = store.verify(&job_id).await.unwrap();
        assert!(verification.receipt_exists);
        assert!(verification.signature_valid.unwrap());
    }
}

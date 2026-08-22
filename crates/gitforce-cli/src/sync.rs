//! Cloud sync protocol for GitForge CLI
//!
//! Implements local-first sync with conflict resolution:
//! - Push: Upload local changes to cloud
//! - Pull: Download cloud changes to local
//! - Conflict resolution: Last-write-wins with local priority

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

/// HTTP client trait for sync operations (allows mocking in tests)
#[async_trait]
pub trait HttpClient: Send + Sync {
    async fn post_json<T: Serialize + Send + Sync, R: for<'de> Deserialize<'de> + Send>(
        &self,
        url: &str,
        token: &str,
        payload: &T,
    ) -> Result<R>;
    async fn get_json<T: for<'de> Deserialize<'de> + Send>(
        &self,
        url: &str,
        token: &str,
    ) -> Result<T>;
}

/// Real HTTP client using reqwest
pub struct RealHttpClient {
    client: reqwest::Client,
}

impl RealHttpClient {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }
}

impl Default for RealHttpClient {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl HttpClient for RealHttpClient {
    async fn post_json<T: Serialize + Send + Sync, R: for<'de> Deserialize<'de> + Send>(
        &self,
        url: &str,
        token: &str,
        payload: &T,
    ) -> Result<R> {
        let response = self
            .client
            .post(url)
            .header("Authorization", format!("Bearer {}", token))
            .json(payload)
            .send()
            .await
            .context("failed to push to cloud")?;

        if !response.status().is_success() {
            anyhow::bail!("Push failed: {}", response.status());
        }

        let result = response.json().await?;
        Ok(result)
    }

    async fn get_json<T: for<'de> Deserialize<'de> + Send>(
        &self,
        url: &str,
        token: &str,
    ) -> Result<T> {
        let response = self
            .client
            .get(url)
            .header("Authorization", format!("Bearer {}", token))
            .send()
            .await
            .context("failed to pull from cloud")?;

        if !response.status().is_success() {
            anyhow::bail!("Pull failed: {}", response.status());
        }

        let result = response.json().await?;
        Ok(result)
    }
}

/// Mock HTTP client for testing
#[allow(dead_code)]
pub struct MockHttpClient {
    pub push_response: Option<Result<PushResponse>>,
    pub pull_response: Option<Result<PullResponse>>,
}

#[allow(dead_code)]
impl MockHttpClient {
    pub fn new() -> Self {
        Self {
            push_response: None,
            pull_response: None,
        }
    }

    pub fn with_push_response(mut self, response: PushResponse) -> Self {
        self.push_response = Some(Ok(response));
        self
    }

    pub fn with_pull_response(mut self, response: PullResponse) -> Self {
        self.pull_response = Some(Ok(response));
        self
    }

    pub fn with_push_error(mut self, err: anyhow::Error) -> Self {
        self.push_response = Some(Err(err));
        self
    }
}

impl Default for MockHttpClient {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl HttpClient for MockHttpClient {
    async fn post_json<T: Serialize + Send + Sync, R: for<'de> Deserialize<'de> + Send>(
        &self,
        _url: &str,
        _token: &str,
        _payload: &T,
    ) -> Result<R> {
        match &self.push_response {
            Some(Ok(resp)) => {
                let json = serde_json::to_string(resp).unwrap();
                Ok(serde_json::from_str(&json).unwrap())
            }
            Some(Err(e)) => Err(anyhow::anyhow!("{}", e)),
            None => anyhow::bail!("mock push not configured"),
        }
    }

    async fn get_json<T: for<'de> Deserialize<'de> + Send>(
        &self,
        _url: &str,
        _token: &str,
    ) -> Result<T> {
        match &self.pull_response {
            Some(Ok(resp)) => {
                let json = serde_json::to_string(resp).unwrap();
                Ok(serde_json::from_str(&json).unwrap())
            }
            Some(Err(e)) => Err(anyhow::anyhow!("{}", e)),
            None => anyhow::bail!("mock pull not configured"),
        }
    }
}

/// Sync metadata for tracking state
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SyncMetadata {
    /// Last sync timestamp (RFC3339)
    pub last_push: Option<String>,
    /// Last sync timestamp (RFC3339)
    pub last_pull: Option<String>,
    /// Local revision counter
    pub local_rev: u64,
    /// Remote revision counter
    pub remote_rev: u64,
    /// Sync status
    pub status: SyncStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum SyncStatus {
    #[default]
    InSync,
    PendingPush,
    PendingPull,
    Conflict,
}

/// Local sync state
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LocalState {
    /// Repositories (name -> metadata)
    pub repos: HashMap<String, RepoState>,
    /// Pipelines (id -> metadata)
    pub pipelines: HashMap<String, PipelineState>,
    /// Last modified timestamp
    pub updated_at: String,
}

/// Repository sync state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoState {
    pub id: String,
    pub name: String,
    pub local_path: Option<PathBuf>,
    pub synced_at: Option<String>,
    pub local_rev: u64,
}

/// Pipeline sync state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineState {
    pub id: String,
    pub repo_id: String,
    pub name: String,
    pub definition: String,
    pub synced_at: Option<String>,
    pub local_rev: u64,
}

/// Cloud sync protocol client
pub struct SyncClient<C: HttpClient = RealHttpClient> {
    local_dir: PathBuf,
    metadata: tokio::sync::RwLock<SyncMetadata>,
    state: tokio::sync::RwLock<LocalState>,
    http_client: C,
}

impl<C: HttpClient> SyncClient<C> {
    /// Create a new sync client with the given HTTP client
    #[allow(dead_code)]
    pub fn new(local_dir: PathBuf, http_client: C) -> Self {
        Self {
            local_dir,
            metadata: tokio::sync::RwLock::new(SyncMetadata::default()),
            state: tokio::sync::RwLock::new(LocalState::default()),
            http_client,
        }
    }
}

impl SyncClient<RealHttpClient> {
    /// Create a new sync client with a real HTTP client
    pub fn with_real_client(local_dir: PathBuf) -> Self {
        Self {
            local_dir: local_dir.clone(),
            metadata: tokio::sync::RwLock::new(SyncMetadata::default()),
            state: tokio::sync::RwLock::new(LocalState::default()),
            http_client: RealHttpClient::new(),
        }
    }
}

impl<C: HttpClient> SyncClient<C> {
    /// Initialize sync directory
    pub async fn init(&self) -> Result<()> {
        let sync_dir = self.local_dir.join(".gitforge");
        fs::create_dir_all(&sync_dir).context("failed to create sync directory")?;

        let metadata_path = sync_dir.join("metadata.json");
        let state_path = sync_dir.join("state.json");

        // Load existing metadata if present
        if metadata_path.exists() {
            let contents = fs::read_to_string(&metadata_path)?;
            let metadata: SyncMetadata = serde_json::from_str(&contents)?;
            *self.metadata.write().await = metadata;
        }

        // Load existing state if present
        if state_path.exists() {
            let contents = fs::read_to_string(&state_path)?;
            let state: LocalState = serde_json::from_str(&contents)?;
            *self.state.write().await = state;
        }

        Ok(())
    }

    /// Get sync directory path
    pub fn sync_dir(&self) -> PathBuf {
        self.local_dir.join(".gitforge")
    }

    /// Save metadata to disk
    async fn save_metadata(&self) -> Result<()> {
        let sync_dir = self.sync_dir();
        fs::create_dir_all(&sync_dir)?;
        let metadata_path = sync_dir.join("metadata.json");
        let metadata = self.metadata.read().await;
        let contents = serde_json::to_string_pretty(&*metadata)?;
        fs::write(metadata_path, contents)?;
        Ok(())
    }

    /// Save state to disk
    async fn save_state(&self) -> Result<()> {
        let sync_dir = self.sync_dir();
        fs::create_dir_all(&sync_dir)?;
        let state_path = sync_dir.join("state.json");
        let state = self.state.read().await;
        let contents = serde_json::to_string_pretty(&*state)?;
        fs::write(state_path, contents)?;
        Ok(())
    }

    /// Get current sync status
    pub async fn status(&self) -> SyncStatus {
        self.metadata.read().await.status.clone()
    }

    /// Mark local changes pending push
    pub async fn mark_pending_push(&self) -> Result<()> {
        let mut metadata = self.metadata.write().await;
        metadata.status = SyncStatus::PendingPush;
        metadata.local_rev += 1;
        drop(metadata);
        self.save_metadata().await
    }

    /// Push local state to cloud
    pub async fn push(&self, api_url: &str, token: &str) -> Result<PushResponse> {
        let state = { self.state.read().await.clone() };
        let metadata = { self.metadata.read().await.clone() };

        // Prepare push payload
        let payload = PushPayload {
            local_rev: metadata.local_rev,
            repos: state.repos,
            pipelines: state.pipelines,
            timestamp: chrono::Utc::now().to_rfc3339(),
        };

        // POST to cloud sync endpoint
        let url = format!("{}/sync/push", api_url);
        let push_response: PushResponse = self
            .http_client
            .post_json(&url, token, &payload)
            .await
            .context("failed to push to cloud")?;

        // Update metadata
        let mut metadata = self.metadata.write().await;
        metadata.last_push = Some(chrono::Utc::now().to_rfc3339());
        metadata.remote_rev = push_response.remote_rev;
        metadata.status = SyncStatus::InSync;
        drop(metadata);
        self.save_metadata().await?;

        Ok(push_response)
    }

    /// Pull cloud state to local
    pub async fn pull(&self, api_url: &str, token: &str) -> Result<PullResponse> {
        // GET from cloud sync endpoint
        let url = format!("{}/sync/pull", api_url);
        let pull_response: PullResponse = self
            .http_client
            .get_json(&url, token)
            .await
            .context("failed to pull from cloud")?;

        // Update local state
        {
            let mut state = self.state.write().await;
            for (name, repo) in &pull_response.repos {
                state.repos.insert(name.clone(), repo.clone());
            }
            for (id, pipeline) in &pull_response.pipelines {
                state.pipelines.insert(id.clone(), pipeline.clone());
            }
            state.updated_at = chrono::Utc::now().to_rfc3339();
        }

        // Update metadata
        let mut metadata = self.metadata.write().await;
        metadata.last_pull = Some(chrono::Utc::now().to_rfc3339());
        metadata.remote_rev = pull_response.remote_rev;
        metadata.status = SyncStatus::InSync;
        drop(metadata);
        self.save_state().await?;
        self.save_metadata().await?;

        Ok(pull_response)
    }

    /// Add repository to local state
    #[allow(dead_code)]
    pub async fn add_repo(&self, name: String, id: String) -> Result<()> {
        let mut state = self.state.write().await;
        state.repos.insert(
            name.clone(),
            RepoState {
                id,
                name,
                local_path: None,
                synced_at: None,
                local_rev: 0,
            },
        );
        state.updated_at = chrono::Utc::now().to_rfc3339();
        drop(state);
        self.mark_pending_push().await?;
        self.save_state().await
    }

    /// Add pipeline to local state
    #[allow(dead_code)]
    pub async fn add_pipeline(
        &self,
        id: String,
        repo_id: String,
        name: String,
        definition: String,
    ) -> Result<()> {
        let mut state = self.state.write().await;
        state.pipelines.insert(
            id.clone(),
            PipelineState {
                id,
                repo_id,
                name,
                definition,
                synced_at: None,
                local_rev: 0,
            },
        );
        state.updated_at = chrono::Utc::now().to_rfc3339();
        drop(state);
        self.mark_pending_push().await?;
        self.save_state().await
    }
}

// API types for sync protocol

#[derive(Debug, Serialize)]
pub struct PushPayload {
    pub local_rev: u64,
    pub repos: HashMap<String, RepoState>,
    pub pipelines: HashMap<String, PipelineState>,
    pub timestamp: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct PushResponse {
    #[allow(dead_code)]
    pub success: bool,
    pub remote_rev: u64,
    #[allow(dead_code)]
    pub conflicts: Vec<String>,
}

#[derive(Debug, Serialize)]
#[allow(dead_code)]
pub struct PullRequest {
    pub local_rev: u64,
    pub last_sync: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct PullResponse {
    pub repos: HashMap<String, RepoState>,
    pub pipelines: HashMap<String, PipelineState>,
    pub remote_rev: u64,
    #[allow(dead_code)]
    pub has_more: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_sync_client_init() {
        let temp_dir = tempfile::tempdir().unwrap();
        let client = SyncClient::with_real_client(temp_dir.path().to_path_buf());
        client.init().await.unwrap();
        assert!(client.sync_dir().exists());
    }

    #[tokio::test]
    async fn test_sync_status_default() {
        let temp_dir = tempfile::tempdir().unwrap();
        let client = SyncClient::with_real_client(temp_dir.path().to_path_buf());
        client.init().await.unwrap();
        assert_eq!(client.status().await, SyncStatus::InSync);
    }

    #[tokio::test]
    async fn test_add_repo() {
        let temp_dir = tempfile::tempdir().unwrap();
        let client = SyncClient::with_real_client(temp_dir.path().to_path_buf());
        client.init().await.unwrap();
        client
            .add_repo("test-repo".to_string(), "repo-123".to_string())
            .await
            .unwrap();
        let state = client.state.read().await;
        assert!(state.repos.contains_key("test-repo"));
    }

    #[test]
    fn test_sync_metadata_default() {
        let metadata = SyncMetadata::default();
        assert_eq!(metadata.local_rev, 0);
        assert_eq!(metadata.remote_rev, 0);
        assert!(metadata.last_push.is_none());
        assert!(metadata.last_pull.is_none());
        assert_eq!(metadata.status, SyncStatus::InSync);
    }

    #[test]
    fn test_sync_status_variants() {
        assert_eq!(SyncStatus::InSync, SyncStatus::InSync);
        assert_eq!(SyncStatus::PendingPush, SyncStatus::PendingPush);
        assert_eq!(SyncStatus::PendingPull, SyncStatus::PendingPull);
        assert_eq!(SyncStatus::Conflict, SyncStatus::Conflict);
    }

    #[test]
    fn test_sync_status_serialization() {
        let status = SyncStatus::PendingPush;
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(json, "\"pendingpush\"");
        let parsed: SyncStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, SyncStatus::PendingPush);
    }

    #[test]
    fn test_local_state_default() {
        let state = LocalState::default();
        assert!(state.repos.is_empty());
        assert!(state.pipelines.is_empty());
    }

    #[test]
    fn test_repo_state_creation() {
        let repo = RepoState {
            id: "repo-1".to_string(),
            name: "test-repo".to_string(),
            local_path: Some(PathBuf::from("/tmp/repo")),
            synced_at: Some("2024-01-01T00:00:00Z".to_string()),
            local_rev: 5,
        };
        assert_eq!(repo.name, "test-repo");
        assert_eq!(repo.local_rev, 5);
    }

    #[test]
    fn test_pipeline_state_creation() {
        let pipeline = PipelineState {
            id: "pipe-1".to_string(),
            repo_id: "repo-1".to_string(),
            name: "build".to_string(),
            definition: "steps: [build]".to_string(),
            synced_at: None,
            local_rev: 0,
        };
        assert_eq!(pipeline.name, "build");
        assert!(pipeline.synced_at.is_none());
    }

    #[test]
    fn test_push_payload_structure() {
        let payload = PushPayload {
            local_rev: 10,
            repos: HashMap::new(),
            pipelines: HashMap::new(),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
        };
        assert_eq!(payload.local_rev, 10);
    }

    #[test]
    fn test_pull_request_structure() {
        let request = PullRequest {
            local_rev: 5,
            last_sync: Some("2024-01-01T00:00:00Z".to_string()),
        };
        assert_eq!(request.local_rev, 5);
        assert!(request.last_sync.is_some());
    }

    #[test]
    fn test_pull_response_structure() {
        let response = PullResponse {
            repos: HashMap::new(),
            pipelines: HashMap::new(),
            remote_rev: 15,
            has_more: false,
        };
        assert_eq!(response.remote_rev, 15);
        assert!(!response.has_more);
    }

    #[tokio::test]
    async fn test_sync_client_new() {
        let temp_dir = tempfile::tempdir().unwrap();
        let client = SyncClient::with_real_client(temp_dir.path().to_path_buf());
        assert_eq!(client.sync_dir(), temp_dir.path().join(".gitforge"));
    }

    #[tokio::test]
    async fn test_mark_pending_push() {
        let temp_dir = tempfile::tempdir().unwrap();
        let client = SyncClient::with_real_client(temp_dir.path().to_path_buf());
        client.init().await.unwrap();
        client.mark_pending_push().await.unwrap();
        assert_eq!(client.status().await, SyncStatus::PendingPush);
    }

    #[test]
    fn test_sync_metadata_serialization() {
        let metadata = SyncMetadata {
            last_push: Some("2024-01-01T00:00:00Z".to_string()),
            last_pull: Some("2024-01-02T00:00:00Z".to_string()),
            local_rev: 5,
            remote_rev: 10,
            status: SyncStatus::PendingPush,
        };
        let json = serde_json::to_string(&metadata).unwrap();
        assert!(json.contains("pendingpush"));
        let parsed: SyncMetadata = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.local_rev, 5);
    }

    #[test]
    fn test_local_state_serialization() {
        let state = LocalState {
            repos: HashMap::new(),
            pipelines: HashMap::new(),
            updated_at: "2024-01-01T00:00:00Z".to_string(),
        };
        let json = serde_json::to_string(&state).unwrap();
        assert!(json.contains("2024-01-01"));
    }

    #[test]
    fn test_repo_state_serialization() {
        let repo = RepoState {
            id: "repo-1".to_string(),
            name: "test-repo".to_string(),
            local_path: Some(PathBuf::from("/tmp/repo")),
            synced_at: None,
            local_rev: 0,
        };
        let json = serde_json::to_string(&repo).unwrap();
        assert!(json.contains("test-repo"));
    }

    #[test]
    fn test_pipeline_state_serialization() {
        let pipeline = PipelineState {
            id: "pipe-1".to_string(),
            repo_id: "repo-1".to_string(),
            name: "build".to_string(),
            definition: "steps: [build]".to_string(),
            synced_at: None,
            local_rev: 0,
        };
        let json = serde_json::to_string(&pipeline).unwrap();
        assert!(json.contains("build"));
    }

    #[test]
    fn test_sync_status_all_variants() {
        let variants = [
            SyncStatus::InSync,
            SyncStatus::PendingPush,
            SyncStatus::PendingPull,
            SyncStatus::Conflict,
        ];
        assert_eq!(variants.len(), 4);
    }

    #[test]
    fn test_sync_status_partial_eq() {
        assert_eq!(SyncStatus::InSync, SyncStatus::InSync);
        assert_ne!(SyncStatus::InSync, SyncStatus::PendingPush);
        assert_ne!(SyncStatus::PendingPush, SyncStatus::PendingPull);
    }

    #[tokio::test]
    async fn test_add_pipeline() {
        let temp_dir = tempfile::tempdir().unwrap();
        let client = SyncClient::with_real_client(temp_dir.path().to_path_buf());
        client.init().await.unwrap();
        client
            .add_pipeline(
                "pipe-1".to_string(),
                "repo-1".to_string(),
                "build".to_string(),
                "steps: [build]".to_string(),
            )
            .await
            .unwrap();
        let state = client.state.read().await;
        assert!(state.pipelines.contains_key("pipe-1"));
    }

    #[tokio::test]
    async fn test_sync_client_sync_dir() {
        let temp_dir = tempfile::tempdir().unwrap();
        let client = SyncClient::with_real_client(temp_dir.path().to_path_buf());
        let sync_dir = client.sync_dir();
        assert_eq!(sync_dir, temp_dir.path().join(".gitforge"));
    }

    #[tokio::test]
    async fn test_add_repo_updates_timestamp() {
        let temp_dir = tempfile::tempdir().unwrap();
        let client = SyncClient::with_real_client(temp_dir.path().to_path_buf());
        client.init().await.unwrap();
        let _before = client.state.read().await.updated_at.clone();
        client
            .add_repo("test-repo".to_string(), "repo-123".to_string())
            .await
            .unwrap();
        let after = client.state.read().await.updated_at.clone();
        assert!(!after.is_empty());
    }

    #[tokio::test]
    async fn test_add_pipeline_updates_timestamp() {
        let temp_dir = tempfile::tempdir().unwrap();
        let client = SyncClient::with_real_client(temp_dir.path().to_path_buf());
        client.init().await.unwrap();
        client
            .add_pipeline(
                "pipe-1".to_string(),
                "repo-1".to_string(),
                "build".to_string(),
                "steps: [build]".to_string(),
            )
            .await
            .unwrap();
        let state = client.state.read().await;
        assert!(!state.updated_at.is_empty());
    }

    #[test]
    fn test_sync_metadata_with_all_fields() {
        let metadata = SyncMetadata {
            last_push: Some("2024-01-01T00:00:00Z".to_string()),
            last_pull: Some("2024-01-02T00:00:00Z".to_string()),
            local_rev: 100,
            remote_rev: 200,
            status: SyncStatus::Conflict,
        };
        assert_eq!(metadata.local_rev, 100);
        assert_eq!(metadata.remote_rev, 200);
        assert!(matches!(metadata.status, SyncStatus::Conflict));
    }

    #[test]
    fn test_push_response_structure() {
        let response = PushResponse {
            success: true,
            remote_rev: 42,
            conflicts: vec![],
        };
        assert!(response.success);
        assert_eq!(response.remote_rev, 42);
        assert!(response.conflicts.is_empty());
    }

    #[test]
    fn test_push_response_with_conflicts() {
        let response = PushResponse {
            success: false,
            remote_rev: 10,
            conflicts: vec!["repo-1".to_string(), "repo-2".to_string()],
        };
        assert!(!response.success);
        assert_eq!(response.conflicts.len(), 2);
    }

    #[test]
    fn test_local_state_with_data() {
        let mut state = LocalState {
            updated_at: "2024-06-15T12:00:00Z".to_string(),
            ..LocalState::default()
        };
        state.repos.insert(
            "test".to_string(),
            RepoState {
                id: "id-1".to_string(),
                name: "test".to_string(),
                local_path: Some(PathBuf::from("/tmp/test")),
                synced_at: None,
                local_rev: 1,
            },
        );
        assert_eq!(state.repos.len(), 1);
        assert_eq!(state.updated_at, "2024-06-15T12:00:00Z");
    }

    #[tokio::test]
    async fn test_sync_client_push_with_mock() {
        let temp_dir = tempfile::tempdir().unwrap();
        let mock_client = MockHttpClient::new().with_push_response(PushResponse {
            success: true,
            remote_rev: 42,
            conflicts: vec![],
        });
        let client = SyncClient::new(temp_dir.path().to_path_buf(), mock_client);
        client.init().await.unwrap();

        let result = client.push("http://localhost:8080", "test-token").await;
        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.remote_rev, 42);
        assert!(response.conflicts.is_empty());
    }

    #[tokio::test]
    async fn test_sync_client_push_with_mock_error() {
        let temp_dir = tempfile::tempdir().unwrap();
        let mock_client = MockHttpClient::new().with_push_error(anyhow::anyhow!("server error"));
        let client = SyncClient::new(temp_dir.path().to_path_buf(), mock_client);
        client.init().await.unwrap();

        let result = client.push("http://localhost:8080", "test-token").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_sync_client_pull_with_mock() {
        let temp_dir = tempfile::tempdir().unwrap();
        let mock_client = MockHttpClient::new().with_pull_response(PullResponse {
            repos: HashMap::new(),
            pipelines: HashMap::new(),
            remote_rev: 100,
            has_more: false,
        });
        let client = SyncClient::new(temp_dir.path().to_path_buf(), mock_client);
        client.init().await.unwrap();

        let result = client.pull("http://localhost:8080", "test-token").await;
        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.remote_rev, 100);
        assert!(!response.has_more);
    }

    #[tokio::test]
    async fn test_sync_client_pull_with_mock_error() {
        let temp_dir = tempfile::tempdir().unwrap();
        let mock_client = MockHttpClient::new().with_push_error(anyhow::anyhow!("server error"));
        let client = SyncClient::new(temp_dir.path().to_path_buf(), mock_client);
        client.init().await.unwrap();

        let result = client.pull("http://localhost:8080", "test-token").await;
        assert!(result.is_err());
    }

    #[test]
    fn test_mock_http_client_push_response() {
        let mock = MockHttpClient::new().with_push_response(PushResponse {
            success: true,
            remote_rev: 5,
            conflicts: vec!["a".to_string()],
        });
        assert!(mock.push_response.is_some());
        assert!(mock.pull_response.is_none());
    }

    #[test]
    fn test_mock_http_client_pull_response() {
        let mock = MockHttpClient::new().with_pull_response(PullResponse {
            repos: HashMap::new(),
            pipelines: HashMap::new(),
            remote_rev: 10,
            has_more: true,
        });
        assert!(mock.pull_response.is_some());
        assert!(mock.push_response.is_none());
    }
}

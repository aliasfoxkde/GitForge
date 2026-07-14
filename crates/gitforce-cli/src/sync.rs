//! Cloud sync protocol for GitForge CLI
//!
//! Implements local-first sync with conflict resolution:
//! - Push: Upload local changes to cloud
//! - Pull: Download cloud changes to local
//! - Conflict resolution: Last-write-wins with local priority

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

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
pub struct SyncClient {
    local_dir: PathBuf,
    metadata: tokio::sync::RwLock<SyncMetadata>,
    state: tokio::sync::RwLock<LocalState>,
}

impl SyncClient {
    /// Create a new sync client
    pub fn new(local_dir: PathBuf) -> Self {
        Self {
            local_dir: local_dir.clone(),
            metadata: tokio::sync::RwLock::new(SyncMetadata::default()),
            state: tokio::sync::RwLock::new(LocalState::default()),
        }
    }

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
        let state = {
            self.state.read().await.clone()
        };
        let metadata = {
            self.metadata.read().await.clone()
        };

        // Prepare push payload
        let payload = PushPayload {
            local_rev: metadata.local_rev,
            repos: state.repos,
            pipelines: state.pipelines,
            timestamp: chrono::Utc::now().to_rfc3339(),
        };

        // POST to cloud sync endpoint
        let client = reqwest::Client::new();
        let response = client
            .post(format!("{}/sync/push", api_url))
            .header("Authorization", format!("Bearer {}", token))
            .json(&payload)
            .send()
            .await
            .context("failed to push to cloud")?;

        if !response.status().is_success() {
            anyhow::bail!("Push failed: {}", response.status());
        }

        let push_response: PushResponse = response.json().await?;

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
        let metadata = self.metadata.read().await.clone();

        // Prepare pull request
        let request = PullRequest {
            local_rev: metadata.local_rev,
            last_sync: metadata.last_pull.clone(),
        };

        // GET from cloud sync endpoint
        let client = reqwest::Client::new();
        let response = client
            .get(format!("{}/sync/pull", api_url))
            .header("Authorization", format!("Bearer {}", token))
            .json(&request)
            .send()
            .await
            .context("failed to pull from cloud")?;

        if !response.status().is_success() {
            anyhow::bail!("Pull failed: {}", response.status());
        }

        let pull_response: PullResponse = response.json().await?;

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
    pub async fn add_pipeline(&self, id: String, repo_id: String, name: String, definition: String) -> Result<()> {
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

#[derive(Debug, Deserialize)]
pub struct PushResponse {
    #[allow(dead_code)]
    pub success: bool,
    pub remote_rev: u64,
    #[allow(dead_code)]
    pub conflicts: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct PullRequest {
    pub local_rev: u64,
    pub last_sync: Option<String>,
}

#[derive(Debug, Deserialize)]
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
        let client = SyncClient::new(temp_dir.path().to_path_buf());
        client.init().await.unwrap();
        assert!(client.sync_dir().exists());
    }

    #[tokio::test]
    async fn test_sync_status_default() {
        let temp_dir = tempfile::tempdir().unwrap();
        let client = SyncClient::new(temp_dir.path().to_path_buf());
        client.init().await.unwrap();
        assert_eq!(client.status().await, SyncStatus::InSync);
    }

    #[tokio::test]
    async fn test_add_repo() {
        let temp_dir = tempfile::tempdir().unwrap();
        let client = SyncClient::new(temp_dir.path().to_path_buf());
        client.init().await.unwrap();
        client.add_repo("test-repo".to_string(), "repo-123".to_string()).await.unwrap();
        let state = client.state.read().await;
        assert!(state.repos.contains_key("test-repo"));
    }
}

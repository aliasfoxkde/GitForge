//! Repository management service

use crate::storage::StorageBackend;
use async_trait::async_trait;
use gitforce_common::{Error, RepoId, Result, UserId};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Git reference information
#[derive(Debug, Clone)]
pub struct GitRef {
    pub name: String,
    pub hash: String,
    pub is_branch: bool,
    pub is_tag: bool,
}

/// Repository service for managing git repositories
pub struct RepoService<S: StorageBackend> {
    storage: Arc<S>,
    /// In-memory repo metadata cache
    repos: Arc<RwLock<HashMap<RepoId, RepoMetadata>>>,
}

#[derive(Debug, Clone)]
pub struct RepoMetadata {
    pub id: RepoId,
    pub name: String,
    pub owner_id: UserId,
    pub git_path: String,
}

/// Errors for repository operations
#[derive(Debug, thiserror::Error)]
pub enum RepoError {
    #[error("Repository not found: {0}")]
    NotFound(RepoId),

    #[error("Repository already exists: {0}")]
    AlreadyExists(String),

    #[error("Invalid repository name: {0}")]
    InvalidName(String),

    #[error("Storage error: {0}")]
    Storage(String),
}

impl From<RepoError> for gitforce_common::Error {
    fn from(err: RepoError) -> Self {
        match err {
            RepoError::NotFound(id) => gitforce_common::Error::not_found("repository", id),
            RepoError::AlreadyExists(name) => gitforce_common::Error::already_exists("repository", name),
            RepoError::InvalidName(name) => gitforce_common::Error::invalid_input(format!("invalid repo name: {}", name)),
            RepoError::Storage(msg) => gitforce_common::Error::storage(msg),
        }
    }
}

impl<S: StorageBackend> RepoService<S> {
    /// Create a new repository service
    pub fn new(storage: S) -> Self {
        Self {
            storage: Arc::new(storage),
            repos: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Create a new repository
    pub async fn create(
        &self,
        name: String,
        owner_id: UserId,
    ) -> Result<RepoMetadata> {
        // Validate name
        if name.is_empty() || name.len() > 255 {
            return Err(RepoError::InvalidName(name).into());
        }

        if !name.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == '.') {
            return Err(RepoError::InvalidName(name).into());
        }

        let repo_id = RepoId::new();
        let git_path = self.storage.repo_path(repo_id);

        // Create storage
        self.storage.create(repo_id).await?;

        let metadata = RepoMetadata {
            id: repo_id,
            name: name.clone(),
            owner_id,
            git_path: git_path.to_string_lossy().to_string(),
        };

        // Cache metadata
        {
            let mut repos = self.repos.write().await;
            repos.insert(repo_id, metadata.clone());
        }

        tracing::info!("created repository {} for owner {}", name, owner_id);
        Ok(metadata)
    }

    /// Get repository metadata
    pub async fn get(&self, repo_id: RepoId) -> Result<Option<RepoMetadata>> {
        {
            let repos = self.repos.read().await;
            if let Some(meta) = repos.get(&repo_id) {
                return Ok(Some(meta.clone()));
            }
        }

        // Could load from database here if needed
        Ok(None)
    }

    /// Delete a repository
    pub async fn delete(&self, repo_id: RepoId) -> Result<()> {
        // Remove from cache
        {
            let mut repos = self.repos.write().await;
            repos.remove(&repo_id);
        }

        // Delete from storage
        self.storage.delete(repo_id).await?;

        tracing::info!("deleted repository {}", repo_id);
        Ok(())
    }

    /// List all repositories
    pub async fn list(&self) -> Vec<RepoMetadata> {
        let repos = self.repos.read().await;
        repos.values().cloned().collect()
    }

    /// List repositories by owner
    pub async fn list_by_owner(&self, owner_id: UserId) -> Vec<RepoMetadata> {
        let repos = self.repos.read().await;
        repos
            .values()
            .filter(|r| r.owner_id == owner_id)
            .cloned()
            .collect()
    }

    /// Get repository git path
    pub async fn get_git_path(&self, repo_id: RepoId) -> Result<String> {
        let meta = self.get(repo_id).await?;
        match meta {
            Some(m) => Ok(m.git_path),
            None => Err(RepoError::NotFound(repo_id).into()),
        }
    }

    /// List references (branches and tags) for a repository
    pub async fn list_refs(&self, repo_id: RepoId) -> Result<Vec<GitRef>> {
        let repo = self.storage.open(repo_id).await?;
        let mut refs = Vec::new();

        // Iterate over all references
        for name in repo.references().map_err(|e| Error::git(format!("failed to list refs: {}", e)))? {
            let name = name.map_err(|e| Error::git(format!("failed to read ref: {}", e)))?;
            let ref_name = name.name().unwrap_or("").to_string();
            let is_branch = ref_name.starts_with("refs/heads/");
            let is_tag = ref_name.starts_with("refs/tags/");

            if let Ok(hash) = name.peel_to_commit() {
                let hash_str = hash.id().to_string();

                refs.push(GitRef {
                    name: ref_name,
                    hash: hash_str,
                    is_branch,
                    is_tag,
                });
            }
        }

        Ok(refs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::FileStorageBackend;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_repo_service() {
        let dir = tempdir().unwrap();
        let storage = FileStorageBackend::new(dir.path());
        let service = RepoService::new(storage);

        let owner_id = UserId::new();

        // Create repository
        let meta = service.create("test-repo".to_string(), owner_id).await.unwrap();
        assert_eq!(meta.name, "test-repo");

        // Get repository
        let retrieved = service.get(meta.id).await.unwrap();
        assert!(retrieved.is_some());

        // List by owner
        let repos = service.list_by_owner(owner_id).await;
        assert_eq!(repos.len(), 1);

        // Delete repository
        service.delete(meta.id).await.unwrap();
        let repos = service.list_by_owner(owner_id).await;
        assert!(repos.is_empty());
    }
}

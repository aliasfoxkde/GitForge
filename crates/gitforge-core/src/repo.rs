//! Repository management service

use crate::storage::StorageBackend;
use gitforge_common::{Error, RepoId, Result, UserId};
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

impl From<RepoError> for gitforge_common::Error {
    fn from(err: RepoError) -> Self {
        match err {
            RepoError::NotFound(id) => gitforge_common::Error::not_found("repository", id),
            RepoError::AlreadyExists(name) => {
                gitforge_common::Error::already_exists("repository", name)
            }
            RepoError::InvalidName(name) => {
                gitforge_common::Error::invalid_input(format!("invalid repo name: {}", name))
            }
            RepoError::Storage(msg) => gitforge_common::Error::storage(msg),
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
    pub async fn create(&self, name: String, owner_id: UserId) -> Result<RepoMetadata> {
        // Validate name
        if name.is_empty() || name.len() > 255 {
            return Err(RepoError::InvalidName(name).into());
        }

        if !name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == '.')
        {
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
        for name in repo
            .references()
            .map_err(|e| Error::git(format!("failed to list refs: {}", e)))?
        {
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
        let meta = service
            .create("test-repo".to_string(), owner_id)
            .await
            .unwrap();
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

    #[test]
    fn test_git_ref_creation() {
        let git_ref = GitRef {
            name: "refs/heads/main".to_string(),
            hash: "abc123".to_string(),
            is_branch: true,
            is_tag: false,
        };
        assert_eq!(git_ref.name, "refs/heads/main");
        assert!(git_ref.is_branch);
        assert!(!git_ref.is_tag);
    }

    #[test]
    fn test_repo_metadata_creation() {
        let meta = RepoMetadata {
            id: RepoId::new(),
            name: "test-repo".to_string(),
            owner_id: UserId::new(),
            git_path: "/git/repos/test".to_string(),
        };
        assert_eq!(meta.name, "test-repo");
    }

    #[tokio::test]
    async fn test_repo_create_invalid_name() {
        let dir = tempdir().unwrap();
        let storage = FileStorageBackend::new(dir.path());
        let service = RepoService::new(storage);

        let owner_id = UserId::new();

        // Empty name should fail
        let result = service.create("".to_string(), owner_id).await;
        assert!(result.is_err());

        // Name with invalid chars should fail
        let result = service.create("invalid name!".to_string(), owner_id).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_repo_get_not_found() {
        let dir = tempdir().unwrap();
        let storage = FileStorageBackend::new(dir.path());
        let service = RepoService::new(storage);

        let result = service.get(RepoId::new()).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_repo_list() {
        let dir = tempdir().unwrap();
        let storage = FileStorageBackend::new(dir.path());
        let service = RepoService::new(storage);

        let owner_id = UserId::new();

        service.create("repo1".to_string(), owner_id).await.unwrap();
        service.create("repo2".to_string(), owner_id).await.unwrap();

        let repos = service.list().await;
        assert_eq!(repos.len(), 2);
    }

    #[tokio::test]
    async fn test_repo_git_path() {
        let dir = tempdir().unwrap();
        let storage = FileStorageBackend::new(dir.path());
        let service = RepoService::new(storage);

        let owner_id = UserId::new();

        let meta = service
            .create("test-repo".to_string(), owner_id)
            .await
            .unwrap();
        let path = service.get_git_path(meta.id).await.unwrap();
        assert!(!path.is_empty());

        // Non-existent repo should fail
        let result = service.get_git_path(RepoId::new()).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_repo_list_by_owner() {
        let dir = tempdir().unwrap();
        let storage = FileStorageBackend::new(dir.path());
        let service = RepoService::new(storage);

        let owner1 = UserId::new();
        let owner2 = UserId::new();

        service.create("repo1".to_string(), owner1).await.unwrap();
        service.create("repo2".to_string(), owner1).await.unwrap();
        service.create("repo3".to_string(), owner2).await.unwrap();

        let owner1_repos = service.list_by_owner(owner1).await;
        assert_eq!(owner1_repos.len(), 2);

        let owner2_repos = service.list_by_owner(owner2).await;
        assert_eq!(owner2_repos.len(), 1);
    }

    #[test]
    fn test_repo_error_display() {
        let repo_id = RepoId::new();
        let err = RepoError::NotFound(repo_id);
        assert!(format!("{}", err).contains("not found"));

        let err = RepoError::AlreadyExists("test".to_string());
        assert!(format!("{}", err).contains("already exists"));

        let err = RepoError::InvalidName("bad".to_string());
        assert!(format!("{}", err).contains("Invalid repository name"));

        let err = RepoError::Storage("disk error".to_string());
        assert!(format!("{}", err).contains("Storage error"));
    }

    #[tokio::test]
    async fn test_repo_list_refs_empty() {
        let dir = tempdir().unwrap();
        let storage = FileStorageBackend::new(dir.path());
        let service = RepoService::new(storage);

        let owner_id = UserId::new();
        let meta = service
            .create("test-repo".to_string(), owner_id)
            .await
            .unwrap();

        // Fresh repo has no refs
        let refs = service.list_refs(meta.id).await.unwrap();
        assert!(refs.is_empty());
    }

    #[tokio::test]
    async fn test_repo_git_ref_fields() {
        let git_ref = GitRef {
            name: "refs/tags/v1.0".to_string(),
            hash: "abc123".to_string(),
            is_branch: false,
            is_tag: true,
        };
        assert!(!git_ref.is_branch);
        assert!(git_ref.is_tag);
        assert_eq!(git_ref.name, "refs/tags/v1.0");
    }

    #[tokio::test]
    async fn test_repo_create_invalid_name_too_long() {
        let dir = tempdir().unwrap();
        let storage = FileStorageBackend::new(dir.path());
        let service = RepoService::new(storage);

        let owner_id = UserId::new();
        let long_name = "a".repeat(256);

        let result = service.create(long_name, owner_id).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_repo_create_invalid_chars() {
        let dir = tempdir().unwrap();
        let storage = FileStorageBackend::new(dir.path());
        let service = RepoService::new(storage);

        let owner_id = UserId::new();

        // Test various invalid characters
        for name in [
            "has!space",
            "has@at",
            "has#hash",
            "has$dollar",
            "has%percent",
        ] {
            let result = service.create(name.to_string(), owner_id).await;
            assert!(result.is_err(), "Expected '{}' to be invalid", name);
        }
    }

    #[tokio::test]
    async fn test_repo_create_valid_special_chars() {
        let dir = tempdir().unwrap();
        let storage = FileStorageBackend::new(dir.path());
        let service = RepoService::new(storage);

        let owner_id = UserId::new();

        // Valid names with dashes, underscores, dots
        for name in ["my-repo", "my_repo", "my.repo", "repo.1", "repo-2_name"] {
            let result = service.create(name.to_string(), owner_id).await;
            assert!(result.is_ok(), "Expected '{}' to be valid", name);
        }
    }

    #[tokio::test]
    async fn test_repo_delete_nonexistent() {
        let dir = tempdir().unwrap();
        let storage = FileStorageBackend::new(dir.path());
        let service = RepoService::new(storage);

        // Deleting non-existent repo should not panic
        let result = service.delete(RepoId::new()).await;
        assert!(result.is_ok()); // delete is idempotent
    }

    #[tokio::test]
    async fn test_repo_list_by_owner_empty() {
        let dir = tempdir().unwrap();
        let storage = FileStorageBackend::new(dir.path());
        let service = RepoService::new(storage);

        let repos = service.list_by_owner(UserId::new()).await;
        assert!(repos.is_empty());
    }

    #[test]
    fn test_git_ref_debug() {
        let git_ref = GitRef {
            name: "refs/heads/main".to_string(),
            hash: "abc123def456".to_string(),
            is_branch: true,
            is_tag: false,
        };
        let debug_str = format!("{:?}", git_ref);
        assert!(debug_str.contains("main"));
        assert!(debug_str.contains("abc123"));
    }

    #[test]
    fn test_repo_metadata_debug() {
        let meta = RepoMetadata {
            id: RepoId::new(),
            name: "debug-test".to_string(),
            owner_id: UserId::new(),
            git_path: "/tmp/git".to_string(),
        };
        let debug_str = format!("{:?}", meta);
        assert!(debug_str.contains("debug-test"));
    }

    #[test]
    fn test_repo_error_not_found_display() {
        let repo_id = RepoId::new();
        let err = RepoError::NotFound(repo_id);
        let display = format!("{}", err);
        assert!(display.contains("Repository not found"));
    }

    #[test]
    fn test_repo_error_already_exists_display() {
        let err = RepoError::AlreadyExists("my-repo".to_string());
        let display = format!("{}", err);
        assert!(display.contains("already exists"));
    }

    #[test]
    fn test_repo_error_invalid_name_display() {
        let err = RepoError::InvalidName("bad!name".to_string());
        let display = format!("{}", err);
        assert!(display.contains("Invalid repository name"));
    }

    #[test]
    fn test_repo_error_storage_display() {
        let err = RepoError::Storage("disk full".to_string());
        let display = format!("{}", err);
        assert!(display.contains("Storage error"));
    }
}

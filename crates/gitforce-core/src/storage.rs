//! Filesystem-based git storage backend

use async_trait::async_trait;
use gitforce_common::{Error, RepoId, Result};
use std::path::{Path, PathBuf};
use tokio::fs;

/// Storage backend trait for git repositories
#[async_trait]
pub trait StorageBackend: Send + Sync {
    /// Get the path for a repository
    fn repo_path(&self, repo_id: RepoId) -> PathBuf;

    /// Check if a repository exists
    async fn exists(&self, repo_id: RepoId) -> bool;

    /// Create a new bare repository
    async fn create(&self, repo_id: RepoId) -> Result<PathBuf>;

    /// Delete a repository
    async fn delete(&self, repo_id: RepoId) -> Result<()>;

    /// Initialize a bare git repository
    async fn init_bare(&self, path: &Path) -> Result<()>;

    /// Open an existing repository
    async fn open(&self, repo_id: RepoId) -> Result<git2::Repository>;
}

/// Filesystem-based storage implementation
#[derive(Clone)]
pub struct FileStorageBackend {
    root: PathBuf,
}

impl FileStorageBackend {
    /// Create a new filesystem storage backend
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Get the root directory
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Ensure the root directory exists
    pub async fn ensure_root(&self) -> Result<()> {
        tokio::fs::create_dir_all(&self.root).await.map_err(|e| {
            Error::storage(format!("failed to create storage root: {}", e))
        })?;
        Ok(())
    }
}

#[async_trait]
impl StorageBackend for FileStorageBackend {
    fn repo_path(&self, repo_id: RepoId) -> PathBuf {
        self.root.join(repo_id.to_string())
    }

    async fn exists(&self, repo_id: RepoId) -> bool {
        let path = self.repo_path(repo_id);
        fs::metadata(&path).await.is_ok()
    }

    async fn create(&self, repo_id: RepoId) -> Result<PathBuf> {
        let path = self.repo_path(repo_id);

        // Create repository directory
        tokio::fs::create_dir_all(&path).await.map_err(|e| {
            Error::storage(format!("failed to create repo directory: {}", e))
        })?;

        // Initialize bare git repository
        self.init_bare(&path).await?;

        tracing::info!("created repository at {:?}", path);
        Ok(path)
    }

    async fn delete(&self, repo_id: RepoId) -> Result<()> {
        let path = self.repo_path(repo_id);

        if path.exists() {
            tokio::fs::remove_dir_all(&path).await.map_err(|e| {
                Error::storage(format!("failed to delete repository: {}", e))
            })?;
            tracing::info!("deleted repository at {:?}", path);
        }

        Ok(())
    }

    async fn init_bare(&self, path: &Path) -> Result<()> {
        // Use git2 to create a bare repository
        let repo = git2::Repository::init_bare(path).map_err(|e| {
            Error::git(format!("failed to initialize bare repository: {}", e))
        })?;

        // Configure repository for optimal git server usage
        let mut config = repo.config().map_err(|e| {
            Error::git(format!("failed to get repository config: {}", e))
        })?;

        // Disable garbage collection for server-side repos
        config.set_bool("gc.autodetach", false).ok();
        config.set_bool("gc.auto", false).ok();

        tracing::debug!("initialized bare git repository at {:?}", path);
        Ok(())
    }

    async fn open(&self, repo_id: RepoId) -> Result<git2::Repository> {
        let path = self.repo_path(repo_id);

        let repo = git2::Repository::open(&path).map_err(|e| {
            Error::git(format!("failed to open repository at {:?}: {}", path, e))
        })?;

        Ok(repo)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_file_storage_backend() {
        let dir = tempdir().unwrap();
        let backend = FileStorageBackend::new(dir.path());

        let repo_id = RepoId::new();

        // Create repository
        let path = backend.create(repo_id).await.unwrap();
        assert!(path.exists());

        // Check exists
        assert!(backend.exists(repo_id).await);

        // Open repository
        let repo = backend.open(repo_id).await.unwrap();
        assert!(repo.is_bare());

        // Delete repository
        backend.delete(repo_id).await.unwrap();
        assert!(!backend.exists(repo_id).await);
    }
}

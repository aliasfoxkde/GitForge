//! Filesystem-based git storage backend

use async_trait::async_trait;
use gitforge_common::{Error, RepoId, Result};
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
        tokio::fs::create_dir_all(&self.root)
            .await
            .map_err(|e| Error::storage(format!("failed to create storage root: {}", e)))?;
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
        tokio::fs::create_dir_all(&path)
            .await
            .map_err(|e| Error::storage(format!("failed to create repo directory: {}", e)))?;

        // Initialize bare git repository
        self.init_bare(&path).await?;

        tracing::info!("created repository at {:?}", path);
        Ok(path)
    }

    async fn delete(&self, repo_id: RepoId) -> Result<()> {
        let path = self.repo_path(repo_id);

        if path.exists() {
            tokio::fs::remove_dir_all(&path)
                .await
                .map_err(|e| Error::storage(format!("failed to delete repository: {}", e)))?;
            tracing::info!("deleted repository at {:?}", path);
        }

        Ok(())
    }

    async fn init_bare(&self, path: &Path) -> Result<()> {
        // Use git2 to create a bare repository
        let repo = git2::Repository::init_bare(path)
            .map_err(|e| Error::git(format!("failed to initialize bare repository: {}", e)))?;

        // Configure repository for optimal git server usage
        let mut config = repo
            .config()
            .map_err(|e| Error::git(format!("failed to get repository config: {}", e)))?;

        // Disable garbage collection for server-side repos
        config.set_bool("gc.autodetach", false).ok();
        config.set_bool("gc.auto", false).ok();

        // Point HEAD at the default branch. init_bare leaves HEAD on an
        // unborn `master`, and clients that push `main` never rewrite it, so
        // clones of newly provisioned repositories could not check out.
        // Setting the symref target of an unborn HEAD is valid here.
        repo.set_head("refs/heads/main")
            .map_err(|e| Error::git(format!("failed to set default branch: {}", e)))?;

        tracing::debug!("initialized bare git repository at {:?}", path);
        Ok(())
    }

    async fn open(&self, repo_id: RepoId) -> Result<git2::Repository> {
        let path = self.repo_path(repo_id);

        let repo = git2::Repository::open(&path)
            .map_err(|e| Error::git(format!("failed to open repository at {:?}: {}", path, e)))?;

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

        // HEAD must point at the default branch so clones of a freshly
        // provisioned (still empty) repository can check out after a push.
        let head = std::fs::read(path.join("HEAD")).unwrap();
        assert_eq!(
            String::from_utf8_lossy(&head).trim(),
            "ref: refs/heads/main"
        );

        // Delete repository
        backend.delete(repo_id).await.unwrap();
        assert!(!backend.exists(repo_id).await);
    }

    #[tokio::test]
    async fn test_file_storage_backend_repo_path() {
        let dir = tempdir().unwrap();
        let backend = FileStorageBackend::new(dir.path());
        let repo_id = RepoId::new();

        let path = backend.repo_path(repo_id);
        assert!(path.to_str().unwrap().ends_with(&repo_id.to_string()));
    }

    #[tokio::test]
    async fn test_file_storage_backend_root() {
        let dir = tempdir().unwrap();
        let backend = FileStorageBackend::new(dir.path());
        assert_eq!(backend.root(), dir.path());
    }

    #[tokio::test]
    async fn test_file_storage_backend_ensure_root() {
        let dir = tempdir().unwrap();
        let backend = FileStorageBackend::new(dir.path().join("nonexistent"));
        backend.ensure_root().await.unwrap();
        assert!(dir.path().join("nonexistent").exists());
    }

    #[tokio::test]
    async fn test_file_storage_backend_delete_nonexistent() {
        let dir = tempdir().unwrap();
        let backend = FileStorageBackend::new(dir.path());
        let repo_id = RepoId::new();

        // Delete should not fail if repo doesn't exist
        backend.delete(repo_id).await.unwrap();
    }

    #[tokio::test]
    async fn test_file_storage_backend_exists_false() {
        let dir = tempdir().unwrap();
        let backend = FileStorageBackend::new(dir.path());
        let repo_id = RepoId::new();

        assert!(!backend.exists(repo_id).await);
    }

    #[tokio::test]
    async fn test_file_storage_open_nonexistent() {
        let dir = tempdir().unwrap();
        let backend = FileStorageBackend::new(dir.path());
        let repo_id = RepoId::new();

        let result = backend.open(repo_id).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_file_storage_backend_clone() {
        let dir = tempdir().unwrap();
        let backend1 = FileStorageBackend::new(dir.path());
        let backend2 = backend1.clone();

        let repo_id = RepoId::new();
        let path1 = backend1.repo_path(repo_id);
        let path2 = backend2.repo_path(repo_id);

        assert_eq!(path1, path2);
    }

    #[test]
    fn test_file_storage_backend_new() {
        let backend = FileStorageBackend::new("/tmp/test");
        assert_eq!(backend.root().to_str().unwrap(), "/tmp/test");
    }
}

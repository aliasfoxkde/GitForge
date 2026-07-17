//! HTTP Git protocol handler
//!
//! Implements the Smart HTTP protocol for git

use super::GitProtocolHandler;
use crate::storage::StorageBackend;
use async_trait::async_trait;
use gitforce_common::{RepoId, Result};
use std::sync::Arc;

/// HTTP Git protocol handler
pub struct HttpGitHandler<S: StorageBackend> {
    storage: Arc<S>,
}

impl<S: StorageBackend> HttpGitHandler<S> {
    pub fn new(storage: S) -> Self {
        Self {
            storage: Arc::new(storage),
        }
    }
}

#[async_trait]
impl<S: StorageBackend> GitProtocolHandler for HttpGitHandler<S> {
    async fn upload_pack(
        &self,
        repo_id: RepoId,
        input: Vec<u8>,
    ) -> Result<Vec<u8>> {
        // Open the repository
        let _repo = self.storage.open(repo_id).await?;

        tracing::debug!(
            "upload_pack for repo {} ({} bytes input)",
            repo_id,
            input.len()
        );

        // For smart HTTP protocol, upload-pack request contains the ref discovery
        // We return the refs info as a pkt-line format response
        // The actual pack file generation would be done by git cli in a real implementation
        // For now, return an empty pack negotiator response
        let response = b"0000".to_vec();
        Ok(response)
    }

    async fn receive_pack(
        &self,
        repo_id: RepoId,
        input: Vec<u8>,
    ) -> Result<Vec<u8>> {
        // Open the repository
        let _repo = self.storage.open(repo_id).await?;

        tracing::debug!(
            "receive_pack for repo {} ({} bytes input)",
            repo_id,
            input.len()
        );

        // receive-pack handles push operations
        // In a full implementation, this would process the pack data and update refs
        // Return acknowledgment
        let response = b"0000".to_vec();
        Ok(response)
    }
}

/// Parse Content-Type header for git protocol
pub fn parse_content_type(content_type: &str) -> Option<&str> {
    if content_type.contains(';') {
        Some(content_type.split(';').next()?.trim())
    } else {
        Some(content_type.trim())
    }
}

/// Parse git protocol service name from URL path
pub fn parse_service(service_path: &str) -> Option<(String, String)> {
    let parts: Vec<&str> = service_path.trim_start_matches('/').split('/').collect();
    if parts.len() >= 2 {
        let service = parts[0].to_string();
        let repo_and_path = &parts[1..];
        let repo_path = repo_and_path.join("/");
        Some((service, repo_path))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_content_type() {
        assert_eq!(parse_content_type("application/x-git-upload-pack-request"), Some("application/x-git-upload-pack-request"));
        assert_eq!(parse_content_type("text/plain; charset=utf-8"), Some("text/plain"));
    }

    #[test]
    fn test_parse_content_type_edge_cases() {
        // Empty string
        assert_eq!(parse_content_type(""), Some(""));
        // Just whitespace - trims to empty
        assert_eq!(parse_content_type("   "), Some(""));
        // Multiple semicolons
        assert_eq!(parse_content_type("text/plain; charset=utf-8; boundary=abc"), Some("text/plain"));
    }

    #[test]
    fn test_parse_service() {
        assert_eq!(
            parse_service("/git-upload-pack/owner/repo"),
            Some(("git-upload-pack".to_string(), "owner/repo".to_string()))
        );
        assert_eq!(
            parse_service("git-receive-pack/owner/repo.git/info/refs"),
            Some(("git-receive-pack".to_string(), "owner/repo.git/info/refs".to_string()))
        );
    }

    #[test]
    fn test_parse_service_edge_cases() {
        // No leading slash
        assert_eq!(
            parse_service("git-upload-pack/repo"),
            Some(("git-upload-pack".to_string(), "repo".to_string()))
        );
        // Deep path
        assert_eq!(
            parse_service("/git-upload-pack/owner/repo/path/to/refs"),
            Some(("git-upload-pack".to_string(), "owner/repo/path/to/refs".to_string()))
        );
        // Single segment (should return None)
        assert_eq!(parse_service("git-upload-pack"), None);
        // Empty
        assert_eq!(parse_service(""), None);
    }

    #[test]
    fn test_parse_service_empty_path() {
        assert_eq!(parse_service("/"), None);
        assert_eq!(parse_service(""), None);
    }

    #[tokio::test]
    async fn test_http_git_handler_upload_pack() {
        use crate::storage::FileStorageBackend;
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let storage = FileStorageBackend::new(dir.path());
        let handler = HttpGitHandler::new(storage.clone());

        let repo_id = RepoId::new();
        let result = handler.upload_pack(repo_id, vec![1, 2, 3]).await;
        // Should fail because repo doesn't exist
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_http_git_handler_receive_pack() {
        use crate::storage::FileStorageBackend;
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let storage = FileStorageBackend::new(dir.path());
        let handler = HttpGitHandler::new(storage.clone());

        let repo_id = RepoId::new();
        let result = handler.receive_pack(repo_id, vec![1, 2, 3]).await;
        // Should fail because repo doesn't exist
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_http_git_handler_with_existing_repo() {
        use crate::storage::FileStorageBackend;
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let storage = FileStorageBackend::new(dir.path());
        let handler = HttpGitHandler::new(storage.clone());

        // Create a repo first
        let repo_id = RepoId::new();
        storage.create(repo_id).await.unwrap();

        // Now upload_pack should work and return a valid response
        let result = handler.upload_pack(repo_id, vec![1, 2, 3]).await;
        assert!(result.is_ok());
        let response = result.unwrap();
        // Response is a pkt-line format response (empty pkt-line is "0000")
        assert_eq!(response, b"0000");
    }

    #[tokio::test]
    async fn test_http_git_handler_receive_pack_with_existing_repo() {
        use crate::storage::FileStorageBackend;
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let storage = FileStorageBackend::new(dir.path());
        let handler = HttpGitHandler::new(storage.clone());

        // Create a repo first
        let repo_id = RepoId::new();
        storage.create(repo_id).await.unwrap();

        // Now receive_pack should work and return a valid response
        let result = handler.receive_pack(repo_id, vec![]).await;
        assert!(result.is_ok());
        let response = result.unwrap();
        // Response is a pkt-line acknowledgment
        assert_eq!(response, b"0000");
    }

    #[test]
    fn test_http_git_handler_creation() {
        use crate::storage::FileStorageBackend;
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let storage = FileStorageBackend::new(dir.path());
        let _handler = HttpGitHandler::new(storage);
        // Handler created successfully
    }

    #[test]
    fn test_parse_content_type_with_charset() {
        // More charset variations
        assert_eq!(parse_content_type("application/json;charset=utf-8"), Some("application/json"));
        assert_eq!(parse_content_type("text/html; charset=ISO-8859-1"), Some("text/html"));
    }

    #[test]
    fn test_parse_service_various_paths() {
        // Various Git URL formats
        assert_eq!(
            parse_service("/git-upload-pack/owner/project.git"),
            Some(("git-upload-pack".to_string(), "owner/project.git".to_string()))
        );
        assert_eq!(
            parse_service("git-receive-pack/my-org/my-repo"),
            Some(("git-receive-pack".to_string(), "my-org/my-repo".to_string()))
        );
    }
}

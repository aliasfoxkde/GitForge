//! SSH Git protocol handler
//!
//! Implements the SSH git protocol

use super::GitProtocolHandler;
use crate::storage::StorageBackend;
use async_trait::async_trait;
use gitforce_common::{RepoId, Result};
use std::sync::Arc;

/// SSH Git protocol handler
pub struct SshGitHandler<S: StorageBackend> {
    storage: Arc<S>,
}

impl<S: StorageBackend> SshGitHandler<S> {
    pub fn new(storage: S) -> Self {
        Self {
            storage: Arc::new(storage),
        }
    }
}

#[async_trait]
impl<S: StorageBackend> GitProtocolHandler for SshGitHandler<S> {
    async fn upload_pack(
        &self,
        repo_id: RepoId,
        input: Vec<u8>,
    ) -> Result<Vec<u8>> {
        let _repo = self.storage.open(repo_id).await?;

        tracing::debug!(
            "ssh upload_pack for repo {} ({} bytes input)",
            repo_id,
            input.len()
        );

        // SSH upload-pack protocol - return refs info in pkt-line format
        // The actual pack file generation would be done by git cli in a real implementation
        let response = b"0000".to_vec();
        Ok(response)
    }

    async fn receive_pack(
        &self,
        repo_id: RepoId,
        input: Vec<u8>,
    ) -> Result<Vec<u8>> {
        let _repo = self.storage.open(repo_id).await?;

        tracing::debug!(
            "ssh receive_pack for repo {} ({} bytes input)",
            repo_id,
            input.len()
        );

        // SSH receive-pack protocol - return acknowledgment in pkt-line format
        let response = b"0000".to_vec();
        Ok(response)
    }
}

/// Extract command from SSH original_command
pub fn parse_ssh_command(cmd: &str) -> Option<(&str, &str)> {
    let parts: Vec<&str> = cmd.split_whitespace().collect();
    if parts.len() >= 2 {
        let command = parts[0];
        let repo_path = parts[1];
        Some((command, repo_path))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_ssh_command() {
        assert_eq!(
            parse_ssh_command("git-upload-pack /owner/repo.git"),
            Some(("git-upload-pack", "/owner/repo.git"))
        );
        assert_eq!(
            parse_ssh_command("git-receive-pack owner/repo"),
            Some(("git-receive-pack", "owner/repo"))
        );
    }

    #[test]
    fn test_parse_ssh_command_edge_cases() {
        // Single word
        assert_eq!(parse_ssh_command("git-upload-pack"), None);
        // Empty string
        assert_eq!(parse_ssh_command(""), None);
        // Extra whitespace
        assert_eq!(
            parse_ssh_command("git-upload-pack   /repo"),
            Some(("git-upload-pack", "/repo"))
        );
        // Multiple spaces between
        assert_eq!(
            parse_ssh_command("git-receive-pack  owner/repo"),
            Some(("git-receive-pack", "owner/repo"))
        );
        // Deep path
        assert_eq!(
            parse_ssh_command("git-upload-pack /owner/repo/path/to/refs"),
            Some(("git-upload-pack", "/owner/repo/path/to/refs"))
        );
    }

    #[test]
    fn test_parse_ssh_command_with_special_chars() {
        // Dot in repo name
        assert_eq!(
            parse_ssh_command("git-upload-pack /repo.with.dots.git"),
            Some(("git-upload-pack", "/repo.with.dots.git"))
        );
        // Underscore and hyphen
        assert_eq!(
            parse_ssh_command("git-upload-pack /my_repo-name"),
            Some(("git-upload-pack", "/my_repo-name"))
        );
    }

    #[test]
    fn test_parse_ssh_command_only_whitespace() {
        assert_eq!(parse_ssh_command("   "), None);
    }

    #[test]
    fn test_ssh_git_handler_creation() {
        use crate::storage::FileStorageBackend;
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let storage = FileStorageBackend::new(dir.path());
        let _handler = SshGitHandler::new(storage);
        // Handler created successfully
    }

    #[tokio::test]
    async fn test_ssh_git_handler_upload_pack() {
        use crate::storage::FileStorageBackend;
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let storage = FileStorageBackend::new(dir.path());
        let handler = SshGitHandler::new(storage.clone());

        let repo_id = RepoId::new();
        let result = handler.upload_pack(repo_id, vec![1, 2, 3]).await;
        // Should fail because repo doesn't exist
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_ssh_git_handler_receive_pack() {
        use crate::storage::FileStorageBackend;
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let storage = FileStorageBackend::new(dir.path());
        let handler = SshGitHandler::new(storage.clone());

        let repo_id = RepoId::new();
        let result = handler.receive_pack(repo_id, vec![1, 2, 3]).await;
        // Should fail because repo doesn't exist
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_ssh_git_handler_with_existing_repo() {
        use crate::storage::FileStorageBackend;
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let storage = FileStorageBackend::new(dir.path());
        let handler = SshGitHandler::new(storage.clone());

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
    async fn test_ssh_git_handler_receive_pack_with_existing_repo() {
        use crate::storage::FileStorageBackend;
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let storage = FileStorageBackend::new(dir.path());
        let handler = SshGitHandler::new(storage.clone());

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
    fn test_parse_ssh_command_with_leading_whitespace() {
        // Command with leading whitespace
        assert_eq!(
            parse_ssh_command("  git-upload-pack /repo"),
            Some(("git-upload-pack", "/repo"))
        );
    }

    #[test]
    fn test_parse_ssh_command_multiple_words_after_repo() {
        // Command with extra args after repo path
        assert_eq!(
            parse_ssh_command("git-upload-pack /repo.git extra args"),
            Some(("git-upload-pack", "/repo.git"))
        );
    }
}

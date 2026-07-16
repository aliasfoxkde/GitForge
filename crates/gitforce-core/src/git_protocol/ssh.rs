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

        // TODO: Implement full ssh upload-pack protocol
        Ok(Vec::new())
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

        // TODO: Implement full ssh receive-pack protocol
        Ok(Vec::new())
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
}

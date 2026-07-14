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
}

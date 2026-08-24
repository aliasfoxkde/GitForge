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

    /// Format a pkt-line
    fn format_pkt_line(content: &str) -> Vec<u8> {
        let len = 4 + content.len();
        let mut result = Vec::with_capacity(len);
        result.extend_from_slice(format!("{:04x}", len).as_bytes());
        result.extend_from_slice(content.as_bytes());
        result
    }
}

#[async_trait]
impl<S: StorageBackend> GitProtocolHandler for SshGitHandler<S> {
    async fn upload_pack(&self, repo_id: RepoId, input: Vec<u8>) -> Result<Vec<u8>> {
        tracing::debug!(
            "ssh upload_pack for repo {} ({} bytes input)",
            repo_id,
            input.len()
        );

        // Check if repository exists
        if !self.storage.exists(repo_id).await {
            return Err(gitforce_common::Error::git(format!(
                "Repository {} not found",
                repo_id
            )));
        }

        // Open the repository
        let repo = self.storage.open(repo_id).await?;

        // Build ref advertisement for SSH protocol
        let mut response = Vec::new();

        // Get HEAD
        if let Ok(reference) = repo.head() {
            if let (Some(name), Some(target)) = (reference.name(), reference.target()) {
                let line = format!("{} {}\n", target, name);
                response.extend_from_slice(&Self::format_pkt_line(&line));
            }
        }

        // Get all refs
        if let Ok(refs) = repo.references() {
            for reference in refs.flatten() {
                if let (Some(name), Some(target)) = (reference.name(), reference.target()) {
                    if name.starts_with("refs/") && !name.contains("^{}") {
                        let line = format!("{} {}\n", target, name);
                        response.extend_from_slice(&Self::format_pkt_line(&line));
                    }
                }
            }
        }

        // End with flush pkt-line
        response.extend_from_slice(b"0000");

        Ok(response)
    }

    async fn receive_pack(&self, repo_id: RepoId, input: Vec<u8>) -> Result<Vec<u8>> {
        tracing::debug!(
            "ssh receive_pack for repo {} ({} bytes input)",
            repo_id,
            input.len()
        );

        // Check if repository exists
        if !self.storage.exists(repo_id).await {
            return Err(gitforce_common::Error::git(format!(
                "Repository {} not found",
                repo_id
            )));
        }

        // Open the repository
        let repo = self.storage.open(repo_id).await?;

        // Process the pack data if present
        let mut unpack_ok = false;

        if !input.is_empty() {
            // Find the pack data start
            // Pack data starts with "PACK" magic bytes (0x50 0x41 0x43 0x4b)
            let pack_start = input.windows(4).position(|w| w == [0x50, 0x41, 0x43, 0x4b]);

            if let Some(pos) = pack_start {
                let pack_data = &input[pos..];
                if pack_data.len() > 8 {
                    match write_pack_to_odb(&repo, pack_data) {
                        Ok(()) => {
                            unpack_ok = true;
                            tracing::info!("Successfully unpacked pack data for repo {}", repo_id);
                        }
                        Err(e) => {
                            tracing::warn!("Failed to process pack data: {}", e);
                        }
                    }
                }
            } else {
                // No pack data - could be a ref-only update
                unpack_ok = true;
            }
        } else {
            // Empty input - just checking capabilities, that's ok
            unpack_ok = true;
        }

        // Get current refs to report
        let mut updated_refs = Vec::new();
        if let Ok(references) = repo.references() {
            for reference in references.flatten() {
                if let Some(name) = reference.name() {
                    if name.starts_with("refs/") && !name.contains("^{}") {
                        updated_refs.push(name.to_string());
                    }
                }
            }
        }

        // Build response
        let mut response = Vec::new();

        // Report unpack result
        response.extend_from_slice(b"0000"); // flush
        if unpack_ok {
            response.extend_from_slice(b"unpack ok\n");
        } else {
            response.extend_from_slice(b"unpack error\n");
        }

        // Report each ref update
        for ref_name in &updated_refs {
            let ok_line = format!("ok {}\n", ref_name);
            response.extend_from_slice(ok_line.as_bytes());
        }

        // End
        response.extend_from_slice(b"0000");

        Ok(response)
    }
}

/// Write pack data to repository's object database
fn write_pack_to_odb(repo: &git2::Repository, pack_data: &[u8]) -> gitforce_common::Result<()> {
    // Write pack data to repository's odb
    let odb = repo
        .odb()
        .map_err(|e| gitforce_common::Error::git(format!("Failed to get odb: {}", e)))?;

    // Create a packwriter to write the pack
    let mut packwriter = odb
        .packwriter()
        .map_err(|e| gitforce_common::Error::git(format!("Failed to create packwriter: {}", e)))?;

    // Write the pack data
    std::io::Write::write_all(&mut packwriter, pack_data)
        .map_err(|e| gitforce_common::Error::git(format!("Failed to write pack: {}", e)))?;

    // The pack is finalized when packwriter is dropped
    drop(packwriter);

    tracing::debug!("Wrote pack data ({} bytes) to odb", pack_data.len());
    Ok(())
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
        // Response should contain ref advertisement (starts with pkt-line format)
        assert!(!response.is_empty());
        // Should end with flush pkt-line (0000)
        assert!(response.ends_with(b"0000"));
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
        // Response should contain acknowledgment
        assert!(!response.is_empty());
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

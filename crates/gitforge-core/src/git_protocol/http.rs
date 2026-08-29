//! HTTP Git protocol handler
//!
//! Implements the Smart HTTP protocol for git

use super::GitProtocolHandler;
use crate::storage::StorageBackend;
use async_trait::async_trait;
use gitforge_common::{RepoId, Result};
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

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

    /// Format a pkt-line response
    fn format_pkt_line(content: &str) -> Vec<u8> {
        let len = 4 + content.len();
        let mut result = Vec::with_capacity(len);
        // pkt-line length as 4-digit hex
        result.extend_from_slice(format!("{:04x}", len).as_bytes());
        result.extend_from_slice(content.as_bytes());
        result
    }

    /// Build ref advertisement from repository
    async fn build_ref_advertisement(&self, repo_id: RepoId, service: &str) -> Result<Vec<u8>> {
        let repo = self.storage.open(repo_id).await?;

        // Smart HTTP protocol v0 starts with a service announcement and a
        // flush packet. The previous implementation wrote plain text here,
        // which made standard Git clients reject the response before they
        // could discover refs.
        let mut response = Vec::new();
        response.extend_from_slice(&Self::format_pkt_line(&format!("# service={service}\n")));
        response.extend_from_slice(b"0000");

        let capabilities =
            "multi_ack_detailed no-done side-band-64k ofs-delta agent=gitforge/0.1.0";
        let mut advertised_ref = false;
        if let Ok(refs) = repo.references() {
            for reference in refs.flatten() {
                if let (Some(name), Some(target)) = (reference.name().ok(), reference.target()) {
                    if name.starts_with("refs/") && !name.contains("^{}") {
                        let ref_line = if advertised_ref {
                            format!("{} {}\n", target, name)
                        } else {
                            advertised_ref = true;
                            format!("{} {}\0{}\n", target, name, capabilities)
                        };
                        response.extend_from_slice(&Self::format_pkt_line(&ref_line));
                    }
                }
            }
        }

        // End with flush pkt-line (0000)
        response.extend_from_slice(b"0000");

        Ok(response)
    }

    /// Build the receive-pack ref advertisement used by standard Git pushes.
    pub async fn receive_pack_advertisement(&self, repo_id: RepoId) -> Result<Vec<u8>> {
        if !self.storage.exists(repo_id).await {
            return Err(gitforge_common::Error::git(format!(
                "Repository {} not found",
                repo_id
            )));
        }
        self.build_ref_advertisement(repo_id, "git-receive-pack")
            .await
    }
}

#[async_trait]
impl<S: StorageBackend> GitProtocolHandler for HttpGitHandler<S> {
    async fn upload_pack(&self, repo_id: RepoId, input: Vec<u8>) -> Result<Vec<u8>> {
        tracing::debug!(
            "upload_pack for repo {} ({} bytes input)",
            repo_id,
            input.len()
        );

        // Check if repository exists
        if !self.storage.exists(repo_id).await {
            return Err(gitforge_common::Error::git(format!(
                "Repository {} not found",
                repo_id
            )));
        }

        // For upload-pack, we need to return the ref advertisement
        // In smart HTTP, the client first does a GET to /info/refs to discover refs
        // then a POST with upload-pack request
        // For now, return the ref advertisement
        self.build_ref_advertisement(repo_id, "git-upload-pack")
            .await
    }

    async fn receive_pack(&self, repo_id: RepoId, input: Vec<u8>) -> Result<Vec<u8>> {
        tracing::debug!(
            "receive_pack for repo {} ({} bytes input)",
            repo_id,
            input.len()
        );

        // Check if repository exists
        if !self.storage.exists(repo_id).await {
            return Err(gitforge_common::Error::git(format!(
                "Repository {} not found",
                repo_id
            )));
        }

        if input.is_empty() {
            return Ok(b"0000".to_vec());
        }

        // Let Git parse the request, update refs, execute hooks, and produce
        // correctly framed status output. Writing only the pack to the object
        // database leaves refs unchanged and breaks clients.
        let repo = self.storage.open(repo_id).await?;
        let mut child = Command::new("git-receive-pack")
            .arg("--stateless-rpc")
            .arg(repo.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| {
                gitforge_common::Error::git(format!("failed to execute git-receive-pack: {error}"))
            })?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(&input).await.map_err(|error| {
                gitforge_common::Error::git(format!("failed to send receive-pack request: {error}"))
            })?;
        }
        let output = child.wait_with_output().await.map_err(|error| {
            gitforge_common::Error::git(format!("failed to wait for git-receive-pack: {error}"))
        })?;
        if !output.status.success() {
            return Err(gitforge_common::Error::git(format!(
                "git-receive-pack failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        return Ok(output.stdout);
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
        assert_eq!(
            parse_content_type("application/x-git-upload-pack-request"),
            Some("application/x-git-upload-pack-request")
        );
        assert_eq!(
            parse_content_type("text/plain; charset=utf-8"),
            Some("text/plain")
        );
    }

    #[test]
    fn test_parse_content_type_edge_cases() {
        // Empty string
        assert_eq!(parse_content_type(""), Some(""));
        // Just whitespace - trims to empty
        assert_eq!(parse_content_type("   "), Some(""));
        // Multiple semicolons
        assert_eq!(
            parse_content_type("text/plain; charset=utf-8; boundary=abc"),
            Some("text/plain")
        );
    }

    #[test]
    fn test_parse_service() {
        assert_eq!(
            parse_service("/git-upload-pack/owner/repo"),
            Some(("git-upload-pack".to_string(), "owner/repo".to_string()))
        );
        assert_eq!(
            parse_service("git-receive-pack/owner/repo.git/info/refs"),
            Some((
                "git-receive-pack".to_string(),
                "owner/repo.git/info/refs".to_string()
            ))
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
            Some((
                "git-upload-pack".to_string(),
                "owner/repo/path/to/refs".to_string()
            ))
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
        // Response should contain ref advertisement (starts with pkt-line format)
        assert!(!response.is_empty());
        assert!(response.starts_with(b"001e# service=git-upload-pack\n0000"));
        // Should end with flush pkt-line (0000)
        assert!(response.ends_with(b"0000"));
    }

    #[tokio::test]
    async fn test_receive_pack_advertisement_uses_receive_service() {
        use crate::storage::FileStorageBackend;
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let storage = FileStorageBackend::new(dir.path());
        let handler = HttpGitHandler::new(storage.clone());
        let repo_id = RepoId::new();
        storage.create(repo_id).await.unwrap();

        let response = handler.receive_pack_advertisement(repo_id).await.unwrap();
        assert!(response.starts_with(b"001f# service=git-receive-pack\n0000"));
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
        // Response should contain acknowledgment
        assert!(!response.is_empty());
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
        assert_eq!(
            parse_content_type("application/json;charset=utf-8"),
            Some("application/json")
        );
        assert_eq!(
            parse_content_type("text/html; charset=ISO-8859-1"),
            Some("text/html")
        );
    }

    #[test]
    fn test_parse_service_various_paths() {
        // Various Git URL formats
        assert_eq!(
            parse_service("/git-upload-pack/owner/project.git"),
            Some((
                "git-upload-pack".to_string(),
                "owner/project.git".to_string()
            ))
        );
        assert_eq!(
            parse_service("git-receive-pack/my-org/my-repo"),
            Some(("git-receive-pack".to_string(), "my-org/my-repo".to_string()))
        );
    }
}

//! HTTP Git protocol handler
//!
//! Implements the Smart HTTP protocol for git

use super::GitProtocolHandler;
use crate::storage::StorageBackend;
use async_trait::async_trait;
use gitforge_common::{RepoId, Result};
use std::process::Stdio;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

/// Upper bound on how long the synchronous abandon-reap may block waiting for
/// a SIGKILLed git child to actually exit. Git processes die within
/// microseconds of SIGKILL unless stuck in uninterruptible I/O; exceeding the
/// bound logs a warning and leaves the child to tokio's best-effort orphan
/// queue rather than blocking the caller indefinitely.
const ABANDONED_CHILD_REAP_GRACE: Duration = Duration::from_secs(1);

/// Owns a spawned `git-*-pack --stateless-rpc` child until it has been reaped.
///
/// GitForge services run with `init_without_sigchld_reaper`, so the spawner is
/// the only party responsible for reaping (see `gitforge_process`). This guard
/// makes that ownership explicit:
///
/// - On the success path `drive` waits on the child itself, so the exit status
///   is observed exactly once by the owner.
/// - On any early return (client disconnect while feeding stdin, child spawn
///   or I/O error) or cancellation (the handler future is dropped at an await
///   point, including during service shutdown), `Drop` kills the child and
///   reaps it synchronously instead of leaving a zombie behind and hoping
///   tokio's documented best-effort orphan queue gets scheduled.
struct GitRpcChild {
    child: Option<tokio::process::Child>,
    /// Binary name, also used in error messages ("git-upload-pack").
    service: &'static str,
    /// Short name used in stdin error messages ("upload-pack").
    short: &'static str,
}

impl GitRpcChild {
    fn spawn(
        service: &'static str,
        short: &'static str,
        repo_path: &std::path::Path,
    ) -> Result<Self> {
        let mut command = Command::new(service);
        command
            .arg("--stateless-rpc")
            .arg(repo_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        Self::from_command(command, service, short)
    }

    fn from_command(
        mut command: Command,
        service: &'static str,
        short: &'static str,
    ) -> Result<Self> {
        let child = command.spawn().map_err(|error| {
            gitforge_common::Error::git(format!("failed to execute {service}: {error}"))
        })?;
        Ok(Self {
            child: Some(child),
            service,
            short,
        })
    }

    /// Feed `input` to the child's stdin and collect its response.
    ///
    /// The child stays owned by `self` across every await point, so dropping
    /// this future at any point still runs the reaping `Drop`.
    async fn drive(mut self, input: Vec<u8>) -> Result<Vec<u8>> {
        if let Some(mut stdin) = self
            .child
            .as_mut()
            .expect(OWNED_CHILD_INVARIANT)
            .stdin
            .take()
        {
            stdin.write_all(&input).await.map_err(|error| {
                gitforge_common::Error::git(format!(
                    "failed to send {} request: {error}",
                    self.short
                ))
            })?;
        }
        // stdin is dropped here, signalling EOF to the git child.

        // Drain stdout and stderr concurrently so a chatty child cannot fill
        // a pipe buffer and deadlock, mirroring `Child::wait_with_output`
        // while keeping the child owned by the guard at every await point.
        let stdout_pipe = self
            .child
            .as_mut()
            .expect(OWNED_CHILD_INVARIANT)
            .stdout
            .take();
        let stderr_pipe = self
            .child
            .as_mut()
            .expect(OWNED_CHILD_INVARIANT)
            .stderr
            .take();
        let (stdout, stderr) = tokio::join!(drain_pipe(stdout_pipe), drain_pipe(stderr_pipe));
        let wait_error = |error: std::io::Error| {
            gitforge_common::Error::git(format!("failed to wait for {}: {error}", self.service))
        };
        let stdout = stdout.map_err(wait_error)?;
        let stderr = stderr.map_err(wait_error)?;

        let status = self
            .child
            .as_mut()
            .expect(OWNED_CHILD_INVARIANT)
            .wait()
            .await
            .map_err(wait_error)?;
        // Fully reaped: tell the Drop guard to stand down.
        self.child = None;

        if !status.success() {
            return Err(gitforge_common::Error::git(format!(
                "{} failed: {}",
                self.service,
                String::from_utf8_lossy(&stderr)
            )));
        }
        Ok(stdout)
    }
}

const OWNED_CHILD_INVARIANT: &str = "git rpc child is owned by GitRpcChild until it is reaped";

/// Read a piped child stream to EOF, returning whatever was collected.
async fn drain_pipe<R: tokio::io::AsyncRead + Unpin>(pipe: Option<R>) -> std::io::Result<Vec<u8>> {
    let mut buffer = Vec::new();
    if let Some(mut pipe) = pipe {
        pipe.read_to_end(&mut buffer).await?;
    }
    Ok(buffer)
}

impl Drop for GitRpcChild {
    fn drop(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        match child.try_wait() {
            Ok(Some(_)) | Err(_) => return, // already reaped (or not ours anymore)
            Ok(None) => {}
        }
        // The child was abandoned before it could be waited on: kill it and
        // reap synchronously so it cannot linger as a zombie in this process,
        // which runs without a global SIGCHLD reaper.
        if let Err(error) = child.start_kill() {
            tracing::warn!(
                "failed to signal abandoned {} child: {}",
                self.service,
                error
            );
        }
        let deadline = Instant::now() + ABANDONED_CHILD_REAP_GRACE;
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    tracing::debug!(
                        "reaped abandoned {} child with status {status}",
                        self.service
                    );
                    break;
                }
                Ok(None) => {
                    if Instant::now() >= deadline {
                        tracing::warn!(
                            "abandoned {} child did not exit within {ABANDONED_CHILD_REAP_GRACE:?}; \
                             leaving it to the runtime orphan queue",
                            self.service
                        );
                        break;
                    }
                    thread::sleep(Duration::from_millis(1));
                }
                Err(error) => {
                    tracing::warn!("failed to reap abandoned {} child: {}", self.service, error);
                    break;
                }
            }
        }
    }
}

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

        let mut capabilities = if service == "git-receive-pack" {
            // Receive-pack negotiation: without report-status the client
            // cannot learn the ref update result and misreports the push.
            "report-status-v2 report-status delete-refs side-band-64k quiet atomic ofs-delta agent=gitforge/0.1.0"
        } else {
            "multi_ack_detailed no-done side-band-64k ofs-delta agent=gitforge/0.1.0"
        }
        .to_string();
        // Clone/fetch need the HEAD symref to pick the default branch;
        // without it clients fetch the objects but cannot check out.
        if let Ok(head) = repo.head() {
            if let Ok(name) = head.name() {
                capabilities = format!("{capabilities} symref=HEAD:{name}");
            }
        }
        // Advertise the HEAD pseudo-ref first, exactly like git upload-pack
        // does: clients require the HEAD entry (plus the symref capability)
        // to select and check out the default branch on clone.
        let mut advertised_ref = false;
        if let Ok(head) = repo.head() {
            if let Some(oid) = head.target() {
                response.extend_from_slice(&Self::format_pkt_line(&format!(
                    "{oid} HEAD\0{capabilities}\n"
                )));
                advertised_ref = true;
            }
        }
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

        if !advertised_ref {
            // An empty repository advertises no refs; the protocol still
            // requires the zero-id capabilities line so clients can
            // negotiate (without it pushes land but clients misreport).
            let cap_line = format!(
                "0000000000000000000000000000000000000000 capabilities^{{}}\0{capabilities}\n"
            );
            response.extend_from_slice(&Self::format_pkt_line(&cap_line));
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

        if input.is_empty() {
            // Legacy explicit-route callers fetch the advertisement directly.
            return self
                .build_ref_advertisement(repo_id, "git-upload-pack")
                .await;
        }

        // Serve fetch negotiation by piping the client wants/haves through a
        // real git-upload-pack (same shape as receive_pack): returning the
        // advertisement here made clones and fetches unusable. The child is
        // owned by a guard that reaps it on every exit path.
        let repo = self.storage.open(repo_id).await?;
        let child = GitRpcChild::spawn("git-upload-pack", "upload-pack", repo.path())?;
        child.drive(input).await
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
        // database leaves refs unchanged and breaks clients. The child is
        // owned by a guard that reaps it on every exit path.
        let repo = self.storage.open(repo_id).await?;
        let child = GitRpcChild::spawn("git-receive-pack", "receive-pack", repo.path())?;
        child.drive(input).await
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

        // Empty input returns the ref advertisement (legacy explicit route).
        let result = handler.upload_pack(repo_id, vec![]).await;
        assert!(result.is_ok());
        let response = result.unwrap();
        // Response should contain ref advertisement (starts with pkt-line format)
        assert!(!response.is_empty());
        assert!(response.starts_with(b"001e# service=git-upload-pack\n0000"));
        // Should end with flush pkt-line (0000)
        assert!(response.ends_with(b"0000"));

        // Non-empty input pipes through a real git-upload-pack, so garbage
        // bytes are rejected by git instead of answered with an advertisement.
        let result = handler.upload_pack(repo_id, vec![1, 2, 3]).await;
        assert!(result.is_err());
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

    // ---- Child lifecycle tests -------------------------------------------
    //
    // GitForge services intentionally run without a process-wide SIGCHLD
    // reaper, so the handler owns every spawned git child until it is reaped.
    // These tests exercise that ownership contract on real child processes in
    // a self-contained way: /bin/sh stand-ins and /proc-free waitid checks, no
    // live host or network dependency.

    /// Spawn `sh -c 'exit N'` as a stand-in git child so lifecycle tests run
    /// anywhere the rest of this suite runs.
    fn spawn_test_child(exit_code: i32) -> GitRpcChild {
        let mut command = Command::new("sh");
        command.arg("-c").arg(format!("exit {exit_code}"));
        command.stdin(Stdio::piped());
        command.stdout(Stdio::piped());
        command.stderr(Stdio::piped());
        GitRpcChild::from_command(command, "git-upload-pack", "upload-pack")
            .expect("failed to spawn sh")
    }

    /// Assert the raw pid has been fully reaped: waitid(WNOWAIT|WNOHANG) must
    /// fail with ECHILD. A zombie (unreaped) or still-running child would make
    /// waitid succeed instead, failing the assertion.
    fn assert_pid_reaped(pid: u32) {
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        let result = unsafe {
            libc::waitid(
                libc::P_PID,
                pid as libc::id_t,
                &mut info,
                libc::WEXITED | libc::WNOWAIT | libc::WNOHANG,
            )
        };
        let errno = std::io::Error::last_os_error().raw_os_error();
        assert_eq!(result, -1, "pid {pid} still exists (not reaped)");
        assert_eq!(errno, Some(libc::ECHILD), "pid {pid} unexpected errno");
    }

    #[tokio::test]
    async fn test_git_rpc_child_drive_reports_success_status() {
        // Normal completion: the child is waited on by the owner and its
        // stdout is returned.
        let child = spawn_test_child(0);
        let output = child.drive(b"ignored".to_vec()).await.unwrap();
        assert!(output.is_empty());
    }

    #[tokio::test]
    async fn test_git_rpc_child_drive_maps_child_error() {
        // Child error path: a nonzero exit maps to the same git error string
        // the handler previously produced, and the child is reaped.
        let child = spawn_test_child(3);
        let error = child.drive(b"ignored".to_vec()).await.unwrap_err();
        let message = error.to_string();
        assert!(
            message.contains("git-upload-pack failed"),
            "unexpected error: {message}"
        );
    }

    #[tokio::test]
    async fn test_git_rpc_child_abandoned_mid_flight_is_reaped_not_zombied() {
        // Cancellation/disconnect path: the guard (with its still-running
        // child) is dropped between spawn and wait, exactly like a handler
        // future dropped at an await point. Drop must kill and reap the child
        // synchronously because the process has no global SIGCHLD reaper.
        let guard = spawn_test_child(0);
        let pid = guard
            .child
            .as_ref()
            .expect(OWNED_CHILD_INVARIANT)
            .id()
            .expect("live child pid");
        drop(guard);
        assert_pid_reaped(pid);
    }

    #[tokio::test]
    async fn test_git_rpc_child_drop_after_reap_is_noop() {
        // After a completed drive the guard no longer owns a child
        // (drive consumed it and cleared `child`), so no second reap happens
        // and the completed pid is fully gone — no double-reap, no zombie.
        let guard = spawn_test_child(0);
        let pid = guard
            .child
            .as_ref()
            .expect(OWNED_CHILD_INVARIANT)
            .id()
            .expect("live child pid");
        let driven = guard.drive(b"ignored".to_vec()).await;
        assert!(driven.is_ok());
        assert_pid_reaped(pid);
    }

    #[tokio::test]
    async fn test_upload_pack_child_spawn_is_reaped_when_abandoned() {
        // Same abandon-reap contract against the real spawn configuration
        // (git-upload-pack --stateless-rpc on a real repository with piped
        // stdio), covering the exact call site the handler uses.
        use crate::storage::FileStorageBackend;
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let storage = FileStorageBackend::new(dir.path());
        let repo_id = RepoId::new();
        storage.create(repo_id).await.unwrap();
        let repo = storage.open(repo_id).await.unwrap();

        let guard = GitRpcChild::spawn("git-upload-pack", "upload-pack", repo.path()).unwrap();
        let pid = guard
            .child
            .as_ref()
            .expect(OWNED_CHILD_INVARIANT)
            .id()
            .expect("live child pid");
        drop(guard);
        assert_pid_reaped(pid);
    }

    #[test]
    fn test_abandoned_child_grace_is_bounded() {
        // The synchronous abandon-reap runs on the caller's thread inside
        // Drop; it must stay bounded so a stuck child can never block the
        // runtime thread indefinitely.
        assert!(ABANDONED_CHILD_REAP_GRACE <= std::time::Duration::from_secs(5));
    }
}

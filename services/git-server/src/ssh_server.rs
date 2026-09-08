//! Git over SSH transport.
//!
//! A real SSH server (russh) that serves `git-upload-pack` and
//! `git-receive-pack` by piping authenticated channels to real git child
//! processes operating on the bare repositories. This is the same shape as
//! an sshd restricted to git commands: the full protocol negotiation
//! (ref advertisement, want/have, pack transfer, status report) is handled
//! by git itself, and refs move exactly as they do over Smart HTTP.
//!
//! Authentication requires a public key. Keys are accepted by possession —
//! there is no per-user key registry yet, matching the unauthenticated
//! Smart HTTP transport — and every accepted fingerprint is logged.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use gitforge_core::{FileStorageBackend, StorageBackend};
use gitforge_db::Pool;
use russh::keys::{ssh_key::LineEnding, Algorithm, HashAlg, PrivateKey, PublicKey};
use russh::server::{Auth, Handler, Msg, Server, Session};
use russh::{Channel, MethodKind, MethodSet};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::ChildStdin;

/// Configuration for the SSH git transport.
pub struct SshServerConfig {
    pub port: u16,
    pub storage: Arc<FileStorageBackend>,
    pub db_pool: Option<Arc<Pool>>,
    /// Path to the ed25519 host key. Generated on first boot if absent.
    pub host_key_path: PathBuf,
}

/// Resolve the host key path: `GITFORGE_SSH_HOST_KEY` if set, otherwise a
/// gitforge key inside the user's `.ssh` directory.
pub fn default_host_key_path() -> PathBuf {
    if let Ok(path) = std::env::var("GITFORGE_SSH_HOST_KEY") {
        return PathBuf::from(path);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    Path::new(&home).join(".ssh").join("gitforge_host_ed25519")
}

/// Load the server host key from `path`, or generate and persist a new
/// ed25519 key on first boot. The public half is written next to it in
/// OpenSSH format so operators can pin the host key in `known_hosts`.
async fn load_or_create_host_key(path: &Path) -> anyhow::Result<PrivateKey> {
    if path.exists() {
        let pem = tokio::fs::read_to_string(path).await?;
        return PrivateKey::from_openssh(&pem).map_err(|error| {
            anyhow::anyhow!("failed to parse host key {}: {error}", path.display())
        });
    }

    let key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519)
        .map_err(|error| anyhow::anyhow!("failed to generate host key: {error}"))?;

    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let pem = key
        .to_openssh(LineEnding::LF)
        .map_err(|error| anyhow::anyhow!("failed to encode host key: {error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::write(path, pem.as_bytes()).await?;
        tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).await?;
    }
    #[cfg(not(unix))]
    tokio::fs::write(path, pem.as_bytes()).await?;

    let public = key
        .public_key()
        .to_openssh()
        .map_err(|error| anyhow::anyhow!("failed to encode host public key: {error}"))?;
    let public_path = path.with_extension("pub");
    tokio::fs::write(&public_path, format!("{public} gitforge-host-key\n")).await?;

    Ok(key)
}

/// State shared across SSH client connections.
struct SshContext {
    storage: Arc<FileStorageBackend>,
    db_pool: Option<Arc<Pool>>,
}

/// Per-process listener implementing `russh::server::Server`.
#[derive(Clone)]
pub struct GitSshServer {
    context: Arc<SshContext>,
}

impl GitSshServer {
    pub fn new(storage: Arc<FileStorageBackend>, db_pool: Option<Arc<Pool>>) -> Self {
        Self {
            context: Arc::new(SshContext { storage, db_pool }),
        }
    }
}

impl Server for GitSshServer {
    type Handler = GitSshSession;

    fn new_client(&mut self, peer_addr: Option<SocketAddr>) -> Self::Handler {
        tracing::debug!(?peer_addr, "new SSH connection");
        GitSshSession {
            context: self.context.clone(),
            processes: HashMap::new(),
        }
    }
}

/// A git child process serving one channel. Only stdin is kept here so
/// channel data and EOF can flow into the child; the reader task owns the
/// child itself along with its stdout and stderr.
struct ChannelProcess {
    stdin: ChildStdin,
}

/// Per-connection handler.
pub struct GitSshSession {
    context: Arc<SshContext>,
    /// Running git processes keyed by channel id.
    processes: HashMap<russh::ChannelId, ChannelProcess>,
}

impl Handler for GitSshSession {
    type Error = russh::Error;

    async fn auth_none(&mut self, _user: &str) -> Result<Auth, Self::Error> {
        Ok(Auth::Reject {
            proceed_with_methods: Some(MethodSet::from(&[MethodKind::PublicKey][..])),
            partial_success: false,
        })
    }

    async fn auth_publickey(&mut self, user: &str, key: &PublicKey) -> Result<Auth, Self::Error> {
        tracing::info!(
            user,
            fingerprint = %key.fingerprint(HashAlg::Sha256),
            "SSH public key accepted"
        );
        Ok(Auth::Accept)
    }

    async fn channel_open_session(
        &mut self,
        _channel: Channel<Msg>,
        reply: russh::server::ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        reply.accept().await;
        Ok(())
    }

    async fn exec_request(
        &mut self,
        channel: russh::ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let command = String::from_utf8_lossy(data).to_string();
        tracing::debug!(%command, "SSH exec request");

        match self.serve_git_command(channel, &command, session).await {
            Ok(()) => Ok(()),
            Err(message) => {
                tracing::warn!(%command, %message, "SSH git command rejected");
                // Report the reason on stderr and fail with a non-zero exit
                // status. Deliberately no CHANNEL_FAILURE here: it makes the
                // OpenSSH client tear the channel down before the buffered
                // stderr text is delivered.
                let handle = session.handle();
                let _ = handle
                    .extended_data(channel, 1, format!("gitforge: {message}\n").into_bytes())
                    .await;
                let _ = handle.exit_status_request(channel, 128).await;
                let _ = handle.eof(channel).await;
                let _ = handle.close(channel).await;
                Ok(())
            }
        }
    }

    async fn data(
        &mut self,
        channel: russh::ChannelId,
        data: &[u8],
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        if let Some(process) = self.processes.get_mut(&channel) {
            if let Err(error) = process.stdin.write_all(data).await {
                tracing::warn!(
                    ?channel,
                    %error,
                    "failed to forward SSH channel data to git process"
                );
                self.processes.remove(&channel);
            }
        }
        Ok(())
    }

    async fn channel_eof(
        &mut self,
        channel: russh::ChannelId,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        // Dropping stdin signals EOF to the git child process.
        self.processes.remove(&channel);
        Ok(())
    }

    async fn channel_close(
        &mut self,
        channel: russh::ChannelId,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.processes.remove(&channel);
        Ok(())
    }
}

impl GitSshSession {
    /// Resolve the requested git command to a repository and start the real
    /// git child process that serves it, wiring the channel to its stdio.
    async fn serve_git_command(
        &mut self,
        channel: russh::ChannelId,
        command: &str,
        session: &mut Session,
    ) -> Result<(), String> {
        let (git_command, repo_path) = parse_git_command(command)?;

        let repo_id = self.resolve_repo_id(&repo_path).await?;
        let repo_disk_path = {
            let storage = self.context.storage.clone();
            if !storage.exists(repo_id).await {
                return Err(format!("repository {repo_path} not found"));
            }
            storage.repo_path(repo_id)
        };

        let mut child = tokio::process::Command::new("git")
            .arg(git_command)
            .arg(&repo_disk_path)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|error| format!("failed to spawn git {git_command}: {error}"))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "git process was spawned without a piped stdin".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "git process was spawned without a piped stdout".to_string())?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| "git process was spawned without a piped stderr".to_string())?;

        self.processes.insert(channel, ChannelProcess { stdin });

        session
            .handle()
            .channel_success(channel)
            .await
            .map_err(|()| format!("channel {channel:?} already closed"))?;

        let handle = session.handle();
        tokio::spawn(pump_until_exit(handle, channel, child, stdout, stderr));
        tracing::info!(command = %command.trim(), %repo_path, "git process started");
        Ok(())
    }

    /// Map an `owner/repo` request path to the repository id via the
    /// database, mirroring the Smart HTTP transport's resolution rules.
    async fn resolve_repo_id(&self, repo_path: &str) -> Result<gitforge_common::RepoId, String> {
        let pool = self
            .context
            .db_pool
            .as_ref()
            .ok_or_else(|| "database not available for SSH connections".to_string())?;

        let trimmed = repo_path.trim().trim_start_matches('/');
        let mut segments = trimmed.split('/');
        let owner = segments.next().unwrap_or_default();
        let repo = segments
            .next()
            .unwrap_or_default()
            .trim_end_matches(".git")
            .trim_end_matches('/');
        if owner.is_empty() || repo.is_empty() {
            return Err(format!("invalid repository path: {repo_path}"));
        }

        match gitforge_db::queries::RepoQueries::get_by_owner_and_name(pool, owner, repo).await {
            Ok(Some(repository)) => Ok(repository.id),
            Ok(None) => Err(format!("repository {owner}/{repo} not found")),
            Err(error) => {
                tracing::error!(%error, "database lookup failed for SSH connection");
                Err("database error".to_string())
            }
        }
    }
}

/// Split an SSH exec request into the git subcommand and repository path.
/// git quotes the path (`git-upload-pack '/owner/repo.git'`), so surrounding
/// quotes are stripped before returning.
fn parse_git_command(command: &str) -> Result<(&str, String), String> {
    let mut parts = command.split_whitespace();
    let git_command = parts.next().ok_or_else(|| "empty command".to_string())?;
    let raw_path = parts
        .next()
        .ok_or_else(|| format!("missing repository path in command: {command}"))?;

    let subcommand = git_command
        .strip_prefix("git-")
        .ok_or_else(|| format!("unsupported command: {git_command}"))?;
    if subcommand != "upload-pack" && subcommand != "receive-pack" {
        return Err(format!(
            "unsupported command: {git_command} (only git-upload-pack and git-receive-pack are served)"
        ));
    }

    let repo_path = raw_path.trim().trim_matches('\'').trim_matches('"');
    if repo_path.is_empty() {
        return Err(format!("missing repository path in command: {command}"));
    }
    Ok((subcommand, repo_path.to_string()))
}

/// Forward the git child's stdout to the channel, its stderr as extended
/// (stderr) data, and report the exit status once both pipes are drained.
async fn pump_until_exit(
    handle: russh::server::Handle,
    channel: russh::ChannelId,
    mut child: tokio::process::Child,
    stdout: tokio::process::ChildStdout,
    stderr: tokio::process::ChildStderr,
) {
    let data_handle = handle.clone();
    let out_task = tokio::spawn(async move {
        pipe_to_channel(data_handle, channel, stdout, false).await;
    });
    let error_handle = handle.clone();
    let err_task = tokio::spawn(async move {
        pipe_to_channel(error_handle, channel, stderr, true).await;
    });
    let _ = out_task.await;
    let _ = err_task.await;

    let status = child
        .wait()
        .await
        .ok()
        .and_then(|status| status.code())
        .unwrap_or(-1);
    let _ = handle.exit_status_request(channel, status as u32).await;
    let _ = handle.eof(channel).await;
    let _ = handle.close(channel).await;
}

/// Copy one pipe of a git child process into the SSH channel until EOF.
async fn pipe_to_channel(
    handle: russh::server::Handle,
    channel: russh::ChannelId,
    mut pipe: impl tokio::io::AsyncRead + Unpin,
    extended: bool,
) {
    let mut buffer = vec![0u8; 16 * 1024];
    loop {
        match pipe.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let chunk = buffer[..n].to_vec();
                let sent = if extended {
                    handle.extended_data(channel, 1, chunk).await
                } else {
                    handle.data(channel, chunk).await
                };
                if sent.is_err() {
                    // The client is gone; the child exits on EOF from stdin.
                    break;
                }
            }
        }
    }
}

/// Bind and run the SSH git transport until `shutdown` is set.
pub async fn run_ssh_server(
    config: SshServerConfig,
    shutdown: Arc<AtomicBool>,
) -> anyhow::Result<()> {
    let host_key = load_or_create_host_key(&config.host_key_path).await?;
    tracing::info!(
        fingerprint = %host_key.public_key().fingerprint(HashAlg::Sha256),
        path = %config.host_key_path.display(),
        "SSH host key ready"
    );

    let ssh_config = Arc::new(russh::server::Config {
        keys: vec![host_key],
        auth_rejection_time: Duration::from_secs(1),
        auth_rejection_time_initial: Some(Duration::ZERO),
        inactivity_timeout: Some(Duration::from_secs(600)),
        ..Default::default()
    });

    let addr = SocketAddr::from(([0, 0, 0, 0], config.port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("Git SSH server listening on {addr}");

    let server = GitSshServer::new(config.storage, config.db_pool);
    let mut server = server;
    let mut running = server.run_on_socket(ssh_config, &listener);
    let shutdown_handle = running.handle();

    let mut shutdown_requested = false;
    tokio::select! {
        result = &mut running => {
            result.map_err(|error| anyhow::anyhow!("SSH server failed: {error}"))?;
        }
        _ = wait_for_shutdown_flag(shutdown) => {
            shutdown_requested = true;
        }
    }

    if shutdown_requested {
        tracing::info!("SSH server shutting down");
        shutdown_handle.shutdown("gitforge git-server shutting down".to_string());
        let _ = running.await;
    }
    Ok(())
}

/// Poll the shared shutdown flag so SIGTERM-style signaling still stops the
/// transport (russh has no fd-level hook for it).
async fn wait_for_shutdown_flag(shutdown: Arc<AtomicBool>) {
    loop {
        if shutdown.load(Ordering::SeqCst) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_git_command_quoted_and_bare_paths() {
        let (command, path) = parse_git_command("git-upload-pack '/owner/repo.git'").unwrap();
        assert_eq!(command, "upload-pack");
        assert_eq!(path, "/owner/repo.git");

        let (command, path) = parse_git_command("git-receive-pack '/owner/repo.git'").unwrap();
        assert_eq!(command, "receive-pack");
        assert_eq!(path, "/owner/repo.git");

        let (_, path) = parse_git_command("git-upload-pack /owner/repo.git").unwrap();
        assert_eq!(path, "/owner/repo.git");

        let (_, path) = parse_git_command("git-upload-pack \"/owner/repo.git\"").unwrap();
        assert_eq!(path, "/owner/repo.git");
    }

    #[test]
    fn test_parse_git_command_rejects_unsupported_and_malformed() {
        assert!(parse_git_command("").is_err());
        assert!(parse_git_command("git-upload-pack").is_err());
        assert!(parse_git_command("git-upload-pack ''").is_err());
        assert!(parse_git_command("ls -la /tmp").is_err());
        assert!(parse_git_command("git-shell '/owner/repo.git'").is_err());
    }

    #[tokio::test]
    async fn test_load_or_create_host_key_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let key_path = dir.path().join("ssh").join("gitforge_host_ed25519");

        let generated = load_or_create_host_key(&key_path).await.unwrap();
        // The public half is published next to the private key.
        let public = tokio::fs::read_to_string(key_path.with_extension("pub"))
            .await
            .unwrap();
        assert!(public.starts_with("ssh-ed25519 "));
        // Reloading must return the same key, not mint a new one.
        let reloaded = load_or_create_host_key(&key_path).await.unwrap();
        assert_eq!(
            generated.to_openssh(LineEnding::LF).unwrap(),
            reloaded.to_openssh(LineEnding::LF).unwrap()
        );
    }
}

//! Git over SSH transport.
//!
//! A real SSH server (russh) that serves `git-upload-pack` and
//! `git-receive-pack` by piping authenticated channels to real git child
//! processes operating on the bare repositories. This is the same shape as
//! an sshd restricted to git commands: the full protocol negotiation
//! (ref advertisement, want/have, pack transfer, status report) is handled
//! by git itself, and refs move exactly as they do over Smart HTTP.
//!
//! Authentication requires a public key registered to a user account via
//! the `/api/ssh-keys` endpoints. The presented key's OpenSSH fingerprint
//! is looked up in the `ssh_keys` table; unregistered keys are rejected,
//! and every accepted fingerprint is logged with the owning account.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use gitforge_core::{FileStorageBackend, StorageBackend};
use gitforge_db::{queries::SshKeyQueries, Pool};
use russh::keys::{ssh_key::LineEnding, Algorithm, HashAlg, PrivateKey, PublicKey};
use russh::server::{Auth, Handler, Msg, Server, Session};
use russh::{Channel, MethodKind, MethodSet};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStdin};
use tokio::sync::oneshot;

use crate::ref_policy::{self, RefPolicyDecision};

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
            authenticated_user: None,
            processes: HashMap::new(),
        }
    }
}

/// A git child process serving one channel. Only stdin is kept here so
/// channel data and EOF can flow into the child; the reader task owns the
/// child itself along with its stdout and stderr.
struct ChannelProcess {
    /// Kept here so channel data can be forwarded and EOF can close stdin.
    stdin: Option<ChildStdin>,
    /// Signals the pump task to kill and reap the child on disconnect.
    cancel: Option<oneshot::Sender<()>>,
    /// Push-buffering state for receive-pack channels (#240). While set,
    /// channel data accumulates until the receive-pack command list is
    /// complete and the repository's ref-update policy has been evaluated;
    /// the bytes are only forwarded to the child afterwards.
    pending_push: Option<PendingPush>,
    push_updates: Option<Vec<crate::ref_policy::ReceiveUpdate>>,
    push_repo_id: Option<gitforge_common::RepoId>,
    push_pusher: Option<gitforge_common::UserId>,
    push_event_sender: Option<oneshot::Sender<PushEventContext>>,
    /// Set once a push has been declined by the ref-update policy: the
    /// status report is already on the wire, and any further client bytes
    /// are discarded so a large pack can never stall on a full pipe.
    declined: bool,
}

/// A receive-pack push whose command list is still arriving.
struct PendingPush {
    buffer: Vec<u8>,
    repo_id: gitforge_common::RepoId,
}

struct ProcessPump {
    handle: russh::server::Handle,
    channel: russh::ChannelId,
    child: Child,
    stdout: tokio::process::ChildStdout,
    stderr: tokio::process::ChildStderr,
    cancellation: oneshot::Receiver<()>,
    push_event: oneshot::Receiver<PushEventContext>,
    db_pool: Option<Arc<Pool>>,
    capture_receive_status: bool,
}

struct PushEventContext {
    repo_id: Option<gitforge_common::RepoId>,
    updates: Vec<crate::ref_policy::ReceiveUpdate>,
    pusher_id: Option<gitforge_common::UserId>,
}

/// Per-connection handler.
pub struct GitSshSession {
    context: Arc<SshContext>,
    /// Account the client authenticated as, set by public-key acceptance.
    authenticated_user: Option<gitforge_common::UserId>,
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
        let fingerprint = key.fingerprint(HashAlg::Sha256).to_string();
        let reject = || Auth::Reject {
            proceed_with_methods: Some(MethodSet::from(&[MethodKind::PublicKey][..])),
            partial_success: false,
        };

        let Some(pool) = self.context.db_pool.as_ref() else {
            tracing::warn!(
                user,
                %fingerprint,
                "SSH public key rejected: the key registry is unavailable"
            );
            return Ok(reject());
        };

        match SshKeyQueries::find_by_fingerprint(pool, &fingerprint).await {
            Ok(Some(record)) => {
                self.authenticated_user = Some(record.user_id);
                tracing::info!(
                    user,
                    %fingerprint,
                    user_id = %record.user_id,
                    key_name = %record.name,
                    "SSH public key accepted"
                );
                Ok(Auth::Accept)
            }
            Ok(None) => {
                tracing::warn!(user, %fingerprint, "SSH public key rejected: key is not registered to any account");
                Ok(reject())
            }
            Err(error) => {
                // Fail closed: an unusable registry must not fall back to
                // accepting possession.
                tracing::error!(%error, %fingerprint, "SSH key registry lookup failed");
                Ok(reject())
            }
        }
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
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        // A receive-pack channel buffers its command list so the ref-update
        // policy can inspect the whole push before any byte reaches the git
        // child (#240).
        let buffering = self
            .processes
            .get(&channel)
            .is_some_and(|process| process.pending_push.is_some());
        if buffering {
            return self.receive_push_data(channel, data, session).await;
        }
        // A declined push has already been answered with its status report;
        // drop the remaining pack bytes instead of backing them up behind a
        // child that will never read them.
        if self
            .processes
            .get(&channel)
            .is_some_and(|process| process.declined)
        {
            return Ok(());
        }
        if let Some(process) = self.processes.get_mut(&channel) {
            let Some(stdin) = process.stdin.as_mut() else {
                return Ok(());
            };
            if let Err(error) = stdin.write_all(data).await {
                tracing::warn!(
                    ?channel,
                    %error,
                    "failed to forward SSH channel data to git process"
                );
                self.stop_process(channel);
            }
        }
        Ok(())
    }

    async fn channel_eof(
        &mut self,
        channel: russh::ChannelId,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        if let Some(process) = self.processes.get_mut(&channel) {
            // Dropping stdin signals EOF to the git child process. Normal
            // EOF must reach git so a valid receive-pack can finish; it is
            // not a disconnect and must not cancel the child.
            process.stdin.take();
            // A push that never delivered its command list is malformed: the
            // buffered prefix was never forwarded, and closing stdin makes
            // receive-pack fail instead of waiting forever.
            if process.pending_push.is_some() {
                process.pending_push = None;
            }
            if let Some(sender) = process.push_event_sender.take() {
                let _ = sender.send(PushEventContext {
                    updates: process.push_updates.take().unwrap_or_default(),
                    repo_id: process.push_repo_id.take(),
                    pusher_id: process.push_pusher,
                });
            }
        }
        Ok(())
    }

    async fn channel_close(
        &mut self,
        channel: russh::ChannelId,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.stop_process(channel);
        Ok(())
    }
}

impl GitSshSession {
    /// Close the child's stdin, signal cancellation, and let the pump task
    /// kill and await the child so it cannot become a zombie.
    fn stop_process(&mut self, channel: russh::ChannelId) {
        if let Some(mut process) = self.processes.remove(&channel) {
            drop(process.stdin.take());
            if let Some(cancel) = process.cancel.take() {
                let _ = cancel.send(());
            }
        }
    }

    /// Accumulate a receive-pack command list, evaluate the repository's
    /// ref-update policy once it is complete (#240), and either decline the
    /// push with a standard receive-pack status report or hand the buffered
    /// bytes to the already-running child.
    async fn receive_push_data(
        &mut self,
        channel: russh::ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), russh::Error> {
        // Take the buffer out, append, and put it back while the command
        // list is still incomplete.
        let buffer = match self
            .processes
            .get_mut(&channel)
            .and_then(|process| process.pending_push.as_mut())
        {
            Some(pending) => {
                pending.buffer.extend_from_slice(data);
                std::mem::take(&mut pending.buffer)
            }
            None => return Ok(()),
        };

        if buffer.len() > ref_policy::MAX_COMMAND_LIST_BYTES {
            tracing::warn!(
                ?channel,
                buffer_len = buffer.len(),
                "push command list exceeded buffer bound; aborting channel"
            );
            return self
                .fail_push(channel, "push command list too large", session)
                .await;
        }

        let Some(_list_len) = ref_policy::command_list_len(&buffer) else {
            // The command list is still incomplete; keep buffering.
            if let Some(process) = self.processes.get_mut(&channel) {
                if let Some(pending) = process.pending_push.as_mut() {
                    pending.buffer = buffer;
                }
            }
            return Ok(());
        };

        let Some(repo_id) = self
            .processes
            .get(&channel)
            .and_then(|process| process.pending_push.as_ref())
            .map(|pending| pending.repo_id)
        else {
            return Ok(());
        };
        let Some(pool) = self.context.db_pool.clone() else {
            // resolve_repo_id already refuses SSH sessions without a
            // database, so this only fires if the context was rebuilt.
            return self
                .fail_push(channel, "database not available", session)
                .await;
        };
        let updates = ref_policy::parse_receive_updates(&buffer);
        let authenticated_user = self.authenticated_user;
        match ref_policy::evaluate_ref_policy(&pool, repo_id, &updates).await {
            RefPolicyDecision::Allow => {}
            RefPolicyDecision::Reject(reasons) => {
                for (ref_name, reason) in &reasons {
                    tracing::warn!(
                        repo_id = %repo_id,
                        ref_name = %ref_name,
                        reason = %reason,
                        "push declined by ref-update policy over SSH"
                    );
                }
                return self.send_push_report(channel, &reasons, session).await;
            }
        }

        // Policy allows: hand the buffered prefix — the command list plus
        // any pack bytes that arrived with it — to the receive-pack child
        // that has been serving the advertisement since the channel opened.
        if let Some(process) = self.processes.get_mut(&channel) {
            process.pending_push = None;
            process.push_updates = Some(updates);
            process.push_repo_id = Some(repo_id);
            process.push_pusher = authenticated_user;
            if let Some(stdin) = process.stdin.as_mut() {
                if let Err(error) = stdin.write_all(&buffer).await {
                    tracing::warn!(?channel, %error, "failed to replay buffered push data");
                    self.stop_process(channel);
                }
            }
        }
        Ok(())
    }

    /// Send a receive-pack status report declining refs, then close the
    /// channel so git renders `! [remote rejected] <ref> (reason)`. The
    /// child is cancelled quietly (the pump kills and reaps it without
    /// sending a second exit status); the entry stays registered as
    /// `declined` so the client can finish streaming its pack into a
    /// discard path instead of stalling on a pipe nobody reads.
    async fn send_push_report(
        &mut self,
        channel: russh::ChannelId,
        reasons: &[(String, String)],
        session: &mut Session,
    ) -> Result<(), russh::Error> {
        if let Some(process) = self.processes.get_mut(&channel) {
            process.pending_push = None;
            process.declined = true;
            drop(process.stdin.take());
            if let Some(cancel) = process.cancel.take() {
                let _ = cancel.send(());
            }
        }
        let handle = session.handle();
        let _ = handle
            .data(channel, ref_policy::synthesize_rejection_report(reasons))
            .await;
        // Refusal is a normal protocol outcome, not a server failure: exit 0
        // with per-ref declines, like a real pre-receive hook rejection.
        let _ = handle.exit_status_request(channel, 0).await;
        let _ = handle.eof(channel).await;
        let _ = handle.close(channel).await;
        Ok(())
    }

    /// Abort a push channel with a diagnostic on stderr and a failed exit
    /// status, killing the child so no partial push can land.
    async fn fail_push(
        &mut self,
        channel: russh::ChannelId,
        message: &str,
        session: &mut Session,
    ) -> Result<(), russh::Error> {
        self.stop_process(channel);
        let handle = session.handle();
        let _ = handle
            .extended_data(channel, 1, format!("gitforge: {message}\n").into_bytes())
            .await;
        let _ = handle.exit_status_request(channel, 128).await;
        let _ = handle.eof(channel).await;
        let _ = handle.close(channel).await;
        Ok(())
    }

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

        session
            .handle()
            .channel_success(channel)
            .await
            .map_err(|()| format!("channel {channel:?} already closed"))?;

        let mut child = tokio::process::Command::new("git")
            .arg(git_command)
            .arg(&repo_disk_path)
            .kill_on_drop(true)
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

        let (cancel, cancellation) = oneshot::channel();
        let (push_event_sender, push_event_receiver) = oneshot::channel();
        // For receive-pack, stdin forwarding is held back until the command
        // list is complete and the ref-update policy (#240) has approved the
        // push. The child itself starts immediately: an interactive receive
        // pack advertises its refs before the client sends anything, so
        // delaying the process would deadlock the push.
        let pending_push = (git_command == "receive-pack").then(|| PendingPush {
            buffer: Vec::new(),
            repo_id,
        });
        if pending_push.is_some() {
            tracing::info!(
                command = %command.trim(),
                %repo_path,
                user_id = ?self.authenticated_user,
                "buffering push command list for ref-update policy"
            );
        }
        self.processes.insert(
            channel,
            ChannelProcess {
                stdin: Some(stdin),
                cancel: Some(cancel),
                pending_push,
                declined: false,
                push_updates: None,
                push_repo_id: None,
                push_pusher: None,
                push_event_sender: (git_command == "receive-pack").then_some(push_event_sender),
            },
        );

        let handle = session.handle();
        let capture_receive_status = git_command == "receive-pack";
        tokio::spawn(pump_until_exit(ProcessPump {
            handle,
            channel,
            child,
            stdout,
            stderr,
            cancellation,
            push_event: push_event_receiver,
            db_pool: self.context.db_pool.clone(),
            capture_receive_status,
        }));
        tracing::info!(
            command = %command.trim(),
            %repo_path,
            user_id = ?self.authenticated_user,
            "git process started"
        );
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
async fn pump_until_exit(pump: ProcessPump) {
    let ProcessPump {
        handle,
        channel,
        mut child,
        stdout,
        stderr,
        mut cancellation,
        push_event,
        db_pool,
        capture_receive_status,
    } = pump;
    let data_handle = handle.clone();
    let mut out_task = tokio::spawn(async move {
        if capture_receive_status {
            Some(pipe_receive_status_to_channel(data_handle, channel, stdout).await)
        } else {
            pipe_to_channel(data_handle, channel, stdout, false).await;
            None
        }
    });
    let error_handle = handle.clone();
    let mut err_task = tokio::spawn(async move {
        pipe_to_channel(error_handle, channel, stderr, true).await;
    });
    let receive_status = tokio::select! {
        _ = &mut cancellation => {
            out_task.abort();
            err_task.abort();
            reap_cancelled_child(&mut child).await;
            return;
        }
        result = async {
            let _ = (&mut err_task).await;
            (&mut out_task).await.ok().flatten()
        } => result,
    };

    let status = tokio::select! {
        result = child.wait() => result.ok().and_then(|status| status.code()).unwrap_or(-1),
        _ = &mut cancellation => {
            reap_cancelled_child(&mut child).await;
            return;
        }
    };
    if status == 0 {
        if let (Ok(push_event), Some(pool)) = (push_event.await, db_pool) {
            let pusher_id = push_event.pusher_id;
            if let Some(repo_id) = push_event.repo_id {
                let accepted_refs = receive_status
                    .as_deref()
                    .and_then(parse_receive_status_ok_refs)
                    .unwrap_or_default();
                for update in push_event.updates {
                    if !accepted_refs.contains(&update.ref_name) {
                        continue;
                    }
                    if let Err(error) =
                        crate::enqueue_ci_event(&pool, repo_id, &update, pusher_id).await
                    {
                        tracing::error!(?channel, ref_name = %update.ref_name, %error,
                            "SSH CI trigger insert deferred to background redelivery");
                    }
                }
            }
        }
    }
    let _ = handle.exit_status_request(channel, status as u32).await;
    let _ = handle.eof(channel).await;
    let _ = handle.close(channel).await;
}

/// Read the bounded receive-pack status report while forwarding it unchanged
/// to the SSH client. The report is small; refusing to enqueue if it exceeds
/// the cap is safer than guessing which refs receive-pack accepted.
async fn pipe_receive_status_to_channel(
    handle: russh::server::Handle,
    channel: russh::ChannelId,
    mut pipe: impl tokio::io::AsyncRead + Unpin,
) -> Vec<u8> {
    const MAX_STATUS_BYTES: usize = 1024 * 1024;
    let mut status = Vec::new();
    let mut overflowed = false;
    let mut buffer = vec![0u8; 16 * 1024];
    loop {
        match pipe.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let chunk = buffer[..n].to_vec();
                if !overflowed && status.len() + n <= MAX_STATUS_BYTES {
                    status.extend_from_slice(&buffer[..n]);
                } else {
                    overflowed = true;
                    status.clear();
                }
                if handle.data(channel, chunk).await.is_err() {
                    break;
                }
            }
        }
    }
    if overflowed {
        Vec::new()
    } else {
        status
    }
}

fn parse_receive_status_ok_refs(status: &[u8]) -> Option<std::collections::HashSet<String>> {
    let mut accepted = std::collections::HashSet::new();
    let mut offset = 0;
    let mut saw_unpack_ok = false;
    while offset + 4 <= status.len() {
        let length =
            usize::from_str_radix(std::str::from_utf8(&status[offset..offset + 4]).ok()?, 16)
                .ok()?;
        if length == 0 {
            if saw_unpack_ok {
                return Some(accepted);
            }
            // receive-pack stdout includes a ref advertisement section before
            // its post-push status report. Skip its flush and keep parsing.
            offset += 4;
            continue;
        }
        if length < 4 || offset + length > status.len() {
            return None;
        }
        let payload = &status[offset + 4..offset + length];
        let is_sideband_data = payload.first() == Some(&1);
        let payload = payload.strip_prefix(&[1]).unwrap_or(payload);

        // Side-band channel 1 carries a second pkt-line stream inside the
        // outer side-band packet. Parse those inner report-status packets;
        // treating their four-byte lengths as line text silently loses every
        // accepted ref even though receive-pack reports success to the client.
        let mut inner_offset = 0;
        let mut found_inner_packet = false;
        while is_sideband_data
            && inner_offset + 4 <= payload.len()
            && payload[inner_offset..inner_offset + 4]
                .iter()
                .all(u8::is_ascii_hexdigit)
        {
            let inner_length = usize::from_str_radix(
                std::str::from_utf8(&payload[inner_offset..inner_offset + 4]).ok()?,
                16,
            )
            .ok()?;
            if inner_length == 0 {
                found_inner_packet = true;
                if saw_unpack_ok {
                    return Some(accepted);
                }
                inner_offset += 4;
                continue;
            }
            if inner_length < 4 || inner_offset + inner_length > payload.len() {
                break;
            }
            found_inner_packet = true;
            for line in
                payload[inner_offset + 4..inner_offset + inner_length].split(|byte| *byte == b'\n')
            {
                if line == b"unpack ok" {
                    saw_unpack_ok = true;
                } else if let Some(ref_name) = line.strip_prefix(b"ok ") {
                    accepted.insert(std::str::from_utf8(ref_name).ok()?.to_string());
                }
            }
            inner_offset += inner_length;
        }
        if !found_inner_packet {
            for line in payload.split(|byte| *byte == b'\n') {
                if line == b"unpack ok" {
                    saw_unpack_ok = true;
                } else if let Some(ref_name) = line.strip_prefix(b"ok ") {
                    accepted.insert(std::str::from_utf8(ref_name).ok()?.to_string());
                }
            }
        }
        offset += length;
    }
    None
}

/// Kill a disconnected git child and await it so it cannot remain a zombie.
async fn reap_cancelled_child(child: &mut Child) {
    let _ = child.kill().await;
    let _ = child.wait().await;
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
        () = wait_for_shutdown_flag(shutdown) => {
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

    #[test]
    fn test_receive_status_only_accepts_reported_refs() {
        let status = b"000eunpack ok\n0017ok refs/heads/main\n001dng refs/heads/bad denied\n0000";
        let accepted = parse_receive_status_ok_refs(status).expect("valid status report");
        assert!(accepted.contains("refs/heads/main"));
        assert!(!accepted.contains("refs/heads/bad"));
        assert!(parse_receive_status_ok_refs(b"0000").is_none());
    }

    #[test]
    fn test_receive_status_skips_initial_advertisement_flush() {
        let status = b"000eversion 1\n0000002e\x01000eunpack ok\n0017ok refs/heads/main\n0000";
        let accepted = parse_receive_status_ok_refs(status).expect("valid post-push status");
        assert!(accepted.contains("refs/heads/main"));
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

    #[tokio::test]
    async fn test_reap_cancelled_child_kills_and_waits() {
        let mut child = tokio::process::Command::new("sh")
            .args(["-c", "sleep 60"])
            .spawn()
            .unwrap();

        reap_cancelled_child(&mut child).await;

        assert!(child.try_wait().unwrap().is_some());
    }
}

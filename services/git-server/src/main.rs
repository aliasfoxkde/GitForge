//! GitForge Git Server
//!
//! Main entry point for the Git SSH/HTTP server.

use axum::{
    body::Body,
    extract::{Path, State},
    http::{Request, StatusCode},
    response::Response,
    routing::{get, post},
    Router,
};
use gitforge_common::RepoId;
use gitforge_core::git_protocol::{http::HttpGitHandler, ssh::SshGitHandler, GitProtocolHandler};
use gitforge_core::{FileStorageBackend, RepoService, StorageBackend};
use gitforge_events::{EventBus, InMemoryEventBus};
use gitforge_process::{create_shutdown_flag, spawn_shutdown_handler, wait_for_shutdown};
use sha2::{Digest, Sha256};
#[allow(unused_imports)]
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;
use tower_http::trace::TraceLayer;
use uuid::Uuid;

/// Application state shared across handlers
#[derive(Clone)]
struct AppState {
    http_handler: Arc<HttpGitHandler<FileStorageBackend>>,
    storage: Arc<FileStorageBackend>,
}

/// SSH server configuration
struct SshServerConfig {
    port: u16,
    storage: Arc<FileStorageBackend>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .init();

    tracing::info!("starting GitForce Git Server");

    // Initialize process supervision (subreaper + SIGCHLD) to prevent zombies
    if let Err(e) = gitforge_process::init() {
        tracing::warn!("failed to initialize process supervision: {}", e);
    }

    // Get ports from environment
    let http_port: u16 = std::env::var("HTTP_PORT")
        .unwrap_or_else(|_| "42782".to_string())
        .parse()
        .unwrap_or(42782);
    let ssh_port: u16 = std::env::var("SSH_PORT")
        .unwrap_or_else(|_| "42022".to_string())
        .parse()
        .unwrap_or(42022);

    // Get git root from environment
    let git_root = get_git_root();
    tracing::info!("using git root: {}", git_root);

    // Initialize storage
    let storage = Arc::new(FileStorageBackend::new(&git_root));
    storage.ensure_root().await?;

    // Initialize repository service
    let _repo_service = Arc::new(RepoService::new((*storage).clone()));

    // Initialize event bus
    let _event_bus: Arc<dyn EventBus> = Arc::new(InMemoryEventBus::new());

    tracing::info!("Git Server initialized successfully");

    // Create HTTP handler
    let http_handler = Arc::new(HttpGitHandler::new((*storage).clone()));

    // Create app state
    let state = AppState {
        http_handler,
        storage: storage.clone(),
    };

    // Build router for Git HTTP protocol
    let app = Router::new()
        .route("/health", get(health_check))
        .route("/git-upload-pack/{owner}/{repo}", get(git_upload_pack))
        .route(
            "/git-upload-pack/{owner}/{repo}/{*path}",
            get(git_upload_pack_path),
        )
        .route("/git-receive-pack/{owner}/{repo}", post(git_receive_pack))
        .route(
            "/git-receive-pack/{owner}/{repo}/{*path}",
            post(git_receive_pack_path),
        )
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    // Start HTTP server
    let http_addr = format!("0.0.0.0:{}", http_port);
    tracing::info!("starting Git HTTP server on {}", http_addr);

    let http_listener = tokio::net::TcpListener::bind(&http_addr).await?;
    tracing::info!("Git HTTP server listening on {}", http_addr);

    // Spawn HTTP server
    let http_handle = tokio::spawn(async move {
        axum::serve(http_listener, app).await.unwrap();
    });

    // Start SSH server for Git operations
    let ssh_config = SshServerConfig {
        port: ssh_port,
        storage: storage.clone(),
    };
    let shutdown = create_shutdown_flag();
    let shutdown_flag = shutdown.clone();

    let ssh_handle = tokio::spawn(async move {
        if let Err(e) = run_ssh_server(ssh_config, shutdown_flag).await {
            tracing::error!("SSH server error: {}", e);
        }
    });

    // Set up shutdown handling
    let shutdown = create_shutdown_flag();
    let shutdown_flag = shutdown.clone();

    // Spawn graceful shutdown handler
    spawn_shutdown_handler(shutdown_flag);

    tracing::info!("Git Server running, press Ctrl+C to stop");

    // Wait for shutdown signal
    let shutdown_future = create_shutdown_future(shutdown.clone());
    timeout(Duration::MAX, shutdown_future).await.ok();

    tracing::info!("shutting down Git Server");

    // Cancel HTTP and SSH servers
    http_handle.abort();
    ssh_handle.abort();

    // Graceful shutdown delay
    graceful_shutdown_delay().await;

    tracing::info!("Git Server stopped");
    Ok(())
}

/// Health check handler
async fn health_check() -> &'static str {
    "OK"
}

/// Derive a deterministic RepoId from owner/repo path
fn derive_repo_id(owner: &str, repo: &str) -> RepoId {
    // Create a deterministic ID based on the path
    let input = format!("{}/{}", owner, repo);
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    let result = hasher.finalize();
    // Use first 16 bytes to create a Uuid, then convert to RepoId
    let bytes: [u8; 16] = result[..16].try_into().unwrap();
    let uuid = Uuid::from_bytes(bytes);
    RepoId::from(uuid)
}

/// Git upload-pack handler (GET) - returns ref advertisement
async fn git_upload_pack(
    Path((owner, repo)): Path<(String, String)>,
    State(state): State<AppState>,
) -> Response {
    let repo_path = format!("{}/{}", owner, repo);
    let repo_id = derive_repo_id(&owner, &repo);

    // Check if repository exists
    if !state.storage.exists(repo_id).await {
        tracing::warn!("repository not found: {}", repo_path);
        return Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Body::from(format!("Repository not found: {}", repo_path)))
            .unwrap();
    }

    match state.http_handler.upload_pack(repo_id, vec![]).await {
        Ok(response) => {
            let mut res = Response::builder()
                .status(StatusCode::OK)
                .header(
                    "Content-Type",
                    "application/x-git-upload-pack-advertisement",
                )
                .body(Body::from(response))
                .unwrap();
            res.headers_mut().insert(
                "Cache-Control",
                axum::http::HeaderValue::from_static("no-cache"),
            );
            res
        }
        Err(e) => {
            tracing::warn!("upload-pack failed for {}: {}", repo_path, e);
            Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Body::from(format!("Error: {}", e)))
                .unwrap()
        }
    }
}

/// Git upload-pack handler with additional path
async fn git_upload_pack_path(
    Path((owner, repo, _path)): Path<(String, String, String)>,
    State(state): State<AppState>,
) -> Response {
    git_upload_pack(Path((owner, repo)), State(state)).await
}

/// Git receive-pack handler (POST) - receives pack data
async fn git_receive_pack(
    Path((owner, repo)): Path<(String, String)>,
    State(state): State<AppState>,
    request: Request<Body>,
) -> Response {
    let repo_path = format!("{}/{}", owner, repo);
    let repo_id = derive_repo_id(&owner, &repo);

    // Check if repository exists
    if !state.storage.exists(repo_id).await {
        tracing::warn!("repository not found: {}", repo_path);
        return Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Body::from(format!("Repository not found: {}", repo_path)))
            .unwrap();
    }

    // Read request body
    let body = axum::body::to_bytes(request.into_body(), 10 * 1024 * 1024)
        .await
        .unwrap_or_default();

    match state
        .http_handler
        .receive_pack(repo_id, body.to_vec())
        .await
    {
        Ok(response) => Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "application/x-git-receive-pack-result")
            .body(Body::from(response))
            .unwrap(),
        Err(e) => {
            tracing::warn!("receive-pack failed for {}: {}", repo_path, e);
            Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Body::from(format!("Error: {}", e)))
                .unwrap()
        }
    }
}

/// Git receive-pack handler with additional path
async fn git_receive_pack_path(
    Path((owner, repo, _path)): Path<(String, String, String)>,
    State(state): State<AppState>,
    request: Request<Body>,
) -> Response {
    git_receive_pack(Path((owner, repo)), State(state), request).await
}

/// Get the git root directory from environment or use default
pub fn get_git_root() -> String {
    std::env::var("GIT_ROOT").unwrap_or_else(|_| "/var/lib/gitforge/repos".to_string())
}

/// Create the shutdown future that waits for shutdown signal
pub async fn create_shutdown_future(shutdown: Arc<AtomicBool>) {
    wait_for_shutdown(shutdown).await;
}

/// Perform graceful shutdown delay
pub async fn graceful_shutdown_delay() {
    timeout(Duration::from_secs(2), async {
        tokio::time::sleep(Duration::from_secs(1)).await;
    })
    .await
    .ok();
}

/// Run the SSH server for Git operations
async fn run_ssh_server(config: SshServerConfig, shutdown: Arc<AtomicBool>) -> anyhow::Result<()> {
    use std::net::SocketAddr;

    tracing::info!("starting Git SSH server on port {}", config.port);

    // Create TCP listener
    let addr = SocketAddr::from(([0, 0, 0, 0], config.port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("Git SSH server listening on {}", addr);

    // Create SSH handler wrapped in Arc for sharing across connections
    let ssh_handler = Arc::new(SshGitHandler::new((*config.storage).clone()));

    loop {
        if shutdown.load(Ordering::SeqCst) {
            tracing::info!("SSH server shutting down");
            break;
        }

        // Accept connection with timeout to allow checking shutdown flag
        let accept_result = tokio::time::timeout(
            Duration::from_secs(1),
            listener.accept(),
        )
        .await;

        match accept_result {
            Ok(Ok((stream, peer_addr))) => {
                tracing::debug!("SSH connection from {}", peer_addr);

                // Clone handler and storage for this connection
                let handler = ssh_handler.clone();
                let storage = config.storage.clone();

                // Handle connection in blocking task since ssh2 is sync
                tokio::task::spawn_blocking(move || {
                    handle_ssh_connection(stream, handler, storage);
                });
            }
            Ok(Err(e)) => {
                tracing::warn!("SSH accept error: {}", e);
            }
            Err(_) => {
                // Timeout - continue loop to check shutdown flag
                continue;
            }
        }
    }

    Ok(())
}

/// Handle a single SSH connection
#[allow(clippy::too_many_lines)]
fn handle_ssh_connection(
    stream: tokio::net::TcpStream,
    handler: Arc<SshGitHandler<FileStorageBackend>>,
    _storage: Arc<FileStorageBackend>,
) {
    use std::io::{Read, Write};

    // Convert tokio TcpStream to blocking TcpStream
    let mut stream = match stream.into_std() {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("failed to convert TcpStream: {}", e);
            return;
        }
    };

    // Create ssh2 session
    let mut session = match ssh2::Session::new() {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("failed to create SSH session: {}", e);
            return;
        }
    };

    // Set blocking mode for ssh2
    session.set_blocking(true);

    // Handshake
    if let Err(e) = session.handshake() {
        tracing::error!("SSH handshake failed: {}", e);
        return;
    }

    // Check if peer is authenticated - Git over SSH typically uses public-key auth
    // For MVP, we'll accept None auth and skip detailed auth verification
    let authenticated = session.authenticated();
    if !authenticated {
        tracing::warn!("SSH session not authenticated - Git operations may fail");
    }

    // Accept a single channel for git command
    match session.channel_session() {
        Ok(mut channel) => {
            // Request exec to run the git command
            // Read the command that was passed
            let mut cmd_buf = [0u8; 4096];
            let cmd_len = match channel.read(&mut cmd_buf) {
                Ok(len) => len,
                Err(e) => {
                    tracing::error!("failed to read from channel: {}", e);
                    return;
                }
            };

            if cmd_len == 0 {
                return;
            }

            let cmd = String::from_utf8_lossy(&cmd_buf[..cmd_len]).to_string();
            tracing::debug!("received SSH command: {}", cmd);

            // Parse the git command (e.g., "git-upload-pack /owner/repo.git" or just "git-upload-pack")
            let parts: Vec<&str> = cmd.split_whitespace().collect();
            if parts.is_empty() {
                tracing::warn!("empty SSH command");
                let _ = channel.write_all(b"empty command\n");
                channel.wait_close().ok();
                return;
            }

            let git_cmd = parts[0];
            let repo_path = if parts.len() > 1 {
                parts[1].trim_start_matches('/')
            } else {
                ""
            };

            // Parse owner/repo from path
            let path_parts: Vec<&str> = repo_path.split('/').collect();
            if path_parts.len() < 2 {
                tracing::warn!("invalid repo path: {}", repo_path);
                let _ = channel.write_all(b"invalid repository path\n");
                channel.wait_close().ok();
                return;
            }

            let owner = path_parts[0];
            let repo = path_parts[1].trim_end_matches(".git").trim_end_matches("/");

            // Derive repo ID
            let input = format!("{}/{}", owner, repo);
            let mut hasher = Sha256::new();
            hasher.update(input.as_bytes());
            let result = hasher.finalize();
            let bytes: [u8; 16] = result[..16].try_into().unwrap();
            let uuid = Uuid::from_bytes(bytes);
            let repo_id = RepoId::from(uuid);

            // Process based on command
            let response = match git_cmd {
                "git-upload-pack" => {
                    tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .unwrap()
                        .block_on(handler.upload_pack(repo_id, vec![]))
                }
                "git-receive-pack" => {
                    // For receive-pack, we need to read the request body
                    let mut input = Vec::new();
                    std::io::Read::read_to_end(&mut stream, &mut input).ok();
                    tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .unwrap()
                        .block_on(handler.receive_pack(repo_id, input))
                }
                _ => {
                    Err(gitforge_common::Error::git(format!("unsupported command: {}", git_cmd)))
                }
            };

            match response {
                Ok(data) => {
                    let _ = channel.write_all(&data);
                }
                Err(e) => {
                    tracing::error!("git command failed: {}", e);
                    let _ = channel.write_all(format!("error: {}\n", e).as_bytes());
                }
            }

            channel.wait_close().ok();
        }
        Err(e) => {
            tracing::error!("failed to open channel: {}", e);
        }
    }

    let _ = session.disconnect(None, "closing", None);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_git_root_default() {
        std::env::remove_var("GIT_ROOT");
        let root = get_git_root();
        assert_eq!(root, "/var/lib/gitforge/repos");
    }

    #[test]
    fn test_get_git_root_from_env() {
        std::env::set_var("GIT_ROOT", "/custom/path");
        let root = get_git_root();
        assert_eq!(root, "/custom/path");
        std::env::remove_var("GIT_ROOT");
    }

    #[test]
    fn test_create_shutdown_flag_initial_state() {
        let flag = create_shutdown_flag();
        assert!(!flag.load(Ordering::SeqCst));
    }

    #[test]
    fn test_create_shutdown_flag_clone() {
        let flag1 = create_shutdown_flag();
        let flag2 = flag1.clone();
        flag1.store(true, Ordering::SeqCst);
        assert!(flag2.load(Ordering::SeqCst));
    }

    #[test]
    fn test_graceful_shutdown_delay_does_not_panic() {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                graceful_shutdown_delay().await;
            });
    }

    #[tokio::test]
    async fn test_create_shutdown_future() {
        let shutdown = create_shutdown_flag();
        let shutdown_flag = shutdown.clone();

        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            shutdown_flag.store(true, Ordering::SeqCst);
        });

        create_shutdown_future(shutdown).await;
    }

    #[tokio::test]
    async fn test_spawn_shutdown_handler_does_not_panic() {
        let flag = create_shutdown_flag();
        spawn_shutdown_handler(flag);
    }

    #[test]
    fn test_shutdown_flag_is_atomic() {
        let flag = create_shutdown_flag();
        assert!(!flag.load(Ordering::SeqCst));
        flag.store(true, Ordering::SeqCst);
        assert!(flag.load(Ordering::SeqCst));
    }

    #[test]
    fn test_shutdown_flag_load_ordering() {
        let flag = create_shutdown_flag();
        let value = flag.load(Ordering::SeqCst);
        assert!(!value);
    }

    #[test]
    fn test_graceful_shutdown_delay_completes() {
        let start = std::time::Instant::now();
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                graceful_shutdown_delay().await;
            });
        assert!(start.elapsed().as_secs() >= 1);
    }

    #[test]
    fn test_get_git_root_empty_string() {
        std::env::set_var("GIT_ROOT", "");
        let _root = get_git_root();
        std::env::remove_var("GIT_ROOT");
    }
}

//! GitForce Git Server
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
use gitforce_common::RepoId;
use gitforce_core::git_protocol::{http::HttpGitHandler, GitProtocolHandler};
use gitforce_core::{FileStorageBackend, RepoService};
use gitforce_events::{EventBus, InMemoryEventBus};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::signal;
use tokio::time::timeout;
use tower_http::trace::TraceLayer;

/// Application state shared across handlers
#[derive(Clone)]
struct AppState {
    http_handler: Arc<HttpGitHandler<FileStorageBackend>>,
    #[allow(dead_code)]
    repo_service: Arc<RepoService<FileStorageBackend>>,
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
    if let Err(e) = gitforce_process::init() {
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
    let repo_service = Arc::new(RepoService::new((*storage).clone()));

    // Initialize event bus
    let _event_bus: Arc<dyn EventBus> = Arc::new(InMemoryEventBus::new());

    tracing::info!("Git Server initialized successfully");

    // Create HTTP handler
    let http_handler = Arc::new(HttpGitHandler::new((*storage).clone()));

    // Create app state
    let state = AppState {
        http_handler,
        repo_service,
    };

    // Build router for Git HTTP protocol
    let app = Router::new()
        .route("/health", get(health_check))
        .route("/git-upload-pack/:owner/:repo", get(git_upload_pack))
        .route(
            "/git-upload-pack/:owner/:repo/*path",
            get(git_upload_pack_path),
        )
        .route("/git-receive-pack/:owner/:repo", post(git_receive_pack))
        .route(
            "/git-receive-pack/:owner/:repo/*path",
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

    // SSH server
    // Note: SSH Git protocol requires russh integration which has API compatibility issues
    // with the current crate version. SSH Git support is planned for a future release.
    tracing::info!(
        "Git SSH server on port {} (SSH support pending russh API resolution)",
        ssh_port
    );

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

    // Cancel HTTP server
    http_handle.abort();

    // Graceful shutdown delay
    graceful_shutdown_delay().await;

    tracing::info!("Git Server stopped");
    Ok(())
}

/// Health check handler
async fn health_check() -> &'static str {
    "OK"
}

/// Git upload-pack handler (GET) - returns ref advertisement
async fn git_upload_pack(
    Path((owner, repo)): Path<(String, String)>,
    State(state): State<AppState>,
) -> Response {
    let repo_path = format!("{}/{}", owner, repo);

    // Try to look up repo by name - for now, just use the path as repo_id
    // In a full implementation, this would query the database
    let repo_id = RepoId::new(); // Placeholder - needs proper lookup

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
                .status(StatusCode::NOT_FOUND)
                .body(Body::from(format!("Repository not found: {}", repo_path)))
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
    let repo_id = RepoId::new(); // Placeholder - needs proper lookup

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

/// Create a shutdown flag
pub fn create_shutdown_flag() -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(false))
}

/// Spawn the shutdown signal handler (Unix-only)
#[cfg(unix)]
pub fn spawn_shutdown_handler(shutdown_flag: Arc<AtomicBool>) {
    tokio::spawn(async move {
        let mut sigterm = signal::unix::signal(signal::unix::SignalKind::terminate()).unwrap();
        let mut sigint = signal::unix::signal(signal::unix::SignalKind::interrupt()).unwrap();

        tokio::select! {
            _ = sigterm.recv() => {
                tracing::info!("received SIGTERM, initiating graceful shutdown...");
            }
            _ = sigint.recv() => {
                tracing::info!("received SIGINT, initiating graceful shutdown...");
            }
        }
        shutdown_flag.store(true, Ordering::SeqCst);
    });
}

/// Spawn the shutdown signal handler (Windows stub)
#[cfg(windows)]
pub fn spawn_shutdown_handler(_shutdown_flag: Arc<AtomicBool>) {}

/// Create the shutdown future that waits for shutdown signal
pub async fn create_shutdown_future(shutdown: Arc<AtomicBool>) {
    while !shutdown.load(Ordering::SeqCst) {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Perform graceful shutdown delay
pub async fn graceful_shutdown_delay() {
    timeout(Duration::from_secs(2), async {
        tokio::time::sleep(Duration::from_secs(1)).await;
    })
    .await
    .ok();
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

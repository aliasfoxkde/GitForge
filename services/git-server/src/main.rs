//! GitForge Git Server
//!
//! Main entry point for the Git SSH/HTTP server.

use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{Request, StatusCode},
    response::Response,
    routing::{get, post},
    Router,
};
use chrono::Utc;
use gitforge_common::RepoId;
use gitforge_core::git_protocol::{http::HttpGitHandler, ssh::SshGitHandler, GitProtocolHandler};
use gitforge_core::{FileStorageBackend, RepoService, StorageBackend};
use gitforge_db::Pool;
use gitforge_events::{EventBus, InMemoryEventBus};
use gitforge_process::{create_shutdown_flag, spawn_shutdown_handler, wait_for_shutdown};
#[allow(unused_imports)]
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;
use tower_http::trace::TraceLayer;

/// Application state shared across handlers
#[derive(Clone)]
struct AppState {
    http_handler: Arc<HttpGitHandler<FileStorageBackend>>,
    storage: Arc<FileStorageBackend>,
    db_pool: Option<Arc<Pool>>,
    ci_trigger_url: Option<String>,
    ci_trigger_token: Option<String>,
    http_client: reqwest::Client,
}

/// SSH server configuration
struct SshServerConfig {
    port: u16,
    storage: Arc<FileStorageBackend>,
    db_pool: Option<Arc<Pool>>,
}

#[derive(Debug, serde::Deserialize)]
struct InfoRefsQuery {
    service: Option<String>,
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

    // Initialize subreaper support without a global waitpid loop. Child
    // ownership must remain with the runtime that spawned it.
    if let Err(e) = gitforge_process::init_without_sigchld_reaper() {
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

    // Initialize database pool (optional - git operations can work without it for local repos)
    let db_pool = match std::env::var("DATABASE_URL") {
        Ok(url) => match Pool::new(&url).await {
            Ok(pool) => {
                if let Err(e) = pool.migrate().await {
                    tracing::warn!("database migration failed: {}", e);
                }
                Some(Arc::new(pool))
            }
            Err(e) => {
                tracing::warn!("failed to create database pool: {}", e);
                None
            }
        },
        Err(_) => {
            tracing::info!("DATABASE_URL not set, running without database lookup");
            None
        }
    };

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

    // Save db_pool before moving state into router
    let saved_db_pool = db_pool.clone();

    // Create app state
    let state = AppState {
        http_handler,
        storage: storage.clone(),
        db_pool: saved_db_pool.clone(),
        ci_trigger_url: std::env::var("GITFORGE_CI_TRIGGER_URL").ok(),
        ci_trigger_token: std::env::var("GITFORGE_CI_TRIGGER_TOKEN").ok(),
        http_client: reqwest::Client::new(),
    };

    if state.ci_trigger_url.is_none() || state.ci_trigger_token.is_none() {
        tracing::warn!(
            "GITFORGE_CI_TRIGGER_URL/TOKEN is not fully configured; pushes will not trigger CI"
        );
    }

    if state.db_pool.is_some() && state.ci_trigger_url.is_some() && state.ci_trigger_token.is_some()
    {
        let delivery_state = state.clone();
        tokio::spawn(async move { ci_delivery_loop(delivery_state).await });
    }

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
        // Standard Git Smart HTTP routes. Keep the legacy explicit service
        // routes above for compatibility with existing callers, but expose
        // the paths used by ordinary `git clone`/`git fetch`/`git push`.
        .route("/{owner}/{repo}/info/refs", get(git_info_refs))
        .route(
            "/{owner}/{repo}/git-upload-pack",
            post(git_upload_pack_standard),
        )
        .route(
            "/{owner}/{repo}/git-receive-pack",
            post(git_receive_pack_standard),
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
        db_pool: saved_db_pool.clone(),
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

/// Look up RepoId from database using owner username and repo name
async fn lookup_repo_id(
    db_pool: &Option<Arc<Pool>>,
    owner: &str,
    repo_name: &str,
) -> Option<RepoId> {
    let pool = db_pool.as_ref()?;

    match gitforge_db::queries::RepoQueries::get_by_owner_and_name(pool, owner, repo_name).await {
        Ok(Some(repo)) => {
            tracing::debug!("looked up repo {:?} for {}/{}", repo.id, owner, repo_name);
            Some(repo.id)
        }
        Ok(None) => {
            tracing::debug!("repo not found in DB for {}/{}", owner, repo_name);
            None
        }
        Err(e) => {
            tracing::warn!("DB lookup failed for {}/{}: {}", owner, repo_name, e);
            None
        }
    }
}

/// Git upload-pack handler (GET) - returns ref advertisement
async fn git_upload_pack(
    Path((owner, repo)): Path<(String, String)>,
    State(state): State<AppState>,
) -> Response {
    let repo_path = format!("{}/{}", owner, repo);

    // Try to look up repo ID from database first
    let repo_id = if let Some(_pool) = &state.db_pool {
        match lookup_repo_id(&state.db_pool, &owner, &repo).await {
            Some(id) => id,
            None => {
                tracing::warn!("repository not found in DB: {}", repo_path);
                return Response::builder()
                    .status(StatusCode::NOT_FOUND)
                    .body(Body::from(format!("Repository not found: {}", repo_path)))
                    .unwrap();
            }
        }
    } else {
        tracing::warn!(
            "database not available, cannot look up repository: {}",
            repo_path
        );
        return Response::builder()
            .status(StatusCode::SERVICE_UNAVAILABLE)
            .body(Body::from("Database not available"))
            .unwrap();
    };

    // Check if repository exists in storage
    if !state.storage.exists(repo_id).await {
        tracing::warn!("repository not found in storage: {}", repo_path);
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

/// Standard Smart HTTP ref advertisement endpoint.
async fn git_info_refs(
    Path((owner, repo)): Path<(String, String)>,
    Query(query): Query<InfoRefsQuery>,
    State(state): State<AppState>,
) -> Response {
    let repo = repo.trim_end_matches(".git").to_string();
    if query.service.as_deref() != Some("git-receive-pack") {
        return git_upload_pack(Path((owner, repo)), State(state)).await;
    }

    let repo_path = format!("{owner}/{repo}");
    let repo_id = match lookup_repo_id(&state.db_pool, &owner, &repo).await {
        Some(id) => id,
        None => {
            return Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(Body::from(format!("Repository not found: {repo_path}")))
                .unwrap()
        }
    };
    if !state.storage.exists(repo_id).await {
        return Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Body::from(format!("Repository not found: {repo_path}")))
            .unwrap();
    }
    match state.http_handler.receive_pack_advertisement(repo_id).await {
        Ok(response) => Response::builder()
            .status(StatusCode::OK)
            .header(
                "Content-Type",
                "application/x-git-receive-pack-advertisement",
            )
            .header("Cache-Control", "no-cache")
            .body(Body::from(response))
            .unwrap(),
        Err(error) => Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .body(Body::from(format!("Error: {error}")))
            .unwrap(),
    }
}

/// Standard Smart HTTP upload-pack endpoint.
async fn git_upload_pack_standard(
    Path((owner, repo)): Path<(String, String)>,
    State(state): State<AppState>,
    request: Request<Body>,
) -> Response {
    let _ = request;
    git_upload_pack(
        Path((owner, repo.trim_end_matches(".git").to_string())),
        State(state),
    )
    .await
}

/// Git receive-pack handler (POST) - receives pack data
async fn git_receive_pack(
    Path((owner, repo)): Path<(String, String)>,
    State(state): State<AppState>,
    request: Request<Body>,
) -> Response {
    let repo_path = format!("{}/{}", owner, repo);

    // Try to look up repo ID from database first
    let repo_id = if let Some(_pool) = &state.db_pool {
        match lookup_repo_id(&state.db_pool, &owner, &repo).await {
            Some(id) => id,
            None => {
                tracing::warn!("repository not found in DB: {}", repo_path);
                return Response::builder()
                    .status(StatusCode::NOT_FOUND)
                    .body(Body::from(format!("Repository not found: {}", repo_path)))
                    .unwrap();
            }
        }
    } else {
        tracing::warn!(
            "database not available, cannot look up repository: {}",
            repo_path
        );
        return Response::builder()
            .status(StatusCode::SERVICE_UNAVAILABLE)
            .body(Body::from("Database not available"))
            .unwrap();
    };

    // Check if repository exists in storage
    if !state.storage.exists(repo_id).await {
        tracing::warn!("repository not found in storage: {}", repo_path);
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
        Ok(response) => {
            for update in parse_receive_updates(&body) {
                if let Err(error) = enqueue_ci_event(&state, repo_id, &update).await {
                    tracing::error!(
                        repo_id = %repo_id,
                        ref_name = %update.ref_name,
                        error = %error,
                        "failed to notify CI after accepted push"
                    );
                }
            }
            if let Err(error) = deliver_pending_ci_events(&state).await {
                tracing::warn!(error = %error, "CI outbox delivery deferred after push");
            }
            Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", "application/x-git-receive-pack-result")
                .body(Body::from(response))
                .unwrap()
        }
        Err(e) => {
            tracing::warn!("receive-pack failed for {}: {}", repo_path, e);
            Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Body::from(format!("Error: {}", e)))
                .unwrap()
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct ReceiveUpdate {
    old_hash: String,
    new_hash: String,
    ref_name: String,
}

fn parse_receive_updates(input: &[u8]) -> Vec<ReceiveUpdate> {
    let mut updates = Vec::new();
    let mut offset = 0;
    while offset + 4 <= input.len() {
        let Ok(length) =
            usize::from_str_radix(&String::from_utf8_lossy(&input[offset..offset + 4]), 16)
        else {
            break;
        };
        if length == 0 {
            break;
        }
        if length < 4 || offset + length > input.len() {
            break;
        }
        let payload = &input[offset + 4..offset + length];
        if let Ok(line) = std::str::from_utf8(payload) {
            let fields: Vec<&str> = line
                .split('\0')
                .next()
                .unwrap_or_default()
                .split_whitespace()
                .collect();
            if fields.len() >= 3 {
                updates.push(ReceiveUpdate {
                    old_hash: fields[0].to_string(),
                    new_hash: fields[1].to_string(),
                    ref_name: fields[2].to_string(),
                });
            }
        }
        offset += length;
    }
    updates
}

async fn enqueue_ci_event(
    state: &AppState,
    repo_id: RepoId,
    update: &ReceiveUpdate,
) -> anyhow::Result<()> {
    let Some(pool) = &state.db_pool else {
        anyhow::bail!("database is required for durable CI delivery");
    };
    let payload = serde_json::json!({
        "repo_id": repo_id.to_string(),
        "ref_name": update.ref_name,
        "old_hash": update.old_hash,
        "new_hash": update.new_hash,
    });
    sqlx::query("INSERT INTO events (id, event_type, payload, created_at) VALUES (?, ?, ?, ?)")
        .bind(uuid::Uuid::new_v4().to_string())
        .bind("ci.trigger.pending")
        .bind(payload.to_string())
        .bind(Utc::now().to_rfc3339())
        .execute(pool.pool())
        .await?;
    Ok(())
}

async fn deliver_pending_ci_events(state: &AppState) -> anyhow::Result<()> {
    let (Some(pool), Some(url), Some(token)) = (
        &state.db_pool,
        &state.ci_trigger_url,
        &state.ci_trigger_token,
    ) else {
        return Ok(());
    };
    let events = sqlx::query_as::<_, (String, String)>(
        "SELECT id, payload FROM events WHERE event_type = 'ci.trigger.pending' ORDER BY created_at LIMIT 50",
    )
    .fetch_all(pool.pool())
    .await?;
    for (id, payload) in events {
        let claimed = sqlx::query(
            "UPDATE events SET event_type = 'ci.trigger.delivering' WHERE id = ? AND event_type = 'ci.trigger.pending'",
        )
        .bind(&id)
        .execute(pool.pool())
        .await?
        .rows_affected();
        if claimed != 1 {
            continue;
        }
        let response = state
            .http_client
            .post(url)
            .bearer_auth(token)
            .json(&serde_json::from_str::<serde_json::Value>(&payload)?)
            .send()
            .await;
        match response {
            Ok(response) if response.status().is_success() => {
                sqlx::query("UPDATE events SET event_type = 'ci.trigger.delivered' WHERE id = ?")
                    .bind(&id)
                    .execute(pool.pool())
                    .await?;
            }
            Ok(response) => {
                sqlx::query("UPDATE events SET event_type = 'ci.trigger.pending' WHERE id = ?")
                    .bind(&id)
                    .execute(pool.pool())
                    .await?;
                anyhow::bail!("CI trigger returned HTTP {}", response.status());
            }
            Err(error) => {
                sqlx::query("UPDATE events SET event_type = 'ci.trigger.pending' WHERE id = ?")
                    .bind(&id)
                    .execute(pool.pool())
                    .await?;
                anyhow::bail!("CI trigger request failed: {error}");
            }
        }
    }
    Ok(())
}

async fn ci_delivery_loop(state: AppState) {
    loop {
        if let Err(error) = deliver_pending_ci_events(&state).await {
            tracing::warn!(error = %error, "CI outbox delivery deferred");
        }
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
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

/// Standard Smart HTTP receive-pack endpoint.
async fn git_receive_pack_standard(
    Path((owner, repo)): Path<(String, String)>,
    State(state): State<AppState>,
    request: Request<Body>,
) -> Response {
    git_receive_pack(
        Path((owner, repo.trim_end_matches(".git").to_string())),
        State(state),
        request,
    )
    .await
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
        let accept_result = tokio::time::timeout(Duration::from_secs(1), listener.accept()).await;

        match accept_result {
            Ok(Ok((stream, peer_addr))) => {
                tracing::debug!("SSH connection from {}", peer_addr);

                // Clone handler and storage for this connection
                let handler = ssh_handler.clone();
                let storage = config.storage.clone();
                let db_pool = config.db_pool.clone();

                // Handle connection in blocking task since ssh2 is sync
                tokio::task::spawn_blocking(move || {
                    handle_ssh_connection(stream, handler, storage, db_pool);
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
    storage: Arc<FileStorageBackend>,
    db_pool: Option<Arc<Pool>>,
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

            // Look up repo ID from database if available
            let repo_id = if let Some(ref pool) = db_pool {
                match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap()
                    .block_on(gitforge_db::queries::RepoQueries::get_by_owner_and_name(
                        pool, owner, repo,
                    )) {
                    Ok(Some(repo)) => repo.id,
                    Ok(None) => {
                        tracing::warn!("repository not found in DB: {}/{}", owner, repo);
                        let _ = channel.write_all(b"repository not found\n");
                        channel.wait_close().ok();
                        return;
                    }
                    Err(e) => {
                        tracing::error!("DB lookup failed: {}", e);
                        let _ = channel.write_all(b"database error\n");
                        channel.wait_close().ok();
                        return;
                    }
                }
            } else {
                tracing::warn!("database not available for SSH connection");
                let _ = channel.write_all(b"database not available\n");
                channel.wait_close().ok();
                return;
            };

            // Check if repository exists in storage
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            if rt.block_on(storage.exists(repo_id)) {
                tracing::debug!("repository found in storage: {:?}", repo_id);
            } else {
                tracing::warn!("repository not found in storage: {}/{}", owner, repo);
                let _ = channel.write_all(b"repository not found\n");
                channel.wait_close().ok();
                return;
            }

            // Process based on command
            let response = match git_cmd {
                "git-upload-pack" => tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap()
                    .block_on(handler.upload_pack(repo_id, vec![])),
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
                _ => Err(gitforge_common::Error::git(format!(
                    "unsupported command: {}",
                    git_cmd
                ))),
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
    fn test_parse_receive_updates() {
        let payload = "1111111111111111111111111111111111111111 2222222222222222222222222222222222222222 refs/heads/main\0report-status\n";
        let input = format!("{:04x}{payload}0000", payload.len() + 4).into_bytes();
        assert_eq!(
            parse_receive_updates(&input),
            vec![ReceiveUpdate {
                old_hash: "1111111111111111111111111111111111111111".to_string(),
                new_hash: "2222222222222222222222222222222222222222".to_string(),
                ref_name: "refs/heads/main".to_string(),
            }]
        );
    }

    #[test]
    fn test_parse_receive_updates_rejects_truncated_packet() {
        assert!(parse_receive_updates(b"0040incomplete").is_empty());
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

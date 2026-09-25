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
use gitforge_core::git_protocol::{http::HttpGitHandler, GitProtocolHandler};

mod ssh_server;
use gitforge_core::{FileStorageBackend, RepoService, StorageBackend};
use gitforge_db::Pool;
use gitforge_events::{EventBus, InMemoryEventBus};
use gitforge_process::{create_shutdown_flag, spawn_shutdown_handler, wait_for_shutdown};
use ssh_server::{run_ssh_server, SshServerConfig};
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
    let http_addr = format!("0.0.0.0:{http_port}");
    tracing::info!("starting Git HTTP server on {}", http_addr);

    let http_listener = tokio::net::TcpListener::bind(&http_addr).await?;
    tracing::info!("Git HTTP server listening on {}", http_addr);

    // Spawn HTTP server
    let http_handle = tokio::spawn(async move {
        axum::serve(http_listener, app).await.unwrap();
    });

    // Start SSH server for Git operations. The host key is generated on
    // first boot and reused afterwards; mount its directory on a volume.
    let ssh_config = SshServerConfig {
        port: ssh_port,
        storage: storage.clone(),
        db_pool: saved_db_pool.clone(),
        host_key_path: ssh_server::default_host_key_path(),
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

/// Complete a statically-constructed response. Builder inputs here are
/// constants, so a builder failure would be a programming error; degrade
/// to a bare 500 instead of unwinding the connection handler.
fn finish_response(builder: axum::http::response::Builder, body: Body) -> Response {
    builder.body(body).unwrap_or_else(|error| {
        tracing::error!(%error, "response construction failed");
        Response::new(Body::from("internal error"))
    })
}

/// Git upload-pack handler (GET) - returns ref advertisement
async fn git_upload_pack(
    Path((owner, repo)): Path<(String, String)>,
    State(state): State<AppState>,
) -> Response {
    let repo_path = format!("{owner}/{repo}");

    // Try to look up repo ID from database first
    let repo_id = if let Some(_pool) = &state.db_pool {
        match lookup_repo_id(&state.db_pool, &owner, &repo).await {
            Some(id) => id,
            None => {
                tracing::warn!("repository not found in DB: {}", repo_path);
                return finish_response(
                    Response::builder().status(StatusCode::NOT_FOUND),
                    Body::from(format!("Repository not found: {repo_path}")),
                );
            }
        }
    } else {
        tracing::warn!(
            "database not available, cannot look up repository: {}",
            repo_path
        );
        return finish_response(
            Response::builder().status(StatusCode::SERVICE_UNAVAILABLE),
            Body::from("Database not available"),
        );
    };

    // Check if repository exists in storage
    if !state.storage.exists(repo_id).await {
        tracing::warn!("repository not found in storage: {}", repo_path);
        return finish_response(
            Response::builder().status(StatusCode::NOT_FOUND),
            Body::from(format!("Repository not found: {repo_path}")),
        );
    }

    match state.http_handler.upload_pack(repo_id, vec![]).await {
        Ok(response) => {
            let mut res = finish_response(
                Response::builder().status(StatusCode::OK).header(
                    "Content-Type",
                    "application/x-git-upload-pack-advertisement",
                ),
                Body::from(response),
            );
            res.headers_mut().insert(
                "Cache-Control",
                axum::http::HeaderValue::from_static("no-cache"),
            );
            res
        }
        Err(e) => {
            tracing::warn!("upload-pack failed for {}: {}", repo_path, e);
            finish_response(
                Response::builder().status(StatusCode::INTERNAL_SERVER_ERROR),
                Body::from(format!("Error: {e}")),
            )
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
    let repo_id = if state.db_pool.is_some() {
        match lookup_repo_id(&state.db_pool, &owner, &repo).await {
            Some(id) => id,
            None => {
                return finish_response(
                    Response::builder().status(StatusCode::NOT_FOUND),
                    Body::from(format!("Repository not found: {repo_path}")),
                )
            }
        }
    } else {
        tracing::warn!(
            "database not available for info/refs, cannot look up repository: {repo_path}"
        );
        return finish_response(
            Response::builder().status(StatusCode::SERVICE_UNAVAILABLE),
            Body::from("Database not available"),
        );
    };
    if !state.storage.exists(repo_id).await {
        return finish_response(
            Response::builder().status(StatusCode::NOT_FOUND),
            Body::from(format!("Repository not found: {repo_path}")),
        );
    }
    match state.http_handler.receive_pack_advertisement(repo_id).await {
        Ok(response) => finish_response(
            Response::builder()
                .status(StatusCode::OK)
                .header(
                    "Content-Type",
                    "application/x-git-receive-pack-advertisement",
                )
                .header("Cache-Control", "no-cache"),
            Body::from(response),
        ),
        Err(error) => finish_response(
            Response::builder().status(StatusCode::INTERNAL_SERVER_ERROR),
            Body::from(format!("Error: {error}")),
        ),
    }
}

/// Standard Smart HTTP upload-pack endpoint.
async fn git_upload_pack_standard(
    Path((owner, repo)): Path<(String, String)>,
    State(state): State<AppState>,
    request: Request<Body>,
) -> Response {
    // Smart HTTP fetch: pipe the client wants/haves through upload-pack
    // instead of replaying the ref advertisement.
    let repo = repo.trim_end_matches(".git").to_string();
    let repo_path = format!("{owner}/{repo}");
    let repo_id = if let Some(_pool) = &state.db_pool {
        match lookup_repo_id(&state.db_pool, &owner, &repo).await {
            Some(id) => id,
            None => {
                tracing::warn!("repository not found in DB: {}", repo_path);
                return finish_response(
                    Response::builder().status(StatusCode::NOT_FOUND),
                    Body::from(format!("Repository not found: {repo_path}")),
                );
            }
        }
    } else {
        tracing::warn!(
            "database not available, cannot look up repository: {}",
            repo_path
        );
        return finish_response(
            Response::builder().status(StatusCode::SERVICE_UNAVAILABLE),
            Body::from("Database not available"),
        );
    };
    if !state.storage.exists(repo_id).await {
        tracing::warn!("repository not found in storage: {}", repo_path);
        return finish_response(
            Response::builder().status(StatusCode::NOT_FOUND),
            Body::from(format!("Repository not found: {repo_path}")),
        );
    }
    let body = match axum::body::to_bytes(request.into_body(), max_git_body_bytes()).await {
        Ok(body) => body,
        Err(error) => {
            tracing::warn!("failed to read upload-pack body: {}", error);
            return finish_response(
                Response::builder().status(StatusCode::BAD_REQUEST),
                Body::from(format!("Bad request: {error}")),
            );
        }
    };
    match state.http_handler.upload_pack(repo_id, body.to_vec()).await {
        Ok(response) => finish_response(
            Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", "application/x-git-upload-pack-result"),
            Body::from(response),
        ),
        Err(e) => {
            tracing::warn!("upload-pack failed for {}: {}", repo_path, e);
            finish_response(
                Response::builder().status(StatusCode::INTERNAL_SERVER_ERROR),
                Body::from(format!("Error: {e}")),
            )
        }
    }
}

/// Git receive-pack handler (POST) - receives pack data
async fn git_receive_pack(
    Path((owner, repo)): Path<(String, String)>,
    State(state): State<AppState>,
    request: Request<Body>,
) -> Response {
    let repo_path = format!("{owner}/{repo}");

    // Try to look up repo ID from database first
    let repo_id = if let Some(_pool) = &state.db_pool {
        match lookup_repo_id(&state.db_pool, &owner, &repo).await {
            Some(id) => id,
            None => {
                tracing::warn!("repository not found in DB: {}", repo_path);
                return finish_response(
                    Response::builder().status(StatusCode::NOT_FOUND),
                    Body::from(format!("Repository not found: {repo_path}")),
                );
            }
        }
    } else {
        tracing::warn!(
            "database not available, cannot look up repository: {}",
            repo_path
        );
        return finish_response(
            Response::builder().status(StatusCode::SERVICE_UNAVAILABLE),
            Body::from("Database not available"),
        );
    };

    // Check if repository exists in storage
    if !state.storage.exists(repo_id).await {
        tracing::warn!("repository not found in storage: {}", repo_path);
        return finish_response(
            Response::builder().status(StatusCode::NOT_FOUND),
            Body::from(format!("Repository not found: {repo_path}")),
        );
    }

    // Read request body. Oversized pushes are rejected explicitly instead of
    // being silently truncated to an empty pack, which used to hang up on
    // clients mid-send.
    let body = match axum::body::to_bytes(request.into_body(), max_git_body_bytes()).await {
        Ok(body) => body,
        Err(error) => {
            tracing::warn!("failed to read receive-pack body: {}", error);
            return finish_response(
                Response::builder().status(StatusCode::PAYLOAD_TOO_LARGE),
                Body::from(format!("Receive-pack body rejected: {error}")),
            );
        }
    };

    match state
        .http_handler
        .receive_pack(repo_id, body.to_vec())
        .await
    {
        Ok(response) => {
            for update in parse_receive_updates(&body) {
                // A deletion push carries git's all-zero new hash and has no
                // commit to build; forwarding it used to spawn a pipeline
                // whose checkout failed immediately (observed 2026-09-24 as
                // run 4b9497bb built at 000…0 after `feat/release-gate` was
                // deleted). Branch creation (all-zero OLD hash) is real and
                // still triggers.
                if gitforge_common::is_zero_hash(&update.new_hash) {
                    tracing::info!(
                        repo_id = %repo_id,
                        ref_name = %update.ref_name,
                        "ref deleted; not triggering CI"
                    );
                    continue;
                }
                if let Err(error) = enqueue_ci_event(&state, repo_id, &update).await {
                    // No longer a drop: the inline retry outlives measured
                    // storms (F42) and anything beyond that hands off to a
                    // background continuation that keeps inserting until the
                    // row lands. This fires only when that deferral engaged.
                    tracing::error!(
                        repo_id = %repo_id,
                        ref_name = %update.ref_name,
                        error = %error,
                        "ci trigger insert deferred; background redelivery engaged"
                    );
                }
            }
            if let Err(error) = deliver_pending_ci_events(&state).await {
                tracing::warn!(error = %error, "CI outbox delivery deferred after push");
            }
            finish_response(
                Response::builder()
                    .status(StatusCode::OK)
                    .header("Content-Type", "application/x-git-receive-pack-result"),
                Body::from(response),
            )
        }
        Err(e) => {
            tracing::warn!("receive-pack failed for {}: {}", repo_path, e);
            finish_response(
                Response::builder().status(StatusCode::INTERNAL_SERVER_ERROR),
                Body::from(format!("Error: {e}")),
            )
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
    // Defense in depth for the deletion filter at the call site: no caller
    // should publish a trigger with nothing to build, whatever the path.
    if gitforge_common::is_zero_hash(&update.new_hash) {
        tracing::warn!(
            repo_id = %repo_id,
            ref_name = %update.ref_name,
            "refusing to enqueue CI trigger for all-zero new hash"
        );
        return Ok(());
    }
    let Some(pool) = &state.db_pool else {
        anyhow::bail!("database is required for durable CI delivery");
    };
    let payload = serde_json::json!({
        "repo_id": repo_id.to_string(),
        "ref_name": update.ref_name,
        "old_hash": update.old_hash,
        "new_hash": update.new_hash,
    });
    // A lost trigger row is a lost pipeline: by this point the push is
    // already accepted, so nothing else will ever retry this insert — the
    // durable redelivery loop below only sees events that made it into the
    // table. A single failed insert has been observed in production
    // (2026-09-23): with a long transaction holding the database write
    // lock, pushes were accepted while their CI triggers silently
    // vanished, leaving no run and no retry. F42 then measured the bounded
    // budget (4 attempts ≈ 60 s) against storms lasting 30+ minutes and
    // dropped events anyway. The retry therefore has no attempt cap: it
    // backs off exponentially (capped) and keeps going for as long as the
    // receive-pack response can plausibly wait; if the storm outlives that
    // window the insert hands off to a background continuation that keeps
    // retrying for the life of the process, and the push still succeeds.
    let persist_result = persist_ci_trigger(
        pool,
        &payload.to_string(),
        CI_TRIGGER_SYNC_WINDOW,
        CI_TRIGGER_MAX_BACKOFF,
    )
    .await;
    match persist_result {
        Ok(()) => Ok(()),
        Err(made_attempts) => {
            tracing::warn!(
                repo_id = %repo_id,
                ref_name = %update.ref_name,
                attempts = made_attempts,
                "ci trigger insert deferred to background redelivery while the database stays contended"
            );
            Err(anyhow::anyhow!(
                "ci trigger insert still failing after {made_attempts} attempts; background redelivery continues"
            ))
        }
    }
}

/// How long the receive-pack path keeps retrying the outbox insert inline
/// before handing off to the background continuation. Generous by design —
/// the git client is already waiting on the push response — but bounded so
/// a persistently broken database cannot hang pushes forever.
const CI_TRIGGER_SYNC_WINDOW: std::time::Duration = std::time::Duration::from_secs(300);

/// Ceiling of the exponential insert backoff.
const CI_TRIGGER_MAX_BACKOFF: std::time::Duration = std::time::Duration::from_secs(30);

/// Insert one `ci.trigger.pending` outbox row, retrying until it lands.
///
/// Returns `Ok(())` once the row is durable. If the database stays
/// unwritable past `sync_window`, the retry continues in a spawned task
/// (backing off `max_backoff` per attempt) and the number of inline
/// attempts is returned as `Err` so the caller can log the deferral: the
/// push is accepted either way, and this insert is the only durable record
/// of the pipeline it must produce.
async fn persist_ci_trigger(
    pool: &gitforge_db::Pool,
    payload: &str,
    sync_window: std::time::Duration,
    max_backoff: std::time::Duration,
) -> Result<(), usize> {
    let started = std::time::Instant::now();
    let mut attempt: usize = 0;
    loop {
        attempt += 1;
        let result = sqlx::query("INSERT INTO events (id, event_type, payload, created_at, delivery_attempts) VALUES (?, ?, ?, ?, 0)")
            .bind(uuid::Uuid::new_v4().to_string())
            .bind("ci.trigger.pending")
            .bind(payload)
            .bind(Utc::now().to_rfc3339())
            .execute(pool.pool())
            .await;
        match result {
            Ok(_) => return Ok(()),
            Err(error) => {
                // Backoff doubles per attempt (2, 4, 8, …) up to the cap:
                // retry pressure scales with however long the storm lasts
                // instead of a fixed budget that measured storms blow
                // through.
                let backoff = std::cmp::min(
                    max_backoff,
                    std::time::Duration::from_secs(1 << attempt.min(6)),
                );
                tracing::warn!(
                    attempt,
                    backoff_secs = backoff.as_secs(),
                    error = %error,
                    "ci trigger insert failed; retrying"
                );
                if started.elapsed() + backoff > sync_window {
                    let pool = pool.clone();
                    let payload = payload.to_string();
                    tokio::spawn(async move {
                        let mut deferred_attempt = 0;
                        loop {
                            deferred_attempt += 1;
                            tokio::time::sleep(max_backoff).await;
                            let result = sqlx::query("INSERT INTO events (id, event_type, payload, created_at, delivery_attempts) VALUES (?, ?, ?, ?, 0)")
                                .bind(uuid::Uuid::new_v4().to_string())
                                .bind("ci.trigger.pending")
                                .bind(&payload)
                                .bind(Utc::now().to_rfc3339())
                                .execute(pool.pool())
                                .await;
                            match result {
                                Ok(_) => {
                                    tracing::info!(
                                        deferred_attempt,
                                        "deferred ci trigger row landed after the database recovered"
                                    );
                                    return;
                                }
                                Err(error) => tracing::warn!(
                                    deferred_attempt,
                                    error = %error,
                                    "deferred ci trigger insert still failing"
                                ),
                            }
                        }
                    });
                    return Err(attempt);
                }
                tokio::time::sleep(backoff).await;
            }
        }
    }
}

async fn deliver_pending_ci_events(state: &AppState) -> anyhow::Result<()> {
    let (Some(pool), Some(url), Some(token)) = (
        &state.db_pool,
        &state.ci_trigger_url,
        &state.ci_trigger_token,
    ) else {
        return Ok(());
    };
    const LEASE_SECONDS: i64 = 120;
    let now = Utc::now();
    let now_text = now.to_rfc3339();
    let lease_until = (now + chrono::Duration::seconds(LEASE_SECONDS)).to_rfc3339();
    let events = sqlx::query_as::<_, (String, String)>(
        "SELECT id, payload FROM events
         WHERE event_type = 'ci.trigger.pending'
            OR (event_type = 'ci.trigger.delivering' AND delivery_until IS NOT NULL AND delivery_until <= ?)
         ORDER BY created_at LIMIT 50",
    )
    .bind(&now_text)
    .fetch_all(pool.pool())
    .await?;
    for (id, payload) in events {
        let delivery_token = uuid::Uuid::new_v4().to_string();
        let claimed = sqlx::query(
            "UPDATE events SET event_type = 'ci.trigger.delivering', delivery_token = ?,
             delivery_until = ?, delivery_attempts = delivery_attempts + 1
             WHERE id = ? AND (event_type = 'ci.trigger.pending'
                OR (event_type = 'ci.trigger.delivering' AND delivery_until IS NOT NULL AND delivery_until <= ?))",
        )
        .bind(&delivery_token)
        .bind(&lease_until)
        .bind(&id)
        .bind(&now_text)
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
                sqlx::query("UPDATE events SET event_type = 'ci.trigger.delivered', delivery_token = NULL, delivery_until = NULL WHERE id = ? AND event_type = 'ci.trigger.delivering' AND delivery_token = ?")
                    .bind(&id)
                    .bind(&delivery_token)
                    .execute(pool.pool())
                    .await?;
            }
            Ok(response) => {
                sqlx::query("UPDATE events SET event_type = 'ci.trigger.pending', delivery_token = NULL, delivery_until = NULL WHERE id = ? AND event_type = 'ci.trigger.delivering' AND delivery_token = ?")
                    .bind(&id)
                    .bind(&delivery_token)
                    .execute(pool.pool())
                    .await?;
                anyhow::bail!("CI trigger returned HTTP {}", response.status());
            }
            Err(error) => {
                sqlx::query("UPDATE events SET event_type = 'ci.trigger.pending', delivery_token = NULL, delivery_until = NULL WHERE id = ? AND event_type = 'ci.trigger.delivering' AND delivery_token = ?")
                    .bind(&id)
                    .bind(&delivery_token)
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
    // Keep the development default aligned with API repository provisioning.
    // Production deployments should set GIT_ROOT explicitly to a durable path.
    std::env::var("GIT_ROOT").unwrap_or_else(|_| "target/gitforge-repos".to_string())
}

/// Maximum buffered request body for Git Smart HTTP. The previous hardcoded
/// 10 MiB cap silently truncated real-world pushes (a small monorepo history
/// can exceed it many times over), so the limit is configurable and the
/// default covers large repositories. Bounds memory use on the server.
pub fn max_git_body_bytes() -> usize {
    std::env::var("GITFORGE_MAX_GIT_BODY_BYTES")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(512 * 1024 * 1024)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    // Tests below mutate the process-wide `GIT_ROOT` environment variable,
    // which races across parallel test threads. A dedicated mutex scopes the
    // serialization to this narrow group only, leaving the rest of the
    // workspace's parallelism untouched.
    fn git_root_env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn test_get_git_root_default() {
        let _guard = git_root_env_lock().lock().unwrap();
        std::env::remove_var("GIT_ROOT");
        let root = get_git_root();
        assert_eq!(root, "target/gitforge-repos");
    }

    #[test]
    fn test_get_git_root_from_env() {
        let _guard = git_root_env_lock().lock().unwrap();
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
        let _guard = git_root_env_lock().lock().unwrap();
        std::env::set_var("GIT_ROOT", "");
        let _root = get_git_root();
        std::env::remove_var("GIT_ROOT");
    }

    #[test]
    fn test_max_git_body_bytes_default_is_512mib() {
        let _guard = git_root_env_lock().lock().unwrap();
        std::env::remove_var("GITFORGE_MAX_GIT_BODY_BYTES");
        assert_eq!(max_git_body_bytes(), 512 * 1024 * 1024);
    }

    #[test]
    fn test_max_git_body_bytes_env_override() {
        let _guard = git_root_env_lock().lock().unwrap();
        std::env::set_var("GITFORGE_MAX_GIT_BODY_BYTES", "4096");
        assert_eq!(max_git_body_bytes(), 4096);
        std::env::remove_var("GITFORGE_MAX_GIT_BODY_BYTES");
    }

    #[test]
    fn test_max_git_body_bytes_invalid_value_falls_back_to_default() {
        let _guard = git_root_env_lock().lock().unwrap();
        std::env::set_var("GITFORGE_MAX_GIT_BODY_BYTES", "not-a-number");
        assert_eq!(max_git_body_bytes(), 512 * 1024 * 1024);
        std::env::remove_var("GITFORGE_MAX_GIT_BODY_BYTES");
    }

    // --- Ref-deletion trigger guard (F37) regression tests ---
    //
    // `git receive-pack` reports an all-zero new hash when a branch is
    // deleted. Forwarding that as a CI trigger produced a pipeline run built
    // at 000…0 whose checkout failed immediately (observed 2026-09-24 as run
    // 4b9497bb after `feat/release-gate` was deleted). The tests below pin
    // both halves of the fix: deletion updates survive wire parsing intact,
    // and `enqueue_ci_event` refuses to publish them.

    #[test]
    fn test_parse_receive_updates_preserves_deletion_sentinel() {
        let payload = "681fb4dfa3059321947bc3cfad93e11f0527f24a 0000000000000000000000000000000000000000 refs/heads/feat/gone\0report-status\n";
        let input = format!("{:04x}{payload}0000", payload.len() + 4).into_bytes();
        let updates = parse_receive_updates(&input);
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].ref_name, "refs/heads/feat/gone");
        // The deletion shape must reach the trigger path recognizable, or the
        // enqueue guard could never classify it.
        assert!(gitforge_common::is_zero_hash(&updates[0].new_hash));
        assert!(!gitforge_common::is_zero_hash(&updates[0].old_hash));
    }

    #[tokio::test]
    async fn test_enqueue_ci_event_skips_all_zero_new_hash() {
        let storage = Arc::new(FileStorageBackend::new("target/test-git-root-f37"));
        let state = AppState {
            http_handler: Arc::new(HttpGitHandler::new((*storage).clone())),
            storage,
            // Deliberately absent: the unguarded durable-delivery path bails
            // without a pool, which is what makes the contrast case below a
            // proof that the zero-hash branch really short-circuited.
            db_pool: None,
            ci_trigger_url: None,
            ci_trigger_token: None,
            http_client: reqwest::Client::new(),
        };
        let repo_id = RepoId::new();

        let deletion = ReceiveUpdate {
            old_hash: "681fb4dfa3059321947bc3cfad93e11f0527f24a".to_string(),
            new_hash: "0000000000000000000000000000000000000000".to_string(),
            ref_name: "refs/heads/feat/gone".to_string(),
        };
        enqueue_ci_event(&state, repo_id, &deletion)
            .await
            .expect("a ref deletion has nothing to build; it must be a silent no-op");

        // Same state, real hash: the guard does not fire, the function
        // reaches the database requirement and fails loudly.
        let real = ReceiveUpdate {
            old_hash: deletion.old_hash,
            new_hash: "681fb4dfa3059321947bc3cfad93e11f0527f24a".to_string(),
            ref_name: "refs/heads/feat/live".to_string(),
        };
        let result = enqueue_ci_event(&state, repo_id, &real).await;
        assert!(
            result.is_err(),
            "a real commit hash must still require the durable database path"
        );
    }

    // --- F42 acceptance: the trigger outbox survives a real SQLITE_BUSY
    // storm ---
    //
    // F42 observed 30+ minute SQLITE_BUSY storms during which every push
    // was accepted by git but produced no pipeline, because the old insert
    // budget (4 attempts ≈ 60 s) gave up long before the database
    // recovered. The tests below hold the write lock past the pool's 15 s
    // busy timeout — the real contention shape, not a mocked error — and
    // pin both halves of the fix: the inline retry lands the row the moment
    // the storm ends, and a storm that outlasts the sync window hands the
    // retry to a background continuation instead of dropping the trigger.

    /// Fresh file-backed pool on tmpfs. WAL file locking is what production
    /// contends on; an in-memory shared-cache DB would not exercise the
    /// same lock path.
    async fn storm_pool(name: &str) -> (gitforge_db::Pool, std::path::PathBuf) {
        let path = std::env::temp_dir().join(format!(
            "gitforge-ci-trigger-storm-{}-{name}.db",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let pool = gitforge_db::Pool::new(&format!("sqlite:{}?mode=rwc", path.display()))
            .await
            .expect("storm test pool must open");
        pool.migrate().await.expect("storm test schema must create");
        (pool, path)
    }

    /// Hold the database write lock for `secs` on a dedicated connection.
    /// Every writer inside the pool then blocks on the 15 s busy timeout,
    /// exactly as during the F42 storms. The returned receiver resolves
    /// once the lock is provably held (a write inside the transaction has
    /// been accepted), so the retry under test always starts inside the
    /// storm.
    async fn hold_write_lock(
        pool: &gitforge_db::Pool,
        secs: u64,
    ) -> (
        tokio::sync::oneshot::Receiver<()>,
        tokio::task::JoinHandle<()>,
    ) {
        let pool = pool.clone();
        let (locked_tx, locked_rx) = tokio::sync::oneshot::channel::<()>();
        let handle = tokio::spawn(async move {
            let mut conn = pool.pool().acquire().await.expect("storm connection");
            sqlx::query("BEGIN IMMEDIATE")
                .execute(&mut *conn)
                .await
                .expect("storm must take the write lock");
            sqlx::query(
                "INSERT INTO events (id, event_type, payload, created_at)
                 VALUES ('storm-holder', 'storm.marker', '{}', '2020-01-01T00:00:00Z')",
            )
            .execute(&mut *conn)
            .await
            .expect("storm marker write must prove the lock is held");
            let _ = locked_tx.send(());
            tokio::time::sleep(std::time::Duration::from_secs(secs)).await;
            sqlx::query("COMMIT")
                .execute(&mut *conn)
                .await
                .expect("storm must release the write lock");
        });
        (locked_rx, handle)
    }

    async fn pending_trigger_count(pool: &gitforge_db::Pool) -> i64 {
        let (count,): (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM events WHERE event_type = 'ci.trigger.pending'")
                .fetch_one(pool.pool())
                .await
                .expect("trigger count query");
        count
    }

    #[tokio::test]
    async fn test_ci_trigger_survives_sqlite_busy_storm_inline() {
        let (pool, path) = storm_pool("inline").await;
        let (locked, storm) = hold_write_lock(&pool, 17).await;
        locked.await.expect("storm lock signal");

        let started = std::time::Instant::now();
        let result = persist_ci_trigger(
            &pool,
            r#"{"repository":"storm","ref":"refs/heads/main"}"#,
            CI_TRIGGER_SYNC_WINDOW,
            CI_TRIGGER_MAX_BACKOFF,
        )
        .await;
        let elapsed = started.elapsed();

        storm.await.expect("storm task");
        assert!(
            result.is_ok(),
            "the trigger must land as soon as the storm ends: {result:?}"
        );
        assert!(
            elapsed >= std::time::Duration::from_secs(15),
            "the first insert must have blocked past the 15 s busy timeout (took {elapsed:?})"
        );
        assert_eq!(
            pending_trigger_count(&pool).await,
            1,
            "exactly one trigger row, no duplicate inserts across retries"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn test_ci_trigger_hands_off_to_background_when_storm_outlasts_window() {
        let (pool, path) = storm_pool("handoff").await;
        // The hold MUST exceed the pool's 30 s busy timeout (connection.rs):
        // a blocked single-statement insert waits inside SQLite's busy
        // handler and simply succeeds when a shorter storm releases, which
        // is honest behavior — but then no deferral ever happens and this
        // test would assert the wrong path. A 35 s hold forces the first
        // attempt to return SQLITE_BUSY at 30 s.
        let (locked, storm) = hold_write_lock(&pool, 35).await;
        locked.await.expect("storm lock signal");

        // One-second window: the first attempt burns the entire busy
        // timeout inside SQLite, so the next backoff step already lands
        // past the window and the insert must move to the background
        // continuation instead of failing the push.
        let Err(attempts) = persist_ci_trigger(
            &pool,
            r#"{"repository":"storm","ref":"refs/heads/handoff"}"#,
            std::time::Duration::from_secs(1),
            std::time::Duration::from_secs(1),
        )
        .await
        .inspect_err(|&made| {
            assert!(
                made >= 1,
                "the deferral report must carry the inline attempt count"
            );
        }) else {
            panic!("a storm past the sync window must defer to background redelivery");
        };
        // Deterministic: the first attempt exhausts the busy timeout, and
        // its very next backoff step already lands past the 1 s window.
        assert_eq!(attempts, 1, "deferral must happen at the first failure");

        // The spawned continuation keeps retrying every max_backoff; the
        // storm releases at 35 s, so the row must land inside the poll
        // window. Nothing else inserts this row — if it never appears, the
        // trigger was dropped, which is the F42 defect itself.
        let mut landed = false;
        for _ in 0..30 {
            if pending_trigger_count(&pool).await == 1 {
                landed = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
        storm.await.expect("storm task");
        assert!(
            landed,
            "the background continuation must land the trigger row after the storm"
        );
        let _ = std::fs::remove_file(&path);
    }
}

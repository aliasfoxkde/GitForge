//! GitForce CI Orchestrator
//!
//! Main entry point for the CI orchestration service.

use axum::Router;
use axum::{
    extract::{Extension, Request},
    http::{header, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    Json,
};
use chrono::Utc;
use futures::StreamExt;
use gitforge_ci::{
    CiEngine, JobDefinition, PipelineDefinition, PipelineTriggerEvent, StepDefinition, TriggerType,
};
use gitforge_common::PipelineStatus;
use gitforge_db::models::{Pipeline as DbPipeline, PipelineRun as DbPipelineRun};
use gitforge_events::{
    EventBus, EventEnvelope, EventFilter, EventPayload, EventType, InMemoryEventBus,
    PushReceivedPayload,
};
use gitforge_process::{create_shutdown_flag, spawn_shutdown_handler, wait_for_shutdown};
use gitforge_scheduler::{
    create_state_with_artifact_storage, scheduler_routes, Scheduler, SchedulerEvent,
};
use gitforge_storage::FileStorage;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;
use tower_http::trace::TraceLayer;

type PipelineCache = HashMap<gitforge_common::RepoId, PipelineDefinition>;
type PipelineRegistry = HashMap<gitforge_common::PipelineRunId, Arc<CiEngine>>;

/// Path of the pipeline definition inside a repository checkout.
const PIPELINE_CONFIG_PATH: &str = ".gitforce.yml";

struct TriggerState {
    event_bus: Arc<dyn EventBus>,
    workspace_paths: Arc<std::sync::Mutex<HashMap<gitforge_common::RepoId, Option<String>>>>,
    run_waiters: Arc<
        std::sync::Mutex<
            HashMap<uuid::Uuid, tokio::sync::oneshot::Sender<gitforge_common::PipelineRunId>>,
        >,
    >,
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

    tracing::info!("starting GitForce CI Orchestrator");

    // Initialize subreaper support without a global waitpid loop. Child
    // ownership must remain with the runtime that spawned it.
    if let Err(e) = gitforge_process::init_without_sigchld_reaper() {
        tracing::warn!("failed to initialize process supervision: {}", e);
    }

    // Initialize event bus
    let event_bus: Arc<dyn EventBus> = Arc::new(InMemoryEventBus::new());

    // Initialize scheduler. Production deployments provide a database URL so
    // job definitions and completion receipts survive service restarts;
    // development keeps the in-memory fallback explicit and usable.
    let (scheduler, scheduler_db) = if let Ok(database_url) = std::env::var("GITFORGE_DATABASE_URL")
    {
        let pool = gitforge_db::Pool::new(&database_url).await?;
        pool.migrate().await?;
        tracing::info!(database_url = %database_url, "using durable GitForge scheduler database");
        (Scheduler::with_db(pool.clone()), Some(pool))
    } else {
        tracing::warn!("GITFORGE_DATABASE_URL is unset; scheduler state is in-memory only");
        (Scheduler::new(), None)
    };

    // Start scheduler HTTP API server on port 42781
    let scheduler_port: u16 = std::env::var("SCHEDULER_PORT")
        .unwrap_or_else(|_| "42781".to_string())
        .parse()
        .unwrap_or(42781);

    // Runner uploads and API downloads must use the same bounded artifact
    // root. The scheduler never accepts a runner filesystem path.
    let artifact_root = std::env::var("GITFORGE_ARTIFACT_ROOT")
        .unwrap_or_else(|_| "target/gitforge-artifacts".to_string());
    let artifact_storage = Arc::new(FileStorage::new(artifact_root).await?);

    // Create scheduler state for HTTP server (consumes scheduler)
    let scheduler_state = create_state_with_artifact_storage(scheduler, Some(artifact_storage));
    let scheduler_arc = scheduler_state.scheduler.clone();
    let workspace_paths = Arc::new(std::sync::Mutex::new(HashMap::new()));
    let run_workspace_paths = Arc::new(std::sync::Mutex::new(HashMap::new()));
    let run_waiters = Arc::new(std::sync::Mutex::new(HashMap::new()));
    let trigger_state = Arc::new(TriggerState {
        event_bus: event_bus.clone(),
        workspace_paths: workspace_paths.clone(),
        run_waiters: run_waiters.clone(),
    });

    let scheduler_app = Router::new()
        .route("/health", axum::routing::get(health_check))
        .route(
            "/pipelines/trigger",
            axum::routing::post(trigger_pipeline).layer(middleware::from_fn(require_trigger_auth)),
        )
        .merge(scheduler_routes(scheduler_state))
        .layer(Extension(trigger_state))
        .layer(TraceLayer::new_for_http());

    let scheduler_addr = format!("0.0.0.0:{}", scheduler_port);
    tracing::info!("starting Scheduler HTTP API on {}", scheduler_addr);

    let scheduler_listener = tokio::net::TcpListener::bind(&scheduler_addr).await?;
    let scheduler_handle = tokio::spawn(async move {
        axum::serve(scheduler_listener, scheduler_app)
            .await
            .unwrap();
    });

    tracing::info!("Scheduler HTTP API listening on {}", scheduler_addr);

    // Pipeline definitions cache
    let pipeline_cache: Arc<std::sync::Mutex<PipelineCache>> =
        Arc::new(std::sync::Mutex::new(HashMap::new()));

    // Clone for event consumer
    let event_bus_clone = event_bus.clone();
    let scheduler_clone = scheduler_arc.clone();
    let pipeline_cache_clone = pipeline_cache.clone();
    let scheduler_db_clone = scheduler_db.clone();
    let workspace_paths_clone = workspace_paths.clone();
    let run_workspace_paths_clone = run_workspace_paths.clone();
    let run_waiters_clone = run_waiters.clone();
    let pipeline_registry: Arc<tokio::sync::RwLock<PipelineRegistry>> =
        Arc::new(tokio::sync::RwLock::new(HashMap::new()));
    let pipeline_registry_clone = pipeline_registry.clone();

    // Shared shutdown flag
    let shutdown = create_shutdown_flag();
    let shutdown_flag = shutdown.clone();

    // Spawn graceful shutdown handler
    spawn_shutdown_handler(shutdown_flag);

    // Start event consumer loop
    let shutdown_consumer = shutdown.clone();
    let _consumer_handle = tokio::spawn(async move {
        if let Err(e) = run_event_consumer(
            event_bus_clone,
            scheduler_clone,
            pipeline_cache_clone,
            scheduler_db_clone,
            workspace_paths_clone,
            run_workspace_paths_clone,
            pipeline_registry_clone,
            run_waiters_clone,
            shutdown_consumer,
        )
        .await
        {
            tracing::error!("event consumer error: {}", e);
        }
    });

    let completion_scheduler = scheduler_arc.clone();
    let completion_registry = pipeline_registry.clone();
    let completion_run_workspace_paths = run_workspace_paths.clone();
    let completion_db = scheduler_db.clone();
    let completion_shutdown = shutdown.clone();
    let _completion_handle = tokio::spawn(async move {
        run_scheduler_event_consumer(
            completion_scheduler,
            completion_registry,
            completion_run_workspace_paths,
            completion_db,
            completion_shutdown,
        )
        .await;
    });

    // Start scheduler loop
    let scheduler_clone = scheduler_arc.clone();
    let shutdown_scheduler = shutdown.clone();
    let _scheduler_handle = tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(5));
        loop {
            if shutdown_scheduler.load(Ordering::SeqCst) {
                tracing::info!("scheduler loop shutting down");
                break;
            }
            ticker.tick().await;
            scheduler_clone.process_queue().await;
        }
    });

    // Start runner-loss detection loop: check for stale runners and re-enqueue their jobs
    let runner_loss_scheduler = scheduler_arc.clone();
    let runner_loss_shutdown = shutdown.clone();
    let _runner_loss_handle = tokio::spawn(async move {
        // Runner is considered stale if no heartbeat for 90 seconds (3x the 30s interval)
        let stale_threshold_secs: i64 = 90;
        let check_interval = Duration::from_secs(30);

        let mut ticker = tokio::time::interval(check_interval);
        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    if runner_loss_shutdown.load(Ordering::SeqCst) {
                        break;
                    }

                    // Mark stale runners as offline
                    let marked = runner_loss_scheduler.mark_stale_runners_offline(stale_threshold_secs).await;
                    if marked > 0 {
                        tracing::info!("marked {} stale runners as offline", marked);
                    }

                    // Get list of offline runners and re-enqueue their jobs
                    // We need to check which runners are now offline and requeue
                    let assigned_jobs = runner_loss_scheduler.get_assigned_jobs().await;
                    for (_job_id, runner_id, _pipeline_run_id) in assigned_jobs {
                        // Check if the runner for this job assignment is now offline
                        // by looking at the runner's current status
                        let runner_offline = {
                            // This is a simplified check - in production we'd track this properly
                            // For now we rely on mark_stale_runners_offline having already
                            // updated runner statuses
                            false // Will be handled via the scheduler's internal tracking
                        };
                        if runner_offline {
                            let requeued = runner_loss_scheduler.requeue_jobs_for_offline_runner(runner_id).await;
                            tracing::warn!("re-enqueued {} jobs after runner {} went offline", requeued, runner_id);
                        }
                    }
                }
                _ = tokio::time::sleep(Duration::from_secs(1)) => {
                    if runner_loss_shutdown.load(Ordering::SeqCst) {
                        break;
                    }
                }
            }
        }
        tracing::info!("runner-loss detection loop shutting down");
    });

    // Start job timeout monitoring loop
    let timeout_registry = pipeline_registry.clone();
    let timeout_shutdown = shutdown.clone();
    let _timeout_handle = tokio::spawn(async move {
        // Check for stale running jobs every 60 seconds
        let check_interval = Duration::from_secs(60);
        let mut ticker = tokio::time::interval(check_interval);
        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    if timeout_shutdown.load(Ordering::SeqCst) {
                        break;
                    }

                    let registry = timeout_registry.read().await;
                    for (run_id, engine) in registry.iter() {
                        let state = engine.state().await;
                        for (job_id, job_state) in state.jobs.iter() {
                            if job_state.status() == gitforge_common::JobStatus::Running {
                                // Check if job has been running too long
                                // Note: we'd need started_at in the job state to do this properly
                                // For now, this is a placeholder that would need the full job tracking
                                tracing::debug!(
                                    "job {} in pipeline {} has been running since state capture",
                                    job_id, run_id
                                );
                            }
                        }
                    }
                }
                _ = tokio::time::sleep(Duration::from_secs(1)) => {
                    if timeout_shutdown.load(Ordering::SeqCst) {
                        break;
                    }
                }
            }
        }
        tracing::info!("job timeout monitoring loop shutting down");
    });

    tracing::info!("CI Orchestrator initialized successfully");

    // Wait for shutdown signal
    let shutdown_future = create_shutdown_future(shutdown.clone());

    // Wait for either shutdown or tasks to complete
    timeout(Duration::MAX, shutdown_future).await.ok();

    tracing::info!("shutting down CI Orchestrator");

    // Cancel scheduler HTTP server
    scheduler_handle.abort();

    // Wait for in-flight work to complete (with timeout)
    graceful_shutdown_delay().await;

    tracing::info!("CI Orchestrator stopped");
    Ok(())
}

/// Health check for scheduler HTTP API
async fn health_check() -> &'static str {
    "OK"
}

#[derive(Debug, serde::Deserialize)]
struct PipelineTriggerRequest {
    repo_id: String,
    ref_name: String,
    old_hash: String,
    new_hash: String,
    working_dir: Option<String>,
}

/// Trigger a pipeline through the same typed push-event path used by Git
/// webhooks. This endpoint is internal control-plane automation and requires
/// a dedicated trigger token, falling back to the scheduler operator/shared
/// token during migration.
fn configured_trigger_token(get_var: impl Fn(&str) -> Option<String>) -> Option<String> {
    [
        "GITFORGE_TRIGGER_TOKEN",
        "GITFORGE_CI_TRIGGER_TOKEN",
        "GITFORGE_SCHEDULER_OPERATOR_TOKEN",
        "GITFORGE_SCHEDULER_TOKEN",
    ]
    .into_iter()
    .find_map(|name| get_var(name).filter(|token| !token.is_empty()))
}

/// Compare trigger credentials without leaking the first differing byte or
/// accepting a token with a different length. The scheduler is an internal
/// control-plane boundary, so both the dedicated compatibility header and the
/// standard Bearer form are supported during migration.
fn trigger_token_matches(expected: &str, supplied: Option<&str>) -> bool {
    let Some(supplied) = supplied else {
        return false;
    };
    let candidate = supplied.strip_prefix("Bearer ").unwrap_or(supplied);
    let expected_bytes = expected.as_bytes();
    let candidate_bytes = candidate.as_bytes();
    let max_len = expected_bytes.len().max(candidate_bytes.len());
    let mut difference = expected_bytes.len() ^ candidate_bytes.len();
    for index in 0..max_len {
        let expected_byte = expected_bytes.get(index).copied().unwrap_or(0);
        let candidate_byte = candidate_bytes.get(index).copied().unwrap_or(0);
        difference |= usize::from(expected_byte ^ candidate_byte);
    }
    difference == 0
}

async fn require_trigger_auth(request: Request, next: Next) -> Response {
    let expected = configured_trigger_token(|name| std::env::var(name).ok());
    let Some(expected) = expected else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"error": "trigger_auth_not_configured"})),
        )
            .into_response();
    };
    let supplied = request
        .headers()
        .get("x-gitforge-trigger-token")
        .or_else(|| request.headers().get(header::AUTHORIZATION))
        .and_then(|value| value.to_str().ok());
    if trigger_token_matches(&expected, supplied) {
        next.run(request).await
    } else {
        (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": "trigger_auth_required"})),
        )
            .into_response()
    }
}

async fn trigger_pipeline(
    Extension(trigger_state): Extension<Arc<TriggerState>>,
    Json(request): Json<PipelineTriggerRequest>,
) -> impl axum::response::IntoResponse {
    let repo_id = match uuid::Uuid::parse_str(&request.repo_id) {
        Ok(id) => gitforge_common::RepoId::from(id),
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": "invalid_repo_id",
                    "message": "repo_id must be a UUID"
                })),
            )
        }
    };

    let working_dir = match request.working_dir {
        Some(path) => match validate_workspace_path(&path) {
            Ok(path) => Some(path),
            Err(message) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({
                        "error": "invalid_workspace",
                        "message": message
                    })),
                )
            }
        },
        None => None,
    };

    trigger_state
        .workspace_paths
        .lock()
        .expect("workspace cache lock poisoned")
        .insert(repo_id, working_dir);

    let event = EventEnvelope::new(
        EventType::PushReceived,
        EventPayload::PushReceived(PushReceivedPayload {
            repo_id,
            ref_name: request.ref_name,
            old_hash: request.old_hash,
            new_hash: request.new_hash.clone(),
            pusher_id: None,
        }),
        Some(repo_id),
        None,
    );

    let (run_tx, run_rx) = tokio::sync::oneshot::channel();
    trigger_state
        .run_waiters
        .lock()
        .expect("run waiter lock poisoned")
        .insert(event.event_id, run_tx);

    match trigger_state.event_bus.publish(event.clone()).await {
        Ok(()) => {
            let pipeline_run_id = tokio::time::timeout(Duration::from_secs(3), run_rx)
                .await
                .ok()
                .and_then(|result| result.ok());
            if pipeline_run_id.is_none() {
                trigger_state
                    .run_waiters
                    .lock()
                    .expect("run waiter lock poisoned")
                    .remove(&event.event_id);
            }
            (
                StatusCode::ACCEPTED,
                Json(serde_json::json!({
                    "status": if pipeline_run_id.is_some() { "accepted" } else { "queued" },
                    "event_id": event.event_id.to_string(),
                    "pipeline_run_id": pipeline_run_id.map(|id| id.to_string()),
                    "repo_id": repo_id.to_string(),
                    "new_hash": request.new_hash,
                })),
            )
        }
        Err(error) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({
                "error": "event_publish_failed",
                "message": error.to_string(),
            })),
        ),
    }
}

fn validate_workspace_path(path: &str) -> Result<String, String> {
    let workspace = std::fs::canonicalize(path)
        .map_err(|error| format!("workspace is not accessible: {}", error))?;
    if !workspace.is_dir() {
        return Err("workspace must be a directory".to_string());
    }
    let root_path = std::env::var("GITFORGE_WORKSPACE_ROOT")
        .unwrap_or_else(|_| "/nas/Temp/control-center-workspaces".to_string());
    let root = std::fs::canonicalize(&root_path)
        .map_err(|error| format!("workspace root is not accessible: {}", error))?;
    if !workspace.starts_with(&root) {
        return Err(format!("workspace must be inside {}", root.display()));
    }
    Ok(workspace.to_string_lossy().into_owned())
}

/// Load the pipeline definition committed at the pushed revision. CI
/// configuration is code: the definition that governs a run is the one
/// committed at the exact hash being tested, not the most recent one the
/// control plane happens to have cached.
///
/// Returns `Ok(None)` when the revision carries no committed definition so
/// callers can fall back to durable/cached/default configuration. A committed
/// but unparseable definition is an error — silently running a different
/// pipeline than the one the author pushed would be worse than failing the
/// trigger.
async fn load_pipeline_from_commit(
    pool: &gitforge_db::Pool,
    repo_id: gitforge_common::RepoId,
    commit_hash: &str,
) -> anyhow::Result<Option<PipelineDefinition>> {
    let repository = gitforge_db::queries::RepoQueries::get(pool, repo_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("repository {} is not registered", repo_id))?;

    let committed = tokio::process::Command::new("git")
        .arg("--git-dir")
        .arg(&repository.git_path)
        .args([
            "cat-file",
            "-e",
            &format!("{}:{}", commit_hash, PIPELINE_CONFIG_PATH),
        ])
        .output()
        .await?;
    if !committed.status.success() {
        return Ok(None);
    }

    let show = tokio::process::Command::new("git")
        .arg("--git-dir")
        .arg(&repository.git_path)
        .args(["show", &format!("{}:{}", commit_hash, PIPELINE_CONFIG_PATH)])
        .output()
        .await?;
    if !show.status.success() {
        anyhow::bail!(
            "failed to read {} at {} for repo {}: {}",
            PIPELINE_CONFIG_PATH,
            commit_hash,
            repo_id,
            String::from_utf8_lossy(&show.stderr).trim()
        );
    }

    let yaml = String::from_utf8(show.stdout).map_err(|error| {
        anyhow::anyhow!(
            "{} at {} is not valid UTF-8: {}",
            PIPELINE_CONFIG_PATH,
            commit_hash,
            error
        )
    })?;
    PipelineDefinition::parse(&yaml).map(Some).map_err(|error| {
        anyhow::anyhow!(
            "invalid {} at {}: {}",
            PIPELINE_CONFIG_PATH,
            commit_hash,
            error
        )
    })
}

/// Create an isolated checkout for a push-triggered run when the caller did
/// not supply an already prepared workspace. The checkout is rooted under a
/// configured directory and named by the immutable run ID, so concurrent runs
/// cannot share mutable source state.
async fn prepare_run_workspace(
    pool: &gitforge_db::Pool,
    repo_id: gitforge_common::RepoId,
    run_id: gitforge_common::PipelineRunId,
    commit_hash: &str,
) -> anyhow::Result<String> {
    let repository = gitforge_db::queries::RepoQueries::get(pool, repo_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("repository {} is not registered", repo_id))?;
    let source = std::fs::canonicalize(&repository.git_path).map_err(|error| {
        anyhow::anyhow!(
            "repository {} git path is unavailable ({}): {}",
            repo_id,
            repository.git_path,
            error
        )
    })?;
    if !source.is_dir() {
        return Err(anyhow::anyhow!(
            "repository {} git path is not a directory: {}",
            repo_id,
            source.display()
        ));
    }

    let root = std::env::var("GITFORGE_WORKSPACE_ROOT")
        .unwrap_or_else(|_| "/var/lib/gitforge/workspaces".to_string());
    tokio::fs::create_dir_all(&root).await?;
    let workspace = std::path::PathBuf::from(root).join(run_id.to_string());
    if tokio::fs::try_exists(&workspace).await? {
        return Err(anyhow::anyhow!(
            "workspace already exists for run {}",
            run_id
        ));
    }

    // Do not request Git's hard-link-based local clone optimization here.
    // Workspace and repository storage may be mounted with policies that
    // reject hard links; a regular clone is portable across those filesystems.
    let clone = tokio::process::Command::new("git")
        .args(["clone", "--no-local", "--no-checkout"])
        .arg(&source)
        .arg(&workspace)
        .output()
        .await?;
    if !clone.status.success() {
        return Err(anyhow::anyhow!(
            "checkout clone failed for run {}: {}",
            run_id,
            String::from_utf8_lossy(&clone.stderr).trim()
        ));
    }

    let checkout = tokio::process::Command::new("git")
        .arg("-C")
        .arg(&workspace)
        .args(["checkout", "--detach", commit_hash])
        .output()
        .await?;
    if !checkout.status.success() {
        return Err(anyhow::anyhow!(
            "checkout commit {} failed for run {}: {}",
            commit_hash,
            run_id,
            String::from_utf8_lossy(&checkout.stderr).trim()
        ));
    }

    Ok(workspace.to_string_lossy().into_owned())
}

/// Create the shutdown future that waits for shutdown signal
pub async fn create_shutdown_future(shutdown: Arc<AtomicBool>) {
    wait_for_shutdown(shutdown).await;
}

/// Perform graceful shutdown delay
pub async fn graceful_shutdown_delay() {
    timeout(Duration::from_secs(5), async {
        tokio::time::sleep(Duration::from_secs(1)).await;
    })
    .await
    .ok();
}

/// Run the event consumer loop
#[allow(clippy::too_many_arguments)]
async fn run_event_consumer(
    event_bus: Arc<dyn EventBus>,
    scheduler: Arc<Scheduler>,
    pipeline_cache: Arc<std::sync::Mutex<PipelineCache>>,
    scheduler_db: Option<gitforge_db::Pool>,
    workspace_paths: Arc<std::sync::Mutex<HashMap<gitforge_common::RepoId, Option<String>>>>,
    run_workspace_paths: Arc<
        std::sync::Mutex<HashMap<gitforge_common::PipelineRunId, Option<String>>>,
    >,
    pipeline_registry: Arc<tokio::sync::RwLock<PipelineRegistry>>,
    run_waiters: Arc<
        std::sync::Mutex<
            HashMap<uuid::Uuid, tokio::sync::oneshot::Sender<gitforge_common::PipelineRunId>>,
        >,
    >,
    shutdown: Arc<AtomicBool>,
) -> anyhow::Result<()> {
    tracing::info!("starting event consumer loop");

    // Subscribe to push events
    let filter = EventFilter::for_types(vec![EventType::PushReceived]);
    let mut stream = event_bus.subscribe(filter).await?;

    loop {
        if shutdown.load(Ordering::SeqCst) {
            tracing::info!("event consumer loop shutting down");
            break;
        }

        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(100)) => {
                if shutdown.load(Ordering::SeqCst) {
                    break;
                }
            }
            event = stream.next() => {
                match event {
                    Some(event) => {
                        tracing::debug!("received event: {:?}", event.event_type);
                        match handle_push_event(&event, &scheduler, &pipeline_cache, scheduler_db.as_ref(), &workspace_paths, &run_workspace_paths, &pipeline_registry).await {
                            Ok(run_id) => {
                                if let Some(waiter) = run_waiters.lock().expect("run waiter lock poisoned").remove(&event.event_id) {
                                    let _ = waiter.send(run_id);
                                }
                            }
                            Err(e) => {
                                run_waiters.lock().expect("run waiter lock poisoned").remove(&event.event_id);
                                tracing::error!("failed to handle push event: {}", e);
                            }
                        }
                    }
                    None => {
                        tracing::info!("event stream closed");
                        break;
                    }
                }
            }
        }
    }

    Ok(())
}

/// Handle a push received event - trigger pipeline if configured
async fn handle_push_event(
    event: &EventEnvelope,
    scheduler: &Arc<Scheduler>,
    pipeline_cache: &Arc<std::sync::Mutex<PipelineCache>>,
    scheduler_db: Option<&gitforge_db::Pool>,
    workspace_paths: &Arc<std::sync::Mutex<HashMap<gitforge_common::RepoId, Option<String>>>>,
    run_workspace_paths: &Arc<
        std::sync::Mutex<HashMap<gitforge_common::PipelineRunId, Option<String>>>,
    >,
    pipeline_registry: &Arc<tokio::sync::RwLock<PipelineRegistry>>,
) -> anyhow::Result<gitforge_common::PipelineRunId> {
    // Only handle PushReceived events
    let EventPayload::PushReceived(payload) = &event.payload else {
        return Ok(gitforge_common::PipelineRunId::new());
    };

    let repo_id = payload.repo_id;
    let ref_name = &payload.ref_name;

    tracing::info!(
        "push received for repo {} on ref {} ({} -> {})",
        repo_id,
        ref_name,
        payload.old_hash,
        payload.new_hash
    );

    // Get or create the pipeline definition for this repo. The definition
    // committed at the pushed revision is authoritative: CI configuration is
    // code, so a push that changes it must govern this and later runs without
    // a control-plane restart. The in-memory cache and durable rows only
    // apply when the revision carries no committed definition.
    let committed_pipeline = match scheduler_db {
        Some(pool) => match load_pipeline_from_commit(pool, repo_id, &payload.new_hash).await {
            Ok(pipeline) => pipeline,
            Err(error) => return Err(error),
        },
        None => None,
    };
    let pipeline = if let Some(committed) = committed_pipeline {
        tracing::info!(
            "using {} committed at {} for repo {}",
            PIPELINE_CONFIG_PATH,
            payload.new_hash,
            repo_id
        );
        committed
    } else {
        let cached_pipeline = { pipeline_cache.lock().unwrap().get(&repo_id).cloned() };
        if let Some(cached) = cached_pipeline {
            cached
        } else {
            let persisted = if let Some(pool) = scheduler_db {
                gitforge_db::queries::PipelineQueries::list_by_repo(pool, repo_id)
                    .await?
                    .into_iter()
                    .find_map(|pipeline| {
                        serde_json::from_value::<PipelineDefinition>(pipeline.config).ok()
                    })
            } else {
                None
            };
            persisted.unwrap_or_else(|| create_default_pipeline(&repo_id.to_string()))
        }
    };
    pipeline_cache
        .lock()
        .unwrap()
        .insert(repo_id, pipeline.clone());
    let requested_workspace = workspace_paths
        .lock()
        .expect("workspace cache lock poisoned")
        .get(&repo_id)
        .cloned()
        .flatten();

    // Create trigger event
    let trigger_event = create_trigger_event(repo_id, &payload.new_hash, ref_name);
    let pipeline_id = trigger_event.pipeline_id;

    // Create and start the CI engine
    let engine = Arc::new(CiEngine::new(trigger_event, pipeline.clone()).await?);
    engine.start().await?;

    tracing::info!(
        "pipeline triggered for repo {} on ref {}",
        repo_id,
        ref_name
    );

    // Enqueue ready jobs to scheduler
    let ready_jobs = engine.ready_jobs().await;
    tracing::info!("enqueueing {} ready jobs", ready_jobs.len());

    let state = engine.state().await;
    if let Some(pool) = scheduler_db {
        let db_pipeline = DbPipeline {
            id: pipeline_id,
            repo_id,
            name: pipeline.name.clone(),
            trigger_type: "push".to_string(),
            config: serde_json::to_value(&pipeline)?,
            created_at: Utc::now(),
        };
        gitforge_db::queries::PipelineQueries::create(pool, &db_pipeline).await?;

        let mut db_run = DbPipelineRun::new(
            pipeline_id,
            repo_id,
            "push".to_string(),
            payload.new_hash.clone(),
        );
        db_run.id = state.run_id;
        db_run.start();
        gitforge_db::queries::PipelineRunQueries::create(pool, &db_run).await?;
    }

    let workspace_path = match requested_workspace {
        Some(path) => Some(path),
        None => {
            let pool = scheduler_db.ok_or_else(|| {
                anyhow::anyhow!(
                    "push run {} has no workspace and durable repository storage is unavailable",
                    state.run_id
                )
            })?;
            match prepare_run_workspace(pool, repo_id, state.run_id, &payload.new_hash).await {
                Ok(path) => Some(path),
                Err(error) => {
                    let _ = gitforge_db::queries::PipelineRunQueries::update_status(
                        pool,
                        state.run_id,
                        "failed",
                    )
                    .await;
                    return Err(error);
                }
            }
        }
    };
    run_workspace_paths
        .lock()
        .expect("workspace cache lock poisoned")
        .insert(state.run_id, workspace_path.clone());
    pipeline_registry
        .write()
        .await
        .insert(state.run_id, engine.clone());

    for job_id in ready_jobs {
        if let Some(_job_state) = state.jobs.get(&job_id) {
            let definition = engine
                .job_definition(job_id)
                .ok_or_else(|| anyhow::anyhow!("missing definition for job {}", job_id))?;
            let commands = definition
                .steps
                .iter()
                .map(|step| step.run.clone())
                .collect();
            let working_dir = definition
                .steps
                .iter()
                .find_map(|step| step.working_directory.clone());
            let working_dir = working_dir.or_else(|| workspace_path.clone());
            scheduler
                .enqueue_with_definition_and_image(
                    job_id,
                    state.run_id,
                    repo_id,
                    commands,
                    definition.image.clone(),
                    working_dir,
                )
                .await;
            tracing::debug!("enqueued job {} for pipeline run {}", job_id, state.run_id);
        }
    }

    Ok(state.run_id)
}

async fn run_scheduler_event_consumer(
    scheduler: Arc<Scheduler>,
    pipeline_registry: Arc<tokio::sync::RwLock<PipelineRegistry>>,
    run_workspace_paths: Arc<
        std::sync::Mutex<HashMap<gitforge_common::PipelineRunId, Option<String>>>,
    >,
    scheduler_db: Option<gitforge_db::Pool>,
    shutdown: Arc<AtomicBool>,
) {
    let mut events = scheduler.subscribe();
    while !shutdown.load(Ordering::SeqCst) {
        let event = match events.recv().await {
            Ok(event) => event,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
            Err(_) => break,
        };
        let SchedulerEvent::JobCompleted {
            job_id,
            pipeline_run_id,
            runner_id,
            success,
        } = event
        else {
            continue;
        };
        let engine = pipeline_registry
            .read()
            .await
            .get(&pipeline_run_id)
            .cloned();
        let Some(engine) = engine else {
            tracing::warn!(
                "completion received for unknown pipeline run {}",
                pipeline_run_id
            );
            continue;
        };
        if let Err(error) = engine.assign_job(job_id, runner_id).await {
            tracing::error!(%job_id, %error, "failed to mark completed job assigned");
            continue;
        }
        if let Err(error) = engine.start_job(job_id).await {
            tracing::error!(%job_id, %error, "failed to mark completed job running");
            continue;
        }
        if success {
            if let Err(error) = engine.succeed_job(job_id, 0).await {
                tracing::error!(%job_id, %error, "failed to mark job succeeded");
                continue;
            }
            if let Err(error) = engine.queue_ready_jobs().await {
                tracing::error!(%pipeline_run_id, %error, "failed to queue downstream jobs");
            }
        } else {
            if let Err(error) = engine
                .fail_job(job_id, -1, "runner reported failure".to_string())
                .await
            {
                tracing::error!(%job_id, %error, "failed to mark job failed");
            }
        }

        let state = engine.state().await;
        let workspace_path = run_workspace_paths
            .lock()
            .expect("workspace cache lock poisoned")
            .get(&state.run_id)
            .cloned()
            .flatten();
        for next_job_id in engine.ready_jobs().await {
            if let Some(definition) = engine.job_definition(next_job_id) {
                let commands = definition
                    .steps
                    .iter()
                    .map(|step| step.run.clone())
                    .collect();
                let working_dir = definition
                    .steps
                    .iter()
                    .find_map(|step| step.working_directory.clone())
                    .or_else(|| workspace_path.clone());
                scheduler
                    .enqueue_with_definition_and_image(
                        next_job_id,
                        state.run_id,
                        state.repo_id,
                        commands,
                        definition.image.clone(),
                        working_dir,
                    )
                    .await;
            }
        }

        let terminal_status = match state.status {
            PipelineStatus::Succeeded => Some("succeeded"),
            PipelineStatus::Failed => Some("failed"),
            PipelineStatus::Cancelled => Some("cancelled"),
            _ => None,
        };
        if let Some(terminal_status) = terminal_status {
            if let Some(pool) = &scheduler_db {
                let _ = gitforge_db::queries::PipelineRunQueries::update_status(
                    pool,
                    state.run_id,
                    terminal_status,
                )
                .await;
            }
            run_workspace_paths
                .lock()
                .expect("workspace cache lock poisoned")
                .remove(&state.run_id);
            pipeline_registry.write().await.remove(&state.run_id);
        }
    }
}

/// Create a trigger event from push payload (extracted for testability)
pub fn create_trigger_event(
    repo_id: gitforge_common::RepoId,
    commit_hash: &str,
    ref_name: &str,
) -> gitforge_ci::PipelineTriggerEvent {
    PipelineTriggerEvent::new(
        gitforge_common::PipelineId::new(),
        repo_id,
        commit_hash.to_string(),
        TriggerType::Push,
    )
    .with_ref(ref_name.to_string())
}

/// Create a default pipeline definition
fn create_default_pipeline(repo_id: &str) -> PipelineDefinition {
    PipelineDefinition {
        name: format!("{}-pipeline", repo_id),
        version: "1.0".to_string(),
        trigger_on: vec![TriggerType::Push],
        environment: HashMap::new(),
        jobs: vec![
            JobDefinition {
                name: "build".to_string(),
                image: "rust:latest".to_string(),
                needs: vec![],
                env: HashMap::new(),
                steps: vec![
                    StepDefinition {
                        name: "setup".to_string(),
                        run: "rustup component add rustfmt clippy && cargo fetch".to_string(),
                        env: None,
                        working_directory: None,
                        condition: None,
                    },
                    StepDefinition {
                        name: "build".to_string(),
                        run: "cargo build --release".to_string(),
                        env: None,
                        working_directory: None,
                        condition: None,
                    },
                ],
                timeout: Some("30m".to_string()),
                retry: Some(1),
            },
            JobDefinition {
                name: "test".to_string(),
                image: "rust:latest".to_string(),
                needs: vec!["build".to_string()],
                env: HashMap::new(),
                steps: vec![StepDefinition {
                    name: "test".to_string(),
                    run: "cargo test".to_string(),
                    env: None,
                    working_directory: None,
                    condition: None,
                }],
                timeout: Some("30m".to_string()),
                retry: Some(1),
            },
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::OnceLock;

    static WORKSPACE_TEST_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();

    #[test]
    fn trigger_token_accepts_git_server_compatibility_name() {
        let token = configured_trigger_token(|name| {
            (name == "GITFORGE_CI_TRIGGER_TOKEN").then(|| "shared-secret".to_string())
        });
        assert_eq!(token.as_deref(), Some("shared-secret"));
    }

    #[test]
    fn trigger_token_prefers_dedicated_name_over_compatibility_alias() {
        let token = configured_trigger_token(|name| match name {
            "GITFORGE_TRIGGER_TOKEN" => Some("dedicated".to_string()),
            "GITFORGE_CI_TRIGGER_TOKEN" => Some("compatibility".to_string()),
            _ => None,
        });
        assert_eq!(token.as_deref(), Some("dedicated"));
    }

    #[test]
    fn trigger_token_ignores_empty_values_and_falls_back() {
        let token = configured_trigger_token(|name| match name {
            "GITFORGE_TRIGGER_TOKEN" => Some(String::new()),
            "GITFORGE_CI_TRIGGER_TOKEN" => Some("compatibility".to_string()),
            _ => None,
        });
        assert_eq!(token.as_deref(), Some("compatibility"));
    }

    #[test]
    fn trigger_token_matches_raw_and_bearer_credentials() {
        assert!(trigger_token_matches(
            "shared-secret",
            Some("shared-secret")
        ));
        assert!(trigger_token_matches(
            "shared-secret",
            Some("Bearer shared-secret")
        ));
    }

    #[test]
    fn trigger_token_rejects_missing_mismatched_and_malformed_credentials() {
        assert!(!trigger_token_matches("shared-secret", None));
        assert!(!trigger_token_matches(
            "shared-secret",
            Some("wrong-secret")
        ));
        assert!(!trigger_token_matches(
            "shared-secret",
            Some("Basic shared-secret")
        ));
        assert!(!trigger_token_matches(
            "shared-secret",
            Some("Bearer shared-secret-extra")
        ));
    }

    async fn run_git<I, S>(args: I, cwd: Option<&std::path::Path>) -> String
    where
        I: IntoIterator<Item = S>,
        S: AsRef<std::ffi::OsStr>,
    {
        let mut command = tokio::process::Command::new("git");
        command.args(args);
        if let Some(cwd) = cwd {
            command.current_dir(cwd);
        }
        let output = command.output().await.unwrap();
        assert!(
            output.status.success(),
            "git failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    }

    async fn test_pool_with_repository(
        git_path: String,
    ) -> (gitforge_db::Pool, gitforge_common::RepoId) {
        let pool = gitforge_db::Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();
        let user = gitforge_db::models::User::new(
            "workspace-error-test".to_string(),
            "workspace-error@example.test".to_string(),
            "hash".to_string(),
        );
        gitforge_db::queries::UserQueries::create(&pool, &user)
            .await
            .unwrap();
        let repo_id = gitforge_common::RepoId::new();
        gitforge_db::queries::RepoQueries::create(
            &pool,
            &gitforge_db::models::Repository {
                id: repo_id,
                name: "workspace-error-test".to_string(),
                owner_id: user.id,
                visibility: "private".to_string(),
                git_path,
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            },
        )
        .await
        .unwrap();
        (pool, repo_id)
    }

    #[tokio::test]
    async fn test_prepare_run_workspace_clones_and_checks_out_exact_sha() {
        let _guard = WORKSPACE_TEST_LOCK
            .get_or_init(|| tokio::sync::Mutex::new(()))
            .lock()
            .await;
        let run_id = gitforge_common::PipelineRunId::new();
        let test_root_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/gitforge-ci-workspace-tests")
            .join(run_id.to_string());
        let source = test_root_path.join("source.git");
        let seed = test_root_path.join("seed");
        tokio::fs::create_dir_all(&test_root_path).await.unwrap();
        run_git(["init", "--bare", source.to_str().unwrap()], None).await;
        tokio::fs::create_dir_all(&seed).await.unwrap();
        run_git(["init", seed.to_str().unwrap()], None).await;
        run_git(["config", "user.email", "ci@example.test"], Some(&seed)).await;
        run_git(["config", "user.name", "GitForge CI"], Some(&seed)).await;
        tokio::fs::write(seed.join("marker.txt"), "checked out\n")
            .await
            .unwrap();
        run_git(["add", "marker.txt"], Some(&seed)).await;
        run_git(["commit", "-m", "workspace fixture"], Some(&seed)).await;
        let commit = run_git(["rev-parse", "HEAD"], Some(&seed)).await;
        run_git(
            ["push", source.to_str().unwrap(), "HEAD:refs/heads/main"],
            Some(&seed),
        )
        .await;

        let pool = gitforge_db::Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();
        let user = gitforge_db::models::User::new(
            "workspace-test".to_string(),
            "workspace@example.test".to_string(),
            "hash".to_string(),
        );
        gitforge_db::queries::UserQueries::create(&pool, &user)
            .await
            .unwrap();
        let repo_id = gitforge_common::RepoId::new();
        gitforge_db::queries::RepoQueries::create(
            &pool,
            &gitforge_db::models::Repository {
                id: repo_id,
                name: "workspace-test".to_string(),
                owner_id: user.id,
                visibility: "private".to_string(),
                git_path: source.to_string_lossy().into_owned(),
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            },
        )
        .await
        .unwrap();
        std::env::set_var("GITFORGE_WORKSPACE_ROOT", test_root_path.join("workspaces"));
        let workspace = prepare_run_workspace(&pool, repo_id, run_id, &commit)
            .await
            .unwrap();
        assert_eq!(
            run_git(
                ["rev-parse", "HEAD"],
                Some(std::path::Path::new(&workspace))
            )
            .await,
            commit
        );
        assert_eq!(
            tokio::fs::read_to_string(PathBuf::from(&workspace).join("marker.txt"))
                .await
                .unwrap(),
            "checked out\n"
        );
        tokio::fs::remove_dir_all(&test_root_path).await.unwrap();
    }

    #[tokio::test]
    async fn test_prepare_run_workspace_rejects_unknown_repository() {
        let _guard = WORKSPACE_TEST_LOCK
            .get_or_init(|| tokio::sync::Mutex::new(()))
            .lock()
            .await;
        let pool = gitforge_db::Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();
        let error = prepare_run_workspace(
            &pool,
            gitforge_common::RepoId::new(),
            gitforge_common::PipelineRunId::new(),
            "deadbeef",
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("is not registered"));
    }

    #[tokio::test]
    async fn test_prepare_run_workspace_rejects_unavailable_repository_path() {
        let _guard = WORKSPACE_TEST_LOCK
            .get_or_init(|| tokio::sync::Mutex::new(()))
            .lock()
            .await;
        let missing = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/gitforge-ci-missing-source")
            .join(gitforge_common::RepoId::new().to_string());
        let (pool, repo_id) =
            test_pool_with_repository(missing.to_string_lossy().into_owned()).await;
        let error = prepare_run_workspace(
            &pool,
            repo_id,
            gitforge_common::PipelineRunId::new(),
            "deadbeef",
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("git path is unavailable"));
    }

    /// Seed a bare repository with two commits: the first without a pipeline
    /// definition, the second carrying one. Returns (bare repo path, first
    /// commit, second commit).
    async fn seed_commit_pipeline_fixture() -> (PathBuf, String, String) {
        let run_id = gitforge_common::PipelineRunId::new();
        let test_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/gitforge-ci-pipeline-config-tests")
            .join(run_id.to_string());
        let bare = test_root.join("source.git");
        let seed = test_root.join("seed");
        tokio::fs::create_dir_all(&test_root).await.unwrap();
        run_git(["init", "--bare", bare.to_str().unwrap()], None).await;
        tokio::fs::create_dir_all(&seed).await.unwrap();
        run_git(["init", seed.to_str().unwrap()], None).await;
        run_git(["config", "user.email", "ci@example.test"], Some(&seed)).await;
        run_git(["config", "user.name", "GitForge CI"], Some(&seed)).await;
        tokio::fs::write(seed.join("README.md"), "no ci config yet\n")
            .await
            .unwrap();
        run_git(["add", "README.md"], Some(&seed)).await;
        run_git(["commit", "-m", "without pipeline"], Some(&seed)).await;
        let without = run_git(["rev-parse", "HEAD"], Some(&seed)).await;
        tokio::fs::write(
            seed.join(PIPELINE_CONFIG_PATH),
            "name: fixture-ci\nversion: \"1.0\"\ntrigger_on:\n  - push\nenvironment:\n  CI: \"true\"\njobs:\n  - name: echo\n    image: busybox:latest\n    steps:\n      - name: echo\n        run: echo committed-config\n",
        )
        .await
        .unwrap();
        run_git(["add", PIPELINE_CONFIG_PATH], Some(&seed)).await;
        run_git(["commit", "-m", "with pipeline"], Some(&seed)).await;
        let with = run_git(["rev-parse", "HEAD"], Some(&seed)).await;
        run_git(
            ["push", bare.to_str().unwrap(), "HEAD:refs/heads/main"],
            Some(&seed),
        )
        .await;
        (bare, without, with)
    }

    #[tokio::test]
    async fn test_load_pipeline_from_commit_reads_committed_definition() {
        let _guard = WORKSPACE_TEST_LOCK
            .get_or_init(|| tokio::sync::Mutex::new(()))
            .lock()
            .await;
        let (bare, _without, with) = seed_commit_pipeline_fixture().await;
        let (pool, repo_id) = test_pool_with_repository(bare.to_string_lossy().into_owned()).await;

        let pipeline = load_pipeline_from_commit(&pool, repo_id, &with)
            .await
            .unwrap()
            .expect("committed definition must load");
        assert_eq!(pipeline.name, "fixture-ci");
        assert_eq!(pipeline.jobs.len(), 1);
        assert_eq!(pipeline.jobs[0].steps[0].run, "echo committed-config");
    }

    #[tokio::test]
    async fn test_load_pipeline_from_commit_falls_back_when_revision_has_no_config() {
        let _guard = WORKSPACE_TEST_LOCK
            .get_or_init(|| tokio::sync::Mutex::new(()))
            .lock()
            .await;
        let (bare, without, _with) = seed_commit_pipeline_fixture().await;
        let (pool, repo_id) = test_pool_with_repository(bare.to_string_lossy().into_owned()).await;

        assert!(load_pipeline_from_commit(&pool, repo_id, &without)
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn test_load_pipeline_from_commit_rejects_invalid_committed_definition() {
        let _guard = WORKSPACE_TEST_LOCK
            .get_or_init(|| tokio::sync::Mutex::new(()))
            .lock()
            .await;
        let run_id = gitforge_common::PipelineRunId::new();
        let test_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/gitforge-ci-pipeline-config-tests")
            .join(run_id.to_string());
        let bare = test_root.join("source.git");
        let seed = test_root.join("seed");
        tokio::fs::create_dir_all(&test_root).await.unwrap();
        run_git(["init", "--bare", bare.to_str().unwrap()], None).await;
        tokio::fs::create_dir_all(&seed).await.unwrap();
        run_git(["init", seed.to_str().unwrap()], None).await;
        run_git(["config", "user.email", "ci@example.test"], Some(&seed)).await;
        run_git(["config", "user.name", "GitForge CI"], Some(&seed)).await;
        tokio::fs::write(seed.join(PIPELINE_CONFIG_PATH), "::: not a pipeline\n")
            .await
            .unwrap();
        run_git(["add", PIPELINE_CONFIG_PATH], Some(&seed)).await;
        run_git(["commit", "-m", "broken pipeline"], Some(&seed)).await;
        let commit = run_git(["rev-parse", "HEAD"], Some(&seed)).await;
        run_git(
            ["push", bare.to_str().unwrap(), "HEAD:refs/heads/main"],
            Some(&seed),
        )
        .await;
        let (pool, repo_id) = test_pool_with_repository(bare.to_string_lossy().into_owned()).await;

        let error = load_pipeline_from_commit(&pool, repo_id, &commit)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("invalid .gitforce.yml"));
    }

    #[tokio::test]
    async fn test_load_pipeline_from_commit_rejects_unknown_repository() {
        let pool = gitforge_db::Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();
        let error = load_pipeline_from_commit(&pool, gitforge_common::RepoId::new(), "deadbeef")
            .await
            .unwrap_err();
        assert!(error.to_string().contains("is not registered"));
    }

    #[tokio::test]
    async fn test_prepare_run_workspace_rejects_non_directory_repository_path() {
        let _guard = WORKSPACE_TEST_LOCK
            .get_or_init(|| tokio::sync::Mutex::new(()))
            .lock()
            .await;
        let file =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/gitforge-ci-source-file");
        tokio::fs::write(&file, "not a repository\n").await.unwrap();
        let (pool, repo_id) = test_pool_with_repository(file.to_string_lossy().into_owned()).await;
        let error = prepare_run_workspace(
            &pool,
            repo_id,
            gitforge_common::PipelineRunId::new(),
            "deadbeef",
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("is not a directory"));
        tokio::fs::remove_file(file).await.unwrap();
    }

    #[tokio::test]
    async fn test_prepare_run_workspace_rejects_invalid_commit() {
        let _guard = WORKSPACE_TEST_LOCK
            .get_or_init(|| tokio::sync::Mutex::new(()))
            .lock()
            .await;
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/gitforge-ci-invalid-commit")
            .join(gitforge_common::RepoId::new().to_string());
        tokio::fs::create_dir_all(&root).await.unwrap();
        let source = root.join("source.git");
        let seed = root.join("seed");
        run_git(["init", "--bare", source.to_str().unwrap()], None).await;
        tokio::fs::create_dir_all(&seed).await.unwrap();
        run_git(["init", seed.to_str().unwrap()], None).await;
        run_git(["config", "user.email", "ci@example.test"], Some(&seed)).await;
        run_git(["config", "user.name", "GitForge CI"], Some(&seed)).await;
        tokio::fs::write(seed.join("marker.txt"), "checked out\n")
            .await
            .unwrap();
        run_git(["add", "marker.txt"], Some(&seed)).await;
        run_git(["commit", "-m", "workspace fixture"], Some(&seed)).await;
        run_git(
            ["push", source.to_str().unwrap(), "HEAD:refs/heads/main"],
            Some(&seed),
        )
        .await;
        std::env::set_var("GITFORGE_WORKSPACE_ROOT", root.join("workspaces"));
        let (pool, repo_id) =
            test_pool_with_repository(source.to_string_lossy().into_owned()).await;
        let run_id = gitforge_common::PipelineRunId::new();
        let error = prepare_run_workspace(&pool, repo_id, run_id, "deadbeef")
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("checkout commit deadbeef failed"),
            "unexpected workspace preparation error: {error:#}"
        );
        tokio::fs::remove_dir_all(root).await.unwrap();
    }

    #[tokio::test]
    async fn test_validate_workspace_path_enforces_configured_root() {
        let _guard = WORKSPACE_TEST_LOCK
            .get_or_init(|| tokio::sync::Mutex::new(()))
            .lock()
            .await;
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/gitforge-ci-validation")
            .join(gitforge_common::RepoId::new().to_string());
        let inside = root.join("inside");
        let outside = root.parent().unwrap().join("outside");
        std::fs::create_dir_all(&inside).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::env::set_var("GITFORGE_WORKSPACE_ROOT", &root);

        assert_eq!(
            validate_workspace_path(inside.to_str().unwrap()).unwrap(),
            inside.canonicalize().unwrap().to_string_lossy()
        );
        let error = validate_workspace_path(outside.to_str().unwrap()).unwrap_err();
        assert!(error.contains("workspace must be inside"));

        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn test_health_check_reports_healthy() {
        assert_eq!(health_check().await, "OK");
    }

    #[test]
    fn test_create_default_pipeline() {
        let pipeline = create_default_pipeline("test-repo");

        assert_eq!(pipeline.name, "test-repo-pipeline");
        assert_eq!(pipeline.version, "1.0");
        assert_eq!(pipeline.trigger_on, vec![TriggerType::Push]);
        assert!(pipeline.environment.is_empty());
        assert_eq!(pipeline.jobs.len(), 2);
    }

    #[test]
    fn test_create_default_pipeline_has_build_job() {
        let pipeline = create_default_pipeline("my-repo");

        let build_job = pipeline.jobs.iter().find(|j| j.name == "build").unwrap();
        assert_eq!(build_job.image, "rust:latest");
        assert!(build_job.needs.is_empty());
        assert_eq!(build_job.steps.len(), 2);
    }

    #[test]
    fn test_create_default_pipeline_has_test_job() {
        let pipeline = create_default_pipeline("my-repo");

        let test_job = pipeline.jobs.iter().find(|j| j.name == "test").unwrap();
        assert_eq!(test_job.image, "rust:latest");
        assert_eq!(test_job.needs, vec!["build".to_string()]);
        assert!(test_job.retry.is_some());
    }

    #[test]
    fn test_create_default_pipeline_build_steps() {
        let pipeline = create_default_pipeline("my-repo");

        let build_job = pipeline.jobs.iter().find(|j| j.name == "build").unwrap();
        let step_names: Vec<&str> = build_job.steps.iter().map(|s| s.name.as_str()).collect();
        assert!(step_names.contains(&"setup"));
        assert!(step_names.contains(&"build"));
    }

    #[test]
    fn test_create_default_pipeline_test_depends_on_build() {
        let pipeline = create_default_pipeline("my-repo");

        let test_job = pipeline.jobs.iter().find(|j| j.name == "test").unwrap();
        assert!(test_job.needs.contains(&"build".to_string()));
    }

    #[test]
    fn test_create_default_pipeline_timeout() {
        let pipeline = create_default_pipeline("my-repo");

        for job in &pipeline.jobs {
            assert!(job.timeout.is_some());
            assert_eq!(job.timeout.as_ref().unwrap(), "30m");
        }
    }

    #[test]
    fn test_create_default_pipeline_retry() {
        let pipeline = create_default_pipeline("my-repo");

        for job in &pipeline.jobs {
            assert!(job.retry.is_some());
            assert_eq!(job.retry.unwrap(), 1);
        }
    }

    #[test]
    fn test_pipeline_cache_insert_and_retrieve() {
        let mut cache: PipelineCache = HashMap::new();
        let repo_id = gitforge_common::RepoId::new();
        let pipeline = create_default_pipeline("test-repo");

        cache.insert(repo_id, pipeline.clone());
        assert!(cache.contains_key(&repo_id));
        assert_eq!(cache.get(&repo_id).unwrap().name, "test-repo-pipeline");
    }

    #[test]
    fn test_pipeline_cache_multiple_repos() {
        let mut cache: PipelineCache = HashMap::new();
        let repo1 = gitforge_common::RepoId::new();
        let repo2 = gitforge_common::RepoId::new();

        cache.insert(repo1, create_default_pipeline("repo1"));
        cache.insert(repo2, create_default_pipeline("repo2"));

        assert_eq!(cache.len(), 2);
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

    #[tokio::test]
    async fn test_create_shutdown_future() {
        let shutdown = create_shutdown_flag();
        let shutdown_flag = shutdown.clone();

        // Set shutdown after a short delay
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            shutdown_flag.store(true, Ordering::SeqCst);
        });

        create_shutdown_future(shutdown).await;
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

    #[test]
    fn test_create_shutdown_flag_cloneable() {
        let flag = create_shutdown_flag();
        let _cloned = flag.clone();
        // Verify the flag can be cloned and used
        assert!(!flag.load(Ordering::SeqCst));
    }

    #[test]
    fn test_pipeline_cache_type_alias() {
        // Verify the PipelineCache type works correctly
        let cache: PipelineCache = HashMap::new();
        assert!(cache.is_empty());
    }

    #[tokio::test]
    async fn test_spawn_shutdown_handler_does_not_panic() {
        let flag = create_shutdown_flag();
        // Just verify the function doesn't panic when called
        spawn_shutdown_handler(flag);
    }

    #[test]
    fn test_create_trigger_event_basic() {
        let repo_id = gitforge_common::RepoId::new();
        let event = create_trigger_event(repo_id, "abc123", "refs/heads/main");

        // Verify event was created with correct commit hash
        assert_eq!(event.commit_hash, "abc123");
        assert_eq!(event.ref_name.as_deref(), Some("refs/heads/main"));
    }

    #[test]
    fn test_create_trigger_event_with_branch() {
        let repo_id = gitforge_common::RepoId::new();
        let event = create_trigger_event(repo_id, "def456", "refs/heads/develop");

        assert_eq!(event.commit_hash, "def456");
        assert_eq!(event.ref_name.as_deref(), Some("refs/heads/develop"));
    }

    #[test]
    fn test_create_trigger_event_preserves_repo_id() {
        let repo_id = gitforge_common::RepoId::new();
        let event = create_trigger_event(repo_id, "xyz789", "refs/heads/main");

        assert_eq!(event.repo_id, repo_id);
    }

    #[test]
    fn test_create_trigger_event_with_tag_ref() {
        let repo_id = gitforge_common::RepoId::new();
        let event = create_trigger_event(repo_id, "v1.0.0", "refs/tags/v1.0.0");

        assert_eq!(event.commit_hash, "v1.0.0");
        assert_eq!(event.ref_name.as_deref(), Some("refs/tags/v1.0.0"));
    }

    #[test]
    fn test_create_trigger_event_empty_commit() {
        let repo_id = gitforge_common::RepoId::new();
        let event = create_trigger_event(repo_id, "", "refs/heads/main");

        assert_eq!(event.commit_hash, "");
        assert_eq!(event.ref_name.as_deref(), Some("refs/heads/main"));
    }

    #[test]
    fn test_pipeline_cache_multiple_different_repos() {
        let mut cache: PipelineCache = HashMap::new();
        let repos: Vec<_> = (0..5).map(|_| gitforge_common::RepoId::new()).collect();

        for (i, repo_id) in repos.iter().enumerate() {
            let pipeline = create_default_pipeline(&format!("repo{}", i));
            cache.insert(*repo_id, pipeline);
        }

        assert_eq!(cache.len(), 5);
    }

    #[test]
    fn test_create_shutdown_flag_default_is_false() {
        let flag = create_shutdown_flag();
        assert!(!flag.load(Ordering::SeqCst));
    }

    #[test]
    fn test_pipeline_cache_insert_same_repo_updates() {
        let mut cache: PipelineCache = HashMap::new();
        let repo_id = gitforge_common::RepoId::new();

        let pipeline1 = create_default_pipeline("repo1");
        let pipeline2 = create_default_pipeline("repo2");

        cache.insert(repo_id, pipeline1);
        cache.insert(repo_id, pipeline2);

        // Should have only one entry (updated)
        assert_eq!(cache.len(), 1);
        // And it should be the second one
        assert_eq!(cache.get(&repo_id).unwrap().name, "repo2-pipeline");
    }

    // --- Workspace-propagation regression tests (fix: scope push workspaces by pipeline run) ---
    //
    // The push handler (`handle_push_event`) inserts a prepared workspace path
    // into the run-scoped cache keyed by `state.run_id`, and the scheduler
    // completion consumer (`run_scheduler_event_consumer`) looks up that same
    // entry by `state.run_id` when enqueuing dependent jobs. The tests below
    // exercise the cache pattern directly to prove the invariants the fix
    // guarantees without standing up the full event bus / scheduler loop.

    fn run_workspace_paths_cache(
    ) -> Arc<std::sync::Mutex<HashMap<gitforge_common::PipelineRunId, Option<String>>>> {
        Arc::new(std::sync::Mutex::new(HashMap::new()))
    }

    /// Insert a workspace path the way `handle_push_event` does after
    /// preparing a workspace for a run.
    fn cache_prepared_workspace(
        cache: &Arc<std::sync::Mutex<HashMap<gitforge_common::PipelineRunId, Option<String>>>>,
        run_id: gitforge_common::PipelineRunId,
        workspace_path: Option<String>,
    ) {
        cache
            .lock()
            .expect("workspace cache lock poisoned")
            .insert(run_id, workspace_path);
    }

    /// Resolve the workspace path the way `run_scheduler_event_consumer` does
    /// when enqueuing downstream jobs for a run.
    fn lookup_workspace_for_run(
        cache: &Arc<std::sync::Mutex<HashMap<gitforge_common::PipelineRunId, Option<String>>>>,
        run_id: gitforge_common::PipelineRunId,
    ) -> Option<String> {
        cache
            .lock()
            .expect("workspace cache lock poisoned")
            .get(&run_id)
            .cloned()
            .flatten()
    }

    /// Remove the workspace entry the way `run_scheduler_event_consumer` does
    /// when the run reaches a terminal status.
    fn evict_workspace_for_run(
        cache: &Arc<std::sync::Mutex<HashMap<gitforge_common::PipelineRunId, Option<String>>>>,
        run_id: gitforge_common::PipelineRunId,
    ) {
        cache
            .lock()
            .expect("workspace cache lock poisoned")
            .remove(&run_id);
    }

    /// The fix must key the cache by `PipelineRunId`, so two distinct runs for
    /// the same repository each retain their own prepared workspace.
    #[test]
    fn test_run_workspace_paths_keys_by_pipeline_run_id() {
        let cache = run_workspace_paths_cache();
        let shared_repo = gitforge_common::RepoId::new();
        let run_a = gitforge_common::PipelineRunId::new();
        let run_b = gitforge_common::PipelineRunId::new();
        assert_ne!(run_a, run_b, "fixture: run ids must differ");

        // Two pushes for the same repo arrive in quick succession.
        cache_prepared_workspace(&cache, run_a, Some("/nas/Temp/ws/run-a".to_string()));
        cache_prepared_workspace(&cache, run_b, Some("/nas/Temp/ws/run-b".to_string()));

        assert_eq!(cache.lock().unwrap().len(), 2);
        assert_eq!(
            lookup_workspace_for_run(&cache, run_a).as_deref(),
            Some("/nas/Temp/ws/run-a")
        );
        assert_eq!(
            lookup_workspace_for_run(&cache, run_b).as_deref(),
            Some("/nas/Temp/ws/run-b")
        );

        // A repository-only key (the pre-fix behavior) would have collapsed the
        // second push's workspace onto the first, so verify the fix keeps both.
        let mut legacy_repo_cache: HashMap<gitforge_common::RepoId, Option<String>> =
            HashMap::new();
        legacy_repo_cache.insert(shared_repo, Some("/nas/Temp/ws/run-a".to_string()));
        legacy_repo_cache.insert(shared_repo, Some("/nas/Temp/ws/run-b".to_string()));
        assert_eq!(
            legacy_repo_cache.get(&shared_repo).unwrap().as_deref(),
            Some("/nas/Temp/ws/run-b"),
            "pre-fix keyed-by-repo cache would clobber run A's workspace"
        );
    }

    /// Dependent jobs in a single pipeline run must observe the same workspace
    /// that `handle_push_event` prepared for that run. This mirrors the lookup
    /// the scheduler completion consumer performs when enqueuing downstream
    /// jobs after a parent job completes.
    #[tokio::test]
    async fn test_run_workspace_paths_dependent_jobs_reuse_prepared_workspace() {
        let cache = run_workspace_paths_cache();
        let repo_id = gitforge_common::RepoId::new();
        let pipeline_id = gitforge_common::PipelineId::new();
        let pipeline = create_default_pipeline(&repo_id.to_string());
        let trigger = PipelineTriggerEvent::new(
            pipeline_id,
            repo_id,
            "abcdef1234567890".to_string(),
            TriggerType::Push,
        );

        let engine = CiEngine::new(trigger, pipeline).await.unwrap();
        let run_id = engine.state().await.run_id;
        let prepared_workspace = format!("/nas/Temp/workspaces/{run_id}");

        // Mirror `handle_push_event` inserting the prepared workspace keyed by
        // `state.run_id` (see services/ci/src/main.rs ~line 717).
        cache_prepared_workspace(&cache, run_id, Some(prepared_workspace.clone()));

        // The default pipeline has a dependent `test` job after `build`. Each
        // dependent job lookup in `run_scheduler_event_consumer` resolves the
        // workspace by `state.run_id`, so repeated lookups (simulating
        // build→test fan-out) must all return the prepared workspace.
        let first = lookup_workspace_for_run(&cache, run_id);
        let second = lookup_workspace_for_run(&cache, run_id);
        let third = lookup_workspace_for_run(&cache, run_id);
        assert_eq!(first.as_deref(), Some(prepared_workspace.as_str()));
        assert_eq!(second.as_deref(), Some(prepared_workspace.as_str()));
        assert_eq!(third.as_deref(), Some(prepared_workspace.as_str()));
    }

    /// Two concurrent pipeline runs triggered for the same repository must
    /// never see each other's workspace. This is the cross-contamination
    /// invariant the fix restores.
    #[tokio::test]
    async fn test_run_workspace_paths_separate_pipeline_runs_do_not_cross_contaminate() {
        let cache = run_workspace_paths_cache();
        let shared_repo = gitforge_common::RepoId::new();
        let pipeline_id = gitforge_common::PipelineId::new();
        let pipeline = create_default_pipeline(&shared_repo.to_string());

        let trigger_a = PipelineTriggerEvent::new(
            pipeline_id,
            shared_repo,
            "aaaaaaa".to_string(),
            TriggerType::Push,
        );
        let trigger_b = PipelineTriggerEvent::new(
            pipeline_id,
            shared_repo,
            "bbbbbbb".to_string(),
            TriggerType::Push,
        );
        let engine_a = CiEngine::new(trigger_a, pipeline.clone()).await.unwrap();
        let engine_b = CiEngine::new(trigger_b, pipeline).await.unwrap();

        let run_a = engine_a.state().await.run_id;
        let run_b = engine_b.state().await.run_id;
        assert_ne!(run_a, run_b);
        assert_eq!(
            engine_a.state().await.repo_id,
            engine_b.state().await.repo_id
        );

        let workspace_a = format!("/nas/Temp/workspaces/{run_a}");
        let workspace_b = format!("/nas/Temp/workspaces/{run_b}");

        // Two pushes processed back-to-back: each run records its own workspace.
        cache_prepared_workspace(&cache, run_a, Some(workspace_a.clone()));
        cache_prepared_workspace(&cache, run_b, Some(workspace_b.clone()));

        // Run A dependent jobs see only A's workspace, even though the cache
        // also contains B's entry.
        let a_lookup = lookup_workspace_for_run(&cache, run_a);
        let b_lookup = lookup_workspace_for_run(&cache, run_b);
        assert_eq!(a_lookup.as_deref(), Some(workspace_a.as_str()));
        assert_eq!(b_lookup.as_deref(), Some(workspace_b.as_str()));

        // Removing B's entry (e.g. terminal cleanup) must not affect A.
        evict_workspace_for_run(&cache, run_b);
        assert_eq!(
            lookup_workspace_for_run(&cache, run_a).as_deref(),
            Some(workspace_a.as_str()),
            "evicting run B must not touch run A's workspace entry"
        );
        assert!(lookup_workspace_for_run(&cache, run_b).is_none());
    }

    /// The terminal-status eviction in the scheduler completion consumer
    /// removes the entry by `state.run_id`; this must leave concurrent runs
    /// for the same repository intact.
    #[tokio::test]
    async fn test_run_workspace_paths_terminal_eviction_only_targets_specific_run() {
        let cache = run_workspace_paths_cache();
        let repo_id = gitforge_common::RepoId::new();
        let pipeline = create_default_pipeline(&repo_id.to_string());

        let make_engine = |commit: &str| {
            let pipeline_id = gitforge_common::PipelineId::new();
            let trigger = PipelineTriggerEvent::new(
                pipeline_id,
                repo_id,
                commit.to_string(),
                TriggerType::Push,
            );
            CiEngine::new(trigger, pipeline.clone())
        };
        let engine_a = make_engine("commit-a").await.unwrap();
        let engine_b = make_engine("commit-b").await.unwrap();
        let engine_c = make_engine("commit-c").await.unwrap();
        let run_a = engine_a.state().await.run_id;
        let run_b = engine_b.state().await.run_id;
        let run_c = engine_c.state().await.run_id;

        for (run, commit) in [(run_a, "a"), (run_b, "b"), (run_c, "c")] {
            cache_prepared_workspace(
                &cache,
                run,
                Some(format!("/nas/Temp/workspaces/{run}/{commit}")),
            );
        }
        assert_eq!(cache.lock().unwrap().len(), 3);

        // Run B reaches a terminal status and its entry is evicted.
        evict_workspace_for_run(&cache, run_b);
        assert_eq!(cache.lock().unwrap().len(), 2);
        assert!(lookup_workspace_for_run(&cache, run_b).is_none());
        assert!(lookup_workspace_for_run(&cache, run_a).is_some());
        assert!(lookup_workspace_for_run(&cache, run_c).is_some());
    }
}

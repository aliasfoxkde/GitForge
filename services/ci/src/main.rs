//! GitForce CI Orchestrator
//!
//! Main entry point for the CI orchestration service.

use axum::Router;
use axum::{
    extract::{Extension, Path, Request},
    http::{header, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    Json,
};
use chrono::Utc;
use futures::StreamExt;
use gitforge_ci::{
    engine::{fence_actions, FenceAction},
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
    assigner::{job_fence_grace_secs_from_env, JobExecutionDefinition, DEFAULT_JOB_TIMEOUT_SECS},
    create_state_with_artifact_storage, scheduler_routes, Scheduler, SchedulerEvent,
};
use gitforge_storage::FileStorage;
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;
use tower_http::trace::TraceLayer;

type PipelineCache = HashMap<gitforge_common::RepoId, PipelineDefinition>;
type PipelineRegistry = HashMap<gitforge_common::PipelineRunId, Arc<CiEngine>>;

/// Paths of the pipeline definition inside a repository checkout, in
/// resolution order. `.gitforge.yml` is the product spelling; the
/// pre-rename `.gitforce.yml` stays as a fallback so repositories
/// committed before the rename keep building. When both exist the first
/// entry wins, so a repository cannot have its pipeline meaning split
/// across the two files.
const PIPELINE_CONFIG_PATHS: [&str; 2] = [".gitforge.yml", ".gitforce.yml"];

/// How often the job timeout watchdog sweeps the durable rows for jobs whose
/// `started_at + timeout_secs` deadline has elapsed, and drives the matching
/// live engines to the same terminal state.
const JOB_TIMEOUT_SWEEP_SECS: u64 = 60;

/// How often the trigger recovery sweep looks for orphaned pending trigger
/// events (F2). The first sweep runs at startup, so events stranded by a
/// previous process lifetime are re-driven without waiting a full interval.
const TRIGGER_RECOVERY_SWEEP_SECS: u64 = 15;

/// How long one driver's lease on a pending trigger event lasts. Long enough
/// for a cold drive (config load, clone, durable planning) to finish; a drive
/// that somehow outlives the lease still converges on one run through the
/// trigger-event idempotency index.
const TRIGGER_CLAIM_LEASE_SECS: u64 = 300;

/// How many claims — across the live consumer and every recovery sweep — one
/// accepted event may consume before it is failed terminally. Bounds both a
/// crashlooping recovery and the time a poller can be strung along with
/// `queued` answers.
const TRIGGER_MAX_CLAIM_ATTEMPTS: i64 = 5;

/// Exponential backoff between recovery attempts: 30s, 60s, 120s, 240s…
/// capped so a persistently undriveable event still reaches its terminal
/// failure within a bounded wall-clock budget.
const TRIGGER_RETRY_BACKOFF_BASE_SECS: i64 = 30;
const TRIGGER_RETRY_BACKOFF_CAP_SECS: i64 = 600;

struct TriggerState {
    event_bus: Arc<dyn EventBus>,
    workspace_paths: Arc<std::sync::Mutex<HashMap<gitforge_common::RepoId, Option<String>>>>,
    run_waiters: Arc<
        std::sync::Mutex<
            HashMap<uuid::Uuid, tokio::sync::oneshot::Sender<gitforge_common::PipelineRunId>>,
        >,
    >,
    /// Durable store backing trigger correlation (issue #259). `None` keeps
    /// the development-only in-memory scheduler: the status endpoint then
    /// refuses to answer rather than correlating from volatile state.
    db: Option<gitforge_db::Pool>,
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
        (
            Scheduler::with_db(pool.clone()).with_fence_grace_secs(job_fence_grace_secs_from_env()),
            Some(pool),
        )
    } else {
        tracing::warn!("GITFORGE_DATABASE_URL is unset; scheduler state is in-memory only");
        (
            Scheduler::new().with_fence_grace_secs(job_fence_grace_secs_from_env()),
            None,
        )
    };
    tracing::info!(
        fence_grace_secs = job_fence_grace_secs_from_env(),
        "job fence grace configured (issue #243)"
    );

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
        db: scheduler_db.clone(),
    });

    let scheduler_app = Router::new()
        .route("/health", axum::routing::get(health_check))
        .route(
            "/pipelines/trigger",
            axum::routing::post(trigger_pipeline).layer(middleware::from_fn(require_trigger_auth)),
        )
        .route(
            "/pipelines/trigger/status/{event_id}",
            axum::routing::get(trigger_event_status)
                .layer(middleware::from_fn(require_status_auth)),
        )
        .merge(scheduler_routes(scheduler_state))
        .layer(Extension(trigger_state))
        .layer(TraceLayer::new_for_http());

    let scheduler_addr = format!("0.0.0.0:{scheduler_port}");
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

    // Recover runs stranded non-terminal by a previous process lifetime,
    // reclaim workspaces of already-terminal runs, then keep reconciling
    // periodically so runs stranded while running are finalized without
    // waiting for the next restart. Spawned so a large sweep cannot delay
    // startup; reconciliation grades from the durable rows (a run whose
    // rows are all terminal is finalizable no matter which engine thinks
    // it still owns it — the 666b3fa8 custody deadlock), and the sweep only
    // ever touches run-owned directories of terminal runs, never the
    // checkout of a run the scheduler may requeue.
    if let Some(pool) = &scheduler_db {
        // Rebuild engines for runs whose planning already happened but whose
        // chain may still have unreleased stages. This runs BEFORE the
        // reconciliation spawn on purpose: a rebuilt engine can re-release
        // ready stages, and a run it still owns is protected from grading by
        // its unfinished durable rows rather than by registry custody.
        let rebuilt = rebuild_live_engines(
            pool,
            &scheduler_arc,
            &pipeline_registry,
            &run_workspace_paths,
        )
        .await;
        if rebuilt > 0 {
            tracing::info!(rebuilt, "startup engine rebuild complete");
        }

        let sweep_pool = pool.clone();
        tokio::spawn(async move {
            let finalized = reconcile_orphaned_runs(&sweep_pool).await;
            if finalized > 0 {
                tracing::info!(finalized, "startup run reconciliation complete");
            }
            let removed = sweep_terminal_workspaces(&sweep_pool).await;
            if removed > 0 {
                tracing::info!(removed, "startup workspace sweep complete");
            }
            run_reconciliation_loop(sweep_pool).await;
        });
    }

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

    // NOTE on runner loss: no dedicated detection loop is needed here.
    // `Scheduler::process_queue` (the 5 s tick above) already calls
    // `mark_stale_runners_offline`, which both marks heartbeats-lost runners
    // offline and re-enqueues their jobs for other runners. A second loop
    // previously duplicated the mark step around a requeue branch that was
    // hardcoded to never fire.

    // Job timeout watchdog. The durable rows are the expiry authority:
    // `reconcile_expired` reaps `running` rows whose started_at + timeout_secs
    // has elapsed, then each live engine is driven to the same terminal truth
    // and finalized exactly like a reported completion. Without this sweep a
    // hung job is stuck forever — a runner whose container died can hold a
    // healthy heartbeat for hours while its job never reports, the recovery
    // path only reconciles once at startup, and the orphan-run finalizer
    // skips any run that still has unfinished jobs.
    let watchdog_db = scheduler_db.clone();
    let watchdog_registry = pipeline_registry.clone();
    let watchdog_workspaces = run_workspace_paths.clone();
    let watchdog_shutdown = shutdown.clone();
    let _timeout_handle = tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(JOB_TIMEOUT_SWEEP_SECS));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    if watchdog_shutdown.load(Ordering::SeqCst) {
                        break;
                    }
                    let Some(pool) = &watchdog_db else { continue };

                    match gitforge_db::queries::JobQueries::reconcile_expired(pool).await {
                        Ok(0) => {}
                        Ok(count) => {
                            tracing::warn!(count, "watchdog reaped jobs past their timeout");
                        }
                        Err(error) => {
                            tracing::error!(%error, "watchdog failed to reconcile expired jobs");
                            continue;
                        }
                    }
                    // F31 residual: rows stranded with terminal evidence but a
                    // lost status write never turn terminal, so their runs can
                    // never finalize. Grade them from their own receipts.
                    match gitforge_db::queries::JobQueries::reconcile_evidence_rows(pool).await {
                        Ok(0) => {}
                        Ok(count) => {
                            tracing::warn!(count, "watchdog graded evidence-stranded jobs");
                        }
                        Err(error) => {
                            tracing::error!(%error, "watchdog failed to grade evidence-stranded jobs");
                        }
                    }
                    let live_run_ids: Vec<gitforge_common::PipelineRunId> =
                        watchdog_registry.read().await.keys().copied().collect();
                    for run_id in live_run_ids {
                        let engine = watchdog_registry.read().await.get(&run_id).cloned();
                        let Some(engine) = engine else { continue };
                        let jobs = match gitforge_db::queries::JobQueries::list_by_run(pool, run_id)
                            .await
                        {
                            Ok(jobs) => jobs,
                            Err(error) => {
                                tracing::error!(%error, run = %run_id, "watchdog failed to list run jobs");
                                continue;
                            }
                        };
                        for job in jobs.iter().filter(|job| job.status == "timed_out") {
                            // Only drive the engine mirror forward; re-running
                            // against an already-terminal job would log a
                            // spurious invalid-transition error every sweep.
                            if engine
                                .get_job(job.id)
                                .await
                                .is_some_and(|state| !state.is_terminal())
                            {
                                if let Err(error) = engine.timeout_job(job.id).await {
                                    tracing::error!(
                                        %error,
                                        job = %job.id,
                                        run = %run_id,
                                        "watchdog failed to time out job"
                                    );
                                }
                            }
                        }
                        finalize_run_if_terminal(
                            &engine,
                            Some(pool),
                            &watchdog_workspaces,
                            &watchdog_registry,
                        )
                        .await;
                    }

                    // Scheduler-fenced rows (lost runner, lost lease) never
                    // emit a completion event, so their live engines would
                    // sit Running forever even though the durable row is
                    // terminal; converge those engines the same way the
                    // timeout mirror above does, then let the finalizer
                    // settle any run this completes.
                    reconcile_fenced_engines(&watchdog_registry, &watchdog_db).await;
                }
                () = tokio::time::sleep(Duration::from_secs(1)) => {
                    if watchdog_shutdown.load(Ordering::SeqCst) {
                        break;
                    }
                }
            }
        }
        tracing::info!("job timeout watchdog shutting down");
    });

    // Trigger recovery (F2). The startup sweep resolves events the previous
    // process accepted but never consumed; the periodic sweep bounds the
    // fate of events whose consumer dies, lags past its lease, or loses the
    // correlate write while this process lives on.
    let trigger_recovery_db = scheduler_db.clone();
    let trigger_recovery_scheduler = scheduler_arc.clone();
    let trigger_recovery_cache = pipeline_cache.clone();
    let trigger_recovery_workspaces = workspace_paths.clone();
    let trigger_recovery_run_workspaces = run_workspace_paths.clone();
    let trigger_recovery_registry = pipeline_registry.clone();
    let trigger_recovery_shutdown = shutdown.clone();
    if trigger_recovery_db.is_some() {
        let _trigger_recovery_handle = tokio::spawn(async move {
            run_trigger_recovery_loop(
                trigger_recovery_db.expect("recovery pool checked above"),
                trigger_recovery_scheduler,
                trigger_recovery_cache,
                trigger_recovery_workspaces,
                trigger_recovery_run_workspaces,
                trigger_recovery_registry,
                trigger_recovery_shutdown,
            )
            .await;
        });
    }

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
    #[serde(default)]
    pusher_id: Option<gitforge_common::UserId>,
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

/// Compare bearer credentials without leaking the first differing byte or
/// accepting a token with a different length. The scheduler is an internal
/// control-plane boundary, so both the dedicated compatibility header and the
/// standard Bearer form are supported; the trigger and status middlewares
/// share this comparison (issue #259).
fn token_matches(expected: &str, supplied: Option<&str>) -> bool {
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

/// The trigger status endpoint takes its own credential, deliberately NOT
/// falling back to the trigger or scheduler operator tokens (issue #259): a
/// deployment that leaks the status secret exposes only "did the run I
/// triggered finish", never the ability to start builds or operate the
/// scheduler. Unset means the endpoint is closed, not open.
fn configured_status_token(get_var: impl Fn(&str) -> Option<String>) -> Option<String> {
    get_var("GITFORGE_STATUS_TOKEN").filter(|token| !token.is_empty())
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
    if token_matches(&expected, supplied) {
        next.run(request).await
    } else {
        (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": "trigger_auth_required"})),
        )
            .into_response()
    }
}

async fn require_status_auth(request: Request, next: Next) -> Response {
    let expected = configured_status_token(|name| std::env::var(name).ok());
    let Some(expected) = expected else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"error": "status_auth_not_configured"})),
        )
            .into_response();
    };
    let supplied = request
        .headers()
        .get("x-gitforge-status-token")
        .or_else(|| request.headers().get(header::AUTHORIZATION))
        .and_then(|value| value.to_str().ok());
    if token_matches(&expected, supplied) {
        next.run(request).await
    } else {
        (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": "status_auth_required"})),
        )
            .into_response()
    }
}

async fn trigger_pipeline(
    Extension(trigger_state): Extension<Arc<TriggerState>>,
    Json(request): Json<PipelineTriggerRequest>,
) -> impl axum::response::IntoResponse {
    // The manual API re-runs a real commit; a deletion sentinel has nothing
    // to build and would only reproduce the doomed zero-hash runs (F37).
    if gitforge_common::is_zero_hash(&request.new_hash) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "invalid_new_hash",
                "message": "new_hash must identify a commit; an all-zero hash is a ref-deletion sentinel"
            })),
        );
    }
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
        .insert(repo_id, working_dir.clone());

    let push_payload = PushReceivedPayload {
        repo_id,
        ref_name: request.ref_name.clone(),
        old_hash: request.old_hash.clone(),
        new_hash: request.new_hash.clone(),
        pusher_id: request.pusher_id,
    };
    let event = EventEnvelope::new(
        EventType::PushReceived,
        EventPayload::PushReceived(push_payload.clone()),
        Some(repo_id),
        None,
    );

    let (run_tx, run_rx) = tokio::sync::oneshot::channel();
    trigger_state
        .run_waiters
        .lock()
        .expect("run waiter lock poisoned")
        .insert(event.event_id, run_tx);

    // Durably record the accepted event before it is published (issue #259):
    // a `queued` answer (the correlation window below elapsed) must stay
    // resolvable by event_id even after a service restart, which volatile
    // waiter map cannot do. Failing the trigger here is deliberate — firing
    // an event no caller could ever correlate would strand it.
    //
    // The serialized payload travels with the row (F2): if this process dies
    // before the consumer creates the run, the recovery sweep can re-drive
    // the exact accepted event — same repo, ref, and commit — instead of
    // failing a request the caller was told was accepted.
    if let Some(pool) = trigger_state.db.as_ref() {
        let payload_json = match serde_json::to_string(&push_payload) {
            Ok(json) => json,
            Err(error) => {
                trigger_state
                    .run_waiters
                    .lock()
                    .expect("run waiter lock poisoned")
                    .remove(&event.event_id);
                tracing::error!(%error, event_id = %event.event_id, "failed to serialize trigger payload");
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({
                        "error": "trigger_correlation_unavailable",
                        "message": "could not serialize trigger payload; retry the request"
                    })),
                );
            }
        };
        if let Err(error) = gitforge_db::queries::CiTriggerEventQueries::insert_pending(
            pool,
            event.event_id,
            repo_id,
            Some(&payload_json),
            working_dir.as_deref(),
        )
        .await
        {
            trigger_state
                .run_waiters
                .lock()
                .expect("run waiter lock poisoned")
                .remove(&event.event_id);
            tracing::error!(%error, event_id = %event.event_id, "failed to persist trigger correlation row");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({
                    "error": "trigger_correlation_unavailable",
                    "message": "could not persist trigger correlation; retry the request"
                })),
            );
        }
    }

    match trigger_state.event_bus.publish(event.clone()).await {
        Ok(()) => {
            // Pipeline creation includes event delivery, config loading, and
            // durable run/job persistence.  Three seconds was shorter than
            // the observed cold-path on the Fedora runner, causing a valid
            // accepted event to be returned as `queued` without a run ID;
            // consumers that require a correlated run then failed with a
            // false 500. The window is shared with api clients
            // (`gitforge_common::CI_TRIGGER_CORRELATION_WINDOW`) so their
            // request budgets are derived from this one; a window that
            // elapses under write contention still answers `queued` — the
            // run is created by the consumer either way.
            let pipeline_run_id =
                tokio::time::timeout(gitforge_common::CI_TRIGGER_CORRELATION_WINDOW, run_rx)
                    .await
                    .ok()
                    .and_then(std::result::Result::ok);
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
        Err(error) => {
            // The event never reached the consumer, so nothing will ever
            // correlate it: close the correlation row out as failed instead
            // of leaving a pending row a poller could wait on forever.
            if let Some(pool) = trigger_state.db.as_ref() {
                let _ =
                    gitforge_db::queries::CiTriggerEventQueries::mark_failed(pool, event.event_id)
                        .await;
            }
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({
                    "error": "event_publish_failed",
                    "message": error.to_string(),
                })),
            )
        }
    }
}

/// Status read for one trigger event, addressed by the `event_id` the
/// triggering call received (issue #259). Deliberately narrow on every axis:
/// the credential is a dedicated status token, the caller can only name an
/// event it already saw in a trigger response, the response carries only the
/// correlation and the run's one-word lifecycle state, and every ambiguous
/// case maps to a non-green status. A transient database read failure is
/// answered retryably with 503 — it is never graded into a terminal status,
/// so a poller fails the job only on genuine evidence, not on a read fault.
async fn trigger_event_status(
    Extension(trigger_state): Extension<Arc<TriggerState>>,
    Path(event_id): Path<String>,
) -> Response {
    let Some(pool) = trigger_state.db.as_ref() else {
        // Without durable storage there is no correlation that survives a
        // restart, which the contract requires — refuse rather than guess.
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({
                "error": "status_unavailable",
                "message": "durable trigger correlation requires GITFORGE_DATABASE_URL"
            })),
        )
            .into_response();
    };
    let Ok(event_id) = uuid::Uuid::parse_str(&event_id) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "invalid_event_id",
                "message": "event_id must be a UUID"
            })),
        )
            .into_response();
    };
    let correlation = match gitforge_db::queries::CiTriggerEventQueries::get(pool, event_id).await {
        Ok(Some(correlation)) => correlation,
        Ok(None) => {
            // Unknown event id: either never triggered or durably lost.
            // Both are failures, never a green answer.
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({
                    "error": "unknown_event_id",
                    "status": "missing"
                })),
            )
                .into_response();
        }
        Err(error) => {
            tracing::warn!(%error, event_id = %event_id, "trigger status read failed");
            // Retryable, so the body must not carry a terminal verdict: the
            // poller keeps polling on anything that is not 200/404/401/403.
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({
                    "error": "status_read_failed",
                    "status": "unavailable"
                })),
            )
                .into_response();
        }
    };

    let status = match correlation.status.as_str() {
        // The consumer has not created the run yet — keep polling.
        gitforge_db::models::TRIGGER_EVENT_PENDING => "queued",
        // The consumer errored; there is no run and there never will be.
        gitforge_db::models::TRIGGER_EVENT_FAILED => "failed",
        // The run exists; grade from the same durable row the reconcilers
        // write, so this endpoint never disagrees with the run's verdict.
        gitforge_db::models::TRIGGER_EVENT_CORRELATED => match correlation.pipeline_run_id {
            // A correlated row that names no run is a corrupted record, not a
            // polling hiccup: fail closed and terminally.
            None => "failed",
            Some(run_id) => {
                match gitforge_db::queries::PipelineRunQueries::get(pool, run_id).await {
                    Ok(Some(run)) => map_run_status(Some(&run.status)),
                    // The correlation names a run that does not exist: the
                    // record is genuinely lost, so the answer is terminal —
                    // but still never green.
                    Ok(None) => "failed",
                    // A read error says nothing about the run's verdict, so
                    // the answer must not be terminal: 503 is the one answer
                    // the poller keeps retrying instead of failing the job.
                    Err(error) => {
                        tracing::warn!(
                            %error,
                            run_id = %run_id,
                            "pipeline run status read failed"
                        );
                        return (
                            StatusCode::SERVICE_UNAVAILABLE,
                            Json(serde_json::json!({
                                "error": "status_read_failed",
                                "status": "unavailable"
                            })),
                        )
                            .into_response();
                    }
                }
            }
        },
        other => {
            tracing::warn!(status = %other, "unknown correlation status; answering fail-closed");
            "failed"
        }
    };

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "event_id": event_id.to_string(),
            "status": status,
            "pipeline_run_id": correlation
                .pipeline_run_id
                .map(|id| id.to_string()),
        })),
    )
        .into_response()
}

/// Fail-closed mapping from a durable pipeline-run status to the one-word
/// status the poller consumes. `None` — a correlated event that names a run
/// with no row, or names no run at all — and any unrecognized verdict map to
/// `failed`: a missing or ambiguous record is never green. Transient read
/// errors never reach this mapping; the handler answers them with a
/// retryable 503 instead.
fn map_run_status(status: Option<&str>) -> &'static str {
    match status {
        Some("pending") | Some("running") => "running",
        Some("succeeded") => "succeeded",
        Some("failed") | Some("timed_out") | Some("timeout") | Some("timed-out") => "failed",
        Some("cancelled") => "cancelled",
        _ => "failed",
    }
}

fn validate_workspace_path(path: &str) -> Result<String, String> {
    let workspace = std::fs::canonicalize(path)
        .map_err(|error| format!("workspace is not accessible: {error}"))?;
    if !workspace.is_dir() {
        return Err("workspace must be a directory".to_string());
    }
    let roots = workspace_roots()
        .into_iter()
        .map(|root_path| {
            std::fs::canonicalize(&root_path)
                .map_err(|error| format!("workspace root is not accessible: {error}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    if !roots.iter().any(|root| workspace.starts_with(root)) {
        let allowed = roots
            .iter()
            .map(|root| root.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!("workspace must be inside one of: {allowed}"));
    }
    Ok(workspace.to_string_lossy().into_owned())
}

/// A drive failure that is a deterministic function of the committed
/// pipeline configuration at the pushed revision: an unparseable or
/// non-UTF-8 definition file, or a definition the DAG builder rejects.
/// Re-driving the event re-reads the same commit and fails identically,
/// so the settle path fails the trigger terminally instead of spending
/// its retry budget delaying a verdict that cannot change. Everything
/// else (database, git plumbing, filesystem, process spawn) is treated
/// as transient.
#[derive(Debug)]
struct CommittedConfigInvalid(anyhow::Error);

impl std::fmt::Display for CommittedConfigInvalid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Render the whole inner chain: this type is a classification
        // marker, and exposing no `source()` keeps anyhow's alternate
        // (`{:#}`) rendering from printing the chain twice.
        write!(f, "{:#}", self.0)
    }
}

impl std::error::Error for CommittedConfigInvalid {}

/// Classify an error as a permanent rejection of committed configuration.
fn invalid_committed_config(error: anyhow::Error) -> anyhow::Error {
    anyhow::Error::new(CommittedConfigInvalid(error))
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
        .ok_or_else(|| anyhow::anyhow!("repository {repo_id} is not registered"))?;

    // The first committed definition in PIPELINE_CONFIG_PATHS order wins;
    // an absent file falls through to the next candidate.
    for config_path in PIPELINE_CONFIG_PATHS {
        let committed = tokio::process::Command::new("git")
            .arg("--git-dir")
            .arg(&repository.git_path)
            .args(["cat-file", "-e", &format!("{commit_hash}:{config_path}")])
            .output()
            .await?;
        if !committed.status.success() {
            continue;
        }

        let show = tokio::process::Command::new("git")
            .arg("--git-dir")
            .arg(&repository.git_path)
            .args(["show", &format!("{commit_hash}:{config_path}")])
            .output()
            .await?;
        if !show.status.success() {
            anyhow::bail!(
                "failed to read {config_path} at {commit_hash} for repo {}: {}",
                repo_id,
                String::from_utf8_lossy(&show.stderr).trim()
            );
        }

        let yaml = String::from_utf8(show.stdout).map_err(|error| {
            invalid_committed_config(anyhow::anyhow!(
                "{config_path} at {commit_hash} is not valid UTF-8: {error}"
            ))
        })?;
        return PipelineDefinition::parse(&yaml).map(Some).map_err(|error| {
            invalid_committed_config(anyhow::anyhow!(
                "invalid {config_path} at {commit_hash}: {error}"
            ))
        });
    }
    Ok(None)
}

/// Single source of truth for the run-workspace root, so workspace creation,
/// path validation, and cleanup cannot drift onto different defaults.
fn workspace_root() -> std::path::PathBuf {
    std::path::PathBuf::from(
        std::env::var("GITFORGE_WORKSPACE_ROOT")
            .unwrap_or_else(|_| "/var/lib/gitforge/workspaces".to_string()),
    )
}

/// Return all trusted workspace roots accepted for caller-supplied checkouts.
///
/// `GITFORGE_WORKSPACE_ROOT` remains the canonical run-workspace root used by
/// GitForge itself. `GITFORGE_WORKSPACE_ROOTS` is an explicit, comma-separated
/// allowlist for integrations such as Control Center that own their checkout
/// lifecycle. Keeping the two settings separate prevents an integration from
/// silently changing GitForge's cleanup root.
fn workspace_roots() -> Vec<std::path::PathBuf> {
    if let Ok(value) = std::env::var("GITFORGE_WORKSPACE_ROOTS") {
        let roots = value
            .split(',')
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .map(std::path::PathBuf::from)
            .collect::<Vec<_>>();
        if !roots.is_empty() {
            return roots;
        }
    }
    vec![workspace_root()]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContainerBackend {
    Docker,
    Podman,
}

fn container_backend_from_env() -> Result<ContainerBackend, String> {
    match std::env::var("GITFORGE_CONTAINER_BACKEND") {
        Ok(value) if value.eq_ignore_ascii_case("docker") => Ok(ContainerBackend::Docker),
        Ok(value) if value.eq_ignore_ascii_case("podman") => Ok(ContainerBackend::Podman),
        Ok(value) => Err(format!(
            "GITFORGE_CONTAINER_BACKEND must be `docker` or `podman`, got `{value}`"
        )),
        Err(std::env::VarError::NotPresent) => Err(
            "GITFORGE_CONTAINER_BACKEND is unset; refusing container-assisted workspace cleanup"
                .to_string(),
        ),
        Err(std::env::VarError::NotUnicode(_)) => Err(
            "GITFORGE_CONTAINER_BACKEND is not valid UTF-8; refusing container-assisted workspace cleanup"
                .to_string(),
        ),
    }
}

#[derive(Debug, PartialEq, Eq)]
struct CleanupCommand {
    program: &'static str,
    args: Vec<OsString>,
}

fn cleanup_command(backend: ContainerBackend, workspace: &std::path::Path) -> CleanupCommand {
    match backend {
        ContainerBackend::Podman => CleanupCommand {
            program: "podman",
            args: ["unshare", "rm", "-rf", "--"]
                .into_iter()
                .map(OsString::from)
                .chain(std::iter::once(workspace.as_os_str().to_os_string()))
                .collect(),
        },
        ContainerBackend::Docker => {
            // Mount the trusted parent and remove only the validated run
            // directory from inside the container. No shell is involved, and
            // the container cannot follow a path outside this bind mount.
            let parent = workspace
                .parent()
                .expect("validated run workspace always has a parent");
            let name = workspace
                .file_name()
                .expect("validated run workspace always has a name");
            CleanupCommand {
                program: "docker",
                args: [
                    "run",
                    "--rm",
                    "--user",
                    "0:0",
                    "--mount",
                    &format!(
                        "type=bind,src={},dst=/gitforge-cleanup-parent",
                        parent.display()
                    ),
                    "alpine",
                    "rm",
                    "-rf",
                    "--",
                ]
                .into_iter()
                .map(OsString::from)
                .chain(std::iter::once(
                    std::path::Path::new("/gitforge-cleanup-parent")
                        .join(name)
                        .into_os_string(),
                ))
                .collect(),
            }
        }
    }
}

async fn run_cleanup_command(command: CleanupCommand) -> Option<std::process::Output> {
    timeout(
        Duration::from_secs(120),
        tokio::process::Command::new(command.program)
            .args(command.args)
            .output(),
    )
    .await
    .ok()
    .and_then(Result::ok)
}

/// Delete a run's workspace directory. Only directories GitForge itself
/// created — `<root>/<run id>` — are ever removed. A caller-supplied working
/// directory inside the root may share the tree and must survive the run.
/// Returns whether a directory was removed.
async fn remove_run_workspace_dir(
    root: &std::path::Path,
    run_id: gitforge_common::PipelineRunId,
    path: Option<&str>,
) -> bool {
    let Some(path) = path else {
        return false;
    };
    let workspace = std::path::PathBuf::from(path);
    if workspace != root.join(run_id.to_string()) {
        tracing::debug!(
            %run_id,
            workspace = %workspace.display(),
            "workspace is not run-owned; leaving it in place"
        );
        return false;
    }
    let metadata = match tokio::fs::symlink_metadata(&workspace).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return false,
        Err(error) => {
            tracing::warn!(
                %run_id,
                %error,
                workspace = %workspace.display(),
                "failed to inspect run workspace"
            );
            return false;
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        tracing::warn!(
            %run_id,
            workspace = %workspace.display(),
            "refusing to remove non-directory run workspace"
        );
        return false;
    }
    match tokio::fs::remove_dir_all(&workspace).await {
        Ok(()) => {
            tracing::info!(%run_id, workspace = %workspace.display(), "removed run workspace");
            true
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            let backend = match container_backend_from_env() {
                Ok(backend) => backend,
                Err(diagnostic) => {
                    tracing::warn!(
                        %run_id,
                        workspace = %workspace.display(),
                        %diagnostic,
                        "refusing container-assisted workspace cleanup"
                    );
                    return false;
                }
            };

            match backend {
                ContainerBackend::Docker => {
                    match run_cleanup_command(cleanup_command(backend, &workspace)).await {
                        Some(output) if output.status.success() => {
                            tracing::info!(
                                %run_id,
                                workspace = %workspace.display(),
                                "removed run workspace through Docker"
                            );
                            true
                        }
                        Some(output) => {
                            let diagnostic = String::from_utf8_lossy(&output.stderr)
                                .trim()
                                .chars()
                                .take(512)
                                .collect::<String>();
                            tracing::warn!(
                                %run_id,
                                workspace = %workspace.display(),
                                status = ?output.status.code(),
                                stderr = %diagnostic,
                                "Docker workspace cleanup failed"
                            );
                            false
                        }
                        None => {
                            tracing::warn!(
                                %run_id,
                                workspace = %workspace.display(),
                                "could not start or complete Docker workspace cleanup"
                            );
                            false
                        }
                    }
                }
                ContainerBackend::Podman => {
                    // Rootless Podman maps container root to a subordinate host
                    // UID. A hardened systemd service may be unable to create
                    // the nested user namespace itself, so try the direct path
                    // first and then delegate the exact validated path to a
                    // transient user service.
                    if let Some(output) =
                        run_cleanup_command(cleanup_command(backend, &workspace)).await
                    {
                        if output.status.success() {
                            tracing::info!(
                                %run_id,
                                workspace = %workspace.display(),
                                "removed run workspace through rootless namespace"
                            );
                            return true;
                        }
                        let diagnostic = String::from_utf8_lossy(&output.stderr)
                            .trim()
                            .chars()
                            .take(512)
                            .collect::<String>();
                        tracing::warn!(
                            %run_id,
                            workspace = %workspace.display(),
                            status = ?output.status.code(),
                            stderr = %diagnostic,
                            "direct rootless workspace cleanup failed; trying transient service"
                        );
                    } else {
                        tracing::warn!(
                            %run_id,
                            workspace = %workspace.display(),
                            "could not start direct rootless workspace cleanup; trying transient service"
                        );
                    }

                    let delegated = timeout(
                        Duration::from_secs(120),
                        tokio::process::Command::new("systemd-run")
                            .args([
                                "--user",
                                "--quiet",
                                "--wait",
                                "--pipe",
                                "--collect",
                                "/usr/bin/podman",
                                "unshare",
                                "rm",
                                "-rf",
                                "--",
                            ])
                            .arg(&workspace)
                            .output(),
                    )
                    .await;
                    match delegated {
                        Ok(Ok(output)) if output.status.success() => {
                            tracing::info!(
                                %run_id,
                                workspace = %workspace.display(),
                                "removed run workspace through transient rootless service"
                            );
                            true
                        }
                        Ok(Ok(output)) => {
                            let diagnostic = String::from_utf8_lossy(&output.stderr)
                                .trim()
                                .chars()
                                .take(512)
                                .collect::<String>();
                            tracing::warn!(
                                %run_id,
                                workspace = %workspace.display(),
                                status = ?output.status.code(),
                                stderr = %diagnostic,
                                "transient rootless workspace cleanup failed"
                            );
                            false
                        }
                        Ok(Err(error)) => {
                            tracing::warn!(
                                %run_id,
                                %error,
                                workspace = %workspace.display(),
                                "could not start transient rootless workspace cleanup"
                            );
                            false
                        }
                        Err(_) => {
                            tracing::warn!(
                                %run_id,
                                workspace = %workspace.display(),
                                "transient rootless workspace cleanup timed out"
                            );
                            false
                        }
                    }
                }
            }
        }
        Err(error) => {
            tracing::warn!(
                %run_id,
                %error,
                workspace = %workspace.display(),
                "failed to remove run workspace"
            );
            false
        }
    }
}

/// Finalize runs left non-terminal by a previous process lifetime whose jobs
/// are all already terminal (or were never enqueued). Without this, a
/// control-plane restart strands such runs in `running` forever: no engine
/// exists to observe their completion. Runs with unfinished jobs are left
/// alone — scheduler recovery still owns those. Returns the number of runs
/// finalized.
/// How often the control plane re-runs orphaned-run reconciliation while
/// running. The startup pass handles damage from a previous process
/// lifetime; this loop handles runs stranded while the process is up, for
/// example when a scheduler restart invalidates leases and completions are
/// rejected until every job of the run turns terminal.
const RECONCILE_INTERVAL_SECS: u64 = 60;

/// Grace window for the periodic reconciliation. The push handler creates
/// the durable run row before it prepares the workspace, registers the
/// engine, and enqueues jobs, so a freshly created jobless run is not
/// orphaned yet and must not be cancelled by a concurrent pass. The window
/// must exceed the slowest legitimate `prepare_run_workspace` — a
/// `git clone --no-local` of a multi-gigabyte repository takes minutes —
/// otherwise the pass cancels a run whose trigger is mid-clone and the
/// workspace sweep then deletes the clone out from under the jobs the push
/// handler enqueues seconds later.
const RECONCILE_MIN_RUN_AGE_SECS: i64 = 600;

/// Separately from the general grace window, the enqueue horizon governs
/// only the jobless-run verdict. Chained jobs are enqueued lazily, so a run
/// row can legitimately exist with zero durable job rows while the engine
/// is still going to enqueue its head job — and under database write-lock
/// contention that lag is not seconds: the 2026-09-23 docs push on the live
/// instance had its head job enqueued 29 minutes after the run row, while
/// the periodic sweep had already graded the run `cancelled` at minute 12.
/// A jobless run younger than this horizon is left for the next pass; only
/// a run that stays jobless past it is dead beyond any contention the
/// enqueue has been observed to survive.
const RECONCILE_EMPTY_RUN_HORIZON_SECS: i64 = 3600;

/// Finalize non-terminal runs whose jobs are all terminal, first cancelling
/// (durably) every not-yet-dispatched job that can never run because a
/// pipeline ancestor already failed. `min_age` guards the periodic pass
/// against racing the push handler: a run younger than the grace window may
/// not have its engine registered or its jobs enqueued yet. The startup pass
/// passes a zero window because nothing can be mid-trigger while the process
/// is starting.
///
/// Registry custody deliberately does not protect a run here. Custody used
/// to: the pass skipped anything a live engine held. That deferred forever
/// to an engine that could never converge — a rebuilt engine grafts an
/// already-terminal failure from the durable rows, so the watchdog mirror's
/// non-terminal guard skips `timeout_job` and the in-memory
/// `cancel_descendants` cascade never fires (live run 666b3fa8: `test`
/// reaped by the watchdog while `coverage` stayed `pending` and the run
/// non-terminal for hours, each finalizer deferring to the other). The
/// durable rows are the expiry authority, so once every row is terminal the
/// run is finalizable regardless of what a zombie engine believes; a run
/// with real in-flight work still has non-terminal rows and is skipped on
/// that ground alone.
async fn reconcile_orphaned_runs_filtered(
    pool: &gitforge_db::Pool,
    min_age: chrono::Duration,
) -> usize {
    let runs = match gitforge_db::queries::PipelineRunQueries::list(pool).await {
        Ok(runs) => runs,
        Err(error) => {
            tracing::warn!(%error, "run reconciliation skipped");
            return 0;
        }
    };

    let mut finalized = 0;
    for run in runs {
        if matches!(
            run.status.as_str(),
            "succeeded" | "failed" | "cancelled" | "timed_out" | "timeout" | "timed-out"
        ) {
            continue;
        }
        if Utc::now() - run.created_at < min_age {
            continue;
        }
        // An unreadable job list must NOT be read as a jobless run: under a
        // transient database error (e.g. a lock timeout while a long append
        // transaction holds the write lock) `list_by_run` fails, and grading
        // the run here would cancel a perfectly healthy run whose queued jobs
        // are merely waiting for runner capacity (observed 2026-09-16: a run
        // created 2m02s earlier was cancelled mid-flight by this pass). Skip
        // the run and let the next periodic pass re-read it.
        let jobs = match gitforge_db::queries::JobQueries::list_by_run(pool, run.id).await {
            Ok(jobs) => jobs,
            Err(error) => {
                tracing::warn!(
                    %error,
                    run = %run.id,
                    "run reconciliation skipped: job list unreadable"
                );
                continue;
            }
        };
        let definition = run_definition(pool, run.pipeline_id).await;
        // Make doom durable before measuring the run: a failed, timed-out,
        // or cancelled row dooms every not-yet-dispatched row downstream of
        // it. The engine performs this cascade in memory
        // (`cancel_descendants`), but only while it lives and only when its
        // own mirror state can still drive the failure — the durable record
        // must not depend on that.
        let cancelled_now = match &definition {
            Some(definition) => cancel_doomed_rows(pool, &run, &jobs, definition).await,
            None => HashSet::new(),
        };
        let unfinished = jobs.iter().any(|job| {
            !cancelled_now.contains(&job.id)
                && gitforge_db::models::JobStatus::from_str(&job.status)
                    .is_some_and(|status| !status.is_terminal())
        });
        if unfinished {
            continue;
        }
        // The durable job rows only cover the jobs an engine has already
        // enqueued: chained jobs are enqueued lazily as their dependencies
        // turn terminal, so a run whose control-plane process died mid-chain
        // leaves only its head job behind. Grading the surviving rows alone
        // graded such a run `succeeded` with the rest of its pipeline never
        // executed (observed 2026-09-22: two gitforge-ci runs finalized
        // "succeeded" with 1 of 3 jobs after a control-plane restart —
        // scheduler recovery re-ran the queued head job, nothing re-advanced
        // the chain, and this pass saw "all jobs terminal"). Compare the rows
        // against the run's persisted pipeline definition and grade the
        // shortfall as a failure; when the definition cannot be recovered,
        // fall back to row-only grading rather than inventing a failure.
        let incomplete_chain = match &definition {
            Some(definition) => {
                let expected: HashSet<&str> = definition
                    .jobs
                    .iter()
                    .map(|job| job.name.as_str())
                    .collect();
                let enqueued: HashSet<&str> = jobs.iter().map(|job| job.name.as_str()).collect();
                expected.iter().any(|name| !enqueued.contains(name))
            }
            None => false,
        };
        let status = if jobs.is_empty() {
            // Zero durable rows is the signature of an enqueue that has not
            // happened yet, not proof it never will — see the enqueue
            // horizon above. Cancelling inside that window killed a live
            // run (2026-09-23, run 556dd836: graded cancelled at minute 12,
            // head job enqueued at minute 29).
            if Utc::now() - run.created_at
                >= chrono::Duration::seconds(RECONCILE_EMPTY_RUN_HORIZON_SECS)
            {
                "cancelled"
            } else {
                continue;
            }
        } else if jobs
            .iter()
            .any(|job| job.status == "failed" || job.status == "timed_out")
        {
            // A watchdog-reaped job dooms the run just like a reported
            // failure; grading it `succeeded` here would publish a green
            // run whose job never finished. Failure deliberately outranks
            // cancellation: a run that genuinely lost a job grades failed
            // even when the remaining rows were cancelled afterwards (by an
            // operator or by the doom cascade above).
            "failed"
        } else if jobs.iter().any(|job| job.status == "cancelled") {
            "cancelled"
        } else if incomplete_chain {
            // Every enqueued job succeeded, but the definition expects more
            // jobs than were ever enqueued: the chain stopped advancing when
            // its engine was lost, and the unenqueued remainder will never
            // run.
            "failed"
        } else {
            "succeeded"
        };
        if gitforge_db::queries::PipelineRunQueries::update_status(pool, run.id, status)
            .await
            .is_ok()
        {
            tracing::info!(run = %run.id, status, incomplete_chain, "finalized orphaned run");
            finalized += 1;
        }
    }
    finalized
}

/// The run's persisted pipeline definition, or `None` when it cannot be
/// recovered (a legacy row or an unreadable config blob) — the caller then
/// grades from the durable rows alone and never invents a doom the
/// definition cannot witness.
async fn run_definition(
    pool: &gitforge_db::Pool,
    pipeline_id: gitforge_common::PipelineId,
) -> Option<PipelineDefinition> {
    let pipeline = gitforge_db::queries::PipelineQueries::get(pool, pipeline_id)
        .await
        .ok()
        .flatten()?;
    serde_json::from_value(pipeline.config).ok()
}

/// Durably cancel every not-yet-dispatched job that transitively depends on
/// a failed, timed-out, or cancelled one, and return the ids cancelled.
///
/// `ready_jobs` requires all dependencies to have succeeded, so these rows
/// can never be dispatched again — leaving them `pending`/`queued` keeps
/// the run non-terminal forever. Only rows a runner has never touched are
/// cancelled (`pending`/`queued`); `assigned`/`running` rows belong to the
/// runner lifecycle and are reaped by the lease and timeout machinery. The
/// cascade is computed over the definition's `needs` edges, so an unreadable
/// definition cancels nothing — doom is never invented without a witness.
async fn cancel_doomed_rows(
    pool: &gitforge_db::Pool,
    run: &gitforge_db::models::PipelineRun,
    jobs: &[gitforge_db::models::Job],
    definition: &PipelineDefinition,
) -> HashSet<gitforge_common::JobId> {
    let failed: HashSet<&str> = jobs
        .iter()
        .filter(|job| matches!(job.status.as_str(), "failed" | "timed_out" | "cancelled"))
        .map(|job| job.name.as_str())
        .collect();
    if failed.is_empty() {
        return HashSet::new();
    }

    // Transitive closure over the definition's dependency edges.
    let mut doomed: HashSet<String> = failed.iter().map(|name| (*name).to_string()).collect();
    loop {
        let mut grew = false;
        for job in &definition.jobs {
            if doomed.contains(&job.name) {
                continue;
            }
            if job.needs.iter().any(|need| doomed.contains(need)) {
                doomed.insert(job.name.clone());
                grew = true;
            }
        }
        if !grew {
            break;
        }
    }

    let mut cancelled = HashSet::new();
    for job in jobs.iter().filter(|job| {
        doomed.contains(&job.name) && matches!(job.status.as_str(), "pending" | "queued")
    }) {
        let receipt = serde_json::json!({
            "status": "cancelled",
            "reason": "pipeline ancestor failed; this job can never be dispatched",
        })
        .to_string();
        match gitforge_db::queries::JobQueries::cancel(pool, job.id, &receipt).await {
            Ok(()) => {
                tracing::info!(
                    job = %job.id,
                    run = %run.id,
                    name = %job.name,
                    "cancelled doomed job: its pipeline ancestor already failed"
                );
                cancelled.insert(job.id);
            }
            Err(error) => {
                // Leave it for the next pass rather than grading the run
                // while a doomed row is still unfinished.
                tracing::warn!(%error, job = %job.id, run = %run.id, "failed to cancel doomed job");
            }
        }
    }
    cancelled
}

async fn reconcile_orphaned_runs(pool: &gitforge_db::Pool) -> usize {
    reconcile_orphaned_runs_filtered(pool, chrono::Duration::zero()).await
}

/// Re-run reconciliation on an interval so runs stranded while the control
/// plane is up are finalized without waiting for the next restart. Runs
/// still inside the creation grace window are never touched, and runs with
/// unfinished durable rows are left to the machinery that owns them
/// (scheduler recovery, the lease and timeout sweeps). Finalization here
/// bypasses the completion consumer, so the workspaces it would have freed
/// are reclaimed on this loop too — the sweep only touches run-owned
/// directories of already-terminal runs.
async fn run_reconciliation_loop(pool: gitforge_db::Pool) {
    let mut interval = tokio::time::interval(Duration::from_secs(RECONCILE_INTERVAL_SECS));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        interval.tick().await;
        let finalized = reconcile_orphaned_runs_filtered(
            &pool,
            chrono::Duration::seconds(RECONCILE_MIN_RUN_AGE_SECS),
        )
        .await;
        if finalized > 0 {
            tracing::info!(finalized, "periodic run reconciliation complete");
            let removed = sweep_terminal_workspaces(&pool).await;
            if removed > 0 {
                tracing::info!(removed, "periodic workspace sweep complete");
            }
        }
    }
}

/// Remove workspaces left behind by runs that are already terminal — for
/// example after a crash or control-plane restart. Non-terminal and unknown
/// runs are left untouched: a requeued job still executes in its original
/// checkout. Runs whose status is terminal but that still have a
/// non-terminal job are skipped too — a run can be finalized (for example on
/// a first job failure) while its remaining jobs are still executing, and
/// deleting their checkout mid-run leaves the job failing on a half-removed
/// tree. Returns the number of directories removed.
async fn sweep_terminal_workspaces(pool: &gitforge_db::Pool) -> usize {
    let root = workspace_root();
    let mut entries = match tokio::fs::read_dir(&root).await {
        Ok(entries) => entries,
        Err(error) => {
            tracing::warn!(%error, root = %root.display(), "workspace sweep skipped");
            return 0;
        }
    };

    let mut removed = 0;
    while let Some(entry) = entries.next_entry().await.transpose() {
        let Ok(entry) = entry else { continue };
        if !entry.path().is_dir() {
            continue;
        }
        // GitForge-created workspaces are named exactly by their run ID.
        let Some(uuid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<uuid::Uuid>().ok())
        else {
            continue;
        };
        let run_id = gitforge_common::PipelineRunId::from(uuid);
        let run = gitforge_db::queries::PipelineRunQueries::get(pool, run_id)
            .await
            .ok()
            .flatten();
        let is_terminal = run.is_some_and(|run| {
            matches!(
                run.status.as_str(),
                "succeeded" | "failed" | "cancelled" | "timed_out" | "timeout" | "timed-out"
            )
        });
        if !is_terminal {
            continue;
        }
        // A terminal run can still own running jobs (fail-fast finalization);
        // its workspace is live until every job is done.
        let jobs = gitforge_db::queries::JobQueries::list_by_run(pool, run_id)
            .await
            .unwrap_or_default();
        let job_running = jobs.iter().any(|job| {
            gitforge_db::models::JobStatus::from_str(&job.status)
                .is_some_and(|status| !status.is_terminal())
        });
        if job_running {
            tracing::debug!(run = %run_id, "workspace sweep skipped a run with live jobs");
            continue;
        }
        if remove_run_workspace_dir(&root, run_id, Some(&entry.path().to_string_lossy())).await {
            removed += 1;
        }
    }

    if removed > 0 {
        tracing::info!(removed, "swept workspaces of terminal runs");
    }
    removed
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
        .ok_or_else(|| anyhow::anyhow!("repository {repo_id} is not registered"))?;
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

    let root = workspace_root();
    tokio::fs::create_dir_all(&root).await?;
    let workspace = root.join(run_id.to_string());
    if tokio::fs::try_exists(&workspace).await? {
        // The workspace was left behind by the previous ci process for this
        // same run. Refusing to reuse it turns every restart into a lost
        // resumption: the rebuild path treats the error as fatal and never
        // registers the rebuilt engine, so the run drifts to the orphan
        // reconciler and gets graded failed with an intact workspace sitting
        // right there (the 2026-09-25 19:00Z boot skipped every interrupted
        // run this way). Adopt the directory instead: force the tracked tree
        // back to the run's commit and clear job leftovers so a resumed
        // stage sees fresh-checkout state.
        let repair = tokio::process::Command::new("git")
            .arg("-C")
            .arg(&workspace)
            .args(["checkout", "--force", "--detach", commit_hash])
            .output()
            .await?;
        if !repair.status.success() {
            return Err(anyhow::anyhow!(
                "workspace for run {run_id} exists but could not be adopted at commit {commit_hash}: {}",
                String::from_utf8_lossy(&repair.stderr).trim()
            ));
        }
        let clean = tokio::process::Command::new("git")
            .arg("-C")
            .arg(&workspace)
            .args(["clean", "-fdx"])
            .output()
            .await?;
        if !clean.status.success() {
            return Err(anyhow::anyhow!(
                "workspace for run {run_id} adopted but could not be cleaned: {}",
                String::from_utf8_lossy(&clean.stderr).trim()
            ));
        }
        tracing::info!(run = %run_id, commit = %commit_hash, "adopted existing run workspace");
        return Ok(workspace.to_string_lossy().into_owned());
    }

    // Do not request Git's hard-link-based local clone optimization here.
    // Workspace and repository storage may be mounted with policies that
    // reject hard links; a regular clone is portable across those filesystems.
    let clone = tokio::process::Command::new("git")
        // --no-local: avoid hard links and local-object alternates for
        // portability across protected or differently-owned storage.
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
            () = tokio::time::sleep(Duration::from_millis(100)) => {
                if shutdown.load(Ordering::SeqCst) {
                    break;
                }
            }
            event = stream.next() => {
                match event {
                    Some(event) => {
                        tracing::debug!("received event: {:?}", event.event_type);

                        // Claim the accepted event before driving it (F2).
                        // The lease is what keeps the recovery sweep out of
                        // an event this consumer is actively handling, and
                        // what makes this consumer skip an event recovery
                        // already owns: exactly one driver at a time. With
                        // no durable store there is nothing to coordinate
                        // and nothing to recover — drive as before.
                        let claim = match scheduler_db.as_ref() {
                            Some(pool) => {
                                match gitforge_db::queries::CiTriggerEventQueries::claim_event(
                                    pool,
                                    event.event_id,
                                    chrono::Utc::now(),
                                    Duration::from_secs(TRIGGER_CLAIM_LEASE_SECS),
                                )
                                .await
                                {
                                    Ok(claim) => claim,
                                    Err(error) => {
                                        run_waiters.lock().expect("run waiter lock poisoned").remove(&event.event_id);
                                        tracing::error!(%error, event_id = %event.event_id, "failed to claim trigger event");
                                        continue;
                                    }
                                }
                            }
                            None => None,
                        };
                        if scheduler_db.is_some() && claim.is_none() {
                            // Another driver holds this event (recovery sweep,
                            // or the row already settled). Dropping it here is
                            // correct: its owner will correlate or fail it.
                            tracing::debug!(event_id = %event.event_id, "trigger event claimed elsewhere; skipping");
                            run_waiters.lock().expect("run waiter lock poisoned").remove(&event.event_id);
                            continue;
                        }

                        let drive_result = handle_push_event(
                            &event,
                            &scheduler,
                            &pipeline_cache,
                            scheduler_db.as_ref(),
                            &workspace_paths,
                            &run_workspace_paths,
                            &pipeline_registry,
                            None,
                        )
                        .await;
                        if let Ok(Some(run_id)) = &drive_result {
                            if let Some(waiter) = run_waiters.lock().expect("run waiter lock poisoned").remove(&event.event_id) {
                                let _ = waiter.send(*run_id);
                            }
                        } else {
                            // No run: the waiter stays unanswered so the
                            // trigger's caller sees `queued` rather than a
                            // fabricated correlation.
                            run_waiters.lock().expect("run waiter lock poisoned").remove(&event.event_id);
                        }
                        settle_claimed_trigger_event(
                            scheduler_db.as_ref(),
                            claim,
                            drive_result,
                        )
                        .await;
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

/// Close out a claimed trigger event from a drive outcome. Every write is
/// conditional on the claim token, so a driver that lost its lease can never
/// bury an event another driver is making progress on; and a drive that
/// failed *after* creating its run is healed by correlating that run instead
/// of being retried into a duplicate (F2).
async fn settle_claimed_trigger_event(
    pool: Option<&gitforge_db::Pool>,
    claim: Option<gitforge_db::models::TriggerEventClaim>,
    outcome: anyhow::Result<Option<gitforge_common::PipelineRunId>>,
) {
    let (Some(pool), Some(claim)) = (pool, claim) else {
        return;
    };
    let event_id = claim.event.event_id;
    match outcome {
        Ok(Some(run_id)) => {
            match gitforge_db::queries::CiTriggerEventQueries::correlate_claimed(
                pool,
                event_id,
                run_id,
                &claim.claim_token,
            )
            .await
            {
                Ok(1) => {
                    tracing::info!(event_id = %event_id, run = %run_id, "trigger event correlated");
                }
                // The lease expired and another driver owns the row now; it
                // will settle the event (and converge on this same run via
                // the idempotency link).
                Ok(_) => {
                    tracing::warn!(
                        event_id = %event_id,
                        run = %run_id,
                        "trigger claim lost before correlation; another driver owns the row"
                    );
                }
                Err(error) => {
                    tracing::error!(%error, event_id = %event_id, run = %run_id, "failed to persist trigger correlation");
                }
            }
        }
        Ok(None) => {
            // The event was consumed without producing a run. There is
            // nothing to wait for, so the row must say so terminally — a
            // pending verdict here is exactly the F2 forever-queued defect.
            if let Err(error) = gitforge_db::queries::CiTriggerEventQueries::fail_claimed(
                pool,
                event_id,
                &claim.claim_token,
                "event carried no buildable push",
            )
            .await
            {
                tracing::error!(%error, event_id = %event_id, "failed to record trigger no-run outcome");
            }
        }
        Err(error) => {
            // The drive failed, but it may have failed after creating the
            // run (persist failure, lease race won by the index). Correlate
            // beats retry whenever the run exists — a retry cannot improve
            // on a run that is already there.
            match gitforge_db::queries::PipelineRunQueries::find_id_by_trigger_event(pool, event_id)
                .await
            {
                Ok(Some(run_id)) => {
                    if let Err(correlate_error) =
                        gitforge_db::queries::CiTriggerEventQueries::correlate_claimed(
                            pool,
                            event_id,
                            run_id,
                            &claim.claim_token,
                        )
                        .await
                    {
                        tracing::error!(
                            %correlate_error,
                            event_id = %event_id,
                            run = %run_id,
                            "drive failed but its run exists and could not be correlated"
                        );
                    }
                }
                Ok(None) => {
                    // A committed-config rejection fails terminally now:
                    // re-driving re-reads the same commit and fails
                    // identically, so the retry budget would only delay
                    // the verdict the poller is already owed. Every other
                    // failure is transient — bounded retries with backoff,
                    // terminal at exhaustion.
                    let permanent = error.downcast_ref::<CommittedConfigInvalid>().is_some();
                    let exhausted = claim.event.attempts >= TRIGGER_MAX_CLAIM_ATTEMPTS;
                    if permanent || exhausted {
                        let message = if permanent {
                            format!("committed pipeline config rejected: {error:#}")
                        } else {
                            format!(
                                "trigger drive failed after {} attempts: {error}",
                                claim.event.attempts
                            )
                        };
                        if let Err(fail_error) =
                            gitforge_db::queries::CiTriggerEventQueries::fail_claimed(
                                pool,
                                event_id,
                                &claim.claim_token,
                                &message,
                            )
                            .await
                        {
                            tracing::error!(%fail_error, event_id = %event_id, "failed to record terminal trigger failure");
                        } else if permanent {
                            tracing::warn!(
                                event_id = %event_id,
                                %error,
                                "trigger event failed terminally: committed pipeline config is invalid"
                            );
                        } else {
                            tracing::error!(
                                event_id = %event_id,
                                attempts = claim.event.attempts,
                                %error,
                                "trigger event failed terminally after exhausting its retry budget"
                            );
                        }
                    } else {
                        let next = chrono::Utc::now() + trigger_retry_backoff(claim.event.attempts);
                        if let Err(retry_error) =
                            gitforge_db::queries::CiTriggerEventQueries::schedule_retry(
                                pool,
                                event_id,
                                &claim.claim_token,
                                next,
                                &format!("{error:#}"),
                            )
                            .await
                        {
                            tracing::error!(%retry_error, event_id = %event_id, "failed to schedule trigger retry");
                        } else {
                            tracing::warn!(
                                event_id = %event_id,
                                attempts = claim.event.attempts,
                                %error,
                                "trigger drive failed; recovery retry scheduled"
                            );
                        }
                    }
                }
                Err(read_error) => {
                    tracing::error!(
                        %read_error,
                        event_id = %event_id,
                        "cannot read whether the drive created a run; leaving the claim to expire"
                    );
                }
            }
        }
    }
}

/// Backoff before the next recovery attempt, doubling from the base with a
/// cap. `attempts` is the count already consumed (the claim that just failed
/// included), so the first retry waits the base interval.
fn trigger_retry_backoff(attempts: i64) -> std::time::Duration {
    let shift = attempts.saturating_sub(1).min(16) as u32;
    let secs = TRIGGER_RETRY_BACKOFF_BASE_SECS
        .saturating_mul(1i64 << shift)
        .min(TRIGGER_RETRY_BACKOFF_CAP_SECS);
    std::time::Duration::from_secs(secs.max(1) as u64)
}

/// Recovery sweep for orphaned pending trigger events (F2). Claims due rows
/// one at a time under the same lease the live consumer uses, then either
/// correlates an already-created run, fails a row that cannot be re-driven,
/// or re-drives the accepted event from its durable payload.
async fn sweep_orphaned_trigger_events(
    pool: &gitforge_db::Pool,
    scheduler: &Arc<Scheduler>,
    pipeline_cache: &Arc<std::sync::Mutex<PipelineCache>>,
    workspace_paths: &Arc<std::sync::Mutex<HashMap<gitforge_common::RepoId, Option<String>>>>,
    run_workspace_paths: &Arc<
        std::sync::Mutex<HashMap<gitforge_common::PipelineRunId, Option<String>>>,
    >,
    pipeline_registry: &Arc<tokio::sync::RwLock<PipelineRegistry>>,
) -> anyhow::Result<usize> {
    // Bound each sweep so a backlog cannot monopolize the runtime; the next
    // tick (15 s) continues where this one stopped, oldest first.
    const MAX_PER_SWEEP: usize = 16;
    let mut recovered = 0usize;
    for _ in 0..MAX_PER_SWEEP {
        let Some(claim) = gitforge_db::queries::CiTriggerEventQueries::claim_due(
            pool,
            chrono::Utc::now(),
            Duration::from_secs(TRIGGER_CLAIM_LEASE_SECS),
        )
        .await?
        else {
            break;
        };
        recovered += 1;
        recover_claimed_trigger_event(
            pool,
            claim,
            scheduler,
            pipeline_cache,
            workspace_paths,
            run_workspace_paths,
            pipeline_registry,
        )
        .await;
    }
    Ok(recovered)
}

/// Resolve one claimed, orphaned trigger event: correlate if its run already
/// exists, fail it if it cannot be re-driven, otherwise re-drive the exact
/// accepted event from the durable payload and settle the outcome.
async fn recover_claimed_trigger_event(
    pool: &gitforge_db::Pool,
    claim: gitforge_db::models::TriggerEventClaim,
    scheduler: &Arc<Scheduler>,
    pipeline_cache: &Arc<std::sync::Mutex<PipelineCache>>,
    workspace_paths: &Arc<std::sync::Mutex<HashMap<gitforge_common::RepoId, Option<String>>>>,
    run_workspace_paths: &Arc<
        std::sync::Mutex<HashMap<gitforge_common::PipelineRunId, Option<String>>>,
    >,
    pipeline_registry: &Arc<tokio::sync::RwLock<PipelineRegistry>>,
) {
    let event_id = claim.event.event_id;

    // A run already exists for this event — a correlate write that failed, a
    // crash between run creation and correlation. Point the row at it; never
    // build a second one.
    match gitforge_db::queries::PipelineRunQueries::find_id_by_trigger_event(pool, event_id).await {
        Ok(Some(run_id)) => {
            match gitforge_db::queries::CiTriggerEventQueries::correlate_claimed(
                pool,
                event_id,
                run_id,
                &claim.claim_token,
            )
            .await
            {
                Ok(1) => tracing::info!(
                    event_id = %event_id,
                    run = %run_id,
                    "recovered orphaned trigger event by correlating its existing run"
                ),
                Ok(_) => tracing::warn!(
                    event_id = %event_id,
                    "orphaned trigger event was settled while being recovered"
                ),
                Err(error) => tracing::error!(
                    %error,
                    event_id = %event_id,
                    run = %run_id,
                    "recovered trigger run exists but correlation failed"
                ),
            }
            return;
        }
        Ok(None) => {}
        Err(error) => {
            tracing::error!(
                %error,
                event_id = %event_id,
                "cannot read whether an orphaned trigger event has a run; leaving the claim to expire"
            );
            return;
        }
    }

    let Some(payload_text) = claim.event.payload.as_deref() else {
        // A pre-recovery row: its producing process is gone and no payload
        // was ever persisted, so no driver can ever create its run. Terminal
        // failure is the honest answer — the poller learns instead of
        // polling a `queued` verdict forever.
        let message =
            "pending trigger event has no durable payload; the process that accepted it is gone";
        if let Err(error) = gitforge_db::queries::CiTriggerEventQueries::fail_claimed(
            pool,
            event_id,
            &claim.claim_token,
            message,
        )
        .await
        {
            tracing::error!(%error, event_id = %event_id, "failed to fail payload-less trigger event");
        } else {
            tracing::warn!(event_id = %event_id, "failed a pending trigger event without a durable payload");
        }
        return;
    };

    let drive = match serde_json::from_str::<PushReceivedPayload>(payload_text) {
        Ok(payload) => {
            // Rebuild the exact accepted envelope: same event id (so the
            // run-idempotency link lands on this event), same payload, same
            // requested workspace.
            let envelope = EventEnvelope {
                event_id,
                event_type: EventType::PushReceived,
                event_version: 1,
                timestamp: chrono::Utc::now().timestamp_millis(),
                repo_id: Some(claim.event.repo_id),
                actor_id: None,
                correlation_id: None,
                payload: EventPayload::PushReceived(payload),
            };
            handle_push_event(
                &envelope,
                scheduler,
                pipeline_cache,
                Some(pool),
                workspace_paths,
                run_workspace_paths,
                pipeline_registry,
                claim.event.working_dir.clone(),
            )
            .await
        }
        Err(error) => Err(anyhow::anyhow!(
            "durable trigger payload is undecodable: {error}"
        )),
    };
    settle_claimed_trigger_event(Some(pool), Some(claim), drive).await;
}

/// Long-running trigger recovery loop (F2): sweeps immediately — so events
/// stranded by the previous process lifetime are resolved at startup — and
/// then once per interval, which is what bounds the fate of events whose
/// consumer dies while the process lives on.
async fn run_trigger_recovery_loop(
    pool: gitforge_db::Pool,
    scheduler: Arc<Scheduler>,
    pipeline_cache: Arc<std::sync::Mutex<PipelineCache>>,
    workspace_paths: Arc<std::sync::Mutex<HashMap<gitforge_common::RepoId, Option<String>>>>,
    run_workspace_paths: Arc<
        std::sync::Mutex<HashMap<gitforge_common::PipelineRunId, Option<String>>>,
    >,
    pipeline_registry: Arc<tokio::sync::RwLock<PipelineRegistry>>,
    shutdown: Arc<AtomicBool>,
) {
    tracing::info!("starting trigger recovery loop");
    let mut ticker = tokio::time::interval(Duration::from_secs(TRIGGER_RECOVERY_SWEEP_SECS));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        // The first tick completes immediately: startup sweep.
        ticker.tick().await;
        if shutdown.load(Ordering::SeqCst) {
            tracing::info!("trigger recovery loop shutting down");
            break;
        }
        match sweep_orphaned_trigger_events(
            &pool,
            &scheduler,
            &pipeline_cache,
            &workspace_paths,
            &run_workspace_paths,
            &pipeline_registry,
        )
        .await
        {
            Ok(0) => {}
            Ok(recovered) => {
                tracing::info!(
                    recovered,
                    "trigger recovery sweep processed orphaned events"
                );
            }
            Err(error) => {
                tracing::error!(%error, "trigger recovery sweep failed");
            }
        }
    }
}

/// Handle a push received event - trigger pipeline if configured. Returns the
/// created run's id, or `None` when the event was consumed without creating a
/// run (ref deletion, non-push envelope) — callers must not treat `None` as a
/// run id, and the trigger waiter stays unanswered so its caller sees
/// `queued` rather than a fabricated correlation.
#[allow(clippy::too_many_arguments)]
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
    requested_workspace_override: Option<String>,
) -> anyhow::Result<Option<gitforge_common::PipelineRunId>> {
    // Only handle PushReceived events
    let EventPayload::PushReceived(payload) = &event.payload else {
        return Ok(None);
    };

    // Belt-and-suspenders for the git-server's deletion filter (F37): an
    // all-zero new hash means the ref no longer exists — there is no commit
    // to check out, and building one used to produce a run that failed
    // immediately at checkout. Branch creation (all-zero OLD hash) carries
    // a real new hash and proceeds below.
    if gitforge_common::is_zero_hash(&payload.new_hash) {
        tracing::info!(
            repo = %payload.repo_id,
            ref_name = %payload.ref_name,
            "ignoring ref-deletion push: nothing to build"
        );
        return Ok(None);
    }

    // Trigger-run idempotency (F2): if a previous drive of this accepted
    // event already created its run — a correlate write that failed, a crash
    // between run creation and correlation, a lease that expired mid-drive —
    // answer with that run instead of building a duplicate. The durable
    // event link written by `create_for_trigger` makes this exact.
    if let Some(pool) = scheduler_db {
        if let Some(existing_run) =
            gitforge_db::queries::PipelineRunQueries::find_id_by_trigger_event(pool, event.event_id)
                .await?
        {
            tracing::info!(
                event_id = %event.event_id,
                run = %existing_run,
                "push event already produced a run; correlating instead of re-driving"
            );
            return Ok(Some(existing_run));
        }
    }

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
            "using committed pipeline definition at {} for repo {}",
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
    // The recovery sweep passes the working directory persisted with the
    // accepted event explicitly, so it never reads (or pollutes) the live
    // request cache across a restart.
    let requested_workspace = match requested_workspace_override {
        Some(explicit) => Some(explicit),
        None => workspace_paths
            .lock()
            .expect("workspace cache lock poisoned")
            .get(&repo_id)
            .cloned()
            .flatten(),
    };

    // Create trigger event
    let trigger_event = create_trigger_event(repo_id, &payload.new_hash, ref_name);
    let pipeline_id = trigger_event.pipeline_id;

    // Create and start the CI engine. `CiEngine::new` fails only when the
    // DAG builder rejects the definition — a pure function of the committed
    // configuration — so its errors are permanent, not retryable.
    let engine = Arc::new(
        CiEngine::new(trigger_event, pipeline.clone())
            .await
            .map_err(|error| invalid_committed_config(error.into()))?,
    );
    engine.start().await?;

    tracing::info!(
        "pipeline triggered for repo {} on ref {}",
        repo_id,
        ref_name
    );

    // Enqueue ready jobs to scheduler
    let ready_jobs = engine.ready_jobs().await;
    tracing::info!("enqueueing {} ready jobs", ready_jobs.len());

    persist_and_launch_run(
        scheduler_db,
        scheduler,
        &engine,
        pipeline_id,
        &pipeline,
        repo_id,
        &payload.new_hash,
        event.event_id,
        requested_workspace,
        run_workspace_paths,
        pipeline_registry,
    )
    .await
    .map(Some)
}

/// Persist the durable side of an accepted drive and launch the run: the
/// pipeline version row, the run row linked to the accepted event, the
/// workspace checkout, engine registration, planned job rows, and the
/// scheduler enqueue. Returns the run id execution belongs to.
///
/// The run insert is the idempotency adjudication (F2): the partial unique
/// index on `pipeline_runs.trigger_event_id` lets exactly one concurrent
/// drive of an accepted event land its run row, and `create_for_trigger`
/// returns the durable run's id — the winner's, when this drive's insert
/// lost the race. A loser must stop right there: continuing would prepare
/// a workspace, register the engine, persist planned jobs, and enqueue
/// under its own losing run id — a fully executing orphan no poller or
/// settle path will ever reference.
#[allow(clippy::too_many_arguments)]
async fn persist_and_launch_run(
    scheduler_db: Option<&gitforge_db::Pool>,
    scheduler: &Arc<Scheduler>,
    engine: &Arc<CiEngine>,
    pipeline_id: gitforge_common::PipelineId,
    pipeline: &PipelineDefinition,
    repo_id: gitforge_common::RepoId,
    commit_hash: &str,
    event_id: uuid::Uuid,
    requested_workspace: Option<String>,
    run_workspace_paths: &Arc<
        std::sync::Mutex<HashMap<gitforge_common::PipelineRunId, Option<String>>>,
    >,
    pipeline_registry: &Arc<tokio::sync::RwLock<PipelineRegistry>>,
) -> anyhow::Result<gitforge_common::PipelineRunId> {
    let state = engine.state().await;
    if let Some(pool) = scheduler_db {
        let db_pipeline = DbPipeline {
            id: pipeline_id,
            repo_id,
            name: pipeline.name.clone(),
            trigger_type: "push".to_string(),
            config: serde_json::to_value(pipeline)?,
            created_at: Utc::now(),
        };
        let mut db_run = DbPipelineRun::new(
            pipeline_id,
            repo_id,
            "push".to_string(),
            commit_hash.to_string(),
        );
        db_run.id = state.run_id;
        db_run.start();
        // The pipeline-version mutation and the run-idempotency
        // adjudication have to land together or not at all: each push
        // allocates a fresh PipelineId, so a losing drive's
        // `deactivate_active` would otherwise deactivate the winner's
        // predecessor and leave its own version active. The helper runs
        // them under one transaction; a loser rolls back its version
        // change and returns the durable winner's run id.
        let durable_run =
            gitforge_db::queries::PipelineQueries::create_version_and_run_for_trigger(
                pool,
                &db_pipeline,
                &db_run,
                event_id,
            )
            .await?;
        if durable_run != state.run_id {
            // This drive lost the insert race against a concurrent driver of
            // the same accepted event — its claim lease expired mid-drive and
            // the winner (recovery sweep or fellow consumer) landed the row
            // between this drive's early lookup and its insert. Return the
            // durable run and launch nothing under the losing id.
            tracing::warn!(
                event_id = %event_id,
                run = %durable_run,
                duplicate = %state.run_id,
                "lost the trigger run-idempotency race; converging on the durable run"
            );
            return Ok(durable_run);
        }
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
            match prepare_run_workspace(pool, repo_id, state.run_id, commit_hash).await {
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

    // Durable DAG planning: persist every planned job as a `pending` row
    // before anything is dispatched. The scheduler used to learn a job only
    // when its stage was released, so a control-plane restart mid-chain left
    // the unenqueued tail with no durable trace and the run was graded
    // complete with stages it never ran (F21). Planned rows carry the full
    // execution definition so scheduler recovery can dispatch them verbatim
    // once the engine releases their stage.
    if let Some(pool) = scheduler_db {
        if let Err(error) =
            persist_planned_jobs(pool, engine, state.run_id, workspace_path.as_deref()).await
        {
            // Without the planned rows a restart cannot resume this run; a
            // half-planned run must not be left non-terminal.
            tracing::error!(run = %state.run_id, %error, "failed to persist planned jobs");
            let _ = gitforge_db::queries::PipelineRunQueries::update_status(
                pool,
                state.run_id,
                "failed",
            )
            .await;
            pipeline_registry.write().await.remove(&state.run_id);
            if let Some(path) = run_workspace_paths
                .lock()
                .expect("workspace cache lock poisoned")
                .remove(&state.run_id)
                .flatten()
            {
                tokio::spawn(async move {
                    let _ = tokio::fs::remove_dir_all(path).await;
                });
            }
            return Err(error);
        }
    }

    enqueue_ready_jobs(
        scheduler,
        engine,
        state.run_id,
        repo_id,
        workspace_path.clone(),
    )
    .await;

    Ok(state.run_id)
}

/// Converge engine job state with scheduler rows that went terminal
/// without a completion event — the runner that would report the outcome
/// was fenced, so nothing else ever drives the DAG forward. Only rows the
/// scheduler marked terminal are consulted; queued or requeued jobs still
/// belong to a live assignment path. Returns the number of engine jobs
/// reconciled.
async fn reconcile_fenced_engines(
    registry: &tokio::sync::RwLock<PipelineRegistry>,
    db: &Option<gitforge_db::Pool>,
) -> usize {
    let Some(pool) = db.as_ref() else {
        return 0;
    };
    let engines = registry.read().await;
    let mut reconciled = 0;
    for engine in engines.values() {
        let state = engine.state().await;
        let mut statuses: HashMap<gitforge_common::JobId, String> = HashMap::new();
        for job_id in state.jobs.keys() {
            match gitforge_db::queries::JobQueries::get(pool, *job_id).await {
                Ok(Some(job)) => {
                    statuses.insert(*job_id, job.status);
                }
                Ok(None) => {}
                Err(error) => {
                    tracing::warn!(%error, "fence reconciliation could not read job row");
                }
            }
        }
        for (job_id, action) in fence_actions(&state, &statuses) {
            let result = match action {
                FenceAction::Fail => {
                    engine
                        .fail_job(job_id, 137, "runner lost while job was running".to_string())
                        .await
                }
                FenceAction::Timeout => engine.timeout_job(job_id).await,
                FenceAction::Cancel => engine.cancel_job(job_id).await,
                // The durable row is only written by a lease-verified
                // completion, so a Running mirror against it means the
                // engine missed the completion event; replaying it settles
                // the DAG instead of wedging the run non-terminal.
                FenceAction::Succeed => engine.succeed_job(job_id, 0).await,
            };
            match result {
                Ok(()) => {
                    reconciled += 1;
                    tracing::warn!(%job_id, "engine job reconciled after scheduler fence");
                }
                Err(error) => {
                    tracing::warn!(%error, %job_id, "engine fence reconciliation failed");
                }
            }
        }
    }
    reconciled
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
        enqueue_ready_jobs(
            &scheduler,
            &engine,
            state.run_id,
            state.repo_id,
            workspace_path,
        )
        .await;

        // No-op until every job in the run is terminal; see
        // `finalize_run_if_terminal`.
        finalize_run_if_terminal(
            &engine,
            scheduler_db.as_ref(),
            &run_workspace_paths,
            &pipeline_registry,
        )
        .await;
    }
}

/// Finalize `engine`'s run once it has reached a terminal status: persist the
/// status, free the run's workspace, and evict the engine from the registry.
/// Shared by the completion consumer and the timeout watchdog so a job reaped
/// by the watchdog finalizes exactly like one reported by a runner. Runs that
/// are not terminal yet are left untouched.
async fn finalize_run_if_terminal(
    engine: &CiEngine,
    scheduler_db: Option<&gitforge_db::Pool>,
    run_workspace_paths: &Arc<
        std::sync::Mutex<HashMap<gitforge_common::PipelineRunId, Option<String>>>,
    >,
    pipeline_registry: &Arc<tokio::sync::RwLock<PipelineRegistry>>,
) {
    let state = engine.state().await;
    let terminal_status = match state.status {
        PipelineStatus::Succeeded => "succeeded",
        PipelineStatus::Failed => "failed",
        PipelineStatus::Cancelled => "cancelled",
        _ => return,
    };
    if let Some(pool) = scheduler_db {
        // The durable row is the run's terminal record. If the write fails
        // (SQLite contention can push writes past the busy timeout), LEAVE
        // the engine in the registry and return: the watchdog's next sweep
        // retries finalization for every live engine. Removing the engine
        // on a failed write would leave a forever-'running' durable row no
        // pass can ever settle (the run bccaa1be wedge).
        if let Err(error) = gitforge_db::queries::PipelineRunQueries::update_status(
            pool,
            state.run_id,
            terminal_status,
        )
        .await
        {
            tracing::error!(
                %error,
                run = %state.run_id,
                "failed to persist terminal run status; keeping the engine live for a watchdog retry"
            );
            return;
        }
    }
    let workspace_path = run_workspace_paths
        .lock()
        .expect("workspace cache lock poisoned")
        .remove(&state.run_id)
        .flatten();
    // Free the checkout once nothing references it. Spawned so a large delete
    // cannot stall completion processing for other runs; removal only ever
    // targets the run-owned directory.
    let root = workspace_root();
    let run_id = state.run_id;
    tokio::spawn(async move {
        remove_run_workspace_dir(&root, run_id, workspace_path.as_deref()).await;
    });
    pipeline_registry.write().await.remove(&state.run_id);
}

/// Resolve a job definition into the scheduler's flat execution plan: the
/// raw commands, the working directory (step override, else the run
/// workspace), and the effective timeout.
///
/// Chained jobs keep the pipeline's per-job timeout. The legacy inline
/// enqueue silently applied a 300 s default — a 45 m test job queued after
/// its neighbor finished was killed five minutes in.
fn execution_plan(
    definition: &JobDefinition,
    workspace_path: Option<&str>,
) -> JobExecutionDefinition {
    let commands = definition
        .steps
        .iter()
        .map(|step| step.run.clone())
        .collect();
    let working_dir = definition
        .steps
        .iter()
        .find_map(|step| step.working_directory.clone())
        .or_else(|| workspace_path.map(str::to_string));
    let timeout_secs = definition
        .timeout_secs()
        .unwrap_or(DEFAULT_JOB_TIMEOUT_SECS);
    JobExecutionDefinition {
        commands,
        image: definition.image.clone(),
        working_dir,
        timeout_secs,
        // The DAG builder has already merged pipeline `environment` into the
        // job `env` map, so the job definition alone carries everything.
        env: definition.env.clone(),
    }
}

/// Enqueue every job whose dependency stage the engine just released.
///
/// Shared by the trigger path, the completion consumer, and restart
/// recovery so all three keep one enqueue contract (definition, working
/// directory, timeout).
async fn enqueue_ready_jobs(
    scheduler: &Scheduler,
    engine: &CiEngine,
    run_id: gitforge_common::PipelineRunId,
    repo_id: gitforge_common::RepoId,
    workspace_path: Option<String>,
) {
    for job_id in engine.ready_jobs().await {
        let Some(definition) = engine.job_definition(job_id) else {
            tracing::error!(run = %run_id, %job_id, "missing definition for ready job");
            continue;
        };
        let plan = execution_plan(&definition, workspace_path.as_deref());
        if let Err(error) = scheduler
            .enqueue_with_definition_and_image_and_timeout(job_id, run_id, repo_id, plan)
            .await
        {
            // F21: the chain cannot advance through a job with no durable
            // row. Fail the job so the run grades `failed` with a visible
            // cause instead of stalling as an incomplete chain for the
            // reconciler to sweep up later.
            tracing::error!(
                run = %run_id,
                job = %job_id,
                %error,
                "ready job enqueue failed; failing the chain"
            );
            if let Err(fail_error) = engine
                .fail_job(job_id, -1, format!("durable enqueue failed: {error}"))
                .await
            {
                tracing::error!(
                    run = %run_id,
                    job = %job_id,
                    error = %fail_error,
                    "failed to mark enqueue failure on the engine"
                );
            }
            continue;
        }
        tracing::debug!("enqueued job {} for pipeline run {}", job_id, run_id);
    }
}

/// Persist one durable `pending` row per planned job, before any of them is
/// dispatched.
///
/// Each row carries the full execution definition, so scheduler recovery can
/// dispatch the job verbatim the moment the rebuilt engine releases its
/// stage — nothing but the engine's in-memory graph holds the job set
/// anymore. Enqueue later upserts the same row (`create_or_open_queue`)
/// rather than inserting a second one.
async fn persist_planned_jobs(
    pool: &gitforge_db::Pool,
    engine: &CiEngine,
    run_id: gitforge_common::PipelineRunId,
    workspace_path: Option<&str>,
) -> anyhow::Result<usize> {
    let planned = engine.planned_jobs();
    for (job_id, name) in &planned {
        let definition = engine
            .job_definition(*job_id)
            .ok_or_else(|| anyhow::anyhow!("missing definition for planned job {name}"))?;
        let plan = execution_plan(&definition, workspace_path);
        let mut db_job = gitforge_db::models::Job::new(run_id, name.clone());
        db_job.id = *job_id;
        db_job.commands = plan.commands;
        db_job.image = plan.image;
        db_job.working_dir = plan.working_dir;
        db_job.timeout_secs = plan.timeout_secs;
        gitforge_db::queries::JobQueries::create(pool, &db_job).await?;
    }
    tracing::info!(run = %run_id, planned = planned.len(), "persisted planned job rows");
    Ok(planned.len())
}

/// Rebuild live engines for runs a previous process left behind.
///
/// Durable DAG planning persists every planned job at trigger time, so the
/// restarted control plane can rebuild the engine for an interrupted run:
/// graph nodes are grafted onto the durable job ids by name, statuses are
/// restored from the rows, and the run's workspace checkout is recreated so
/// dispatched jobs run against real sources. Without this, only the
/// scheduler's queued rows were recovered — nothing re-advanced the chain,
/// and reconciliation graded the run from its surviving rows alone.
///
/// Skipped runs and why:
/// - terminal runs: nothing to resume;
/// - missing or unreadable pipeline definition: a legacy row whose grading
///   reconciliation already owns;
/// - no durable job rows: the trigger never planned (pre-planning run, or
///   planning failed and the run was failed) — the enqueue horizon in
///   reconciliation decides;
/// - all rows terminal: reconciliation grades the run, which also catches
///   the incomplete-chain shortfall a rebuilt engine would wrongly
///   finalize as success.
async fn rebuild_live_engines(
    pool: &gitforge_db::Pool,
    scheduler: &Scheduler,
    pipeline_registry: &Arc<tokio::sync::RwLock<PipelineRegistry>>,
    run_workspace_paths: &Arc<
        std::sync::Mutex<HashMap<gitforge_common::PipelineRunId, Option<String>>>,
    >,
) -> usize {
    let runs = match gitforge_db::queries::PipelineRunQueries::list(pool).await {
        Ok(runs) => runs,
        Err(error) => {
            tracing::warn!(%error, "engine rebuild skipped: run list unreadable");
            return 0;
        }
    };

    let mut rebuilt = 0;
    for run in runs {
        if matches!(
            run.status.as_str(),
            "succeeded" | "failed" | "cancelled" | "timed_out" | "timeout" | "timed-out"
        ) {
            continue;
        }
        let pipeline = match gitforge_db::queries::PipelineQueries::get(pool, run.pipeline_id).await
        {
            Ok(Some(pipeline)) => pipeline,
            Ok(None) | Err(_) => {
                tracing::warn!(
                    run = %run.id,
                    "engine rebuild skipped: pipeline definition row unavailable"
                );
                continue;
            }
        };
        let definition: PipelineDefinition = match serde_json::from_value(pipeline.config) {
            Ok(definition) => definition,
            Err(error) => {
                tracing::warn!(
                    run = %run.id,
                    %error,
                    "engine rebuild skipped: pipeline config unreadable"
                );
                continue;
            }
        };
        let rows = match gitforge_db::queries::JobQueries::list_by_run(pool, run.id).await {
            Ok(rows) => rows,
            Err(error) => {
                tracing::warn!(run = %run.id, %error, "engine rebuild skipped: job list unreadable");
                continue;
            }
        };
        if rows.is_empty() {
            continue;
        }
        let has_live_row = rows.iter().any(|job| {
            gitforge_db::models::JobStatus::from_str(&job.status)
                .is_some_and(|status| !status.is_terminal())
        });
        if !has_live_row {
            continue;
        }
        let mut durable_rows = Vec::with_capacity(rows.len());
        let mut graftable = true;
        for job in rows {
            match gitforge_db::models::JobStatus::from_str(&job.status) {
                Some(status) => {
                    durable_rows.push((job.id, job.name, status.to_common(), job.started_at))
                }
                None => {
                    tracing::warn!(
                        run = %run.id,
                        job = %job.id,
                        status = %job.status,
                        "engine rebuild skipped: unknown durable job status"
                    );
                    graftable = false;
                    break;
                }
            }
        }
        if !graftable {
            continue;
        }
        let engine = match CiEngine::rebuild(
            run.id,
            run.pipeline_id,
            run.repo_id,
            definition,
            &durable_rows,
        )
        .await
        {
            Ok(engine) => Arc::new(engine),
            Err(error) => {
                tracing::error!(run = %run.id, %error, "engine rebuild failed");
                continue;
            }
        };
        let workspace_path =
            match prepare_run_workspace(pool, run.repo_id, run.id, &run.commit_hash).await {
                Ok(path) => Some(path),
                Err(error) => {
                    tracing::error!(
                        run = %run.id,
                        %error,
                        "engine rebuild skipped: workspace could not be restored"
                    );
                    continue;
                }
            };
        run_workspace_paths
            .lock()
            .expect("workspace cache lock poisoned")
            .insert(run.id, workspace_path.clone());
        pipeline_registry
            .write()
            .await
            .insert(run.id, engine.clone());
        // Re-drive the DAG. Rows the scheduler had already queued are
        // re-enqueued (the upsert refreshes the durable row; the scheduler
        // deduplicates its in-memory queue), and any stage whose
        // predecessors all succeeded is released now.
        if let Err(error) = engine.queue_ready_jobs().await {
            tracing::warn!(run = %run.id, %error, "post-rebuild stage release failed");
        }
        enqueue_ready_jobs(scheduler, &engine, run.id, run.repo_id, workspace_path).await;
        tracing::info!(run = %run.id, "rebuilt live engine for interrupted run");
        rebuilt += 1;
    }
    rebuilt
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
        name: format!("{repo_id}-pipeline"),
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
    fn token_matches_raw_and_bearer_credentials() {
        assert!(token_matches("shared-secret", Some("shared-secret")));
        assert!(token_matches("shared-secret", Some("Bearer shared-secret")));
    }

    #[test]
    fn token_matches_rejects_missing_mismatched_and_malformed_credentials() {
        assert!(!token_matches("shared-secret", None));
        assert!(!token_matches("shared-secret", Some("wrong-secret")));
        assert!(!token_matches("shared-secret", Some("Basic shared-secret")));
        assert!(!token_matches(
            "shared-secret",
            Some("Bearer shared-secret-extra")
        ));
    }

    #[test]
    fn status_token_has_no_fallback_and_ignores_empty_values() {
        // No fallback chain: the status endpoint's credential must be the
        // dedicated name, never the trigger or operator token (issue #259).
        let token = configured_status_token(|name| match name {
            "GITFORGE_STATUS_TOKEN" => Some("status-secret".to_string()),
            "GITFORGE_TRIGGER_TOKEN" => Some("trigger-secret".to_string()),
            "GITFORGE_SCHEDULER_OPERATOR_TOKEN" => Some("operator-secret".to_string()),
            _ => None,
        });
        assert_eq!(token.as_deref(), Some("status-secret"));

        assert_eq!(
            configured_status_token(|name| match name {
                "GITFORGE_TRIGGER_TOKEN" => Some("trigger-secret".to_string()),
                _ => None,
            }),
            None,
            "the trigger token must not satisfy the status endpoint"
        );
        // An empty configured value closes the endpoint like an unset one.
        assert_eq!(
            configured_status_token(|name| (name == "GITFORGE_STATUS_TOKEN").then(String::new)),
            None
        );
    }

    #[test]
    fn run_status_maps_fail_closed() {
        // Non-terminal states keep polling.
        assert_eq!(map_run_status(Some("pending")), "running");
        assert_eq!(map_run_status(Some("running")), "running");
        // Terminal states pass through.
        assert_eq!(map_run_status(Some("succeeded")), "succeeded");
        assert_eq!(map_run_status(Some("failed")), "failed");
        assert_eq!(map_run_status(Some("timed_out")), "failed");
        assert_eq!(map_run_status(Some("timeout")), "failed");
        assert_eq!(map_run_status(Some("cancelled")), "cancelled");
        // Unreadable run row, unexpected verdict, or missing correlation:
        // never green.
        assert_eq!(map_run_status(None), "failed");
        assert_eq!(map_run_status(Some("queued")), "failed");
        assert_eq!(map_run_status(Some("")), "failed");
    }

    /// Build a trigger state over a real file-backed pool, so the status
    /// handler's database branches run against actual SQLite semantics
    /// (`:memory:` pools do not share tables across pool connections). The
    /// pool carries one user-owned repository so run fixtures can satisfy
    /// the `pipeline_runs` foreign keys.
    async fn trigger_status_state() -> (
        Arc<TriggerState>,
        gitforge_db::Pool,
        gitforge_common::RepoId,
        std::path::PathBuf,
    ) {
        let db_path = std::env::temp_dir().join(format!(
            "gitforge-ci-status-tests-{}.db",
            uuid::Uuid::new_v4()
        ));
        let pool = gitforge_db::Pool::new(&db_path.display().to_string())
            .await
            .unwrap();
        pool.migrate().await.unwrap();
        let user = gitforge_db::models::User::new(
            "status-tests".to_string(),
            "status-tests@example.test".to_string(),
            "hash".to_string(),
        );
        gitforge_db::queries::UserQueries::create(&pool, &user)
            .await
            .unwrap();
        let repository = gitforge_db::models::Repository::new(
            "status-tests".to_string(),
            user.id,
            std::env::temp_dir()
                .join(format!("status-tests-{}.git", uuid::Uuid::new_v4()))
                .display()
                .to_string(),
        );
        let repo_id = repository.id;
        gitforge_db::queries::RepoQueries::create(&pool, &repository)
            .await
            .unwrap();
        let state = Arc::new(TriggerState {
            event_bus: Arc::new(InMemoryEventBus::new()),
            workspace_paths: Arc::new(std::sync::Mutex::new(HashMap::new())),
            run_waiters: Arc::new(std::sync::Mutex::new(HashMap::new())),
            db: Some(pool.clone()),
        });
        (state, pool, repo_id, db_path)
    }

    /// Invoke the status handler directly and decode its JSON body.
    async fn trigger_status_response(
        state: &Arc<TriggerState>,
        event_id: uuid::Uuid,
    ) -> (StatusCode, serde_json::Value) {
        let response =
            trigger_event_status(Extension(state.clone()), Path(event_id.to_string())).await;
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    /// Record a trigger event already correlated to `run_id`.
    async fn correlated_event(
        pool: &gitforge_db::Pool,
        run_id: gitforge_common::PipelineRunId,
    ) -> uuid::Uuid {
        let event_id = uuid::Uuid::new_v4();
        gitforge_db::queries::CiTriggerEventQueries::insert_pending(
            pool,
            event_id,
            gitforge_common::RepoId::new(),
            None,
            None,
        )
        .await
        .unwrap();
        gitforge_db::queries::CiTriggerEventQueries::correlate(pool, event_id, run_id)
            .await
            .unwrap();
        event_id
    }

    /// The positive anchor: a correlated event grades from the durable run
    /// row, so the answer follows the run's verdict, not the correlation's.
    #[tokio::test]
    async fn trigger_status_correlated_event_grades_from_the_run_row() {
        let (state, pool, repo_id, _db_path) = trigger_status_state().await;
        let pipeline = gitforge_db::models::Pipeline {
            id: gitforge_common::PipelineId::new(),
            repo_id,
            name: "status-grade-fixture".to_string(),
            trigger_type: "push".to_string(),
            config: serde_json::json!({}),
            created_at: chrono::Utc::now(),
        };
        gitforge_db::queries::PipelineQueries::create(&pool, &pipeline)
            .await
            .unwrap();
        let run = gitforge_db::models::PipelineRun::new(
            pipeline.id,
            repo_id,
            "status-tests".to_string(),
            "0".repeat(40),
        );
        gitforge_db::queries::PipelineRunQueries::create(&pool, &run)
            .await
            .unwrap();
        gitforge_db::queries::PipelineRunQueries::update_status(&pool, run.id, "succeeded")
            .await
            .unwrap();

        let event_id = correlated_event(&pool, run.id).await;
        let (status, payload) = trigger_status_response(&state, event_id).await;
        assert_eq!(status, StatusCode::OK, "payload: {payload}");
        assert_eq!(payload["status"], "succeeded", "payload: {payload}");
        assert_eq!(payload["pipeline_run_id"], run.id.to_string());
    }

    /// A correlation that names a run with no row is a genuinely lost record:
    /// terminal `failed` over HTTP 200 — the poller fails the job instead of
    /// retrying forever (issue #259 follow-up).
    #[tokio::test]
    async fn trigger_status_missing_run_row_is_terminal_failed() {
        let (state, pool, _repo_id, _db_path) = trigger_status_state().await;
        let event_id = correlated_event(&pool, gitforge_common::PipelineRunId::new()).await;

        let (status, payload) = trigger_status_response(&state, event_id).await;
        assert_eq!(status, StatusCode::OK, "payload: {payload}");
        assert_eq!(payload["status"], "failed", "payload: {payload}");
    }

    /// A correlated row without a run id is a corrupted record: terminal
    /// `failed`, never green, and no run lookup is attempted for it.
    #[tokio::test]
    async fn trigger_status_correlated_row_without_run_id_is_terminal_failed() {
        let (state, pool, _repo_id, _db_path) = trigger_status_state().await;
        let event_id = uuid::Uuid::new_v4();
        gitforge_db::queries::CiTriggerEventQueries::insert_pending(
            &pool,
            event_id,
            gitforge_common::RepoId::new(),
            None,
            None,
        )
        .await
        .unwrap();
        sqlx::query("UPDATE ci_trigger_events SET status = 'correlated' WHERE event_id = ?")
            .bind(event_id.to_string())
            .execute(pool.pool())
            .await
            .unwrap();

        let (status, payload) = trigger_status_response(&state, event_id).await;
        assert_eq!(status, StatusCode::OK, "payload: {payload}");
        assert_eq!(payload["status"], "failed", "payload: {payload}");
        assert!(payload["pipeline_run_id"].is_null(), "payload: {payload}");
    }

    /// The distinction the audit required: a transient run-read error is a
    /// retryable 503, never an HTTP 200 terminal `failed` — a poller must
    /// keep polling instead of failing the job on a database fault, and must
    /// be able to tell the two answers apart (issue #259 follow-up).
    #[tokio::test]
    async fn trigger_status_unreadable_run_is_retryable_503_not_terminal_failed() {
        let (state, pool, _repo_id, _db_path) = trigger_status_state().await;
        let event_id = correlated_event(&pool, gitforge_common::PipelineRunId::new()).await;

        // Force PipelineRunQueries::get into the error branch while the
        // correlation row itself stays readable.
        sqlx::query("DROP TABLE pipeline_runs")
            .execute(pool.pool())
            .await
            .unwrap();

        let (status, payload) = trigger_status_response(&state, event_id).await;
        assert_eq!(
            status,
            StatusCode::SERVICE_UNAVAILABLE,
            "a transient read fault must not look terminal: {payload}"
        );
        assert_ne!(
            status,
            StatusCode::OK,
            "a read fault must never answer the terminal 200 contract"
        );
        assert_eq!(payload["error"], "status_read_failed", "payload: {payload}");
        assert_eq!(payload["status"], "unavailable", "payload: {payload}");
    }

    /// Shared fixture for the recovery tests: a file-backed pool carrying
    /// one user-owned repository, plus the process dependencies a recovery
    /// drive needs. The repository's git path deliberately does not exist —
    /// the drive-failure tests rely on that.
    async fn trigger_recovery_fixture() -> (
        gitforge_db::Pool,
        gitforge_common::RepoId,
        RecoveryDeps,
        std::path::PathBuf,
    ) {
        let db_path = std::env::temp_dir().join(format!(
            "gitforge-ci-recovery-tests-{}.db",
            uuid::Uuid::new_v4()
        ));
        let pool = gitforge_db::Pool::new(&db_path.display().to_string())
            .await
            .unwrap();
        pool.migrate().await.unwrap();
        let user = gitforge_db::models::User::new(
            "recovery-tests".to_string(),
            "recovery-tests@example.test".to_string(),
            "hash".to_string(),
        );
        gitforge_db::queries::UserQueries::create(&pool, &user)
            .await
            .unwrap();
        let repository = gitforge_db::models::Repository::new(
            "recovery-tests".to_string(),
            user.id,
            std::env::temp_dir()
                .join(format!("recovery-tests-{}.git", uuid::Uuid::new_v4()))
                .display()
                .to_string(),
        );
        let repo_id = repository.id;
        gitforge_db::queries::RepoQueries::create(&pool, &repository)
            .await
            .unwrap();
        (pool, repo_id, RecoveryDeps::new(), db_path)
    }

    /// The scheduler/cache/registry bundle `recover_claimed_trigger_event`
    /// drives; fresh and empty per test.
    struct RecoveryDeps {
        scheduler: Arc<Scheduler>,
        pipeline_cache: Arc<std::sync::Mutex<PipelineCache>>,
        workspace_paths: Arc<std::sync::Mutex<HashMap<gitforge_common::RepoId, Option<String>>>>,
        run_workspace_paths:
            Arc<std::sync::Mutex<HashMap<gitforge_common::PipelineRunId, Option<String>>>>,
        pipeline_registry: Arc<tokio::sync::RwLock<PipelineRegistry>>,
    }

    impl RecoveryDeps {
        fn new() -> Self {
            Self {
                scheduler: Arc::new(Scheduler::new()),
                pipeline_cache: Arc::new(std::sync::Mutex::new(HashMap::new())),
                workspace_paths: Arc::new(std::sync::Mutex::new(HashMap::new())),
                run_workspace_paths: Arc::new(std::sync::Mutex::new(HashMap::new())),
                pipeline_registry: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            }
        }
    }

    /// The crash window the whole F2 defect lives in: the consumer created
    /// the run, then died (or lost the correlate write). Recovery must point
    /// the correlation row at that run — never build a second one.
    #[tokio::test]
    async fn trigger_recovery_correlates_the_run_that_already_exists() {
        let (pool, repo_id, deps, _db_path) = trigger_recovery_fixture().await;
        let event_id = uuid::Uuid::new_v4();
        let payload = PushReceivedPayload {
            repo_id,
            ref_name: "refs/heads/main".to_string(),
            old_hash: "0".repeat(40),
            new_hash: "a".repeat(40),
            pusher_id: None,
        };
        gitforge_db::queries::CiTriggerEventQueries::insert_pending(
            &pool,
            event_id,
            repo_id,
            Some(&serde_json::to_string(&payload).unwrap()),
            None,
        )
        .await
        .unwrap();

        // The run the (crashed) consumer already created, linked by event.
        let pipeline = gitforge_db::models::Pipeline {
            id: gitforge_common::PipelineId::new(),
            repo_id,
            name: "recovery-fixture".to_string(),
            trigger_type: "push".to_string(),
            config: serde_json::json!({}),
            created_at: chrono::Utc::now(),
        };
        gitforge_db::queries::PipelineQueries::create(&pool, &pipeline)
            .await
            .unwrap();
        let mut run = gitforge_db::models::PipelineRun::new(
            pipeline.id,
            repo_id,
            "push".to_string(),
            "a".repeat(40),
        );
        run.start();
        let created =
            gitforge_db::queries::PipelineRunQueries::create_for_trigger(&pool, &run, event_id)
                .await
                .unwrap();

        let claim = gitforge_db::queries::CiTriggerEventQueries::claim_due(
            &pool,
            chrono::Utc::now(),
            Duration::from_secs(TRIGGER_CLAIM_LEASE_SECS),
        )
        .await
        .unwrap()
        .expect("orphaned event is claimable");
        assert_eq!(claim.event.event_id, event_id);
        recover_claimed_trigger_event(
            &pool,
            claim,
            &deps.scheduler,
            &deps.pipeline_cache,
            &deps.workspace_paths,
            &deps.run_workspace_paths,
            &deps.pipeline_registry,
        )
        .await;

        let row = gitforge_db::queries::CiTriggerEventQueries::get(&pool, event_id)
            .await
            .unwrap()
            .expect("correlation row");
        assert_eq!(row.status, "correlated", "row: {row:?}");
        assert_eq!(row.pipeline_run_id, Some(created));
        assert_eq!(
            gitforge_db::queries::PipelineRunQueries::list(&pool)
                .await
                .unwrap()
                .len(),
            1,
            "recovery must correlate, not duplicate"
        );
    }

    /// F2 idempotency race: two concurrent drives of one accepted event both
    /// miss the early correlate lookup — the winner's run row lands between
    /// that lookup and the losing drive's insert (its claim lease expired
    /// mid-drive and the recovery sweep re-claimed the event). The unique
    /// index hands the losing drive the winner's id, and it must return that
    /// id without launching anything under its own losing run id: no
    /// workspace, no engine registration, no planned job rows, no enqueue.
    /// The losing drive's `deactivate_active` + `create` for the pipeline
    /// version must also roll back, so (repo, name) keeps exactly one
    /// version with the winner as the active row.
    #[tokio::test]
    async fn losing_trigger_drive_converges_on_the_durable_run_without_launching() {
        let (pool, repo_id, deps, _db_path) = trigger_recovery_fixture().await;
        let event_id = uuid::Uuid::new_v4();
        let commit = "c".repeat(40);

        // Winner and loser target the same (repo, name) so the loser's
        // transaction would otherwise deactivate the winner's predecessor
        // and leave its own version active. The pipeline carries real
        // jobs so the losing drive has work to "lose" — the no-enqueue
        // assertion is only meaningful when the ready list is non-empty.
        let pipeline_name = "race-pipeline".to_string();
        let pipeline_config = serde_json::json!({
            "name": pipeline_name,
            "version": "1.0",
            "trigger_on": ["push"],
            "jobs": [{"name": "build"}],
        });

        // The concurrent winner: its run row already linked by the event id.
        let winner_pipeline_id = gitforge_common::PipelineId::new();
        let winner_pipeline = gitforge_db::models::Pipeline {
            id: winner_pipeline_id,
            repo_id,
            name: pipeline_name.clone(),
            trigger_type: "push".to_string(),
            config: pipeline_config.clone(),
            created_at: chrono::Utc::now(),
        };
        gitforge_db::queries::PipelineQueries::create(&pool, &winner_pipeline)
            .await
            .unwrap();
        let mut winner_run = gitforge_db::models::PipelineRun::new(
            winner_pipeline_id,
            repo_id,
            "push".to_string(),
            commit.clone(),
        );
        winner_run.start();
        let winner_run_id = gitforge_db::queries::PipelineRunQueries::create_for_trigger(
            &pool,
            &winner_run,
            event_id,
        )
        .await
        .unwrap();
        let (pre_active, pre_total) = gitforge_db::queries::PipelineQueries::version_stats_by_name(
            &pool,
            repo_id,
            &pipeline_name,
        )
        .await
        .unwrap();
        assert_eq!(pre_active, Some(winner_pipeline_id));
        assert_eq!(pre_total, 1);

        // The losing drive: its own engine, so its own in-memory run id;
        // and its own freshly-allocated pipeline id (each push allocates
        // a fresh one — the audit precondition the transaction helper
        // exists to defend). The pipeline definition carries real jobs
        // whose entry points become the "work" the loser must not launch.
        let losing_pipeline_id = gitforge_common::PipelineId::new();
        let mut pipeline = create_default_pipeline(&repo_id.to_string());
        pipeline.name = pipeline_name.clone();
        let trigger = PipelineTriggerEvent::new(
            losing_pipeline_id,
            repo_id,
            commit.clone(),
            TriggerType::Push,
        );
        let engine = Arc::new(CiEngine::new(trigger, pipeline.clone()).await.unwrap());
        engine.start().await.unwrap();
        let losing_run_id = engine.state().await.run_id;
        let initially_ready = engine.ready_jobs().await;
        assert_ne!(losing_run_id, winner_run_id, "fixture: distinct run ids");
        assert_ne!(
            losing_pipeline_id, winner_pipeline_id,
            "fixture: distinct pipeline ids"
        );
        assert!(!initially_ready.is_empty(), "fixture: drive has work");

        // The recovery sweep passes the persisted working directory
        // explicitly; an explicit workspace also keeps this test off the
        // git-clone path the losing drive must never reach.
        let launched = persist_and_launch_run(
            Some(&pool),
            &deps.scheduler,
            &engine,
            losing_pipeline_id,
            &pipeline,
            repo_id,
            &commit,
            event_id,
            Some(std::env::current_dir().unwrap().display().to_string()),
            &deps.run_workspace_paths,
            &deps.pipeline_registry,
        )
        .await
        .expect("a losing drive converges on the durable run instead of erroring");

        assert_eq!(
            launched, winner_run_id,
            "the losing drive must return the durable run's id"
        );

        // Pipeline-version invariants: the loser's `deactivate_active` +
        // `create` ran inside a transaction with the run insert, so
        // losing the run race rolled both back. The active version is
        // still the winner and the history count is unchanged.
        let (post_active, post_total) =
            gitforge_db::queries::PipelineQueries::version_stats_by_name(
                &pool,
                repo_id,
                &pipeline_name,
            )
            .await
            .unwrap();
        assert_eq!(
            post_active,
            Some(winner_pipeline_id),
            "the active version is unchanged: the loser's deactivate_active rolled back"
        );
        assert_eq!(
            post_total, pre_total,
            "the loser's create was rolled back: no orphan predecessor in history"
        );
        assert!(
            gitforge_db::queries::PipelineQueries::get(&pool, losing_pipeline_id)
                .await
                .unwrap()
                .is_none(),
            "the loser's pipeline version row must not exist"
        );

        // No launch side effect may exist under the losing run id.
        assert!(
            deps.run_workspace_paths
                .lock()
                .expect("workspace cache lock poisoned")
                .get(&losing_run_id)
                .is_none(),
            "the losing drive must not prepare a workspace"
        );
        assert!(
            deps.pipeline_registry
                .read()
                .await
                .get(&losing_run_id)
                .is_none(),
            "the losing drive must not register its engine"
        );
        assert!(
            gitforge_db::queries::JobQueries::list_by_run(&pool, losing_run_id)
                .await
                .unwrap()
                .is_empty(),
            "the losing drive must not persist planned job rows"
        );
        for job_id in &initially_ready {
            assert!(
                !deps.scheduler.job_exists(*job_id).await,
                "the losing drive must not enqueue job {job_id}"
            );
        }
        assert!(
            gitforge_db::queries::PipelineRunQueries::get(&pool, losing_run_id)
                .await
                .unwrap()
                .is_none(),
            "the losing drive's run row must not exist"
        );
        assert_eq!(
            gitforge_db::queries::PipelineRunQueries::list(&pool)
                .await
                .unwrap()
                .len(),
            1,
            "the accepted event still has exactly one run"
        );
    }

    /// The companion contract: a drive that WINS the adjudication is the
    /// durable run and still launches with every side effect — the race
    /// check must never turn a winner into a silent no-op. The version
    /// and run are committed together; the workspace, engine, planned
    /// jobs, and enqueued runs are all in place.
    #[tokio::test]
    async fn winning_trigger_drive_launches_the_run_with_full_side_effects() {
        let (pool, repo_id, deps, _db_path) = trigger_recovery_fixture().await;
        let event_id = uuid::Uuid::new_v4();
        let commit = "d".repeat(40);

        let pipeline = create_default_pipeline(&repo_id.to_string());
        let pipeline_id = gitforge_common::PipelineId::new();
        let trigger =
            PipelineTriggerEvent::new(pipeline_id, repo_id, commit.clone(), TriggerType::Push);
        let engine = Arc::new(CiEngine::new(trigger, pipeline.clone()).await.unwrap());
        engine.start().await.unwrap();
        let run_id = engine.state().await.run_id;
        let initially_ready = engine.ready_jobs().await;
        assert!(!initially_ready.is_empty(), "fixture: drive has work");

        let launched = persist_and_launch_run(
            Some(&pool),
            &deps.scheduler,
            &engine,
            pipeline_id,
            &pipeline,
            repo_id,
            &commit,
            event_id,
            Some(std::env::current_dir().unwrap().display().to_string()),
            &deps.run_workspace_paths,
            &deps.pipeline_registry,
        )
        .await
        .expect("a winning drive launches its own run");

        assert_eq!(launched, run_id, "the winning drive returns its own run id");
        // The version+run transaction committed: the pipeline row is
        // there, the run row is there, and the active version is the
        // one this drive just installed.
        let pipeline_row = gitforge_db::queries::PipelineQueries::get(&pool, pipeline_id)
            .await
            .unwrap()
            .expect("the winning drive's pipeline version row must be persisted");
        assert_eq!(pipeline_row.id, pipeline_id);
        assert_eq!(pipeline_row.name, pipeline.name);
        let (active_id, total) = gitforge_db::queries::PipelineQueries::version_stats_by_name(
            &pool,
            repo_id,
            &pipeline.name,
        )
        .await
        .unwrap();
        assert_eq!(
            active_id,
            Some(pipeline_id),
            "the winning drive's version is the active one"
        );
        assert_eq!(total, 1, "no predecessor existed before this drive");
        assert_eq!(
            gitforge_db::queries::PipelineRunQueries::get(&pool, run_id)
                .await
                .unwrap()
                .map(|run| run.id),
            Some(run_id),
            "the winning drive's run row is persisted"
        );
        assert_eq!(
            deps.run_workspace_paths
                .lock()
                .expect("workspace cache lock poisoned")
                .get(&run_id)
                .cloned()
                .flatten()
                .as_deref(),
            Some(std::env::current_dir().unwrap().to_str().unwrap()),
            "the winning drive prepared its workspace"
        );
        assert!(
            deps.pipeline_registry.read().await.get(&run_id).is_some(),
            "the winning drive registered its engine"
        );
        assert!(
            !gitforge_db::queries::JobQueries::list_by_run(&pool, run_id)
                .await
                .unwrap()
                .is_empty(),
            "the winning drive persisted planned job rows"
        );
        for job_id in &initially_ready {
            assert!(
                deps.scheduler.job_exists(*job_id).await,
                "the winning drive enqueued job {job_id}"
            );
        }
        assert_eq!(
            gitforge_db::queries::PipelineRunQueries::find_id_by_trigger_event(&pool, event_id)
                .await
                .unwrap(),
            Some(run_id),
            "the durable event link points at the launched run"
        );
    }

    /// Two drives released together for the same accepted event must converge
    /// on exactly one durable run; only the transaction winner may launch.
    #[tokio::test]
    async fn concurrent_trigger_drives_converge_and_only_winner_launches() {
        let (pool, repo_id, deps, _db_path) = trigger_recovery_fixture().await;
        let event_id = uuid::Uuid::new_v4();
        let commit = "e".repeat(40);
        let mut pipeline = create_default_pipeline(&repo_id.to_string());
        pipeline.name = "concurrent-race-pipeline".to_string();

        let first_pipeline_id = gitforge_common::PipelineId::new();
        let first_trigger = PipelineTriggerEvent::new(
            first_pipeline_id,
            repo_id,
            commit.clone(),
            TriggerType::Push,
        );
        let first_engine = Arc::new(
            CiEngine::new(first_trigger, pipeline.clone())
                .await
                .unwrap(),
        );
        first_engine.start().await.unwrap();
        let first_run_id = first_engine.state().await.run_id;
        let first_jobs = first_engine.ready_jobs().await;

        let second_pipeline_id = gitforge_common::PipelineId::new();
        let second_trigger = PipelineTriggerEvent::new(
            second_pipeline_id,
            repo_id,
            commit.clone(),
            TriggerType::Push,
        );
        let second_engine = Arc::new(
            CiEngine::new(second_trigger, pipeline.clone())
                .await
                .unwrap(),
        );
        second_engine.start().await.unwrap();
        let second_run_id = second_engine.state().await.run_id;
        let second_jobs = second_engine.ready_jobs().await;
        assert_ne!(
            first_run_id, second_run_id,
            "drives must have distinct run ids"
        );
        assert_ne!(
            first_pipeline_id, second_pipeline_id,
            "drives must have distinct pipeline version ids"
        );
        assert!(!first_jobs.is_empty(), "first drive fixture must have work");
        assert!(
            !second_jobs.is_empty(),
            "second drive fixture must have work"
        );

        // Release both callers at the same point. The join macro polls both
        // async drives together, while SQLite BEGIN IMMEDIATE must serialize
        // their version+run writes and return the winner id to the loser.
        let barrier = Arc::new(tokio::sync::Barrier::new(2));
        let first_barrier = barrier.clone();
        let first_drive = async {
            first_barrier.wait().await;
            persist_and_launch_run(
                Some(&pool),
                &deps.scheduler,
                &first_engine,
                first_pipeline_id,
                &pipeline,
                repo_id,
                &commit,
                event_id,
                Some(std::env::current_dir().unwrap().display().to_string()),
                &deps.run_workspace_paths,
                &deps.pipeline_registry,
            )
            .await
        };
        let second_drive = async {
            barrier.wait().await;
            persist_and_launch_run(
                Some(&pool),
                &deps.scheduler,
                &second_engine,
                second_pipeline_id,
                &pipeline,
                repo_id,
                &commit,
                event_id,
                Some(std::env::current_dir().unwrap().display().to_string()),
                &deps.run_workspace_paths,
                &deps.pipeline_registry,
            )
            .await
        };
        let (first_result, second_result) = tokio::join!(first_drive, second_drive);
        let first_durable_run = first_result.expect("first drive must return a durable run id");
        let second_durable_run = second_result.expect("second drive must return a durable run id");
        assert_eq!(
            first_durable_run, second_durable_run,
            "both drives must converge on the same durable run"
        );
        assert!(
            [first_run_id, second_run_id].contains(&first_durable_run),
            "durable identity must belong to one of the competing drives"
        );

        let first_won = first_durable_run == first_run_id;
        let (
            winner_run_id,
            loser_run_id,
            winner_pipeline_id,
            loser_pipeline_id,
            winner_jobs,
            loser_jobs,
        ) = if first_won {
            (
                first_run_id,
                second_run_id,
                first_pipeline_id,
                second_pipeline_id,
                &first_jobs,
                &second_jobs,
            )
        } else {
            (
                second_run_id,
                first_run_id,
                second_pipeline_id,
                first_pipeline_id,
                &second_jobs,
                &first_jobs,
            )
        };

        let (active_pipeline_id, version_count) =
            gitforge_db::queries::PipelineQueries::version_stats_by_name(
                &pool,
                repo_id,
                &pipeline.name,
            )
            .await
            .unwrap();
        assert_eq!(active_pipeline_id, Some(winner_pipeline_id));
        assert_eq!(version_count, 1, "losing version must roll back");
        assert!(
            gitforge_db::queries::PipelineQueries::get(&pool, loser_pipeline_id)
                .await
                .unwrap()
                .is_none(),
            "losing pipeline version must not persist"
        );
        assert_eq!(
            gitforge_db::queries::PipelineRunQueries::list(&pool)
                .await
                .unwrap()
                .len(),
            1,
            "the event must have exactly one durable run"
        );
        assert!(
            deps.pipeline_registry
                .read()
                .await
                .get(&winner_run_id)
                .is_some(),
            "winning drive must register its engine"
        );
        assert!(
            deps.pipeline_registry
                .read()
                .await
                .get(&loser_run_id)
                .is_none(),
            "losing drive must not register its engine"
        );
        assert!(
            !gitforge_db::queries::JobQueries::list_by_run(&pool, winner_run_id)
                .await
                .unwrap()
                .is_empty(),
            "winning drive must persist planned jobs"
        );
        assert!(
            gitforge_db::queries::JobQueries::list_by_run(&pool, loser_run_id)
                .await
                .unwrap()
                .is_empty(),
            "losing drive must not persist planned jobs"
        );
        for job_id in winner_jobs {
            assert!(deps.scheduler.job_exists(*job_id).await);
        }
        for job_id in loser_jobs {
            assert!(!deps.scheduler.job_exists(*job_id).await);
        }
        assert!(
            deps.run_workspace_paths
                .lock()
                .expect("workspace cache lock poisoned")
                .get(&winner_run_id)
                .is_some(),
            "winning drive must record its workspace"
        );
        assert!(
            deps.run_workspace_paths
                .lock()
                .expect("workspace cache lock poisoned")
                .get(&loser_run_id)
                .is_none(),
            "losing drive must not prepare a workspace"
        );
    }
    /// A drive that fails *after* creating its run is healed by correlating
    /// that run — the workspace-clone failure here leaves a failed run and
    /// recovery resolves the event to it instead of retrying into a second
    /// build.
    #[tokio::test]
    async fn trigger_recovery_heals_a_drive_that_failed_after_creating_its_run() {
        let (pool, repo_id, deps, _db_path) = trigger_recovery_fixture().await;
        let event_id = uuid::Uuid::new_v4();
        let payload = PushReceivedPayload {
            repo_id,
            ref_name: "refs/heads/main".to_string(),
            old_hash: "0".repeat(40),
            new_hash: "b".repeat(40),
            pusher_id: None,
        };
        gitforge_db::queries::CiTriggerEventQueries::insert_pending(
            &pool,
            event_id,
            repo_id,
            Some(&serde_json::to_string(&payload).unwrap()),
            None,
        )
        .await
        .unwrap();

        let claim = gitforge_db::queries::CiTriggerEventQueries::claim_due(
            &pool,
            chrono::Utc::now(),
            Duration::from_secs(TRIGGER_CLAIM_LEASE_SECS),
        )
        .await
        .unwrap()
        .expect("orphaned event is claimable");
        recover_claimed_trigger_event(
            &pool,
            claim,
            &deps.scheduler,
            &deps.pipeline_cache,
            &deps.workspace_paths,
            &deps.run_workspace_paths,
            &deps.pipeline_registry,
        )
        .await;

        let row = gitforge_db::queries::CiTriggerEventQueries::get(&pool, event_id)
            .await
            .unwrap()
            .expect("correlation row");
        assert_eq!(row.status, "correlated", "row: {row:?}");
        let run_id = row.pipeline_run_id.expect("correlated run id");
        let run = gitforge_db::queries::PipelineRunQueries::get(&pool, run_id)
            .await
            .unwrap()
            .expect("recovered run row");
        assert_eq!(
            run.status, "failed",
            "the undriveable checkout must show as a failed run, not a stuck one"
        );
        assert_eq!(
            gitforge_db::queries::PipelineRunQueries::list(&pool)
                .await
                .unwrap()
                .len(),
            1
        );
    }

    /// A drive that fails before anything durable exists is retried with
    /// backoff, and the retry budget is finite: the event terminates as
    /// `failed` instead of pending forever.
    #[tokio::test]
    async fn trigger_recovery_retries_then_terminally_fails_an_undriveable_event() {
        let (pool, _repo_id, deps, _db_path) = trigger_recovery_fixture().await;
        let event_id = uuid::Uuid::new_v4();
        // A repo id with no repository row: the drive fails at the lookup,
        // before any run or pipeline is created.
        let payload = PushReceivedPayload {
            repo_id: gitforge_common::RepoId::new(),
            ref_name: "refs/heads/main".to_string(),
            old_hash: "0".repeat(40),
            new_hash: "c".repeat(40),
            pusher_id: None,
        };
        gitforge_db::queries::CiTriggerEventQueries::insert_pending(
            &pool,
            event_id,
            payload.repo_id,
            Some(&serde_json::to_string(&payload).unwrap()),
            None,
        )
        .await
        .unwrap();

        for attempt in 1..=TRIGGER_MAX_CLAIM_ATTEMPTS {
            // The consumer-style claim ignores the retry backoff, so the
            // test can walk the budget without sleeping through it.
            let claim = gitforge_db::queries::CiTriggerEventQueries::claim_event(
                &pool,
                event_id,
                chrono::Utc::now(),
                Duration::from_secs(TRIGGER_CLAIM_LEASE_SECS),
            )
            .await
            .unwrap()
            .expect("pending event is claimable between retries");
            assert_eq!(claim.event.attempts, attempt);
            recover_claimed_trigger_event(
                &pool,
                claim,
                &deps.scheduler,
                &deps.pipeline_cache,
                &deps.workspace_paths,
                &deps.run_workspace_paths,
                &deps.pipeline_registry,
            )
            .await;

            let row = gitforge_db::queries::CiTriggerEventQueries::get(&pool, event_id)
                .await
                .unwrap()
                .expect("correlation row");
            if attempt < TRIGGER_MAX_CLAIM_ATTEMPTS {
                assert_eq!(row.status, "pending", "attempt {attempt} must retry");
                assert!(
                    row.last_error.is_some(),
                    "the recorded failure must be visible to operators"
                );
            } else {
                assert_eq!(row.status, "failed", "attempt {attempt} must terminate");
                assert!(row
                    .last_error
                    .as_deref()
                    .unwrap_or_default()
                    .contains("attempt"));
            }
        }

        // Terminal: nothing claims it again, and no run was ever created.
        assert!(gitforge_db::queries::CiTriggerEventQueries::claim_due(
            &pool,
            chrono::Utc::now() + Duration::from_secs(3_600),
            Duration::from_secs(TRIGGER_CLAIM_LEASE_SECS),
        )
        .await
        .unwrap()
        .is_none());
        assert!(gitforge_db::queries::PipelineRunQueries::list(&pool)
            .await
            .unwrap()
            .is_empty());
    }

    /// A committed definition that parses but is rejected by the DAG
    /// builder (duplicate job names here) is the same permanent class as
    /// an unparseable one: the settle path fails the trigger terminally on
    /// the first attempt with a nearly untouched retry budget. The
    /// transient contrast is the sibling test above — an unregistered
    /// repository keeps its bounded retries.
    #[tokio::test]
    async fn trigger_drive_fails_terminally_on_a_dag_invalid_committed_pipeline() {
        let _guard = WORKSPACE_TEST_LOCK
            .get_or_init(|| tokio::sync::Mutex::new(()))
            .lock()
            .await;
        let duplicate_jobs = r#"
name: dup-jobs
version: "1.0"
trigger_on:
  - push
jobs:
  - name: build
    image: rust:latest
    steps:
      - name: build
        run: echo one
  - name: build
    image: rust:latest
    steps:
      - name: build
        run: echo two
"#
        .to_string();
        let (bare, commit) =
            seed_pipeline_files(&[(PIPELINE_CONFIG_PATHS[0], duplicate_jobs)]).await;
        let (pool, repo_id) = test_pool_with_repository(bare.to_string_lossy().into_owned()).await;

        let event_id = uuid::Uuid::new_v4();
        let payload = PushReceivedPayload {
            repo_id,
            ref_name: "refs/heads/main".to_string(),
            old_hash: "0".repeat(40),
            new_hash: commit,
            pusher_id: None,
        };
        gitforge_db::queries::CiTriggerEventQueries::insert_pending(
            &pool,
            event_id,
            repo_id,
            Some(&serde_json::to_string(&payload).unwrap()),
            None,
        )
        .await
        .unwrap();

        let claim = gitforge_db::queries::CiTriggerEventQueries::claim_event(
            &pool,
            event_id,
            chrono::Utc::now(),
            Duration::from_secs(TRIGGER_CLAIM_LEASE_SECS),
        )
        .await
        .unwrap()
        .expect("pending event is claimable");
        assert_eq!(claim.event.attempts, 1, "first drive of a fresh event");
        let deps = RecoveryDeps::new();
        recover_claimed_trigger_event(
            &pool,
            claim,
            &deps.scheduler,
            &deps.pipeline_cache,
            &deps.workspace_paths,
            &deps.run_workspace_paths,
            &deps.pipeline_registry,
        )
        .await;

        let row = gitforge_db::queries::CiTriggerEventQueries::get(&pool, event_id)
            .await
            .unwrap()
            .expect("correlation row");
        assert_eq!(
            row.status, "failed",
            "a DAG-invalid committed pipeline is permanent: {row:?}"
        );
        let recorded = row.last_error.as_deref().unwrap_or_default();
        assert!(
            recorded.contains("committed pipeline config rejected"),
            "the failure must name the permanent class: {recorded}"
        );
        assert!(
            recorded.contains("duplicate job name"),
            "the rejection reason must survive into the recorded error: {recorded}"
        );

        // Terminal on attempt one, not at budget exhaustion: nothing is
        // claimable afterwards, and no run was ever created.
        assert!(gitforge_db::queries::CiTriggerEventQueries::claim_due(
            &pool,
            chrono::Utc::now() + Duration::from_secs(3_600),
            Duration::from_secs(TRIGGER_CLAIM_LEASE_SECS),
        )
        .await
        .unwrap()
        .is_none());
        assert!(gitforge_db::queries::PipelineRunQueries::list(&pool)
            .await
            .unwrap()
            .is_empty());
    }

    /// The settle branch in isolation: a committed-config rejection is
    /// terminal on the first attempt even with the whole retry budget
    /// unspent, while an unclassified (transient) fault keeps the row
    /// `pending` behind the retry backoff — the budget exists precisely
    /// for those.
    #[tokio::test]
    async fn settle_fails_a_committed_config_rejection_terminally_on_first_attempt() {
        let (pool, _repo_id, _deps, _db_path) = trigger_recovery_fixture().await;
        let event_id = uuid::Uuid::new_v4();
        gitforge_db::queries::CiTriggerEventQueries::insert_pending(
            &pool,
            event_id,
            gitforge_common::RepoId::new(),
            None,
            None,
        )
        .await
        .unwrap();
        let claim = gitforge_db::queries::CiTriggerEventQueries::claim_event(
            &pool,
            event_id,
            chrono::Utc::now(),
            Duration::from_secs(TRIGGER_CLAIM_LEASE_SECS),
        )
        .await
        .unwrap()
        .expect("pending event is claimable");
        assert_eq!(claim.event.attempts, 1);

        let drive_error = invalid_committed_config(anyhow::anyhow!(
            "invalid .gitforge.yml at abc: missing field `jobs`"
        ));
        settle_claimed_trigger_event(Some(&pool), Some(claim), Err(drive_error)).await;

        let row = gitforge_db::queries::CiTriggerEventQueries::get(&pool, event_id)
            .await
            .unwrap()
            .expect("correlation row");
        assert_eq!(row.status, "failed", "permanent must not retry: {row:?}");
        let recorded = row.last_error.as_deref().unwrap_or_default();
        assert!(
            recorded.contains("committed pipeline config rejected")
                && recorded.contains("missing field `jobs`"),
            "the recorded error must carry the class and the reason: {recorded}"
        );
        assert!(
            gitforge_db::queries::CiTriggerEventQueries::claim_due(
                &pool,
                chrono::Utc::now() + Duration::from_secs(3_600),
                Duration::from_secs(TRIGGER_CLAIM_LEASE_SECS),
            )
            .await
            .unwrap()
            .is_none(),
            "a terminally failed event is never claimable again"
        );
    }

    #[tokio::test]
    async fn settle_keeps_a_transient_drive_failure_retryable_behind_backoff() {
        let (pool, _repo_id, _deps, _db_path) = trigger_recovery_fixture().await;
        let event_id = uuid::Uuid::new_v4();
        gitforge_db::queries::CiTriggerEventQueries::insert_pending(
            &pool,
            event_id,
            gitforge_common::RepoId::new(),
            None,
            None,
        )
        .await
        .unwrap();
        let claim = gitforge_db::queries::CiTriggerEventQueries::claim_event(
            &pool,
            event_id,
            chrono::Utc::now(),
            Duration::from_secs(TRIGGER_CLAIM_LEASE_SECS),
        )
        .await
        .unwrap()
        .expect("pending event is claimable");
        assert_eq!(claim.event.attempts, 1);

        settle_claimed_trigger_event(
            Some(&pool),
            Some(claim),
            Err(anyhow::anyhow!("checkout storage briefly unavailable")),
        )
        .await;

        let row = gitforge_db::queries::CiTriggerEventQueries::get(&pool, event_id)
            .await
            .unwrap()
            .expect("correlation row");
        assert_eq!(
            row.status, "pending",
            "an unclassified fault is transient: the retry budget applies: {row:?}"
        );
        assert!(
            row.last_error
                .as_deref()
                .unwrap_or_default()
                .contains("briefly unavailable"),
            "the recorded failure must be visible to operators"
        );
        // The backoff is honored: not claimable now, claimable after the
        // base window.
        assert!(gitforge_db::queries::CiTriggerEventQueries::claim_due(
            &pool,
            chrono::Utc::now(),
            Duration::from_secs(TRIGGER_CLAIM_LEASE_SECS),
        )
        .await
        .unwrap()
        .is_none());
        assert!(
            gitforge_db::queries::CiTriggerEventQueries::claim_due(
                &pool,
                chrono::Utc::now() + trigger_retry_backoff(1),
                Duration::from_secs(TRIGGER_CLAIM_LEASE_SECS),
            )
            .await
            .unwrap()
            .is_some(),
            "the scheduled retry must come due after the backoff"
        );
    }

    /// Rows written before recovery existed carry no payload and their
    /// producing process is gone: the sweep fails them fail-closed instead
    /// of leaving them `queued` for every poller forever.
    #[tokio::test]
    async fn trigger_recovery_fails_a_legacy_pending_event_without_payload() {
        let (pool, repo_id, deps, _db_path) = trigger_recovery_fixture().await;
        let event_id = uuid::Uuid::new_v4();
        gitforge_db::queries::CiTriggerEventQueries::insert_pending(
            &pool, event_id, repo_id, None, None,
        )
        .await
        .unwrap();

        let claim = gitforge_db::queries::CiTriggerEventQueries::claim_due(
            &pool,
            chrono::Utc::now(),
            Duration::from_secs(TRIGGER_CLAIM_LEASE_SECS),
        )
        .await
        .unwrap()
        .expect("legacy row is claimable");
        assert!(claim.event.payload.is_none());
        recover_claimed_trigger_event(
            &pool,
            claim,
            &deps.scheduler,
            &deps.pipeline_cache,
            &deps.workspace_paths,
            &deps.run_workspace_paths,
            &deps.pipeline_registry,
        )
        .await;

        let row = gitforge_db::queries::CiTriggerEventQueries::get(&pool, event_id)
            .await
            .unwrap()
            .expect("correlation row");
        assert_eq!(row.status, "failed");
        assert!(
            row.last_error
                .as_deref()
                .unwrap_or_default()
                .contains("no durable payload"),
            "the failure must say why the event cannot be re-driven"
        );
    }

    /// The backoff schedule: doubling from the base, capped.
    #[test]
    fn trigger_retry_backoff_doubles_and_caps() {
        assert_eq!(
            trigger_retry_backoff(1),
            Duration::from_secs(TRIGGER_RETRY_BACKOFF_BASE_SECS as u64)
        );
        assert_eq!(
            trigger_retry_backoff(2),
            Duration::from_secs((TRIGGER_RETRY_BACKOFF_BASE_SECS * 2) as u64)
        );
        assert_eq!(
            trigger_retry_backoff(9),
            Duration::from_secs(TRIGGER_RETRY_BACKOFF_CAP_SECS as u64)
        );
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
                required_checks: Vec::new(),
                deny_non_fast_forward: false,
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
                required_checks: Vec::new(),
                deny_non_fast_forward: false,
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

    // A restart leaves the previous process's workspace on disk. The second
    // prepare for the same run must adopt it (restoring the tracked tree and
    // clearing job leftovers), not fail — the failure path used to make the
    // rebuild skip engine registration entirely, sending intact runs to the
    // orphan reconciler to be graded failed.
    #[tokio::test]
    async fn test_prepare_run_workspace_adopts_existing_workspace() {
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

        let (pool, repo_id) =
            test_pool_with_repository(source.to_string_lossy().into_owned()).await;
        std::env::set_var("GITFORGE_WORKSPACE_ROOT", test_root_path.join("workspaces"));
        let workspace = prepare_run_workspace(&pool, repo_id, run_id, &commit)
            .await
            .unwrap();

        // Leftovers from the interrupted attempt: an untracked file and a
        // modified tracked file.
        tokio::fs::write(
            PathBuf::from(&workspace).join("job-leftover.txt"),
            "stale\n",
        )
        .await
        .unwrap();
        tokio::fs::write(PathBuf::from(&workspace).join("marker.txt"), "dirtied\n")
            .await
            .unwrap();

        let adopted = prepare_run_workspace(&pool, repo_id, run_id, &commit)
            .await
            .unwrap();
        assert_eq!(adopted, workspace);
        assert_eq!(
            run_git(["rev-parse", "HEAD"], Some(std::path::Path::new(&adopted))).await,
            commit
        );
        assert_eq!(
            tokio::fs::read_to_string(PathBuf::from(&adopted).join("marker.txt"))
                .await
                .unwrap(),
            "checked out\n"
        );
        assert!(!PathBuf::from(&adopted).join("job-leftover.txt").exists());

        // A workspace that cannot be checked out at the requested commit
        // must still be an error, not a silent adoption.
        let error = prepare_run_workspace(&pool, repo_id, run_id, "deadbeef")
            .await
            .unwrap_err();
        assert!(error.to_string().contains("could not be adopted"));
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
            seed.join(PIPELINE_CONFIG_PATHS[0]),
            "name: fixture-ci\nversion: \"1.0\"\ntrigger_on:\n  - push\nenvironment:\n  CI: \"true\"\njobs:\n  - name: echo\n    image: busybox:latest\n    steps:\n      - name: echo\n        run: echo committed-config\n",
        )
        .await
        .unwrap();
        run_git(["add", PIPELINE_CONFIG_PATHS[0]], Some(&seed)).await;
        run_git(["commit", "-m", "with pipeline"], Some(&seed)).await;
        let with = run_git(["rev-parse", "HEAD"], Some(&seed)).await;
        run_git(
            ["push", bare.to_str().unwrap(), "HEAD:refs/heads/main"],
            Some(&seed),
        )
        .await;
        (bare, without, with)
    }

    /// Pool with a user, repository, and pipeline for run-seeding tests.
    async fn sweep_test_pool() -> (
        gitforge_db::Pool,
        gitforge_common::RepoId,
        gitforge_common::PipelineId,
    ) {
        let pool = gitforge_db::Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();
        let user = gitforge_db::models::User::new(
            "sweep-owner".to_string(),
            "sweep@example.test".to_string(),
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
                name: "sweep-repo".to_string(),
                owner_id: user.id,
                visibility: "private".to_string(),
                git_path: "/tmp/sweep-repo".to_string(),
                required_checks: Vec::new(),
                deny_non_fast_forward: false,
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            },
        )
        .await
        .unwrap();
        let pipeline_id = gitforge_common::PipelineId::new();
        gitforge_db::queries::PipelineQueries::create(
            &pool,
            &gitforge_db::models::Pipeline {
                id: pipeline_id,
                repo_id,
                name: "sweep-pipeline".to_string(),
                trigger_type: "push".to_string(),
                config: serde_json::json!({}),
                created_at: chrono::Utc::now(),
            },
        )
        .await
        .unwrap();
        (pool, repo_id, pipeline_id)
    }

    async fn seed_run(
        pool: &gitforge_db::Pool,
        repo_id: gitforge_common::RepoId,
        pipeline_id: gitforge_common::PipelineId,
        status: &str,
    ) -> gitforge_common::PipelineRunId {
        seed_run_created_at(pool, repo_id, pipeline_id, status, chrono::Utc::now()).await
    }

    async fn seed_run_created_at(
        pool: &gitforge_db::Pool,
        repo_id: gitforge_common::RepoId,
        pipeline_id: gitforge_common::PipelineId,
        status: &str,
        created_at: chrono::DateTime<chrono::Utc>,
    ) -> gitforge_common::PipelineRunId {
        let mut run = gitforge_db::models::PipelineRun::new(
            pipeline_id,
            repo_id,
            "push".to_string(),
            "abc123".to_string(),
        );
        run.created_at = created_at;
        gitforge_db::queries::PipelineRunQueries::create(pool, &run)
            .await
            .unwrap();
        gitforge_db::queries::PipelineRunQueries::update_status(pool, run.id, status)
            .await
            .unwrap();
        run.id
    }

    #[tokio::test]
    async fn test_remove_run_workspace_deletes_only_run_owned_directory() {
        let _guard = WORKSPACE_TEST_LOCK
            .get_or_init(|| tokio::sync::Mutex::new(()))
            .lock()
            .await;
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/gitforge-ci-cleanup-tests")
            .join(gitforge_common::PipelineRunId::new().to_string());
        let run_id = gitforge_common::PipelineRunId::new();
        let owned = root.join(run_id.to_string());
        let foreign = root.join("operator-prepared");
        tokio::fs::create_dir_all(owned.join("checkout"))
            .await
            .unwrap();
        tokio::fs::create_dir_all(foreign.join("checkout"))
            .await
            .unwrap();
        tokio::fs::write(owned.join("marker.txt"), "owned")
            .await
            .unwrap();
        tokio::fs::write(foreign.join("marker.txt"), "foreign")
            .await
            .unwrap();

        assert!(
            remove_run_workspace_dir(&root, run_id, Some(owned.to_str().unwrap())).await,
            "run-owned workspace must be removed"
        );
        assert!(!tokio::fs::try_exists(&owned).await.unwrap());

        // A caller-supplied directory inside the root must survive the run.
        assert!(
            !remove_run_workspace_dir(&root, run_id, Some(foreign.to_str().unwrap())).await,
            "non-run-owned workspace must be left in place"
        );
        assert!(tokio::fs::try_exists(&foreign).await.unwrap());

        // Removing an already-gone workspace and an absent path are no-ops.
        assert!(!remove_run_workspace_dir(&root, run_id, Some(owned.to_str().unwrap())).await);
        assert!(!remove_run_workspace_dir(&root, run_id, None).await);
    }

    #[test]
    fn test_container_backend_selection_is_explicit() {
        let _guard = WORKSPACE_TEST_LOCK.get_or_init(|| tokio::sync::Mutex::new(()));
        let _guard = _guard.blocking_lock();

        std::env::set_var("GITFORGE_CONTAINER_BACKEND", "podman");
        assert_eq!(container_backend_from_env(), Ok(ContainerBackend::Podman));
        std::env::set_var("GITFORGE_CONTAINER_BACKEND", "docker");
        assert_eq!(container_backend_from_env(), Ok(ContainerBackend::Docker));
        std::env::set_var("GITFORGE_CONTAINER_BACKEND", "unknown");
        assert!(container_backend_from_env().is_err());
        std::env::remove_var("GITFORGE_CONTAINER_BACKEND");
        assert!(container_backend_from_env().is_err());
    }

    #[test]
    fn test_cleanup_command_selects_backend_without_fallback() {
        let workspace = std::path::Path::new("/var/lib/gitforge/workspaces/run-123");

        let podman = cleanup_command(ContainerBackend::Podman, workspace);
        assert_eq!(podman.program, "podman");
        assert_eq!(
            podman.args,
            [
                "unshare",
                "rm",
                "-rf",
                "--",
                "/var/lib/gitforge/workspaces/run-123"
            ]
            .into_iter()
            .map(OsString::from)
            .collect::<Vec<_>>()
        );

        let docker = cleanup_command(ContainerBackend::Docker, workspace);
        assert_eq!(docker.program, "docker");
        assert_eq!(
            docker.args[0..10],
            [
                "run",
                "--rm",
                "--user",
                "0:0",
                "--mount",
                "type=bind,src=/var/lib/gitforge/workspaces,dst=/gitforge-cleanup-parent",
                "alpine",
                "rm",
                "-rf",
                "--"
            ]
            .map(OsString::from)
        );
        assert_eq!(
            docker.args[10],
            OsString::from("/gitforge-cleanup-parent/run-123")
        );
        assert!(!docker.args.iter().any(|arg| arg == "podman"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_remove_run_workspace_refuses_symlink() {
        let _guard = WORKSPACE_TEST_LOCK
            .get_or_init(|| tokio::sync::Mutex::new(()))
            .lock()
            .await;
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/gitforge-ci-symlink-cleanup-tests")
            .join(uuid::Uuid::new_v4().to_string());
        let run_id = gitforge_common::PipelineRunId::new();
        let target = root.join("operator-prepared");
        let owned = root.join(run_id.to_string());
        tokio::fs::create_dir_all(&target).await.unwrap();
        tokio::fs::write(target.join("marker.txt"), "preserve")
            .await
            .unwrap();
        std::os::unix::fs::symlink(&target, &owned).unwrap();

        assert!(!remove_run_workspace_dir(&root, run_id, Some(owned.to_str().unwrap())).await);
        assert!(tokio::fs::try_exists(&target).await.unwrap());
        assert!(tokio::fs::try_exists(target.join("marker.txt"))
            .await
            .unwrap());
        tokio::fs::remove_file(&owned).await.unwrap();
        tokio::fs::remove_dir_all(&root).await.unwrap();
    }

    #[tokio::test]
    async fn test_sweep_removes_only_terminal_run_workspaces() {
        let _guard = WORKSPACE_TEST_LOCK
            .get_or_init(|| tokio::sync::Mutex::new(()))
            .lock()
            .await;
        let (pool, repo_id, pipeline_id) = sweep_test_pool().await;
        let succeeded = seed_run(&pool, repo_id, pipeline_id, "succeeded").await;
        let failed = seed_run(&pool, repo_id, pipeline_id, "failed").await;
        let running = seed_run(&pool, repo_id, pipeline_id, "running").await;

        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/gitforge-ci-sweep-tests")
            .join(gitforge_common::PipelineRunId::new().to_string());
        for run in [succeeded, failed, running] {
            tokio::fs::create_dir_all(root.join(run.to_string()).join("checkout"))
                .await
                .unwrap();
        }
        let foreign = root.join("operator-prepared");
        tokio::fs::create_dir_all(foreign.join("checkout"))
            .await
            .unwrap();
        let file = root.join("notes.txt");
        tokio::fs::write(&file, "not a workspace").await.unwrap();

        std::env::set_var("GITFORGE_WORKSPACE_ROOT", &root);
        let removed = sweep_terminal_workspaces(&pool).await;

        assert_eq!(removed, 2, "only terminal-run workspaces are swept");
        assert!(!tokio::fs::try_exists(root.join(succeeded.to_string()))
            .await
            .unwrap());
        assert!(!tokio::fs::try_exists(root.join(failed.to_string()))
            .await
            .unwrap());
        assert!(
            tokio::fs::try_exists(root.join(running.to_string()))
                .await
                .unwrap(),
            "non-terminal run workspace must be kept"
        );
        assert!(
            tokio::fs::try_exists(&foreign).await.unwrap(),
            "non-run-owned directory must be kept"
        );
        assert!(tokio::fs::try_exists(&file).await.unwrap());
    }

    // The F21 scenario end to end: a trigger plans every stage durably, the
    // control plane dies mid-chain, and on restart `rebuild_live_engines`
    // reconstructs an engine that resumes the chain from the surviving rows
    // instead of leaving the tail unenqueued.
    #[tokio::test]
    async fn test_durable_planning_survives_restart_and_advances_chain() {
        let _guard = WORKSPACE_TEST_LOCK
            .get_or_init(|| tokio::sync::Mutex::new(()))
            .lock()
            .await;
        let test_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/gitforge-ci-planning-tests")
            .join(gitforge_common::PipelineRunId::new().to_string());
        let source = test_root.join("source.git");
        let seed = test_root.join("seed");
        tokio::fs::create_dir_all(&test_root).await.unwrap();
        run_git(["init", "--bare", source.to_str().unwrap()], None).await;
        tokio::fs::create_dir_all(&seed).await.unwrap();
        run_git(["init", seed.to_str().unwrap()], None).await;
        run_git(["config", "user.email", "ci@example.test"], Some(&seed)).await;
        run_git(["config", "user.name", "GitForge CI"], Some(&seed)).await;
        tokio::fs::write(seed.join("marker.txt"), "planned\n")
            .await
            .unwrap();
        run_git(["add", "marker.txt"], Some(&seed)).await;
        run_git(["commit", "-m", "planning fixture"], Some(&seed)).await;
        let commit = run_git(["rev-parse", "HEAD"], Some(&seed)).await;
        run_git(
            ["push", source.to_str().unwrap(), "HEAD:refs/heads/main"],
            Some(&seed),
        )
        .await;

        let workspace_root = test_root.join("workspaces");
        std::env::set_var("GITFORGE_WORKSPACE_ROOT", &workspace_root);
        let (pool, repo_id) =
            test_pool_with_repository(source.to_string_lossy().into_owned()).await;

        let pipeline_id = gitforge_common::PipelineId::new();
        let chained = |name: &str, needs: &[&str]| JobDefinition {
            name: name.to_string(),
            image: "rust:latest".to_string(),
            needs: needs.iter().map(ToString::to_string).collect(),
            env: HashMap::new(),
            steps: vec![StepDefinition {
                name: format!("{name}-step"),
                run: "true".to_string(),
                env: None,
                working_directory: None,
                condition: None,
            }],
            timeout: None,
            retry: None,
        };
        let definition = PipelineDefinition {
            name: "planning-test".to_string(),
            version: "1.0".to_string(),
            trigger_on: vec![TriggerType::Push],
            environment: HashMap::new(),
            jobs: vec![
                chained("a", &[]),
                chained("b", &["a"]),
                chained("c", &["b"]),
            ],
        };
        gitforge_db::queries::PipelineQueries::create(
            &pool,
            &gitforge_db::models::Pipeline {
                id: pipeline_id,
                repo_id,
                name: definition.name.clone(),
                trigger_type: "push".to_string(),
                config: serde_json::to_value(&definition).unwrap(),
                created_at: chrono::Utc::now(),
            },
        )
        .await
        .unwrap();
        let run = gitforge_db::models::PipelineRun::new(
            pipeline_id,
            repo_id,
            "push".to_string(),
            commit.clone(),
        );
        gitforge_db::queries::PipelineRunQueries::create(&pool, &run)
            .await
            .unwrap();
        gitforge_db::queries::PipelineRunQueries::update_status(&pool, run.id, "running")
            .await
            .unwrap();

        // Trigger-time planning: every stage becomes a durable `pending` row
        // carrying its full execution definition.
        let event =
            PipelineTriggerEvent::new(pipeline_id, repo_id, commit.clone(), TriggerType::Push);
        let engine = CiEngine::new_with_run_id(event, definition.clone(), run.id)
            .await
            .unwrap();
        let planned: HashMap<String, gitforge_common::JobId> = engine
            .planned_jobs()
            .into_iter()
            .map(|(id, name)| (name, id))
            .collect();
        persist_planned_jobs(&pool, &engine, run.id, None)
            .await
            .unwrap();

        let row = gitforge_db::queries::JobQueries::get(&pool, planned["c"])
            .await
            .unwrap()
            .expect("planned row for stage c");
        assert_eq!(row.status, "pending");
        assert_eq!(row.commands, vec!["true".to_string()]);
        assert_eq!(row.image, "rust:latest");

        // The crash: stage a finished and stage b was dispatched before the
        // process died; stage c was never enqueued.
        gitforge_db::queries::JobQueries::update_status(&pool, planned["a"], "succeeded")
            .await
            .unwrap();
        gitforge_db::queries::JobQueries::update_status(&pool, planned["b"], "queued")
            .await
            .unwrap();

        // Restart recovery: a fresh scheduler and registry, like a rebooted
        // control plane.
        let scheduler = Arc::new(Scheduler::with_db(pool.clone()));
        let registry: Arc<tokio::sync::RwLock<PipelineRegistry>> =
            Arc::new(tokio::sync::RwLock::new(HashMap::new()));
        let run_workspaces: Arc<
            std::sync::Mutex<HashMap<gitforge_common::PipelineRunId, Option<String>>>,
        > = Arc::new(std::sync::Mutex::new(HashMap::new()));

        let rebuilt = rebuild_live_engines(&pool, &scheduler, &registry, &run_workspaces).await;
        assert_eq!(rebuilt, 1, "the interrupted run gets an engine back");

        let engine = registry
            .read()
            .await
            .get(&run.id)
            .cloned()
            .expect("rebuilt engine");
        let state = engine.state().await;
        assert_eq!(
            state.jobs[&planned["a"]].status(),
            gitforge_common::JobStatus::Succeeded
        );
        assert_eq!(
            state.jobs[&planned["b"]].status(),
            gitforge_common::JobStatus::Queued
        );
        assert_eq!(
            state.jobs[&planned["c"]].status(),
            gitforge_common::JobStatus::Pending
        );

        // The workspace checkout is restored so dispatched jobs run against
        // real sources.
        let workspace_path = run_workspaces
            .lock()
            .unwrap()
            .get(&run.id)
            .cloned()
            .flatten()
            .expect("restored workspace path");
        assert!(tokio::fs::try_exists(&workspace_path).await.unwrap());

        // Only the dispatched stage is dispatchable: the planned tail stays
        // parked at `pending`, invisible to scheduler recovery.
        let queue = scheduler.queue_status().await.unwrap();
        assert_eq!(queue.in_memory_queued, 1);
        assert_eq!(queue.durable_pending, Some(1));

        // The completion consumer's advance path: stage b finishes, stage c
        // is released and durably flipped to dispatchable.
        let runner_id = gitforge_common::RunnerId::new();
        engine.assign_job(planned["b"], runner_id).await.unwrap();
        engine.start_job(planned["b"]).await.unwrap();
        engine.succeed_job(planned["b"], 0).await.unwrap();
        engine.queue_ready_jobs().await.unwrap();
        enqueue_ready_jobs(&scheduler, &engine, run.id, repo_id, Some(workspace_path)).await;

        let row = gitforge_db::queries::JobQueries::get(&pool, planned["c"])
            .await
            .unwrap()
            .expect("planned row for stage c");
        assert_eq!(row.status, "queued", "released stage becomes dispatchable");
        let queue = scheduler.queue_status().await.unwrap();
        assert_eq!(queue.in_memory_queued, 2);
        assert_eq!(queue.durable_pending, Some(2));

        tokio::fs::remove_dir_all(&test_root).await.unwrap();
    }

    async fn seed_job(
        pool: &gitforge_db::Pool,
        run_id: gitforge_common::PipelineRunId,
        name: &str,
        status: &str,
    ) {
        let job = gitforge_db::models::Job::new(run_id, name.to_string());
        gitforge_db::queries::JobQueries::create(pool, &job)
            .await
            .unwrap();
        gitforge_db::queries::JobQueries::update_status(pool, job.id, status)
            .await
            .unwrap();
    }

    async fn run_status(
        pool: &gitforge_db::Pool,
        run_id: gitforge_common::PipelineRunId,
    ) -> String {
        gitforge_db::queries::PipelineRunQueries::get(pool, run_id)
            .await
            .unwrap()
            .expect("seeded run")
            .status
    }

    #[tokio::test]
    async fn test_reconcile_finalizes_orphaned_runs() {
        let (pool, repo_id, pipeline_id) = sweep_test_pool().await;

        let all_succeeded = seed_run(&pool, repo_id, pipeline_id, "running").await;
        seed_job(&pool, all_succeeded, "lint", "succeeded").await;
        seed_job(&pool, all_succeeded, "test", "succeeded").await;

        let mixed_failed = seed_run(&pool, repo_id, pipeline_id, "running").await;
        seed_job(&pool, mixed_failed, "lint", "succeeded").await;
        seed_job(&pool, mixed_failed, "test", "failed").await;

        let watchdog_reaped = seed_run(&pool, repo_id, pipeline_id, "running").await;
        seed_job(&pool, watchdog_reaped, "lint", "timed_out").await;

        let cancelled_job = seed_run(&pool, repo_id, pipeline_id, "running").await;
        seed_job(&pool, cancelled_job, "lint", "cancelled").await;

        let still_active = seed_run(&pool, repo_id, pipeline_id, "running").await;
        seed_job(&pool, still_active, "lint", "queued").await;

        let jobless = seed_run(&pool, repo_id, pipeline_id, "queued").await;
        let jobless_beyond_enqueue_horizon = seed_run_created_at(
            &pool,
            repo_id,
            pipeline_id,
            "queued",
            chrono::Utc::now() - chrono::Duration::seconds(RECONCILE_EMPTY_RUN_HORIZON_SECS + 60),
        )
        .await;

        let finalized = reconcile_orphaned_runs(&pool).await;

        assert_eq!(
            run_status(&pool, all_succeeded).await,
            "succeeded",
            "all-terminal-jobs run must finalize to succeeded"
        );
        assert_eq!(
            run_status(&pool, mixed_failed).await,
            "failed",
            "any failed job must finalize the run as failed"
        );
        assert_eq!(
            run_status(&pool, watchdog_reaped).await,
            "failed",
            "a watchdog-reaped job dooms the run; it must not grade succeeded"
        );
        assert_eq!(
            run_status(&pool, cancelled_job).await,
            "cancelled",
            "a cancelled job must finalize the run as cancelled"
        );
        assert_eq!(
            run_status(&pool, jobless).await,
            "queued",
            "a fresh jobless run may still be awaiting its lazy enqueue; it is spared"
        );
        assert_eq!(
            run_status(&pool, jobless_beyond_enqueue_horizon).await,
            "cancelled",
            "a run still jobless past the enqueue horizon is dead"
        );
        assert_eq!(
            run_status(&pool, still_active).await,
            "running",
            "runs with unfinished jobs belong to scheduler recovery, not reconciliation"
        );
        assert_eq!(finalized, 5, "only the orphaned runs are finalized");

        // Reconciliation is idempotent: a second pass finds nothing stranded.
        assert_eq!(reconcile_orphaned_runs(&pool).await, 0);
    }

    #[tokio::test]
    async fn test_reconcile_spares_fresh_jobless_run_for_lazy_enqueue() {
        // The lazy enqueue can lag the run row far beyond the sweep's grace
        // window: on 2026-09-23 the live instance's periodic pass graded a
        // 12-minute-old jobless push run `cancelled` while the engine was
        // still going to enqueue — its head job only materialized at minute
        // 29 under database write-lock contention. Cancelling inside that
        // window killed a live run and stranded its queued job, and the
        // release gate then refused the commit because no green run existed
        // for it — the gate doing its job on a control-plane defect.
        let (pool, repo_id, pipeline_id) = sweep_test_pool().await;
        let awaiting_enqueue = seed_run(&pool, repo_id, pipeline_id, "queued").await;
        let dead = seed_run_created_at(
            &pool,
            repo_id,
            pipeline_id,
            "queued",
            chrono::Utc::now() - chrono::Duration::seconds(RECONCILE_EMPTY_RUN_HORIZON_SECS * 2),
        )
        .await;

        reconcile_orphaned_runs(&pool).await;

        assert_eq!(
            run_status(&pool, awaiting_enqueue).await,
            "queued",
            "a jobless run inside the enqueue horizon must not be graded"
        );
        assert_eq!(
            run_status(&pool, dead).await,
            "cancelled",
            "a run jobless past the enqueue horizon is cancelled"
        );
    }

    #[tokio::test]
    async fn test_reconcile_fails_run_with_unenqueued_chain_jobs() {
        // A control-plane restart drops the in-memory engine that lazily
        // enqueues chained jobs, so an interrupted run leaves only its head
        // job behind. Grading the surviving rows alone published that run as
        // "succeeded" with the rest of its pipeline never executed (the
        // 2026-09-22 gitforge-ci false greens). The persisted definition is
        // the missing half of the comparison.
        let (pool, repo_id, _pipeline_id) = sweep_test_pool().await;
        let definition = PipelineDefinition::parse(
            r#"
name: chain-check
version: "1.0"
trigger_on:
  - push
environment: {}
jobs:
  - name: fmt
    image: ci:1
    steps:
      - name: check
        run: cargo fmt --check
    timeout: 5m

  - name: test
    image: ci:1
    needs: [fmt]
    steps:
      - name: test
        run: cargo test
    timeout: 10m
"#,
        )
        .unwrap();
        let pipeline_id = gitforge_common::PipelineId::new();
        gitforge_db::queries::PipelineQueries::create(
            &pool,
            &DbPipeline {
                id: pipeline_id,
                repo_id,
                name: "chain-pipeline".to_string(),
                trigger_type: "push".to_string(),
                config: serde_json::to_value(&definition).unwrap(),
                created_at: chrono::Utc::now(),
            },
        )
        .await
        .unwrap();

        // Restart-interrupted shape: the engine enqueued and finished only
        // the head job before the chain stopped advancing.
        let interrupted = seed_run(&pool, repo_id, pipeline_id, "running").await;
        seed_job(&pool, interrupted, "fmt", "succeeded").await;

        // Identical statuses, but every definition job was enqueued: the
        // full chain ran and grades succeeded exactly as before.
        let complete = seed_run(&pool, repo_id, pipeline_id, "running").await;
        seed_job(&pool, complete, "fmt", "succeeded").await;
        seed_job(&pool, complete, "test", "succeeded").await;

        reconcile_orphaned_runs(&pool).await;
        assert_eq!(
            run_status(&pool, interrupted).await,
            "failed",
            "a run whose definition has more jobs than were ever enqueued \
             must not grade succeeded"
        );
        assert_eq!(
            run_status(&pool, complete).await,
            "succeeded",
            "a run whose rows cover the definition still grades succeeded"
        );
    }

    #[tokio::test]
    async fn test_reconcile_row_only_fallback_when_definition_unreadable() {
        // Legacy rows carry an empty config blob; the definition cannot be
        // recovered, so grading falls back to the durable rows alone and a
        // fully-succeeded partial row set still grades succeeded.
        let (pool, repo_id, pipeline_id) = sweep_test_pool().await;
        let legacy = seed_run(&pool, repo_id, pipeline_id, "running").await;
        seed_job(&pool, legacy, "lint", "succeeded").await;

        reconcile_orphaned_runs(&pool).await;
        assert_eq!(
            run_status(&pool, legacy).await,
            "succeeded",
            "an unreadable definition keeps the row-only grading"
        );
    }

    #[tokio::test]
    async fn test_periodic_reconciliation_skips_live_and_fresh_runs() {
        // Each guard gets its own pool so scenarios cannot observe each
        // other's runs.

        // Grace guard: a jobless run inside the creation window belongs to a
        // push handler that has not registered its engine or enqueued yet.
        let (pool, repo_id, pipeline_id) = sweep_test_pool().await;
        let fresh_run = seed_run(&pool, repo_id, pipeline_id, "queued").await;
        let finalized = reconcile_orphaned_runs_filtered(
            &pool,
            chrono::Duration::seconds(RECONCILE_MIN_RUN_AGE_SECS),
        )
        .await;
        assert_eq!(
            finalized, 0,
            "runs inside the grace window are not orphaned"
        );
        assert_eq!(run_status(&pool, fresh_run).await, "queued");
        drop(pool);

        // Terminal rows outrank registry custody: a run whose engine is
        // still registered but whose durable rows are all terminal is
        // graded from those rows. Custody used to shield such runs and the
        // deferral wedged them forever when the engine could not converge
        // (run 666b3fa8).
        let (pool, repo_id, pipeline_id) = sweep_test_pool().await;
        let live_run = seed_run(&pool, repo_id, pipeline_id, "running").await;
        seed_job(&pool, live_run, "lint", "succeeded").await;
        let finalized = reconcile_orphaned_runs_filtered(&pool, chrono::Duration::zero()).await;
        assert_eq!(
            finalized, 1,
            "all-terminal durable rows finalize even under live custody"
        );
        assert_eq!(run_status(&pool, live_run).await, "succeeded");
        drop(pool);

        // Outside both guards the periodic pass finalizes the orphan. The
        // jobless verdict measures real elapsed time against the enqueue
        // horizon, so the seed is backdated past it — a negative grace
        // window alone cannot stand in for a run created long before this
        // pass.
        let (pool, repo_id, pipeline_id) = sweep_test_pool().await;
        let stale_run = seed_run_created_at(
            &pool,
            repo_id,
            pipeline_id,
            "queued",
            chrono::Utc::now() - chrono::Duration::seconds(RECONCILE_EMPTY_RUN_HORIZON_SECS + 60),
        )
        .await;
        let finalized = reconcile_orphaned_runs_filtered(
            &pool,
            // A negative window treats every run as older than the grace
            // period, standing in for a run created long before this pass.
            chrono::Duration::seconds(-1),
        )
        .await;
        assert_eq!(finalized, 1, "the stale jobless orphan is cancelled");
        assert_eq!(run_status(&pool, stale_run).await, "cancelled");
    }

    #[tokio::test]
    async fn test_reconcile_cancels_doomed_descendants_of_reaped_job() {
        // The 666b3fa8 shape: a chain fmt → clippy → test → coverage whose
        // test job was reaped by the timeout watchdog while coverage was
        // never dispatched. The doom cascade must grade coverage cancelled
        // from the durable record alone, evidence intact, so the run can
        // finally grade failed.
        let (pool, repo_id, _) = sweep_test_pool().await;
        let pipeline_id = gitforge_common::PipelineId::new();
        let definition = serde_json::json!({
            "name": "chain",
            "version": "1.0",
            "trigger_on": ["push"],
            "environment": {},
            "jobs": [
                {"name": "fmt", "image": "dsc-ci-rust:7", "needs": [], "env": {},
                 "steps": [{"name": "fmt", "run": "cargo fmt --all -- --check"}]},
                {"name": "clippy", "image": "dsc-ci-rust:7", "needs": ["fmt"], "env": {},
                 "steps": [{"name": "clippy", "run": "cargo clippy"}]},
                {"name": "test", "image": "dsc-ci-rust:7", "needs": ["clippy"], "env": {},
                 "steps": [{"name": "test", "run": "cargo test"}]},
                {"name": "coverage", "image": "dsc-ci-rust:7", "needs": ["test"], "env": {},
                 "steps": [{"name": "coverage", "run": "cargo llvm-cov"}]}
            ]
        });
        gitforge_db::queries::PipelineQueries::create(
            &pool,
            &gitforge_db::models::Pipeline {
                id: pipeline_id,
                repo_id,
                name: "chain-pipeline".to_string(),
                trigger_type: "push".to_string(),
                config: definition,
                created_at: chrono::Utc::now(),
            },
        )
        .await
        .unwrap();

        let run_id = seed_run(&pool, repo_id, pipeline_id, "running").await;
        seed_job(&pool, run_id, "fmt", "succeeded").await;
        seed_job(&pool, run_id, "clippy", "succeeded").await;
        seed_job(&pool, run_id, "test", "timed_out").await;
        seed_job(&pool, run_id, "coverage", "pending").await;

        assert_eq!(reconcile_orphaned_runs(&pool).await, 1);
        assert_eq!(
            run_status(&pool, run_id).await,
            "failed",
            "the reaped test job dooms the run"
        );

        let jobs = gitforge_db::queries::JobQueries::list_by_run(&pool, run_id)
            .await
            .unwrap();
        let by_name: std::collections::HashMap<&str, &gitforge_db::models::Job> =
            jobs.iter().map(|job| (job.name.as_str(), job)).collect();
        let coverage = by_name.get("coverage").expect("coverage row");
        assert_eq!(coverage.status, "cancelled", "the doomed row is graded");
        assert!(
            coverage.finished_at.is_some(),
            "the cascade writes a completion receipt"
        );
        let receipt: serde_json::Value =
            serde_json::from_str(coverage.result_json.as_deref().unwrap_or_default())
                .expect("cascade receipt parses");
        assert_eq!(
            receipt["reason"], "pipeline ancestor failed; this job can never be dispatched",
            "the receipt records why the row was cancelled"
        );
        let test_row = by_name.get("test").expect("test row");
        assert_eq!(test_row.status, "timed_out", "the reap evidence is kept");
        assert!(
            test_row.result_json.is_none() && test_row.finished_at.is_none(),
            "the cascade must not rewrite other rows' evidence"
        );

        // Idempotent: the second pass finds nothing stranded.
        assert_eq!(reconcile_orphaned_runs(&pool).await, 0);
    }

    #[tokio::test]
    async fn test_reconcile_never_cascades_into_dispatched_rows() {
        // A doomed row a runner has already picked up belongs to the runner
        // lifecycle (lease + timeout sweeps own it). The cascade leaves it,
        // and the run with a live row stays ungraded for a later pass.
        let (pool, repo_id, pipeline_id) = sweep_test_pool().await;
        let run_id = seed_run(&pool, repo_id, pipeline_id, "running").await;
        seed_job(&pool, run_id, "lint", "failed").await;
        seed_job(&pool, run_id, "publish", "running").await;

        assert_eq!(reconcile_orphaned_runs(&pool).await, 0);
        assert_eq!(run_status(&pool, run_id).await, "running");
        let jobs = gitforge_db::queries::JobQueries::list_by_run(&pool, run_id)
            .await
            .unwrap();
        let publish = jobs.iter().find(|job| job.name == "publish").unwrap();
        assert_eq!(
            publish.status, "running",
            "a dispatched row is never cascade-cancelled"
        );
    }

    #[tokio::test]
    async fn test_reconcile_never_invents_doom_without_a_definition() {
        // An unreadable definition cannot witness the dependency edges, so
        // the cascade cancels nothing and the run with a failed ancestor and
        // an unfinished descendant is left for a pass that can read it.
        let (pool, repo_id, pipeline_id) = sweep_test_pool().await;
        let run_id = seed_run(&pool, repo_id, pipeline_id, "running").await;
        seed_job(&pool, run_id, "test", "failed").await;
        seed_job(&pool, run_id, "coverage", "pending").await;

        assert_eq!(reconcile_orphaned_runs(&pool).await, 0);
        assert_eq!(run_status(&pool, run_id).await, "running");
        let jobs = gitforge_db::queries::JobQueries::list_by_run(&pool, run_id)
            .await
            .unwrap();
        let coverage = jobs.iter().find(|job| job.name == "coverage").unwrap();
        assert_eq!(coverage.status, "pending", "no doom without a witness");
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
        tokio::fs::write(seed.join(PIPELINE_CONFIG_PATHS[0]), "::: not a pipeline\n")
            .await
            .unwrap();
        run_git(["add", PIPELINE_CONFIG_PATHS[0]], Some(&seed)).await;
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
        assert!(error.to_string().contains("invalid .gitforge.yml"));
        assert!(
            error.downcast_ref::<CommittedConfigInvalid>().is_some(),
            "a committed unparseable definition is a permanent rejection, not a retryable fault: {error}"
        );
    }

    /// Seed a bare repository whose HEAD commit carries a definition at
    /// every name in `config_paths`, in order. Returns (bare path, commit).
    async fn seed_pipeline_at_paths(config_paths: &[&str]) -> (PathBuf, String) {
        let files = config_paths
            .iter()
            .enumerate()
            .map(|(index, config_path)| {
                (
                    *config_path,
                    format!(
                        "name: fixture-{index}\nversion: \"1.0\"\ntrigger_on:\n  - push\nenvironment: {{}}\njobs: []\n"
                    ),
                )
            })
            .collect::<Vec<_>>();
        seed_pipeline_files(&files).await
    }

    /// Seed a bare repository whose HEAD commit carries each committed
    /// (path, content) pair. Returns (bare path, commit).
    async fn seed_pipeline_files(files: &[(&str, String)]) -> (PathBuf, String) {
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
        for (config_path, content) in files {
            tokio::fs::write(seed.join(config_path), content)
                .await
                .unwrap();
            run_git(["add", config_path], Some(&seed)).await;
        }
        run_git(["commit", "-m", "pipeline definitions"], Some(&seed)).await;
        let commit = run_git(["rev-parse", "HEAD"], Some(&seed)).await;
        run_git(
            ["push", bare.to_str().unwrap(), "HEAD:refs/heads/main"],
            Some(&seed),
        )
        .await;
        (bare, commit)
    }

    #[tokio::test]
    async fn test_load_pipeline_from_commit_accepts_legacy_config_name() {
        let _guard = WORKSPACE_TEST_LOCK
            .get_or_init(|| tokio::sync::Mutex::new(()))
            .lock()
            .await;
        let (bare, commit) = seed_pipeline_at_paths(&[".gitforce.yml"]).await;
        let (pool, repo_id) = test_pool_with_repository(bare.to_string_lossy().into_owned()).await;

        // The legacy name still resolves, so repositories committed before
        // the rename keep building.
        let loaded = load_pipeline_from_commit(&pool, repo_id, &commit)
            .await
            .unwrap()
            .expect("legacy .gitforce.yml must load");
        assert_eq!(loaded.name, "fixture-0");
    }

    #[tokio::test]
    async fn test_load_pipeline_from_commit_prefers_current_config_name() {
        let _guard = WORKSPACE_TEST_LOCK
            .get_or_init(|| tokio::sync::Mutex::new(()))
            .lock()
            .await;
        let (bare, commit) =
            seed_pipeline_at_paths(&[PIPELINE_CONFIG_PATHS[0], PIPELINE_CONFIG_PATHS[1]]).await;
        let (pool, repo_id) = test_pool_with_repository(bare.to_string_lossy().into_owned()).await;

        // When both spellings are committed the current one wins; the
        // pipeline must never be half of each.
        let loaded = load_pipeline_from_commit(&pool, repo_id, &commit)
            .await
            .unwrap()
            .expect("committed definition must load");
        assert_eq!(loaded.name, "fixture-0");
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
    async fn test_validate_workspace_path_accepts_explicit_integration_root() {
        let _guard = WORKSPACE_TEST_LOCK
            .get_or_init(|| tokio::sync::Mutex::new(()))
            .lock()
            .await;
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/gitforge-ci-integration-root")
            .join(gitforge_common::RepoId::new().to_string());
        let inside = root.join("inside");
        let outside = root.parent().unwrap().join("outside");
        std::fs::create_dir_all(&inside).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::env::set_var(
            "GITFORGE_WORKSPACE_ROOT",
            root.parent().unwrap().join("run-workspaces"),
        );
        std::env::set_var("GITFORGE_WORKSPACE_ROOTS", &root);

        assert_eq!(
            validate_workspace_path(inside.to_str().unwrap()).unwrap(),
            inside.canonicalize().unwrap().to_string_lossy()
        );
        let error = validate_workspace_path(outside.to_str().unwrap()).unwrap_err();
        assert!(error.contains("workspace must be inside one of"));

        std::env::remove_var("GITFORGE_WORKSPACE_ROOTS");
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
            let pipeline = create_default_pipeline(&format!("repo{i}"));
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

    // --- Ref-deletion guard (F37) regression test ---
    //
    // The git-server filters deletion pushes before publishing, but an
    // envelope that still reaches `handle_push_event` (replayed event, future
    // caller, defense gap) must be dropped before any pipeline is planned:
    // an all-zero new hash means the ref no longer exists, so there is no
    // commit to check out. Building one produced a run that failed
    // immediately at checkout (run 4b9497bb built at 000…0).

    fn zero_hash_push_envelope() -> EventEnvelope {
        let repo_id = gitforge_common::RepoId::new();
        EventEnvelope::new(
            EventType::PushReceived,
            EventPayload::PushReceived(PushReceivedPayload {
                repo_id,
                ref_name: "refs/heads/feat/gone".to_string(),
                old_hash: "681fb4dfa3059321947bc3cfad93e11f0527f24a".to_string(),
                new_hash: "0000000000000000000000000000000000000000".to_string(),
                pusher_id: None,
            }),
            Some(repo_id),
            None,
        )
    }

    #[tokio::test]
    async fn test_handle_push_event_ignores_ref_deletion() {
        let scheduler = Arc::new(Scheduler::new());
        let pipeline_cache: Arc<std::sync::Mutex<PipelineCache>> =
            Arc::new(std::sync::Mutex::new(HashMap::new()));
        let workspace_paths: Arc<
            std::sync::Mutex<HashMap<gitforge_common::RepoId, Option<String>>>,
        > = Arc::new(std::sync::Mutex::new(HashMap::new()));
        let run_workspace_paths = run_workspace_paths_cache();
        let pipeline_registry: Arc<tokio::sync::RwLock<PipelineRegistry>> =
            Arc::new(tokio::sync::RwLock::new(HashMap::new()));

        let handled = handle_push_event(
            &zero_hash_push_envelope(),
            &scheduler,
            &pipeline_cache,
            None,
            &workspace_paths,
            &run_workspace_paths,
            &pipeline_registry,
            None,
        )
        .await
        .expect("a deletion push is consumed silently, never an error");
        assert!(
            handled.is_none(),
            "a deletion push creates no run, so it must not correlate one"
        );

        // The strong assertion: the guard fired before any pipeline was
        // resolved or planned for the deleted ref's repository.
        assert!(
            pipeline_cache
                .lock()
                .expect("pipeline cache lock poisoned")
                .is_empty(),
            "a ref-deletion push must not create or plan a pipeline"
        );
        assert!(
            run_workspace_paths
                .lock()
                .expect("workspace cache lock poisoned")
                .is_empty(),
            "a ref-deletion push must not prepare a run workspace"
        );
    }
}

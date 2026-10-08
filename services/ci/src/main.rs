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

/// Restart pacing for the supervised trigger-event consumer. The first retry
/// is quick so a one-shot failure costs the service one short delivery gap;
/// the delay doubles per consecutive failure up to
/// [`CONSUMER_RESTART_MAX_BACKOFF`], so a permanently failing consumer
/// cannot turn its own crash into a hot loop.
const CONSUMER_RESTART_MIN_BACKOFF: Duration = Duration::from_millis(250);
const CONSUMER_RESTART_MAX_BACKOFF: Duration = Duration::from_secs(30);

/// Liveness of the in-process trigger-event consumer. The trigger endpoint
/// consults this fail-closed: an accepted trigger is published to an
/// in-process bus, so while the consumer is down the event has no receiver
/// and nothing would ever claim, plan, or link it. The flag opens only when
/// the consumer's subscription is actually live — the supervisor keeps it
/// down across every restart window and the worker raises it after
/// `subscribe` returns — which keeps `/pipelines/trigger` refusing work with
/// an explicit 503 until an event would have a receiver, instead of silently
/// accepting a trigger whose run can never appear. A broadcast bus delivers
/// nothing to subscribers that do not exist yet, so readiness before the
/// first subscription is not a formality: an event published into that
/// window is simply gone.
#[derive(Debug)]
struct ConsumerHealth {
    running: AtomicBool,
}

impl Default for ConsumerHealth {
    /// `new` is the honest default: a service under construction has no
    /// consumer, and nothing else may open acceptance on its behalf.
    fn default() -> Self {
        Self::new()
    }
}

impl ConsumerHealth {
    fn new() -> Self {
        Self {
            running: AtomicBool::new(false),
        }
    }

    /// Raised only by the consumer itself, after its bus subscription is
    /// live: from this point a published event has a receiver.
    fn mark_running(&self) {
        self.running.store(true, Ordering::SeqCst);
    }

    fn mark_down(&self) {
        self.running.store(false, Ordering::SeqCst);
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
}

/// RAII guard for one consumer attempt's live subscription. Held for the
/// attempt's whole body: every exit path — normal return, error return, a
/// panic's unwind, or a task abort — runs `drop`, so an attempt that dies
/// cannot stay advertised healthy until the supervisor's next observation.
/// The supervisor's own `mark_down` remains as the backstop for the restart
/// window; the guard closes the same window for anything that ends the
/// attempt between supervision polls.
struct ConsumerSubscriptionGuard {
    health: Arc<ConsumerHealth>,
}

impl Drop for ConsumerSubscriptionGuard {
    fn drop(&mut self) {
        self.health.mark_down();
    }
}

struct TriggerState {
    event_bus: Arc<dyn EventBus>,
    workspace_paths: Arc<std::sync::Mutex<HashMap<gitforge_common::RepoId, Option<String>>>>,
    run_waiters: Arc<
        std::sync::Mutex<
            HashMap<uuid::Uuid, tokio::sync::oneshot::Sender<gitforge_common::PipelineRunId>>,
        >,
    >,
    /// Durable store for the trigger requests this service accepts. `None`
    /// keeps the historical in-memory development mode, where a trigger
    /// cannot be correlated after the response and the status endpoint
    /// answers 503.
    db: Option<gitforge_db::Pool>,
    /// Shared liveness of the process's trigger-event consumer; the submit
    /// endpoint refuses new work while it is down (fail closed).
    consumer_health: Arc<ConsumerHealth>,
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
        // Pool::migrate owns the durable trigger-request schema before the
        // HTTP listener can accept a trigger the service cannot correlate.
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
    let consumer_health = Arc::new(ConsumerHealth::new());
    let trigger_state = Arc::new(TriggerState {
        event_bus: event_bus.clone(),
        workspace_paths: workspace_paths.clone(),
        run_waiters: run_waiters.clone(),
        db: scheduler_db.clone(),
        consumer_health: consumer_health.clone(),
    });

    let scheduler_app = Router::new()
        .route("/health", axum::routing::get(health_check))
        .route(
            "/pipelines/trigger",
            axum::routing::post(trigger_pipeline).layer(middleware::from_fn(require_trigger_auth)),
        )
        .route(
            "/pipelines/trigger-requests/{trigger_id}",
            axum::routing::get(get_trigger_request)
                .layer(middleware::from_fn(require_trigger_request_read_auth)),
        )
        .merge(scheduler_routes(scheduler_state))
        .layer(Extension(trigger_state))
        .layer(TraceLayer::new_for_http());

    // Pipeline definitions cache
    let pipeline_cache: Arc<std::sync::Mutex<PipelineCache>> =
        Arc::new(std::sync::Mutex::new(HashMap::new()));
    let pipeline_registry: Arc<tokio::sync::RwLock<PipelineRegistry>> =
        Arc::new(tokio::sync::RwLock::new(HashMap::new()));

    // Shared shutdown flag; the handler is spawned before the consumer so
    // Ctrl+C also works while startup below waits for the first subscription.
    let shutdown = create_shutdown_flag();
    let shutdown_flag = shutdown.clone();
    spawn_shutdown_handler(shutdown_flag);

    // Start the event consumer under supervision. This task is the only
    // reader of the in-process bus the trigger endpoint publishes to, so an
    // unobserved exit — an error return, or a panic while handling an event —
    // used to leave the service running for the rest of the process lifetime
    // with trigger delivery dead. The supervisor restarts the consumer with a
    // bounded backoff, holds `consumer_health` down across every restart
    // window so `/pipelines/trigger` answers an explicit retryable 503
    // instead of accepting such work, and stops only on shutdown.
    //
    // It is spawned BEFORE the listener binds on purpose: a broadcast bus
    // delivers nothing to a subscriber that does not exist yet, so a trigger
    // accepted before the consumer's first subscription would be silently
    // lost. The listener comes up only once the consumer reports a live
    // subscription; until then no socket exists to accept a trigger at all.
    let event_bus_clone = event_bus.clone();
    let scheduler_clone = scheduler_arc.clone();
    let pipeline_cache_clone = pipeline_cache.clone();
    let scheduler_db_clone = scheduler_db.clone();
    let workspace_paths_clone = workspace_paths.clone();
    let run_workspace_paths_clone = run_workspace_paths.clone();
    let run_waiters_clone = run_waiters.clone();
    let pipeline_registry_clone = pipeline_registry.clone();
    let shutdown_consumer = shutdown.clone();
    let _consumer_handle = tokio::spawn(supervise_event_consumer(
        event_bus_clone,
        scheduler_clone,
        pipeline_cache_clone,
        scheduler_db_clone,
        workspace_paths_clone,
        run_workspace_paths_clone,
        pipeline_registry_clone,
        run_waiters_clone,
        consumer_health.clone(),
        shutdown_consumer,
    ));

    // Bound the wait: a consumer that cannot subscribe within 30 seconds is
    // a broken bus, not a slow one, and a service that would never be able
    // to deliver triggers must fail loudly rather than come up empty.
    let readiness_deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !consumer_health.is_running() {
        if shutdown.load(Ordering::SeqCst) {
            tracing::info!("shutdown requested before the trigger consumer subscribed");
            return Ok(());
        }
        if tokio::time::Instant::now() >= readiness_deadline {
            anyhow::bail!(
                "trigger-event consumer did not subscribe within 30s; \
                 refusing to serve triggers it could not deliver"
            );
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    // Ownership boundary for the restart sweep, captured before the listener
    // can accept this process's first trigger: a claimed-but-unlinked request
    // created at or before this instant belongs to the previous process,
    // whose in-memory bus died with it. Anything claimed after it belongs to
    // this process's consumer, which resolves the row on every code path.
    let boot_cutoff = Utc::now();

    let scheduler_addr = format!("0.0.0.0:{scheduler_port}");
    tracing::info!("starting Scheduler HTTP API on {}", scheduler_addr);

    let scheduler_listener = tokio::net::TcpListener::bind(&scheduler_addr).await?;
    let scheduler_handle = tokio::spawn(async move {
        axum::serve(scheduler_listener, scheduler_app)
            .await
            .unwrap();
    });

    tracing::info!("Scheduler HTTP API listening on {}", scheduler_addr);

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
            let abandoned = fail_trigger_claims_lost_at_restart(&sweep_pool, boot_cutoff).await;
            if abandoned > 0 {
                tracing::info!(abandoned, "startup abandoned trigger-claim sweep complete");
            }
            let stale = fail_stale_trigger_requests(&sweep_pool).await;
            if stale > 0 {
                tracing::info!(stale, "startup stale trigger-request sweep complete");
            }
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
/// a dedicated trigger token. Read-only scheduler credentials are never
/// accepted here; deployments must configure a submit credential explicitly.
fn configured_trigger_token(get_var: impl Fn(&str) -> Option<String>) -> Option<String> {
    ["GITFORGE_TRIGGER_TOKEN", "GITFORGE_CI_TRIGGER_TOKEN"]
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

/// Whether the submit and read credentials are configured to the same
/// non-empty value. Both arguments are values this service resolved from its
/// own environment (the resolvers drop empty values), so plain equality is
/// sufficient; neither value is ever logged. `None` on either side is not a
/// collision: configuring only one role is supported, and that role's
/// endpoint already fails closed on its missing credential.
fn trigger_credentials_collide(submit: Option<&str>, read: Option<&str>) -> bool {
    matches!((submit, read), (Some(submit), Some(read)) if submit == read)
}

/// The generic fail-closed answer for a deployment that configured one secret
/// for both credential roles. It names the misconfiguration and nothing else:
/// no request detail, no token material.
fn trigger_role_collision_response() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(serde_json::json!({"error": "trigger_auth_misconfigured"})),
    )
        .into_response()
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
    // One secret in both roles leaves no separation to enforce, so the
    // endpoint fails closed instead of honoring either role with it.
    if trigger_credentials_collide(
        Some(&expected),
        configured_trigger_request_read_token(|name| std::env::var(name).ok()).as_deref(),
    ) {
        return trigger_role_collision_response();
    }
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

/// Resolve the operator credential that may read the durable trigger-request
/// lifecycle. Deliberately disjoint from [`configured_trigger_token`]: the
/// trigger credential submits work and must never read status, and the
/// operator credential reads status and must never submit work. The fallback
/// to the shared scheduler token mirrors how the scheduler routes in this
/// process resolve `GET /pipelines/runs/{id}`, so one operator secret covers
/// both polling paths.
fn configured_trigger_request_read_token(
    get_var: impl Fn(&str) -> Option<String>,
) -> Option<String> {
    [
        "GITFORGE_SCHEDULER_OPERATOR_TOKEN",
        "GITFORGE_SCHEDULER_TOKEN",
    ]
    .into_iter()
    .find_map(|name| get_var(name).filter(|token| !token.is_empty()))
}

/// Gate the trigger-request status endpoint behind the operator credential.
/// Reads are accepted only through `Authorization`; the trigger credential's
/// dedicated header is not consulted, so a leaked trigger token buys no
/// visibility into other triggers.
async fn require_trigger_request_read_auth(request: Request, next: Next) -> Response {
    let Some(expected) = configured_trigger_request_read_token(|name| std::env::var(name).ok())
    else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"error": "trigger_request_auth_not_configured"})),
        )
            .into_response();
    };
    // Same guard as the submit path, from the read side: a deployment whose
    // two roles share one secret fails closed here too.
    if trigger_credentials_collide(
        configured_trigger_token(|name| std::env::var(name).ok()).as_deref(),
        Some(&expected),
    ) {
        return trigger_role_collision_response();
    }
    let supplied = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok());
    if trigger_token_matches(&expected, supplied) {
        next.run(request).await
    } else {
        (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": "trigger_request_auth_required"})),
        )
            .into_response()
    }
}

async fn trigger_pipeline(
    Extension(trigger_state): Extension<Arc<TriggerState>>,
    Json(request): Json<PipelineTriggerRequest>,
) -> impl axum::response::IntoResponse {
    // Fail closed while the trigger-event consumer is not running. An
    // accepted trigger is published to the in-process bus, and with the
    // consumer down nothing would ever claim, plan, or link it — the caller
    // would be holding an acceptance whose run never appears. The flag opens
    // only when a consumer's bus subscription is live and the supervisor
    // holds it down across every restart window, so the gap is explicit and
    // retryable rather than silent — including the window between process
    // boot and the first subscription.
    if !trigger_state.consumer_health.is_running() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"error": "trigger_consumer_unavailable"})),
        );
    }
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

    let event = EventEnvelope::new(
        EventType::PushReceived,
        EventPayload::PushReceived(PushReceivedPayload {
            repo_id,
            // Cloned so the durable record below can still read the request's
            // ref and commit.
            ref_name: request.ref_name.clone(),
            old_hash: request.old_hash,
            new_hash: request.new_hash.clone(),
            pusher_id: None,
        }),
        Some(repo_id),
        None,
    );

    let (run_tx, run_rx) = tokio::sync::oneshot::channel();

    // Mint the durable correlation handle before publishing: the consumer can
    // plan the run as soon as the event lands and links it back through
    // `event_id`. Without a database the trigger keeps its historical
    // in-memory behavior and simply cannot be polled afterwards.
    let trigger_record = match trigger_state.db.as_ref() {
        Some(pool) => match record_trigger_request(
            pool,
            repo_id,
            &request.ref_name,
            &request.new_hash,
            event.event_id,
        )
        .await
        {
            Ok(record) => Some(record),
            Err(error) => {
                tracing::error!(%error, repo = %repo_id, "failed to record trigger request");
                // Fail closed: publishing here would create a run no caller
                // can ever correlate.
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(serde_json::json!({"error": "trigger_request_store_unavailable"})),
                );
            }
        },
        None => None,
    };
    if let Some(record) = trigger_record.filter(|record| record.deduplicated) {
        // A request for the same push is still open and already planning a
        // run. Publishing a second event would defeat the dedup, so the
        // caller is pointed at the id it can poll instead.
        return (
            StatusCode::ACCEPTED,
            Json(serde_json::json!({
                "status": "deduplicated",
                "trigger_id": record.trigger_id.to_string(),
                "deduplicated": true,
                "repo_id": repo_id.to_string(),
                "new_hash": request.new_hash,
            })),
        );
    }

    // The requested workspace is published only after the dedupe verdict, so
    // a duplicate carrying a different working_dir cannot override the path
    // the original request is already building in. It must land before the
    // publish below: the event consumer reads this cache while planning.
    trigger_state
        .workspace_paths
        .lock()
        .expect("workspace cache lock poisoned")
        .insert(repo_id, working_dir);

    trigger_state
        .run_waiters
        .lock()
        .expect("run waiter lock poisoned")
        .insert(event.event_id, run_tx);

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
            let mut payload = serde_json::json!({
                "status": if pipeline_run_id.is_some() { "accepted" } else { "queued" },
                "event_id": event.event_id.to_string(),
                "pipeline_run_id": pipeline_run_id.map(|id| id.to_string()),
                "repo_id": repo_id.to_string(),
                "new_hash": request.new_hash,
            });
            if let Some(record) = &trigger_record {
                // Stable across the new and deduplicated paths, so a caller
                // polls the lifecycle instead of re-submitting the trigger:
                // after a request completes, a repeat POST is a new build.
                payload["trigger_id"] = serde_json::json!(record.trigger_id.to_string());
                payload["deduplicated"] = serde_json::json!(false);
            }
            (StatusCode::ACCEPTED, Json(payload))
        }
        Err(error) => {
            // The waiter registered above can never be delivered — the event
            // never reached a consumer. Drop it so the map holds no dangling
            // sender for this event.
            trigger_state
                .run_waiters
                .lock()
                .expect("run waiter lock poisoned")
                .remove(&event.event_id);
            // A durable request was opened before publish so that fast
            // consumers can correlate it. If publication fails, close that
            // request or future retries will deduplicate into a trigger that
            // can never run.
            if let Some(pool) = trigger_state.db.as_ref() {
                fail_trigger_request_for_event(pool, event.event_id, &error).await;
            }
            tracing::error!(%error, event = %event.event_id, "failed to publish trigger event");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({"error": "event_publish_failed"})),
            )
        }
    }
}

/// Lifecycle of a durable trigger request.
///
/// `pending` → `claimed` → `processing` are the open states a repeat trigger
/// deduplicates into; `completed` and `failed` are terminal, so a repeat
/// trigger for the same push is a new build rather than a status query.
///
/// The `claimed` transition is what separates a live event from a lost one.
/// `pending` means the event was published into the in-memory bus but no
/// consumer has taken it yet; `claimed` means the consumer received it and
/// now owns the row — it will either reserve its run id and link a run
/// (`processing`), close the row with a failure cause, or die holding the
/// claim, in which case the successor attempt's recovery sweep closes it if
/// no run came to exist (the next boot's restart sweep is the same verdict
/// across a process boundary), and a claim whose run does exist resolves
/// through the run row. The window sweep only ever fails `pending` rows, and
/// a late event whose row it already failed is dropped at claim time instead
/// of planning a second run for the same push. Three fail-closed rules
/// complete the machine: planning happens only under a durable claim, so a
/// claim write that cannot land drops its event rather than proceeding; the
/// run id is reserved on the claim before any run-side write, and a
/// reservation that cannot land refuses to plan; and a terminal verdict is
/// final — no later write links or reopens a `failed` row.
const TRIGGER_REQUEST_PENDING: &str = "pending";
const TRIGGER_REQUEST_CLAIMED: &str = "claimed";
const TRIGGER_REQUEST_PROCESSING: &str = "processing";
const TRIGGER_REQUEST_COMPLETED: &str = "completed";
const TRIGGER_REQUEST_FAILED: &str = "failed";

/// A durable trigger request: the record behind `POST /pipelines/trigger`'s
/// `trigger_id` and the lifecycle the status endpoint reports. The row's
/// `event_id` column is the consumer's lookup key and is deliberately not
/// part of this view. The table and indexes are owned by
/// `gitforge_db::Pool::migrate`, while this service owns lifecycle behavior.
#[derive(Debug, Clone)]
struct TriggerRequestRow {
    id: uuid::Uuid,
    repo_id: String,
    ref_name: String,
    new_hash: String,
    status: String,
    pipeline_run_id: Option<String>,
    error: Option<String>,
    created_at: String,
    updated_at: String,
}

impl TriggerRequestRow {
    /// Body of the status endpoint. Correlation and lifecycle evidence only:
    /// the row holds no credential, and the endpoint's error paths are fixed
    /// codes, so no response can echo a secret.
    fn to_response(&self) -> serde_json::Value {
        serde_json::json!({
            "trigger_id": self.id.to_string(),
            "status": self.status,
            "repo_id": self.repo_id,
            "ref_name": self.ref_name,
            "new_hash": self.new_hash,
            "pipeline_run_id": self.pipeline_run_id,
            "error": self.error,
            "created_at": self.created_at,
            "updated_at": self.updated_at,
        })
    }
}

/// The result of recording an accepted trigger.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TriggerRequestRecord {
    trigger_id: uuid::Uuid,
    deduplicated: bool,
}

/// Trigger request statuses a repeat trigger never deduplicates into.
fn is_terminal_trigger_status(status: &str) -> bool {
    matches!(status, TRIGGER_REQUEST_COMPLETED | TRIGGER_REQUEST_FAILED)
}

/// Terminal verdicts of a durable pipeline run row.
fn is_terminal_run_status(status: &str) -> bool {
    matches!(
        status,
        "succeeded" | "failed" | "cancelled" | "timed_out" | "timeout" | "timed-out"
    )
}

/// Resolve the lifecycle a trigger request should report from its stored
/// status plus the linked run row, when there is one. `run` is the run's
/// `(status, error)`.
///
/// A stored terminal status wins. Otherwise the run row grades the request:
/// a run the reconciler or a restart finalized without the trigger-status
/// write landing must not read as still open forever — the durable rows are
/// the truth, the same rule the orphan-run reconciler applies to runs.
fn grade_trigger_status(
    stored_status: &str,
    stored_error: Option<&str>,
    run: Option<(&str, Option<&str>)>,
) -> (String, Option<String>) {
    let stored = (stored_status.to_string(), stored_error.map(str::to_string));
    if is_terminal_trigger_status(stored_status) {
        return stored;
    }
    let Some((run_status, run_error)) = run else {
        return stored;
    };
    if run_status == "succeeded" {
        return (TRIGGER_REQUEST_COMPLETED.to_string(), None);
    }
    if is_terminal_run_status(run_status) {
        return (
            TRIGGER_REQUEST_FAILED.to_string(),
            Some(run_error.unwrap_or(run_status).to_string()),
        );
    }
    if run_status_is_open(run_status) {
        return (TRIGGER_REQUEST_PROCESSING.to_string(), None);
    }
    // An unrecognised run status degrades to what the request row says.
    stored
}

/// Run statuses that mean the run still exists and has not finished.
fn run_status_is_open(status: &str) -> bool {
    matches!(status, "pending" | "running" | "queued")
}

/// Read one trigger request, graded against its linked run.
///
/// Returns `Ok(None)` for an unknown id.
async fn graded_trigger_request(
    pool: &gitforge_db::Pool,
    trigger_id: &uuid::Uuid,
) -> anyhow::Result<Option<TriggerRequestRow>> {
    let Some(mut row) = load_trigger_request(pool, trigger_id).await? else {
        return Ok(None);
    };
    if is_terminal_trigger_status(&row.status) {
        return Ok(Some(row));
    }
    if let Some(run_id) = row.pipeline_run_id.as_deref() {
        // A stored run id is always minted by this service; an unparseable one
        // degrades to the stored status instead of failing a read.
        if let Ok(run_id) = uuid::Uuid::parse_str(run_id) {
            let run = gitforge_db::queries::PipelineRunQueries::get(
                pool,
                gitforge_common::PipelineRunId::from(run_id),
            )
            .await
            .ok()
            .flatten();
            if let Some(run) = run {
                let (status, error) = grade_trigger_status(
                    &row.status,
                    row.error.as_deref(),
                    Some((run.status.as_str(), run.error.as_deref())),
                );
                row.status = status;
                row.error = error;
            }
        }
    }
    Ok(Some(row))
}

async fn load_trigger_request(
    pool: &gitforge_db::Pool,
    trigger_id: &uuid::Uuid,
) -> anyhow::Result<Option<TriggerRequestRow>> {
    use sqlx::Row;
    let row = sqlx::query(
        "SELECT id, repo_id, ref_name, new_hash, status, pipeline_run_id, error, \
         created_at, updated_at FROM ci_trigger_requests WHERE id = ?",
    )
    .bind(trigger_id.to_string())
    .fetch_optional(pool.pool())
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    Ok(Some(TriggerRequestRow {
        id: uuid::Uuid::parse_str(row.try_get("id")?)?,
        repo_id: row.try_get("repo_id")?,
        ref_name: row.try_get("ref_name")?,
        new_hash: row.try_get("new_hash")?,
        status: row.try_get("status")?,
        pipeline_run_id: row.try_get("pipeline_run_id")?,
        error: row.try_get("error")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    }))
}

/// Record an accepted trigger, or return the request that is already open for
/// the same push.
///
/// Deduplication is deliberately narrow — only a request that can still be
/// resolved absorbs a repeat trigger:
/// - `processing`, because the linked run is durable and the watchdog
///   guarantees it finalizes;
/// - `claimed`, because a live consumer received the event and resolves the
///   row on every path;
/// - `pending` inside the correlation window, because its event is still
///   queued in this process's bus. The dedup read and the stale sweep apply
///   the same cutoff (`stale_pending_cutoff`), so there is no gap in which a
///   retry is admitted while the sweep is about to rule the request lost —
///   that gap once admitted a retry that created a second event while the
///   first was still queued.
///
/// Once a request is terminal the same push is a new build, so a repeat
/// trigger creates a fresh request; a caller that wants status polls the
/// `trigger_id` it was given instead of re-submitting.
async fn record_trigger_request(
    pool: &gitforge_db::Pool,
    repo_id: gitforge_common::RepoId,
    ref_name: &str,
    new_hash: &str,
    event_id: uuid::Uuid,
) -> anyhow::Result<TriggerRequestRecord> {
    // BEGIN IMMEDIATE holds the write lock across the check and the insert, so
    // two recorders for the same push cannot both decide they are first.
    let mut tx = pool.pool().begin_with("BEGIN IMMEDIATE").await?;
    let now = Utc::now();
    let now_text = now.to_rfc3339();
    let pending_cutoff = stale_pending_cutoff(now).to_rfc3339();
    // Retire expired pending deliveries under the same write lock as the
    // dedupe/insert. Otherwise a retry could be inserted while the old event
    // row remains claimable, allowing both deliveries to plan a run.
    sqlx::query(
        "UPDATE ci_trigger_requests SET status = ?, error = ?, updated_at = ? \
         WHERE repo_id = ? AND ref_name = ? AND new_hash = ? \
           AND status = ? AND created_at <= ?",
    )
    .bind(TRIGGER_REQUEST_FAILED)
    .bind("pending trigger expired before retry")
    .bind(&now_text)
    .bind(repo_id.to_string())
    .bind(ref_name)
    .bind(new_hash)
    .bind(TRIGGER_REQUEST_PENDING)
    .bind(&pending_cutoff)
    .execute(&mut *tx)
    .await?;
    if let Some(trigger_id) =
        open_trigger_request(&mut *tx, repo_id, ref_name, new_hash, &pending_cutoff).await?
    {
        // Preserve any stale-row retirement above even when a live claimed
        // or processing request absorbs this retry.
        tx.commit().await?;
        return Ok(TriggerRequestRecord {
            trigger_id,
            deduplicated: true,
        });
    }
    let trigger_id = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO ci_trigger_requests \
         (id, event_id, repo_id, ref_name, new_hash, status, pipeline_run_id, error, \
          created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, NULL, NULL, ?, ?)",
    )
    .bind(trigger_id.to_string())
    .bind(event_id.to_string())
    .bind(repo_id.to_string())
    .bind(ref_name)
    .bind(new_hash)
    .bind(TRIGGER_REQUEST_PENDING)
    .bind(&now_text)
    .bind(&now_text)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(TriggerRequestRecord {
        trigger_id,
        deduplicated: false,
    })
}

/// Id of the request that is still open for a push, most recent first.
///
/// Open means `processing`, or `claimed` at any age, or `pending` inside the
/// correlation window, and not linked to a run that already reached a
/// terminal verdict — the run row grades the request, so a build that
/// finished without the trigger-status write landing is never reported as
/// still running. A `claimed` row is open at any age because its consumer is
/// alive and resolves the row on every code path; only the death of the
/// consumer that holds it — a process restart, or the supervised attempt
/// being replaced — closes such claims, through the sweeps that run before
/// the successor accepts work, and only then when no run exists behind the
/// claim. A `pending` row older
/// than the correlation window lost its in-memory event — the consumer never
/// received it — and blocking a re-submission on it would wedge the caller on
/// an id that can never finish. `fail_stale_trigger_requests` records that
/// same verdict durably so the row does not sit `pending` forever.
async fn open_trigger_request<'e, E>(
    executor: E,
    repo_id: gitforge_common::RepoId,
    ref_name: &str,
    new_hash: &str,
    pending_cutoff: &str,
) -> anyhow::Result<Option<uuid::Uuid>>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    let existing = sqlx::query_scalar::<_, String>(
        "SELECT id FROM ci_trigger_requests \
         WHERE repo_id = ? AND ref_name = ? AND new_hash = ? \
           AND (status = ? OR status = ? OR (status = ? AND created_at > ?)) \
           AND NOT EXISTS ( \
               SELECT 1 FROM pipeline_runs runs \
                WHERE runs.id = ci_trigger_requests.pipeline_run_id \
                  AND runs.status IN ('succeeded', 'failed', 'cancelled', \
                                      'timed_out', 'timeout', 'timed-out')) \
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(repo_id.to_string())
    .bind(ref_name)
    .bind(new_hash)
    .bind(TRIGGER_REQUEST_PROCESSING)
    .bind(TRIGGER_REQUEST_CLAIMED)
    .bind(TRIGGER_REQUEST_PENDING)
    .bind(pending_cutoff)
    .fetch_optional(executor)
    .await?;
    existing
        .map(|id| uuid::Uuid::parse_str(&id).map_err(anyhow::Error::from))
        .transpose()
}

/// Reserve the run id a claimed event is about to plan on its request row —
/// *before* any run-side write exists. This is the durability seam the
/// claimed-request recovery leans on: from here, a `claimed` row either
/// names its run id (and a `pipeline_runs` row for it may or may not exist
/// yet) or provably has none, which is exactly the distinction
/// [`fail_trigger_claims_lost_without_run`] is allowed to act on.
///
/// Like the claim, the reservation is fail-closed: a write that does not land
/// exactly once leaves correlation unproven, and planning on an unproven
/// reservation could create a run the recovery sweep later rules lost —
/// inviting a retry that builds the same push twice. The caller must treat
/// `false` as a refusal to plan.
async fn reserve_trigger_request_run(
    pool: &gitforge_db::Pool,
    event_id: uuid::Uuid,
    run_id: gitforge_common::PipelineRunId,
) -> bool {
    match sqlx::query(
        "UPDATE ci_trigger_requests SET pipeline_run_id = ?, updated_at = ? \
         WHERE event_id = ? AND status = ? AND pipeline_run_id IS NULL",
    )
    .bind(run_id.to_string())
    .bind(Utc::now().to_rfc3339())
    .bind(event_id.to_string())
    .bind(TRIGGER_REQUEST_CLAIMED)
    .execute(pool.pool())
    .await
    {
        Ok(result) if result.rows_affected() == 1 => true,
        // The row is no longer a live claim: the recovery sweep or a consumer
        // failure path closed it while this event was being planned. None of
        // those states can absorb a run.
        Ok(_) => false,
        Err(error) => {
            tracing::warn!(
                %error,
                event = %event_id,
                "trigger run reservation failed; refusing to plan without durable correlation"
            );
            false
        }
    }
}

/// Link a trigger request to the run its consumer planned. Best effort: the
/// correlation write must never fail the run it describes; read-time grading
/// heals a lost terminal-status write on a request that was linked.
///
/// The link lifts a row with a durable `claimed` receipt into `processing`,
/// which stays open for the run's whole life. Only the exact
/// (event, reserved run id) pair links: the run id was written by
/// [`reserve_trigger_request_run`] before the run row existed, so the link
/// can never attach a request to a run some other claim reserved. `pending`
/// does not prove this consumer owns the event, and a terminal `failed`
/// request is never linked or reopened.
async fn mark_trigger_request_processing(
    pool: &gitforge_db::Pool,
    event_id: uuid::Uuid,
    run_id: gitforge_common::PipelineRunId,
) {
    if let Err(error) = sqlx::query(
        "UPDATE ci_trigger_requests SET status = ?, updated_at = ? \
         WHERE event_id = ? AND status = ? AND pipeline_run_id = ?",
    )
    .bind(TRIGGER_REQUEST_PROCESSING)
    .bind(Utc::now().to_rfc3339())
    .bind(event_id.to_string())
    .bind(TRIGGER_REQUEST_CLAIMED)
    .bind(run_id.to_string())
    .execute(pool.pool())
    .await
    {
        tracing::warn!(%error, event = %event_id, "failed to link trigger request to its run");
    }
}

/// Close the trigger request an event was recorded under with a failure
/// cause. Best effort, and idempotent against the run-linked close. The
/// consumer's failure paths run while it holds the row in `claimed` — the
/// receipt it took before handling — so that state closes here too: only the
/// consumer that claimed a row may fail its claim.
async fn fail_trigger_request_for_event(
    pool: &gitforge_db::Pool,
    event_id: uuid::Uuid,
    error: &impl std::fmt::Display,
) {
    let cause = error.to_string();
    if let Err(close_error) = sqlx::query(
        "UPDATE ci_trigger_requests SET status = ?, error = ?, updated_at = ? \
         WHERE event_id = ? AND status IN (?, ?, ?)",
    )
    .bind(TRIGGER_REQUEST_FAILED)
    .bind(&cause)
    .bind(Utc::now().to_rfc3339())
    .bind(event_id.to_string())
    .bind(TRIGGER_REQUEST_PENDING)
    .bind(TRIGGER_REQUEST_CLAIMED)
    .bind(TRIGGER_REQUEST_PROCESSING)
    .execute(pool.pool())
    .await
    {
        tracing::warn!(error = %close_error, event = %event_id, "failed to fail trigger request");
    }
}

/// The outcome of the live consumer's claim on the request an event was
/// recorded under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TriggerClaim {
    /// The row moved `pending` → `claimed`: this consumer owns the request
    /// and resolves it on every code path — it links its run or closes the
    /// row with a failure cause.
    Claimed,
    /// The row was no longer `pending`: the stale sweep already failed it as
    /// lost, another delivery owns it, it is terminal, or it was never
    /// recorded. None of those states can absorb a run, so this late queued
    /// event must not plan one.
    Superseded,
    /// The claim write itself failed, so the row's ownership is unknown. The
    /// durable claim is the only proof that a live consumer received this
    /// event and may resolve its request; without it, planning a run here
    /// could build a push whose request was already closed. The event is
    /// dropped and the request resolves through the stale sweep or the
    /// caller's retry — never through a run planned without a claim.
    Indeterminate,
}

/// Atomically claim the request an event was recorded under: exactly
/// `pending` → `claimed` in one conditional UPDATE, so the stale sweep's
/// verdict and the consumer's receipt can never interleave. `None` is the
/// historical no-store development mode — there is no row to claim and every
/// published event proceeds. A claim whose write fails returns
/// [`TriggerClaim::Indeterminate`] and never [`TriggerClaim::Claimed`]: a run
/// is planned only on a durable claim.
async fn claim_trigger_request(
    pool: Option<&gitforge_db::Pool>,
    event_id: uuid::Uuid,
) -> TriggerClaim {
    let Some(pool) = pool else {
        return TriggerClaim::Claimed;
    };
    match sqlx::query(
        "UPDATE ci_trigger_requests SET status = ?, updated_at = ? \
         WHERE event_id = ? AND status = ?",
    )
    .bind(TRIGGER_REQUEST_CLAIMED)
    .bind(Utc::now().to_rfc3339())
    .bind(event_id.to_string())
    .bind(TRIGGER_REQUEST_PENDING)
    .execute(pool.pool())
    .await
    {
        Ok(result) if result.rows_affected() == 1 => TriggerClaim::Claimed,
        // The write landed but matched nothing: the request is no longer
        // `pending`, so this delivery is late and the state it finds owns the
        // rebuild. An event with no recorded row cannot be correlated either,
        // so it is refused the same way rather than planned blind.
        Ok(_) => TriggerClaim::Superseded,
        Err(error) => {
            // Fail closed. The claim is the receipt proving this consumer
            // received the event and owns the row; a write that did not land
            // leaves ownership unproven, and planning on an unproven claim is
            // exactly the race this receipt exists to close — the row may
            // already have been swept failed, with its caller told to retry.
            // The event is dropped instead; the request resolves through the
            // stale sweep, or the caller's retry records a fresh one.
            tracing::warn!(
                %error,
                event = %event_id,
                "trigger claim write failed; refusing to plan without a durable claim"
            );
            TriggerClaim::Indeterminate
        }
    }
}

/// The instant before which an unlinked `pending` request is ruled lost: its
/// event had the whole correlation window to reach the consumer. The dedup
/// read and the stale sweep both derive their verdict from this one cutoff,
/// so neither can admit a retry the other is about to fail.
fn stale_pending_cutoff(now: chrono::DateTime<Utc>) -> chrono::DateTime<Utc> {
    now - chrono::Duration::seconds(
        i64::try_from(gitforge_common::CI_TRIGGER_CORRELATION_WINDOW.as_secs()).unwrap_or(i64::MAX),
    )
}

/// Durably close the verdict `open_trigger_request` already applies at read
/// time: a `pending` request with no linked run that outlived the correlation
/// window lost its in-memory event, so it can never resolve and must not sit
/// `pending` forever. Only `pending` rows match: a `claimed` row is a live
/// event this process's consumer owns, whatever its age, and this sweep must
/// never rule it lost. Rows linked to a run are never touched here — their
/// lifecycle is graded from the run row, so an active run is not failed by
/// this sweep. A consumer that claims before planning is out of this sweep's
/// reach entirely: only `pending` rows match, so a slow planner under a live
/// claim is never failed. A `pending` row this sweep closes was still queued
/// when it expired; when its event is finally delivered, the claim matches
/// nothing and the delivery is dropped instead of planning a second run for
/// the push, and the caller's retry records a fresh request.
/// Returns the number of requests closed.
async fn fail_stale_trigger_requests(pool: &gitforge_db::Pool) -> u64 {
    let cutoff = stale_pending_cutoff(Utc::now());
    match sqlx::query(
        "UPDATE ci_trigger_requests SET status = ?, error = ?, updated_at = ? \
         WHERE status = ? AND pipeline_run_id IS NULL AND created_at <= ?",
    )
    .bind(TRIGGER_REQUEST_FAILED)
    .bind("trigger event was never planned within the correlation window")
    .bind(Utc::now().to_rfc3339())
    .bind(TRIGGER_REQUEST_PENDING)
    .bind(cutoff.to_rfc3339())
    .execute(pool.pool())
    .await
    {
        Ok(result) => result.rows_affected(),
        Err(error) => {
            tracing::warn!(%error, "stale trigger-request sweep failed");
            0
        }
    }
}

/// Close the `claimed` trigger requests that provably have no run behind
/// them: the in-process recovery for a supervised consumer attempt that died
/// while holding a claim. Two shapes can exist, and both are safe to fail:
///
/// - `pipeline_run_id IS NULL` — the attempt claimed the row but died before
///   reserving its run id. Planning reserves the run id on the claimed row
///   *before* any run-side write and refuses to plan if that write does not
///   land, so a claimed row without a run id cannot have a run row anywhere.
/// - `pipeline_run_id` names a `pipeline_runs` row that does not exist — the
///   attempt reserved its run id but died between that reservation and the
///   run row's creation.
///
/// A claimed row whose run row *does* exist is never touched here, whatever
/// its age: the run was really created, and failing its request would invite
/// a retry that builds the same push twice. Such a row resolves through the
/// run itself — read-time grading reports the run's verdict and
/// `complete_trigger_request_for_run` closes the row when the run reaches
/// one. A matched row's reservation — when one existed — is cleared: the
/// sweep just proved no run row exists behind that id, so the terminal row
/// carries no correlation that names nothing. The single-consumer invariant
/// makes the timing safe: this runs at an attempt's start, before it claims
/// anything, so every matched row was held by an attempt that is already
/// dead and whose in-memory events are gone. Returns the number of requests
/// closed, or an error when the store cannot be written: the caller
/// propagates that instead of swallowing it, because a sweep that ran against
/// an unavailable store proved nothing — proceeding would leave another
/// attempt's lost claims standing while acceptance reopens on top of them.
async fn fail_trigger_claims_lost_without_run(pool: &gitforge_db::Pool) -> anyhow::Result<u64> {
    let result = sqlx::query(
        "UPDATE ci_trigger_requests SET status = ?, error = ?, pipeline_run_id = NULL, \
         updated_at = ? \
         WHERE status = ? AND ( \
             pipeline_run_id IS NULL \
             OR NOT EXISTS ( \
                 SELECT 1 FROM pipeline_runs runs \
                  WHERE runs.id = ci_trigger_requests.pipeline_run_id) \
         )",
    )
    .bind(TRIGGER_REQUEST_FAILED)
    .bind(
        "the consumer that claimed this trigger was lost to a restart or \
         crash before the run was created",
    )
    .bind(Utc::now().to_rfc3339())
    .bind(TRIGGER_REQUEST_CLAIMED)
    .execute(pool.pool())
    .await?;
    Ok(result.rows_affected())
}

/// Close the trigger requests the previous process claimed but never
/// resolved. A `claimed` row's event lived in the previous process's
/// in-memory bus, which died with that process, so no consumer will ever link
/// a run or record a failure for it. Rows created after `boot_cutoff` are
/// claims this process's own consumer took and resolves on every code path;
/// `pending` rows are the stale sweep's jurisdiction, and rows linked to a
/// run grade from the run row. Returns the number of requests closed.
async fn fail_trigger_claims_lost_at_restart(
    pool: &gitforge_db::Pool,
    boot_cutoff: chrono::DateTime<Utc>,
) -> u64 {
    match sqlx::query(
        "UPDATE ci_trigger_requests SET status = ?, error = ?, updated_at = ? \
         WHERE status = ? AND pipeline_run_id IS NULL AND created_at <= ?",
    )
    .bind(TRIGGER_REQUEST_FAILED)
    .bind("control-plane restart lost the claimed trigger event before it was planned")
    .bind(Utc::now().to_rfc3339())
    .bind(TRIGGER_REQUEST_CLAIMED)
    .bind(boot_cutoff.to_rfc3339())
    .execute(pool.pool())
    .await
    {
        Ok(result) => result.rows_affected(),
        Err(error) => {
            tracing::warn!(%error, "restart trigger-claim sweep failed");
            0
        }
    }
}

/// Close the trigger requests linked to a run with the run's own verdict.
/// `error` carries the failure cause; `None` means the run succeeded.
///
/// `claimed` closes here too: a claim that reserved its run id but died
/// before the `processing` link still names a run that really exists, and the
/// run's durable verdict is the resolution the dead attempt never wrote. The
/// reservation only ever happens under a live claim on this consumer's own
/// planned run, so a verdict matched through `pipeline_run_id` is that
/// request's own run — never a second build of the same push.
async fn complete_trigger_request_for_run(
    pool: &gitforge_db::Pool,
    run_id: gitforge_common::PipelineRunId,
    error: Option<&str>,
) {
    let status = if error.is_some() {
        TRIGGER_REQUEST_FAILED
    } else {
        TRIGGER_REQUEST_COMPLETED
    };
    if let Err(close_error) = sqlx::query(
        "UPDATE ci_trigger_requests SET status = ?, error = COALESCE(?, error), updated_at = ? \
         WHERE pipeline_run_id = ? AND status IN (?, ?, ?)",
    )
    .bind(status)
    .bind(error)
    .bind(Utc::now().to_rfc3339())
    .bind(run_id.to_string())
    .bind(TRIGGER_REQUEST_PENDING)
    .bind(TRIGGER_REQUEST_CLAIMED)
    .bind(TRIGGER_REQUEST_PROCESSING)
    .execute(pool.pool())
    .await
    {
        tracing::warn!(error = %close_error, run = %run_id, "failed to close trigger request");
    }
}

/// Report the durable lifecycle of one accepted trigger. Reads are an
/// operator capability: the trigger credential submits work and must not be
/// able to observe other triggers (see
/// [`require_trigger_request_read_auth`]).
async fn get_trigger_request(
    Extension(trigger_state): Extension<Arc<TriggerState>>,
    Path(trigger_id): Path<String>,
) -> impl IntoResponse {
    let Ok(trigger_id) = uuid::Uuid::parse_str(&trigger_id) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "invalid_trigger_id",
                "message": "trigger_id must be a UUID"
            })),
        );
    };
    let Some(pool) = trigger_state.db.as_ref() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"error": "trigger_store_unavailable"})),
        );
    };
    match graded_trigger_request(pool, &trigger_id).await {
        Ok(Some(request)) => (StatusCode::OK, Json(request.to_response())),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "trigger_request_not_found"})),
        ),
        Err(error) => {
            tracing::error!(%error, trigger = %trigger_id, "failed to load trigger request");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "trigger_request_lookup_failed"})),
            )
        }
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
            anyhow::anyhow!("{config_path} at {commit_hash} is not valid UTF-8: {error}")
        })?;
        return PipelineDefinition::parse(&yaml)
            .map(Some)
            .map_err(|error| anyhow::anyhow!("invalid {config_path} at {commit_hash}: {error}"));
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
/// is starting. Each run finalized here also closes its linked trigger
/// request with the same verdict, so the durable lifecycle never lags the
/// run row it was graded from.
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
        if is_terminal_run_status(run.status.as_str()) {
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
        let (status, cause) = if jobs.is_empty() {
            // Zero durable rows is the signature of an enqueue that has not
            // happened yet, not proof it never will — see the enqueue
            // horizon above. Cancelling inside that window killed a live
            // run (2026-09-23, run 556dd836: graded cancelled at minute 12,
            // head job enqueued at minute 29).
            if Utc::now() - run.created_at
                >= chrono::Duration::seconds(RECONCILE_EMPTY_RUN_HORIZON_SECS)
            {
                (
                    "cancelled",
                    Some("orphaned run finalized cancelled: no job was ever enqueued".to_string()),
                )
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
            let mut failed: Vec<String> = jobs
                .iter()
                .filter(|job| job.status == "failed" || job.status == "timed_out")
                .map(|job| format!("{} ({})", job.name, job.status))
                .collect();
            failed.sort();
            (
                "failed",
                Some(format!(
                    "orphaned run finalized failed: {}",
                    failed.join(", ")
                )),
            )
        } else if jobs.iter().any(|job| job.status == "cancelled") {
            let mut cancelled: Vec<String> = jobs
                .iter()
                .filter(|job| job.status == "cancelled")
                .map(|job| format!("{} ({})", job.name, job.status))
                .collect();
            cancelled.sort();
            (
                "cancelled",
                Some(format!(
                    "orphaned run finalized cancelled: {}",
                    cancelled.join(", ")
                )),
            )
        } else if incomplete_chain {
            // Every enqueued job succeeded, but the definition expects more
            // jobs than were ever enqueued: the chain stopped advancing when
            // its engine was lost, and the unenqueued remainder will never
            // run.
            let enqueued: HashSet<&str> = jobs.iter().map(|job| job.name.as_str()).collect();
            let mut missing: Vec<&str> = definition
                .as_ref()
                .map(|definition| {
                    definition
                        .jobs
                        .iter()
                        .map(|job| job.name.as_str())
                        .filter(|name| !enqueued.contains(name))
                        .collect()
                })
                .unwrap_or_default();
            missing.sort();
            (
                "failed",
                Some(format!(
                    "orphaned run finalized failed: jobs never enqueued: {}",
                    missing.join(", ")
                )),
            )
        } else {
            ("succeeded", None)
        };
        if gitforge_db::queries::PipelineRunQueries::update_status(pool, run.id, status)
            .await
            .is_ok()
        {
            tracing::info!(run = %run.id, status, incomplete_chain, "finalized orphaned run");
            // The run's terminal row is durable, so the trigger request
            // linked to it must close with the same verdict instead of
            // sitting `processing` until a graded read heals it. `None`
            // only for a succeeded run; every other verdict carries the
            // specific cause graded above. Best effort, like every
            // trigger-bookkeeping write: a failed close still heals at read
            // time through `grade_trigger_status`.
            complete_trigger_request_for_run(pool, run.id, cause.as_deref()).await;
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
        let stale = fail_stale_trigger_requests(&pool).await;
        if stale > 0 {
            tracing::info!(stale, "periodic stale trigger-request sweep complete");
        }
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
        let is_terminal = run.is_some_and(|run| is_terminal_run_status(run.status.as_str()));
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
        let mut repair = tokio::process::Command::new("git");
        repair
            .arg("-C")
            .arg(&workspace)
            .args(["checkout", "--force", "--detach", commit_hash]);
        let repair = run_git_output(repair, "adopt checkout", workspace_prep_timeout()).await?;
        if !repair.status.success() {
            return Err(anyhow::anyhow!(
                "workspace for run {run_id} exists but could not be adopted at commit {commit_hash}: {}",
                String::from_utf8_lossy(&repair.stderr).trim()
            ));
        }
        let mut clean = tokio::process::Command::new("git");
        clean.arg("-C").arg(&workspace).args(["clean", "-fdx"]);
        let clean = run_git_output(clean, "adopt clean", workspace_prep_timeout()).await?;
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
    let mut clone = tokio::process::Command::new("git");
    // --no-local: avoid hard links and local-object alternates for
    // portability across protected or differently-owned storage.
    clone
        .args(["clone", "--no-local", "--no-checkout"])
        .arg(&source)
        .arg(&workspace);
    let clone = run_git_output(clone, "clone", workspace_prep_timeout()).await?;
    if !clone.status.success() {
        return Err(anyhow::anyhow!(
            "checkout clone failed for run {}: {}",
            run_id,
            String::from_utf8_lossy(&clone.stderr).trim()
        ));
    }

    let mut checkout = tokio::process::Command::new("git");
    checkout
        .arg("-C")
        .arg(&workspace)
        .args(["checkout", "--detach", commit_hash]);
    let checkout = run_git_output(checkout, "checkout", workspace_prep_timeout()).await?;
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

/// Wall-clock budget for one workspace-prep git command.
///
/// Workspace prep runs inside the push handler, so one hung child wedged the
/// whole corridor: a `git clone --no-local` that never returned left the run
/// row `running` with zero jobs forever (observed 2026-10-06, run 7a612e78:
/// clone at 12m36s etime, empty workspace, nothing reaped it). The default
/// sits far above the slowest observed legitimate clone — the reconciler's
/// own grace window documents multi-gigabyte clones at minutes — and the
/// override exists for hosts that legitimately need longer.
fn workspace_prep_timeout() -> Duration {
    parse_workspace_prep_timeout(std::env::var("GITFORGE_WORKSPACE_PREP_TIMEOUT_SECS").ok())
}

/// Parse the workspace-prep budget override; any unusable value (unset,
/// non-numeric, zero) falls back to the default. Split from
/// [`workspace_prep_timeout`] so the parsing contract is testable without
/// racing other tests over the process environment.
fn parse_workspace_prep_timeout(value: Option<String>) -> Duration {
    let seconds = value
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .filter(|&seconds| seconds > 0);
    Duration::from_secs(seconds.unwrap_or(300))
}

/// Run one workspace-prep git command under the prep budget.
///
/// `kill_on_drop` is the part that actually bounds the damage: on timeout the
/// `output()` future is dropped, and without the kill flag the child would
/// outlive its future and keep cloning — the exact unbounded state this
/// helper exists to prevent.
async fn run_git_output(
    mut command: tokio::process::Command,
    step: &str,
    budget: Duration,
) -> anyhow::Result<std::process::Output> {
    command.kill_on_drop(true);
    match timeout(budget, command.output()).await {
        Ok(output) => output.map_err(|error| {
            anyhow::anyhow!("workspace prep step '{step}' failed to spawn: {error}")
        }),
        Err(_) => Err(anyhow::anyhow!(
            "workspace prep step '{step}' exceeded its {}s budget and was killed",
            budget.as_secs()
        )),
    }
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
    consumer_health: Arc<ConsumerHealth>,
    shutdown: Arc<AtomicBool>,
) -> anyhow::Result<()> {
    tracing::info!("starting event consumer loop");

    // Subscribe to push events
    let filter = EventFilter::for_types(vec![EventType::PushReceived]);
    let mut stream = event_bus.subscribe(filter).await?;
    // The subscription is live from here, so everything after this point —
    // including this function's own exits — must keep the health flag
    // honest: hold the guard so any exit marks the flag down immediately
    // instead of leaving a dead attempt advertised healthy until the
    // supervisor observes it.
    let _subscription_guard = ConsumerSubscriptionGuard {
        health: consumer_health.clone(),
    };

    // The previous attempt may have died holding a durable claim whose event
    // died with it. Close those out before anything else: at this instant
    // this attempt is the process's only consumer and has claimed nothing,
    // so every `claimed` row in the store belongs to a dead attempt, and the
    // sweep only fails claims that provably have no run behind them (see
    // [`fail_trigger_claims_lost_without_run`]). A failure propagates: an
    // unavailable recovery store proved nothing, so this attempt exits
    // before raising acceptance and the supervisor retries with backoff.
    if let Some(pool) = scheduler_db.as_ref() {
        let lost = fail_trigger_claims_lost_without_run(pool).await?;
        if lost > 0 {
            tracing::info!(
                lost,
                "consumer attempt closed trigger claims left by a dead attempt"
            );
        }
    }

    // The subscription is live: an event published from here on has a
    // receiver, so trigger acceptance may open. This is the only raise, and
    // it happens strictly after `subscribe` succeeded — never on spawn.
    consumer_health.mark_running();

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
                        // Claim the durable row before planning. From here the
                        // live consumer owns it: the sweep no longer considers
                        // it lost, and a repeat trigger deduplicates into it no
                        // matter how long planning takes. Both failure outcomes
                        // refuse to plan, because each push must be built at
                        // most once: a claim that matched nothing means the
                        // sweep already resolved the row as lost while this
                        // event sat queued — the caller was given a retryable
                        // failure and the retry owns the rebuild — and a claim
                        // write that errored leaves ownership unproven, which
                        // is no basis for a run.
                        match claim_trigger_request(scheduler_db.as_ref(), event.event_id).await {
                            TriggerClaim::Superseded => {
                                run_waiters
                                    .lock()
                                    .expect("run waiter lock poisoned")
                                    .remove(&event.event_id);
                                tracing::info!(
                                    event = %event.event_id,
                                    "dropping event whose trigger request was already failed as lost"
                                );
                            }
                            TriggerClaim::Indeterminate => {
                                run_waiters
                                    .lock()
                                    .expect("run waiter lock poisoned")
                                    .remove(&event.event_id);
                                tracing::warn!(
                                    event = %event.event_id,
                                    "dropping event whose trigger claim could not be written; \
                                     refusing to plan without a durable claim"
                                );
                            }
                            TriggerClaim::Claimed => {
                                match handle_push_event(
                                    &event,
                                    &scheduler,
                                    &pipeline_cache,
                                    scheduler_db.as_ref(),
                                    &workspace_paths,
                                    &run_workspace_paths,
                                    &pipeline_registry,
                                )
                                .await
                                {
                                    Ok(run_id) => {
                                        if let Some(waiter) = run_waiters
                                            .lock()
                                            .expect("run waiter lock poisoned")
                                            .remove(&event.event_id)
                                        {
                                            let _ = waiter.send(run_id);
                                        }
                                    }
                                    Err(e) => {
                                        run_waiters
                                            .lock()
                                            .expect("run waiter lock poisoned")
                                            .remove(&event.event_id);
                                        tracing::error!("failed to handle push event: {}", e);
                                        // The trigger the caller submitted can never
                                        // produce a run now, so close it with the
                                        // cause instead of leaving it open forever.
                                        if let Some(pool) = scheduler_db.as_ref() {
                                            fail_trigger_request_for_event(pool, event.event_id, &e)
                                                .await;
                                        }
                                    }
                                }
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

/// Supervise the process's trigger-event consumer for its whole lifetime.
///
/// The consumer turns accepted triggers into planned runs, so its exit must
/// be neither permanent nor silent: this wrapper restarts
/// [`run_event_consumer`] after a backoff that doubles from
/// [`CONSUMER_RESTART_MIN_BACKOFF`] to [`CONSUMER_RESTART_MAX_BACKOFF`],
/// holds `consumer_health` down across every restart window so
/// `/pipelines/trigger` fails closed while no consumer exists, and returns
/// only when shutdown is requested. The consumer is spawned as a separate
/// task precisely so a panic inside it is observed as a `JoinError` here
/// instead of taking the supervisor — and with it the recovery — down too.
#[allow(clippy::too_many_arguments)]
async fn supervise_event_consumer(
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
    consumer_health: Arc<ConsumerHealth>,
    shutdown: Arc<AtomicBool>,
) {
    supervise_consumer(consumer_health.clone(), shutdown, move || {
        run_event_consumer(
            event_bus.clone(),
            scheduler.clone(),
            pipeline_cache.clone(),
            scheduler_db.clone(),
            workspace_paths.clone(),
            run_workspace_paths.clone(),
            pipeline_registry.clone(),
            run_waiters.clone(),
            consumer_health.clone(),
            shutdown.clone(),
        )
    })
    .await;
}

/// The supervision loop itself, parameterized over the worker factory so
/// tests exercise the exact failure handling production gets. Each call to
/// `spawn_worker` starts one consumer attempt; awaiting its handle observes
/// error returns and panics alike. The supervisor never raises
/// `consumer_health` itself: the worker raises it only once its bus
/// subscription is live, so acceptance stays closed across the boot window
/// and every restart gap, not merely while the backoff runs. Events
/// published into a gap between attempts are not lost: the durable
/// trigger-request rows they left `pending` are closed out by
/// `fail_stale_trigger_requests` once they age past the correlation window,
/// and claims the dead attempt already took are closed by the successor's
/// startup sweep in [`run_event_consumer`].
async fn supervise_consumer<F, S>(
    consumer_health: Arc<ConsumerHealth>,
    shutdown: Arc<AtomicBool>,
    spawn_worker: S,
) where
    F: std::future::Future<Output = anyhow::Result<()>> + Send + 'static,
    S: Fn() -> F + Send + 'static,
{
    let mut backoff = CONSUMER_RESTART_MIN_BACKOFF;
    loop {
        if shutdown.load(Ordering::SeqCst) {
            break;
        }
        let started = std::time::Instant::now();
        match tokio::spawn(spawn_worker()).await {
            Ok(Ok(())) if shutdown.load(Ordering::SeqCst) => {
                tracing::info!("event consumer exited for shutdown");
                break;
            }
            // The bus outlives the consumer, so a clean return without a
            // shutdown signal means the loop ended for a reason that is not
            // shutdown (its own stream closing, a future refactor) — treat
            // delivery as broken and restart.
            Ok(Ok(())) => {
                tracing::warn!("event consumer stopped without a shutdown signal; restarting");
            }
            Ok(Err(error)) => {
                tracing::error!(%error, "event consumer failed; restarting");
            }
            Err(join_error) if join_error.is_panic() => {
                tracing::error!(%join_error, "event consumer panicked; restarting");
            }
            Err(join_error) => {
                tracing::error!(%join_error, "event consumer task aborted; restarting");
            }
        }
        if shutdown.load(Ordering::SeqCst) {
            break;
        }
        // Down for the whole backoff: triggers submitted during the gap get
        // the endpoint's explicit 503 rather than a silently lost event.
        consumer_health.mark_down();
        // An attempt that survived a full max backoff was genuinely healthy,
        // so the next failure starts the climb from the minimum again.
        if started.elapsed() >= CONSUMER_RESTART_MAX_BACKOFF {
            backoff = CONSUMER_RESTART_MIN_BACKOFF;
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(CONSUMER_RESTART_MAX_BACKOFF);
    }
    // Shutting down is still "no live consumer": refuse further work during
    // the drain window instead of accepting what cannot be delivered.
    consumer_health.mark_down();
    tracing::info!("event consumer supervision stopped");
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
        // Unreachable through the trigger endpoint, the bus's only publisher,
        // but the consumer claimed this event's row before handling: a row
        // stranded claimed with neither run nor verdict would wedge dedup for
        // its push forever.
        if let Some(pool) = scheduler_db {
            fail_trigger_request_for_event(pool, event.event_id, "event carried no push payload")
                .await;
        }
        return Ok(gitforge_common::PipelineRunId::new());
    };

    // Belt-and-suspenders for the git-server's deletion filter (F37): an
    // all-zero new hash means the ref no longer exists — there is no commit
    // to check out, and building one used to produce a run that failed
    // immediately at checkout. Branch creation (all-zero OLD hash) carries
    // a real new hash and proceeds below. The trigger endpoint rejects the
    // zero hash before publishing, so this path has no row to resolve in
    // production; the close is defense against any future publisher.
    if gitforge_common::is_zero_hash(&payload.new_hash) {
        tracing::info!(
            repo = %payload.repo_id,
            ref_name = %payload.ref_name,
            "ignoring ref-deletion push: nothing to build"
        );
        if let Some(pool) = scheduler_db {
            fail_trigger_request_for_event(
                pool,
                event.event_id,
                "ref-deletion push: nothing to build",
            )
            .await;
        }
        return Ok(gitforge_common::PipelineRunId::new());
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
        // Reserve this run id on the claimed request row before any run-side
        // write exists. The reservation is the correlation seam the recovery
        // sweep leans on: a claimed row either provably has no run (nothing
        // was planned, so failing it cannot duplicate a build) or names a run
        // id the sweep must respect. A reservation that does not land exactly
        // once is a refusal to plan, same rule as the claim itself.
        if !reserve_trigger_request_run(pool, event.event_id, state.run_id).await {
            return Err(anyhow::anyhow!(
                "trigger correlation could not reserve run {} before planning; \
                 refusing to build without it",
                state.run_id
            ));
        }
        let db_pipeline = DbPipeline {
            id: pipeline_id,
            repo_id,
            name: pipeline.name.clone(),
            trigger_type: "push".to_string(),
            config: serde_json::to_value(&pipeline)?,
            created_at: Utc::now(),
        };
        // Only one active pipeline version per (repo, name) is allowed by
        // idx_pipelines_active_repo_name — retire the predecessor before
        // recording this push's version, or every push after the first
        // fails run creation with a constraint violation.
        gitforge_db::queries::PipelineQueries::deactivate_active(pool, repo_id, &pipeline.name)
            .await?;
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

        // The run row is durable, so the trigger request links to it now —
        // before the workspace clone and job planning below, which can take
        // minutes. The run id was already reserved on the claimed row above,
        // so this link only lifts the request out of `claimed` into
        // `processing`; a claimed request already absorbs repeat triggers at
        // any age, and `processing` keeps it open for the run's whole life.
        // Best effort: correlation bookkeeping must never fail the run it
        // describes — if this write is lost, the reserved pair still resolves
        // through read-time grading and the run's own verdict.
        mark_trigger_request_processing(pool, event.event_id, state.run_id).await;
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
                    // The cause must survive the process: a failed run with
                    // zero job rows is otherwise indistinguishable from a
                    // silent planning crash once the log has rotated.
                    fail_run(pool, state.run_id, &error).await;
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
            persist_planned_jobs(pool, &engine, state.run_id, workspace_path.as_deref()).await
        {
            // Without the planned rows a restart cannot resume this run; a
            // half-planned run must not be left non-terminal.
            tracing::error!(run = %state.run_id, %error, "failed to persist planned jobs");
            fail_run(pool, state.run_id, &error).await;
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
        &engine,
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
/// status, sweep any never-dispatched job rows left behind by a non-success
/// verdict, free the run's workspace, and evict the engine from the registry.
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
        if terminal_status != "succeeded" {
            sweep_unclaimed_jobs(pool, state.run_id).await;
        }
        // Close the trigger request that started this run with the run's own
        // verdict, so a caller polling its `trigger_id` sees the same truth
        // the durable run row holds.
        let cause = if terminal_status == "succeeded" {
            None
        } else {
            match gitforge_db::queries::PipelineRunQueries::get(pool, state.run_id).await {
                Ok(Some(run)) => Some(run.error.unwrap_or_else(|| terminal_status.to_string())),
                _ => Some(terminal_status.to_string()),
            }
        };
        complete_trigger_request_for_run(pool, state.run_id, cause.as_deref()).await;
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

/// Record a run's failure cause durably, then sweep any job rows the run
/// left unclaimed.
///
/// Both halves answer the same defect from opposite sides: the reason makes
/// the run explain itself in the API (a zero-job failed run's cause
/// otherwise lives only in the process log), and the sweep keeps the run's
/// `pending`/`queued` rows from outliving it — once the run row is terminal
/// the orphan reconciler skips it forever, so an unswept row can never reach
/// a terminal state on its own (observed 2026-10-06, run 84e2ab37: `fmt`
/// succeeded, `clippy` failed, `test` and `coverage` pending for hours after
/// the run was graded `failed`).
async fn fail_run(
    pool: &gitforge_db::Pool,
    run_id: gitforge_common::PipelineRunId,
    error: &anyhow::Error,
) {
    let reason = format!("{error:#}");
    if let Err(status_error) = gitforge_db::queries::PipelineRunQueries::update_status_with_error(
        pool,
        run_id,
        "failed",
        Some(&reason),
    )
    .await
    {
        tracing::error!(%status_error, run = %run_id, "failed to persist run failure reason");
    }
    sweep_unclaimed_jobs(pool, run_id).await;
    complete_trigger_request_for_run(pool, run_id, Some(&reason)).await;
}

/// Cancel the run's never-dispatched (`pending`/`queued`) job rows. Best
/// effort: a failed sweep logs and moves on, leaving the rows legible to a
/// future operator pass rather than hiding behind a swallowed error.
async fn sweep_unclaimed_jobs(pool: &gitforge_db::Pool, run_id: gitforge_common::PipelineRunId) {
    match gitforge_db::queries::JobQueries::cancel_unclaimed_for_run(pool, run_id).await {
        Ok(0) => {}
        Ok(count) => {
            tracing::info!(
                run = %run_id,
                count,
                "cancelled never-dispatched job rows on a terminal run"
            );
        }
        Err(error) => {
            tracing::error!(
                %error,
                run = %run_id,
                "failed to sweep never-dispatched job rows"
            );
        }
    }
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
        if is_terminal_run_status(run.status.as_str()) {
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

    #[test]
    fn trigger_request_reads_resolve_operator_then_shared_credential() {
        let token = configured_trigger_request_read_token(|name| match name {
            "GITFORGE_SCHEDULER_OPERATOR_TOKEN" => Some("operator".to_string()),
            "GITFORGE_SCHEDULER_TOKEN" => Some("shared".to_string()),
            _ => None,
        });
        assert_eq!(token.as_deref(), Some("operator"));

        let shared = configured_trigger_request_read_token(|name| {
            (name == "GITFORGE_SCHEDULER_TOKEN").then(|| "shared".to_string())
        });
        assert_eq!(shared.as_deref(), Some("shared"));

        let unset = configured_trigger_request_read_token(|name| match name {
            "GITFORGE_SCHEDULER_OPERATOR_TOKEN" => Some(String::new()),
            "GITFORGE_SCHEDULER_TOKEN" => Some(String::new()),
            _ => None,
        });
        assert_eq!(unset, None, "empty credentials are never accepted");
    }

    /// Reads and writes stay disjoint: a deployment that only configures the
    /// trigger credential cannot read the lifecycle, and one that only
    /// configures the operator credential cannot submit work.
    #[test]
    fn trigger_submit_and_read_credentials_are_disjoint() {
        let read = configured_trigger_request_read_token(|name| {
            (name == "GITFORGE_TRIGGER_TOKEN").then(|| "trigger-only".to_string())
        });
        assert_eq!(read, None);

        let write = configured_trigger_token(|name| match name {
            "GITFORGE_SCHEDULER_OPERATOR_TOKEN" => Some("operator-only".to_string()),
            "GITFORGE_SCHEDULER_TOKEN" => Some("shared-only".to_string()),
            _ => None,
        });
        assert_eq!(write, None, "read-only scheduler credentials cannot submit");

        let write = configured_trigger_token(|name| {
            (name == "GITFORGE_CI_TRIGGER_TOKEN").then(|| "trigger-only".to_string())
        });
        assert_eq!(write.as_deref(), Some("trigger-only"));

        assert_ne!(
            configured_trigger_request_read_token(|name| match name {
                "GITFORGE_TRIGGER_TOKEN" => Some("trigger".to_string()),
                "GITFORGE_SCHEDULER_OPERATOR_TOKEN" => Some("operator".to_string()),
                _ => None,
            })
            .as_deref(),
            Some("trigger"),
            "the trigger credential must not be the read credential",
        );
    }

    /// One secret configured for both roles must collide, while distinct or
    /// partially configured roles keep their documented behavior — a missing
    /// role is a supported deployment, not a misconfiguration.
    #[test]
    fn identical_submit_and_read_credentials_collide_only_when_both_are_set() {
        assert!(trigger_credentials_collide(
            Some("same-secret"),
            Some("same-secret")
        ));
        assert!(!trigger_credentials_collide(
            Some("submit-secret"),
            Some("read-secret")
        ));
        assert!(!trigger_credentials_collide(Some("submit-secret"), None));
        assert!(!trigger_credentials_collide(None, Some("read-secret")));
        assert!(!trigger_credentials_collide(None, None));
    }

    #[test]
    fn grade_trigger_status_follows_the_linked_run() {
        // An open request with no run yet stays pending.
        assert_eq!(
            grade_trigger_status(TRIGGER_REQUEST_PENDING, None, None),
            ("pending".to_string(), None)
        );
        // A run that is planning or executing means processing.
        for run_status in ["pending", "queued", "running"] {
            assert_eq!(
                grade_trigger_status(TRIGGER_REQUEST_PENDING, None, Some((run_status, None))),
                ("processing".to_string(), None),
                "run status {run_status} must read as processing"
            );
        }
        // Success completes the request without a cause.
        assert_eq!(
            grade_trigger_status(TRIGGER_REQUEST_PENDING, None, Some(("succeeded", None))),
            ("completed".to_string(), None)
        );
        // Any terminal non-success verdict fails it and carries the cause,
        // including the timeout spellings the run rows use.
        for run_status in ["failed", "cancelled", "timed_out", "timeout", "timed-out"] {
            assert_eq!(
                grade_trigger_status(TRIGGER_REQUEST_PENDING, None, Some((run_status, None))),
                ("failed".to_string(), Some(run_status.to_string())),
                "run status {run_status} must fail the request"
            );
        }
        assert_eq!(
            grade_trigger_status(
                TRIGGER_REQUEST_PENDING,
                None,
                Some(("failed", Some("clone rejected ref")))
            ),
            ("failed".to_string(), Some("clone rejected ref".to_string()))
        );
    }

    #[test]
    fn grade_trigger_status_keeps_a_terminal_request_and_unknown_run_statuses() {
        // A stored verdict is final: a late run row cannot rewrite it.
        assert_eq!(
            grade_trigger_status(
                TRIGGER_REQUEST_COMPLETED,
                None,
                Some(("failed", Some("too late")))
            ),
            ("completed".to_string(), None)
        );
        assert_eq!(
            grade_trigger_status(
                TRIGGER_REQUEST_FAILED,
                Some("planning failed"),
                Some(("succeeded", None))
            ),
            ("failed".to_string(), Some("planning failed".to_string()))
        );
        // An unrecognised run status degrades to the stored lifecycle rather
        // than inventing a state the caller never saw.
        assert_eq!(
            grade_trigger_status(TRIGGER_REQUEST_PENDING, None, Some(("migrated", None))),
            ("pending".to_string(), None)
        );
    }

    async fn trigger_request_pool() -> (
        gitforge_db::Pool,
        gitforge_common::RepoId,
        gitforge_common::PipelineId,
    ) {
        let (pool, repo_id, pipeline_id) = sweep_test_pool().await;
        (pool, repo_id, pipeline_id)
    }

    /// Record a request under a generated event id, the way the handler does.
    async fn record_request(
        pool: &gitforge_db::Pool,
        repo_id: gitforge_common::RepoId,
        ref_name: &str,
        new_hash: &str,
    ) -> anyhow::Result<TriggerRequestRecord> {
        record_trigger_request(pool, repo_id, ref_name, new_hash, uuid::Uuid::new_v4()).await
    }

    /// Link a run to the request a trigger recorded, the way the consumer
    /// does: the row's own event id is the only one allowed to link it, and
    /// the run id is reserved on the claimed row before the link, exactly as
    /// `handle_push_event` does before the run row is created.
    async fn link_request_run(
        pool: &gitforge_db::Pool,
        trigger_id: &uuid::Uuid,
        run_id: gitforge_common::PipelineRunId,
    ) {
        let event = first_trigger_event(pool, trigger_id).await;
        assert_eq!(
            claim_trigger_request(Some(pool), event).await,
            TriggerClaim::Claimed,
            "a run can link only after its event is durably claimed"
        );
        assert!(
            reserve_trigger_request_run(pool, event, run_id).await,
            "a live claim accepts its own run reservation"
        );
        mark_trigger_request_processing(pool, event, run_id).await;
    }

    #[tokio::test]
    async fn trigger_request_store_records_and_dedupes_open_requests() {
        let (pool, repo_id, pipeline_id) = trigger_request_pool().await;
        let ref_name = "refs/heads/main";
        let new_hash = "a".repeat(40);

        let first = record_request(&pool, repo_id, ref_name, &new_hash)
            .await
            .unwrap();
        assert!(!first.deduplicated);
        assert_eq!(
            load_trigger_request(&pool, &first.trigger_id)
                .await
                .unwrap()
                .expect("recorded request")
                .status,
            TRIGGER_REQUEST_PENDING
        );

        // A different push is never deduplicated.
        let other = record_request(&pool, repo_id, ref_name, &"b".repeat(40))
            .await
            .unwrap();
        assert!(!other.deduplicated);
        assert_ne!(other.trigger_id, first.trigger_id);

        // While the request is open, a repeat trigger for the same push
        // returns the same id instead of planning a second run.
        let repeat = record_request(&pool, repo_id, ref_name, &new_hash)
            .await
            .unwrap();
        assert!(repeat.deduplicated);
        assert_eq!(repeat.trigger_id, first.trigger_id);

        // Linking the planned run keeps the request open but resolvable, and
        // only the event that recorded the request may link it.
        let run_id = seed_run(&pool, repo_id, pipeline_id, "running").await;
        mark_trigger_request_processing(&pool, uuid::Uuid::new_v4(), run_id).await;
        let untouched = load_trigger_request(&pool, &first.trigger_id)
            .await
            .unwrap()
            .expect("recorded request");
        assert_eq!(
            untouched.status, TRIGGER_REQUEST_PENDING,
            "an unrelated event must not link a run"
        );
        assert_eq!(untouched.pipeline_run_id, None);
        link_request_run(&pool, &first.trigger_id, run_id).await;
        let graded = graded_trigger_request(&pool, &first.trigger_id)
            .await
            .unwrap()
            .expect("graded request");
        assert_eq!(graded.status, TRIGGER_REQUEST_PROCESSING);
        assert_eq!(
            graded.pipeline_run_id.as_deref(),
            Some(run_id.to_string().as_str())
        );

        // Still open: the repeat trigger keeps collapsing into the same id.
        let repeat = record_request(&pool, repo_id, ref_name, &new_hash)
            .await
            .unwrap();
        assert!(repeat.deduplicated);
        assert_eq!(repeat.trigger_id, first.trigger_id);
    }

    /// The event id a trigger request row was recorded under.
    async fn first_trigger_event(pool: &gitforge_db::Pool, trigger_id: &uuid::Uuid) -> uuid::Uuid {
        use sqlx::Row;
        let event_id: String = sqlx::query("SELECT event_id FROM ci_trigger_requests WHERE id = ?")
            .bind(trigger_id.to_string())
            .fetch_one(pool.pool())
            .await
            .unwrap()
            .try_get("event_id")
            .unwrap();
        uuid::Uuid::parse_str(&event_id).unwrap()
    }

    /// A completed request is closed: the same push becomes a new request,
    /// which is why status must be polled through `trigger_id` rather than by
    /// re-submitting the trigger.
    #[tokio::test]
    async fn trigger_request_store_completed_creates_a_fresh_request() {
        let (pool, repo_id, pipeline_id) = trigger_request_pool().await;
        let ref_name = "refs/heads/feature";
        let new_hash = "c".repeat(40);

        let first = record_request(&pool, repo_id, ref_name, &new_hash)
            .await
            .unwrap();
        let run_id = seed_run(&pool, repo_id, pipeline_id, "running").await;
        link_request_run(&pool, &first.trigger_id, run_id).await;
        complete_trigger_request_for_run(&pool, run_id, None).await;

        let graded = graded_trigger_request(&pool, &first.trigger_id)
            .await
            .unwrap()
            .expect("graded request");
        assert_eq!(graded.status, TRIGGER_REQUEST_COMPLETED);
        assert_eq!(graded.error, None);

        let repeat = record_request(&pool, repo_id, ref_name, &new_hash)
            .await
            .unwrap();
        assert!(!repeat.deduplicated, "a completed request is closed");
        assert_ne!(repeat.trigger_id, first.trigger_id);
    }

    /// A run that failed closes its request with the run's cause, and the
    /// failure also frees the push for a fresh request.
    #[tokio::test]
    async fn trigger_request_store_failure_carries_the_run_cause() {
        let (pool, repo_id, pipeline_id) = trigger_request_pool().await;
        let ref_name = "refs/heads/broken";
        let new_hash = "d".repeat(40);

        let first = record_request(&pool, repo_id, ref_name, &new_hash)
            .await
            .unwrap();
        let run_id = seed_run(&pool, repo_id, pipeline_id, "running").await;
        link_request_run(&pool, &first.trigger_id, run_id).await;
        complete_trigger_request_for_run(&pool, run_id, Some("checkout failed")).await;

        let graded = graded_trigger_request(&pool, &first.trigger_id)
            .await
            .unwrap()
            .expect("graded request");
        assert_eq!(graded.status, TRIGGER_REQUEST_FAILED);
        assert_eq!(graded.error.as_deref(), Some("checkout failed"));

        let repeat = record_request(&pool, repo_id, ref_name, &new_hash)
            .await
            .unwrap();
        assert!(!repeat.deduplicated);
    }

    /// A trigger whose consumer failed before planning a run is closed with
    /// the failure cause by the event that recorded it.
    #[tokio::test]
    async fn trigger_request_store_event_failure_closes_the_request() {
        let (pool, repo_id, _pipeline_id) = trigger_request_pool().await;
        let ref_name = "refs/heads/unplannable";
        let new_hash = "e".repeat(40);

        let event_id = uuid::Uuid::new_v4();
        let first = record_trigger_request(&pool, repo_id, ref_name, &new_hash, event_id)
            .await
            .unwrap();
        assert!(!first.deduplicated);

        fail_trigger_request_for_event(&pool, event_id, &anyhow::anyhow!("invalid pipeline")).await;
        let graded = graded_trigger_request(&pool, &first.trigger_id)
            .await
            .unwrap()
            .expect("graded request");
        assert_eq!(graded.status, TRIGGER_REQUEST_FAILED);
        assert_eq!(graded.error.as_deref(), Some("invalid pipeline"));

        // Closing again through the run path cannot resurrect or rewrite it.
        complete_trigger_request_for_run(&pool, gitforge_common::PipelineRunId::new(), None).await;
        let graded = graded_trigger_request(&pool, &first.trigger_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(graded.status, TRIGGER_REQUEST_FAILED);

        let repeat = record_request(&pool, repo_id, ref_name, &new_hash)
            .await
            .unwrap();
        assert!(!repeat.deduplicated);
    }

    /// A `pending` request older than the correlation window lost its
    /// in-memory event, so it must not absorb a re-submission forever.
    #[tokio::test]
    async fn trigger_request_store_expired_pending_does_not_absorb_a_repeat() {
        let (pool, repo_id, _pipeline_id) = trigger_request_pool().await;
        let ref_name = "refs/heads/stale";
        let new_hash = "f".repeat(40);

        let first_event_id = uuid::Uuid::new_v4();
        let first = record_trigger_request(&pool, repo_id, ref_name, &new_hash, first_event_id)
            .await
            .unwrap();
        let stale = Utc::now()
            - chrono::Duration::seconds(
                i64::try_from(gitforge_common::CI_TRIGGER_CORRELATION_WINDOW.as_secs()).unwrap(),
            )
            - chrono::Duration::seconds(1);
        sqlx::query("UPDATE ci_trigger_requests SET created_at = ? WHERE id = ?")
            .bind(stale.to_rfc3339())
            .bind(first.trigger_id.to_string())
            .execute(pool.pool())
            .await
            .unwrap();

        let repeat_event_id = uuid::Uuid::new_v4();
        let repeat = record_trigger_request(&pool, repo_id, ref_name, &new_hash, repeat_event_id)
            .await
            .unwrap();
        assert!(!repeat.deduplicated, "an expired pending row is closed");
        assert_ne!(repeat.trigger_id, first.trigger_id);

        let old_row = graded_trigger_request(&pool, &first.trigger_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(old_row.status, TRIGGER_REQUEST_FAILED);
        assert_eq!(
            claim_trigger_request(Some(&pool), first_event_id).await,
            TriggerClaim::Superseded
        );

        assert_eq!(
            claim_trigger_request(Some(&pool), repeat_event_id).await,
            TriggerClaim::Claimed
        );
        let run_id = gitforge_common::PipelineRunId::new();
        assert!(reserve_trigger_request_run(&pool, repeat_event_id, run_id).await);
        mark_trigger_request_processing(&pool, repeat_event_id, run_id).await;
        let linked_row = graded_trigger_request(&pool, &repeat.trigger_id)
            .await
            .unwrap()
            .unwrap();
        let run_id_text = run_id.to_string();
        assert_eq!(linked_row.status, TRIGGER_REQUEST_PROCESSING);
        assert_eq!(
            linked_row.pipeline_run_id.as_deref(),
            Some(run_id_text.as_str())
        );
    }

    /// Regression for the delayed-link duplicate: preparation that outlives
    /// the correlation window must not un-deduplicate an open trigger. The
    /// consumer links the request the moment the run row is durable — before
    /// the workspace clone — so once the run exists the request is
    /// `processing`, which stays open for the run's whole life regardless of
    /// the window. The slow preparation itself is simulated deterministically
    /// by backdating the row past the window instead of sleeping through it.
    #[tokio::test]
    async fn trigger_request_stays_deduplicated_while_preparation_outlives_the_window() {
        let (pool, repo_id, pipeline_id) = trigger_request_pool().await;
        let ref_name = "refs/heads/slow-clone";
        let new_hash = "7".repeat(40);

        let event_id = uuid::Uuid::new_v4();
        let first = record_trigger_request(&pool, repo_id, ref_name, &new_hash, event_id)
            .await
            .unwrap();
        assert!(!first.deduplicated);

        // The clone grinds on and the correlation window elapses while the
        // request is still unlinked.
        let stale = Utc::now()
            - chrono::Duration::seconds(
                i64::try_from(gitforge_common::CI_TRIGGER_CORRELATION_WINDOW.as_secs()).unwrap(),
            )
            - chrono::Duration::seconds(1);
        sqlx::query("UPDATE ci_trigger_requests SET created_at = ? WHERE id = ?")
            .bind(stale.to_rfc3339())
            .bind(first.trigger_id.to_string())
            .execute(pool.pool())
            .await
            .unwrap();

        // The push handler's fixed ordering: the run id is reserved on the
        // claim, the run row goes durable, and the link lands immediately,
        // before any workspace preparation.
        assert_eq!(
            claim_trigger_request(Some(&pool), event_id).await,
            TriggerClaim::Claimed
        );
        let run_id = seed_run(&pool, repo_id, pipeline_id, "running").await;
        assert!(reserve_trigger_request_run(&pool, event_id, run_id).await);
        mark_trigger_request_processing(&pool, event_id, run_id).await;

        // Minutes into the clone, a repeat POST still collapses into the
        // same request instead of planning a second run for the same push.
        let repeat =
            record_trigger_request(&pool, repo_id, ref_name, &new_hash, uuid::Uuid::new_v4())
                .await
                .unwrap();
        assert!(
            repeat.deduplicated,
            "a linked request must absorb a repeat past the window"
        );
        assert_eq!(repeat.trigger_id, first.trigger_id);

        let graded = graded_trigger_request(&pool, &first.trigger_id)
            .await
            .unwrap()
            .expect("graded request");
        assert_eq!(graded.status, TRIGGER_REQUEST_PROCESSING);
        assert_eq!(
            graded.pipeline_run_id.as_deref(),
            Some(run_id.to_string().as_str()),
            "the expired-window link must still name the run"
        );
    }

    /// The stale sweep durably closes only what `open_trigger_request`
    /// already refuses to deduplicate into: an unlinked `pending` row past
    /// the correlation window. Fresh requests and requests whose run is
    /// linked — even an active one — are never marked failed by it.
    #[tokio::test]
    async fn stale_trigger_sweep_closes_only_unlinked_expired_pending_rows() {
        let (pool, repo_id, pipeline_id) = trigger_request_pool().await;
        let window = chrono::Duration::seconds(
            i64::try_from(gitforge_common::CI_TRIGGER_CORRELATION_WINDOW.as_secs()).unwrap(),
        );

        // Stranded: pending, never linked, past the window.
        let stranded_event = uuid::Uuid::new_v4();
        let stranded = record_trigger_request(
            &pool,
            repo_id,
            "refs/heads/stranded",
            &"8".repeat(40),
            stranded_event,
        )
        .await
        .unwrap();
        sqlx::query("UPDATE ci_trigger_requests SET created_at = ? WHERE id = ?")
            .bind((Utc::now() - window - window).to_rfc3339())
            .bind(stranded.trigger_id.to_string())
            .execute(pool.pool())
            .await
            .unwrap();

        // Fresh: pending, never linked, inside the window.
        let fresh = record_request(&pool, repo_id, "refs/heads/fresh", &"9".repeat(40))
            .await
            .unwrap();

        // Active: linked to a run that is still running, past the window.
        let active = record_request(&pool, repo_id, "refs/heads/active", &"a".repeat(41))
            .await
            .unwrap();
        let active_run = seed_run(&pool, repo_id, pipeline_id, "running").await;
        link_request_run(&pool, &active.trigger_id, active_run).await;
        sqlx::query("UPDATE ci_trigger_requests SET created_at = ? WHERE id = ?")
            .bind((Utc::now() - window - window).to_rfc3339())
            .bind(active.trigger_id.to_string())
            .execute(pool.pool())
            .await
            .unwrap();

        let closed = fail_stale_trigger_requests(&pool).await;
        assert_eq!(closed, 1, "only the stranded row may be closed");

        let stranded_row = load_trigger_request(&pool, &stranded.trigger_id)
            .await
            .unwrap()
            .expect("stranded request");
        assert_eq!(stranded_row.status, TRIGGER_REQUEST_FAILED);
        assert!(
            stranded_row.error.is_some(),
            "a swept request carries a cause"
        );

        let fresh_row = load_trigger_request(&pool, &fresh.trigger_id)
            .await
            .unwrap()
            .expect("fresh request");
        assert_eq!(fresh_row.status, TRIGGER_REQUEST_PENDING);

        let active_row = graded_trigger_request(&pool, &active.trigger_id)
            .await
            .unwrap()
            .expect("active request");
        assert_eq!(
            active_row.status, TRIGGER_REQUEST_PROCESSING,
            "an active linked run must not be failed by the sweep"
        );
    }

    /// The safe lifecycle around a sweep verdict, end to end. A request the
    /// stale sweep failed while its event sat queued is terminal: its late
    /// delivery is superseded at claim time, its row is never linked or
    /// reopened, and the same push does not deduplicate into the dead request
    /// — the caller's retry records a fresh request that follows the ordinary
    /// `pending → claimed → processing` path. A request a consumer closed
    /// with a cause is equally final. And a claim write that fails outright
    /// (injected here by closing the store) is indeterminate — never a
    /// durable claim — so no run is ever planned on a write that did not land.
    #[tokio::test]
    async fn swept_requests_stay_failed_and_planning_requires_a_durable_claim() {
        let (pool, repo_id, pipeline_id) = trigger_request_pool().await;
        let ref_name = "refs/heads/swept-then-retried";
        let new_hash = "b".repeat(41);
        let window = chrono::Duration::seconds(
            i64::try_from(gitforge_common::CI_TRIGGER_CORRELATION_WINDOW.as_secs()).unwrap(),
        );

        // Slow consumer: recorded, then the sweep fires before any claim
        // exists because the event sat queued past the correlation window.
        let slow_event = uuid::Uuid::new_v4();
        let slow = record_trigger_request(&pool, repo_id, ref_name, &new_hash, slow_event)
            .await
            .unwrap();
        sqlx::query("UPDATE ci_trigger_requests SET created_at = ? WHERE id = ?")
            .bind((Utc::now() - window - window).to_rfc3339())
            .bind(slow.trigger_id.to_string())
            .execute(pool.pool())
            .await
            .unwrap();
        assert_eq!(fail_stale_trigger_requests(&pool).await, 1);

        // The late delivery arrives after the sweep: the row is terminal, so
        // the claim matches nothing and the event must not plan a run.
        assert_eq!(
            claim_trigger_request(Some(&pool), slow_event).await,
            TriggerClaim::Superseded,
            "a late delivery of a swept request must not claim it"
        );

        // Terminal is terminal: a link for the late delivery has nowhere to
        // land, so no run can follow a request the sweep ruled lost.
        let late_run = seed_run(&pool, repo_id, pipeline_id, "running").await;
        mark_trigger_request_processing(&pool, slow_event, late_run).await;
        let swept_row = load_trigger_request(&pool, &slow.trigger_id)
            .await
            .unwrap()
            .expect("swept request");
        assert_eq!(
            swept_row.status, TRIGGER_REQUEST_FAILED,
            "a swept request must not be reopened by a link"
        );
        assert_eq!(
            swept_row.pipeline_run_id, None,
            "a swept request is never linked to the late run"
        );

        // The same push does not deduplicate into the dead request: the
        // caller was told the trigger failed, so the retry is a new build.
        let retry =
            record_trigger_request(&pool, repo_id, ref_name, &new_hash, uuid::Uuid::new_v4())
                .await
                .unwrap();
        assert!(!retry.deduplicated, "a failed request is closed");
        assert_ne!(retry.trigger_id, slow.trigger_id);

        // The retry follows the ordinary safe lifecycle: a durable claim
        // first, the run id reserved on the claim, then the link into
        // `processing` with the run it planned.
        let retry_event = first_trigger_event(&pool, &retry.trigger_id).await;
        assert_eq!(
            claim_trigger_request(Some(&pool), retry_event).await,
            TriggerClaim::Claimed
        );
        let retry_run = seed_run(&pool, repo_id, pipeline_id, "running").await;
        assert!(reserve_trigger_request_run(&pool, retry_event, retry_run).await);
        mark_trigger_request_processing(&pool, retry_event, retry_run).await;
        let retry_row = graded_trigger_request(&pool, &retry.trigger_id)
            .await
            .unwrap()
            .expect("retry request");
        assert_eq!(retry_row.status, TRIGGER_REQUEST_PROCESSING);
        assert_eq!(
            retry_row.pipeline_run_id.as_deref(),
            Some(retry_run.to_string().as_str())
        );

        // The sweep never rewrites a terminal verdict, whatever its age.
        assert_eq!(
            fail_stale_trigger_requests(&pool).await,
            0,
            "terminal rows are not re-swept"
        );

        // A request a consumer closed with a cause is equally final: no link
        // and no second sweep may reopen it either.
        let failed_event = uuid::Uuid::new_v4();
        let failed = record_trigger_request(
            &pool,
            repo_id,
            "refs/heads/doomed",
            &"c".repeat(41),
            failed_event,
        )
        .await
        .unwrap();
        fail_trigger_request_for_event(&pool, failed_event, &anyhow::anyhow!("invalid pipeline"))
            .await;
        let refused_run = seed_run(&pool, repo_id, pipeline_id, "running").await;
        mark_trigger_request_processing(&pool, failed_event, refused_run).await;
        let failed_row = load_trigger_request(&pool, &failed.trigger_id)
            .await
            .unwrap()
            .expect("failed request");
        assert_eq!(failed_row.status, TRIGGER_REQUEST_FAILED);
        assert_eq!(
            failed_row.pipeline_run_id, None,
            "a consumer-failed request stays unlinked and failed"
        );

        // Injected claim-write failure: with the store closed, ownership is
        // unproven and the state machine fails closed instead of planning a
        // run without a durable claim.
        let (closed_pool, closed_repo, _closed_pipeline) = trigger_request_pool().await;
        let closed_event = uuid::Uuid::new_v4();
        record_trigger_request(
            &closed_pool,
            closed_repo,
            "refs/heads/closed-store",
            &"m".repeat(41),
            closed_event,
        )
        .await
        .unwrap();
        closed_pool.pool().close().await;
        assert_eq!(
            claim_trigger_request(Some(&closed_pool), closed_event).await,
            TriggerClaim::Indeterminate,
            "a claim write that fails must never report a durable claim"
        );
    }

    /// The consumer's claim is the receipt that moves a request exactly
    /// `pending` → `claimed`, once. A late delivery — a redelivery of an
    /// owned event, an event whose request was already failed, a request
    /// whose run is linked, or one that was never recorded — is superseded
    /// instead of planning a second run for the same push, and the no-store
    /// development mode keeps planning everything.
    #[tokio::test]
    async fn trigger_claim_transitions_only_pending_and_supersedes_late_deliveries() {
        let (pool, repo_id, pipeline_id) = trigger_request_pool().await;

        // Without a durable store there is nothing to claim and the
        // historical in-memory behavior plans every published event.
        assert_eq!(
            claim_trigger_request(None, uuid::Uuid::new_v4()).await,
            TriggerClaim::Claimed
        );

        let live_event = uuid::Uuid::new_v4();
        let live = record_trigger_request(
            &pool,
            repo_id,
            "refs/heads/claim-live",
            &"d".repeat(41),
            live_event,
        )
        .await
        .unwrap();
        assert_eq!(
            claim_trigger_request(Some(&pool), live_event).await,
            TriggerClaim::Claimed
        );
        let claimed_row = load_trigger_request(&pool, &live.trigger_id)
            .await
            .unwrap()
            .expect("claimed request");
        assert_eq!(claimed_row.status, TRIGGER_REQUEST_CLAIMED);

        // A second delivery of the same event finds the row owned and stands
        // down: planning again would build the same push twice.
        assert_eq!(
            claim_trigger_request(Some(&pool), live_event).await,
            TriggerClaim::Superseded
        );

        // A request the stale sweep already failed supersedes its late queued
        // event — the caller was given a retryable failure and the retry owns
        // the rebuild.
        let swept_event = uuid::Uuid::new_v4();
        record_trigger_request(
            &pool,
            repo_id,
            "refs/heads/claim-swept",
            &"e".repeat(41),
            swept_event,
        )
        .await
        .unwrap();
        fail_trigger_request_for_event(&pool, swept_event, &anyhow::anyhow!("planning failed"))
            .await;
        assert_eq!(
            claim_trigger_request(Some(&pool), swept_event).await,
            TriggerClaim::Superseded
        );

        // A request whose run is already linked supersedes a redelivery too.
        let linked_event = uuid::Uuid::new_v4();
        record_trigger_request(
            &pool,
            repo_id,
            "refs/heads/claim-linked",
            &"f".repeat(41),
            linked_event,
        )
        .await
        .unwrap();
        assert_eq!(
            claim_trigger_request(Some(&pool), linked_event).await,
            TriggerClaim::Claimed
        );
        let linked_run = seed_run(&pool, repo_id, pipeline_id, "running").await;
        assert!(reserve_trigger_request_run(&pool, linked_event, linked_run).await);
        mark_trigger_request_processing(&pool, linked_event, linked_run).await;
        assert_eq!(
            claim_trigger_request(Some(&pool), linked_event).await,
            TriggerClaim::Superseded
        );

        // An event with no recorded request cannot be correlated, so it is
        // refused rather than planned blind.
        assert_eq!(
            claim_trigger_request(Some(&pool), uuid::Uuid::new_v4()).await,
            TriggerClaim::Superseded
        );
    }

    /// The stale-age sweeper rules only unclaimed requests lost: a `claimed`
    /// row is a live event this process's consumer owns, whatever its age,
    /// and the consumer proves ownership by linking its run after the sweep
    /// fired.
    #[tokio::test]
    async fn stale_trigger_sweep_never_fails_a_claimed_live_event() {
        let (pool, repo_id, pipeline_id) = trigger_request_pool().await;
        let window = chrono::Duration::seconds(
            i64::try_from(gitforge_common::CI_TRIGGER_CORRELATION_WINDOW.as_secs()).unwrap(),
        );

        // Claimed and ancient: a planner grinding far past the correlation
        // window is slow, not lost.
        let live_event = uuid::Uuid::new_v4();
        let live = record_trigger_request(
            &pool,
            repo_id,
            "refs/heads/sweep-live",
            &"g".repeat(41),
            live_event,
        )
        .await
        .unwrap();
        assert_eq!(
            claim_trigger_request(Some(&pool), live_event).await,
            TriggerClaim::Claimed
        );

        // Pending and equally ancient: genuinely lost, for the sweep to close.
        let lost_event = uuid::Uuid::new_v4();
        let lost = record_trigger_request(
            &pool,
            repo_id,
            "refs/heads/sweep-lost",
            &"h".repeat(41),
            lost_event,
        )
        .await
        .unwrap();
        for trigger_id in [&live.trigger_id, &lost.trigger_id] {
            sqlx::query("UPDATE ci_trigger_requests SET created_at = ? WHERE id = ?")
                .bind((Utc::now() - window - window).to_rfc3339())
                .bind(trigger_id.to_string())
                .execute(pool.pool())
                .await
                .unwrap();
        }

        assert_eq!(fail_stale_trigger_requests(&pool).await, 1);

        let live_row = load_trigger_request(&pool, &live.trigger_id)
            .await
            .unwrap()
            .expect("claimed request");
        assert_eq!(
            live_row.status, TRIGGER_REQUEST_CLAIMED,
            "the sweep must never fail a claimed live event"
        );

        // The live consumer still resolves its own row: the run lands and the
        // link lifts the claim into `processing`.
        let run_id = seed_run(&pool, repo_id, pipeline_id, "running").await;
        assert!(reserve_trigger_request_run(&pool, live_event, run_id).await);
        mark_trigger_request_processing(&pool, live_event, run_id).await;
        let linked = graded_trigger_request(&pool, &live.trigger_id)
            .await
            .unwrap()
            .expect("linked request");
        assert_eq!(linked.status, TRIGGER_REQUEST_PROCESSING);

        let lost_row = load_trigger_request(&pool, &lost.trigger_id)
            .await
            .unwrap()
            .expect("lost request");
        assert_eq!(lost_row.status, TRIGGER_REQUEST_FAILED);
    }

    /// The restart sweep closes exactly the claims the previous process took:
    /// a claim created before the boot cutoff lost its in-memory event with
    /// that process, while a claim the current process holds and a `pending`
    /// row (the stale sweep's jurisdiction) are left alone. A closed claim is
    /// terminal, so the same push becomes a fresh request.
    #[tokio::test]
    async fn restart_sweep_fails_only_claims_that_predate_the_boot_cutoff() {
        let (pool, repo_id, _pipeline_id) = trigger_request_pool().await;

        // Claimed before the cutoff, never linked: the abandoned shape.
        let abandoned_event = uuid::Uuid::new_v4();
        let abandoned = record_trigger_request(
            &pool,
            repo_id,
            "refs/heads/restart-abandoned",
            &"i".repeat(41),
            abandoned_event,
        )
        .await
        .unwrap();
        assert_eq!(
            claim_trigger_request(Some(&pool), abandoned_event).await,
            TriggerClaim::Claimed
        );

        let boot_cutoff = Utc::now();

        // Claimed after the cutoff: this process's consumer owns it.
        let live_event = uuid::Uuid::new_v4();
        let live = record_trigger_request(
            &pool,
            repo_id,
            "refs/heads/restart-live",
            &"j".repeat(41),
            live_event,
        )
        .await
        .unwrap();
        assert_eq!(
            claim_trigger_request(Some(&pool), live_event).await,
            TriggerClaim::Claimed
        );

        // Pending and older than the cutoff: the stale sweep rules it lost,
        // not the restart sweep.
        let pending_event = uuid::Uuid::new_v4();
        let pending = record_trigger_request(
            &pool,
            repo_id,
            "refs/heads/restart-pending",
            &"k".repeat(41),
            pending_event,
        )
        .await
        .unwrap();
        sqlx::query("UPDATE ci_trigger_requests SET created_at = ? WHERE id = ?")
            .bind((boot_cutoff - chrono::Duration::hours(1)).to_rfc3339())
            .bind(pending.trigger_id.to_string())
            .execute(pool.pool())
            .await
            .unwrap();

        assert_eq!(
            fail_trigger_claims_lost_at_restart(&pool, boot_cutoff).await,
            1,
            "only the pre-cutoff claim is closed"
        );

        let abandoned_row = load_trigger_request(&pool, &abandoned.trigger_id)
            .await
            .unwrap()
            .expect("abandoned request");
        assert_eq!(abandoned_row.status, TRIGGER_REQUEST_FAILED);
        assert!(
            abandoned_row
                .error
                .is_some_and(|cause| cause.contains("restart")),
            "the cause names the restart: {:?}",
            abandoned_row.error
        );

        let live_row = load_trigger_request(&pool, &live.trigger_id)
            .await
            .unwrap()
            .expect("live request");
        assert_eq!(
            live_row.status, TRIGGER_REQUEST_CLAIMED,
            "a post-cutoff claim belongs to the live consumer"
        );

        let pending_row = load_trigger_request(&pool, &pending.trigger_id)
            .await
            .unwrap()
            .expect("pending request");
        assert_eq!(
            pending_row.status, TRIGGER_REQUEST_PENDING,
            "pending rows are the stale sweep's jurisdiction"
        );

        // The closed claim is terminal: a repeat trigger for the same push is
        // a fresh request instead of deduplicating into a dead one.
        let repeat = record_request(
            &pool,
            repo_id,
            "refs/heads/restart-abandoned",
            &"i".repeat(41),
        )
        .await
        .unwrap();
        assert!(!repeat.deduplicated);
        assert_ne!(repeat.trigger_id, abandoned.trigger_id);
    }

    /// The run reservation is the correlation seam the recovery leans on, so
    /// it must be exact: a live claim accepts exactly one run id, the link
    /// lifts only the reserved (event, run) pair, and reservations on rows
    /// that are not live claims are refused.
    #[tokio::test]
    async fn run_reservation_is_single_use_and_the_link_lifts_only_the_reserved_pair() {
        let (pool, repo_id, pipeline_id) = trigger_request_pool().await;
        let ref_name = "refs/heads/reservation";
        let new_hash = "n".repeat(41);

        let event_id = uuid::Uuid::new_v4();
        let request = record_trigger_request(&pool, repo_id, ref_name, &new_hash, event_id)
            .await
            .unwrap();
        assert_eq!(
            claim_trigger_request(Some(&pool), event_id).await,
            TriggerClaim::Claimed
        );

        let run_id = seed_run(&pool, repo_id, pipeline_id, "running").await;
        assert!(reserve_trigger_request_run(&pool, event_id, run_id).await);

        // A claim is single-use: a second reservation — a redelivery planning
        // a second run for the same push — cannot re-book the row.
        assert!(
            !reserve_trigger_request_run(&pool, event_id, gitforge_common::PipelineRunId::new())
                .await,
            "an already-reserved claim must not be re-booked"
        );

        // The link lifts only the reserved pair: a different run id, even one
        // this consumer planned, cannot attach itself to the row.
        mark_trigger_request_processing(&pool, event_id, gitforge_common::PipelineRunId::new())
            .await;
        let row = load_trigger_request(&pool, &request.trigger_id)
            .await
            .unwrap()
            .expect("reserved request");
        assert_eq!(row.status, TRIGGER_REQUEST_CLAIMED);
        assert_eq!(
            row.pipeline_run_id.as_deref(),
            Some(run_id.to_string().as_str()),
            "the reservation survives a mismatched link attempt"
        );

        mark_trigger_request_processing(&pool, event_id, run_id).await;
        let row = load_trigger_request(&pool, &request.trigger_id)
            .await
            .unwrap()
            .expect("reserved request");
        assert_eq!(row.status, TRIGGER_REQUEST_PROCESSING);
        assert_eq!(
            row.pipeline_run_id.as_deref(),
            Some(run_id.to_string().as_str())
        );

        // Rows that are not live claims take no reservation at all.
        let pending_event = uuid::Uuid::new_v4();
        record_trigger_request(
            &pool,
            repo_id,
            "refs/heads/reservation-pending",
            &"o".repeat(41),
            pending_event,
        )
        .await
        .unwrap();
        assert!(
            !reserve_trigger_request_run(&pool, pending_event, run_id).await,
            "a pending row has no live claim to reserve against"
        );

        let failed_event = uuid::Uuid::new_v4();
        record_trigger_request(
            &pool,
            repo_id,
            "refs/heads/reservation-failed",
            &"p".repeat(41),
            failed_event,
        )
        .await
        .unwrap();
        fail_trigger_request_for_event(&pool, failed_event, &anyhow::anyhow!("planning failed"))
            .await;
        assert!(
            !reserve_trigger_request_run(&pool, failed_event, run_id).await,
            "a terminal row is never reserved"
        );
    }

    /// The in-process recovery sweep closes exactly the claims a dead
    /// consumer attempt can leave — no run id at all, or a reserved run id
    /// whose run row never came to exist — and never touches a claim whose
    /// run really exists. A run-backed claim resolves through the run
    /// itself: read-time grading reports an open run as `processing`, and
    /// the run's durable verdict closes the request the dead attempt never
    /// linked.
    #[tokio::test]
    async fn claim_recovery_fails_only_claims_without_a_run_and_run_verdicts_close_the_rest() {
        let (pool, repo_id, pipeline_id) = trigger_request_pool().await;

        // Claimed, nothing reserved: the attempt died before planning, so no
        // run can exist for this push.
        let unreserved_event = uuid::Uuid::new_v4();
        let unreserved = record_trigger_request(
            &pool,
            repo_id,
            "refs/heads/recovery-unreserved",
            &"q".repeat(41),
            unreserved_event,
        )
        .await
        .unwrap();
        assert_eq!(
            claim_trigger_request(Some(&pool), unreserved_event).await,
            TriggerClaim::Claimed
        );

        // Reserved a run id, but the run row never landed: the attempt died
        // between the reservation and run creation.
        let stranded_event = uuid::Uuid::new_v4();
        let stranded = record_trigger_request(
            &pool,
            repo_id,
            "refs/heads/recovery-stranded",
            &"r".repeat(41),
            stranded_event,
        )
        .await
        .unwrap();
        assert_eq!(
            claim_trigger_request(Some(&pool), stranded_event).await,
            TriggerClaim::Claimed
        );
        assert!(
            reserve_trigger_request_run(
                &pool,
                stranded_event,
                gitforge_common::PipelineRunId::new()
            )
            .await
        );

        // Reserved and the run really exists, still running: the sweep must
        // leave it alone, whatever its age.
        let live_event = uuid::Uuid::new_v4();
        let live = record_trigger_request(
            &pool,
            repo_id,
            "refs/heads/recovery-live",
            &"s".repeat(41),
            live_event,
        )
        .await
        .unwrap();
        assert_eq!(
            claim_trigger_request(Some(&pool), live_event).await,
            TriggerClaim::Claimed
        );
        let live_run = seed_run(&pool, repo_id, pipeline_id, "running").await;
        assert!(reserve_trigger_request_run(&pool, live_event, live_run).await);

        // Reserved and the run already reached a verdict without the close
        // landing: equally untouched by the sweep.
        let graded_event = uuid::Uuid::new_v4();
        let graded = record_trigger_request(
            &pool,
            repo_id,
            "refs/heads/recovery-graded",
            &"t".repeat(41),
            graded_event,
        )
        .await
        .unwrap();
        assert_eq!(
            claim_trigger_request(Some(&pool), graded_event).await,
            TriggerClaim::Claimed
        );
        let graded_run = seed_run(&pool, repo_id, pipeline_id, "succeeded").await;
        assert!(reserve_trigger_request_run(&pool, graded_event, graded_run).await);

        assert_eq!(
            fail_trigger_claims_lost_without_run(&pool)
                .await
                .expect("recovery sweep against a healthy store"),
            2,
            "only the claims without a run may be closed"
        );

        let unreserved_row = load_trigger_request(&pool, &unreserved.trigger_id)
            .await
            .unwrap()
            .expect("unreserved request");
        assert_eq!(unreserved_row.status, TRIGGER_REQUEST_FAILED);
        let stranded_row = load_trigger_request(&pool, &stranded.trigger_id)
            .await
            .unwrap()
            .expect("stranded request");
        assert_eq!(stranded_row.status, TRIGGER_REQUEST_FAILED);
        assert_eq!(
            stranded_row.pipeline_run_id, None,
            "a reservation to a run that never existed is not a correlation"
        );

        let live_row = load_trigger_request(&pool, &live.trigger_id)
            .await
            .unwrap()
            .expect("live request");
        assert_eq!(
            live_row.status, TRIGGER_REQUEST_CLAIMED,
            "a claim whose run exists must never be failed by the sweep"
        );
        let live_graded = graded_trigger_request(&pool, &live.trigger_id)
            .await
            .unwrap()
            .expect("live request");
        assert_eq!(
            live_graded.status, TRIGGER_REQUEST_PROCESSING,
            "an open run grades the orphaned claim as in flight"
        );

        let graded_row = load_trigger_request(&pool, &graded.trigger_id)
            .await
            .unwrap()
            .expect("graded request");
        assert_eq!(graded_row.status, TRIGGER_REQUEST_CLAIMED);

        // The run's own verdict is the resolution the dead attempt never
        // wrote — closing the claim cannot duplicate the build, because the
        // reservation already named this run and only this run.
        complete_trigger_request_for_run(&pool, live_run, Some("runner lost")).await;
        let closed = graded_trigger_request(&pool, &live.trigger_id)
            .await
            .unwrap()
            .expect("live request");
        assert_eq!(closed.status, TRIGGER_REQUEST_FAILED);
        assert_eq!(closed.error.as_deref(), Some("runner lost"));

        complete_trigger_request_for_run(&pool, graded_run, None).await;
        let succeeded = graded_trigger_request(&pool, &graded.trigger_id)
            .await
            .unwrap()
            .expect("graded request");
        assert_eq!(succeeded.status, TRIGGER_REQUEST_COMPLETED);
        assert_eq!(succeeded.error, None);
    }

    /// A consumer attempt opens trigger acceptance only once its bus
    /// subscription is live — never on spawn — and its startup sweep closes
    /// the claims the dead predecessor left behind before acceptance opens,
    /// while a claim whose run exists stays untouched.
    #[tokio::test]
    async fn consumer_attempt_opens_acceptance_after_subscribing_and_recovers_dead_claims() {
        let (pool, repo_id, pipeline_id) = trigger_request_pool().await;

        // Dead-claim shapes a previous attempt could have left behind.
        let lost_event = uuid::Uuid::new_v4();
        let lost = record_trigger_request(
            &pool,
            repo_id,
            "refs/heads/attempt-lost",
            &"u".repeat(41),
            lost_event,
        )
        .await
        .unwrap();
        assert_eq!(
            claim_trigger_request(Some(&pool), lost_event).await,
            TriggerClaim::Claimed
        );

        let live_event = uuid::Uuid::new_v4();
        let live = record_trigger_request(
            &pool,
            repo_id,
            "refs/heads/attempt-live",
            &"v".repeat(41),
            live_event,
        )
        .await
        .unwrap();
        assert_eq!(
            claim_trigger_request(Some(&pool), live_event).await,
            TriggerClaim::Claimed
        );
        let live_run = seed_run(&pool, repo_id, pipeline_id, "running").await;
        assert!(reserve_trigger_request_run(&pool, live_event, live_run).await);

        let health = Arc::new(ConsumerHealth::new());
        assert!(
            !health.is_running(),
            "acceptance is closed before any consumer exists"
        );
        let shutdown: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));

        let handle = tokio::spawn(run_event_consumer(
            Arc::new(InMemoryEventBus::new()),
            Arc::new(Scheduler::new()),
            Arc::new(std::sync::Mutex::new(HashMap::new())),
            Some(pool.clone()),
            Arc::new(std::sync::Mutex::new(HashMap::new())),
            Arc::new(std::sync::Mutex::new(HashMap::new())),
            Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            Arc::new(std::sync::Mutex::new(HashMap::new())),
            health.clone(),
            shutdown.clone(),
        ));

        wait_for(
            || health.is_running(),
            "the subscription went live and opened acceptance",
        )
        .await;

        // The startup sweep ran before acceptance opened: the claim with no
        // run is closed with a cause, the run-backed claim is not.
        let lost_row = load_trigger_request(&pool, &lost.trigger_id)
            .await
            .unwrap()
            .expect("lost request");
        assert_eq!(lost_row.status, TRIGGER_REQUEST_FAILED);
        assert!(
            lost_row.error.is_some(),
            "a recovered claim carries a cause"
        );
        let live_row = load_trigger_request(&pool, &live.trigger_id)
            .await
            .unwrap()
            .expect("live request");
        assert_eq!(
            live_row.status, TRIGGER_REQUEST_CLAIMED,
            "the sweep must not fail a claim whose run exists"
        );

        shutdown.store(true, Ordering::SeqCst);
        timeout(Duration::from_secs(5), handle)
            .await
            .expect("consumer joins for shutdown")
            .expect("the consumer exits cleanly");
    }

    /// An unavailable recovery store must fail the consumer attempt, not be
    /// swallowed into a zero-count sweep: the error propagates so readiness
    /// stays down and the supervisor retries instead of reopening acceptance
    /// over claims the sweep never examined.
    #[tokio::test]
    async fn recovery_sweep_failure_propagates_instead_of_reporting_zero() {
        let path = std::env::temp_dir().join(format!(
            "gitforge-sweep-failure-{}.db",
            uuid::Uuid::new_v4()
        ));
        let pool = gitforge_db::Pool::new(&path.to_string_lossy())
            .await
            .unwrap();
        pool.migrate().await.unwrap();
        // Remove the table the sweep writes, simulating an unusable store.
        sqlx::query("DROP TABLE ci_trigger_requests")
            .execute(pool.pool())
            .await
            .unwrap();

        let result = fail_trigger_claims_lost_without_run(&pool).await;
        assert!(
            result.is_err(),
            "a sweep against an unusable store must report failure, not zero"
        );
    }

    /// The subscription guard keeps the health flag honest on every exit
    /// path: dropping it after a clean exit marks the flag down, and a panic
    /// in an attempt holding it marks the flag down during the unwind.
    #[tokio::test]
    async fn subscription_guard_marks_consumer_health_down_on_drop_and_panic() {
        let health = Arc::new(ConsumerHealth::new());
        health.mark_running();
        {
            let _guard = ConsumerSubscriptionGuard {
                health: health.clone(),
            };
            assert!(
                health.is_running(),
                "holding the guard keeps acceptance open"
            );
        }
        assert!(
            !health.is_running(),
            "dropping the guard marks the attempt down"
        );

        // The panic path: the guard's drop runs during the unwind, so a dead
        // attempt never stays advertised healthy past its own death.
        health.mark_running();
        let panicked = tokio::spawn({
            let health = health.clone();
            async move {
                let _guard = ConsumerSubscriptionGuard { health };
                panic!("simulated consumer panic");
            }
        });
        assert!(panicked.await.is_err(), "the panic is observed");
        assert!(
            !health.is_running(),
            "a panicked attempt is marked down by the guard's unwind drop"
        );
    }

    /// A claimed request absorbs a repeat trigger at any age: the consumer
    /// owns the row and resolves it on every path, so a planner that outlives
    /// the correlation window cannot turn a repeat POST into a second event
    /// for the same push.
    #[tokio::test]
    async fn claimed_request_absorbs_repeats_past_the_correlation_window() {
        let (pool, repo_id, _pipeline_id) = trigger_request_pool().await;
        let ref_name = "refs/heads/claimed-dedup";
        let new_hash = "l".repeat(41);
        let window = chrono::Duration::seconds(
            i64::try_from(gitforge_common::CI_TRIGGER_CORRELATION_WINDOW.as_secs()).unwrap(),
        );

        let event_id = uuid::Uuid::new_v4();
        let first = record_trigger_request(&pool, repo_id, ref_name, &new_hash, event_id)
            .await
            .unwrap();
        assert_eq!(
            claim_trigger_request(Some(&pool), event_id).await,
            TriggerClaim::Claimed
        );

        // The correlation window elapses while the claimed consumer is still
        // planning. The claim — not the row's age — decides openness, so a
        // repeat POST still collapses into the same request.
        sqlx::query("UPDATE ci_trigger_requests SET created_at = ? WHERE id = ?")
            .bind((Utc::now() - window - window).to_rfc3339())
            .bind(first.trigger_id.to_string())
            .execute(pool.pool())
            .await
            .unwrap();

        let repeat = record_request(&pool, repo_id, ref_name, &new_hash)
            .await
            .unwrap();
        assert!(repeat.deduplicated);
        assert_eq!(repeat.trigger_id, first.trigger_id);
    }

    /// The status response carries correlation and lifecycle evidence only —
    /// no credential-shaped field exists for it to leak.
    #[tokio::test]
    async fn trigger_request_response_exposes_lifecycle_only() {
        let (pool, repo_id, _pipeline_id) = trigger_request_pool().await;
        let first = record_request(&pool, repo_id, "refs/heads/main", &"1".repeat(40))
            .await
            .unwrap();
        let row = load_trigger_request(&pool, &first.trigger_id)
            .await
            .unwrap()
            .expect("recorded request");
        let payload = row.to_response();
        let serialized = payload.to_string();
        for field in [
            "trigger_id",
            "status",
            "repo_id",
            "new_hash",
            "pipeline_run_id",
            "error",
        ] {
            assert!(
                payload.get(field).is_some(),
                "missing {field}: {serialized}"
            );
        }
        assert_eq!(payload["status"], TRIGGER_REQUEST_PENDING);
        assert_eq!(payload["pipeline_run_id"], serde_json::Value::Null);
        assert_eq!(payload["error"], serde_json::Value::Null);
        assert!(
            !serialized.contains("token") && !serialized.contains("secret"),
            "response must not carry credential-shaped fields: {serialized}"
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

    #[test]
    fn workspace_prep_timeout_parses_override_and_falls_back() {
        let default = parse_workspace_prep_timeout(None);
        assert_eq!(default, Duration::from_secs(300));

        let override_secs = parse_workspace_prep_timeout(Some("45".to_string()));
        assert_eq!(override_secs, Duration::from_secs(45));

        let padded = parse_workspace_prep_timeout(Some("  120  ".to_string()));
        assert_eq!(padded, Duration::from_secs(120));

        // Any unusable override falls back to the default rather than
        // producing a zero budget that would kill every clone instantly.
        for unusable in [
            Some("0".to_string()),
            Some("abc".to_string()),
            Some("-5".to_string()),
            Some(String::new()),
        ] {
            assert_eq!(
                parse_workspace_prep_timeout(unusable),
                Duration::from_secs(300)
            );
        }
    }

    /// A prep command that hangs must return the timeout error promptly AND
    /// stop producing output: the kill_on_drop contract, observed through
    /// the child's own heartbeat. A dead shell can never append another
    /// line, so growth after the kill is the one thing this test cannot
    /// tolerate.
    #[tokio::test]
    async fn run_git_output_times_out_and_kills_the_child() {
        let marker_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/gitforge-ci-prep-timeout-tests")
            .join(gitforge_common::PipelineRunId::new().to_string());
        tokio::fs::create_dir_all(&marker_dir).await.unwrap();
        let heartbeat = marker_dir.join("heartbeat.log");
        let heartbeat_path = heartbeat.to_string_lossy().into_owned();

        let mut command = tokio::process::Command::new("sh");
        command.arg("-c").arg(format!(
            "while :; do date +%s%N >> {heartbeat_path}; sleep 0.1; done"
        ));

        let started = std::time::Instant::now();
        let error = run_git_output(command, "clone", Duration::from_millis(300))
            .await
            .unwrap_err();
        assert!(
            started.elapsed() < Duration::from_secs(15),
            "timeout returned only after {:?}",
            started.elapsed()
        );
        assert!(error.to_string().contains("exceeded its"));

        // Let any in-flight final write land, then verify the heartbeat
        // stopped for a full second's worth of the old 10 Hz cadence.
        tokio::time::sleep(Duration::from_millis(500)).await;
        let size_after_kill = tokio::fs::metadata(&heartbeat).await.unwrap().len();
        tokio::time::sleep(Duration::from_secs(1)).await;
        let size_settled = tokio::fs::metadata(&heartbeat).await.unwrap().len();
        assert_eq!(
            size_settled, size_after_kill,
            "child kept writing after the prep budget killed it"
        );

        tokio::fs::remove_dir_all(&marker_dir).await.unwrap();
    }

    /// `fail_run` is the planning-stage finalizer: the run row must carry
    /// the human-readable cause and its never-dispatched job rows must not
    /// outlive it, while rows a runner already took stay with the runner
    /// lifecycle.
    #[tokio::test]
    async fn fail_run_records_reason_and_sweeps_unclaimed_jobs() {
        let pool = gitforge_db::Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();
        let user = gitforge_db::models::User::new(
            "fail-run-test".to_string(),
            "fail-run@example.test".to_string(),
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
                name: "fail-run-test".to_string(),
                owner_id: user.id,
                visibility: "private".to_string(),
                git_path: "/git/fail-run-test".to_string(),
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
                name: "fail-run-pipeline".to_string(),
                trigger_type: "push".to_string(),
                config: serde_json::json!({}),
                created_at: chrono::Utc::now(),
            },
        )
        .await
        .unwrap();
        let run_id = seed_run(&pool, repo_id, pipeline_id, "running").await;

        let pending = gitforge_db::models::Job::new(run_id, "pending-job".to_string());
        let mut queued = gitforge_db::models::Job::new(run_id, "queued-job".to_string());
        queued.status = gitforge_db::models::JobStatus::Queued.as_str().to_string();
        let running = gitforge_db::models::Job::new(run_id, "running-job".to_string());
        for job in [&pending, &queued, &running] {
            gitforge_db::queries::JobQueries::create(&pool, job)
                .await
                .unwrap();
        }
        gitforge_db::queries::JobQueries::update_status(&pool, running.id, "running")
            .await
            .unwrap();

        fail_run(
            &pool,
            run_id,
            &anyhow::anyhow!("clone blew its time budget"),
        )
        .await;

        let run = gitforge_db::queries::PipelineRunQueries::get(&pool, run_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(run.status, "failed");
        assert!(run
            .error
            .is_some_and(|reason| reason.contains("clone blew its time budget")));

        let swept = gitforge_db::queries::JobQueries::get(&pool, pending.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(swept.status, "cancelled");
        assert!(swept.finished_at.is_some());
        let swept = gitforge_db::queries::JobQueries::get(&pool, queued.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(swept.status, "cancelled");
        let untouched = gitforge_db::queries::JobQueries::get(&pool, running.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(untouched.status, "running");
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

    /// The reconciler settles runs no live engine can finalize, so it must
    /// also close the trigger request linked to each run it grades: a run
    /// whose durable row is terminal may not leave its linked lifecycle
    /// reading as still open until a graded GET heals it. The stored verdict
    /// matches the run — `completed` with no cause only for a succeeded run,
    /// `failed` with the specific cause for every other terminal verdict.
    #[tokio::test]
    async fn reconcile_closes_the_trigger_request_linked_to_the_run_it_finalizes() {
        let (pool, repo_id, pipeline_id) = trigger_request_pool().await;

        let succeeded = record_request(&pool, repo_id, "refs/heads/reconcile-ok", &"n".repeat(41))
            .await
            .unwrap();
        let succeeded_run = seed_run(&pool, repo_id, pipeline_id, "running").await;
        link_request_run(&pool, &succeeded.trigger_id, succeeded_run).await;
        seed_job(&pool, succeeded_run, "lint", "succeeded").await;

        let failed = record_request(&pool, repo_id, "refs/heads/reconcile-fail", &"o".repeat(41))
            .await
            .unwrap();
        let failed_run = seed_run(&pool, repo_id, pipeline_id, "running").await;
        link_request_run(&pool, &failed.trigger_id, failed_run).await;
        seed_job(&pool, failed_run, "lint", "succeeded").await;
        seed_job(&pool, failed_run, "test", "failed").await;

        let cancelled = record_request(
            &pool,
            repo_id,
            "refs/heads/reconcile-cancel",
            &"p".repeat(41),
        )
        .await
        .unwrap();
        let cancelled_run = seed_run(&pool, repo_id, pipeline_id, "running").await;
        link_request_run(&pool, &cancelled.trigger_id, cancelled_run).await;
        seed_job(&pool, cancelled_run, "lint", "cancelled").await;

        assert_eq!(reconcile_orphaned_runs(&pool).await, 3);
        assert_eq!(run_status(&pool, succeeded_run).await, "succeeded");
        assert_eq!(run_status(&pool, failed_run).await, "failed");
        assert_eq!(run_status(&pool, cancelled_run).await, "cancelled");

        // The stored rows — not the read-time grading — must already be
        // terminal, and agree with the run each request is linked to.
        let succeeded_row = load_trigger_request(&pool, &succeeded.trigger_id)
            .await
            .unwrap()
            .expect("succeeded request");
        assert_eq!(
            succeeded_row.pipeline_run_id.as_deref(),
            Some(succeeded_run.to_string().as_str())
        );
        assert_eq!(succeeded_row.status, TRIGGER_REQUEST_COMPLETED);
        assert_eq!(
            succeeded_row.error, None,
            "a succeeded run carries no cause"
        );

        let failed_row = load_trigger_request(&pool, &failed.trigger_id)
            .await
            .unwrap()
            .expect("failed request");
        assert_eq!(failed_row.status, TRIGGER_REQUEST_FAILED);
        assert!(
            failed_row
                .error
                .as_deref()
                .is_some_and(|cause| cause.contains("test")),
            "the cause names the failed job: {:?}",
            failed_row.error
        );

        let cancelled_row = load_trigger_request(&pool, &cancelled.trigger_id)
            .await
            .unwrap()
            .expect("cancelled request");
        assert_eq!(cancelled_row.status, TRIGGER_REQUEST_FAILED);
        assert!(
            cancelled_row
                .error
                .as_deref()
                .is_some_and(|cause| cause.contains("lint")),
            "the cause names the cancelled job: {:?}",
            cancelled_row.error
        );

        // Terminal is terminal: a second pass rewrites nothing.
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
    }

    /// Seed a bare repository whose HEAD commit carries a definition at
    /// every name in `config_paths`, in order. Returns (bare path, commit).
    async fn seed_pipeline_at_paths(config_paths: &[&str]) -> (PathBuf, String) {
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
        for (index, config_path) in config_paths.iter().enumerate() {
            tokio::fs::write(
                seed.join(config_path),
                format!("name: fixture-{index}\nversion: \"1.0\"\ntrigger_on:\n  - push\nenvironment: {{}}\njobs: []\n"),
            )
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

        handle_push_event(
            &zero_hash_push_envelope(),
            &scheduler,
            &pipeline_cache,
            None,
            &workspace_paths,
            &run_workspace_paths,
            &pipeline_registry,
        )
        .await
        .expect("a deletion push is consumed silently, never an error");

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

    #[test]
    fn consumer_health_fails_closed_until_a_consumer_marks_itself_running() {
        let health = ConsumerHealth::new();
        assert!(
            !health.is_running(),
            "a fresh state refuses work: no consumer has subscribed yet, and a \
             broadcast bus delivers nothing to a subscriber that does not exist"
        );
        // Only the consumer itself raises the flag, after its subscription
        // is live; nothing else may open acceptance on its behalf.
        health.mark_running();
        assert!(
            health.is_running(),
            "a consumer with a live subscription reopens the endpoint"
        );
        health.mark_down();
        assert!(
            !health.is_running(),
            "a down consumer must fail the endpoint closed"
        );
    }

    /// Poll until `condition` holds or a bounded deadline lapses, so the
    /// supervision tests assert on observable state instead of sleeping on
    /// hope.
    async fn wait_for(condition: impl Fn() -> bool, what: &str) {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !condition() {
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for {what}"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    /// The supervision contract, exercised through the same loop production
    /// runs: a panic and an error return both restart the consumer, the
    /// endpoint's fail-closed flag drops across the whole restart window,
    /// a serving attempt raises it again, and shutdown ends the loop without
    /// one more restart.
    #[tokio::test]
    async fn supervised_consumer_restarts_after_panic_and_error_and_fails_closed_between_attempts()
    {
        let health = Arc::new(ConsumerHealth::new());
        let shutdown: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));
        let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));

        let worker_attempts = attempts.clone();
        let worker_health = health.clone();
        let worker_shutdown = shutdown.clone();
        let supervisor = tokio::spawn(supervise_consumer(
            health.clone(),
            shutdown.clone(),
            move || {
                let attempts = worker_attempts.clone();
                let health = worker_health.clone();
                let shutdown = worker_shutdown.clone();
                async move {
                    match attempts.fetch_add(1, Ordering::SeqCst) {
                        // Death by panic: the poisoned-lock class of failure
                        // that used to end trigger delivery for the process.
                        0 => panic!("simulated event consumer panic"),
                        // Death by error return.
                        1 => Err(anyhow::anyhow!("simulated event consumer failure")),
                        // A serving consumer raises acceptance itself — as
                        // the real loop does, once its subscription is live
                        // — and then idles on it until shutdown.
                        _ => {
                            health.mark_running();
                            assert!(
                                health.is_running(),
                                "the serving consumer opens acceptance itself"
                            );
                            while !shutdown.load(Ordering::SeqCst) {
                                tokio::time::sleep(Duration::from_millis(10)).await;
                            }
                            Ok(())
                        }
                    }
                }
            },
        ));

        // The panic was observed, the consumer was restarted, and the
        // fail-closed flag dropped while the backoff ran.
        wait_for(|| attempts.load(Ordering::SeqCst) >= 1, "the first attempt").await;
        wait_for(|| !health.is_running(), "fail-closed after the panic").await;
        wait_for(
            || attempts.load(Ordering::SeqCst) >= 2,
            "a restart after the panic",
        )
        .await;
        // The same contract for the error-return death.
        wait_for(
            || !health.is_running(),
            "fail-closed after the error return",
        )
        .await;
        wait_for(
            || attempts.load(Ordering::SeqCst) >= 3,
            "a restart after the error",
        )
        .await;
        wait_for(
            || health.is_running(),
            "the recovered consumer reopens acceptance",
        )
        .await;

        // Shutdown ends supervision without a further restart and leaves the
        // endpoint fail-closed behind it.
        shutdown.store(true, Ordering::SeqCst);
        timeout(Duration::from_secs(5), supervisor)
            .await
            .expect("supervision joins after shutdown")
            .expect("the supervisor task itself never panics");
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
        assert!(!health.is_running(), "a stopped consumer stays fail-closed");
    }

    /// A serving consumer is left alone: no spurious restarts while it is
    /// healthy, and shutdown closes the endpoint's acceptance behind it.
    #[tokio::test]
    async fn supervised_consumer_keeps_a_serving_worker_open_until_shutdown() {
        let health = Arc::new(ConsumerHealth::new());
        let shutdown: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));
        let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        assert!(
            !health.is_running(),
            "acceptance stays closed until the worker's subscription is live"
        );

        let worker_attempts = attempts.clone();
        let worker_health = health.clone();
        let worker_shutdown = shutdown.clone();
        let supervisor = tokio::spawn(supervise_consumer(
            health.clone(),
            shutdown.clone(),
            move || {
                let attempts = worker_attempts.clone();
                let health = worker_health.clone();
                let shutdown = worker_shutdown.clone();
                async move {
                    attempts.fetch_add(1, Ordering::SeqCst);
                    // Subscription goes live, so the worker opens acceptance.
                    health.mark_running();
                    while !shutdown.load(Ordering::SeqCst) {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                    Ok(())
                }
            },
        ));

        wait_for(
            || attempts.load(Ordering::SeqCst) == 1,
            "the consumer attempt",
        )
        .await;
        wait_for(
            || health.is_running(),
            "the subscribed worker opens acceptance",
        )
        .await;
        // A full restart backoff with a serving consumer must not produce a
        // second attempt.
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        assert!(
            health.is_running(),
            "a serving consumer keeps acceptance open"
        );

        shutdown.store(true, Ordering::SeqCst);
        timeout(Duration::from_secs(5), supervisor)
            .await
            .expect("supervision joins after shutdown")
            .expect("the supervisor task itself never panics");
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        assert!(!health.is_running(), "shutdown closes acceptance");
    }

    /// The endpoint's fail-closed answer while the process's consumer is
    /// down — the state every process boots in, before its consumer's first
    /// subscription: an explicit, retryable 503 before any durable side
    /// effect (dev mode here: nothing recorded, nothing published).
    #[tokio::test]
    async fn trigger_endpoint_fails_closed_while_consumer_is_down() {
        let trigger_state = Arc::new(TriggerState {
            event_bus: Arc::new(InMemoryEventBus::new()),
            workspace_paths: Arc::new(std::sync::Mutex::new(HashMap::new())),
            run_waiters: Arc::new(std::sync::Mutex::new(HashMap::new())),
            db: None,
            consumer_health: Arc::new(ConsumerHealth::new()),
        });
        assert!(
            !trigger_state.consumer_health.is_running(),
            "a fresh state refuses work: the consumer has not subscribed yet"
        );

        let response = trigger_pipeline(
            Extension(trigger_state),
            Json(PipelineTriggerRequest {
                repo_id: uuid::Uuid::new_v4().to_string(),
                ref_name: "refs/heads/main".to_string(),
                old_hash: "0".repeat(40),
                new_hash: "a".repeat(40),
                working_dir: None,
            }),
        )
        .await
        .into_response();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body");
        let payload: serde_json::Value = serde_json::from_slice(&body).expect("json body");
        assert_eq!(payload["error"], "trigger_consumer_unavailable");
    }
}

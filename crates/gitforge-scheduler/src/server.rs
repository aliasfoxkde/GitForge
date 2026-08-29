//! Scheduler HTTP server

use crate::Scheduler;
use axum::{
    body::Bytes,
    extract::Request,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{HeaderMap, StatusCode},
    middleware::{self, Next},
    response::IntoResponse,
    response::Response,
    routing::{get, post},
    Json, Router,
};
use gitforge_common::{JobId, RunnerId};
use gitforge_db::models::{Runner, RunnerType};
use gitforge_storage::{Artifact, ArtifactId, ArtifactStore, FileStorage, MAX_ARTIFACT_BYTES};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

/// Application state for scheduler server
#[derive(Clone)]
pub struct SchedulerServerState {
    pub scheduler: Arc<Scheduler>,
    runner_auth_token: Option<Arc<str>>,
    operator_auth_token: Option<Arc<str>>,
    artifact_storage: Option<Arc<FileStorage>>,
}

/// Runner registration request
#[derive(Debug, Deserialize)]
pub struct RegisterRunnerRequest {
    pub name: String,
    #[serde(rename = "type")]
    pub runner_type: String,
    pub capacity: i32,
}

/// Job info for pending jobs response
#[derive(Debug, Serialize)]
pub struct PendingJobInfo {
    pub contract_version: &'static str,
    pub job_id: String,
    pub name: String,
    pub pipeline_run_id: String,
    pub commands: Vec<String>,
    pub working_dir: Option<String>,
    pub runner_id: String,
    pub lease_token: String,
}

#[derive(Debug, Deserialize)]
struct PendingJobsQuery {
    runner_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LeaseRequest {
    runner_id: String,
    lease_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LogAppendRequest {
    runner_id: String,
    lease_token: String,
    chunk: String,
}

/// Authenticated operator submission. The idempotency key is mandatory so a
/// retry after a lost response cannot start duplicate work.
#[derive(Debug, Deserialize, Serialize)]
pub struct SubmitJobRequest {
    pub pipeline_run_id: String,
    pub repo_id: String,
    pub commands: Vec<String>,
    pub working_dir: Option<String>,
    pub idempotency_key: String,
}

/// Create scheduler server state
pub fn create_state(scheduler: Scheduler) -> SchedulerServerState {
    create_state_with_artifact_storage(scheduler, None)
}

/// Create scheduler state with an optional shared artifact store. The CI
/// service should attach the same root used by the API gateway.
pub fn create_state_with_artifact_storage(
    scheduler: Scheduler,
    artifact_storage: Option<Arc<FileStorage>>,
) -> SchedulerServerState {
    let shared = std::env::var("GITFORGE_SCHEDULER_TOKEN")
        .ok()
        .filter(|token| !token.is_empty())
        .map(Arc::from);
    let runner = std::env::var("GITFORGE_RUNNER_TOKEN")
        .ok()
        .filter(|token| !token.is_empty())
        .map(Arc::from)
        .or_else(|| shared.clone());
    let operator = std::env::var("GITFORGE_SCHEDULER_OPERATOR_TOKEN")
        .ok()
        .filter(|token| !token.is_empty())
        .map(Arc::from)
        .or(shared);
    SchedulerServerState {
        scheduler: Arc::new(scheduler),
        runner_auth_token: runner,
        operator_auth_token: operator,
        artifact_storage,
    }
}

impl SchedulerServerState {
    /// Attach the shared artifact store used by the API gateway.
    pub fn with_artifact_storage(mut self, storage: Arc<FileStorage>) -> Self {
        self.artifact_storage = Some(storage);
        self
    }
}

/// Create scheduler routes
pub fn scheduler_routes<S: Clone + Send + Sync + 'static>(
    state: SchedulerServerState,
) -> Router<S> {
    let runner_auth_token = state.runner_auth_token.clone();
    let operator_auth_token = state.operator_auth_token.clone();
    scheduler_routes_with_tokens(state, runner_auth_token, operator_auth_token)
}

/// Build scheduler routes with independently scoped runner and operator
/// credentials. Public so integration tests and colocated service harnesses
/// can exercise the real HTTP boundary without relying on process globals.
pub fn scheduler_routes_with_tokens<S: Clone + Send + Sync + 'static>(
    state: SchedulerServerState,
    runner_auth_token: Option<Arc<str>>,
    operator_auth_token: Option<Arc<str>>,
) -> Router<S> {
    let runner_routes = Router::new()
        .route("/runners", post(register_runner))
        .route("/runners/{id}/heartbeat", post(runner_heartbeat))
        .route("/jobs/pending", get(get_pending_jobs))
        .route("/jobs/{id}/claim", post(claim_job))
        .route("/jobs/{id}/started", post(start_job))
        .route("/jobs/{id}/logs", post(append_job_log))
        .route("/jobs/{id}/artifacts", post(upload_job_artifact))
        .layer(DefaultBodyLimit::max(MAX_ARTIFACT_BYTES as usize))
        .route("/jobs/{id}/cancelled", get(job_cancelled))
        .route("/jobs/{id}/assign", post(assign_job))
        .route("/jobs/{id}/complete", post(complete_job))
        .layer(middleware::from_fn(move |request, next: Next| {
            require_scheduler_auth(request, next, runner_auth_token.clone(), "runner")
        }));
    let operator_routes = Router::new()
        .route("/jobs/{id}/cancel", post(cancel_job))
        .route("/jobs", post(submit_job))
        .route("/pipelines/runs/{id}", get(get_pipeline_run))
        .layer(middleware::from_fn(move |request, next: Next| {
            require_scheduler_auth(request, next, operator_auth_token.clone(), "operator")
        }));
    runner_routes.merge(operator_routes).with_state(state)
}

async fn require_scheduler_auth(
    request: Request,
    next: Next,
    expected: Option<Arc<str>>,
    role: &'static str,
) -> Response {
    let Some(expected) = expected else {
        tracing::error!(%role, "scheduler API credential is unset; refusing access");
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"error": "scheduler_auth_not_configured"})),
        )
            .into_response();
    };
    let supplied = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok());
    if supplied == Some(&format!("Bearer {expected}")) {
        next.run(request).await
    } else {
        (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": "scheduler_auth_required"})),
        )
            .into_response()
    }
}

/// Return a durable pipeline run for LAN control-plane adapters.
async fn get_pipeline_run(
    State(state): State<SchedulerServerState>,
    Path(run_id): Path<String>,
) -> impl IntoResponse {
    let run_id = match Uuid::parse_str(&run_id) {
        Ok(id) => gitforge_common::PipelineRunId::from(id),
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "invalid_pipeline_run_id"})),
            )
        }
    };
    match state.scheduler.get_pipeline_run(run_id).await {
        Ok(Some(run)) => match state.scheduler.get_pipeline_run_jobs(run_id).await {
            Ok(jobs) => {
                let mut payload = match serde_json::to_value(run) {
                    Ok(serde_json::Value::Object(object)) => object,
                    Ok(_) | Err(_) => {
                        return (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            Json(serde_json::json!({"error": "pipeline_run_serialize_failed"})),
                        )
                    }
                };
                payload.insert("jobs".to_string(), serde_json::json!(jobs));
                (StatusCode::OK, Json(serde_json::Value::Object(payload)))
            }
            Err(error) => {
                tracing::error!(%error, %run_id, "failed to load pipeline jobs");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({"error": "pipeline_job_lookup_failed"})),
                )
            }
        },
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "pipeline_run_not_found"})),
        ),
        Err(error) => {
            tracing::error!(%error, %run_id, "failed to load pipeline run");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "pipeline_run_lookup_failed"})),
            )
        }
    }
}

/// Register a new runner
async fn register_runner(
    State(state): State<SchedulerServerState>,
    Json(request): Json<RegisterRunnerRequest>,
) -> impl IntoResponse {
    let runner_type = match request.runner_type.as_str() {
        "docker" => RunnerType::Docker,
        "firecracker" => RunnerType::Firecracker,
        "bare-metal" | "bare_metal" => RunnerType::BareMetal,
        _ => RunnerType::Docker,
    };

    let runner = Runner::new(request.name, runner_type, request.capacity);

    state.scheduler.register_runner(runner.clone()).await;

    tracing::info!("runner {} registered via HTTP", runner.id);

    (
        StatusCode::CREATED,
        Json(serde_json::json!({
            "id": runner.id.to_string(),
            "name": runner.name,
            "type": runner.runner_type,
            "status": runner.status,
            "capacity": runner.capacity,
        })),
    )
}

/// Runner heartbeat
async fn runner_heartbeat(
    State(state): State<SchedulerServerState>,
    Path(runner_id): Path<String>,
) -> impl IntoResponse {
    let runner_id: RunnerId = match Uuid::parse_str(&runner_id) {
        Ok(id) => RunnerId::from(id),
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": "invalid_runner_id",
                    "message": "Invalid runner ID format"
                })),
            )
        }
    };

    if !state.scheduler.heartbeat(runner_id).await {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "runner_not_registered"})),
        );
    }

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "status": "ok"
        })),
    )
}

/// Cancel a queued, assigned, or running job and persist the terminal state.
async fn cancel_job(
    State(state): State<SchedulerServerState>,
    Path(job_id): Path<String>,
) -> impl IntoResponse {
    let job_id = match Uuid::parse_str(&job_id) {
        Ok(id) => JobId::from(id),
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "invalid_job_id"})),
            )
        }
    };
    if !state.scheduler.job_exists(job_id).await {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "job_not_found", "job_id": job_id.to_string()})),
        );
    }
    state.scheduler.cancel(job_id).await;
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "contract_version": "harness.job.v1",
            "status": "cancelled",
            "job_id": job_id.to_string(),
        })),
    )
}

async fn submit_job(
    State(state): State<SchedulerServerState>,
    Json(request): Json<SubmitJobRequest>,
) -> impl IntoResponse {
    if request.idempotency_key.is_empty() || request.idempotency_key.len() > 128 {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "invalid_idempotency_key"})),
        );
    }
    if request.commands.is_empty()
        || request.commands.len() > 64
        || request
            .commands
            .iter()
            .any(|command| command.is_empty() || command.len() > 16 * 1024)
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "invalid_commands"})),
        );
    }
    if request
        .working_dir
        .as_deref()
        .is_some_and(|path| path.is_empty() || path.len() > 1024)
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "invalid_working_dir"})),
        );
    }
    let pipeline_run_id = match Uuid::parse_str(&request.pipeline_run_id) {
        Ok(id) => gitforge_common::PipelineRunId::from(id),
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "invalid_pipeline_run_id"})),
            )
        }
    };
    let repo_id = match Uuid::parse_str(&request.repo_id) {
        Ok(id) => gitforge_common::RepoId::from(id),
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "invalid_repo_id"})),
            )
        }
    };
    match state.scheduler.get_pipeline_run(pipeline_run_id).await {
        Ok(Some(run)) if run.repo_id == repo_id => {}
        Ok(Some(_)) => {
            return (
                StatusCode::CONFLICT,
                Json(serde_json::json!({"error": "pipeline_run_repository_mismatch"})),
            )
        }
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "pipeline_run_not_found"})),
            )
        }
        Err(error) => {
            tracing::error!(%error, %pipeline_run_id, "failed to validate pipeline run");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({"error": "pipeline_run_lookup_failed"})),
            )
        }
    }
    let fingerprint = match serde_json::to_string(&request) {
        Ok(value) => value,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "invalid_request"})),
            )
        }
    };
    match state
        .scheduler
        .submit_idempotent(
            pipeline_run_id,
            repo_id,
            request.commands,
            request.working_dir,
            &request.idempotency_key,
            &fingerprint,
        )
        .await
    {
        Ok((job_id, true)) => (
            StatusCode::CREATED,
            Json(serde_json::json!({
                "contract_version": "harness.job.v1",
                "status": "queued",
                "job_id": job_id.to_string()
            })),
        ),
        Ok((job_id, false)) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "contract_version": "harness.job.v1",
                "status": "already_queued",
                "job_id": job_id.to_string()
            })),
        ),
        Err(error) if error.to_string().contains("idempotency_key_reused") => (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": "idempotency_key_reused_with_different_request"
            })),
        ),
        Err(error) => {
            tracing::error!(%error, "operator job submission failed");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({"error": "durable_scheduler_unavailable"})),
            )
        }
    }
}

/// Let an assigned runner observe operator cancellation and stop its local
/// executor. This does not authorize cancellation and is protected by the
/// same scheduler token as all runner-control routes.
async fn job_cancelled(
    State(state): State<SchedulerServerState>,
    Path(job_id): Path<String>,
) -> impl IntoResponse {
    let job_id = match Uuid::parse_str(&job_id) {
        Ok(id) => JobId::from(id),
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "invalid_job_id"})),
            )
        }
    };
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "contract_version": "harness.job.v1",
            "job_id": job_id.to_string(),
            "cancelled": state.scheduler.is_cancelled(job_id).await,
        })),
    )
}

/// Get pending jobs for a runner
async fn get_pending_jobs(
    State(state): State<SchedulerServerState>,
    Query(query): Query<PendingJobsQuery>,
) -> impl IntoResponse {
    // Process queue to assign pending jobs
    state.scheduler.process_queue().await;

    // Get jobs assigned to runners (these are pending execution)
    let assigned_jobs = state.scheduler.get_assigned_job_details().await;
    let requested_runner = query
        .runner_id
        .as_deref()
        .and_then(|id| Uuid::parse_str(id).ok())
        .map(RunnerId::from);

    // Convert to response format
    let mut job_infos = Vec::new();
    for (job_id, runner_id, pipeline_run_id, definition) in assigned_jobs {
        if requested_runner
            .map(|requested| requested == runner_id)
            .unwrap_or(false)
        {
            if let Some(lease_token) = state.scheduler.ensure_job_lease(job_id).await {
                job_infos.push(PendingJobInfo {
                    contract_version: "harness.job.v1",
                    job_id: job_id.to_string(),
                    name: format!("job-{}", job_id),
                    pipeline_run_id: pipeline_run_id.to_string(),
                    commands: definition.commands,
                    working_dir: definition.working_dir,
                    runner_id: runner_id.to_string(),
                    lease_token,
                });
            }
        }
    }

    Json(serde_json::json!(job_infos))
}

/// Claim an already scheduler-assigned job with a runner lease.
async fn claim_job(
    State(state): State<SchedulerServerState>,
    Path(job_id): Path<String>,
    Json(request): Json<LeaseRequest>,
) -> impl IntoResponse {
    let job_id = match Uuid::parse_str(&job_id) {
        Ok(id) => JobId::from(id),
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "invalid_job_id"})),
            )
        }
    };
    let runner_id = match Uuid::parse_str(&request.runner_id) {
        Ok(id) => RunnerId::from(id),
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "invalid_runner_id"})),
            )
        }
    };
    let Some(lease_token) = state.scheduler.ensure_job_lease(job_id).await else {
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({"error": "job_not_assigned"})),
        );
    };
    let assigned = state.scheduler.is_assigned(job_id).await == Some(runner_id);
    if !assigned {
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({"error": "runner_not_assigned"})),
        );
    }
    if let Some(requested) = request.lease_token {
        if requested != lease_token {
            return (
                StatusCode::CONFLICT,
                Json(serde_json::json!({"error": "invalid_lease"})),
            );
        }
    }
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "contract_version": "harness.job.v1",
            "status": "assigned",
            "job_id": job_id.to_string(),
            "runner_id": runner_id.to_string(),
            "lease_token": lease_token,
        })),
    )
}

/// Record the assigned-to-running transition.
async fn start_job(
    State(state): State<SchedulerServerState>,
    Path(job_id): Path<String>,
    Json(request): Json<LeaseRequest>,
) -> impl IntoResponse {
    let job_id = match Uuid::parse_str(&job_id) {
        Ok(id) => JobId::from(id),
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "invalid_job_id"})),
            )
        }
    };
    let runner_id = match Uuid::parse_str(&request.runner_id) {
        Ok(id) => RunnerId::from(id),
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "invalid_runner_id"})),
            )
        }
    };
    let Some(lease_token) = request.lease_token else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "missing_lease_token"})),
        );
    };
    match state
        .scheduler
        .start_job(job_id, runner_id, &lease_token)
        .await
    {
        Ok(()) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "contract_version": "harness.job.v1",
                "status": "running",
                "job_id": job_id.to_string(),
            })),
        ),
        Err(error) => (
            StatusCode::CONFLICT,
            Json(serde_json::json!({"error": error.to_string()})),
        ),
    }
}

/// Append bounded runner output while the durable lease is active.
async fn append_job_log(
    State(state): State<SchedulerServerState>,
    Path(job_id): Path<String>,
    Json(request): Json<LogAppendRequest>,
) -> impl IntoResponse {
    let job_id = match Uuid::parse_str(&job_id) {
        Ok(id) => JobId::from(id),
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "invalid_job_id"})),
            )
        }
    };
    let runner_id = match Uuid::parse_str(&request.runner_id) {
        Ok(id) => RunnerId::from(id),
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "invalid_runner_id"})),
            )
        }
    };
    match state
        .scheduler
        .append_log_with_lease(job_id, runner_id, &request.lease_token, &request.chunk)
        .await
    {
        Ok(Some(sequence)) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "contract_version": "harness.job.v1",
                "job_id": job_id.to_string(),
                "sequence": sequence,
            })),
        ),
        Ok(None) => (
            StatusCode::CONFLICT,
            Json(serde_json::json!({"error": "invalid_job_lease"})),
        ),
        Err(error) if error.to_string().contains("exceeds") => (
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(serde_json::json!({"error": "log_limit_exceeded", "message": error.to_string()})),
        ),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(
                serde_json::json!({"error": "log_persistence_failed", "message": error.to_string()}),
            ),
        ),
    }
}

/// Upload one bounded artifact from a runner into the shared store.
/// Metadata is created server-side; the runner can provide only a safe name
/// and optional media type, never an arbitrary filesystem path.
async fn upload_job_artifact(
    State(state): State<SchedulerServerState>,
    Path(job_id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    if body.len() as u64 > MAX_ARTIFACT_BYTES {
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(serde_json::json!({"error": "artifact_too_large"})),
        );
    }
    let job_id = match Uuid::parse_str(&job_id) {
        Ok(id) => JobId::from(id),
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "invalid_job_id"})),
            )
        }
    };
    let runner_id = match headers
        .get("x-runner-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| Uuid::parse_str(value).ok())
    {
        Some(id) => RunnerId::from(id),
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "missing_runner_id"})),
            )
        }
    };
    let Some(lease_token) = headers
        .get("x-lease-token")
        .and_then(|value| value.to_str().ok())
    else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "missing_lease_token"})),
        );
    };
    let Some(name) = headers
        .get("x-artifact-name")
        .and_then(|value| value.to_str().ok())
    else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "missing_artifact_name"})),
        );
    };
    if name.is_empty() || name.len() > 256 || name.contains('\0') || name.contains("..") {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "invalid_artifact_name"})),
        );
    }
    let Some(storage) = state.artifact_storage else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"error": "artifact_storage_not_configured"})),
        );
    };
    let active = state
        .scheduler
        .job_lease_active(job_id, runner_id, lease_token)
        .await;
    if !active {
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({"error": "invalid_job_lease"})),
        );
    }

    let checksum = sha256_hex(&body);
    if let Some(expected) = headers
        .get("x-artifact-sha256")
        .and_then(|value| value.to_str().ok())
    {
        if expected != checksum {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "artifact_checksum_mismatch"})),
            );
        }
    }
    let artifact_id = ArtifactId::new();
    let artifact = Artifact {
        id: artifact_id,
        job_id,
        name: name.to_string(),
        path: format!("gitforge://artifact/{artifact_id}"),
        checksum: checksum.clone(),
        size_bytes: body.len() as u64,
        content_type: headers
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string),
        created_at: chrono::Utc::now(),
    };
    let artifact_id = artifact.id.to_string();
    if let Err(error) = storage.put(&artifact, &body).await {
        tracing::error!(%error, %job_id, "failed to persist runner artifact");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "artifact_persistence_failed"})),
        );
    }
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "contract_version": "harness.job.v1",
            "artifact_id": artifact_id,
            "job_id": job_id.to_string(),
            "name": name,
            "sha256": checksum,
            "bytes": body.len(),
        })),
    )
}

fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

/// Assign a job to a runner (runner claims a job)
async fn assign_job(
    State(_state): State<SchedulerServerState>,
    Path(job_id): Path<String>,
    Json(request): Json<serde_json::Value>,
) -> impl IntoResponse {
    let job_id: JobId = match Uuid::parse_str(&job_id) {
        Ok(id) => JobId::from(id),
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": "invalid_job_id",
                    "message": "Invalid job ID format"
                })),
            )
        }
    };

    let runner_id: Option<RunnerId> = request["runner_id"]
        .as_str()
        .and_then(|s| Uuid::parse_str(s).ok().map(RunnerId::from));

    if let Some(r_id) = runner_id {
        // In real impl, mark job as assigned to this runner
        tracing::info!("job {} assigned to runner {} via HTTP", job_id, r_id);
    }

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "status": "assigned",
            "job_id": job_id.to_string()
        })),
    )
}

/// Complete a job
async fn complete_job(
    State(state): State<SchedulerServerState>,
    Path(job_id): Path<String>,
    Json(request): Json<serde_json::Value>,
) -> impl IntoResponse {
    let job_id: JobId = match Uuid::parse_str(&job_id) {
        Ok(id) => JobId::from(id),
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": "invalid_job_id",
                    "message": "Invalid job ID format"
                })),
            )
        }
    };

    let success = request["success"].as_bool().unwrap_or(false);
    let exit_code = request["exit_code"].as_i64().unwrap_or(-1);
    let error = request["error"].as_str();
    let runner_id = request["runner_id"]
        .as_str()
        .and_then(|value| Uuid::parse_str(value).ok())
        .map(RunnerId::from);
    let lease_token = request["lease_token"].as_str();

    tracing::info!(
        "job {} completed via HTTP: success={}, exit_code={}",
        job_id,
        success,
        exit_code
    );

    let receipt = serde_json::json!({
        "job_id": job_id.to_string(),
        "success": success,
        "exit_code": exit_code,
        "error": error,
        "step_results": request["step_results"].clone(),
        "artifacts": request["artifacts"].clone(),
    })
    .to_string();

    let assigned_runner = state.scheduler.is_assigned(job_id).await;
    let completion = match (runner_id, lease_token, assigned_runner) {
        (Some(runner_id), Some(lease_token), Some(_)) => {
            state
                .scheduler
                .complete_job_with_lease(job_id, runner_id, lease_token, success, receipt)
                .await
        }
        (None, None, None) => {
            // Preserve the synthetic no-database handler behavior used by
            // legacy callers/tests. Real assigned jobs must use a lease.
            state.scheduler.complete_job(job_id, success, receipt).await
        }
        _ => Err(anyhow::anyhow!("runner_id and lease_token are required")),
    };
    if let Err(error) = completion {
        tracing::error!("failed to persist job {} completion: {}", job_id, error);
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": "completion_persistence_failed",
                "message": error.to_string(),
            })),
        );
    }

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "status": "completed",
            "job_id": job_id.to_string()
        })),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    fn assert_status(response: axum::response::Response, expected: StatusCode) {
        let status = response.status();
        assert_eq!(
            status, expected,
            "Expected status {:?}, got {:?}",
            expected, status
        );
    }

    #[tokio::test]
    async fn test_register_runner_handler() {
        let scheduler = crate::Scheduler::new();
        let state = create_state(scheduler);

        let request = RegisterRunnerRequest {
            name: "test-runner".to_string(),
            runner_type: "docker".to_string(),
            capacity: 4,
        };

        let response = register_runner(axum::extract::State(state), axum::Json(request)).await;

        assert_status(response.into_response(), StatusCode::CREATED);
    }

    #[tokio::test]
    async fn test_register_runner_bare_metal_handler() {
        let scheduler = crate::Scheduler::new();
        let state = create_state(scheduler);

        let request = RegisterRunnerRequest {
            name: "bare-runner".to_string(),
            runner_type: "bare-metal".to_string(),
            capacity: 8,
        };

        let response = register_runner(axum::extract::State(state), axum::Json(request)).await;

        assert_status(response.into_response(), StatusCode::CREATED);
    }

    #[tokio::test]
    async fn test_runner_heartbeat_valid_uuid() {
        let scheduler = crate::Scheduler::new();
        let state = create_state(scheduler);
        let runner = Runner::new("heartbeat-runner".to_string(), RunnerType::Docker, 1);
        let runner_id = runner.id;
        state.scheduler.register_runner(runner).await;

        let response = runner_heartbeat(
            axum::extract::State(state),
            axum::extract::Path(runner_id.to_string()),
        )
        .await;

        assert_status(response.into_response(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_runner_heartbeat_unknown_runner_is_not_found() {
        let state = create_state(crate::Scheduler::new());
        let response = runner_heartbeat(
            axum::extract::State(state),
            axum::extract::Path(uuid::Uuid::new_v4().to_string()),
        )
        .await;
        assert_status(response.into_response(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_runner_heartbeat_invalid_uuid() {
        let scheduler = crate::Scheduler::new();
        let state = create_state(scheduler);

        let response = runner_heartbeat(
            axum::extract::State(state),
            axum::extract::Path("not-a-uuid".to_string()),
        )
        .await;

        assert_status(response.into_response(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_get_pending_jobs_handler() {
        let scheduler = crate::Scheduler::new();
        let state = create_state(scheduler);

        let response = get_pending_jobs(
            axum::extract::State(state),
            axum::extract::Query(PendingJobsQuery { runner_id: None }),
        )
        .await;

        let resp = response.into_response();
        assert_status(resp, StatusCode::OK);
    }

    #[tokio::test]
    async fn test_http_runner_protocol_uses_durable_lease() {
        let pool = gitforge_db::Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();
        let user = gitforge_db::models::User::new(
            "http-lease-owner".to_string(),
            "http-lease-owner@example.com".to_string(),
            "hash".to_string(),
        );
        gitforge_db::queries::UserQueries::create(&pool, &user)
            .await
            .unwrap();
        let repo = gitforge_db::models::Repository::new(
            "http-lease-repo".to_string(),
            user.id,
            "/git/http-lease-repo".to_string(),
        );
        gitforge_db::queries::RepoQueries::create(&pool, &repo)
            .await
            .unwrap();
        let pipeline = gitforge_db::models::Pipeline {
            id: gitforge_common::PipelineId::new(),
            repo_id: repo.id,
            name: "http-lease-pipeline".to_string(),
            trigger_type: "manual".to_string(),
            config: serde_json::json!({}),
            created_at: chrono::Utc::now(),
        };
        gitforge_db::queries::PipelineQueries::create(&pool, &pipeline)
            .await
            .unwrap();
        let run = gitforge_db::models::PipelineRun::new(
            pipeline.id,
            repo.id,
            "http-lease-owner".to_string(),
            "http-lease-commit".to_string(),
        );
        gitforge_db::queries::PipelineRunQueries::create(&pool, &run)
            .await
            .unwrap();
        let job = gitforge_db::models::Job::new(run.id, "http-lease-job".to_string());
        let job_id = job.id;
        gitforge_db::queries::JobQueries::create(&pool, &job)
            .await
            .unwrap();

        let scheduler = crate::Scheduler::with_db(pool.clone());
        let runner = Runner::new("http-lease-runner".to_string(), RunnerType::Docker, 1);
        let runner_id = runner.id;
        scheduler.register_runner(runner).await;
        scheduler
            .enqueue_with_definition(
                job_id,
                run.id,
                repo.id,
                vec!["cargo test".to_string()],
                None,
            )
            .await;
        let artifact_root =
            std::env::temp_dir().join(format!("gitforge-scheduler-artifacts-{}", Uuid::new_v4()));
        let artifact_storage = Arc::new(FileStorage::new(&artifact_root).await.unwrap());
        let state = create_state_with_artifact_storage(scheduler, Some(artifact_storage.clone()));
        let app = scheduler_routes_with_tokens(
            state,
            Some(Arc::from("runner-token")),
            Some(Arc::from("operator-token")),
        );

        let pending = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/jobs/pending?runner_id={runner_id}"))
                    .header("Authorization", "Bearer runner-token")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(pending.status(), StatusCode::OK);
        let pending_body = axum::body::to_bytes(pending.into_body(), 64 * 1024)
            .await
            .unwrap();
        let assignments: Vec<serde_json::Value> = serde_json::from_slice(&pending_body).unwrap();
        assert_eq!(assignments.len(), 1);
        let lease_token = assignments[0]["lease_token"].as_str().unwrap();

        let started = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/jobs/{job_id}/started"))
                    .header("Authorization", "Bearer runner-token")
                    .header("Content-Type", "application/json")
                    .body(axum::body::Body::from(
                        serde_json::json!({"runner_id": runner_id.to_string(), "lease_token": lease_token}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(started.status(), StatusCode::OK);

        let artifact_body = b"runner artifact";
        let artifact_checksum = sha256_hex(artifact_body);
        let artifact = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/jobs/{job_id}/artifacts"))
                    .header("Authorization", "Bearer runner-token")
                    .header("Content-Type", "application/octet-stream")
                    .header("x-runner-id", runner_id.to_string())
                    .header("x-lease-token", lease_token)
                    .header("x-artifact-name", "result.txt")
                    .header("x-artifact-sha256", artifact_checksum)
                    .body(axum::body::Body::from(artifact_body.as_slice()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(artifact.status(), StatusCode::OK);
        let artifact_payload = axum::body::to_bytes(artifact.into_body(), 64 * 1024)
            .await
            .unwrap();
        let artifact_id: String = serde_json::from_slice::<serde_json::Value>(&artifact_payload)
            .unwrap()["artifact_id"]
            .as_str()
            .unwrap()
            .to_string();

        let stale_log = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/jobs/{job_id}/logs"))
                    .header("Authorization", "Bearer runner-token")
                    .header("Content-Type", "application/json")
                    .body(axum::body::Body::from(
                        serde_json::json!({
                            "runner_id": runner_id.to_string(),
                            "lease_token": "stale",
                            "chunk": "must be fenced"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(stale_log.status(), StatusCode::CONFLICT);

        let log = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/jobs/{job_id}/logs"))
                    .header("Authorization", "Bearer runner-token")
                    .header("Content-Type", "application/json")
                    .body(axum::body::Body::from(
                        serde_json::json!({
                            "runner_id": runner_id.to_string(),
                            "lease_token": lease_token,
                            "chunk": "live output\n"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(log.status(), StatusCode::OK);

        let completed = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/jobs/{job_id}/complete"))
                    .header("Authorization", "Bearer runner-token")
                    .header("Content-Type", "application/json")
                    .body(axum::body::Body::from(
                        serde_json::json!({
                            "runner_id": runner_id.to_string(),
                            "lease_token": lease_token,
                            "success": true,
                            "receipt": {"http": true}
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(completed.status(), StatusCode::OK);
        assert_eq!(
            gitforge_db::queries::JobQueries::get(&pool, job_id)
                .await
                .unwrap()
                .unwrap()
                .status,
            "succeeded"
        );
        assert_eq!(
            gitforge_db::queries::JobQueries::list_logs(&pool, job_id)
                .await
                .unwrap()[0]
                .chunk,
            "live output\n"
        );
        let stored = artifact_storage
            .get(gitforge_storage::ArtifactId::from(
                Uuid::parse_str(&artifact_id).unwrap(),
            ))
            .await
            .unwrap();
        assert_eq!(stored, artifact_body);
        let _ = tokio::fs::remove_dir_all(artifact_root).await;
    }

    #[tokio::test]
    async fn test_job_cancelled_probe_tracks_operator_cancellation() {
        let scheduler = crate::Scheduler::new();
        let state = create_state(scheduler);
        let job_id = JobId::new();

        assert!(!state.scheduler.is_cancelled(job_id).await);
        state.scheduler.cancel(job_id).await;

        let response = job_cancelled(
            axum::extract::State(state),
            axum::extract::Path(job_id.to_string()),
        )
        .await
        .into_response();
        assert_status(response, StatusCode::OK);
    }

    #[tokio::test]
    async fn test_assign_job_valid_uuid() {
        let scheduler = crate::Scheduler::new();
        let state = create_state(scheduler);
        let job_id = uuid::Uuid::new_v4();
        let runner_id = uuid::Uuid::new_v4();

        let response = assign_job(
            axum::extract::State(state),
            axum::extract::Path(job_id.to_string()),
            axum::Json(serde_json::json!({
                "runner_id": runner_id.to_string()
            })),
        )
        .await;

        assert_status(response.into_response(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_assign_job_invalid_uuid() {
        let scheduler = crate::Scheduler::new();
        let state = create_state(scheduler);

        let response = assign_job(
            axum::extract::State(state),
            axum::extract::Path("not-a-uuid".to_string()),
            axum::Json(serde_json::json!({
                "runner_id": "something"
            })),
        )
        .await;

        assert_status(response.into_response(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_complete_job_success_handler() {
        let scheduler = crate::Scheduler::new();
        let state = create_state(scheduler);
        let job_id = uuid::Uuid::new_v4();

        let response = complete_job(
            axum::extract::State(state),
            axum::extract::Path(job_id.to_string()),
            axum::Json(serde_json::json!({
                "success": true
            })),
        )
        .await;

        assert_status(response.into_response(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_complete_job_failure_handler() {
        let scheduler = crate::Scheduler::new();
        let state = create_state(scheduler);
        let job_id = uuid::Uuid::new_v4();

        let response = complete_job(
            axum::extract::State(state),
            axum::extract::Path(job_id.to_string()),
            axum::Json(serde_json::json!({
                "success": false,
                "error": "test error"
            })),
        )
        .await;

        assert_status(response.into_response(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_complete_job_invalid_uuid() {
        let scheduler = crate::Scheduler::new();
        let state = create_state(scheduler);

        let response = complete_job(
            axum::extract::State(state),
            axum::extract::Path("not-a-uuid".to_string()),
            axum::Json(serde_json::json!({
                "success": true
            })),
        )
        .await;

        assert_status(response.into_response(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn test_register_runner_request_deserialize() {
        let json = r#"{"name":"test-runner","type":"docker","capacity":4}"#;
        let request: RegisterRunnerRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.name, "test-runner");
        assert_eq!(request.runner_type, "docker");
        assert_eq!(request.capacity, 4);
    }

    #[test]
    fn test_pending_job_info_serialize() {
        let info = PendingJobInfo {
            contract_version: "harness.job.v1",
            job_id: "job-123".to_string(),
            name: "build".to_string(),
            pipeline_run_id: "run-456".to_string(),
            commands: vec!["cargo build".to_string()],
            working_dir: Some("/workspace".to_string()),
            runner_id: "runner-123".to_string(),
            lease_token: "lease-123".to_string(),
        };
        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains("job-123"));
        assert!(json.contains("build"));
    }

    #[test]
    fn test_create_state_fn() {
        let scheduler = crate::Scheduler::new();
        let state = create_state(scheduler);
        let _ = state.scheduler;
    }

    #[test]
    fn test_scheduler_routes_creation() {
        let scheduler = crate::Scheduler::new();
        let state = create_state(scheduler);
        let _routes: Router = scheduler_routes(state);
    }

    #[tokio::test]
    async fn test_register_runner_firecracker_handler() {
        let scheduler = crate::Scheduler::new();
        let state = create_state(scheduler);

        let request = RegisterRunnerRequest {
            name: "fire-runner".to_string(),
            runner_type: "firecracker".to_string(),
            capacity: 2,
        };

        let response = register_runner(axum::extract::State(state), axum::Json(request)).await;

        assert_status(response.into_response(), StatusCode::CREATED);
    }

    fn authenticated_test_state() -> SchedulerServerState {
        SchedulerServerState {
            scheduler: Arc::new(crate::Scheduler::new()),
            runner_auth_token: Some(Arc::from("runner-secret")),
            operator_auth_token: Some(Arc::from("operator-secret")),
            artifact_storage: None,
        }
    }

    #[tokio::test]
    async fn test_runner_token_can_read_runner_route() {
        let app: Router = scheduler_routes_with_tokens(
            authenticated_test_state(),
            Some(Arc::from("runner-secret")),
            Some(Arc::from("operator-secret")),
        );
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/jobs/pending")
                    .header("authorization", "Bearer runner-secret")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_runner_token_cannot_cancel_job() {
        let app: Router = scheduler_routes_with_tokens(
            authenticated_test_state(),
            Some(Arc::from("runner-secret")),
            Some(Arc::from("operator-secret")),
        );
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/jobs/{}/cancel", JobId::new()))
                    .header("authorization", "Bearer runner-secret")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_operator_token_can_cancel_job() {
        let state = authenticated_test_state();
        let job_id = JobId::new();
        state
            .scheduler
            .enqueue(
                job_id,
                gitforge_common::PipelineRunId::new(),
                gitforge_common::RepoId::new(),
            )
            .await;
        let app: Router = scheduler_routes_with_tokens(
            state,
            Some(Arc::from("runner-secret")),
            Some(Arc::from("operator-secret")),
        );
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/jobs/{job_id}/cancel"))
                    .header("authorization", "Bearer operator-secret")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_operator_cancel_unknown_job_is_not_found() {
        let app: Router = scheduler_routes_with_tokens(
            authenticated_test_state(),
            Some(Arc::from("runner-secret")),
            Some(Arc::from("operator-secret")),
        );
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/jobs/{}/cancel", JobId::new()))
                    .header("authorization", "Bearer operator-secret")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_operator_submit_unknown_pipeline_run_is_not_found() {
        let pool = gitforge_db::Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();
        let state = SchedulerServerState {
            scheduler: Arc::new(crate::Scheduler::with_db(pool)),
            runner_auth_token: Some(Arc::from("runner-secret")),
            operator_auth_token: Some(Arc::from("operator-secret")),
            artifact_storage: None,
        };
        let app: Router = scheduler_routes_with_tokens(
            state,
            Some(Arc::from("runner-secret")),
            Some(Arc::from("operator-secret")),
        );
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/jobs")
                    .header("authorization", "Bearer operator-secret")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(
                        serde_json::json!({
                            "pipeline_run_id": PipelineRunId::new().to_string(),
                            "repo_id": RepoId::new().to_string(),
                            "commands": ["/bin/true"],
                            "idempotency_key": "unknown-run-test"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_missing_runner_credential_is_service_unavailable() {
        let app: Router = scheduler_routes_with_tokens(
            authenticated_test_state(),
            None,
            Some(Arc::from("operator-secret")),
        );
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/jobs/pending")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }
}

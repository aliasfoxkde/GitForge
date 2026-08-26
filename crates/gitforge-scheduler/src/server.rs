//! Scheduler HTTP server

use crate::Scheduler;
use axum::{
    extract::Request,
    extract::{Path, Query, State},
    http::StatusCode,
    middleware::{self, Next},
    response::IntoResponse,
    response::Response,
    routing::{get, post},
    Json, Router,
};
use gitforge_common::{JobId, RunnerId};
use gitforge_db::models::{Runner, RunnerType};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

/// Application state for scheduler server
#[derive(Clone)]
pub struct SchedulerServerState {
    pub scheduler: Arc<Scheduler>,
    auth_token: Option<Arc<str>>,
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

/// Create scheduler server state
pub fn create_state(scheduler: Scheduler) -> SchedulerServerState {
    SchedulerServerState {
        scheduler: Arc::new(scheduler),
        auth_token: std::env::var("GITFORGE_SCHEDULER_TOKEN")
            .ok()
            .filter(|token| !token.is_empty())
            .map(Arc::from),
    }
}

/// Create scheduler routes
pub fn scheduler_routes<S: Clone + Send + Sync + 'static>(
    state: SchedulerServerState,
) -> Router<S> {
    let auth_token = state.auth_token.clone();
    Router::new()
        .route("/runners", post(register_runner))
        .route("/runners/{id}/heartbeat", post(runner_heartbeat))
        .route("/jobs/pending", get(get_pending_jobs))
        .route("/jobs/{id}/claim", post(claim_job))
        .route("/jobs/{id}/started", post(start_job))
        .route("/jobs/{id}/cancel", post(cancel_job))
        .route("/jobs/{id}/cancelled", get(job_cancelled))
        .route("/jobs/{id}/assign", post(assign_job))
        .route("/jobs/{id}/complete", post(complete_job))
        .route("/pipelines/runs/{id}", get(get_pipeline_run))
        .layer(middleware::from_fn(move |request, next: Next| {
            require_scheduler_auth(request, next, auth_token.clone())
        }))
        .with_state(state)
}

async fn require_scheduler_auth(
    request: Request,
    next: Next,
    expected: Option<Arc<str>>,
) -> Response {
    let Some(expected) = expected else {
        tracing::error!("GITFORGE_SCHEDULER_TOKEN is unset; refusing scheduler API access");
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
    use axum::http::StatusCode;

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
}

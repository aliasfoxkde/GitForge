//! Scheduler HTTP server

use crate::Scheduler;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use gitforce_common::{JobId, PipelineRunId, RepoId, RunnerId};
use gitforce_db::models::{Runner, RunnerType, SchedulerJob};
use gitforce_db::queries::SchedulerJobQueries;
use gitforce_db::Pool;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

/// Application state for scheduler server
#[derive(Clone)]
pub struct SchedulerServerState {
    pub scheduler: Arc<Scheduler>,
    pool: Option<Pool>,
    jobs: Arc<RwLock<HashMap<JobId, PendingJobInfo>>>,
    claimed: Arc<RwLock<HashSet<JobId>>>,
    completed: Arc<RwLock<HashMap<JobId, JobCompletion>>>,
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
#[derive(Debug, Clone, Serialize)]
pub struct PendingJobInfo {
    pub job_id: String,
    pub name: String,
    pub pipeline_run_id: String,
    pub commands: Vec<String>,
    pub working_dir: Option<String>,
}

/// Semantically observable result of a runner execution.
#[derive(Debug, Clone, Serialize)]
pub struct JobCompletion {
    pub job_id: String,
    pub status: String,
    pub success: bool,
    pub exit_code: i64,
    pub error: Option<String>,
    pub completed_at: String,
}

/// Request to enqueue a runnable synthetic/CI job.
#[derive(Debug, Deserialize)]
pub struct CreateJobRequest {
    pub job_id: Option<String>,
    pub pipeline_run_id: Option<String>,
    pub repo_id: Option<String>,
    pub name: String,
    pub commands: Vec<String>,
    pub working_dir: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PendingJobsQuery {
    runner_id: Option<String>,
}

/// Create scheduler server state
pub fn create_state(scheduler: Scheduler) -> SchedulerServerState {
    SchedulerServerState {
        scheduler: Arc::new(scheduler),
        pool: None,
        jobs: Arc::new(RwLock::new(HashMap::new())),
        claimed: Arc::new(RwLock::new(HashSet::new())),
        completed: Arc::new(RwLock::new(HashMap::new())),
    }
}

/// Build a production scheduler state from a migrated durable store.
pub async fn create_state_with_pool(
    scheduler: Scheduler,
    pool: Pool,
) -> gitforce_common::Result<SchedulerServerState> {
    let active = SchedulerJobQueries::list_active(&pool).await?;
    let state = SchedulerServerState {
        scheduler: Arc::new(scheduler),
        pool: Some(pool),
        jobs: Arc::new(RwLock::new(HashMap::new())),
        claimed: Arc::new(RwLock::new(HashSet::new())),
        completed: Arc::new(RwLock::new(HashMap::new())),
    };
    for job in active {
        let info = pending_info_from_job(&job);
        if job.status == "pending" {
            state
                .scheduler
                .enqueue(job.id, job.pipeline_run_id, job.repo_id)
                .await;
        } else if job.status == "claimed" {
            state.claimed.write().await.insert(job.id);
        }
        state.jobs.write().await.insert(job.id, info);
    }
    Ok(state)
}

fn pending_info_from_job(job: &SchedulerJob) -> PendingJobInfo {
    PendingJobInfo {
        job_id: job.id.to_string(),
        name: job.name.clone(),
        pipeline_run_id: job.pipeline_run_id.to_string(),
        commands: job.commands.clone(),
        working_dir: job.working_dir.clone(),
    }
}

fn completion_from_job(job: &SchedulerJob) -> Option<JobCompletion> {
    Some(JobCompletion {
        job_id: job.id.to_string(),
        status: job.status.clone(),
        success: job.success?,
        exit_code: job.exit_code.unwrap_or(if job.success? { 0 } else { -1 }),
        error: job.error.clone(),
        completed_at: job.completed_at?.to_rfc3339(),
    })
}

/// Create scheduler routes
pub fn scheduler_routes<S: Clone + Send + Sync + 'static>(
    state: SchedulerServerState,
) -> Router<S> {
    Router::new()
        .route("/runners", post(register_runner))
        .route("/runners/:id/heartbeat", post(runner_heartbeat))
        .route("/jobs", post(create_job))
        .route("/jobs/pending", get(get_pending_jobs))
        .route("/jobs/:id", get(get_job_status))
        .route("/jobs/:id/assign", post(assign_job))
        .route("/jobs/:id/complete", post(complete_job))
        .with_state(state)
}

/// Return the current status and, after completion, the runner result.
async fn get_job_status(
    State(state): State<SchedulerServerState>,
    Path(job_id): Path<String>,
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

    if let Some(completion) = state.completed.read().await.get(&job_id).cloned() {
        return (StatusCode::OK, Json(serde_json::json!(completion)));
    }

    if let Some(pool) = &state.pool {
        match SchedulerJobQueries::get(pool, job_id).await {
            Ok(Some(job)) if job.is_terminal() => {
                if let Some(completion) = completion_from_job(&job) {
                    return (StatusCode::OK, Json(serde_json::json!(completion)));
                }
            }
            Ok(Some(job)) => {
                return (
                    StatusCode::OK,
                    Json(serde_json::json!({
                        "job_id": job.id.to_string(),
                        "status": job.status
                    })),
                );
            }
            Ok(None) => {}
            Err(error) => {
                tracing::error!(%error, "failed to read durable scheduler job");
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({"error": "scheduler_storage_failure"})),
                );
            }
        }
    }

    if state.jobs.read().await.contains_key(&job_id) {
        return (
            StatusCode::OK,
            Json(serde_json::json!({
                "job_id": job_id.to_string(),
                "status": "queued"
            })),
        );
    }

    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({
            "error": "job_not_found",
            "job_id": job_id.to_string()
        })),
    )
}

/// Enqueue a runnable job for a registered runner.
async fn create_job(
    State(state): State<SchedulerServerState>,
    Json(request): Json<CreateJobRequest>,
) -> impl IntoResponse {
    let job_id = request
        .job_id
        .as_deref()
        .and_then(|value| Uuid::parse_str(value).ok().map(JobId::from))
        .unwrap_or_else(JobId::new);
    let pipeline_run_id = request
        .pipeline_run_id
        .as_deref()
        .and_then(|value| Uuid::parse_str(value).ok().map(PipelineRunId::from))
        .unwrap_or_else(PipelineRunId::new);
    let repo_id = request
        .repo_id
        .as_deref()
        .and_then(|value| Uuid::parse_str(value).ok().map(RepoId::from))
        .unwrap_or_else(RepoId::new);
    let info = PendingJobInfo {
        job_id: job_id.to_string(),
        name: request.name,
        pipeline_run_id: pipeline_run_id.to_string(),
        commands: request.commands,
        working_dir: request.working_dir,
    };

    if let Some(pool) = &state.pool {
        let mut durable = SchedulerJob::new(
            pipeline_run_id,
            repo_id,
            info.name.clone(),
            info.commands.clone(),
        )
        .with_working_dir(info.working_dir.clone());
        durable.id = job_id;
        match SchedulerJobQueries::insert_if_absent(pool, &durable).await {
            Ok(true) => {
                state
                    .scheduler
                    .enqueue(job_id, pipeline_run_id, repo_id)
                    .await
            }
            Ok(false) => {
                if let Ok(Some(existing)) = SchedulerJobQueries::get(pool, job_id).await {
                    let existing_info = pending_info_from_job(&existing);
                    state
                        .jobs
                        .write()
                        .await
                        .insert(job_id, existing_info.clone());
                    return (
                        StatusCode::OK,
                        Json(serde_json::to_value(existing_info).unwrap_or_default()),
                    );
                }
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({"error":"scheduler_storage_failure"})),
                );
            }
            Err(error) => {
                tracing::error!(%error, "failed to persist scheduler job");
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({"error":"scheduler_storage_failure"})),
                );
            }
        }
    } else {
        state
            .scheduler
            .enqueue(job_id, pipeline_run_id, repo_id)
            .await;
    }
    state.jobs.write().await.insert(job_id, info.clone());

    (
        StatusCode::CREATED,
        Json(serde_json::to_value(info).unwrap_or_default()),
    )
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

    state.scheduler.heartbeat(runner_id).await;

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "status": "ok"
        })),
    )
}

/// Get pending jobs for a runner
async fn get_pending_jobs(
    State(state): State<SchedulerServerState>,
    Query(query): Query<PendingJobsQuery>,
) -> impl IntoResponse {
    state.scheduler.process_queue().await;
    let requested_runner = query
        .runner_id
        .as_deref()
        .and_then(|value| Uuid::parse_str(value).ok().map(RunnerId::from));
    let jobs = state.jobs.read().await;
    let mut claimed = state.claimed.write().await;
    let mut pending = Vec::new();
    for (job_id, info) in jobs.iter() {
        if claimed.contains(job_id) {
            continue;
        }
        let Some(assigned_runner) = state.scheduler.is_assigned(*job_id).await else {
            continue;
        };
        if requested_runner.is_some_and(|runner| runner != assigned_runner) {
            continue;
        }
        claimed.insert(*job_id);
        if let Some(pool) = &state.pool {
            let Some(runner_id) = requested_runner.or(Some(assigned_runner)) else {
                continue;
            };
            match SchedulerJobQueries::mark_claimed(pool, *job_id, runner_id).await {
                Ok(true) => pending.push(info.clone()),
                Ok(false) => {
                    claimed.remove(job_id);
                }
                Err(error) => {
                    claimed.remove(job_id);
                    tracing::error!(%error, "failed to persist scheduler claim");
                }
            }
        } else {
            pending.push(info.clone());
        }
    }
    Json(pending)
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
    let exit_code = request["exit_code"]
        .as_i64()
        .unwrap_or(if success { 0 } else { -1 });
    let error = request["error"].as_str().map(str::to_string);

    if let Some(completion) = state.completed.read().await.get(&job_id).cloned() {
        return (StatusCode::OK, Json(serde_json::json!(completion)));
    }
    if state.pool.is_none() && !state.jobs.read().await.contains_key(&job_id) {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": "job_not_found",
                "job_id": job_id.to_string()
            })),
        );
    }

    if let Some(pool) = &state.pool {
        match SchedulerJobQueries::complete(pool, job_id, success, exit_code, error.clone()).await {
            Ok(Some(job)) if job.is_terminal() => {
                if let Some(completion) = completion_from_job(&job) {
                    state
                        .completed
                        .write()
                        .await
                        .insert(job_id, completion.clone());
                    state.jobs.write().await.remove(&job_id);
                    state.claimed.write().await.remove(&job_id);
                    state.scheduler.complete(job_id).await;
                    return (StatusCode::OK, Json(serde_json::json!(completion)));
                }
            }
            Ok(None) => {
                return (
                    StatusCode::NOT_FOUND,
                    Json(serde_json::json!({"error":"job_not_found","job_id":job_id.to_string()})),
                )
            }
            Ok(Some(_)) => {}
            Err(error) => {
                tracing::error!(%error, "failed to persist scheduler completion");
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({"error":"scheduler_storage_failure"})),
                );
            }
        }
    }

    let completion = JobCompletion {
        job_id: job_id.to_string(),
        status: if success { "succeeded" } else { "failed" }.to_string(),
        success,
        exit_code,
        error,
        completed_at: chrono::Utc::now().to_rfc3339(),
    };
    state.scheduler.complete(job_id).await;
    state.jobs.write().await.remove(&job_id);
    state.claimed.write().await.remove(&job_id);
    state
        .completed
        .write()
        .await
        .insert(job_id, completion.clone());
    tracing::info!("job {} completed via HTTP: success={}", job_id, success);

    (StatusCode::OK, Json(serde_json::json!(completion)))
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
        let runner_id = uuid::Uuid::new_v4();

        let response = runner_heartbeat(
            axum::extract::State(state),
            axum::extract::Path(runner_id.to_string()),
        )
        .await;

        assert_status(response.into_response(), StatusCode::OK);
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
        state.jobs.write().await.insert(
            JobId::from(job_id),
            PendingJobInfo {
                job_id: job_id.to_string(),
                name: "success".to_string(),
                pipeline_run_id: uuid::Uuid::new_v4().to_string(),
                commands: vec!["true".to_string()],
                working_dir: None,
            },
        );

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
        state.jobs.write().await.insert(
            JobId::from(job_id),
            PendingJobInfo {
                job_id: job_id.to_string(),
                name: "failure".to_string(),
                pipeline_run_id: uuid::Uuid::new_v4().to_string(),
                commands: vec!["false".to_string()],
                working_dir: None,
            },
        );

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

    #[tokio::test]
    async fn test_completion_is_observable_through_status_endpoint() {
        let scheduler = crate::Scheduler::new();
        let state = create_state(scheduler);
        let job_id = uuid::Uuid::new_v4();
        state.jobs.write().await.insert(
            JobId::from(job_id),
            PendingJobInfo {
                job_id: job_id.to_string(),
                name: "semantic-canary".to_string(),
                pipeline_run_id: uuid::Uuid::new_v4().to_string(),
                commands: vec!["exit 7".to_string()],
                working_dir: None,
            },
        );

        let completion = complete_job(
            axum::extract::State(state.clone()),
            axum::extract::Path(job_id.to_string()),
            axum::Json(serde_json::json!({
                "success": false,
                "exit_code": 7,
                "error": "intentional canary failure"
            })),
        )
        .await
        .into_response();
        assert_status(completion, StatusCode::OK);

        let status = get_job_status(
            axum::extract::State(state),
            axum::extract::Path(job_id.to_string()),
        )
        .await
        .into_response();
        assert_eq!(status.status(), StatusCode::OK);
        let body = axum::body::to_bytes(status.into_body(), usize::MAX)
            .await
            .unwrap();
        let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(payload["status"], "failed");
        assert_eq!(payload["success"], false);
        assert_eq!(payload["exit_code"], 7);
        assert_eq!(payload["error"], "intentional canary failure");
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
            job_id: "job-123".to_string(),
            name: "build".to_string(),
            pipeline_run_id: "run-456".to_string(),
            commands: vec!["cargo build".to_string()],
            working_dir: Some("/workspace".to_string()),
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

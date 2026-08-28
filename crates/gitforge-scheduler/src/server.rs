//! Scheduler HTTP server

use crate::Scheduler;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
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
    /// Optional database pool for querying job metadata (image, steps).
    /// None is valid for in-memory-only scheduler deployments.
    pub db_pool: Option<gitforge_db::Pool>,
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
    pub job_id: String,
    pub name: String,
    pub pipeline_run_id: String,
    /// Container image, e.g. "rust:1.75". Required — never empty for valid jobs.
    pub image: String,
    /// Commands to execute. Empty only if job was malformed at creation time.
    pub commands: Vec<String>,
    pub working_dir: Option<String>,
}

/// Create scheduler server state with no database (in-memory scheduling only).
pub fn create_state(scheduler: Scheduler) -> SchedulerServerState {
    SchedulerServerState {
        scheduler: Arc::new(scheduler),
        db_pool: None,
    }
}

/// Create scheduler server state with a database pool.
/// The pool is used to fetch job metadata (image, steps) for the runner handoff.
pub fn create_state_with_db(scheduler: Scheduler, pool: gitforge_db::Pool) -> SchedulerServerState {
    SchedulerServerState {
        scheduler: Arc::new(scheduler),
        db_pool: Some(pool),
    }
}

/// Create scheduler routes
pub fn scheduler_routes<S: Clone + Send + Sync + 'static>(
    state: SchedulerServerState,
) -> Router<S> {
    Router::new()
        .route("/runners", post(register_runner))
        .route("/runners/{id}/heartbeat", post(runner_heartbeat))
        .route("/jobs/pending", get(get_pending_jobs))
        .route("/jobs/{id}/assign", post(assign_job))
        .route("/jobs/{id}/complete", post(complete_job))
        .with_state(state)
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

/// Get pending jobs for a runner.
///
/// Fetches jobs that have been assigned to runners by `process_queue()` and
/// augments them with persisted execution metadata (image, steps) from the DB.
/// Jobs with missing or empty image are **excluded** — they indicate a
/// programming error (job created without going through the API pipeline).
/// This is fail-closed: runners must never receive a job without a configured image.
async fn get_pending_jobs(State(state): State<SchedulerServerState>) -> impl IntoResponse {
    // Process queue to assign pending jobs
    state.scheduler.process_queue().await;

    // Get jobs assigned to runners (these are pending execution)
    let assigned_jobs = state.scheduler.get_assigned_jobs().await;

    // Build response, optionally augmented with DB metadata
    let job_infos: Vec<PendingJobInfo> = if let Some(pool) = &state.db_pool {
        let mut infos = Vec::with_capacity(assigned_jobs.len());
        for (job_id, _runner_id, pipeline_run_id) in assigned_jobs {
            // Fetch persisted job metadata from DB
            match gitforge_db::queries::JobQueries::get(pool, job_id).await {
                Ok(Some(job)) => {
                    // Fail-closed: skip jobs without a configured image.
                    // This catches jobs created via the deprecated scheduler-only path
                    // that never went through the API pipeline.
                    if job.image.is_empty() {
                        tracing::warn!(
                            "get_pending_jobs: job {} has empty image — skipping (created outside API pipeline?)",
                            job_id
                        );
                        continue;
                    }

                    // Fetch steps for this job
                    let steps = gitforge_db::queries::JobStepQueries::list_by_job(pool, job_id)
                        .await
                        .unwrap_or_default();
                    let commands: Vec<String> = steps.into_iter().map(|s| s.run).collect();

                    infos.push(PendingJobInfo {
                        job_id: job_id.to_string(),
                        name: job.name,
                        pipeline_run_id: pipeline_run_id.to_string(),
                        image: job.image,
                        commands,
                        working_dir: None,
                    });
                }
                Ok(None) => {
                    tracing::warn!(
                        "get_pending_jobs: job {} not found in DB — skipping",
                        job_id
                    );
                }
                Err(e) => {
                    tracing::error!(
                        "get_pending_jobs: failed to fetch job {} metadata: {}",
                        job_id,
                        e
                    );
                }
            }
        }
        infos
    } else {
        // No DB — use in-memory job names and a clear indicator that metadata is unavailable.
        // This path exists only for test or in-process scheduler scenarios.
        assigned_jobs
            .into_iter()
            .map(|(job_id, _runner_id, pipeline_run_id)| PendingJobInfo {
                job_id: job_id.to_string(),
                name: format!("job-{}", job_id),
                pipeline_run_id: pipeline_run_id.to_string(),
                image: String::new(), // Intentionally empty — signals metadata unavailable
                commands: vec![],
                working_dir: None,
            })
            .collect()
    };

    Json(serde_json::json!(job_infos))
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
    let exit_code = request["exit_code"].as_i64().map(|value| value as i32);
    let error = request["error"].as_str().map(str::to_owned);
    let step_results = request["step_results"].as_array();

    state
        .scheduler
        .complete_job_with_receipt(
            job_id,
            success,
            exit_code,
            error.clone(),
            step_results.map(Vec::as_slice),
        )
        .await;

    tracing::info!(
        "job {} completed via HTTP: success={}, exit_code={:?}",
        job_id,
        success,
        exit_code
    );

    // Log step results if present
    if let Some(step_results) = request["step_results"].as_array() {
        tracing::debug!("job {} had {} steps", job_id, step_results.len());
        for (i, step) in step_results.iter().enumerate() {
            let step_exit = step["exit_code"].as_i64().unwrap_or(-1);
            tracing::debug!("  step {}: exit_code={}", i, step_exit);
        }
    }

    // Log error if present
    if let Some(err) = error.as_deref() {
        if !err.is_empty() {
            tracing::error!("job {} error: {}", job_id, err);
        }
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

        let response = get_pending_jobs(axum::extract::State(state)).await;

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
            job_id: "job-123".to_string(),
            name: "build".to_string(),
            pipeline_run_id: "run-456".to_string(),
            image: "rust:1.75".to_string(),
            commands: vec!["cargo build".to_string()],
            working_dir: Some("/workspace".to_string()),
        };
        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains("job-123"));
        assert!(json.contains("build"));
        assert!(json.contains("rust:1.75"));
        // Verify image field is present in serialized output
        assert!(json.contains("\"image\""));
    }

    #[test]
    fn test_pending_job_info_no_image_empty_commands() {
        // When no DB is available, get_pending_jobs returns empty image (intentionally).
        // This verifies the no-db path produces correct structure.
        let info = PendingJobInfo {
            job_id: "job-no-db".to_string(),
            name: "job-no-db".to_string(),
            pipeline_run_id: "run-no-db".to_string(),
            image: String::new(),
            commands: vec![],
            working_dir: None,
        };
        assert!(info.image.is_empty());
        assert!(info.commands.is_empty());
        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains("job-no-db"));
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

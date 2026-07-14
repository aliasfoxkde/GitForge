//! Scheduler HTTP server

use crate::Scheduler;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use gitforce_common::{JobId, RunnerId};
use gitforce_db::models::{Runner, RunnerType};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

/// Application state for scheduler server
#[derive(Clone)]
pub struct SchedulerServerState {
    pub scheduler: Arc<Scheduler>,
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
    pub commands: Vec<String>,
    pub working_dir: Option<String>,
}

/// Create scheduler server state
pub fn create_state(scheduler: Scheduler) -> SchedulerServerState {
    SchedulerServerState {
        scheduler: Arc::new(scheduler),
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

    (StatusCode::CREATED, Json(serde_json::json!({
        "id": runner.id.to_string(),
        "name": runner.name,
        "type": runner.runner_type,
        "status": runner.status,
        "capacity": runner.capacity,
    })))
}

/// Runner heartbeat
async fn runner_heartbeat(
    State(state): State<SchedulerServerState>,
    Path(runner_id): Path<String>,
) -> impl IntoResponse {
    let runner_id: RunnerId = match Uuid::parse_str(&runner_id) {
        Ok(id) => RunnerId::from(id),
        Err(_) => {
            return (StatusCode::BAD_REQUEST, Json(serde_json::json!({
                "error": "invalid_runner_id",
                "message": "Invalid runner ID format"
            })))
        }
    };

    state.scheduler.heartbeat(runner_id).await;

    (StatusCode::OK, Json(serde_json::json!({
        "status": "ok"
    })))
}

/// Get pending jobs for a runner
async fn get_pending_jobs(
    State(state): State<SchedulerServerState>,
) -> impl IntoResponse {
    // Process queue to assign pending jobs
    state.scheduler.process_queue().await;

    // Return empty array - actual job info would come from database
    // In real implementation, this would query job details from DB
    Json(serde_json::json!([]))
}

/// Assign a job to a runner (runner claims a job)
async fn assign_job(
    State(state): State<SchedulerServerState>,
    Path(job_id): Path<String>,
    Json(request): Json<serde_json::Value>,
) -> impl IntoResponse {
    let job_id: JobId = match Uuid::parse_str(&job_id) {
        Ok(id) => JobId::from(id),
        Err(_) => {
            return (StatusCode::BAD_REQUEST, Json(serde_json::json!({
                "error": "invalid_job_id",
                "message": "Invalid job ID format"
            })))
        }
    };

    let runner_id: Option<RunnerId> = request["runner_id"]
        .as_str()
        .and_then(|s| Uuid::parse_str(s).ok().map(RunnerId::from));

    if let Some(r_id) = runner_id {
        // In real impl, mark job as assigned to this runner
        tracing::info!("job {} assigned to runner {} via HTTP", job_id, r_id);
    }

    (StatusCode::OK, Json(serde_json::json!({
        "status": "assigned",
        "job_id": job_id.to_string()
    })))
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
            return (StatusCode::BAD_REQUEST, Json(serde_json::json!({
                "error": "invalid_job_id",
                "message": "Invalid job ID format"
            })))
        }
    };

    let success = request["success"].as_bool().unwrap_or(false);
    tracing::info!("job {} completed via HTTP: success={}", job_id, success);

    (StatusCode::OK, Json(serde_json::json!({
        "status": "completed",
        "job_id": job_id.to_string()
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

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
}

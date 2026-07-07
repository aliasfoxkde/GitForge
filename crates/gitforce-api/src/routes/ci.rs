//! CI API routes

use axum::{
    extract::Path,
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use gitforce_common::PipelineId;
use serde::{Deserialize, Serialize};

/// Pipeline run response
#[derive(Debug, Serialize, Deserialize)]
pub struct PipelineRunResponse {
    pub id: String,
    pub pipeline_id: String,
    pub status: String,
    pub commit_hash: String,
    pub triggered_by: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
}

/// Job response
#[derive(Debug, Serialize, Deserialize)]
pub struct JobResponse {
    pub id: String,
    pub name: String,
    pub status: String,
    pub runner_id: Option<String>,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
}

/// CI routes
pub fn ci_routes<S: Clone + Send + Sync + 'static>() -> Router<S> {
    Router::new()
        .route("/pipelines", get(list_pipelines))
        .route("/pipelines/{id}", get(get_pipeline))
        .route("/pipeline-runs", get(list_pipeline_runs))
        .route("/pipeline-runs/{id}", get(get_pipeline_run))
        .route("/pipeline-runs/{id}/jobs", get(get_pipeline_run_jobs))
        .route("/jobs/{id}", get(get_job))
        .route("/jobs/{id}/logs", get(get_job_logs))
}

/// List pipelines
async fn list_pipelines() -> impl IntoResponse {
    Json(serde_json::Value::Array(vec![]))
}

/// Get pipeline
async fn get_pipeline(Path(id): Path<String>) -> impl IntoResponse {
    tracing::debug!("get pipeline: {}", id);
    (StatusCode::OK, Json(serde_json::json!({
        "id": id,
        "name": "test-pipeline"
    })))
}

/// List pipeline runs
async fn list_pipeline_runs() -> impl IntoResponse {
    Json(serde_json::Value::Array(vec![]))
}

/// Get pipeline run
async fn get_pipeline_run(Path(id): Path<String>) -> impl IntoResponse {
    tracing::debug!("get pipeline run: {}", id);
    (StatusCode::OK, Json(PipelineRunResponse {
        id,
        pipeline_id: PipelineId::new().to_string(),
        status: "running".to_string(),
        commit_hash: "abc123".to_string(),
        triggered_by: "push".to_string(),
        started_at: Some(chrono::Utc::now().to_rfc3339()),
        finished_at: None,
    }))
}

/// Get pipeline run jobs
async fn get_pipeline_run_jobs(Path(id): Path<String>) -> impl IntoResponse {
    tracing::debug!("get pipeline run jobs: {}", id);
    Json(serde_json::Value::Array(vec![]))
}

/// Get job
async fn get_job(Path(id): Path<String>) -> impl IntoResponse {
    tracing::debug!("get job: {}", id);
    (StatusCode::OK, Json(JobResponse {
        id,
        name: "build".to_string(),
        status: "running".to_string(),
        runner_id: Some("runner-1".to_string()),
        started_at: Some(chrono::Utc::now().to_rfc3339()),
        finished_at: None,
    }))
}

/// Get job logs
async fn get_job_logs(Path(id): Path<String>) -> impl IntoResponse {
    tracing::debug!("get job logs: {}", id);
    (StatusCode::OK, Json(serde_json::json!({
        "job_id": id,
        "logs": "Building...\nRunning tests...\nAll tests passed!"
    })))
}

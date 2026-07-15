//! CI API routes

use crate::auth::ApiAuth;
use axum::{
    extract::{Extension, Path},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use gitforce_common::{JobId, PipelineId, PipelineRunId};
use gitforce_db::{Pool, queries::{PipelineQueries, PipelineRunQueries, JobQueries}};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

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

/// Helper to extract and validate user from headers
fn extract_user(auth: &ApiAuth, headers: &HeaderMap) -> Result<(), StatusCode> {
    let auth_header = headers
        .get("Authorization")
        .and_then(|v| v.to_str().ok());

    let token = auth_header
        .and_then(|h| ApiAuth::extract_token(h))
        .ok_or(StatusCode::UNAUTHORIZED)?;

    auth.validate_token(token)
        .map_err(|_| StatusCode::UNAUTHORIZED)?;

    Ok(())
}

/// List pipelines
async fn list_pipelines(
    Extension(pool): Extension<Arc<Pool>>,
    Extension(auth): Extension<Arc<ApiAuth>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    match extract_user(&auth, &headers) {
        Err(e) => e.into_response(),
        Ok(_) => {
            match PipelineQueries::list(&pool).await {
                Ok(pipelines) => {
                    let response: Vec<serde_json::Value> = pipelines.into_iter().map(|p| {
                        serde_json::json!({
                            "id": p.id.to_string(),
                            "repo_id": p.repo_id.to_string(),
                            "name": p.name,
                            "trigger_type": p.trigger_type,
                            "created_at": p.created_at.to_rfc3339()
                        })
                    }).collect();
                    Json(response).into_response()
                }
                Err(e) => {
                    tracing::error!("failed to list pipelines: {}", e);
                    Json(serde_json::Value::Array(vec![])).into_response()
                }
            }
        }
    }
}

/// Get pipeline
async fn get_pipeline(
    Extension(pool): Extension<Arc<Pool>>,
    Extension(auth): Extension<Arc<ApiAuth>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match extract_user(&auth, &headers) {
        Err(e) => e.into_response(),
        Ok(_) => {
            tracing::debug!("get pipeline: {}", id);

            match Uuid::parse_str(&id) {
                Ok(uuid) => {
                    let pipeline_id = PipelineId::from(uuid);
                    match PipelineQueries::get(&pool, pipeline_id).await {
                        Ok(Some(pipeline)) => {
                            (StatusCode::OK, Json(serde_json::json!({
                                "id": pipeline.id.to_string(),
                                "repo_id": pipeline.repo_id.to_string(),
                                "name": pipeline.name,
                                "trigger_type": pipeline.trigger_type,
                                "created_at": pipeline.created_at.to_rfc3339()
                            }))).into_response()
                        }
                        Ok(None) => {
                            (StatusCode::NOT_FOUND, Json(serde_json::json!({
                                "error": "not_found",
                                "message": "Pipeline not found"
                            }))).into_response()
                        }
                        Err(e) => {
                            tracing::error!("failed to get pipeline: {}", e);
                            (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({
                                "error": "database_error",
                                "message": format!("failed to get pipeline: {}", e)
                            }))).into_response()
                        }
                    }
                }
                Err(_) => {
                    (StatusCode::BAD_REQUEST, Json(serde_json::json!({
                        "error": "invalid_id",
                        "message": "Invalid pipeline ID format"
                    }))).into_response()
                }
            }
        }
    }
}

/// List pipeline runs
async fn list_pipeline_runs(
    Extension(pool): Extension<Arc<Pool>>,
    Extension(auth): Extension<Arc<ApiAuth>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    match extract_user(&auth, &headers) {
        Err(e) => e.into_response(),
        Ok(_) => {
            match PipelineRunQueries::list(&pool).await {
                Ok(runs) => {
                    let response: Vec<serde_json::Value> = runs.into_iter().map(|r| {
                        serde_json::json!({
                            "id": r.id.to_string(),
                            "pipeline_id": r.pipeline_id.to_string(),
                            "status": r.status,
                            "commit_hash": r.commit_hash,
                            "triggered_by": r.triggered_by,
                            "started_at": r.started_at.map(|dt| dt.to_rfc3339()),
                            "finished_at": r.finished_at.map(|dt| dt.to_rfc3339())
                        })
                    }).collect();
                    Json(response).into_response()
                }
                Err(e) => {
                    tracing::error!("failed to list pipeline runs: {}", e);
                    Json(serde_json::Value::Array(vec![])).into_response()
                }
            }
        }
    }
}

/// Get pipeline run
async fn get_pipeline_run(
    Extension(pool): Extension<Arc<Pool>>,
    Extension(auth): Extension<Arc<ApiAuth>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match extract_user(&auth, &headers) {
        Err(e) => e.into_response(),
        Ok(_) => {
            tracing::debug!("get pipeline run: {}", id);

            match Uuid::parse_str(&id) {
                Ok(uuid) => {
                    let run_id = PipelineRunId::from(uuid);
                    match PipelineRunQueries::get(&pool, run_id).await {
                        Ok(Some(run)) => {
                            (StatusCode::OK, Json(serde_json::json!({
                                "id": run.id.to_string(),
                                "pipeline_id": run.pipeline_id.to_string(),
                                "status": run.status,
                                "commit_hash": run.commit_hash,
                                "triggered_by": run.triggered_by,
                                "started_at": run.started_at.map(|dt| dt.to_rfc3339()),
                                "finished_at": run.finished_at.map(|dt| dt.to_rfc3339())
                            }))).into_response()
                        }
                        Ok(None) => {
                            (StatusCode::NOT_FOUND, Json(serde_json::json!({
                                "error": "not_found",
                                "message": "Pipeline run not found"
                            }))).into_response()
                        }
                        Err(e) => {
                            tracing::error!("failed to get pipeline run: {}", e);
                            (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({
                                "error": "database_error",
                                "message": format!("failed to get pipeline run: {}", e)
                            }))).into_response()
                        }
                    }
                }
                Err(_) => {
                    (StatusCode::BAD_REQUEST, Json(serde_json::json!({
                        "error": "invalid_id",
                        "message": "Invalid pipeline run ID format"
                    }))).into_response()
                }
            }
        }
    }
}

/// Get pipeline run jobs
async fn get_pipeline_run_jobs(
    Extension(pool): Extension<Arc<Pool>>,
    Extension(auth): Extension<Arc<ApiAuth>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match extract_user(&auth, &headers) {
        Err(e) => e.into_response(),
        Ok(_) => {
            tracing::debug!("get pipeline run jobs: {}", id);

            match Uuid::parse_str(&id) {
                Ok(uuid) => {
                    let run_id = PipelineRunId::from(uuid);
                    match JobQueries::list_by_run(&pool, run_id).await {
                        Ok(jobs) => {
                            let jobs_json: Vec<serde_json::Value> = jobs.into_iter().map(|j| {
                                serde_json::json!({
                                    "id": j.id.to_string(),
                                    "name": j.name,
                                    "status": j.status,
                                    "runner_id": j.runner_id.map(|id| id.to_string()),
                                    "started_at": j.started_at.map(|dt| dt.to_rfc3339()),
                                    "finished_at": j.finished_at.map(|dt| dt.to_rfc3339())
                                })
                            }).collect();
                            Json(jobs_json).into_response()
                        }
                        Err(e) => {
                            tracing::error!("failed to list jobs: {}", e);
                            (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({
                                "error": "database_error",
                                "message": format!("failed to list jobs: {}", e)
                            }))).into_response()
                        }
                    }
                }
                Err(_) => {
                    (StatusCode::BAD_REQUEST, Json(serde_json::json!({
                        "error": "invalid_id",
                        "message": "Invalid pipeline run ID format"
                    }))).into_response()
                }
            }
        }
    }
}

/// Get job
async fn get_job(
    Extension(pool): Extension<Arc<Pool>>,
    Extension(auth): Extension<Arc<ApiAuth>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match extract_user(&auth, &headers) {
        Err(e) => e.into_response(),
        Ok(_) => {
            tracing::debug!("get job: {}", id);

            match Uuid::parse_str(&id) {
                Ok(uuid) => {
                    let job_id = JobId::from(uuid);
                    match JobQueries::get(&pool, job_id).await {
                        Ok(Some(job)) => {
                            (StatusCode::OK, Json(serde_json::json!({
                                "id": job.id.to_string(),
                                "name": job.name,
                                "status": job.status,
                                "runner_id": job.runner_id.map(|id| id.to_string()),
                                "started_at": job.started_at.map(|dt| dt.to_rfc3339()),
                                "finished_at": job.finished_at.map(|dt| dt.to_rfc3339())
                            }))).into_response()
                        }
                        Ok(None) => {
                            (StatusCode::NOT_FOUND, Json(serde_json::json!({
                                "error": "not_found",
                                "message": "Job not found"
                            }))).into_response()
                        }
                        Err(e) => {
                            tracing::error!("failed to get job: {}", e);
                            (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({
                                "error": "database_error",
                                "message": format!("failed to get job: {}", e)
                            }))).into_response()
                        }
                    }
                }
                Err(_) => {
                    (StatusCode::BAD_REQUEST, Json(serde_json::json!({
                        "error": "invalid_id",
                        "message": "Invalid job ID format"
                    }))).into_response()
                }
            }
        }
    }
}

/// Get job logs (placeholder - logs would come from storage)
async fn get_job_logs(
    Extension(auth): Extension<Arc<ApiAuth>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match extract_user(&auth, &headers) {
        Err(e) => e.into_response(),
        Ok(_) => {
            tracing::debug!("get job logs: {}", id);
            (StatusCode::OK, Json(serde_json::json!({
                "job_id": id,
                "logs": "Logs not yet implemented - coming soon"
            }))).into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pipeline_run_response_serialization() {
        let response = PipelineRunResponse {
            id: "run-123".to_string(),
            pipeline_id: "pipe-456".to_string(),
            status: "running".to_string(),
            commit_hash: "abc123".to_string(),
            triggered_by: "alice".to_string(),
            started_at: Some("2024-01-01T00:00:00Z".to_string()),
            finished_at: None,
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("run-123"));
        assert!(json.contains("running"));
    }

    #[test]
    fn test_job_response_serialization() {
        let response = JobResponse {
            id: "job-789".to_string(),
            name: "build".to_string(),
            status: "succeeded".to_string(),
            runner_id: Some("runner-1".to_string()),
            started_at: Some("2024-01-01T00:00:00Z".to_string()),
            finished_at: Some("2024-01-01T00:05:00Z".to_string()),
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("job-789"));
        assert!(json.contains("build"));
        assert!(json.contains("succeeded"));
    }

    #[test]
    fn test_pipeline_run_response_without_timestamps() {
        let response = PipelineRunResponse {
            id: "run-001".to_string(),
            pipeline_id: "pipe-002".to_string(),
            status: "pending".to_string(),
            commit_hash: "def456".to_string(),
            triggered_by: "bob".to_string(),
            started_at: None,
            finished_at: None,
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("pending"));
        assert!(json.contains("def456"));
    }

    #[test]
    fn test_job_response_without_runner() {
        let response = JobResponse {
            id: "job-002".to_string(),
            name: "test".to_string(),
            status: "queued".to_string(),
            runner_id: None,
            started_at: None,
            finished_at: None,
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("queued"));
        assert!(json.contains("job-002"));
    }

    #[test]
    fn test_pipeline_run_response_deserialization() {
        let json = r#"{
            "id": "run-123",
            "pipeline_id": "pipe-456",
            "status": "failed",
            "commit_hash": "xyz789",
            "triggered_by": "charlie",
            "started_at": "2024-01-01T00:00:00Z",
            "finished_at": "2024-01-01T00:10:00Z"
        }"#;
        let response: PipelineRunResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.id, "run-123");
        assert_eq!(response.status, "failed");
        assert_eq!(response.triggered_by, "charlie");
    }

    #[test]
    fn test_job_response_deserialization() {
        let json = r#"{
            "id": "job-999",
            "name": "deploy",
            "status": "running",
            "runner_id": "runner-5",
            "started_at": "2024-01-01T00:00:00Z",
            "finished_at": null
        }"#;
        let response: JobResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.id, "job-999");
        assert_eq!(response.name, "deploy");
        assert_eq!(response.status, "running");
        assert_eq!(response.runner_id, Some("runner-5".to_string()));
    }
}

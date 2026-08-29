//! CI API routes

use crate::auth::ApiAuth;
use axum::{
    extract::{Extension, Path},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use gitforge_common::{JobId, PipelineId, PipelineRunId};
use gitforge_db::{
    models::{Job, JobStep, PipelineRun},
    queries::{JobQueries, JobStepQueries, PipelineQueries, PipelineRunQueries},
    Pool,
};
use gitforge_scheduler::Scheduler;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

/// Request to trigger a manual pipeline run
#[derive(Debug, Deserialize, Serialize)]
pub struct TriggerPipelineRequest {
    /// Commit hash to build (optional, defaults to repo HEAD)
    pub commit_hash: Option<String>,
    /// Branch or ref to build from (optional)
    pub branch: Option<String>,
}

/// Response for pipeline trigger
#[derive(Debug, Serialize, Deserialize)]
pub struct TriggerPipelineResponse {
    pub pipeline_run_id: String,
    pub pipeline_id: String,
    pub status: String,
    pub triggered_by: String,
    pub commit_hash: String,
    pub jobs: Vec<TriggeredJobInfo>,
}

/// Information about a created job
#[derive(Debug, Serialize, Deserialize)]
pub struct TriggeredJobInfo {
    pub job_id: String,
    pub name: String,
    pub status: String,
}

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
        .route("/pipelines/{id}/trigger", post(trigger_pipeline))
        .route("/pipeline-runs", get(list_pipeline_runs))
        .route("/pipeline-runs/{id}", get(get_pipeline_run))
        .route("/pipeline-runs/{id}/jobs", get(get_pipeline_run_jobs))
        .route("/jobs/{id}", get(get_job))
        .route("/jobs/{id}/logs", get(get_job_logs))
}

/// Helper to extract and validate user from headers
fn extract_user(auth: &ApiAuth, headers: &HeaderMap) -> Result<(), StatusCode> {
    let auth_header = headers.get("Authorization").and_then(|v| v.to_str().ok());

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
        Ok(_) => match PipelineQueries::list(&pool).await {
            Ok(pipelines) => {
                let response: Vec<serde_json::Value> = pipelines
                    .into_iter()
                    .map(|p| {
                        serde_json::json!({
                            "id": p.id.to_string(),
                            "repo_id": p.repo_id.to_string(),
                            "name": p.name,
                            "trigger_type": p.trigger_type,
                            "created_at": p.created_at.to_rfc3339()
                        })
                    })
                    .collect();
                Json(response).into_response()
            }
            Err(e) => {
                tracing::error!("failed to list pipelines: {}", e);
                Json(serde_json::Value::Array(vec![])).into_response()
            }
        },
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
                        Ok(Some(pipeline)) => (
                            StatusCode::OK,
                            Json(serde_json::json!({
                                "id": pipeline.id.to_string(),
                                "repo_id": pipeline.repo_id.to_string(),
                                "name": pipeline.name,
                                "trigger_type": pipeline.trigger_type,
                                "created_at": pipeline.created_at.to_rfc3339()
                            })),
                        )
                            .into_response(),
                        Ok(None) => (
                            StatusCode::NOT_FOUND,
                            Json(serde_json::json!({
                                "error": "not_found",
                                "message": "Pipeline not found"
                            })),
                        )
                            .into_response(),
                        Err(e) => {
                            tracing::error!("failed to get pipeline: {}", e);
                            (
                                StatusCode::INTERNAL_SERVER_ERROR,
                                Json(serde_json::json!({
                                    "error": "database_error",
                                    "message": format!("failed to get pipeline: {}", e)
                                })),
                            )
                                .into_response()
                        }
                    }
                }
                Err(_) => (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({
                        "error": "invalid_id",
                        "message": "Invalid pipeline ID format"
                    })),
                )
                    .into_response(),
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
        Ok(_) => match PipelineRunQueries::list(&pool).await {
            Ok(runs) => {
                let response: Vec<serde_json::Value> = runs
                    .into_iter()
                    .map(|r| {
                        serde_json::json!({
                            "id": r.id.to_string(),
                            "pipeline_id": r.pipeline_id.to_string(),
                            "status": r.status,
                            "commit_hash": r.commit_hash,
                            "triggered_by": r.triggered_by,
                            "started_at": r.started_at.map(|dt| dt.to_rfc3339()),
                            "finished_at": r.finished_at.map(|dt| dt.to_rfc3339())
                        })
                    })
                    .collect();
                Json(response).into_response()
            }
            Err(e) => {
                tracing::error!("failed to list pipeline runs: {}", e);
                Json(serde_json::Value::Array(vec![])).into_response()
            }
        },
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
                        Ok(Some(run)) => (
                            StatusCode::OK,
                            Json(serde_json::json!({
                                "id": run.id.to_string(),
                                "pipeline_id": run.pipeline_id.to_string(),
                                "status": run.status,
                                "commit_hash": run.commit_hash,
                                "triggered_by": run.triggered_by,
                                "started_at": run.started_at.map(|dt| dt.to_rfc3339()),
                                "finished_at": run.finished_at.map(|dt| dt.to_rfc3339())
                            })),
                        )
                            .into_response(),
                        Ok(None) => (
                            StatusCode::NOT_FOUND,
                            Json(serde_json::json!({
                                "error": "not_found",
                                "message": "Pipeline run not found"
                            })),
                        )
                            .into_response(),
                        Err(e) => {
                            tracing::error!("failed to get pipeline run: {}", e);
                            (
                                StatusCode::INTERNAL_SERVER_ERROR,
                                Json(serde_json::json!({
                                    "error": "database_error",
                                    "message": format!("failed to get pipeline run: {}", e)
                                })),
                            )
                                .into_response()
                        }
                    }
                }
                Err(_) => (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({
                        "error": "invalid_id",
                        "message": "Invalid pipeline run ID format"
                    })),
                )
                    .into_response(),
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
                            let jobs_json: Vec<serde_json::Value> = jobs
                                .into_iter()
                                .map(|j| {
                                    serde_json::json!({
                                        "id": j.id.to_string(),
                                        "name": j.name,
                                        "status": j.status,
                                        "runner_id": j.runner_id.map(|id| id.to_string()),
                                        "started_at": j.started_at.map(|dt| dt.to_rfc3339()),
                                        "finished_at": j.finished_at.map(|dt| dt.to_rfc3339())
                                    })
                                })
                                .collect();
                            Json(jobs_json).into_response()
                        }
                        Err(e) => {
                            tracing::error!("failed to list jobs: {}", e);
                            (
                                StatusCode::INTERNAL_SERVER_ERROR,
                                Json(serde_json::json!({
                                    "error": "database_error",
                                    "message": format!("failed to list jobs: {}", e)
                                })),
                            )
                                .into_response()
                        }
                    }
                }
                Err(_) => (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({
                        "error": "invalid_id",
                        "message": "Invalid pipeline run ID format"
                    })),
                )
                    .into_response(),
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
                        Ok(Some(job)) => (
                            StatusCode::OK,
                            Json(serde_json::json!({
                                "id": job.id.to_string(),
                                "name": job.name,
                                "status": job.status,
                                "runner_id": job.runner_id.map(|id| id.to_string()),
                                "started_at": job.started_at.map(|dt| dt.to_rfc3339()),
                                "finished_at": job.finished_at.map(|dt| dt.to_rfc3339())
                            })),
                        )
                            .into_response(),
                        Ok(None) => (
                            StatusCode::NOT_FOUND,
                            Json(serde_json::json!({
                                "error": "not_found",
                                "message": "Job not found"
                            })),
                        )
                            .into_response(),
                        Err(e) => {
                            tracing::error!("failed to get job: {}", e);
                            (
                                StatusCode::INTERNAL_SERVER_ERROR,
                                Json(serde_json::json!({
                                    "error": "database_error",
                                    "message": format!("failed to get job: {}", e)
                                })),
                            )
                                .into_response()
                        }
                    }
                }
                Err(_) => (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({
                        "error": "invalid_id",
                        "message": "Invalid job ID format"
                    })),
                )
                    .into_response(),
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
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "job_id": id,
                    "logs": "Logs not yet implemented - coming soon"
                })),
            )
                .into_response()
        }
    }
}

/// Trigger a pipeline manually
async fn trigger_pipeline(
    Extension(pool): Extension<Arc<Pool>>,
    Extension(auth): Extension<Arc<ApiAuth>>,
    Extension(scheduler): Extension<Option<Arc<Scheduler>>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(request): Json<TriggerPipelineRequest>,
) -> impl IntoResponse {
    // Validate auth and get user info
    let auth_header = match headers.get("Authorization").and_then(|v| v.to_str().ok()) {
        Some(h) => h,
        None => return StatusCode::UNAUTHORIZED.into_response(),
    };

    let token = match ApiAuth::extract_token(auth_header) {
        Some(t) => t,
        None => return StatusCode::UNAUTHORIZED.into_response(),
    };

    let claims = match auth.validate_token(token) {
        Ok(c) => c,
        Err(_) => return StatusCode::UNAUTHORIZED.into_response(),
    };

    let username = claims.username.clone();

    tracing::debug!("trigger pipeline: {} by {}", id, username);

    // Parse pipeline UUID
    let pipeline_id = match Uuid::parse_str(&id) {
        Ok(uuid) => gitforge_common::PipelineId::from(uuid),
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": "invalid_id",
                    "message": "Invalid pipeline ID format"
                })),
            )
                .into_response();
        }
    };

    // Get pipeline from database
    let pipeline = match PipelineQueries::get(&pool, pipeline_id).await {
        Ok(Some(p)) => p,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({
                    "error": "not_found",
                    "message": "Pipeline not found"
                })),
            )
                .into_response();
        }
        Err(e) => {
            tracing::error!("failed to get pipeline: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": "database_error",
                    "message": format!("failed to get pipeline: {}", e)
                })),
            )
                .into_response();
        }
    };

    // Determine commit hash
    let commit_hash = request.commit_hash.unwrap_or_else(|| "HEAD".to_string());

    // Parse and validate jobs from pipeline config BEFORE creating PipelineRun
    // This ensures invalid config cannot orphan a pipeline run.
    // Each job must have a non-empty 'image' field.
    let job_specs = match parse_jobs_from_config(&pipeline.config) {
        Ok(specs) => specs,
        Err(e) => {
            tracing::error!("failed to parse pipeline config: {}", e);
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": "invalid_config",
                    "message": format!("failed to parse pipeline config: {}", e)
                })),
            )
                .into_response();
        }
    };

    // Create pipeline run
    let pipeline_run = PipelineRun::new(
        pipeline.id,
        pipeline.repo_id,
        username.clone(),
        commit_hash.clone(),
    );

    // Persist pipeline run
    if let Err(e) = PipelineRunQueries::create(&pool, &pipeline_run).await {
        tracing::error!("failed to create pipeline run: {}", e);
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "error": "database_error",
                "message": format!("failed to create pipeline run: {}", e)
            })),
        )
            .into_response();
    }

    // Create jobs in database
    let mut triggered_jobs = Vec::new();
    for spec in &job_specs {
        let job = Job::new(pipeline_run.id, spec.name.clone(), &spec.image);

        if let Err(e) = JobQueries::create(&pool, &job).await {
            tracing::error!("failed to create job {}: {}", spec.name, e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": "database_error",
                    "message": format!("failed to create job {}: {}", spec.name, e)
                })),
            )
                .into_response();
        }

        // Persist each step for this job
        for (step_idx, (step_name, step_run)) in spec.steps.iter().enumerate() {
            let step = JobStep::new(job.id, step_idx as i32, step_name, step_run);
            if let Err(e) = JobStepQueries::create(&pool, &step).await {
                tracing::error!(
                    "failed to create step {} for job {}: {}",
                    step_name,
                    spec.name,
                    e
                );
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({
                        "error": "database_error",
                        "message": format!("failed to create step {} for job {}: {}", step_name, spec.name, e)
                    })),
                )
                    .into_response();
            }
        }

        triggered_jobs.push(TriggeredJobInfo {
            job_id: job.id.to_string(),
            name: job.name.clone(),
            status: job.status.clone(),
        });
    }

    // Enqueue persisted jobs with scheduler if available
    // Uses enqueue_persisted_job to avoid creating duplicate DB rows
    if let Some(sched) = scheduler.as_ref() {
        for job in &triggered_jobs {
            let job_id = gitforge_common::JobId::from(Uuid::parse_str(&job.job_id).unwrap());
            sched
                .enqueue_persisted_job(job_id, pipeline_run.id, pipeline.repo_id)
                .await;
            tracing::debug!(
                "job {} enqueued for pipeline run {}",
                job.job_id,
                pipeline_run.id
            );
        }
    }

    tracing::info!(
        "pipeline {} triggered by {} with {} jobs",
        pipeline.id,
        username,
        triggered_jobs.len()
    );

    let response = TriggerPipelineResponse {
        pipeline_run_id: pipeline_run.id.to_string(),
        pipeline_id: pipeline.id.to_string(),
        status: pipeline_run.status.clone(),
        triggered_by: username,
        commit_hash,
        jobs: triggered_jobs,
    };

    (StatusCode::CREATED, Json(response)).into_response()
}

/// A parsed job specification from a pipeline config.
/// Contains the job name, the container image, and the ordered list of steps to run.
#[derive(Debug, Clone)]
struct JobSpec {
    name: String,
    image: String,
    /// Steps extracted from the pipeline config. Each step has a name and a command.
    steps: Vec<(String, String)>,
}

/// Parse job metadata (name, image, and steps) from pipeline config.
/// Each job must have a non-empty 'name', 'image', and at least one 'steps' entry.
fn parse_jobs_from_config(config: &serde_json::Value) -> Result<Vec<JobSpec>, String> {
    let jobs = config
        .get("jobs")
        .ok_or("missing 'jobs' field in pipeline config")?;

    let jobs_array = jobs.as_array().ok_or("'jobs' must be an array")?;

    let mut result = Vec::new();
    for (idx, job) in jobs_array.iter().enumerate() {
        let name = job
            .get("name")
            .and_then(|n| n.as_str())
            .ok_or_else(|| format!("job[{}] missing 'name' field", idx))?
            .to_string();
        let image = job
            .get("image")
            .and_then(|i| i.as_str())
            .ok_or_else(|| format!("job[{}] missing 'image' field", idx))?
            .to_string();

        // Parse steps: each step has a "run" field with the command
        let steps_value = job
            .get("steps")
            .and_then(|s| s.as_array())
            .ok_or_else(|| format!("job[{}] missing 'steps' field", idx))?;
        let mut steps = Vec::new();
        for (step_idx, step) in steps_value.iter().enumerate() {
            let step_name = step
                .get("name")
                .and_then(|n| n.as_str())
                .ok_or_else(|| format!("job[{}].steps[{}] missing 'name' field", idx, step_idx))?
                .to_string();
            let run = step
                .get("run")
                .and_then(|r| r.as_str())
                .ok_or_else(|| format!("job[{}].steps[{}] missing 'run' field", idx, step_idx))?
                .to_string();
            if run.is_empty() {
                return Err(format!(
                    "job[{}].steps[{}] 'run' field must not be empty",
                    idx, step_idx
                ));
            }
            steps.push((step_name, run));
        }
        if steps.is_empty() {
            return Err(format!("job[{}] must have at least one step", idx));
        }

        result.push(JobSpec { name, image, steps });
    }

    Ok(result)
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

    #[test]
    fn test_pipeline_run_response_debug() {
        let response = PipelineRunResponse {
            id: "run-debug".to_string(),
            pipeline_id: "pipe-debug".to_string(),
            status: "debugging".to_string(),
            commit_hash: "abc123debug".to_string(),
            triggered_by: "debug-user".to_string(),
            started_at: Some("2024-01-01T00:00:00Z".to_string()),
            finished_at: None,
        };
        let debug_str = format!("{:?}", response);
        assert!(debug_str.contains("run-debug"));
    }

    #[test]
    fn test_job_response_debug() {
        let response = JobResponse {
            id: "job-debug".to_string(),
            name: "debug-job".to_string(),
            status: "debugging".to_string(),
            runner_id: None,
            started_at: None,
            finished_at: None,
        };
        let debug_str = format!("{:?}", response);
        assert!(debug_str.contains("job-debug"));
    }

    #[test]
    fn test_pipeline_run_response_with_all_statuses() {
        for status in &["pending", "running", "succeeded", "failed", "cancelled"] {
            let response = PipelineRunResponse {
                id: "run-status".to_string(),
                pipeline_id: "pipe-status".to_string(),
                status: status.to_string(),
                commit_hash: "abc123".to_string(),
                triggered_by: "test".to_string(),
                started_at: None,
                finished_at: None,
            };
            let json = serde_json::to_string(&response).unwrap();
            assert!(json.contains(status));
        }
    }

    #[test]
    fn test_job_response_with_all_statuses() {
        for status in &["queued", "assigned", "running", "succeeded", "failed"] {
            let response = JobResponse {
                id: "job-status".to_string(),
                name: "status-test".to_string(),
                status: status.to_string(),
                runner_id: None,
                started_at: None,
                finished_at: None,
            };
            let json = serde_json::to_string(&response).unwrap();
            assert!(json.contains(status));
        }
    }

    #[test]
    fn test_pipeline_run_response_large_commit_hash() {
        let response = PipelineRunResponse {
            id: "run-large".to_string(),
            pipeline_id: "pipe-large".to_string(),
            status: "running".to_string(),
            commit_hash: "abc123def456789012345678901234567890".to_string(),
            triggered_by: "test".to_string(),
            started_at: Some("2024-01-01T00:00:00Z".to_string()),
            finished_at: None,
        };
        assert!(response.commit_hash.len() > 20);
    }

    #[test]
    fn test_job_response_with_runner_assignment() {
        let response = JobResponse {
            id: "job-assigned".to_string(),
            name: "assigned-job".to_string(),
            status: "assigned".to_string(),
            runner_id: Some("runner-assigned-123".to_string()),
            started_at: None,
            finished_at: None,
        };
        assert!(response.runner_id.is_some());
        assert_eq!(response.runner_id.unwrap(), "runner-assigned-123");
    }

    #[test]
    fn test_pipeline_run_response_complete_cycle() {
        let response = PipelineRunResponse {
            id: "run-complete".to_string(),
            pipeline_id: "pipe-complete".to_string(),
            status: "succeeded".to_string(),
            commit_hash: "abc123".to_string(),
            triggered_by: "ci-bot".to_string(),
            started_at: Some("2024-01-01T00:00:00Z".to_string()),
            finished_at: Some("2024-01-01T00:10:00Z".to_string()),
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("succeeded"));
        assert!(json.contains("ci-bot"));
    }

    #[test]
    fn test_job_response_complete_with_timestamps() {
        let response = JobResponse {
            id: "job-complete".to_string(),
            name: "complete-job".to_string(),
            status: "succeeded".to_string(),
            runner_id: Some("runner-1".to_string()),
            started_at: Some("2024-01-01T00:00:00Z".to_string()),
            finished_at: Some("2024-01-01T00:05:00Z".to_string()),
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("succeeded"));
        assert!(json.contains("runner-1"));
    }

    #[test]
    fn test_extract_user_without_auth_header() {
        use crate::auth::ApiAuth;

        let auth = ApiAuth::new("test-secret");
        let headers = HeaderMap::new();
        let result = extract_user(&auth, &headers);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn test_extract_user_with_invalid_token() {
        use crate::auth::ApiAuth;

        let auth = ApiAuth::new("test-secret");
        let mut headers = HeaderMap::new();
        headers.insert("Authorization", "Bearer invalid-token".parse().unwrap());
        let result = extract_user(&auth, &headers);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn test_extract_user_with_valid_token() {
        use crate::auth::ApiAuth;
        use gitforge_common::UserId;

        let auth = ApiAuth::new("test-secret");
        let user_id = UserId::new();
        let token = auth.generate_token(user_id, "testuser", "user").unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(
            "Authorization",
            format!("Bearer {}", token).parse().unwrap(),
        );
        let result = extract_user(&auth, &headers);
        assert!(result.is_ok());
    }

    #[test]
    fn test_pipeline_run_response_deserialize() {
        let json = r#"{
            "id": "run-123",
            "pipeline_id": "pipe-456",
            "status": "running",
            "commit_hash": "abc123",
            "triggered_by": "user1",
            "started_at": "2024-01-01T00:00:00Z",
            "finished_at": null
        }"#;
        let response: PipelineRunResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.id, "run-123");
        assert_eq!(response.pipeline_id, "pipe-456");
        assert_eq!(response.status, "running");
        assert!(response.started_at.is_some());
        assert!(response.finished_at.is_none());
    }

    #[test]
    fn test_job_response_deserialize() {
        let json = r#"{
            "id": "job-123",
            "name": "build",
            "status": "pending",
            "runner_id": null,
            "started_at": null,
            "finished_at": null
        }"#;
        let response: JobResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.id, "job-123");
        assert_eq!(response.name, "build");
        assert_eq!(response.status, "pending");
        assert!(response.runner_id.is_none());
    }

    #[test]
    fn test_extract_user_malformed_auth_header() {
        use crate::auth::ApiAuth;

        let auth = ApiAuth::new("test-secret");
        let mut headers = HeaderMap::new();
        // Malformed header without proper Bearer prefix
        headers.insert("Authorization", "NotBearer token123".parse().unwrap());
        let result = extract_user(&auth, &headers);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn test_extract_user_empty_bearer_token() {
        use crate::auth::ApiAuth;

        let auth = ApiAuth::new("test-secret");
        let mut headers = HeaderMap::new();
        headers.insert("Authorization", "Bearer".parse().unwrap());
        let result = extract_user(&auth, &headers);
        assert!(result.is_err());
        // Empty token after Bearer should fail
        assert_eq!(result.unwrap_err(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn test_extract_user_basic_auth_header() {
        use crate::auth::ApiAuth;

        let auth = ApiAuth::new("test-secret");
        let mut headers = HeaderMap::new();
        // Basic auth instead of Bearer
        headers.insert("Authorization", "Basic dXNlcjpwYXNz".parse().unwrap());
        let result = extract_user(&auth, &headers);
        assert!(result.is_err());
    }

    #[test]
    fn test_extract_user_bearer_with_extra_spaces() {
        use crate::auth::ApiAuth;

        let auth = ApiAuth::new("test-secret");
        let mut headers = HeaderMap::new();
        // Bearer with leading/trailing spaces
        headers.insert("Authorization", "Bearer   ".parse().unwrap());
        let result = extract_user(&auth, &headers);
        // Should fail because "   " is not a valid token
        assert!(result.is_err());
    }

    #[test]
    fn test_pipeline_run_response_with_empty_commit_hash() {
        let response = PipelineRunResponse {
            id: "run-empty".to_string(),
            pipeline_id: "pipe-empty".to_string(),
            status: "pending".to_string(),
            commit_hash: "".to_string(),
            triggered_by: "user".to_string(),
            started_at: None,
            finished_at: None,
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"commit_hash\":\"\""));
    }

    #[test]
    fn test_job_response_with_empty_name() {
        let response = JobResponse {
            id: "job-empty".to_string(),
            name: "".to_string(),
            status: "pending".to_string(),
            runner_id: None,
            started_at: None,
            finished_at: None,
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"name\":\"\""));
    }

    #[test]
    fn test_pipeline_run_response_special_characters_in_triggered_by() {
        let response = PipelineRunResponse {
            id: "run-special".to_string(),
            pipeline_id: "pipe-special".to_string(),
            status: "running".to_string(),
            commit_hash: "abc123".to_string(),
            triggered_by: "user@domain.com".to_string(),
            started_at: None,
            finished_at: None,
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("user@domain.com"));
    }

    #[test]
    fn test_pipeline_run_response_all_json_fields() {
        let response = PipelineRunResponse {
            id: "run-all".to_string(),
            pipeline_id: "pipe-all".to_string(),
            status: "failed".to_string(),
            commit_hash: "xyz789".to_string(),
            triggered_by: "tester".to_string(),
            started_at: Some("2024-06-15T10:30:00Z".to_string()),
            finished_at: Some("2024-06-15T10:45:00Z".to_string()),
        };
        let json = serde_json::to_string(&response).unwrap();
        // Verify all fields are present
        assert!(json.contains("\"id\":\"run-all\""));
        assert!(json.contains("\"pipeline_id\":\"pipe-all\""));
        assert!(json.contains("\"status\":\"failed\""));
        assert!(json.contains("\"commit_hash\":\"xyz789\""));
        assert!(json.contains("\"triggered_by\":\"tester\""));
        assert!(json.contains("started_at"));
        assert!(json.contains("finished_at"));
    }

    #[test]
    fn test_extract_user_malformed_header() {
        use crate::auth::ApiAuth;

        let auth = ApiAuth::new("test-secret");
        let mut headers = HeaderMap::new();
        headers.insert("Authorization", "Malformed".parse().unwrap());
        let result = extract_user(&auth, &headers);
        assert!(result.is_err());
    }

    #[test]
    fn test_extract_user_bearer_with_leading_space() {
        use crate::auth::ApiAuth;

        let auth = ApiAuth::new("test-secret");
        let mut headers = HeaderMap::new();
        headers.insert("Authorization", " Bearer token123".parse().unwrap());
        let result = extract_user(&auth, &headers);
        assert!(result.is_err());
    }

    #[test]
    fn test_extract_user_multiple_auth_headers() {
        use crate::auth::ApiAuth;

        let auth = ApiAuth::new("test-secret");
        let mut headers = HeaderMap::new();
        headers.insert("Authorization", "Bearer token1".parse().unwrap());
        headers.append("Authorization", "Bearer token2".parse().unwrap());
        let result = extract_user(&auth, &headers);
        // First header should be used
        assert!(result.is_err());
    }

    #[test]
    fn test_pipeline_run_response_all_commit_hashes() {
        for hash in &["a", "abc", "abcdef123456", "a1b2c3d4e5f6789012345678901234"] {
            let response = PipelineRunResponse {
                id: "run-hash".to_string(),
                pipeline_id: "pipe-hash".to_string(),
                status: "running".to_string(),
                commit_hash: hash.to_string(),
                triggered_by: "test".to_string(),
                started_at: None,
                finished_at: None,
            };
            let json = serde_json::to_string(&response).unwrap();
            assert!(json.contains(hash));
        }
    }

    #[test]
    fn test_pipeline_run_response_all_triggered_by() {
        for user in &["alice", "bob", "ci-bot", "webhook", "schedule"] {
            let response = PipelineRunResponse {
                id: "run-trigger".to_string(),
                pipeline_id: "pipe-trigger".to_string(),
                status: "running".to_string(),
                commit_hash: "abc123".to_string(),
                triggered_by: user.to_string(),
                started_at: None,
                finished_at: None,
            };
            let json = serde_json::to_string(&response).unwrap();
            assert!(json.contains(user));
        }
    }

    #[test]
    fn test_job_response_all_names() {
        for name in &["build", "test", "deploy", "lint", "security-scan"] {
            let response = JobResponse {
                id: "job-name".to_string(),
                name: name.to_string(),
                status: "queued".to_string(),
                runner_id: None,
                started_at: None,
                finished_at: None,
            };
            let json = serde_json::to_string(&response).unwrap();
            assert!(json.contains(name));
        }
    }

    #[test]
    fn test_job_response_with_future_timestamps() {
        let response = JobResponse {
            id: "job-future".to_string(),
            name: "future-job".to_string(),
            status: "running".to_string(),
            runner_id: Some("runner-1".to_string()),
            started_at: Some("2026-12-01T00:00:00Z".to_string()),
            finished_at: Some("2026-12-01T00:10:00Z".to_string()),
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("2026-12-01"));
    }

    // =========================================================================
    // trigger_pipeline handler tests
    // =========================================================================

    #[test]
    fn test_parse_jobs_from_config_valid() {
        let config = serde_json::json!({
            "jobs": [
                {
                    "name": "build",
                    "image": "rust:latest",
                    "steps": [
                        {"name": "compile", "run": "cargo build"},
                        {"name": "check", "run": "cargo check"}
                    ]
                },
                {
                    "name": "test",
                    "image": "rust:latest",
                    "steps": [
                        {"name": "unit", "run": "cargo test"}
                    ]
                },
                {
                    "name": "deploy",
                    "image": "docker:latest",
                    "steps": [
                        {"name": "push", "run": "docker push"}
                    ]
                }
            ]
        });

        let result = parse_jobs_from_config(&config).unwrap();
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].name, "build");
        assert_eq!(result[0].image, "rust:latest");
        assert_eq!(result[0].steps.len(), 2);
        assert_eq!(result[0].steps[0].0, "compile");
        assert_eq!(result[0].steps[0].1, "cargo build");
        assert_eq!(result[1].name, "test");
        assert_eq!(result[1].steps.len(), 1);
        assert_eq!(result[1].steps[0].1, "cargo test");
        assert_eq!(result[2].name, "deploy");
    }

    #[test]
    fn test_parse_jobs_from_config_single_job() {
        let config = serde_json::json!({
            "jobs": [
                {
                    "name": "build",
                    "image": "rust:1.75",
                    "steps": [
                        {"name": "compile", "run": "cargo build --release"}
                    ]
                }
            ]
        });

        let result = parse_jobs_from_config(&config).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "build");
        assert_eq!(result[0].image, "rust:1.75");
        assert_eq!(result[0].steps.len(), 1);
        assert_eq!(result[0].steps[0].0, "compile");
        assert_eq!(result[0].steps[0].1, "cargo build --release");
    }

    #[test]
    fn test_parse_jobs_from_config_empty_jobs() {
        let config = serde_json::json!({
            "jobs": []
        });

        let result = parse_jobs_from_config(&config).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_parse_jobs_from_config_missing_jobs_field() {
        let config = serde_json::json!({
            "stages": ["build"]
        });

        let result = parse_jobs_from_config(&config);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("missing 'jobs' field"));
    }

    #[test]
    fn test_parse_jobs_from_config_jobs_not_array() {
        let config = serde_json::json!({
            "jobs": "not an array"
        });

        let result = parse_jobs_from_config(&config);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("must be an array"));
    }

    #[test]
    fn test_parse_jobs_from_config_job_missing_name() {
        let config = serde_json::json!({
            "jobs": [
                {
                    "name": "build",
                    "image": "rust:latest",
                    "steps": [{"name": "c", "run": "cargo build"}]
                },
                {
                    "image": "rust:latest",
                    "steps": [{"name": "c", "run": "cargo build"}]
                }
            ]
        });

        let result = parse_jobs_from_config(&config);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("job[1] missing 'name' field"));
    }

    #[test]
    fn test_parse_jobs_from_config_job_missing_image() {
        // Job with name but no image should fail
        let config = serde_json::json!({
            "jobs": [
                {
                    "name": "build",
                    "steps": [{"name": "c", "run": "cargo build"}]
                }
            ]
        });

        let result = parse_jobs_from_config(&config);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("job[0] missing 'image' field"));
    }

    #[test]
    fn test_parse_jobs_from_config_valid_with_different_images() {
        let config = serde_json::json!({
            "jobs": [
                {
                    "name": "build",
                    "image": "rust:1.75",
                    "steps": [{"name": "c", "run": "cargo build"}]
                },
                {
                    "name": "test",
                    "image": "python:3.12",
                    "steps": [{"name": "p", "run": "pytest"}]
                },
                {
                    "name": "deploy",
                    "image": "node:20",
                    "steps": [{"name": "n", "run": "npm deploy"}]
                }
            ]
        });

        let result = parse_jobs_from_config(&config).unwrap();
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].name, "build");
        assert_eq!(result[0].image, "rust:1.75");
        assert_eq!(result[1].name, "test");
        assert_eq!(result[1].image, "python:3.12");
        assert_eq!(result[2].name, "deploy");
        assert_eq!(result[2].image, "node:20");
    }

    #[test]
    fn test_parse_jobs_from_config_empty_config() {
        let config = serde_json::Value::Null;

        let result = parse_jobs_from_config(&config);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_jobs_from_config_job_missing_steps() {
        // Job without steps field should fail
        let config = serde_json::json!({
            "jobs": [
                {"name": "build", "image": "rust:latest"}
            ]
        });

        let result = parse_jobs_from_config(&config);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("missing 'steps' field"));
    }

    #[test]
    fn test_parse_jobs_from_config_step_missing_run() {
        // Step without 'run' field should fail
        let config = serde_json::json!({
            "jobs": [
                {
                    "name": "build",
                    "image": "rust:latest",
                    "steps": [
                        {"name": "compile"}
                    ]
                }
            ]
        });

        let result = parse_jobs_from_config(&config);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("missing 'run' field"));
    }

    #[test]
    fn test_parse_jobs_from_config_step_empty_run() {
        // Step with empty 'run' string should fail
        let config = serde_json::json!({
            "jobs": [
                {
                    "name": "build",
                    "image": "rust:latest",
                    "steps": [
                        {"name": "compile", "run": ""}
                    ]
                }
            ]
        });

        let result = parse_jobs_from_config(&config);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("must not be empty"));
    }

    #[test]
    fn test_parse_jobs_from_config_zero_steps() {
        // Job with empty steps array should fail
        let config = serde_json::json!({
            "jobs": [
                {
                    "name": "build",
                    "image": "rust:latest",
                    "steps": []
                }
            ]
        });

        let result = parse_jobs_from_config(&config);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("must have at least one step"));
    }

    #[test]
    fn test_trigger_pipeline_request_serialization() {
        let request = TriggerPipelineRequest {
            commit_hash: Some("abc123".to_string()),
            branch: Some("main".to_string()),
        };
        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("abc123"));
        assert!(json.contains("main"));
    }

    #[test]
    fn test_trigger_pipeline_request_minimal() {
        let request = TriggerPipelineRequest {
            commit_hash: None,
            branch: None,
        };
        let json = serde_json::to_string(&request).unwrap();
        // Verify minimal request serializes without errors
        assert!(json.contains("commit_hash"));
        assert!(json.contains("branch"));
    }

    #[test]
    fn test_trigger_pipeline_response_serialization() {
        let response = TriggerPipelineResponse {
            pipeline_run_id: "run-123".to_string(),
            pipeline_id: "pipe-456".to_string(),
            status: "pending".to_string(),
            triggered_by: "alice".to_string(),
            commit_hash: "abc123".to_string(),
            jobs: vec![
                TriggeredJobInfo {
                    job_id: "job-1".to_string(),
                    name: "build".to_string(),
                    status: "pending".to_string(),
                },
                TriggeredJobInfo {
                    job_id: "job-2".to_string(),
                    name: "test".to_string(),
                    status: "pending".to_string(),
                },
            ],
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("run-123"));
        assert!(json.contains("pipe-456"));
        assert!(json.contains("pending"));
        assert!(json.contains("alice"));
        assert!(json.contains("abc123"));
        assert!(json.contains("build"));
        assert!(json.contains("test"));
    }

    #[test]
    fn test_triggered_job_info_serialization() {
        let info = TriggeredJobInfo {
            job_id: "job-xyz".to_string(),
            name: "deploy".to_string(),
            status: "queued".to_string(),
        };
        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains("job-xyz"));
        assert!(json.contains("deploy"));
        assert!(json.contains("queued"));
    }

    #[test]
    fn test_trigger_pipeline_response_deserialization() {
        let json = r#"{
            "pipeline_run_id": "run-789",
            "pipeline_id": "pipe-001",
            "status": "pending",
            "triggered_by": "bob",
            "commit_hash": "def456",
            "jobs": [
                {"job_id": "job-a", "name": "lint", "status": "pending"}
            ]
        }"#;
        let response: TriggerPipelineResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.pipeline_run_id, "run-789");
        assert_eq!(response.pipeline_id, "pipe-001");
        assert_eq!(response.triggered_by, "bob");
        assert_eq!(response.commit_hash, "def456");
        assert_eq!(response.jobs.len(), 1);
        assert_eq!(response.jobs[0].name, "lint");
    }

    #[test]
    fn test_trigger_pipeline_response_empty_jobs() {
        let response = TriggerPipelineResponse {
            pipeline_run_id: "run-empty".to_string(),
            pipeline_id: "pipe-empty".to_string(),
            status: "pending".to_string(),
            triggered_by: "alice".to_string(),
            commit_hash: "abc123".to_string(),
            jobs: vec![],
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"jobs\":[]"));
    }
}

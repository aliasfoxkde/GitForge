//! CI API routes

use crate::auth::Claims;
use crate::middleware::AuthenticatedUser;
use axum::{
    extract::{Extension, Path},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use gitforge_common::{JobId, PipelineId, PipelineRunId};
use gitforge_db::{
    models::JobStatus,
    queries::{JobQueries, PipelineQueries, PipelineRunQueries, RepoQueries},
    Pool,
};
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

/// User-facing job submission. The durable idempotency key is scoped to the
/// authenticated user so client retries cannot create duplicate work.
#[derive(Debug, Deserialize, Serialize)]
pub struct SubmitJobRequest {
    pub pipeline_run_id: String,
    pub name: String,
    pub commands: Vec<String>,
    pub working_dir: Option<String>,
    pub idempotency_key: String,
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
        .route("/jobs/{id}/cancel", post(cancel_job))
        .route("/jobs", post(submit_job))
}

fn can_manage_jobs(claims: &Claims) -> bool {
    matches!(claims.role.as_str(), "admin" | "maintainer")
}

fn claims_from_user(user: AuthenticatedUser) -> Result<Claims, StatusCode> {
    Ok(user.claims)
}

/// Require ownership of a repository, with admin/maintainer override.
async fn authorize_repo(
    pool: &Pool,
    claims: &Claims,
    repo_id: gitforge_common::RepoId,
) -> Result<(), StatusCode> {
    if can_manage_jobs(claims) {
        return Ok(());
    }
    match RepoQueries::get(pool, repo_id).await {
        Ok(Some(repo)) if repo.owner_id == claims.user_id => Ok(()),
        Ok(Some(_)) => Err(StatusCode::FORBIDDEN),
        Ok(None) => Err(StatusCode::NOT_FOUND),
        Err(error) => {
            tracing::error!(%error, %repo_id, "failed to authorize repository access");
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// Load a job and enforce repository ownership through its pipeline run.
async fn authorized_job(
    pool: &Pool,
    claims: &Claims,
    job_id: JobId,
) -> Result<gitforge_db::models::Job, StatusCode> {
    let job = match JobQueries::get(pool, job_id).await {
        Ok(Some(job)) => job,
        Ok(None) => return Err(StatusCode::NOT_FOUND),
        Err(error) => {
            tracing::error!(%error, %job_id, "failed to load job for authorization");
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };
    let run = match PipelineRunQueries::get(pool, job.pipeline_run_id).await {
        Ok(Some(run)) => run,
        Ok(None) => return Err(StatusCode::NOT_FOUND),
        Err(error) => {
            tracing::error!(%error, %job_id, "failed to load pipeline run for authorization");
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };
    authorize_repo(pool, claims, run.repo_id).await?;
    Ok(job)
}

/// List pipelines
async fn list_pipelines(
    Extension(pool): Extension<Arc<Pool>>,
    user: AuthenticatedUser,
) -> impl IntoResponse {
    match claims_from_user(user) {
        Err(e) => e.into_response(),
        Ok(claims) => match PipelineQueries::list(&pool).await {
            Ok(pipelines) => {
                let mut response = Vec::new();
                for p in pipelines {
                    if authorize_repo(&pool, &claims, p.repo_id).await.is_ok() {
                        response.push(serde_json::json!({
                            "id": p.id.to_string(),
                            "repo_id": p.repo_id.to_string(),
                            "name": p.name,
                            "trigger_type": p.trigger_type,
                            "created_at": p.created_at.to_rfc3339()
                        }));
                    }
                }
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
    user: AuthenticatedUser,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match claims_from_user(user) {
        Err(e) => e.into_response(),
        Ok(claims) => {
            tracing::debug!("get pipeline: {}", id);

            match Uuid::parse_str(&id) {
                Ok(uuid) => {
                    let pipeline_id = PipelineId::from(uuid);
                    match PipelineQueries::get(&pool, pipeline_id).await {
                        Ok(Some(pipeline)) => match authorize_repo(&pool, &claims, pipeline.repo_id).await {
                            Ok(()) => (
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
                            Err(status) => (
                                status,
                                Json(serde_json::json!({"error": "forbidden", "message": "Pipeline access is not permitted"})),
                            )
                                .into_response(),
                        },
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
    user: AuthenticatedUser,
) -> impl IntoResponse {
    match claims_from_user(user) {
        Err(e) => e.into_response(),
        Ok(claims) => match PipelineRunQueries::list(&pool).await {
            Ok(runs) => {
                let mut response = Vec::new();
                for r in runs {
                    if authorize_repo(&pool, &claims, r.repo_id).await.is_ok() {
                        response.push(serde_json::json!({
                            "id": r.id.to_string(),
                            "pipeline_id": r.pipeline_id.to_string(),
                            "status": r.status,
                            "commit_hash": r.commit_hash,
                            "triggered_by": r.triggered_by,
                            "started_at": r.started_at.map(|dt| dt.to_rfc3339()),
                            "finished_at": r.finished_at.map(|dt| dt.to_rfc3339())
                        }));
                    }
                }
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
    user: AuthenticatedUser,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match claims_from_user(user) {
        Err(e) => e.into_response(),
        Ok(claims) => {
            tracing::debug!("get pipeline run: {}", id);

            match Uuid::parse_str(&id) {
                Ok(uuid) => {
                    let run_id = PipelineRunId::from(uuid);
                    match PipelineRunQueries::get(&pool, run_id).await {
                        Ok(Some(run)) => match authorize_repo(&pool, &claims, run.repo_id).await {
                            Ok(()) => (
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
                            Err(status) => (
                                status,
                                Json(serde_json::json!({"error": "forbidden", "message": "Pipeline run access is not permitted"})),
                            )
                                .into_response(),
                        },
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
    user: AuthenticatedUser,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match claims_from_user(user) {
        Err(e) => e.into_response(),
        Ok(claims) => {
            tracing::debug!("get pipeline run jobs: {}", id);

            match Uuid::parse_str(&id) {
                Ok(uuid) => {
                    let run_id = PipelineRunId::from(uuid);
                    let run = match PipelineRunQueries::get(&pool, run_id).await {
                        Ok(Some(run)) => run,
                        Ok(None) => {
                            return (
                                StatusCode::NOT_FOUND,
                                Json(serde_json::json!({"error": "not_found", "message": "Pipeline run not found"})),
                            )
                                .into_response();
                        }
                        Err(error) => {
                            tracing::error!(%error, %run_id, "failed to load pipeline run for jobs");
                            return (
                                StatusCode::INTERNAL_SERVER_ERROR,
                                Json(serde_json::json!({"error": "database_error", "message": "Failed to load pipeline run"})),
                            )
                                .into_response();
                        }
                    };
                    if let Err(status) = authorize_repo(&pool, &claims, run.repo_id).await {
                        return (
                            status,
                            Json(serde_json::json!({"error": "forbidden", "message": "Pipeline run access is not permitted"})),
                        )
                            .into_response();
                    }
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
                                        "finished_at": j.finished_at.map(|dt| dt.to_rfc3339()),
                                        "receipt": j.result_json.and_then(|value| serde_json::from_str::<serde_json::Value>(&value).ok())
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

/// Submit a user-owned job to the durable queue. The scheduler reloads queued
/// rows on its next bounded scheduling tick, so this remains safe across the
/// separate API and CI processes.
async fn submit_job(
    Extension(pool): Extension<Arc<Pool>>,
    Extension(claims): Extension<Claims>,
    Json(request): Json<SubmitJobRequest>,
) -> impl IntoResponse {
    if request.name.is_empty()
        || request.name.len() > 128
        || request.idempotency_key.is_empty()
        || request.idempotency_key.len() > 128
        || request.commands.is_empty()
        || request.commands.len() > 64
        || request
            .commands
            .iter()
            .any(|command| command.is_empty() || command.len() > 16 * 1024)
        || request
            .working_dir
            .as_deref()
            .is_some_and(|path| path.is_empty() || path.len() > 1024)
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "invalid_job_submission"})),
        )
            .into_response();
    }
    let run_id = match Uuid::parse_str(&request.pipeline_run_id) {
        Ok(uuid) => PipelineRunId::from(uuid),
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "invalid_pipeline_run_id"})),
            )
                .into_response();
        }
    };
    let run = match PipelineRunQueries::get(&pool, run_id).await {
        Ok(Some(run)) => run,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "pipeline_run_not_found"})),
            )
                .into_response();
        }
        Err(error) => {
            tracing::error!(%error, %run_id, "failed to load pipeline run for submission");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "database_error"})),
            )
                .into_response();
        }
    };
    if let Err(status) = authorize_repo(&pool, &claims, run.repo_id).await {
        return (
            status,
            Json(serde_json::json!({"error": if status == StatusCode::FORBIDDEN { "forbidden" } else { "not_found" }})),
        )
            .into_response();
    }
    if JobStatus::from_str(&run.status)
        .map(|status| status.is_terminal())
        .unwrap_or(false)
    {
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({"error": "pipeline_run_already_terminal"})),
        )
            .into_response();
    }
    let fingerprint = match serde_json::to_string(&request) {
        Ok(fingerprint) => fingerprint,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "invalid_job_submission"})),
            )
                .into_response();
        }
    };
    let scope = format!("user:{}", claims.user_id);
    if let Ok(Some((job_id, stored_fingerprint))) =
        JobQueries::get_idempotency(&pool, &scope, &request.idempotency_key).await
    {
        if stored_fingerprint != fingerprint {
            return (
                StatusCode::CONFLICT,
                Json(serde_json::json!({"error": "idempotency_key_reused_with_different_request"})),
            )
                .into_response();
        }
        return (
            StatusCode::OK,
            Json(serde_json::json!({"status": "already_queued", "job_id": job_id.to_string()})),
        )
            .into_response();
    }
    let job_id = JobId::new();
    match JobQueries::reserve_idempotency(
        &pool,
        &scope,
        &request.idempotency_key,
        &fingerprint,
        job_id,
    )
    .await
    {
        Ok(true) => {}
        Ok(false) => {
            match JobQueries::get_idempotency(&pool, &scope, &request.idempotency_key).await {
                Ok(Some((existing_id, stored_fingerprint)))
                    if stored_fingerprint == fingerprint =>
                {
                    return (
                        StatusCode::OK,
                        Json(serde_json::json!({
                            "status": "already_queued",
                            "job_id": existing_id.to_string()
                        })),
                    )
                        .into_response();
                }
                Ok(Some(_)) => {
                    return (
                        StatusCode::CONFLICT,
                        Json(serde_json::json!({
                            "error": "idempotency_key_reused_with_different_request"
                        })),
                    )
                        .into_response();
                }
                Ok(None) | Err(_) => {
                    return (
                        StatusCode::CONFLICT,
                        Json(serde_json::json!({"error": "idempotency_retry_race"})),
                    )
                        .into_response();
                }
            }
        }
        Err(error) => {
            tracing::error!(%error, "failed to reserve job idempotency key");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "database_error"})),
            )
                .into_response();
        }
    }
    let mut job = gitforge_db::models::Job::new(run_id, request.name);
    job.id = job_id;
    job.commands = request.commands.clone();
    job.working_dir = request.working_dir.clone();
    if let Err(error) = JobQueries::create(&pool, &job).await {
        tracing::error!(%error, %job_id, "failed to create submitted job");
        let _ = JobQueries::delete_idempotency(&pool, &scope, &request.idempotency_key).await;
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "database_error"})),
        )
            .into_response();
    }
    if let Err(error) = JobQueries::update_status(&pool, job_id, "queued").await {
        tracing::error!(%error, %job_id, "failed to queue submitted job");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "database_error"})),
        )
            .into_response();
    }
    if let Err(error) = JobQueries::set_definition(
        &pool,
        job_id,
        &request.commands,
        request.working_dir.as_deref(),
    )
    .await
    {
        tracing::error!(%error, %job_id, "failed to persist submitted job definition");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "database_error"})),
        )
            .into_response();
    }
    (
        StatusCode::CREATED,
        Json(serde_json::json!({"status": "queued", "job_id": job_id.to_string()})),
    )
        .into_response()
}

/// Get job
async fn get_job(
    Extension(pool): Extension<Arc<Pool>>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    tracing::debug!("get job: {}", id);
    let job_id = match Uuid::parse_str(&id) {
        Ok(uuid) => JobId::from(uuid),
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": "invalid_id",
                    "message": "Invalid job ID format"
                })),
            )
                .into_response();
        }
    };
    match authorized_job(&pool, &claims, job_id).await {
        Ok(job) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "id": job.id.to_string(),
                "name": job.name,
                "status": job.status,
                "runner_id": job.runner_id.map(|id| id.to_string()),
                "started_at": job.started_at.map(|dt| dt.to_rfc3339()),
                "finished_at": job.finished_at.map(|dt| dt.to_rfc3339()),
                "receipt": job.result_json.and_then(|value| serde_json::from_str::<serde_json::Value>(&value).ok())
            })),
        )
            .into_response(),
        Err(status) => (
            status,
            Json(serde_json::json!({
                "error": if status == StatusCode::FORBIDDEN { "forbidden" } else { "not_found" },
                "message": "Job access is not permitted"
            })),
        )
            .into_response(),
    }
}

/// Get the persisted bounded completion receipt for a job.
async fn get_job_logs(
    Extension(pool): Extension<Arc<Pool>>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let uuid = match Uuid::parse_str(&id) {
        Ok(uuid) => uuid,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": "invalid_id",
                    "message": "Invalid job ID format"
                })),
            )
                .into_response();
        }
    };
    match authorized_job(&pool, &claims, JobId::from(uuid)).await {
        Ok(job) => {
            let receipt = job
                .result_json
                .and_then(|value| serde_json::from_str::<serde_json::Value>(&value).ok());
            match JobQueries::list_logs(&pool, JobId::from(uuid)).await {
                Ok(logs) => (
                    StatusCode::OK,
                    Json(serde_json::json!({
                        "job_id": id,
                        "receipt": receipt,
                        "logs": logs,
                    })),
                )
                    .into_response(),
                Err(error) => {
                    tracing::error!(%error, %uuid, "failed to load job logs");
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(serde_json::json!({
                            "error": "log_read_failed",
                            "message": "Job logs are temporarily unavailable"
                        })),
                    )
                        .into_response()
                }
            }
        }
        Err(status) => (
            status,
            Json(serde_json::json!({
                "error": if status == StatusCode::FORBIDDEN { "forbidden" } else { "not_found" },
                "message": "Job access is not permitted"
            })),
        )
            .into_response(),
    }
}

/// Cancel a job through the durable control-plane state. The scheduler and
/// runner observe this row, so API and scheduler remain safe as separate
/// processes; no shared in-memory scheduler extension is required.
async fn cancel_job(
    Extension(pool): Extension<Arc<Pool>>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let uuid = match Uuid::parse_str(&id) {
        Ok(uuid) => uuid,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": "invalid_id",
                    "message": "Invalid job ID format"
                })),
            )
                .into_response();
        }
    };
    let job_id = JobId::from(uuid);
    let job = match authorized_job(&pool, &claims, job_id).await {
        Ok(job) => job,
        Err(status) => {
            return (
                status,
                Json(serde_json::json!({
                    "error": if status == StatusCode::FORBIDDEN { "forbidden" } else { "not_found" },
                    "message": "Job cancellation is not permitted"
                })),
            )
                .into_response();
        }
    };
    if let Some(status) = JobStatus::from_str(&job.status) {
        if status.is_terminal() && status != JobStatus::Cancelled {
            return (
                StatusCode::CONFLICT,
                Json(serde_json::json!({
                    "error": "job_already_terminal",
                    "status": job.status
                })),
            )
                .into_response();
        }
        if status == JobStatus::Cancelled {
            return (
                StatusCode::OK,
                Json(serde_json::json!({
                    "contract_version": "harness.job.v1",
                    "status": "cancelled",
                    "job_id": job_id.to_string()
                })),
            )
                .into_response();
        }
    }
    let receipt = serde_json::json!({
        "job_id": job_id.to_string(),
        "status": "cancelled",
        "reason": "api operator requested cancellation"
    })
    .to_string();
    match JobQueries::cancel(&pool, job_id, &receipt).await {
        Ok(()) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "contract_version": "harness.job.v1",
                "status": "cancelled",
                "job_id": job_id.to_string()
            })),
        )
            .into_response(),
        Err(error) => {
            tracing::error!(%error, %job_id, "failed to persist API job cancellation");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "database_error", "message": "Failed to persist cancellation"})),
            )
                .into_response()
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
}

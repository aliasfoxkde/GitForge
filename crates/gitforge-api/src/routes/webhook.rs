//! Webhook receiver routes for CI triggers
//!
//! Allows external services to trigger pipeline runs via webhooks.

use crate::middleware::AuthenticatedUser;
use axum::{
    extract::{Extension, Path},
    http::StatusCode,
    response::IntoResponse,
    routing::post,
    Json, Router,
};
use gitforge_common::{JobId, PipelineId, RepoId};
use gitforge_db::{
    models::JobStatus,
    queries::{JobQueries, PipelineQueries, PipelineRunQueries},
    Pool,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;

/// Webhook payload for triggering a pipeline
#[derive(Debug, Deserialize, Serialize)]
pub struct WebhookTriggerPayload {
    /// Repository ID
    pub repo_id: String,
    /// Commit hash
    pub commit_hash: String,
    /// Branch name
    pub branch: String,
    /// Previous commit hash when supplied by the webhook provider. A missing
    /// value is treated as an initial push by the CI bridge.
    #[serde(default)]
    pub old_commit_hash: Option<String>,
    /// Optional pipeline name (defaults to "default")
    pub pipeline_name: Option<String>,
}

/// Webhook trigger response
#[derive(Debug, Serialize)]
pub struct WebhookTriggerResponse {
    pub success: bool,
    pub message: String,
    pub pipeline_id: Option<String>,
}

/// HTTP client for the separately deployed CI orchestrator. The API gateway
/// must hand webhook execution to CI so CI can create the run-owned checkout,
/// register the pipeline engine, and progress the dependency DAG.
#[derive(Clone)]
pub struct CiTriggerClient {
    url: reqwest::Url,
    token: String,
    client: reqwest::Client,
}

impl CiTriggerClient {
    pub fn new(url: impl Into<String>, token: impl Into<String>) -> Result<Self, String> {
        let url = reqwest::Url::parse(&url.into())
            .map_err(|error| format!("invalid CI trigger URL: {error}"))?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err(
                "CI trigger URL must use http/https, include a host, and omit credentials"
                    .to_string(),
            );
        }

        Ok(Self {
            url,
            token: token.into(),
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .map_err(|error| format!("failed to build CI trigger client: {error}"))?,
        })
    }

    async fn trigger(
        &self,
        repo_id: RepoId,
        branch: &str,
        old_commit_hash: Option<&str>,
        commit_hash: &str,
    ) -> Result<Option<String>, String> {
        let old_hash = old_commit_hash
            .filter(|hash| !hash.is_empty())
            .unwrap_or("0000000000000000000000000000000000000000");
        let response = self
            .client
            .post(self.url.clone())
            .header("x-gitforge-trigger-token", &self.token)
            .json(&serde_json::json!({
                "repo_id": repo_id.to_string(),
                "ref_name": branch,
                "old_hash": old_hash,
                "new_hash": commit_hash,
                "working_dir": null
            }))
            .send()
            .await
            .map_err(|error| format!("CI trigger request failed: {error}"))?;
        let status = response.status();
        let body: serde_json::Value = response
            .json()
            .await
            .map_err(|error| format!("CI trigger returned invalid JSON: {error}"))?;
        if !status.is_success() {
            return Err(format!("CI trigger returned HTTP {status}: {body}"));
        }
        Ok(body
            .get("pipeline_run_id")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string))
    }
}

/// Webhook routes
pub fn webhook_routes<S: Clone + Send + Sync + 'static>() -> Router<S> {
    Router::new().route("/webhook/trigger/{pipeline_id}", post(trigger_pipeline))
}

/// Trigger a pipeline via webhook
async fn trigger_pipeline(
    Extension(pool): Extension<Arc<Pool>>,
    ci_trigger: Option<Extension<Arc<CiTriggerClient>>>,
    _user: AuthenticatedUser,
    Path(pipeline_id): Path<String>,
    Json(payload): Json<WebhookTriggerPayload>,
) -> impl IntoResponse {
    tracing::info!(
        "Webhook trigger for pipeline {} from repo {}",
        pipeline_id,
        payload.repo_id
    );

    // Parse pipeline ID
    let pipeline_uuid = match uuid::Uuid::parse_str(&pipeline_id) {
        Ok(uuid) => PipelineId::from(uuid),
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(WebhookTriggerResponse {
                    success: false,
                    message: "Invalid pipeline ID format".to_string(),
                    pipeline_id: None,
                }),
            )
                .into_response();
        }
    };

    // Verify pipeline exists
    match PipelineQueries::get(&pool, pipeline_uuid).await {
        Ok(Some(pipeline)) => {
            let repo_id = match uuid::Uuid::parse_str(&payload.repo_id) {
                Ok(uuid) if RepoId::from(uuid) == pipeline.repo_id => RepoId::from(uuid),
                _ => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(WebhookTriggerResponse {
                            success: false,
                            message: "Webhook repository does not match pipeline".to_string(),
                            pipeline_id: None,
                        }),
                    )
                        .into_response();
                }
            };

            if let Some(Extension(client)) = ci_trigger {
                match client
                    .trigger(
                        repo_id,
                        &payload.branch,
                        payload.old_commit_hash.as_deref(),
                        &payload.commit_hash,
                    )
                    .await
                {
                    Ok(run_id) => {
                        return (
                            StatusCode::ACCEPTED,
                            Json(WebhookTriggerResponse {
                                success: true,
                                message: format!(
                                    "Pipeline delegated to CI for branch '{}'{}",
                                    payload.branch,
                                    run_id
                                        .as_deref()
                                        .map(|id| format!(" (run {id})"))
                                        .unwrap_or_default()
                                ),
                                pipeline_id: Some(pipeline_id),
                            }),
                        )
                            .into_response();
                    }
                    Err(error) => {
                        tracing::error!(%error, %pipeline_id, "failed to delegate webhook to CI");
                        return (
                            StatusCode::BAD_GATEWAY,
                            Json(WebhookTriggerResponse {
                                success: false,
                                message: "CI trigger unavailable".to_string(),
                                pipeline_id: None,
                            }),
                        )
                            .into_response();
                    }
                }
            }

            let definition: gitforge_ci::PipelineDefinition = match serde_json::from_value(
                pipeline.config.clone(),
            ) {
                Ok(definition) => definition,
                Err(error) => {
                    tracing::error!(%error, %pipeline_id, "stored pipeline definition is invalid");
                    return (
                        StatusCode::UNPROCESSABLE_ENTITY,
                        Json(WebhookTriggerResponse {
                            success: false,
                            message: "Stored pipeline definition is invalid".to_string(),
                            pipeline_id: None,
                        }),
                    )
                        .into_response();
                }
            };
            let Some(job_definition) = definition.jobs.iter().find(|job| job.needs.is_empty())
            else {
                return (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    Json(WebhookTriggerResponse {
                        success: false,
                        message: "Pipeline has no runnable entry job".to_string(),
                        pipeline_id: None,
                    }),
                )
                    .into_response();
            };
            let timeout_secs = match job_definition.timeout_secs() {
                Ok(timeout_secs) => timeout_secs,
                Err(error) => {
                    tracing::error!(%error, job = %job_definition.name, "pipeline job timeout is invalid");
                    return (
                        StatusCode::UNPROCESSABLE_ENTITY,
                        Json(WebhookTriggerResponse {
                            success: false,
                            message: format!(
                                "Invalid timeout for job '{}': {error}",
                                job_definition.name
                            ),
                            pipeline_id: None,
                        }),
                    )
                        .into_response();
                }
            };
            let commands: Vec<String> = job_definition
                .steps
                .iter()
                .map(|step| step.run.clone())
                .collect();
            let working_dir = job_definition
                .steps
                .iter()
                .find_map(|step| step.working_directory.clone());

            // Persist the run before queueing so scheduler job foreign keys
            // and recovery have a durable parent record.
            let run = gitforge_db::models::PipelineRun::new(
                pipeline.id,
                repo_id,
                "webhook".to_string(),
                payload.commit_hash.clone(),
            );
            let run_id = run.id;
            if let Err(error) = PipelineRunQueries::create(&pool, &run).await {
                tracing::error!(%error, %pipeline_id, "failed to persist webhook pipeline run");
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(WebhookTriggerResponse {
                        success: false,
                        message: "Failed to persist pipeline run".to_string(),
                        pipeline_id: None,
                    }),
                )
                    .into_response();
            }

            // Pipeline exists - persist its first runnable job using the
            // durable queue contract. The API and CI scheduler are separate
            // processes in production, so relying on an optional in-memory
            // Scheduler extension loses work (the run remains pending forever).
            // The scheduler reloads queued jobs from the shared database on
            // its next bounded tick.
            tracing::info!(
                "Triggering pipeline {} for repo {} at commit {}",
                pipeline_id,
                payload.repo_id,
                payload.commit_hash
            );

            let job_id = JobId::new();
            let idempotency_key = format!("webhook:{pipeline_id}:{}", payload.commit_hash);
            let fingerprint = serde_json::json!({
                "name": job_definition.name,
                "commands": commands,
                "working_dir": working_dir,
                "image": job_definition.image,
                "timeout_secs": timeout_secs,
            })
            .to_string();
            let scope = format!("webhook:{pipeline_id}");

            match JobQueries::get_idempotency(&pool, &scope, &idempotency_key).await {
                Ok(Some((existing_job_id, stored_fingerprint))) => {
                    if stored_fingerprint != fingerprint {
                        let _ = PipelineRunQueries::update_status(&pool, run_id, "failed").await;
                        return (
                            StatusCode::CONFLICT,
                            Json(WebhookTriggerResponse {
                                success: false,
                                message: "Webhook idempotency key was reused with a different job"
                                    .to_string(),
                                pipeline_id: None,
                            }),
                        )
                            .into_response();
                    }
                    let _ = PipelineRunQueries::update_status(&pool, run_id, "cancelled").await;
                    tracing::info!(%existing_job_id, %pipeline_id, "Webhook delivery already queued");
                }
                Ok(None) => {
                    if let Err(error) = JobQueries::reserve_idempotency(
                        &pool,
                        &scope,
                        &idempotency_key,
                        &fingerprint,
                        job_id,
                    )
                    .await
                    {
                        let _ = PipelineRunQueries::update_status(&pool, run_id, "failed").await;
                        tracing::error!(%error, %pipeline_id, "failed to reserve webhook job idempotency key");
                        return (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            Json(WebhookTriggerResponse {
                                success: false,
                                message: "Failed to queue pipeline job".to_string(),
                                pipeline_id: None,
                            }),
                        )
                            .into_response();
                    }
                    let mut job =
                        gitforge_db::models::Job::new(run_id, job_definition.name.clone());
                    job.id = job_id;
                    job.commands = commands.clone();
                    job.image = job_definition.image.clone();
                    job.working_dir = working_dir.clone();
                    job.timeout_secs = timeout_secs;
                    job.status = JobStatus::Queued.as_str().to_string();
                    if let Err(error) = JobQueries::create(&pool, &job).await {
                        let _ =
                            JobQueries::delete_idempotency(&pool, &scope, &idempotency_key).await;
                        let _ = PipelineRunQueries::update_status(&pool, run_id, "failed").await;
                        tracing::error!(%error, %pipeline_id, "failed to persist webhook job");
                        return (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            Json(WebhookTriggerResponse {
                                success: false,
                                message: "Failed to queue pipeline job".to_string(),
                                pipeline_id: None,
                            }),
                        )
                            .into_response();
                    }
                    tracing::info!(%job_id, %pipeline_id, "Persisted webhook job in durable queue");
                }
                Err(error) => {
                    let _ = PipelineRunQueries::update_status(&pool, run_id, "failed").await;
                    tracing::error!(%error, %pipeline_id, "failed to inspect webhook idempotency key");
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(WebhookTriggerResponse {
                            success: false,
                            message: "Failed to queue pipeline job".to_string(),
                            pipeline_id: None,
                        }),
                    )
                        .into_response();
                }
            }

            (
                StatusCode::OK,
                Json(WebhookTriggerResponse {
                    success: true,
                    message: format!("Pipeline triggered for branch '{}'", payload.branch),
                    pipeline_id: Some(pipeline_id),
                }),
            )
                .into_response()
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(WebhookTriggerResponse {
                success: false,
                message: "Pipeline not found".to_string(),
                pipeline_id: None,
            }),
        )
            .into_response(),
        Err(e) => {
            tracing::error!("Failed to get pipeline: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(WebhookTriggerResponse {
                    success: false,
                    message: "Database error".to_string(),
                    pipeline_id: None,
                }),
            )
                .into_response()
        }
    }
}

#[cfg(test)]
mod ci_trigger_client_tests {
    use super::CiTriggerClient;

    #[test]
    fn accepts_http_ci_trigger_url_without_credentials() {
        assert!(CiTriggerClient::new("http://127.0.0.1:42781/pipelines/trigger", "token").is_ok());
    }

    #[test]
    fn rejects_invalid_ci_trigger_url() {
        assert!(CiTriggerClient::new("not a URL", "token").is_err());
    }

    #[test]
    fn rejects_unsupported_ci_trigger_scheme() {
        assert!(CiTriggerClient::new("ftp://127.0.0.1/trigger", "token").is_err());
    }

    #[test]
    fn rejects_ci_trigger_credentials() {
        assert!(CiTriggerClient::new("http://user:password@127.0.0.1/trigger", "token").is_err());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_webhook_trigger_payload_deserialization() {
        let json = r#"{
            "repo_id": "550e8400-e29b-41d4-a716-446655440000",
            "commit_hash": "abc123def456",
            "branch": "main"
        }"#;
        let payload: WebhookTriggerPayload = serde_json::from_str(json).unwrap();
        assert_eq!(payload.commit_hash, "abc123def456");
        assert_eq!(payload.branch, "main");
    }

    #[test]
    fn test_webhook_trigger_payload_with_pipeline_name() {
        let json = r#"{
            "repo_id": "550e8400-e29b-41d4-a716-446655440000",
            "commit_hash": "abc123",
            "branch": "develop",
            "pipeline_name": "ci-pipeline"
        }"#;
        let payload: WebhookTriggerPayload = serde_json::from_str(json).unwrap();
        assert_eq!(payload.pipeline_name, Some("ci-pipeline".to_string()));
    }

    #[test]
    fn test_webhook_trigger_response_serialization() {
        let response = WebhookTriggerResponse {
            success: true,
            message: "Pipeline triggered".to_string(),
            pipeline_id: Some("pipeline-123".to_string()),
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("success"));
        assert!(json.contains("pipeline-123"));
    }

    #[test]
    fn test_webhook_trigger_response_failure() {
        let response = WebhookTriggerResponse {
            success: false,
            message: "Pipeline not found".to_string(),
            pipeline_id: None,
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("false"));
        assert!(json.contains("pipeline_id")); // field is present but null
    }

    #[test]
    fn test_webhook_trigger_payload_empty_branch() {
        let json = r#"{
            "repo_id": "550e8400-e29b-41d4-a716-446655440000",
            "commit_hash": "abc123",
            "branch": ""
        }"#;
        let payload: WebhookTriggerPayload = serde_json::from_str(json).unwrap();
        assert_eq!(payload.branch, "");
    }
}

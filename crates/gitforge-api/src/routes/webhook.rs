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
    queries::{PipelineQueries, PipelineRunQueries},
    Pool,
};
use gitforge_scheduler::{assigner::JobExecutionDefinition, Scheduler};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Webhook payload for triggering a pipeline
#[derive(Debug, Deserialize, Serialize)]
pub struct WebhookTriggerPayload {
    /// Repository ID
    pub repo_id: String,
    /// Commit hash
    pub commit_hash: String,
    /// Branch name
    pub branch: String,
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

/// Webhook routes
pub fn webhook_routes<S: Clone + Send + Sync + 'static>() -> Router<S> {
    Router::new().route("/webhook/trigger/{pipeline_id}", post(trigger_pipeline))
}

/// Trigger a pipeline via webhook
async fn trigger_pipeline(
    Extension(pool): Extension<Arc<Pool>>,
    scheduler: Option<Extension<Arc<Scheduler>>>,
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
            let commands = job_definition
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

            // Pipeline exists - enqueue its first runnable job with the
            // stored execution contract. Dependent-job progression is handled
            // by the CI DAG integration and is intentionally not fabricated
            // by this webhook adapter.
            tracing::info!(
                "Triggering pipeline {} for repo {} at commit {}",
                pipeline_id,
                payload.repo_id,
                payload.commit_hash
            );

            // Create a job for this pipeline run
            let job_id = JobId::new();

            // Enqueue the job to the scheduler (if available)
            if let Some(Extension(sched)) = scheduler {
                sched
                    .enqueue_with_definition_and_image_and_timeout(
                        job_id,
                        run_id,
                        repo_id,
                        JobExecutionDefinition {
                            commands,
                            image: job_definition.image.clone(),
                            working_dir,
                            timeout_secs,
                        },
                    )
                    .await;
                tracing::info!(
                    "Enqueued job {} for pipeline {} on branch {}",
                    job_id,
                    pipeline_id,
                    payload.branch
                );
            } else {
                tracing::warn!("No scheduler available, job not enqueued");
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

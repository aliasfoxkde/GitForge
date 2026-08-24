//! Webhook receiver routes for CI triggers
//!
//! Allows external services to trigger pipeline runs via webhooks.

use crate::auth::ApiAuth;
use axum::{
    extract::{Extension, Path},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::post,
    Json, Router,
};
use gitforge_common::PipelineId;
use gitforge_db::{queries::PipelineQueries, Pool};
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
    pub pipeline_run_id: Option<String>,
    pub event_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CiTriggerResponse {
    accepted: bool,
    event_id: Option<String>,
    pipeline_run_id: Option<String>,
}

/// Webhook routes
pub fn webhook_routes<S: Clone + Send + Sync + 'static>() -> Router<S> {
    Router::new().route("/webhook/trigger/{pipeline_id}", post(trigger_pipeline))
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

/// Trigger a pipeline via webhook
async fn trigger_pipeline(
    Extension(pool): Extension<Arc<Pool>>,
    Extension(auth): Extension<Arc<ApiAuth>>,
    headers: HeaderMap,
    Path(pipeline_id): Path<String>,
    Json(payload): Json<WebhookTriggerPayload>,
) -> impl IntoResponse {
    // Check auth
    if let Err(e) = extract_user(&auth, &headers) {
        return e.into_response();
    }

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
                    pipeline_run_id: None,
                    event_id: None,
                }),
            )
                .into_response();
        }
    };

    // Verify pipeline exists
    match PipelineQueries::get(&pool, pipeline_uuid).await {
        Ok(Some(_pipeline)) => {
            tracing::info!(
                "Triggering pipeline {} for repo {} at commit {}",
                pipeline_id,
                payload.repo_id,
                payload.commit_hash
            );
            let repo_id = match uuid::Uuid::parse_str(&payload.repo_id) {
                Ok(value) => value.to_string(),
                Err(_) => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(WebhookTriggerResponse {
                            success: false,
                            message: "Invalid repository ID format".to_string(),
                            pipeline_id: None,
                            pipeline_run_id: None,
                            event_id: None,
                        }),
                    )
                        .into_response();
                }
            };
            if !is_git_hash(&payload.commit_hash) || payload.branch.trim().is_empty() {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(WebhookTriggerResponse {
                        success: false,
                        message: "Invalid commit hash or branch".to_string(),
                        pipeline_id: None,
                        pipeline_run_id: None,
                        event_id: None,
                    }),
                )
                    .into_response();
            }
            let token = match std::env::var("GITFORGE_CI_TRIGGER_TOKEN") {
                Ok(token) if !token.is_empty() => token,
                _ => {
                    return (
                        StatusCode::SERVICE_UNAVAILABLE,
                        Json(WebhookTriggerResponse {
                            success: false,
                            message: "CI trigger integration is not configured".to_string(),
                            pipeline_id: None,
                            pipeline_run_id: None,
                            event_id: None,
                        }),
                    )
                        .into_response();
                }
            };
            let ref_name = if payload.branch.starts_with("refs/") {
                payload.branch.clone()
            } else {
                format!("refs/heads/{}", payload.branch)
            };
            let ci_url = std::env::var("GITFORGE_CI_BASE_URL")
                .unwrap_or_else(|_| "http://127.0.0.1:42781".to_string());
            let ci_response = reqwest::Client::new()
                .post(format!(
                    "{}/pipelines/trigger",
                    ci_url.trim_end_matches('/')
                ))
                .header("x-gitforge-trigger-token", token)
                .json(&serde_json::json!({
                    "repo_id": repo_id,
                    "pipeline_id": pipeline_id,
                    "ref_name": ref_name,
                    "old_hash": "0000000000000000000000000000000000000000",
                    "new_hash": payload.commit_hash,
                }))
                .timeout(std::time::Duration::from_secs(10))
                .send()
                .await;
            match ci_response {
                Ok(response) => {
                    let status = response.status();
                    let body = response.json::<CiTriggerResponse>().await.ok();
                    if !status.is_success() || !body.as_ref().is_some_and(|body| body.accepted) {
                        return (
                            StatusCode::from_u16(status.as_u16())
                                .unwrap_or(StatusCode::BAD_GATEWAY),
                            Json(WebhookTriggerResponse {
                                success: false,
                                message: "CI trigger rejected request".to_string(),
                                pipeline_id: Some(pipeline_id),
                                pipeline_run_id: None,
                                event_id: body.and_then(|body| body.event_id),
                            }),
                        )
                            .into_response();
                    }
                    let body = body.expect("successful CI response must have a body");
                    (
                        StatusCode::ACCEPTED,
                        Json(WebhookTriggerResponse {
                            success: true,
                            message: format!("Pipeline accepted for branch '{}'", payload.branch),
                            pipeline_id: Some(pipeline_id),
                            pipeline_run_id: body.pipeline_run_id,
                            event_id: body.event_id,
                        }),
                    )
                        .into_response()
                }
                Err(error) => {
                    tracing::error!(%error, "CI trigger request failed");
                    (
                        StatusCode::BAD_GATEWAY,
                        Json(WebhookTriggerResponse {
                            success: false,
                            message: "CI trigger service unavailable".to_string(),
                            pipeline_id: Some(pipeline_id),
                            pipeline_run_id: None,
                            event_id: None,
                        }),
                    )
                        .into_response()
                }
            }
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(WebhookTriggerResponse {
                success: false,
                message: "Pipeline not found".to_string(),
                pipeline_id: None,
                pipeline_run_id: None,
                event_id: None,
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
                    pipeline_run_id: None,
                    event_id: None,
                }),
            )
                .into_response()
        }
    }
}

fn is_git_hash(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
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
            pipeline_run_id: None,
            event_id: None,
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
            pipeline_run_id: None,
            event_id: None,
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

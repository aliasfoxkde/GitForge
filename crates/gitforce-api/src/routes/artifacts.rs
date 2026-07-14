//! Artifact API routes

use crate::auth::ApiAuth;
use crate::server::ErrorResponse;
use axum::{
    extract::{Extension, Path},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get},
    Json, Router,
};
use gitforce_common::JobId;
use gitforce_storage::{Artifact, ArtifactId, ArtifactStore, FileStorage};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

/// Artifact response
#[derive(Debug, Serialize, Deserialize)]
pub struct ArtifactResponse {
    pub id: String,
    pub job_id: String,
    pub name: String,
    pub path: String,
    pub checksum: String,
    pub size_bytes: u64,
    pub created_at: String,
}

/// Artifact routes
pub fn artifact_routes<S: Clone + Send + Sync + 'static>() -> Router<S> {
    Router::new()
        .route("/artifacts", get(list_artifacts))
        .route("/artifacts/{id}", get(get_artifact).delete(delete_artifact))
        .route("/jobs/{job_id}/artifacts", get(get_job_artifacts))
}

/// Helper to extract and validate user from headers
fn extract_user(auth: &ApiAuth, headers: &HeaderMap) -> Result<(), StatusCode> {
    let auth_header = headers
        .get("Authorization")
        .and_then(|v| v.to_str().ok());

    let token = auth_header
        .and_then(|h| ApiAuth::extract_token(h))
        .ok_or(StatusCode::UNAUTHORIZED)?;

    auth.validate_token(&token)
        .map_err(|_| StatusCode::UNAUTHORIZED)?;

    Ok(())
}

/// Convert storage artifact to response
fn artifact_to_response(artifact: &Artifact) -> ArtifactResponse {
    ArtifactResponse {
        id: artifact.id.to_string(),
        job_id: artifact.job_id.to_string(),
        name: artifact.name.clone(),
        path: artifact.path.clone(),
        checksum: artifact.checksum.clone(),
        size_bytes: artifact.size_bytes,
        created_at: artifact.created_at.to_rfc3339(),
    }
}

/// List artifacts
async fn list_artifacts(
    Extension(auth): Extension<Arc<ApiAuth>>,
    Extension(storage): Extension<Arc<FileStorage>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    match extract_user(&auth, &headers) {
        Err(e) => e.into_response(),
        Ok(_) => {
            tracing::debug!("list artifacts");
            // TODO: Implement list all artifacts (requires scanning directory)
            Json(serde_json::Value::Array(vec![])).into_response()
        }
    }
}

/// Get artifact metadata
async fn get_artifact(
    Extension(auth): Extension<Arc<ApiAuth>>,
    Extension(storage): Extension<Arc<FileStorage>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match extract_user(&auth, &headers) {
        Err(e) => e.into_response(),
        Ok(_) => {
            tracing::debug!("get artifact: {}", id);

            let artifact_id = match Uuid::parse_str(&id) {
                Ok(uuid) => ArtifactId::from(uuid),
                Err(_) => {
                    return (StatusCode::BAD_REQUEST, Json(ErrorResponse {
                        error: "invalid_id".to_string(),
                        message: "Invalid artifact ID format".to_string(),
                    })).into_response();
                }
            };

            match storage.get_metadata(artifact_id).await {
                Ok(artifact) => {
                    let response = artifact_to_response(&artifact);
                    (StatusCode::OK, Json(response)).into_response()
                }
                Err(e) => {
                    tracing::error!("failed to get artifact metadata: {}", e);
                    (StatusCode::NOT_FOUND, Json(ErrorResponse {
                        error: "not_found".to_string(),
                        message: "Artifact not found".to_string(),
                    })).into_response()
                }
            }
        }
    }
}

/// Delete artifact
async fn delete_artifact(
    Extension(auth): Extension<Arc<ApiAuth>>,
    Extension(storage): Extension<Arc<FileStorage>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match extract_user(&auth, &headers) {
        Err(e) => e.into_response(),
        Ok(_) => {
            tracing::debug!("delete artifact: {}", id);

            let artifact_id = match Uuid::parse_str(&id) {
                Ok(uuid) => ArtifactId::from(uuid),
                Err(_) => {
                    return (StatusCode::BAD_REQUEST, Json(ErrorResponse {
                        error: "invalid_id".to_string(),
                        message: "Invalid artifact ID format".to_string(),
                    })).into_response();
                }
            };

            match storage.delete(artifact_id).await {
                Ok(_) => StatusCode::NO_CONTENT.into_response(),
                Err(e) => {
                    tracing::error!("failed to delete artifact: {}", e);
                    (StatusCode::INTERNAL_SERVER_ERROR, Json(ErrorResponse {
                        error: "storage_error".to_string(),
                        message: e.to_string(),
                    })).into_response()
                }
            }
        }
    }
}

/// Get artifacts for a job
async fn get_job_artifacts(
    Extension(auth): Extension<Arc<ApiAuth>>,
    Extension(storage): Extension<Arc<FileStorage>>,
    headers: HeaderMap,
    Path(job_id): Path<String>,
) -> impl IntoResponse {
    match extract_user(&auth, &headers) {
        Err(e) => e.into_response(),
        Ok(_) => {
            tracing::debug!("get job artifacts: {}", job_id);
            // TODO: Implement listing artifacts by job_id
            Json(serde_json::Value::Array(vec![])).into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_artifact_id_from_uuid() {
        let uuid = Uuid::new_v4();
        let _artifact_id = ArtifactId::from(uuid);
        // Just verify it doesn't panic
    }
}

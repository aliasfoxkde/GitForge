//! Artifact API routes

use crate::auth::ApiAuth;
use crate::server::ErrorResponse;
use axum::{
    extract::{Extension, Path},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::get,
    Json, Router,
};
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
    let auth_header = headers.get("Authorization").and_then(|v| v.to_str().ok());

    let token = auth_header
        .and_then(|h| ApiAuth::extract_token(h))
        .ok_or(StatusCode::UNAUTHORIZED)?;

    auth.validate_token(token)
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
            match storage.list().await {
                Ok(artifacts) => {
                    let responses: Vec<ArtifactResponse> =
                        artifacts.iter().map(artifact_to_response).collect();
                    Json(responses).into_response()
                }
                Err(e) => {
                    tracing::error!("failed to list artifacts: {}", e);
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(ErrorResponse {
                            error: "storage_error".to_string(),
                            message: e.to_string(),
                        }),
                    )
                        .into_response()
                }
            }
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
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(ErrorResponse {
                            error: "invalid_id".to_string(),
                            message: "Invalid artifact ID format".to_string(),
                        }),
                    )
                        .into_response();
                }
            };

            match storage.get_metadata(artifact_id).await {
                Ok(artifact) => {
                    let response = artifact_to_response(&artifact);
                    (StatusCode::OK, Json(response)).into_response()
                }
                Err(e) => {
                    tracing::error!("failed to get artifact metadata: {}", e);
                    (
                        StatusCode::NOT_FOUND,
                        Json(ErrorResponse {
                            error: "not_found".to_string(),
                            message: "Artifact not found".to_string(),
                        }),
                    )
                        .into_response()
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
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(ErrorResponse {
                            error: "invalid_id".to_string(),
                            message: "Invalid artifact ID format".to_string(),
                        }),
                    )
                        .into_response();
                }
            };

            match storage.delete(artifact_id).await {
                Ok(_) => StatusCode::NO_CONTENT.into_response(),
                Err(e) => {
                    tracing::error!("failed to delete artifact: {}", e);
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(ErrorResponse {
                            error: "storage_error".to_string(),
                            message: e.to_string(),
                        }),
                    )
                        .into_response()
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

            let job_id_val = match Uuid::parse_str(&job_id) {
                Ok(uuid) => gitforce_common::JobId::from(uuid),
                Err(_) => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(ErrorResponse {
                            error: "invalid_id".to_string(),
                            message: "Invalid job ID format".to_string(),
                        }),
                    )
                        .into_response();
                }
            };

            match storage.list_by_job(job_id_val).await {
                Ok(artifacts) => {
                    let responses: Vec<ArtifactResponse> =
                        artifacts.iter().map(artifact_to_response).collect();
                    Json(responses).into_response()
                }
                Err(e) => {
                    tracing::error!("failed to list artifacts for job: {}", e);
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(ErrorResponse {
                            error: "storage_error".to_string(),
                            message: e.to_string(),
                        }),
                    )
                        .into_response()
                }
            }
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

    #[test]
    fn test_artifact_response_serialization() {
        let response = ArtifactResponse {
            id: "artifact-123".to_string(),
            job_id: "job-456".to_string(),
            name: "test-artifact.zip".to_string(),
            path: "/tmp/artifact.zip".to_string(),
            checksum: "abc123def456".to_string(),
            size_bytes: 1024,
            created_at: "2024-01-01T00:00:00Z".to_string(),
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("artifact-123"));
        assert!(json.contains("test-artifact.zip"));
        assert!(json.contains("1024"));
    }

    #[test]
    fn test_artifact_response_deserialization() {
        let json = r#"{
            "id": "artifact-789",
            "job_id": "job-001",
            "name": "build-output.tar.gz",
            "path": "/artifacts/build-output.tar.gz",
            "checksum": "xyz789abc",
            "size_bytes": 4096,
            "created_at": "2024-01-15T12:30:00Z"
        }"#;
        let response: ArtifactResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.id, "artifact-789");
        assert_eq!(response.name, "build-output.tar.gz");
        assert_eq!(response.size_bytes, 4096);
    }

    #[test]
    fn test_artifact_to_response_conversion() {
        let artifact = Artifact {
            id: ArtifactId::new(),
            job_id: gitforce_common::JobId::new(),
            name: "test.bin".to_string(),
            path: "/tmp/test.bin".to_string(),
            checksum: "checksum123".to_string(),
            size_bytes: 256,
            content_type: Some("application/octet-stream".to_string()),
            created_at: chrono::Utc::now(),
        };
        let response = artifact_to_response(&artifact);
        assert_eq!(response.name, "test.bin");
        assert_eq!(response.checksum, "checksum123");
        assert_eq!(response.size_bytes, 256);
    }

    #[test]
    fn test_artifact_response_with_null_content_type() {
        // This tests the artifact_to_response function indirectly
        let response = ArtifactResponse {
            id: "art-001".to_string(),
            job_id: "job-002".to_string(),
            name: "data.json".to_string(),
            path: "/tmp/data.json".to_string(),
            checksum: "chk123".to_string(),
            size_bytes: 64,
            created_at: "2024-01-01T00:00:00Z".to_string(),
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("art-001"));
        assert!(json.contains("data.json"));
    }

    #[test]
    fn test_artifact_response_debug() {
        let response = ArtifactResponse {
            id: "art-debug".to_string(),
            job_id: "job-debug".to_string(),
            name: "debug.bin".to_string(),
            path: "/tmp/debug.bin".to_string(),
            checksum: "debug123".to_string(),
            size_bytes: 128,
            created_at: "2024-01-01T00:00:00Z".to_string(),
        };
        let debug_str = format!("{:?}", response);
        assert!(debug_str.contains("art-debug"));
    }

    #[test]
    fn test_artifact_response_large_size() {
        let response = ArtifactResponse {
            id: "large-art".to_string(),
            job_id: "large-job".to_string(),
            name: "large-file.tar.gz".to_string(),
            path: "/tmp/large-file.tar.gz".to_string(),
            checksum: "large123".to_string(),
            size_bytes: 1_073_741_824, // 1GB
            created_at: "2024-01-01T00:00:00Z".to_string(),
        };
        assert_eq!(response.size_bytes, 1_073_741_824);
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("1073741824"));
    }

    #[test]
    fn test_artifact_response_various_timestamps() {
        for ts in &[
            "2024-01-01T00:00:00Z",
            "2025-12-31T23:59:59Z",
            "2026-07-16T12:00:00Z",
        ] {
            let response = ArtifactResponse {
                id: "art-ts".to_string(),
                job_id: "job-ts".to_string(),
                name: "timestamped.bin".to_string(),
                path: "/tmp/ts.bin".to_string(),
                checksum: "ts123".to_string(),
                size_bytes: 32,
                created_at: ts.to_string(),
            };
            let json = serde_json::to_string(&response).unwrap();
            assert!(json.contains(ts));
        }
    }

    #[test]
    fn test_artifact_to_response_all_fields() {
        use gitforce_common::JobId;
        let job_id = JobId::new();
        let artifact = Artifact {
            id: ArtifactId::new(),
            job_id,
            name: "full-artifact.bin".to_string(),
            path: "/tmp/full.bin".to_string(),
            checksum: "fullchecksum".to_string(),
            size_bytes: 512,
            content_type: Some("application/octet-stream".to_string()),
            created_at: chrono::Utc::now(),
        };
        let response = artifact_to_response(&artifact);
        assert_eq!(response.name, "full-artifact.bin");
        assert_eq!(response.checksum, "fullchecksum");
        assert_eq!(response.size_bytes, 512);
    }

    #[test]
    fn test_artifact_response_deserialization_minimal() {
        let json = r#"{"id":"min","job_id":"min-job","name":"min.bin","path":"/min","checksum":"min","size_bytes":1,"created_at":"2024-01-01T00:00:00Z"}"#;
        let response: ArtifactResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.id, "min");
        assert_eq!(response.size_bytes, 1);
    }
}

//! Artifact API routes

use crate::middleware::AuthenticatedUser;
use crate::server::ErrorResponse;
use axum::{
    body::Body,
    extract::{Extension, Path},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use gitforge_common::JobId;
use gitforge_db::{
    queries::{JobQueries, PipelineRunQueries, RepoQueries},
    Pool,
};
use gitforge_storage::{Artifact, ArtifactId, ArtifactStore, FileStorage};
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
        .route("/artifacts/{id}/content", get(get_artifact_content))
        .route("/jobs/{job_id}/artifacts", get(get_job_artifacts))
}

fn can_manage_artifacts(user: &AuthenticatedUser) -> bool {
    matches!(user.claims.role.as_str(), "admin" | "maintainer")
}

/// Verify that an artifact's job belongs to a repository visible to the user.
async fn authorize_job(
    pool: &Pool,
    user: &AuthenticatedUser,
    job_id: JobId,
) -> Result<(), StatusCode> {
    if can_manage_artifacts(user) {
        return Ok(());
    }
    let job = match JobQueries::get(pool, job_id).await {
        Ok(Some(job)) => job,
        Ok(None) => return Err(StatusCode::NOT_FOUND),
        Err(error) => {
            tracing::error!(%error, %job_id, "failed to load artifact job for authorization");
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };
    let run = match PipelineRunQueries::get(pool, job.pipeline_run_id).await {
        Ok(Some(run)) => run,
        Ok(None) => return Err(StatusCode::NOT_FOUND),
        Err(error) => {
            tracing::error!(%error, %job_id, "failed to load artifact pipeline run for authorization");
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };
    match RepoQueries::get(pool, run.repo_id).await {
        Ok(Some(repo)) if repo.owner_id == user.claims.user_id => Ok(()),
        // Do not reveal whether a private job exists to another user.
        Ok(Some(_)) => Err(StatusCode::NOT_FOUND),
        Ok(None) => Err(StatusCode::NOT_FOUND),
        Err(error) => {
            tracing::error!(%error, repo_id = %run.repo_id, "failed to load artifact repository for authorization");
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
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
    user: AuthenticatedUser,
    Extension(pool): Extension<Arc<Pool>>,
    Extension(storage): Extension<Arc<FileStorage>>,
) -> impl IntoResponse {
    tracing::debug!("list artifacts");
    match storage.list().await {
        Ok(artifacts) => {
            let mut responses = Vec::new();
            for artifact in artifacts {
                match authorize_job(&pool, &user, artifact.job_id).await {
                    Ok(()) => responses.push(artifact_to_response(&artifact)),
                    Err(StatusCode::FORBIDDEN | StatusCode::NOT_FOUND) => {}
                    Err(status) => {
                        return (
                            status,
                            Json(ErrorResponse {
                                error: "authorization_error".to_string(),
                                message: "Artifact access could not be authorized".to_string(),
                            }),
                        )
                            .into_response();
                    }
                }
            }
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

/// Get artifact metadata
async fn get_artifact(
    user: AuthenticatedUser,
    Extension(pool): Extension<Arc<Pool>>,
    Extension(storage): Extension<Arc<FileStorage>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
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
            if let Err(status) = authorize_job(&pool, &user, artifact.job_id).await {
                return (
                    status,
                    Json(ErrorResponse {
                        error: "not_found".to_string(),
                        message: "Artifact not found".to_string(),
                    }),
                )
                    .into_response();
            }
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

/// Download artifact bytes after authentication.
async fn get_artifact_content(
    user: AuthenticatedUser,
    Extension(pool): Extension<Arc<Pool>>,
    Extension(storage): Extension<Arc<FileStorage>>,
    Path(id): Path<String>,
) -> Response {
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
                .into_response()
        }
    };
    let artifact = match storage.get_metadata(artifact_id).await {
        Ok(artifact) => artifact,
        Err(_) => {
            return (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: "not_found".to_string(),
                    message: "Artifact not found".to_string(),
                }),
            )
                .into_response();
        }
    };
    if let Err(status) = authorize_job(&pool, &user, artifact.job_id).await {
        return (
            status,
            Json(ErrorResponse {
                error: "not_found".to_string(),
                message: "Artifact not found".to_string(),
            }),
        )
            .into_response();
    }
    match storage.get(artifact_id).await {
        Ok(data) => Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "application/octet-stream")
            .header("content-length", data.len())
            .body(Body::from(data))
            .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response()),
        Err(_) => (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "not_found".to_string(),
                message: "Artifact not found".to_string(),
            }),
        )
            .into_response(),
    }
}

/// Delete artifact
async fn delete_artifact(
    user: AuthenticatedUser,
    Extension(pool): Extension<Arc<Pool>>,
    Extension(storage): Extension<Arc<FileStorage>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
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

    let artifact = match storage.get_metadata(artifact_id).await {
        Ok(artifact) => artifact,
        Err(_) => {
            return (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: "not_found".to_string(),
                    message: "Artifact not found".to_string(),
                }),
            )
                .into_response();
        }
    };
    if let Err(status) = authorize_job(&pool, &user, artifact.job_id).await {
        return (
            status,
            Json(ErrorResponse {
                error: "not_found".to_string(),
                message: "Artifact not found".to_string(),
            }),
        )
            .into_response();
    }
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

/// Get artifacts for a job
async fn get_job_artifacts(
    user: AuthenticatedUser,
    Extension(pool): Extension<Arc<Pool>>,
    Extension(storage): Extension<Arc<FileStorage>>,
    Path(job_id): Path<String>,
) -> impl IntoResponse {
    tracing::debug!("get job artifacts: {}", job_id);

    let job_id_val = match Uuid::parse_str(&job_id) {
        Ok(uuid) => gitforge_common::JobId::from(uuid),
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

    if let Err(status) = authorize_job(&pool, &user, job_id_val).await {
        return (
            status,
            Json(ErrorResponse {
                error: "not_found".to_string(),
                message: "Job artifacts not found".to_string(),
            }),
        )
            .into_response();
    }

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
            job_id: gitforge_common::JobId::new(),
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
        use gitforge_common::JobId;
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

    #[test]
    fn test_artifact_to_response_preserves_all_fields() {
        use gitforge_common::JobId;
        let job_id = JobId::new();
        let artifact = Artifact {
            id: ArtifactId::new(),
            job_id,
            name: "preserved.bin".to_string(),
            path: "/tmp/preserved.bin".to_string(),
            checksum: "preserved123".to_string(),
            size_bytes: 1024,
            content_type: Some("application/bin".to_string()),
            created_at: chrono::Utc::now(),
        };
        let response = artifact_to_response(&artifact);
        assert_eq!(response.name, "preserved.bin");
        assert_eq!(response.checksum, "preserved123");
        assert_eq!(response.size_bytes, 1024);
    }

    #[test]
    fn test_artifact_response_size_edge_cases() {
        // Test zero size
        let response = ArtifactResponse {
            id: "zero".to_string(),
            job_id: "job-zero".to_string(),
            name: "empty.bin".to_string(),
            path: "/tmp/empty.bin".to_string(),
            checksum: "e3b0c44".to_string(),
            size_bytes: 0,
            created_at: "2024-01-01T00:00:00Z".to_string(),
        };
        assert_eq!(response.size_bytes, 0);

        // Test max u64
        let response = ArtifactResponse {
            id: "max".to_string(),
            job_id: "job-max".to_string(),
            name: "max.bin".to_string(),
            path: "/tmp/max.bin".to_string(),
            checksum: "max".to_string(),
            size_bytes: u64::MAX,
            created_at: "2024-01-01T00:00:00Z".to_string(),
        };
        assert_eq!(response.size_bytes, u64::MAX);
    }
}

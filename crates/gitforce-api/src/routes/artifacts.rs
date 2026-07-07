//! Artifact API routes

use axum::{
    extract::Path,
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get},
    Json, Router,
};
use serde::{Deserialize, Serialize};
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
        .route("/artifacts/:id", get(get_artifact))
        .route("/artifacts/:id", delete(delete_artifact))
        .route("/jobs/:job_id/artifacts", get(get_job_artifacts))
}

/// List artifacts
async fn list_artifacts() -> impl IntoResponse {
    Json(serde_json::Value::Array(vec![]))
}

/// Get artifact metadata
async fn get_artifact(Path(id): Path<String>) -> impl IntoResponse {
    tracing::debug!("get artifact: {}", id);
    (StatusCode::OK, Json(ArtifactResponse {
        id,
        job_id: Uuid::new_v4().to_string(),
        name: "test-artifact.zip".to_string(),
        path: "/artifacts/test-artifact.zip".to_string(),
        checksum: "abc123def456".to_string(),
        size_bytes: 1024,
        created_at: chrono::Utc::now().to_rfc3339(),
    }))
}

/// Delete artifact
async fn delete_artifact(Path(id): Path<String>) -> impl IntoResponse {
    tracing::debug!("delete artifact: {}", id);
    StatusCode::NO_CONTENT
}

/// Get artifacts for a job
async fn get_job_artifacts(Path(job_id): Path<String>) -> impl IntoResponse {
    tracing::debug!("get job artifacts: {}", job_id);
    Json(serde_json::Value::Array(vec![]))
}

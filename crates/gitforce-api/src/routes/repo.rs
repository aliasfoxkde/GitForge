//! Repository API routes

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get, post},
    Json, Router,
};
use gitforce_common::{Error, RepoId, Result};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Repository response
#[derive(Debug, Serialize, Deserialize)]
pub struct RepoResponse {
    pub id: String,
    pub name: String,
    pub owner_id: String,
    pub visibility: String,
    pub git_path: String,
    pub created_at: String,
}

/// Create repository request
#[derive(Debug, Deserialize)]
pub struct CreateRepoRequest {
    pub name: String,
    pub visibility: Option<String>,
}

/// Repository routes
pub fn repo_routes<S: Clone + Send + Sync + 'static>() -> Router<S> {
    Router::new()
        .route("/repos", get(list_repos))
        .route("/repos", post(create_repo))
        .route("/repos/:id", get(get_repo))
        .route("/repos/:id", delete(delete_repo))
}

/// List repositories
async fn list_repos() -> impl IntoResponse {
    // TODO: Implement actual listing
    Json(serde_json::Value::Array(vec![]))
}

/// Create a repository
async fn create_repo(Json(req): Json<CreateRepoRequest>) -> impl IntoResponse {
    // TODO: Implement actual creation
    tracing::debug!("create repo request: {:?}", req);
    (StatusCode::CREATED, Json(RepoResponse {
        id: RepoId::new().to_string(),
        name: req.name,
        owner_id: "system".to_string(),
        visibility: req.visibility.unwrap_or_else(|| "private".to_string()),
        git_path: "/tmp/repos/test".to_string(),
        created_at: chrono::Utc::now().to_rfc3339(),
    }))
}

/// Get a repository
async fn get_repo(Path(id): Path<String>) -> impl IntoResponse {
    // TODO: Implement actual retrieval
    tracing::debug!("get repo request: {}", id);
    (StatusCode::OK, Json(RepoResponse {
        id,
        name: "test-repo".to_string(),
        owner_id: "system".to_string(),
        visibility: "private".to_string(),
        git_path: "/tmp/repos/test".to_string(),
        created_at: chrono::Utc::now().to_rfc3339(),
    }))
}

/// Delete a repository
async fn delete_repo(Path(id): Path<String>) -> impl IntoResponse {
    // TODO: Implement actual deletion
    tracing::debug!("delete repo request: {}", id);
    StatusCode::NO_CONTENT
}

//! Repository API routes

use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get, post},
    Json, Router,
};
use gitforce_common::{RepoId, UserId};
use gitforce_db::{Pool, queries::RepoQueries};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

/// Repository response
#[derive(Debug, Serialize, Deserialize)]
pub struct RepoResponse {
    pub id: String,
    pub name: String,
    pub owner_id: String,
    pub visibility: String,
    pub git_path: String,
    pub created_at: String,
    pub updated_at: String,
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
        .route("/repos", get(list_repos).post(create_repo))
        .route("/repos/{id}", get(get_repo).delete(delete_repo))
}

/// List repositories
async fn list_repos(
    Extension(pool): Extension<Arc<Pool>>,
) -> impl IntoResponse {
    match RepoQueries::list(&pool).await {
        Ok(repos) => {
            let response: Vec<RepoResponse> = repos.into_iter().map(|r| RepoResponse {
                id: r.id.to_string(),
                name: r.name,
                owner_id: r.owner_id.to_string(),
                visibility: r.visibility,
                git_path: r.git_path,
                created_at: r.created_at.to_rfc3339(),
                updated_at: r.updated_at.to_rfc3339(),
            }).collect();
            return Json(response).into_response();
        }
        Err(e) => {
            tracing::error!("failed to list repos: {}", e);
            return Json(serde_json::Value::Array(vec![])).into_response();
        }
    }
}

/// Create a repository
async fn create_repo(
    Extension(pool): Extension<Arc<Pool>>,
    Json(req): Json<CreateRepoRequest>,
) -> impl IntoResponse {
    tracing::debug!("create repo request: {:?}", req);

    // For MVP, use a system user. In production, get from auth context.
    let owner_id = UserId::new();
    let visibility = req.visibility.unwrap_or_else(|| "private".to_string());
    let git_path = format!("/git/repos/{}", req.name);

    let repo = gitforce_db::models::Repository::new(
        req.name,
        owner_id,
        git_path.clone(),
    );

    match RepoQueries::create(&pool, &repo).await {
        Ok(_) => {
            let response = RepoResponse {
                id: repo.id.to_string(),
                name: repo.name,
                owner_id: repo.owner_id.to_string(),
                visibility: repo.visibility,
                git_path: repo.git_path,
                created_at: repo.created_at.to_rfc3339(),
                updated_at: repo.updated_at.to_rfc3339(),
            };
            return (StatusCode::CREATED, Json(response)).into_response();
        }
        Err(e) => {
            tracing::error!("failed to create repo: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({
                "error": "database_error",
                "message": format!("failed to create repository: {}", e)
            }))).into_response();
        }
    }
}

/// Get a repository
async fn get_repo(
    Extension(pool): Extension<Arc<Pool>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    tracing::debug!("get repo request: {}", id);

    let response = match Uuid::parse_str(&id) {
        Ok(uuid) => {
            let repo_id = RepoId::from(uuid);
            match RepoQueries::get(&pool, repo_id).await {
                Ok(Some(repo)) => {
                    let response = RepoResponse {
                        id: repo.id.to_string(),
                        name: repo.name,
                        owner_id: repo.owner_id.to_string(),
                        visibility: repo.visibility,
                        git_path: repo.git_path,
                        created_at: repo.created_at.to_rfc3339(),
                        updated_at: repo.updated_at.to_rfc3339(),
                    };
                    return (StatusCode::OK, Json(response)).into_response();
                }
                Ok(None) => {
                    serde_json::json!({
                        "error": "not_found",
                        "message": "Repository not found"
                    })
                }
                Err(e) => {
                    tracing::error!("failed to get repo: {}", e);
                    serde_json::json!({
                        "error": "database_error",
                        "message": format!("failed to get repository: {}", e)
                    })
                }
            }
        }
        Err(_) => {
            serde_json::json!({
                "error": "invalid_id",
                "message": "Invalid repository ID format"
            })
        }
    };
    (StatusCode::BAD_REQUEST, Json(response)).into_response()
}

/// Delete a repository
async fn delete_repo(
    Extension(pool): Extension<Arc<Pool>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    tracing::debug!("delete repo request: {}", id);

    let response = match Uuid::parse_str(&id) {
        Ok(uuid) => {
            let repo_id = RepoId::from(uuid);
            match RepoQueries::delete(&pool, repo_id).await {
                Ok(_) => return StatusCode::NO_CONTENT.into_response(),
                Err(e) => {
                    tracing::error!("failed to delete repo: {}", e);
                    serde_json::json!({
                        "error": "database_error",
                        "message": format!("failed to delete repository: {}", e)
                    })
                }
            }
        }
        Err(_) => {
            serde_json::json!({
                "error": "invalid_id",
                "message": "Invalid repository ID format"
            })
        }
    };
    (StatusCode::BAD_REQUEST, Json(response)).into_response()
}

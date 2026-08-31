//! Repository API routes

use crate::auth::Claims;
use axum::{
    extract::{Extension, Path},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use gitforge_common::{RepoId, UserId};
use gitforge_core::{FileStorageBackend, StorageBackend};
use gitforge_db::{queries::RepoQueries, Pool};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

/// Validate repository name to prevent path traversal and injection attacks
fn validate_repo_name(name: &str) -> Result<(), String> {
    // Check for empty name
    if name.is_empty() {
        return Err("Repository name cannot be empty".to_string());
    }

    // Check for path traversal attempts
    if name.contains("..") {
        return Err("Repository name cannot contain '..'".to_string());
    }

    // Check for path separators that could create directories
    if name.contains('/') && !name.contains("./") {
        // Allow org/repo format but validate each part
        for part in name.split('/') {
            validate_repo_name(part)?;
        }
        return Ok(());
    }

    // Check length
    if name.len() > 255 {
        return Err("Repository name too long (max 255 characters)".to_string());
    }

    // Check for special characters that could be problematic
    let invalid_chars = [
        '\0', '\n', '\r', '\t', '\\', '"', '\'', '`', '>', '<', '|', '&', ';', '$', '!', '{', '}',
        '[', ']', '(', ')',
    ];
    for c in invalid_chars {
        if name.contains(c) {
            return Err(format!(
                "Repository name contains invalid character: {:?}",
                c
            ));
        }
    }

    // Must start and end with alphanumeric
    let chars: Vec<char> = name.chars().collect();
    if !chars.first().map(|c| c.is_alphanumeric()).unwrap_or(false) {
        return Err("Repository name must start with an alphanumeric character".to_string());
    }
    if !chars.last().map(|c| c.is_alphanumeric()).unwrap_or(false) {
        return Err("Repository name must end with an alphanumeric character".to_string());
    }

    Ok(())
}

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

fn can_access_repo(claims: &Claims, owner_id: UserId) -> bool {
    claims.user_id == owner_id || matches!(claims.role.as_str(), "admin" | "maintainer")
}

/// List repositories
async fn list_repos(
    Extension(pool): Extension<Arc<Pool>>,
    Extension(claims): Extension<Claims>,
) -> impl IntoResponse {
    match RepoQueries::list(&pool).await {
        Ok(repos) => {
            let response: Vec<RepoResponse> = repos
                .into_iter()
                .filter(|repo| can_access_repo(&claims, repo.owner_id))
                .map(|r| RepoResponse {
                    id: r.id.to_string(),
                    name: r.name,
                    owner_id: r.owner_id.to_string(),
                    visibility: r.visibility,
                    git_path: r.git_path,
                    created_at: r.created_at.to_rfc3339(),
                    updated_at: r.updated_at.to_rfc3339(),
                })
                .collect();
            Json(response).into_response()
        }
        Err(e) => {
            tracing::error!("failed to list repos: {}", e);
            Json(serde_json::Value::Array(vec![])).into_response()
        }
    }
}

/// Create a repository
async fn create_repo(
    Extension(pool): Extension<Arc<Pool>>,
    Extension(claims): Extension<Claims>,
    Json(req): Json<CreateRepoRequest>,
) -> impl IntoResponse {
    let owner_id = claims.user_id;

    tracing::debug!("create repo request: {:?} by user {}", req, owner_id);

    // Validate repository name to prevent path traversal and injection
    if let Err(e) = validate_repo_name(&req.name) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "invalid_name",
                "message": e
            })),
        )
            .into_response();
    }

    let repo_id = RepoId::new();
    let storage_root = std::env::var("GIT_ROOT")
        .ok()
        .filter(|root| !root.trim().is_empty())
        .unwrap_or_else(|| "target/gitforge-repos".to_string());
    let storage = FileStorageBackend::new(&storage_root);

    if let Err(error) = storage.ensure_root().await {
        tracing::error!(%error, "failed to initialize git storage root");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "error": "storage_error",
                "message": "failed to initialize repository storage"
            })),
        )
            .into_response();
    }

    let git_path = storage.repo_path(repo_id);
    if let Err(error) = storage.create(repo_id).await {
        tracing::error!(%error, repo_id = %repo_id, "failed to provision bare repository");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "error": "storage_error",
                "message": "failed to provision repository storage"
            })),
        )
            .into_response();
    }

    let repo = gitforge_db::models::Repository::new_with_id(
        repo_id,
        req.name,
        owner_id,
        git_path.to_string_lossy().into_owned(),
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
            (StatusCode::CREATED, Json(response)).into_response()
        }
        Err(e) => {
            if let Err(cleanup_error) = storage.delete(repo_id).await {
                tracing::error!(
                    %cleanup_error,
                    repo_id = %repo_id,
                    "failed to clean up repository storage after database error"
                );
            }
            tracing::error!("failed to create repo: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": "database_error",
                    "message": format!("failed to create repository: {}", e)
                })),
            )
                .into_response()
        }
    }
}

/// Get a repository
async fn get_repo(
    Extension(pool): Extension<Arc<Pool>>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    tracing::debug!("get repo request: {}", id);

    match Uuid::parse_str(&id) {
        Ok(uuid) => match RepoQueries::get(&pool, RepoId::from(uuid)).await {
            Ok(Some(repo)) if can_access_repo(&claims, repo.owner_id) => {
                let response = RepoResponse {
                    id: repo.id.to_string(),
                    name: repo.name,
                    owner_id: repo.owner_id.to_string(),
                    visibility: repo.visibility,
                    git_path: repo.git_path,
                    created_at: repo.created_at.to_rfc3339(),
                    updated_at: repo.updated_at.to_rfc3339(),
                };
                (StatusCode::OK, Json(response)).into_response()
            }
            Ok(Some(_)) | Ok(None) => (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({
                    "error": "not_found",
                    "message": "Repository not found"
                })),
            )
                .into_response(),
            Err(e) => {
                tracing::error!("failed to get repo: {}", e);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({
                        "error": "database_error",
                        "message": format!("failed to get repository: {}", e)
                    })),
                )
                    .into_response()
            }
        },
        Err(_) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "invalid_id",
                "message": "Invalid repository ID format"
            })),
        )
            .into_response(),
    }
}

/// Delete a repository
async fn delete_repo(
    Extension(pool): Extension<Arc<Pool>>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    tracing::debug!("delete repo request: {}", id);

    match Uuid::parse_str(&id) {
        Ok(uuid) => {
            let repo_id = RepoId::from(uuid);
            match RepoQueries::get(&pool, repo_id).await {
                Ok(Some(repo)) if can_access_repo(&claims, repo.owner_id) => {
                    match RepoQueries::delete(&pool, repo_id).await {
                        Ok(_) => StatusCode::NO_CONTENT.into_response(),
                        Err(e) => {
                            tracing::error!("failed to delete repo: {}", e);
                            (
                                StatusCode::INTERNAL_SERVER_ERROR,
                                Json(serde_json::json!({
                                    "error": "database_error",
                                    "message": format!("failed to delete repository: {}", e)
                                })),
                            )
                                .into_response()
                        }
                    }
                }
                Ok(Some(_)) | Ok(None) => (
                    StatusCode::NOT_FOUND,
                    Json(serde_json::json!({
                        "error": "not_found",
                        "message": "Repository not found"
                    })),
                )
                    .into_response(),
                Err(e) => {
                    tracing::error!("failed to get repo for delete: {}", e);
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(serde_json::json!({
                            "error": "database_error",
                            "message": format!("failed to get repository: {}", e)
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
                "message": "Invalid repository ID format"
            })),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_repo_response_serialization() {
        let response = RepoResponse {
            id: "repo-123".to_string(),
            name: "test-repo".to_string(),
            owner_id: "user-456".to_string(),
            visibility: "private".to_string(),
            git_path: "/git/repos/test-repo".to_string(),
            created_at: "2024-01-01T00:00:00Z".to_string(),
            updated_at: "2024-01-01T00:00:00Z".to_string(),
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("repo-123"));
        assert!(json.contains("test-repo"));
        assert!(json.contains("private"));
    }

    #[test]
    fn test_repo_response_deserialization() {
        let json = r#"{
            "id": "repo-789",
            "name": "my-project",
            "owner_id": "user-001",
            "visibility": "public",
            "git_path": "/git/repos/my-project",
            "created_at": "2024-01-15T10:30:00Z",
            "updated_at": "2024-01-15T10:30:00Z"
        }"#;
        let response: RepoResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.id, "repo-789");
        assert_eq!(response.name, "my-project");
        assert_eq!(response.visibility, "public");
    }

    #[test]
    fn test_create_repo_request_deserialization() {
        let json = r#"{"name": "new-repo", "visibility": "private"}"#;
        let request: CreateRepoRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.name, "new-repo");
        assert_eq!(request.visibility, Some("private".to_string()));
    }

    #[test]
    fn test_create_repo_request_without_visibility() {
        let json = r#"{"name": "another-repo"}"#;
        let request: CreateRepoRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.name, "another-repo");
        assert!(request.visibility.is_none());
    }

    #[test]
    fn test_repo_response_public_visibility() {
        let response = RepoResponse {
            id: "repo-001".to_string(),
            name: "public-repo".to_string(),
            owner_id: "user-100".to_string(),
            visibility: "public".to_string(),
            git_path: "/git/repos/public-repo".to_string(),
            created_at: "2024-01-01T00:00:00Z".to_string(),
            updated_at: "2024-01-01T00:00:00Z".to_string(),
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("public"));
    }

    #[test]
    fn test_repo_response_all_fields() {
        let response = RepoResponse {
            id: "repo-full".to_string(),
            name: "complete-repo".to_string(),
            owner_id: "owner-123".to_string(),
            visibility: "private".to_string(),
            git_path: "/git/repos/complete-repo".to_string(),
            created_at: "2024-06-01T12:00:00Z".to_string(),
            updated_at: "2024-06-15T18:30:00Z".to_string(),
        };
        assert_eq!(response.id, "repo-full");
        assert_eq!(response.name, "complete-repo");
        assert_eq!(response.git_path, "/git/repos/complete-repo");
    }

    #[test]
    fn test_validate_repo_name_empty() {
        let result = validate_repo_name("");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("empty"));
    }

    #[test]
    fn test_validate_repo_name_path_traversal() {
        let result = validate_repo_name("../etc/passwd");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains(".."));
    }

    #[test]
    fn test_validate_repo_name_too_long() {
        let long_name = "a".repeat(256);
        let result = validate_repo_name(&long_name);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("255"));
    }

    #[test]
    fn test_validate_repo_name_valid() {
        let result = validate_repo_name("valid-repo-name");
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_repo_name_with_org_format() {
        let result = validate_repo_name("org/repo");
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_repo_name_invalid_char() {
        let result = validate_repo_name("repo|name");
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_repo_name_starts_with_non_alphanumeric() {
        let result = validate_repo_name("-starts-with-dash");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("start"));
    }

    #[test]
    fn test_validate_repo_name_ends_with_non_alphanumeric() {
        let result = validate_repo_name("ends-with-dash-");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("end"));
    }

    #[test]
    fn test_validate_repo_name_with_underscore() {
        // underscores should be valid in the middle
        let result = validate_repo_name("repo_with_underscore");
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_repo_name_with_period() {
        // periods should be valid in the middle
        let result = validate_repo_name("repo.name");
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_repo_name_with_hyphen() {
        // hyphens should be valid
        let result = validate_repo_name("repo-name");
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_repo_name_org_format_valid() {
        let result = validate_repo_name("my-org/my-repo");
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_repo_name_org_format_invalid_second_part() {
        // org/repo where second part starts with dash
        let result = validate_repo_name("org/-invalid");
        assert!(result.is_err());
    }
}

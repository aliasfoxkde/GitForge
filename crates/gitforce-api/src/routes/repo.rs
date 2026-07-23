//! Repository API routes

use crate::auth::ApiAuth;
use axum::{
    extract::{Extension, Path},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use gitforce_common::{RepoId, UserId};
use gitforce_db::{queries::RepoQueries, Pool};
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

/// Helper to extract and validate user from headers
fn extract_user(auth: &ApiAuth, headers: &HeaderMap) -> Result<UserId, StatusCode> {
    let auth_header = headers.get("Authorization").and_then(|v| v.to_str().ok());

    let token = auth_header
        .and_then(|h| ApiAuth::extract_token(h))
        .ok_or(StatusCode::UNAUTHORIZED)?;

    let claims = auth
        .validate_token(token)
        .map_err(|_| StatusCode::UNAUTHORIZED)?;

    Ok(claims.user_id)
}

/// List repositories
async fn list_repos(
    Extension(pool): Extension<Arc<Pool>>,
    Extension(auth): Extension<Arc<ApiAuth>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    // Check auth
    match extract_user(&auth, &headers) {
        Err(e) => e.into_response(),
        Ok(_) => match RepoQueries::list(&pool).await {
            Ok(repos) => {
                let response: Vec<RepoResponse> = repos
                    .into_iter()
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
        },
    }
}

/// Create a repository
async fn create_repo(
    Extension(pool): Extension<Arc<Pool>>,
    Extension(auth): Extension<Arc<ApiAuth>>,
    headers: HeaderMap,
    Json(req): Json<CreateRepoRequest>,
) -> impl IntoResponse {
    // Check auth and get user
    let owner_id = match extract_user(&auth, &headers) {
        Err(e) => return e.into_response(),
        Ok(id) => id,
    };

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

    let git_path = format!("/git/repos/{}", req.name);

    let repo = gitforce_db::models::Repository::new(req.name, owner_id, git_path.clone());

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
    Extension(auth): Extension<Arc<ApiAuth>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    // Check auth
    match extract_user(&auth, &headers) {
        Err(e) => e.into_response(),
        Ok(_) => {
            tracing::debug!("get repo request: {}", id);

            match Uuid::parse_str(&id) {
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
                            (StatusCode::OK, Json(response)).into_response()
                        }
                        Ok(None) => (
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
    }
}

/// Delete a repository
async fn delete_repo(
    Extension(pool): Extension<Arc<Pool>>,
    Extension(auth): Extension<Arc<ApiAuth>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    // Check auth
    match extract_user(&auth, &headers) {
        Err(e) => e.into_response(),
        Ok(_) => {
            tracing::debug!("delete repo request: {}", id);

            match Uuid::parse_str(&id) {
                Ok(uuid) => {
                    let repo_id = RepoId::from(uuid);
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
    fn test_extract_user_without_auth_header() {
        use crate::auth::ApiAuth;

        let auth = ApiAuth::new("test-secret");
        let headers = HeaderMap::new();
        let result = extract_user(&auth, &headers);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn test_extract_user_with_invalid_token() {
        use crate::auth::ApiAuth;

        let auth = ApiAuth::new("test-secret");
        let mut headers = HeaderMap::new();
        headers.insert("Authorization", "Bearer invalid-token".parse().unwrap());
        let result = extract_user(&auth, &headers);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn test_extract_user_with_valid_token() {
        use crate::auth::ApiAuth;
        use gitforce_common::UserId;

        let auth = ApiAuth::new("test-secret");
        let user_id = UserId::new();
        let token = auth.generate_token(user_id, "testuser", "user").unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(
            "Authorization",
            format!("Bearer {}", token).parse().unwrap(),
        );
        let result = extract_user(&auth, &headers);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), user_id);
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

    #[test]
    fn test_extract_user_malformed_auth_header() {
        use crate::auth::ApiAuth;

        let auth = ApiAuth::new("test-secret");
        let mut headers = HeaderMap::new();
        headers.insert("Authorization", "NotBearer token123".parse().unwrap());
        let result = extract_user(&auth, &headers);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn test_extract_user_empty_bearer_token() {
        use crate::auth::ApiAuth;

        let auth = ApiAuth::new("test-secret");
        let mut headers = HeaderMap::new();
        headers.insert("Authorization", "Bearer".parse().unwrap());
        let result = extract_user(&auth, &headers);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn test_extract_user_basic_auth_header() {
        use crate::auth::ApiAuth;

        let auth = ApiAuth::new("test-secret");
        let mut headers = HeaderMap::new();
        headers.insert("Authorization", "Basic dXNlcjpwYXNz".parse().unwrap());
        let result = extract_user(&auth, &headers);
        assert!(result.is_err());
    }

    #[test]
    fn test_extract_user_bearer_with_leading_space() {
        use crate::auth::ApiAuth;

        let auth = ApiAuth::new("test-secret");
        let mut headers = HeaderMap::new();
        headers.insert("Authorization", " Bearer token123".parse().unwrap());
        let result = extract_user(&auth, &headers);
        assert!(result.is_err());
    }
}

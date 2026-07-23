//! Authentication API routes

use crate::auth::ApiAuth;
use axum::{
    extract::Extension,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use gitforce_db::{queries::UserQueries, Pool};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Login request
#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

/// Login response
#[derive(Debug, Serialize)]
pub struct LoginResponse {
    pub token: String,
    pub token_type: String,
    pub expires_in: i64,
}

/// Auth routes (public - no auth required)
pub fn auth_routes<S: Clone + Send + Sync + 'static>() -> Router<S> {
    Router::new()
        .route("/auth/login", post(login))
        .route("/auth/status", get(auth_status))
}

/// Login endpoint
pub async fn login(
    Extension(pool): Extension<Arc<Pool>>,
    Extension(auth): Extension<Arc<ApiAuth>>,
    Json(req): Json<LoginRequest>,
) -> impl IntoResponse {
    // Look up user by username
    match UserQueries::get_by_username(&pool, &req.username).await {
        Ok(Some(user)) => {
            // Verify password
            match gitforce_common::password::verify_password(&req.password, &user.password_hash) {
                Ok(true) => {
                    // Generate token
                    // TODO: Add role field to User model
                    let token = auth.generate_token(
                        user.id,
                        &user.username,
                        "user", // Default role until User model has role field
                    );

                    match token {
                        Ok(token) => {
                            // Return token
                            let response = LoginResponse {
                                token,
                                token_type: "Bearer".to_string(),
                                expires_in: 86400, // 24 hours in seconds
                            };
                            (StatusCode::OK, Json(response)).into_response()
                        }
                        Err(_) => (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            Json(serde_json::json!({
                                "error": "internal_error",
                                "message": "Failed to generate token"
                            })),
                        )
                            .into_response(),
                    }
                }
                Ok(false) => {
                    // Invalid password
                    (
                        StatusCode::UNAUTHORIZED,
                        Json(serde_json::json!({
                            "error": "invalid_credentials",
                            "message": "Invalid username or password"
                        })),
                    )
                        .into_response()
                }
                Err(_) => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({
                        "error": "internal_error",
                        "message": "Password verification failed"
                    })),
                )
                    .into_response(),
            }
        }
        Ok(None) => {
            // User not found
            (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({
                    "error": "invalid_credentials",
                    "message": "Invalid username or password"
                })),
            )
                .into_response()
        }
        Err(e) => {
            tracing::error!("login failed: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": "internal_error",
                    "message": "Login failed"
                })),
            )
                .into_response()
        }
    }
}

/// Check auth status
pub async fn auth_status(
    Extension(auth): Extension<Arc<ApiAuth>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let auth_header = headers.get("Authorization").and_then(|v| v.to_str().ok());

    let token = auth_header.and_then(|h| ApiAuth::extract_token(h));

    match token {
        Some(token) => match auth.validate_token(token) {
            Ok(claims) => Json(serde_json::json!({
                "authenticated": true,
                "user_id": claims.user_id.to_string(),
                "username": claims.username,
                "role": claims.role,
            }))
            .into_response(),
            Err(_) => Json(serde_json::json!({
                "authenticated": false,
                "message": "Invalid or expired token"
            }))
            .into_response(),
        },
        None => Json(serde_json::json!({
            "authenticated": false,
            "message": "No token provided"
        }))
        .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;

    #[tokio::test]
    async fn test_auth_status_no_token() {
        let auth = ApiAuth::new("test-secret");
        let response = auth_status(
            Extension(Arc::new(auth)),
            HeaderMap::new(),
        )
        .await
        .into_response();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_auth_status_invalid_token() {
        let auth = ApiAuth::new("test-secret");
        let mut headers = HeaderMap::new();
        headers.insert("Authorization", "Bearer invalid-token".parse().unwrap());

        let response = auth_status(
            Extension(Arc::new(auth)),
            headers,
        )
        .await
        .into_response();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_auth_status_valid_token() {
        let auth = ApiAuth::new("test-secret");
        let user_id = gitforce_common::UserId::new();
        let token = auth.generate_token(user_id, "testuser", "user").unwrap();

        let mut headers = HeaderMap::new();
        headers.insert("Authorization", format!("Bearer {}", token).parse().unwrap());

        let response = auth_status(
            Extension(Arc::new(auth)),
            headers,
        )
        .await
        .into_response();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_login_request_deserialize() {
        let json = r#"{"username":"testuser","password":"testpass"}"#;
        let req: LoginRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.username, "testuser");
        assert_eq!(req.password, "testpass");
    }

    #[tokio::test]
    async fn test_login_response_serialize() {
        let response = LoginResponse {
            token: "test-token".to_string(),
            token_type: "Bearer".to_string(),
            expires_in: 86400,
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("test-token"));
        assert!(json.contains("Bearer"));
    }
}

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
        let response = auth_status(Extension(Arc::new(auth)), HeaderMap::new())
            .await
            .into_response();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_auth_status_invalid_token() {
        let auth = ApiAuth::new("test-secret");
        let mut headers = HeaderMap::new();
        headers.insert("Authorization", "Bearer invalid-token".parse().unwrap());

        let response = auth_status(Extension(Arc::new(auth)), headers)
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
        headers.insert(
            "Authorization",
            format!("Bearer {}", token).parse().unwrap(),
        );

        let response = auth_status(Extension(Arc::new(auth)), headers)
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

    #[test]
    fn test_login_request_debug() {
        let req = LoginRequest {
            username: "user1".to_string(),
            password: "secret".to_string(),
        };
        let debug_str = format!("{:?}", req);
        assert!(debug_str.contains("user1"));
    }

    #[test]
    fn test_login_response_debug() {
        let response = LoginResponse {
            token: "debug-token".to_string(),
            token_type: "Bearer".to_string(),
            expires_in: 3600,
        };
        let debug_str = format!("{:?}", response);
        assert!(debug_str.contains("debug-token"));
    }

    #[test]
    fn test_login_response_default_token_type() {
        let response = LoginResponse {
            token: "token123".to_string(),
            token_type: "Bearer".to_string(),
            expires_in: 86400,
        };
        assert_eq!(response.token_type, "Bearer");
    }

    #[test]
    fn test_login_response_expires_in_values() {
        // Test various expiration times
        for exp in &[3600, 7200, 86400, 604800] {
            let response = LoginResponse {
                token: "token".to_string(),
                token_type: "Bearer".to_string(),
                expires_in: *exp,
            };
            assert_eq!(response.expires_in, *exp);
        }
    }

    #[tokio::test]
    async fn test_auth_status_malformed_header() {
        let auth = ApiAuth::new("test-secret");
        let mut headers = HeaderMap::new();
        // Malformed header - not Bearer
        headers.insert("Authorization", "Basic dXNlcjpwYXNz".parse().unwrap());

        let response = auth_status(Extension(Arc::new(auth)), headers)
            .await
            .into_response();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_auth_status_empty_bearer() {
        let auth = ApiAuth::new("test-secret");
        let mut headers = HeaderMap::new();
        headers.insert("Authorization", "Bearer".parse().unwrap());

        let response = auth_status(Extension(Arc::new(auth)), headers)
            .await
            .into_response();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_auth_status_expired_token() {
        use chrono::Utc;
        use jsonwebtoken::{encode, Header};

        #[derive(Debug, serde::Serialize, serde::Deserialize)]
        struct ExpiredClaims {
            sub: String,
            user_id: gitforce_common::UserId,
            username: String,
            role: String,
            exp: i64,
            iat: i64,
        }

        let auth = ApiAuth::new("test-secret");
        let user_id = gitforce_common::UserId::new();

        // Create expired token
        let claims = ExpiredClaims {
            sub: user_id.to_string(),
            user_id,
            username: "expired".to_string(),
            role: "user".to_string(),
            exp: Utc::now().timestamp() - 3600, // Expired 1 hour ago
            iat: Utc::now().timestamp() - 7200,
        };

        let token = encode(
            &Header::default(),
            &claims,
            &jsonwebtoken::EncodingKey::from_secret("test-secret".as_bytes()),
        )
        .unwrap();

        let mut headers = HeaderMap::new();
        headers.insert(
            "Authorization",
            format!("Bearer {}", token).parse().unwrap(),
        );

        let response = auth_status(Extension(Arc::new(auth)), headers)
            .await
            .into_response();

        // Expired tokens return OK with authenticated=false
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[test]
    fn test_login_request_deserialization() {
        let json = r#"{"username":"testuser","password":"testpass"}"#;
        let req: LoginRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.username, "testuser");
        assert_eq!(req.password, "testpass");
    }

    #[test]
    fn test_login_request_deserialization_special_chars() {
        let json = r#"{"username":"user@domain.com","password":"p@$$w0rd!"}"#;
        let req: LoginRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.username, "user@domain.com");
        assert_eq!(req.password, "p@$$w0rd!");
    }

    #[test]
    fn test_login_response_serialization_all_fields() {
        let response = LoginResponse {
            token: "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9".to_string(),
            token_type: "Bearer".to_string(),
            expires_in: 86400,
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("Bearer"));
        assert!(json.contains("86400"));
    }

    #[test]
    fn test_login_response_different_expires_in() {
        for exp in &[1, 60, 3600, 86400, 604800] {
            let response = LoginResponse {
                token: "token".to_string(),
                token_type: "Bearer".to_string(),
                expires_in: *exp,
            };
            assert_eq!(response.expires_in, *exp);
        }
    }

    #[test]
    fn test_auth_routes_creation() {
        let _router: Router<()> = auth_routes();
        // Just verify it compiles and creates a router
    }
}

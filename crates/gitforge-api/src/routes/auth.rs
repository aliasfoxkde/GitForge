//! Authentication API routes

use crate::auth::{ApiAuth, ACCESS_TTL_SECS, REFRESH_TTL_SECS};
use axum::{
    extract::Extension,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use chrono::Utc;
use gitforge_db::{queries::RefreshTokenQueries, queries::UserQueries, Pool};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Login request
#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

/// Login response
///
/// The refresh fields are optional so the response stays backward
/// compatible: an older client ignores them, and a login whose refresh
/// row could not be persisted degrades to the pre-refresh behavior
/// (24-hour JWT only) instead of failing the whole login.
#[derive(Debug, Serialize)]
pub struct LoginResponse {
    pub token: String,
    pub token_type: String,
    pub expires_in: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_expires_in: Option<i64>,
}

/// Refresh request: the long-lived credential handed out at login.
#[derive(Debug, Deserialize)]
pub struct RefreshRequest {
    pub refresh_token: String,
}

/// Compute the storage/lookup digest for a refresh credential: an
/// unsalted SHA-256 hex string. Bcrypt cannot be used here — its random
/// salt makes every digest of the same value differ, so an equality
/// lookup could never match. Unsalted is the right call for a lookup key
/// over a credential with 244 bits of entropy (unguessable, so there is
/// nothing to brute-force); the slow, salted path stays reserved for
/// human passwords.
fn refresh_credential_digest(value: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    hex::encode(hasher.finalize())
}

/// Mint a refresh credential, persist only its digest, and return the
/// plaintext for the response. Returns `None` when the row could not be
/// stored — the caller degrades to an access-only login rather than
/// failing.
async fn issue_refresh_credential(pool: &Pool, user_id: gitforge_common::UserId) -> Option<String> {
    // Two UUIDv4s give 244 bits of entropy; the `gfrt` prefix keeps the
    // credential identifiable in config files and logs-without-secrets.
    let value = format!(
        "gfrt{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let value_digest = refresh_credential_digest(&value);
    let expires_at = Utc::now() + chrono::Duration::seconds(REFRESH_TTL_SECS);
    match RefreshTokenQueries::create(pool, user_id, &value_digest, expires_at).await {
        Ok(()) => Some(value),
        Err(error) => {
            // Log only a static message; the Error Display carries raw
            // database state that must not enter structured log fields.
            tracing::warn!("failed to persist refresh credential; issuing JWT-only session");
            let _ = error;
            None
        }
    }
}

/// Auth routes (public - no auth required)
pub fn auth_routes<S: Clone + Send + Sync + 'static>() -> Router<S> {
    Router::new()
        .route("/auth/login", post(login))
        .route("/auth/refresh", post(refresh))
        .route("/auth/logout", post(logout))
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
            match gitforge_common::password::verify_password(&req.password, &user.password_hash) {
                Ok(true) => {
                    // Generate a token with the persisted least-privilege
                    // role. Old databases are migrated with `developer`.
                    let role = UserQueries::get_role(&pool, user.id)
                        .await
                        .ok()
                        .flatten()
                        .unwrap_or_else(|| "developer".to_string());
                    let token = auth.generate_token(user.id, &user.username, &role);

                    match token {
                        Ok(token) => {
                            // Long-lived refresh credential for silent
                            // renewal; omitted when its row could not be
                            // stored (JWT-only session).
                            let refresh = issue_refresh_credential(&pool, user.id).await;
                            let refresh_expires_in = refresh.as_ref().map(|_| REFRESH_TTL_SECS);
                            let response = LoginResponse {
                                token,
                                token_type: "Bearer".to_string(),
                                expires_in: ACCESS_TTL_SECS,
                                refresh_token: refresh,
                                refresh_expires_in,
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
        Err(_e) => {
            // Log only a static message. The Error Display impl renders as
            // "{kind}: {message}" where message includes raw database state
            // (paths, constraint names, SQL text); that payload must not enter
            // structured log fields. The error kind (Database) is intentionally
            // not forwarded — client-facing response already returns only
            // "internal_error" / "Login failed".
            tracing::error!("login failed: database error");
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

/// Renew the short-lived JWT from a refresh credential. The presented
/// credential is rotated on every use: the old row is revoked and a new
/// pair is issued, so a replayed refresh token fails closed and a stolen
/// one is detected by the legitimate client's next refresh.
pub async fn refresh(
    Extension(pool): Extension<Arc<Pool>>,
    Extension(auth): Extension<Arc<ApiAuth>>,
    Json(req): Json<RefreshRequest>,
) -> impl IntoResponse {
    let value_digest = refresh_credential_digest(&req.refresh_token);

    let stored = match RefreshTokenQueries::find_active_by_hash(&pool, &value_digest).await {
        Ok(Some(stored)) => stored,
        Ok(None) => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({
                    "error": "invalid_credentials",
                    "message": "Invalid or expired refresh credential"
                })),
            )
                .into_response()
        }
        Err(_) => {
            tracing::error!("refresh failed: database error");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": "internal_error",
                    "message": "Refresh failed"
                })),
            )
                .into_response();
        }
    };

    // Rotate first: the presented credential is dead regardless of what
    // happens below, so a partially failed refresh cannot leave two live
    // credentials for the same session.
    if let Err(error) = RefreshTokenQueries::revoke(&pool, &value_digest).await {
        tracing::error!("refresh rotation failed: database error");
        let _ = error;
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "error": "internal_error",
                "message": "Refresh failed"
            })),
        )
            .into_response();
    }

    // The user must still exist (deleted accounts cannot renew) and the
    // role is resolved fresh, mirroring the login and middleware paths so
    // a demoted role takes effect at the next renewal.
    let user = match UserQueries::get(&pool, stored.user_id).await {
        Ok(Some(user)) => user,
        Ok(None) => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({
                    "error": "invalid_credentials",
                    "message": "Invalid or expired refresh credential"
                })),
            )
                .into_response()
        }
        Err(_) => {
            tracing::error!("refresh failed: database error");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": "internal_error",
                    "message": "Refresh failed"
                })),
            )
                .into_response();
        }
    };
    let role = UserQueries::get_role(&pool, user.id)
        .await
        .ok()
        .flatten()
        .unwrap_or_else(|| "developer".to_string());

    let token = match auth.generate_token(user.id, &user.username, &role) {
        Ok(token) => token,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": "internal_error",
                    "message": "Failed to generate token"
                })),
            )
                .into_response()
        }
    };

    let new_refresh = issue_refresh_credential(&pool, user.id).await;
    let refresh_expires_in = new_refresh.as_ref().map(|_| REFRESH_TTL_SECS);
    let response = LoginResponse {
        token,
        token_type: "Bearer".to_string(),
        expires_in: ACCESS_TTL_SECS,
        refresh_token: new_refresh,
        refresh_expires_in,
    };
    (StatusCode::OK, Json(response)).into_response()
}

/// Revoke a refresh credential. Idempotent by design: revoking an
/// unknown or already-revoked credential still answers 200 so logout
/// never leaks whether a credential existed.
pub async fn logout(
    Extension(pool): Extension<Arc<Pool>>,
    Json(req): Json<RefreshRequest>,
) -> impl IntoResponse {
    let value_digest = refresh_credential_digest(&req.refresh_token);
    if let Err(error) = RefreshTokenQueries::revoke(&pool, &value_digest).await {
        tracing::error!("logout revocation failed: database error");
        let _ = error;
    }
    (
        StatusCode::OK,
        Json(serde_json::json!({ "logged_out": true })),
    )
        .into_response()
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
        let user_id = gitforge_common::UserId::new();
        let token = auth.generate_token(user_id, "testuser", "user").unwrap();

        let mut headers = HeaderMap::new();
        headers.insert("Authorization", format!("Bearer {token}").parse().unwrap());

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
            refresh_token: None,
            refresh_expires_in: None,
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
        let debug_str = format!("{req:?}");
        assert!(debug_str.contains("user1"));
    }

    #[test]
    fn test_login_response_debug() {
        let response = LoginResponse {
            token: "debug-token".to_string(),
            token_type: "Bearer".to_string(),
            expires_in: 3600,
            refresh_token: None,
            refresh_expires_in: None,
        };
        let debug_str = format!("{response:?}");
        assert!(debug_str.contains("debug-token"));
    }

    #[test]
    fn test_login_response_default_token_type() {
        let response = LoginResponse {
            token: "token123".to_string(),
            token_type: "Bearer".to_string(),
            expires_in: 86400,
            refresh_token: None,
            refresh_expires_in: None,
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
                refresh_token: None,
                refresh_expires_in: None,
            };
            assert_eq!(response.expires_in, *exp);
        }
    }

    /// Build a migrated in-memory pool holding one user, then issue that
    /// user a live refresh credential. Returns (pool, auth, credential).
    async fn refresh_fixture() -> (Arc<Pool>, Arc<ApiAuth>, String) {
        let pool = Arc::new(Pool::memory().await.unwrap());
        pool.migrate().await.unwrap();
        let auth = Arc::new(ApiAuth::new("test-secret"));

        let user = gitforge_db::models::User::new(
            "refresh-user".to_string(),
            "refresh-user@example.com".to_string(),
            "hash".to_string(),
        );
        UserQueries::create(&pool, &user).await.unwrap();

        let credential = issue_refresh_credential(&pool, user.id)
            .await
            .expect("refresh credential must be storable for a live user");
        (pool, auth, credential)
    }

    async fn body_json(response: axum::response::Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn test_refresh_rotates_and_rejects_replay() {
        let (pool, auth, credential) = refresh_fixture().await;

        // First refresh answers 200 with a fresh pair.
        let response = refresh(
            Extension(pool.clone()),
            Extension(auth.clone()),
            Json(RefreshRequest {
                refresh_token: credential.clone(),
            }),
        )
        .await
        .into_response();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        let rotated = body["refresh_token"].as_str().unwrap().to_string();
        assert_ne!(rotated, credential, "rotation must mint a new credential");
        assert!(body["refresh_expires_in"].is_i64());
        assert!(!body["token"].as_str().unwrap().is_empty());

        // Replaying the just-rotated credential fails closed.
        let replay = refresh(
            Extension(pool.clone()),
            Extension(auth.clone()),
            Json(RefreshRequest {
                refresh_token: credential,
            }),
        )
        .await
        .into_response();
        assert_eq!(replay.status(), StatusCode::UNAUTHORIZED);

        // The rotated credential is live and rotates again.
        let second = refresh(
            Extension(pool.clone()),
            Extension(auth),
            Json(RefreshRequest {
                refresh_token: rotated,
            }),
        )
        .await
        .into_response();
        assert_eq!(second.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_refresh_unknown_credential_is_unauthorized() {
        let (pool, auth, _live) = refresh_fixture().await;

        // A well-formed but never-issued credential: real issuance uses
        // two concatenated UUIDs, so this hash cannot collide with a
        // stored row.
        let unknown = format!(
            "gfrt{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        let response = refresh(
            Extension(pool),
            Extension(auth),
            Json(RefreshRequest {
                refresh_token: unknown,
            }),
        )
        .await
        .into_response();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_logout_revokes_and_is_idempotent() {
        let (pool, auth, credential) = refresh_fixture().await;

        let first = logout(
            Extension(pool.clone()),
            Json(RefreshRequest {
                refresh_token: credential.clone(),
            }),
        )
        .await
        .into_response();
        assert_eq!(first.status(), StatusCode::OK);

        // The revoked credential can no longer mint a JWT.
        let denied = refresh(
            Extension(pool.clone()),
            Extension(auth),
            Json(RefreshRequest {
                refresh_token: credential,
            }),
        )
        .await
        .into_response();
        assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);

        // Logout stays 200 for an already-revoked credential.
        let repeat = logout(
            Extension(pool),
            Json(RefreshRequest {
                refresh_token: format!(
                    "gfrt{}{}",
                    uuid::Uuid::new_v4().simple(),
                    uuid::Uuid::new_v4().simple()
                ),
            }),
        )
        .await
        .into_response();
        assert_eq!(repeat.status(), StatusCode::OK);
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
            user_id: gitforge_common::UserId,
            username: String,
            role: String,
            exp: i64,
            iat: i64,
        }

        let auth = ApiAuth::new("test-secret");
        let user_id = gitforge_common::UserId::new();

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
        headers.insert("Authorization", format!("Bearer {token}").parse().unwrap());

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
            refresh_token: None,
            refresh_expires_in: None,
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
                refresh_token: None,
                refresh_expires_in: None,
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

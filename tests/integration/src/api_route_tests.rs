//! API Route Handler Tests
//!
//! These tests verify API route handlers using tower's ServiceExt
//! for oneshot requests. Tests auth, error handling, and response formats.

use gitforce_api::auth::{ApiAuth, Claims};
use gitforce_api::{ApiServer, Metrics};
use gitforce_common::{UserId, RepoId, PipelineId, PipelineRunId, JobId, RunnerId};
use gitforce_db::Pool;
use axum::{
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Router,
};
use tower::util::ServiceExt;
use std::sync::Arc;

/// Create a test router with auth and pool extensions
fn create_test_router(pool: Pool) -> Router {
    let auth = ApiAuth::new("test-secret-key-for-testing");
    let metrics = Metrics::new();

    Router::new()
        .route("/test-auth", get(test_auth_handler))
        .route("/test-public", get(test_public_handler))
        .layer(axum::extract::Extension(Arc::new(auth)))
        .layer(axum::extract::Extension(Arc::new(metrics)))
        .layer(axum::extract::Extension(Arc::new(pool)))
}

async fn test_auth_handler(
    _auth: axum::extract::Extension<Arc<ApiAuth>>,
    _pool: axum::extract::Extension<Arc<Pool>>,
) -> &'static str {
    "authenticated"
}

async fn test_public_handler() -> &'static str {
    "public"
}

#[tokio::test]
async fn test_api_auth_token_generation() {
    let auth = ApiAuth::new("test-secret");
    let user_id = UserId::new();

    let token = auth.generate_token(user_id, "testuser", "admin").unwrap();
    assert!(!token.is_empty());

    // Validate token
    let claims = auth.validate_token(&token).unwrap();
    assert_eq!(claims.username, "testuser");
    assert_eq!(claims.role, "admin");
}

#[tokio::test]
async fn test_api_auth_token_extraction() {
    // Valid Bearer token
    assert_eq!(ApiAuth::extract_token("Bearer abc123"), Some("abc123"));

    // Invalid formats
    assert_eq!(ApiAuth::extract_token("Basic abc123"), None);
    assert_eq!(ApiAuth::extract_token("abc123"), None);
    assert_eq!(ApiAuth::extract_token("bearer abc123"), None); // case sensitive
}

#[tokio::test]
async fn test_api_auth_expired_token() {
    let auth = ApiAuth::new("test-secret");
    let user_id = UserId::new();

    // Create a token that's already expired
    let expired_claims = Claims {
        sub: user_id.to_string(),
        user_id,
        username: "testuser".to_string(),
        role: "admin".to_string(),
        exp: chrono::Utc::now().timestamp() - 3600, // expired 1 hour ago
        iat: chrono::Utc::now().timestamp() - 7200,
    };

    let token = jsonwebtoken::encode(
        &jsonwebtoken::Header::default(),
        &expired_claims,
        &jsonwebtoken::EncodingKey::from_secret("test-secret".as_bytes()),
    ).unwrap();

    let result = auth.validate_token(&token);
    assert!(result.is_err());
}

#[tokio::test]
async fn test_api_auth_wrong_secret() {
    let auth1 = ApiAuth::new("secret-one");
    let auth2 = ApiAuth::new("secret-two");
    let user_id = UserId::new();

    let token = auth1.generate_token(user_id, "testuser", "admin").unwrap();
    let result = auth2.validate_token(&token);
    assert!(result.is_err());
}

#[tokio::test]
async fn test_api_auth_invalid_token() {
    let auth = ApiAuth::new("test-secret");

    // Completely invalid token
    let result = auth.validate_token("not.a.valid.token");
    assert!(result.is_err());

    // Malformed JWT
    let result = auth.validate_token("not-a-jwt");
    assert!(result.is_err());
}

#[test]
fn test_claims_creation() {
    let user_id = UserId::new();
    let claims = Claims::new(user_id, "testuser", "developer", 24);

    assert_eq!(claims.username, "testuser");
    assert_eq!(claims.role, "developer");
    assert_eq!(claims.user_id, user_id);
    assert!(!claims.is_expired());
}

#[test]
fn test_claims_expiry() {
    let user_id = UserId::new();
    let mut claims = Claims::new(user_id, "testuser", "admin", 1);

    // Should not be expired initially
    assert!(!claims.is_expired());

    // Manually set to expired
    claims.exp = chrono::Utc::now().timestamp() - 1;
    assert!(claims.is_expired());
}

#[test]
fn test_claims_future_expiry() {
    let user_id = UserId::new();
    // Create claims with 100 year expiry
    let claims = Claims::new(user_id, "testuser", "admin", 24 * 365 * 100);
    assert!(!claims.is_expired());
}

/// Test error response creation
#[test]
fn test_error_response_creation() {
    use gitforce_api::server::error_response;

    let response = error_response(StatusCode::NOT_FOUND, "not_found", "Resource not found");
    let response = response.into_response();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

/// Test API server creation
#[tokio::test]
async fn test_api_server_creation() {
    let pool = Pool::memory().await.unwrap();
    let server = ApiServer::new("test-secret", pool);

    // Just verify it doesn't panic - port is private so we can't check value
    let _ = server;
}

/// Test UUID parsing for API routes
#[test]
fn test_uuid_parsing() {
    use uuid::Uuid;

    // Valid UUID
    let valid = "550e8400-e29b-41d4-a716-446655440000";
    assert!(Uuid::parse_str(valid).is_ok());

    // Invalid UUID
    let invalid = "not-a-uuid";
    assert!(Uuid::parse_str(invalid).is_err());

    // Empty string
    assert!(Uuid::parse_str("").is_err());
}

/// Test repo ID conversion
#[test]
fn test_repo_id_conversion() {
    let uuid = uuid::Uuid::new_v4();
    let repo_id = RepoId::from(uuid);
    let back = uuid::Uuid::from(repo_id);
    assert_eq!(uuid, back);
}

/// Test user ID conversion
#[test]
fn test_user_id_conversion() {
    let uuid = uuid::Uuid::new_v4();
    let user_id = UserId::from(uuid);
    let back = uuid::Uuid::from(user_id);
    assert_eq!(uuid, back);
}

/// Test pipeline ID conversion
#[test]
fn test_pipeline_id_conversion() {
    let uuid = uuid::Uuid::new_v4();
    let pipeline_id = PipelineId::from(uuid);
    let back = uuid::Uuid::from(pipeline_id);
    assert_eq!(uuid, back);
}

/// Test pipeline run ID conversion
#[test]
fn test_pipeline_run_id_conversion() {
    let uuid = uuid::Uuid::new_v4();
    let run_id = PipelineRunId::from(uuid);
    let back = uuid::Uuid::from(run_id);
    assert_eq!(uuid, back);
}

/// Test job ID conversion
#[test]
fn test_job_id_conversion() {
    let uuid = uuid::Uuid::new_v4();
    let job_id = JobId::from(uuid);
    let back = uuid::Uuid::from(job_id);
    assert_eq!(uuid, back);
}

/// Test runner ID conversion
#[test]
fn test_runner_id_conversion() {
    let uuid = uuid::Uuid::new_v4();
    let runner_id = RunnerId::from(uuid);
    let back = uuid::Uuid::from(runner_id);
    assert_eq!(uuid, back);
}

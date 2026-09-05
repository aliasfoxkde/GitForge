//! Authentication middleware

use crate::auth::Claims;
use axum::{
    extract::{Extension, Request},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use gitforge_db::{queries::UserQueries, Pool};
use std::sync::Arc;

/// Paths that don't require authentication
const PUBLIC_PATHS: &[&str] = &["/health", "/metrics", "/swagger-ui", "/api-docs", "/docs"];

/// Error response for auth failures
#[derive(Debug, serde::Serialize)]
pub struct AuthErrorResponse {
    pub error: String,
    pub message: String,
}

/// Authentication middleware
pub async fn auth_middleware(
    Extension(auth): Extension<Arc<crate::auth::ApiAuth>>,
    Extension(pool): Extension<Arc<Pool>>,
    request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path().to_string();

    // Skip auth for public paths
    for public_path in PUBLIC_PATHS {
        if path.starts_with(public_path) {
            return next.run(request).await;
        }
    }

    // Extract Authorization header
    let auth_header = request
        .headers()
        .get("Authorization")
        .and_then(|v| v.to_str().ok());

    let token = match auth_header {
        Some(header) => match crate::auth::ApiAuth::extract_token(header) {
            Some(token) => token,
            None => {
                return auth_error_response(
                    "missing_token",
                    "Missing or invalid Authorization header",
                );
            }
        },
        None => {
            return auth_error_response("missing_token", "Missing Authorization header");
        }
    };

    // Validate token
    match auth.validate_token(token) {
        Ok(mut claims) => {
            // Resolve the current persisted role so role changes take effect
            // immediately instead of waiting for a JWT to expire.
            match UserQueries::get_role(&pool, claims.user_id).await {
                Ok(Some(role)) => claims.role = role,
                Ok(None) => {
                    return auth_error_response("invalid_token", "Unknown user");
                }
                Err(error) => {
                    tracing::error!(%error, user_id = %claims.user_id, "failed to resolve authenticated user role");
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(AuthErrorResponse {
                            error: "authentication_unavailable".to_string(),
                            message: "Authentication service unavailable".to_string(),
                        }),
                    )
                        .into_response();
                }
            }
            let mut request = request;
            request.extensions_mut().insert(claims);
            next.run(request).await
        }
        Err(e) => {
            tracing::debug!(error = %e, "token validation failed");
            auth_error_response("invalid_token", "Invalid or expired token")
        }
    }
}

/// Create an authentication error response
fn auth_error_response(error: &str, message: &str) -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(AuthErrorResponse {
            error: error.to_string(),
            message: message.to_string(),
        }),
    )
        .into_response()
}

/// Extractor for authenticated user claims
use axum::extract::FromRequestParts;
use axum::http::request::Parts;

#[derive(Clone, Debug)]
pub struct AuthenticatedUser {
    pub claims: Claims,
}

impl<S: Send + Sync> FromRequestParts<S> for AuthenticatedUser {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let claims = parts.extensions.get::<Claims>().cloned().ok_or_else(|| {
            (
                StatusCode::UNAUTHORIZED,
                Json(AuthErrorResponse {
                    error: "unauthenticated".to_string(),
                    message: "No authentication context".to_string(),
                }),
            )
                .into_response()
        })?;

        Ok(AuthenticatedUser { claims })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use gitforge_common::UserId;

    #[test]
    fn test_public_paths() {
        assert!(PUBLIC_PATHS.iter().any(|p| "/health".starts_with(p)));
        assert!(PUBLIC_PATHS.iter().any(|p| "/metrics".starts_with(p)));
        assert!(!PUBLIC_PATHS.iter().any(|p| "/repos".starts_with(p)));
    }

    #[test]
    fn test_auth_error_response() {
        let response = auth_error_response("test_error", "test message");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn test_public_paths_includes_swagger_ui() {
        assert!(PUBLIC_PATHS.iter().any(|p| "/swagger-ui".starts_with(p)));
        assert!(PUBLIC_PATHS.iter().any(|p| "/api-docs".starts_with(p)));
    }

    #[test]
    fn test_public_paths_includes_docs() {
        assert!(PUBLIC_PATHS.iter().any(|p| "/docs".starts_with(p)));
    }

    #[test]
    fn test_auth_error_response_content() {
        let response = auth_error_response("invalid_token", "Token has expired");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn test_auth_middleware_skips_public_paths() {
        // Test that the path matching logic works correctly
        let public_path = "/health";
        let protected_path = "/api/repos";

        // Public paths should match their prefixes
        assert!(PUBLIC_PATHS.iter().any(|p| public_path.starts_with(p)));
        // Protected paths should not match public prefixes
        assert!(!PUBLIC_PATHS.iter().any(|p| protected_path.starts_with(p)));
    }

    #[test]
    fn test_auth_error_response_missing_token() {
        let response = auth_error_response("missing_token", "Missing Authorization header");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn test_auth_error_response_invalid_token() {
        let response = auth_error_response("invalid_token", "Invalid or expired token");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn test_authenticated_user_error_response() {
        // Test that the error response for missing auth context has correct status
        let response = auth_error_response("unauthenticated", "No authentication context");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn test_authenticated_user_creation() {
        let claims = Claims {
            sub: "user-123".to_string(),
            user_id: UserId::new(),
            username: "testuser".to_string(),
            role: "admin".to_string(),
            exp: Utc::now().timestamp() + 3600,
            iat: Utc::now().timestamp(),
        };
        let user = AuthenticatedUser { claims };
        assert_eq!(user.claims.username, "testuser");
    }

    #[test]
    fn test_authenticated_user_debug() {
        let claims = Claims {
            sub: "user-123".to_string(),
            user_id: UserId::new(),
            username: "testuser".to_string(),
            role: "admin".to_string(),
            exp: Utc::now().timestamp() + 3600,
            iat: Utc::now().timestamp(),
        };
        let user = AuthenticatedUser { claims };
        let debug_str = format!("{:?}", user);
        assert!(debug_str.contains("testuser"));
    }

    #[test]
    fn test_authenticated_user_clone() {
        let claims = Claims {
            sub: "user-123".to_string(),
            user_id: UserId::new(),
            username: "testuser".to_string(),
            role: "admin".to_string(),
            exp: Utc::now().timestamp() + 3600,
            iat: Utc::now().timestamp(),
        };
        let user = AuthenticatedUser { claims };
        let cloned = user.clone();
        assert_eq!(cloned.claims.username, user.claims.username);
    }

    #[test]
    fn test_public_paths_exact_match() {
        // Test that paths that are exactly the public path work
        assert!(PUBLIC_PATHS
            .iter()
            .any(|p| "/health" == *p || "/health".starts_with(p)));
        assert!(PUBLIC_PATHS
            .iter()
            .any(|p| "/metrics" == *p || "/metrics".starts_with(p)));
    }

    #[test]
    fn test_auth_error_response_all_variants() {
        // Test all error types
        let resp1 = auth_error_response("missing_token", "Missing token");
        assert_eq!(resp1.status(), StatusCode::UNAUTHORIZED);

        let resp2 = auth_error_response("invalid_token", "Invalid token");
        assert_eq!(resp2.status(), StatusCode::UNAUTHORIZED);

        let resp3 = auth_error_response("expired_token", "Token expired");
        assert_eq!(resp3.status(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn test_claims_in_authenticated_user() {
        let user_id = UserId::new();
        let claims = Claims::new(user_id, "testuser", "developer", 2);
        let user = AuthenticatedUser { claims };
        assert_eq!(user.claims.username, "testuser");
        assert_eq!(user.claims.role, "developer");
    }

    #[test]
    fn test_auth_error_response_debug() {
        let response = auth_error_response("test", "message");
        // Just verify it doesn't panic when formatted
        let debug_str = format!("{:?}", response);
        assert!(!debug_str.is_empty());
    }

    #[test]
    fn test_public_paths_all_values() {
        assert_eq!(PUBLIC_PATHS.len(), 5);
        assert!(PUBLIC_PATHS.contains(&"/health"));
        assert!(PUBLIC_PATHS.contains(&"/metrics"));
        assert!(PUBLIC_PATHS.contains(&"/swagger-ui"));
        assert!(PUBLIC_PATHS.contains(&"/api-docs"));
        assert!(PUBLIC_PATHS.contains(&"/docs"));
    }

    #[test]
    fn test_authenticated_user_type_equivalence() {
        let claims = Claims {
            sub: "user-456".to_string(),
            user_id: UserId::new(),
            username: "anotheruser".to_string(),
            role: "viewer".to_string(),
            exp: Utc::now().timestamp() + 7200,
            iat: Utc::now().timestamp(),
        };
        let user = AuthenticatedUser { claims };
        assert_eq!(user.claims.username, "anotheruser");
        assert_eq!(user.claims.role, "viewer");
    }

    #[test]
    fn test_claims_with_different_expiry() {
        let user_id = UserId::new();
        for hours in &[1, 2, 24, 48, 168] {
            let claims = Claims::new(user_id, "user", "role", *hours);
            let expected_exp = claims.iat + (hours * 3600);
            assert_eq!(claims.exp, expected_exp);
        }
    }

    #[test]
    fn test_auth_error_response_message_preservation() {
        let test_message = "This is a detailed error message";
        let response = auth_error_response("code", test_message);
        // Response is created successfully - the actual message content
        // is tested via JSON serialization in integration tests
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
}

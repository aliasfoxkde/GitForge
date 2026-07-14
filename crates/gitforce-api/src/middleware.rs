//! Authentication middleware

use crate::auth::Claims;
use axum::{
    extract::Request,
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use std::sync::Arc;

/// Paths that don't require authentication
const PUBLIC_PATHS: &[&str] = &[
    "/health",
    "/metrics",
    "/swagger-ui",
    "/api-docs",
    "/docs",
];

/// Error response for auth failures
#[derive(Debug, serde::Serialize)]
pub struct AuthErrorResponse {
    pub error: String,
    pub message: String,
}

/// Authentication middleware
pub async fn auth_middleware(
    auth: Arc<crate::auth::ApiAuth>,
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
                return auth_error_response("missing_token", "Missing or invalid Authorization header");
            }
        },
        None => {
            return auth_error_response("missing_token", "Missing Authorization header");
        }
    };

    // Validate token
    match auth.validate_token(&token) {
        Ok(claims) => {
            // Store claims in extensions for route handlers
            let mut request = request;
            request.extensions_mut().insert(claims);
            next.run(request).await
        }
        Err(e) => {
            tracing::debug!("token validation failed: {}", e);
            auth_error_response("invalid_token", "Invalid or expired token")
        }
    }
}

/// Create an authentication error response
fn auth_error_response(error: &str, message: &str) -> Response {
    (StatusCode::UNAUTHORIZED, Json(AuthErrorResponse {
        error: error.to_string(),
        message: message.to_string(),
    })).into_response()
}

/// Extractor for authenticated user claims
use axum::extract::FromRequestParts;
use axum::http::request::Parts;

#[derive(Clone, Debug)]
pub struct AuthenticatedUser {
    pub claims: Claims,
}

#[axum::async_trait]
impl<S: Send + Sync> FromRequestParts<S> for AuthenticatedUser {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let claims = parts
            .extensions
            .get::<Claims>()
            .cloned()
            .ok_or_else(|| {
                (StatusCode::UNAUTHORIZED, Json(AuthErrorResponse {
                    error: "unauthenticated".to_string(),
                    message: "No authentication context".to_string(),
                })).into_response()
            })?;

        Ok(AuthenticatedUser { claims })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}

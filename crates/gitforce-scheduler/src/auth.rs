//! Optional Bearer-token authentication for the scheduler HTTP API.
//!
//! Phase 1/2 is backward-compatible: when `SCHEDULER_SHARED_SECRET` is absent or
//! empty the scheduler accepts all requests (no auth). When a non-empty secret is
//! configured, every route except `/health` requires a valid `Authorization: Bearer <token>`
//! header.  Missing, malformed, and wrong tokens all return an identical `401`
//! JSON response without logging the token value.

use axum::{
    body::Body,
    extract::Request,
    http::{header::AUTHORIZATION, StatusCode},
    response::IntoResponse,
    Router,
};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tower::{Layer, Service};

/// Application state that carries an optional shared secret.
#[derive(Clone)]
pub struct SchedulerAuthState {
    /// The configured shared secret, or `None` when auth is disabled.
    pub(crate) secret: Option<String>,
}

impl SchedulerAuthState {
    /// Construct auth state from an environment variable.
    ///
    /// Empty strings are treated as "not configured" (auth disabled).
    pub fn from_env() -> Self {
        Self {
            secret: std::env::var("SCHEDULER_SHARED_SECRET")
                .ok()
                .filter(|v| !v.is_empty()),
        }
    }

    /// Returns `true` when a non-empty secret is configured.
    pub fn is_enabled(&self) -> bool {
        self.secret.is_some()
    }

    /// Validate a bearer token against the configured secret using constant-time
    /// comparison.
    ///
    /// Returns `true` when auth is disabled (`secret.is_none()`) or when the
    /// token matches the secret. Returns `false` for all other cases (missing,
    /// malformed, wrong) so callers receive identical responses.
    #[must_use]
    pub fn validate_token(&self, token: Option<&str>) -> bool {
        let Some(secret) = &self.secret else {
            return true; // Auth disabled.
        };

        let Some(token) = token else {
            return false;
        };

        // Strip the "Bearer " prefix if present.
        let token = token.strip_prefix("Bearer ").unwrap_or(token);

        constant_time_eq(secret.as_bytes(), token.as_bytes())
    }
}

/// Constant-time byte-slice equality check.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// JSON 401 error body for all auth failures (identical for missing/wrong/malformed).
pub fn auth_error_response() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [
            (axum::http::header::CONTENT_TYPE, "application/json"),
            (
                axum::http::header::WWW_AUTHENTICATE,
                r#"Bearer realm="scheduler""#,
            ),
        ],
        r#"{"error":"unauthorized","message":"missing or invalid bearer token"}"#,
    )
        .into_response()
}

/// A tower [`Layer`] that applies bearer-token auth to a service.
///
/// When no secret is configured the service is returned unchanged.
#[derive(Clone)]
pub struct AuthLayer {
    state: Arc<SchedulerAuthState>,
}

impl AuthLayer {
    pub fn new(state: SchedulerAuthState) -> Self {
        Self {
            state: Arc::new(state),
        }
    }
}

impl<S> Layer<S> for AuthLayer {
    type Service = AuthService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        AuthService {
            inner,
            state: self.state.clone(),
        }
    }
}

/// A tower [`Service`] that guards the inner service with bearer-token auth.
#[derive(Clone)]
pub struct AuthService<S> {
    inner: S,
    state: Arc<SchedulerAuthState>,
}

impl<S> Service<Request<Body>> for AuthService<S>
where
    S: Service<
            Request<Body>,
            Response = axum::response::Response,
            Error = std::convert::Infallible,
        > + Clone
        + Send
        + 'static,
    S::Future: Send + 'static,
{
    type Response = axum::response::Response;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;
    type Error = std::convert::Infallible;

    fn poll_ready(
        &mut self,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        // Router is always ready (Error = Infallible, never errors).
        std::task::Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: Request<Body>) -> Self::Future {
        let state = self.state.clone();
        // Clone the inner service — S: Clone + 'static, so this is safe.
        let mut inner = self.inner.clone();

        Box::pin(async move {
            if !state.is_enabled() {
                return inner.call(request).await;
            }

            let token = request
                .headers()
                .get(AUTHORIZATION)
                .and_then(|v| v.to_str().ok());

            if state.validate_token(token) {
                inner.call(request).await
            } else {
                Ok(auth_error_response())
            }
        })
    }
}

/// Wrap a [`axum::Router`] with auth middleware when a secret is configured.
///
/// Routes added via `.route()` AFTER this call (e.g. `/health`) remain
/// unauthenticated — the middleware is only applied to routes present at
/// wrapping time.
pub fn with_auth<S: Clone + Send + Sync + 'static>(
    state: SchedulerAuthState,
    router: Router<S>,
) -> Router<S> {
    if !state.is_enabled() {
        return router;
    }
    router.layer(AuthLayer::new(state))
}

type Response = axum::response::Response;

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------------
    // SchedulerAuthState construction
    // -------------------------------------------------------------------------

    #[test]
    fn test_auth_state_disabled_when_var_absent() {
        std::env::remove_var("SCHEDULER_SHARED_SECRET");
        let state = SchedulerAuthState::from_env();
        assert!(!state.is_enabled());
        assert!(state.validate_token(None));
        assert!(state.validate_token(Some("Bearer anything")));
    }

    #[test]
    fn test_auth_state_disabled_when_var_empty() {
        std::env::set_var("SCHEDULER_SHARED_SECRET", "");
        let state = SchedulerAuthState::from_env();
        assert!(!state.is_enabled());
        assert!(state.validate_token(None));
        assert!(state.validate_token(Some("Bearer anything")));
        std::env::remove_var("SCHEDULER_SHARED_SECRET");
    }

    #[test]
    fn test_auth_state_enabled_when_var_present() {
        std::env::set_var("SCHEDULER_SHARED_SECRET", "super-secret-123");
        let state = SchedulerAuthState::from_env();
        assert!(state.is_enabled());
        std::env::remove_var("SCHEDULER_SHARED_SECRET");
    }

    // -------------------------------------------------------------------------
    // Token validation
    // -------------------------------------------------------------------------

    #[test]
    fn test_validate_token_auth_disabled_always_true() {
        let state = SchedulerAuthState { secret: None };
        assert!(state.validate_token(None));
        assert!(state.validate_token(Some("")));
        assert!(state.validate_token(Some("Bearer abc")));
    }

    #[test]
    fn test_validate_token_auth_enabled_correct() {
        std::env::set_var("SCHEDULER_SHARED_SECRET", "hunter2");
        let state = SchedulerAuthState::from_env();

        assert!(state.validate_token(Some("hunter2")));
        assert!(state.validate_token(Some("Bearer hunter2")));

        std::env::remove_var("SCHEDULER_SHARED_SECRET");
    }

    #[test]
    fn test_validate_token_auth_enabled_wrong() {
        std::env::set_var("SCHEDULER_SHARED_SECRET", "hunter2");
        let state = SchedulerAuthState::from_env();

        assert!(!state.validate_token(Some("wrong-token")));
        assert!(!state.validate_token(Some("Bearer wrong-token")));
        assert!(!state.validate_token(Some("")));
        assert!(!state.validate_token(None));

        std::env::remove_var("SCHEDULER_SHARED_SECRET");
    }

    // -------------------------------------------------------------------------
    // constant_time_eq
    // -------------------------------------------------------------------------

    #[test]
    fn test_constant_time_eq_equal() {
        assert!(constant_time_eq(b"hello", b"hello"));
        assert!(constant_time_eq(b"", b""));
    }

    #[test]
    fn test_constant_time_eq_unequal_length() {
        assert!(!constant_time_eq(b"hello", b"hell"));
        assert!(!constant_time_eq(b"hell", b"hello"));
    }

    #[test]
    fn test_constant_time_eq_unequal_content() {
        assert!(!constant_time_eq(b"hello", b"world"));
        assert!(!constant_time_eq(b"hunter2", b"hunter3"));
    }

    // -------------------------------------------------------------------------
    // auth_error_response
    // -------------------------------------------------------------------------

    #[test]
    fn test_auth_error_response_status() {
        let resp = auth_error_response();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn test_auth_error_response_content_type() {
        let resp = auth_error_response();
        assert_eq!(
            resp.headers()
                .get(axum::http::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok()),
            Some("application/json")
        );
    }

    #[tokio::test]
    async fn test_auth_error_response_body() {
        let resp = auth_error_response();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"], "unauthorized");
        assert!(json["message"].as_str().unwrap().contains("bearer"));
    }

    #[test]
    fn test_auth_error_response_www_authenticate() {
        let resp = auth_error_response();
        assert_eq!(
            resp.headers()
                .get(axum::http::header::WWW_AUTHENTICATE)
                .and_then(|v| v.to_str().ok()),
            Some(r#"Bearer realm="scheduler""#)
        );
    }
}

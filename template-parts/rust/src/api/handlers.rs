//! HTTP handler stubs using axum.

use axum::{
    body::Body,
    extract::State,
    response::Response,
    routing::{get, post},
    Router,
};
use axum::http::StatusCode;
use std::sync::Arc;

use crate::models::types::{ApiResponse, HealthResponse};
use crate::services::business::BusinessService;

/// Application state shared across handlers.
#[derive(Clone)]
pub struct AppState {
    pub business: Arc<BusinessService>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            business: Arc::new(BusinessService::new()),
        }
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

/// GET /health - Health check endpoint.
pub async fn health() -> Response<Body> {
    let response = HealthResponse {
        status: "ok".to_string(),
        version: env!("CARGO_PKG_VERSION"),
    };
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "application/json")
        .body(Body::from(serde_json::to_string(&response).unwrap()))
        .unwrap()
}

/// GET /api/hello - Hello endpoint.
pub async fn hello(State(state): State<AppState>) -> Response<Body> {
    let message = state.business.get_hello_message();
    let response = ApiResponse {
        success: true,
        data: Some(serde_json::json!({ "message": message })),
        error: None,
    };
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "application/json")
        .body(Body::from(serde_json::to_string(&response).unwrap()))
        .unwrap()
}

/// POST /api/echo - Echo endpoint that returns the request body.
pub async fn echo(
    State(_state): State<AppState>,
    body: String,
) -> Response<Body> {
    let response = ApiResponse {
        success: true,
        data: Some(serde_json::json!({ "echo": body })),
        error: None,
    };
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "application/json")
        .body(Body::from(serde_json::to_string(&response).unwrap()))
        .unwrap()
}

/// Create the application router with all routes.
pub fn create_router() -> Router {
    let state = AppState::new();
    Router::new()
        .route("/health", get(health))
        .route("/api/hello", get(hello))
        .route("/api/echo", post(echo))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_app_state_creation() {
        let state = AppState::new();
        assert!(state.business.get_hello_message().contains("Hello"));
    }
}

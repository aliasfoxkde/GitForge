//! API server

use crate::auth::ApiAuth;
use crate::routes::{artifact_routes, ci_routes, repo_routes, runner_routes};
use axum::{
    extract::Extension,
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use serde::Serialize;
use std::net::SocketAddr;
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};

/// API server
pub struct ApiServer {
    router: Router,
    port: u16,
}

impl ApiServer {
    /// Create a new API server
    pub fn new(jwt_secret: &str) -> Self {
        let auth = ApiAuth::new(jwt_secret);

        let cors = CorsLayer::new()
            .allow_origin(Any)
            .allow_methods(Any)
            .allow_headers(Any);

        let app = Router::new()
            .route("/health", get(health_check))
            .merge(repo_routes())
            .merge(ci_routes())
            .merge(runner_routes())
            .merge(artifact_routes())
            .layer(cors)
            .layer(Extension(Arc::new(auth)));

        Self { router: app, port: 8080 }
    }

    /// Set the port
    pub fn with_port(mut self, port: u16) -> Self {
        self.port = port;
        self
    }

    /// Start the server
    pub async fn start(self) -> anyhow::Result<()> {
        let addr = SocketAddr::from(([0, 0, 0, 0], self.port));
        tracing::info!("API server listening on {}", addr);

        let listener = tokio::net::TcpListener::bind(addr).await?;
        axum::serve(listener, self.router).await?;

        Ok(())
    }
}

/// Health check endpoint
async fn health_check() -> impl IntoResponse {
    Json(serde_json::json!({
        "status": "healthy",
        "timestamp": chrono::Utc::now().to_rfc3339()
    }))
}

/// Error response
#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: String,
    pub message: String,
}

/// Helper to create an error response
pub fn error_response(status: StatusCode, error: &str, message: &str) -> impl IntoResponse {
    (status, Json(ErrorResponse {
        error: error.to_string(),
        message: message.to_string(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_server_creation() {
        let server = ApiServer::new("test-secret");
        assert_eq!(server.port, 8080);
    }
}

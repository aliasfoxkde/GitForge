//! API server

use crate::auth::ApiAuth;
use crate::metrics::Metrics;
use crate::openapi::api_docs_routes;
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
        let metrics = Metrics::new();

        let cors = CorsLayer::new()
            .allow_origin(Any)
            .allow_methods(Any)
            .allow_headers(Any);

        let app = Router::new()
            .route("/health", get(health_check))
            .route("/metrics", get(metrics_handler))
            .merge(api_docs_routes())
            .merge(repo_routes())
            .merge(ci_routes())
            .merge(runner_routes())
            .merge(artifact_routes())
            .layer(cors)
            .layer(Extension(Arc::new(auth)))
            .layer(Extension(Arc::new(metrics)));

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
        tracing::info!("Swagger UI available at /swagger-ui");
        tracing::info!("OpenAPI spec at /api-docs/openapi.json");
        tracing::info!("Metrics available at /metrics");

        let listener = tokio::net::TcpListener::bind(addr).await?;
        axum::serve(listener, self.router).await?;

        Ok(())
    }
}

/// Health check endpoint
async fn health_check() -> impl IntoResponse {
    Json(HealthResponse {
        status: "healthy".to_string(),
        timestamp: chrono::Utc::now().to_rfc3339(),
    })
}

/// Health response
#[derive(serde::Serialize)]
pub struct HealthResponse {
    pub status: String,
    pub timestamp: String,
}

/// Metrics endpoint handler
async fn metrics_handler(
    Extension(metrics): Extension<Arc<Metrics>>,
) -> impl IntoResponse {
    use prometheus::Encoder;
    let encoder = prometheus::TextEncoder::new();
    let metric_families = metrics.registry.gather();
    let mut buffer = Vec::new();
    if let Err(e) = encoder.encode(&metric_families, &mut buffer) {
        tracing::error!("Failed to encode metrics: {}", e);
    }
    String::from_utf8(buffer).unwrap_or_default()
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

    #[tokio::test]
    async fn test_health_check() {
        let response = health_check().await.into_response();
        assert_eq!(response.status(), StatusCode::OK);
    }
}

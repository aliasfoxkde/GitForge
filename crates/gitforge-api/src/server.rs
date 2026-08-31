//! API server

use crate::auth::ApiAuth;
use crate::metrics::Metrics;
use crate::metrics_middleware::MetricsLayer;
use crate::routes::{
    artifact_routes, ci_routes, public_runner_routes, repo_routes, runner_routes, user_routes,
    webhook_routes,
};
use axum::{
    extract::Extension,
    http::StatusCode,
    middleware,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use gitforge_db::Pool;
use gitforge_scheduler::Scheduler;
use gitforge_storage::FileStorage;
use serde::Serialize;
use std::net::SocketAddr;
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};

/// API server
pub struct ApiServer {
    pub router: Router,
    pub port: u16,
}

impl ApiServer {
    /// Create a new API server
    pub fn new(jwt_secret: &str, pool: Pool) -> Self {
        Self::with_storage(jwt_secret, pool, None)
    }

    /// Create a new API server with custom storage path
    pub fn with_storage(
        jwt_secret: &str,
        pool: Pool,
        _storage_path: Option<std::path::PathBuf>,
    ) -> Self {
        let auth = ApiAuth::new(jwt_secret);
        let metrics = Metrics::new();
        let metrics_arc = Arc::new(metrics);

        // Configure CORS - restrictive by default
        // Use CORS_ALLOWED_ORIGINS env var to specify comma-separated allowed origins
        // e.g., CORS_ALLOWED_ORIGINS=https://app.example.com,https://dashboard.example.com
        // If not set, defaults to allowing only localhost for development
        let allowed_origins = std::env::var("CORS_ALLOWED_ORIGINS")
            .unwrap_or_else(|_| "http://localhost:3000,http://localhost:42780".to_string());

        let origins: Vec<&str> = allowed_origins.split(',').map(|s| s.trim()).collect();

        let cors = if origins.contains(&"*") {
            // Wildcard only allowed in development (not production)
            tracing::warn!("CORS wildcard '*' is insecure for production!");
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any)
        } else {
            let allow_list: Vec<axum::http::HeaderValue> =
                origins.iter().filter_map(|s| s.parse().ok()).collect();

            CorsLayer::new()
                .allow_origin(allow_list)
                .allow_methods(Any)
                .allow_headers(Any)
        };

        // Public routes (no auth required)
        let health = Router::new().route("/health", get(health_check));
        let metrics = Router::new().route("/metrics", get(metrics_handler));
        let swagger = Router::new()
            .route("/swagger-ui", get(crate::openapi::swagger_ui))
            .route("/api-docs/openapi.json", get(crate::openapi::openapi_spec));
        let dashboard = crate::dashboard::dashboard_routes();

        let mut public_routes = Router::new();
        public_routes = public_routes.merge(health);
        public_routes = public_routes.merge(metrics);
        public_routes = public_routes.merge(dashboard);
        public_routes = public_routes.merge(swagger);
        public_routes = public_routes
            .nest("/api", public_runner_routes())
            .layer(Extension(Arc::new(pool.clone())));

        // Auth routes (public - no auth required for login)
        let auth_routes = Router::new()
            .route("/auth/login", post(crate::routes::login))
            .route("/auth/status", get(crate::routes::auth_status))
            .layer(Extension(Arc::new(auth.clone())))
            .layer(Extension(Arc::new(pool.clone())));

        // Protected routes (auth required)
        let pool_arc = Arc::new(pool);
        let protected_routes = Router::new()
            .merge(repo_routes())
            .merge(ci_routes())
            .merge(runner_routes())
            .merge(user_routes())
            .merge(artifact_routes())
            .merge(webhook_routes())
            // Authenticate once at the protected-route boundary. Individual
            // handlers may still apply resource authorization, but token
            // parsing and validation must not depend on every handler
            // remembering to duplicate it.
            .layer(middleware::from_fn(crate::middleware::auth_middleware))
            .layer(Extension(Arc::new(auth.clone())))
            .layer(Extension(pool_arc.clone()));

        // Metrics layer for automatic request recording
        let metrics_layer = MetricsLayer::new(metrics_arc.clone());

        let app = public_routes
            .layer(cors)
            .layer(metrics_layer)
            .layer(Extension(pool_arc.clone()))
            .layer(Extension(metrics_arc))
            .merge(auth_routes)
            .nest("/api", protected_routes);

        Self {
            router: app,
            port: 42780,
        }
    }

    /// Add storage extension to the router
    pub fn with_storage_extension(self, storage: Arc<FileStorage>) -> Self {
        let app = self.router.layer(Extension(storage));
        Self {
            router: app,
            ..self
        }
    }

    /// Add scheduler extension for job queuing
    pub fn with_scheduler_extension(self, scheduler: Arc<Scheduler>) -> Self {
        let app = self.router.layer(Extension(scheduler));
        Self {
            router: app,
            ..self
        }
    }

    /// Set the port
    #[must_use]
    pub fn with_port(mut self, port: u16) -> Self {
        self.port = port;
        self
    }

    /// Get the router (for testing)
    pub fn into_router(self) -> Router {
        self.router
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
async fn health_check(Extension(pool): Extension<Arc<Pool>>) -> impl IntoResponse {
    let db_status = match pool.health_check().await {
        Ok(_) => "connected",
        Err(e) => {
            tracing::warn!("database health check failed: {}", e);
            "disconnected"
        }
    };

    let overall_status = if db_status == "connected" {
        "healthy"
    } else {
        "unhealthy"
    };

    Json(HealthResponse {
        status: overall_status.to_string(),
        timestamp: chrono::Utc::now().to_rfc3339(),
        database: db_status.to_string(),
    })
}

/// Health response
#[derive(serde::Serialize)]
pub struct HealthResponse {
    pub status: String,
    pub timestamp: String,
    pub database: String,
}

/// Metrics endpoint handler
async fn metrics_handler(Extension(metrics): Extension<Arc<Metrics>>) -> impl IntoResponse {
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
    (
        status,
        Json(ErrorResponse {
            error: error.to_string(),
            message: message.to_string(),
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_server_creation() {
        let pool = Pool::memory().await.unwrap();
        let server = ApiServer::new("test-secret", pool);
        assert_eq!(server.port, 42780);
    }

    #[tokio::test]
    async fn test_health_check() {
        let pool = Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();
        let response = health_check(Extension(Arc::new(pool)))
            .await
            .into_response();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_health_check_no_migration() {
        let pool = Pool::memory().await.unwrap();
        // Don't migrate - health check should still work
        let response = health_check(Extension(Arc::new(pool)))
            .await
            .into_response();
        // May return unhealthy but shouldn't panic
        assert!(
            response.status() == StatusCode::OK
                || response.status() == StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[tokio::test]
    async fn test_error_response_returns_correct_status() {
        let response = error_response(StatusCode::NOT_FOUND, "not_found", "Resource not found");
        let resp = response.into_response();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn test_error_response_serialization() {
        let response = ErrorResponse {
            error: "not_found".to_string(),
            message: "Item not found".to_string(),
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("not_found"));
        assert!(json.contains("Item not found"));
    }

    #[test]
    fn test_health_response_serialization() {
        let response = HealthResponse {
            status: "healthy".to_string(),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            database: "connected".to_string(),
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("healthy"));
        assert!(json.contains("connected"));
    }

    #[test]
    fn test_error_response_content() {
        let response = ErrorResponse {
            error: "bad_request".to_string(),
            message: "Invalid input".to_string(),
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("bad_request"));
        assert!(json.contains("Invalid input"));
    }

    #[tokio::test]
    async fn test_server_with_port() {
        let pool = Pool::memory().await.unwrap();
        let server = ApiServer::new("test-secret", pool).with_port(3000);
        assert_eq!(server.port, 3000);
    }

    #[tokio::test]
    async fn test_server_with_storage() {
        let pool = Pool::memory().await.unwrap();
        let server = ApiServer::with_storage(
            "test-secret",
            pool,
            Some(std::path::PathBuf::from("/tmp/storage")),
        );
        assert_eq!(server.port, 42780);
    }

    #[tokio::test]
    async fn test_server_into_router() {
        let pool = Pool::memory().await.unwrap();
        let server = ApiServer::new("test-secret", pool);
        let _router = server.into_router();
        // Router was created successfully
    }
}

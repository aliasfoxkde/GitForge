//! GitForge API
//!
//! REST API gateway for GitForge.
//!
//! ## Features
//!
//! - **OpenAPI/Swagger UI**: Interactive API documentation at `/swagger-ui`
//! - **Prometheus Metrics**: Observability at `/metrics`
//! - **RESTful Endpoints**: Repos, CI, Runners, Artifacts

// The OpenAPI spec is one large serde_json::json! literal; the macro needs
// extra recursion depth as the spec grows.
#![recursion_limit = "512"]

pub mod auth;
pub mod dashboard;
pub mod metrics;
pub mod metrics_middleware;
pub mod middleware;
pub mod openapi;
pub mod rate_limit;
pub mod routes;
pub mod server;

pub use auth::{ApiAuth, Claims};
pub use metrics::Metrics;
pub use rate_limit::{RateLimitConfig, RateLimiter};
pub use routes::CiTriggerClient;
pub use server::ApiServer;

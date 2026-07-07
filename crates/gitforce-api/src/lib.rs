//! GitForge API
//!
//! REST API gateway for GitForge.
//!
//! ## Features
//!
//! - **OpenAPI/Swagger UI**: Interactive API documentation at `/swagger-ui`
//! - **Prometheus Metrics**: Observability at `/metrics`
//! - **RESTful Endpoints**: Repos, CI, Runners, Artifacts

pub mod auth;
pub mod metrics;
pub mod openapi;
pub mod routes;
pub mod server;

pub use auth::{ApiAuth, Claims};
pub use metrics::Metrics;
pub use server::ApiServer;

//! GitForge API
//!
//! REST API gateway for GitForge.
//!
//! ## Features
//!
//! - **Prometheus Metrics**: Observability at `/metrics`
//! - **RESTful Endpoints**: Repos, CI, Runners, Artifacts

pub mod auth;
pub mod metrics;
pub mod routes;
pub mod server;

pub use auth::{ApiAuth, Claims};
pub use metrics::Metrics;
pub use server::ApiServer;

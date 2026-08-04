//! project_name - A Rust library built with axum, tokio, and tower.

pub mod api;
pub mod models;
pub mod services;
pub mod utils;

pub use models::types::*;
pub use services::business::BusinessService;
pub use utils::logging::init_tracing;

/// Re-export commonly used types for convenience.
pub mod prelude {
    pub use crate::models::types::*;
    pub use crate::services::business::BusinessService;
}

//! API routes

pub mod artifacts;
pub mod auth;
pub mod ci;
pub mod repo;
pub mod review;
pub mod runners;
pub mod users;
pub mod webhook;

pub use artifacts::*;
pub use auth::*;
pub use ci::*;
pub use repo::*;
pub use review::*;
pub use runners::*;
pub use users::*;
pub use webhook::*;

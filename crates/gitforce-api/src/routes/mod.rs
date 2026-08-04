//! API routes

pub mod artifacts;
pub mod auth;
pub mod ci;
pub mod repo;
pub mod runners;
pub mod webhook;

pub use artifacts::*;
pub use auth::*;
pub use ci::*;
pub use repo::*;
pub use runners::*;
pub use webhook::*;

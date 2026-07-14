//! GitForce Storage
//!
//! Artifact and cache storage.

pub mod artifact;
pub mod cache;
pub mod filesystem;

pub use artifact::{Artifact, ArtifactId, ArtifactStore};
pub use cache::{CacheKey, CacheStore};
pub use filesystem::FileStorage;

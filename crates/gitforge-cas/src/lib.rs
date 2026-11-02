//! GitForge content-addressed storage (CAS)
//!
//! Blobs are stored by SHA-256 content hash under a configurable root, with
//! refcounts in a dedicated SQLite index so GC can prove a blob is dead.
//! Issue #339.

pub mod store;
pub mod workspace;

pub use store::{CasStore, SweepOutcome};
pub use workspace::{WorkspaceHandle, WorkspaceManager};

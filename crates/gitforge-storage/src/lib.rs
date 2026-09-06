//! GitForce Storage
//!
//! Artifact and cache storage.

pub mod artifact;
pub mod cache;
pub mod filesystem;
pub mod job_logs;
pub mod publication;
pub mod receipt;
pub mod receipt_store;

pub use artifact::{Artifact, ArtifactId, ArtifactStore};
pub use cache::{CacheKey, CacheStore, FileCacheStore};
pub use filesystem::FileStorage;
pub use job_logs::{FileJobLogStore, InMemoryJobLogStore, JobLogMeta, JobLogStore};
pub use receipt::{
    ArtifactReceipt, JobReceipt, LogReceipt, ReceiptStatus, MAX_ARTIFACT_BYTES, MAX_LOG_BYTES,
    RECEIPT_VERSION,
};
pub use receipt_store::{
    FileReceiptStore, InMemoryReceiptStore, ReceiptMeta, ReceiptStore, ReceiptVerification,
};

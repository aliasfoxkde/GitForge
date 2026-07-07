//! Git protocol handlers

pub mod http;
pub mod ssh;

use gitforce_common::{RepoId, Result};
use std::future::Future;

/// Git protocol handler trait
#[async_trait::async_trait]
pub trait GitProtocolHandler: Send + Sync {
    /// Handle a git upload-pack request (git clone/fetch)
    async fn upload_pack(
        &self,
        repo_id: RepoId,
        input: Vec<u8>,
    ) -> Result<Vec<u8>>;

    /// Handle a git receive-pack request (git push)
    async fn receive_pack(
        &self,
        repo_id: RepoId,
        input: Vec<u8>,
    ) -> Result<Vec<u8>>;
}

/// Async function wrapper for GitProtocolHandler
pub struct FnHandler<F> {
    f: F,
}

impl<F> FnHandler<F> {
    pub fn new(f: F) -> Self {
        Self { f }
    }
}

#[async_trait::async_trait]
impl<F, R> GitProtocolHandler for FnHandler<F>
where
    F: Fn(RepoId, Vec<u8>) -> R + Send + Sync,
    R: Future<Output = Result<Vec<u8>>> + Send + 'static,
{
    async fn upload_pack(&self, repo_id: RepoId, input: Vec<u8>) -> Result<Vec<u8>> {
        (self.f)(repo_id, input).await
    }

    async fn receive_pack(&self, repo_id: RepoId, input: Vec<u8>) -> Result<Vec<u8>> {
        (self.f)(repo_id, input).await
    }
}

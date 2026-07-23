//! Git protocol handlers

pub mod http;
pub mod ssh;

use gitforce_common::{RepoId, Result};
use std::future::Future;

/// Git protocol handler trait
#[async_trait::async_trait]
pub trait GitProtocolHandler: Send + Sync {
    /// Handle a git upload-pack request (git clone/fetch)
    async fn upload_pack(&self, repo_id: RepoId, input: Vec<u8>) -> Result<Vec<u8>>;

    /// Handle a git receive-pack request (git push)
    async fn receive_pack(&self, repo_id: RepoId, input: Vec<u8>) -> Result<Vec<u8>>;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_fn_handler_upload_pack() {
        let handler = FnHandler::new(|_repo_id: RepoId, input: Vec<u8>| async move { Ok(input) });

        let repo_id = RepoId::new();
        let result = handler.upload_pack(repo_id, vec![1, 2, 3]).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), vec![1, 2, 3]);
    }

    #[tokio::test]
    async fn test_fn_handler_receive_pack() {
        let handler = FnHandler::new(|_repo_id: RepoId, input: Vec<u8>| async move { Ok(input) });

        let repo_id = RepoId::new();
        let result = handler.receive_pack(repo_id, vec![4, 5, 6]).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), vec![4, 5, 6]);
    }

    #[tokio::test]
    async fn test_fn_handler_empty_input() {
        let handler = FnHandler::new(|_repo_id: RepoId, input: Vec<u8>| async move { Ok(input) });

        let repo_id = RepoId::new();
        let result = handler.upload_pack(repo_id, vec![]).await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn test_fn_handler_new() {
        let handler = FnHandler::new(|_repo_id: RepoId, _input: Vec<u8>| async move {
            Ok::<Vec<u8>, gitforce_common::Error>(vec![])
        });
        assert!(matches!(handler, FnHandler { .. }));
    }
}

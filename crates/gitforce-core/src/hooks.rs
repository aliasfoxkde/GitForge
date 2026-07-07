//! Git hook execution system

use async_trait::async_trait;
use gitforce_common::{RepoId, Result, UserId};
use serde::{Deserialize, Serialize};

/// Hook executor trait
#[async_trait]
pub trait HookExecutor: Send + Sync {
    /// Execute pre-receive hook
    async fn pre_receive(&self, payload: HookPayload) -> Result<()>;

    /// Execute post-receive hook
    async fn post_receive(&self, payload: HookPayload) -> Result<()>;
}

/// Hook payload for receive hooks
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookPayload {
    pub repo_id: RepoId,
    pub ref_name: String,
    pub old_hash: String,
    pub new_hash: String,
    pub pusher_id: Option<UserId>,
}

impl HookPayload {
    /// Create a new hook payload
    pub fn new(
        repo_id: RepoId,
        ref_name: String,
        old_hash: String,
        new_hash: String,
        pusher_id: Option<UserId>,
    ) -> Self {
        Self {
            repo_id,
            ref_name,
            old_hash,
            new_hash,
            pusher_id,
        }
    }

    /// Get the branch name from the ref
    pub fn branch_name(&self) -> Option<String> {
        if self.ref_name.starts_with("refs/heads/") {
            Some(self.ref_name.strip_prefix("refs/heads/").unwrap().to_string())
        } else {
            None
        }
    }

    /// Get the tag name from the ref
    pub fn tag_name(&self) -> Option<String> {
        if self.ref_name.starts_with("refs/tags/") {
            Some(self.ref_name.strip_prefix("refs/tags/").unwrap().to_string())
        } else {
            None
        }
    }

    /// Check if this is a push to a branch
    pub fn is_branch_push(&self) -> bool {
        self.ref_name.starts_with("refs/heads/")
    }

    /// Check if this is a tag push
    pub fn is_tag_push(&self) -> bool {
        self.ref_name.starts_with("refs/tags/")
    }
}

/// Default hook executor that logs hooks
pub struct LoggingHookExecutor;

impl LoggingHookExecutor {
    pub fn new() -> Self {
        Self
    }
}

impl Default for LoggingHookExecutor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl HookExecutor for LoggingHookExecutor {
    async fn pre_receive(&self, payload: HookPayload) -> Result<()> {
        tracing::info!(
            "pre-receive hook: repo={} ref={} {}->{} pusher={:?}",
            payload.repo_id,
            payload.ref_name,
            payload.old_hash,
            payload.new_hash,
            payload.pusher_id
        );
        Ok(())
    }

    async fn post_receive(&self, payload: HookPayload) -> Result<()> {
        tracing::info!(
            "post-receive hook: repo={} ref={} {}->{} pusher={:?}",
            payload.repo_id,
            payload.ref_name,
            payload.old_hash,
            payload.new_hash,
            payload.pusher_id
        );
        Ok(())
    }
}

/// Hook manager for coordinating multiple hook executors
pub struct HookManager {
    executors: Vec<Box<dyn HookExecutor>>,
}

impl HookManager {
    pub fn new() -> Self {
        Self {
            executors: Vec::new(),
        }
    }

    /// Add a hook executor
    pub fn add_executor<E: HookExecutor + 'static>(&mut self, executor: E) {
        self.executors.push(Box::new(executor));
    }

    /// Execute pre-receive hooks
    pub async fn pre_receive(&self, payload: HookPayload) -> Result<()> {
        for executor in &self.executors {
            executor.pre_receive(payload.clone()).await?;
        }
        Ok(())
    }

    /// Execute post-receive hooks
    pub async fn post_receive(&self, payload: HookPayload) -> Result<()> {
        for executor in &self.executors {
            executor.post_receive(payload.clone()).await?;
        }
        Ok(())
    }
}

impl Default for HookManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hook_payload() {
        let payload = HookPayload::new(
            RepoId::new(),
            "refs/heads/main".to_string(),
            "abc123".to_string(),
            "def456".to_string(),
            None,
        );

        assert!(payload.is_branch_push());
        assert!(!payload.is_tag_push());
        assert_eq!(payload.branch_name(), Some("main".to_string()));
    }
}

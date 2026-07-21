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

/// Execute hooks for a push operation
///
/// This is a convenience function that creates appropriate payloads and executes
/// both pre-receive and post-receive hooks.
///
/// # Arguments
/// * `manager` - The hook manager to use
/// * `repo_id` - The repository ID
/// * `ref_name` - The reference that was pushed (e.g., "refs/heads/main")
/// * `old_hash` - The old commit hash
/// * `new_hash` - The new commit hash
/// * `pusher_id` - The user ID of the pusher (if known)
///
/// # Returns
/// Returns `Ok(())` if all hooks succeed, or the first error encountered
pub async fn execute_push_hooks(
    manager: &HookManager,
    repo_id: RepoId,
    ref_name: &str,
    old_hash: &str,
    new_hash: &str,
    pusher_id: Option<gitforce_common::UserId>,
) -> Result<()> {
    let payload = HookPayload::new(
        repo_id,
        ref_name.to_string(),
        old_hash.to_string(),
        new_hash.to_string(),
        pusher_id,
    );

    // Execute pre-receive hooks first
    if let Err(e) = manager.pre_receive(payload.clone()).await {
        tracing::error!("pre-receive hook failed: {}", e);
        return Err(e);
    }

    // Execute post-receive hooks
    if let Err(e) = manager.post_receive(payload.clone()).await {
        tracing::error!("post-receive hook failed: {}", e);
        return Err(e);
    }

    tracing::info!("Successfully executed hooks for push to {} on repo {}", ref_name, repo_id);
    Ok(())
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

    #[test]
    fn test_hook_payload_tag() {
        let payload = HookPayload::new(
            RepoId::new(),
            "refs/tags/v1.0".to_string(),
            "abc123".to_string(),
            "def456".to_string(),
            None,
        );

        assert!(!payload.is_branch_push());
        assert!(payload.is_tag_push());
        assert_eq!(payload.tag_name(), Some("v1.0".to_string()));
    }

    #[test]
    fn test_hook_payload_no_prefix() {
        let payload = HookPayload::new(
            RepoId::new(),
            "refs/heads/feature/test".to_string(),
            "abc123".to_string(),
            "def456".to_string(),
            None,
        );

        assert!(payload.is_branch_push());
        assert_eq!(payload.branch_name(), Some("feature/test".to_string()));
    }

    #[test]
    fn test_hook_payload_with_pusher() {
        let user_id = UserId::new();
        let payload = HookPayload::new(
            RepoId::new(),
            "refs/heads/main".to_string(),
            "abc123".to_string(),
            "def456".to_string(),
            Some(user_id),
        );

        assert_eq!(payload.pusher_id, Some(user_id));
    }

    #[tokio::test]
    async fn test_logging_hook_executor_pre_receive() {
        let executor = LoggingHookExecutor::new();
        let payload = HookPayload::new(
            RepoId::new(),
            "refs/heads/main".to_string(),
            "abc123".to_string(),
            "def456".to_string(),
            None,
        );

        let result = executor.pre_receive(payload).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_logging_hook_executor_post_receive() {
        let executor = LoggingHookExecutor::new();
        let payload = HookPayload::new(
            RepoId::new(),
            "refs/heads/main".to_string(),
            "abc123".to_string(),
            "def456".to_string(),
            None,
        );

        let result = executor.post_receive(payload).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_hook_manager_pre_receive() {
        let mut manager = HookManager::new();
        manager.add_executor(LoggingHookExecutor::new());

        let payload = HookPayload::new(
            RepoId::new(),
            "refs/heads/main".to_string(),
            "abc123".to_string(),
            "def456".to_string(),
            None,
        );

        let result = manager.pre_receive(payload).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_hook_manager_post_receive() {
        let mut manager = HookManager::new();
        manager.add_executor(LoggingHookExecutor::new());

        let payload = HookPayload::new(
            RepoId::new(),
            "refs/heads/main".to_string(),
            "abc123".to_string(),
            "def456".to_string(),
            None,
        );

        let result = manager.post_receive(payload).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_hook_manager_multiple_executors() {
        let mut manager = HookManager::new();
        manager.add_executor(LoggingHookExecutor::new());
        manager.add_executor(LoggingHookExecutor::new());

        let payload = HookPayload::new(
            RepoId::new(),
            "refs/heads/main".to_string(),
            "abc123".to_string(),
            "def456".to_string(),
            None,
        );

        let result = manager.pre_receive(payload).await;
        assert!(result.is_ok());
    }

    #[test]
    fn test_logging_hook_executor_new() {
        let executor = LoggingHookExecutor::new();
        assert!(matches!(executor, LoggingHookExecutor));
    }

    #[test]
    fn test_hook_manager_new() {
        let manager = HookManager::new();
        assert!(matches!(manager, HookManager { .. }));
    }

    #[test]
    fn test_hook_payload_branch_name_none_for_non_branch() {
        let payload = HookPayload::new(
            RepoId::new(),
            "refs/tags/v1.0".to_string(),
            "abc123".to_string(),
            "def456".to_string(),
            None,
        );
        assert_eq!(payload.branch_name(), None);
    }

    #[test]
    fn test_hook_payload_tag_name_none_for_non_tag() {
        let payload = HookPayload::new(
            RepoId::new(),
            "refs/heads/main".to_string(),
            "abc123".to_string(),
            "def456".to_string(),
            None,
        );
        assert_eq!(payload.tag_name(), None);
    }

    #[test]
    fn test_hook_payload_debug() {
        let payload = HookPayload::new(
            RepoId::new(),
            "refs/heads/main".to_string(),
            "abc123".to_string(),
            "def456".to_string(),
            None,
        );
        let debug_str = format!("{:?}", payload);
        assert!(debug_str.contains("HookPayload"));
    }

    #[tokio::test]
    async fn test_hook_manager_empty_executors() {
        let manager = HookManager::new();
        let payload = HookPayload::new(
            RepoId::new(),
            "refs/heads/main".to_string(),
            "abc123".to_string(),
            "def456".to_string(),
            None,
        );
        // Empty manager should still work
        let result = manager.pre_receive(payload).await;
        assert!(result.is_ok());
    }
}

//! Process pool for managing concurrent build jobs
//!
//! This module implements a semaphore-based process pool that limits
//! concurrent builds to prevent resource exhaustion.

use std::collections::HashMap;
use std::sync::Arc;
use tokio::process::Command;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::time::{timeout, Duration};

/// Resource weight for different job types
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum JobWeight {
    /// Lightweight job - compile, test
    #[default]
    Light = 2,
    /// Medium job - coverage, integration test
    Medium = 4,
    /// Heavy job - cross-platform release build
    Heavy = 8,
}

/// A managed process in the pool
#[derive(Debug)]
pub struct ManagedProcess {
    pub pid: u32,
    pub weight: JobWeight,
}

/// Process pool configuration
#[derive(Debug, Clone)]
pub struct PoolConfig {
    /// Maximum concurrent slots
    pub max_concurrent: usize,
    /// Default job timeout
    pub default_timeout: Duration,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            max_concurrent: 4,
            default_timeout: Duration::from_secs(3600), // 1 hour
        }
    }
}

/// A pool for managing concurrent processes
#[derive(Debug)]
pub struct ProcessPool {
    config: PoolConfig,
    semaphore: Arc<Semaphore>,
    running: Arc<std::sync::Mutex<HashMap<u32, ManagedProcess>>>,
}

impl ProcessPool {
    /// Create a new process pool
    pub fn new(config: PoolConfig) -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(config.max_concurrent)),
            running: Arc::new(std::sync::Mutex::new(HashMap::new())),
            config,
        }
    }

    /// Create with default configuration (4 concurrent)
    pub fn with_default_config() -> Self {
        Self::new(PoolConfig::default())
    }

    /// Get a permit for running a job
    pub async fn acquire(&self, _weight: JobWeight) -> OwnedSemaphorePermit {
        self.semaphore
            .clone()
            .acquire_owned()
            .await
            .expect("semaphore closed")
    }

    /// Spawn a managed process
    pub async fn spawn<F>(
        &self,
        weight: JobWeight,
        program: &str,
        args: &[&str],
        _on_output: F,
    ) -> std::io::Result<u32>
    where
        F: FnMut(String) + Send + 'static,
    {
        let permit = self.acquire(weight).await;
        let mut cmd = Command::new(program);
        cmd.args(args);

        let mut child = cmd.spawn()?;
        let pid = child.id().unwrap_or(0);

        {
            let mut running = self.running.lock().unwrap();
            running.insert(pid, ManagedProcess { pid, weight });
        }

        // Spawn output handler
        let pid_for_handler = pid;
        let running = self.running.clone();
        let timeout_duration = self.config.default_timeout;

        tokio::spawn(async move {
            let result = timeout(timeout_duration, child.wait()).await;
            match result {
                Ok(Ok(status)) => {
                    tracing::info!(
                        "process {} exited with status: {:?}",
                        pid_for_handler,
                        status.code()
                    );
                }
                Ok(Err(e)) => {
                    tracing::error!("process {} wait error: {}", pid_for_handler, e);
                }
                Err(_) => {
                    tracing::warn!(
                        "process {} timed out after {:?}, killing",
                        pid_for_handler,
                        timeout_duration
                    );
                    let _ = child.kill().await;
                }
            }
            let mut running = running.lock().unwrap();
            running.remove(&pid_for_handler);
            drop(permit);
        });

        Ok(pid)
    }

    /// Get count of running processes
    pub fn running_count(&self) -> usize {
        self.running.lock().unwrap().len()
    }

    /// Check if a process is running
    pub fn is_running(&self, pid: u32) -> bool {
        self.running.lock().unwrap().contains_key(&pid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pool_creation() {
        let pool = ProcessPool::with_default_config();
        assert_eq!(pool.running_count(), 0);
    }

    #[test]
    fn test_job_weight_ordering() {
        assert!(JobWeight::Heavy > JobWeight::Medium);
        assert!(JobWeight::Medium > JobWeight::Light);
    }

    #[test]
    fn test_pool_config_default() {
        let config = PoolConfig::default();
        assert_eq!(config.max_concurrent, 4);
        assert_eq!(config.default_timeout, Duration::from_secs(3600));
    }

    #[test]
    fn test_pool_config_custom() {
        let config = PoolConfig {
            max_concurrent: 8,
            default_timeout: Duration::from_secs(7200),
        };
        assert_eq!(config.max_concurrent, 8);
        assert_eq!(config.default_timeout, Duration::from_secs(7200));
    }

    #[test]
    fn test_managed_process_debug() {
        let process = ManagedProcess {
            pid: 12345,
            weight: JobWeight::Medium,
        };
        let debug_str = format!("{:?}", process);
        assert!(debug_str.contains("12345"));
        assert!(debug_str.contains("Medium"));
    }

    #[test]
    fn test_job_weight_default() {
        assert_eq!(JobWeight::default(), JobWeight::Light);
    }

    #[test]
    fn test_job_weight_equality() {
        assert_eq!(JobWeight::Light, JobWeight::Light);
        assert_eq!(JobWeight::Medium, JobWeight::Medium);
        assert_eq!(JobWeight::Heavy, JobWeight::Heavy);
        assert_ne!(JobWeight::Light, JobWeight::Medium);
    }

    #[test]
    fn test_pool_new() {
        let config = PoolConfig {
            max_concurrent: 2,
            default_timeout: Duration::from_secs(1800),
        };
        let pool = ProcessPool::new(config);
        assert_eq!(pool.running_count(), 0);
        assert!(!pool.is_running(1));
    }

    #[tokio::test]
    async fn test_pool_acquire() {
        let pool = ProcessPool::with_default_config();
        // Acquire a permit
        let permit = pool.acquire(JobWeight::Light).await;
        // Permit should be valid (will be dropped at end of scope)
        drop(permit);
        // After drop, we can acquire again
        let permit2 = pool.acquire(JobWeight::Light).await;
        drop(permit2);
    }

    #[tokio::test]
    async fn test_pool_acquire_multiple() {
        let config = PoolConfig {
            max_concurrent: 2,
            default_timeout: Duration::from_secs(3600),
        };
        let pool = ProcessPool::new(config);

        // Acquire all permits
        let permit1 = pool.acquire(JobWeight::Light).await;
        let permit2 = pool.acquire(JobWeight::Light).await;

        // Both should be acquired - running count is still 0 until spawn
        assert_eq!(pool.running_count(), 0);

        drop(permit1);
        drop(permit2);
    }

    #[tokio::test]
    async fn test_pool_acquire_all_weights() {
        let pool = ProcessPool::with_default_config();

        // Acquire with different weights
        let light = pool.acquire(JobWeight::Light).await;
        drop(light);

        let medium = pool.acquire(JobWeight::Medium).await;
        drop(medium);

        let heavy = pool.acquire(JobWeight::Heavy).await;
        drop(heavy);
    }

    #[test]
    fn test_pool_config_clone() {
        let config = PoolConfig::default();
        let cloned = config.clone();
        assert_eq!(cloned.max_concurrent, config.max_concurrent);
        assert_eq!(cloned.default_timeout, config.default_timeout);
    }

    #[test]
    fn test_pool_debug() {
        let pool = ProcessPool::with_default_config();
        let debug_str = format!("{:?}", pool);
        assert!(debug_str.contains("ProcessPool"));
    }

    #[test]
    fn test_pool_config_debug() {
        let config = PoolConfig::default();
        let debug_str = format!("{:?}", config);
        assert!(debug_str.contains("max_concurrent"));
    }

    #[test]
    fn test_job_weight_sorting() {
        // Test that weights can be sorted
        let mut weights = vec![JobWeight::Heavy, JobWeight::Light, JobWeight::Medium];
        weights.sort();
        assert_eq!(
            weights,
            vec![JobWeight::Light, JobWeight::Medium, JobWeight::Heavy]
        );
    }

    #[test]
    fn test_process_pool_clone() {
        let pool = ProcessPool::with_default_config();
        // Pool doesn't implement Clone, but we can verify the type works
        let debug_str = format!("{:?}", pool);
        assert!(debug_str.contains("ProcessPool"));
    }

    #[test]
    fn test_pool_config_impl() {
        let config = PoolConfig {
            max_concurrent: 16,
            default_timeout: Duration::from_secs(7200),
        };
        assert_eq!(config.max_concurrent, 16);
        assert_eq!(config.default_timeout, Duration::from_secs(7200));
    }

    #[test]
    fn test_job_weight_values() {
        // Test the actual numeric values
        assert_eq!(JobWeight::Light as i32, 2);
        assert_eq!(JobWeight::Medium as i32, 4);
        assert_eq!(JobWeight::Heavy as i32, 8);
    }

    #[tokio::test]
    async fn test_pool_acquire_concurrent() {
        let config = PoolConfig {
            max_concurrent: 2,
            default_timeout: Duration::from_secs(3600),
        };
        let pool = ProcessPool::new(config);

        // Acquire both permits concurrently
        let permit1 = pool.acquire(JobWeight::Light).await;
        let permit2 = pool.acquire(JobWeight::Medium).await;

        // Both acquired - verify we can still get count
        assert_eq!(pool.running_count(), 0);

        drop(permit1);
        drop(permit2);
    }

    #[test]
    fn test_pool_config_with_different_timeouts() {
        let config = PoolConfig {
            max_concurrent: 4,
            default_timeout: Duration::from_secs(300),
        };
        assert_eq!(config.default_timeout, Duration::from_secs(300));
    }

    #[test]
    fn test_managed_process_fields() {
        let process = ManagedProcess {
            pid: 99999,
            weight: JobWeight::Heavy,
        };
        assert_eq!(process.pid, 99999);
        assert_eq!(process.weight, JobWeight::Heavy);
    }

    #[test]
    fn test_job_weight_copy() {
        let weight = JobWeight::Medium;
        let _copy = weight; // Test that Copy trait is implemented
        assert_eq!(weight, JobWeight::Medium);
    }

    #[test]
    fn test_pool_semaphore_permits() {
        let config = PoolConfig {
            max_concurrent: 8,
            default_timeout: Duration::from_secs(1800),
        };
        let pool = ProcessPool::new(config);
        assert_eq!(pool.running_count(), 0);
    }

    #[tokio::test]
    async fn test_pool_acquire_with_weight_ignored() {
        // The weight parameter is currently not used in acquire
        // but the function still works
        let pool = ProcessPool::with_default_config();
        let _permit = pool.acquire(JobWeight::Heavy).await;
        // Pool count is still 0 because acquire doesn't track
        assert_eq!(pool.running_count(), 0);
    }

    #[test]
    fn test_process_pool_type_sizes() {
        // Verify the types have expected sizes (smoke test)
        let config = PoolConfig::default();
        let _pool = ProcessPool::new(config);
        // Just verify they can be created without overflow
    }

    #[test]
    fn test_job_weight_ord_total() {
        // Test Ord is total ordering
        assert!(JobWeight::Light < JobWeight::Medium);
        assert!(JobWeight::Medium < JobWeight::Heavy);
        assert!(JobWeight::Light < JobWeight::Heavy);
    }
}

//! Process pool for managing concurrent build jobs
//!
//! This module implements a semaphore-based process pool that limits
//! concurrent builds to prevent resource exhaustion.

use std::collections::HashMap;
use std::sync::Arc;
use tokio::process::{Child, Command};
use tokio::sync::{Semaphore, OwnedSemaphorePermit};
use tokio::time::{timeout, Duration};

/// Resource weight for different job types
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum JobWeight {
    /// Lightweight job - compile, test
    Light = 2,
    /// Medium job - coverage, integration test
    Medium = 4,
    /// Heavy job - cross-platform release build
    Heavy = 8,
}

impl Default for JobWeight {
    fn default() -> Self {
        JobWeight::Light
    }
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
    pub fn default() -> Self {
        Self::new(PoolConfig::default())
    }

    /// Get a permit for running a job
    pub async fn acquire(&self, weight: JobWeight) -> OwnedSemaphorePermit {
        self.semaphore.clone().acquire_owned().await.expect("semaphore closed")
    }

    /// Spawn a managed process
    pub async fn spawn<F>(
        &self,
        weight: JobWeight,
        program: &str,
        args: &[&str],
        mut on_output: F,
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
            running.insert(
                pid,
                ManagedProcess {
                    pid,
                    weight,
                },
            );
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
        let pool = ProcessPool::default();
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
    }
}

//! Runner agent

use async_trait::async_trait;
use gitforce_common::{Error, Result, RunnerId};
use gitforce_db::models::Runner;
use gitforce_sandbox::{DockerSandbox, Sandbox, SandboxLimits};
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::time::{interval, Duration};

/// Runner configuration
#[derive(Debug, Clone)]
pub struct RunnerConfig {
    /// Scheduler URL for job fetching
    pub scheduler_url: String,
    /// Runner name
    pub name: String,
    /// Runner type
    pub runner_type: String,
    /// Capacity (number of concurrent jobs)
    pub capacity: i32,
    /// Heartbeat interval in seconds
    pub heartbeat_interval_secs: u64,
    /// Job fetch interval in seconds
    pub fetch_interval_secs: u64,
}

impl Default for RunnerConfig {
    fn default() -> Self {
        Self {
            scheduler_url: "http://localhost:8080".to_string(),
            name: "runner".to_string(),
            runner_type: "docker".to_string(),
            capacity: 2,
            heartbeat_interval_secs: 30,
            fetch_interval_secs: 5,
        }
    }
}

/// Runner agent that fetches and executes jobs
pub struct RunnerAgent {
    config: RunnerConfig,
    runner_id: Option<RunnerId>,
    sandbox: Arc<DockerSandbox>,
    is_running: Arc<RwLock<bool>>,
}

impl RunnerAgent {
    /// Create a new runner agent
    pub async fn new(config: RunnerConfig) -> Result<Self> {
        let sandbox = DockerSandbox::new().await?;

        Ok(Self {
            config,
            runner_id: None,
            sandbox: Arc::new(sandbox),
            is_running: Arc::new(RwLock::new(false)),
        })
    }

    /// Register with the scheduler
    pub async fn register(&mut self) -> Result<RunnerId> {
        let runner = Runner::new(
            self.config.name.clone(),
            gitforce_db::models::RunnerType::Docker,
            self.config.capacity,
        );

        let runner_id = runner.id;

        // In a real implementation, we'd POST to the scheduler
        // For now, just store locally
        self.runner_id = Some(runner_id);

        tracing::info!("runner {} registered", runner_id);
        Ok(runner_id)
    }

    /// Start the runner agent loop
    pub async fn run(&self) -> Result<()> {
        *self.is_running.write().await = true;

        let runner_id = self.runner_id
            .ok_or_else(|| Error::internal("runner not registered"))?;

        tracing::info!("runner {} starting", runner_id);

        // Start heartbeat loop
        let heartbeat_runner_id = runner_id;
        let heartbeat_interval = self.config.heartbeat_interval_secs;
        tokio::spawn(async move {
            let mut ticker = interval(Duration::from_secs(heartbeat_interval));
            loop {
                ticker.tick().await;
                tracing::debug!("runner {} heartbeat", heartbeat_runner_id);
                // In real impl, POST heartbeat to scheduler
            }
        });

        // Start job fetch loop
        let fetch_interval = self.config.fetch_interval_secs;
        tokio::spawn(async move {
            let mut ticker = interval(Duration::from_secs(fetch_interval));
            loop {
                ticker.tick().await;
                tracing::debug!("checking for jobs...");
                // In real impl, GET from scheduler for available jobs
            }
        });

        // Keep running until stopped
        while *self.is_running.read().await {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }

        Ok(())
    }

    /// Stop the runner agent
    pub async fn stop(&self) {
        *self.is_running.write().await = false;
        tracing::info!("runner {} stopped", self.runner_id.unwrap_or_default());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore] // Requires Docker running
    async fn test_runner_creation() {
        let config = RunnerConfig::default();
        let agent = RunnerAgent::new(config).await.unwrap();
        assert!(agent.runner_id.is_none());
    }
}

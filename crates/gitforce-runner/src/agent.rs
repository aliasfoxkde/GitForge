//! Runner agent

use gitforce_common::{Error, Result, RunnerId};
use gitforce_db::models::Runner;
use gitforce_sandbox::{DockerSandbox};
use reqwest::Client;
use serde::{Deserialize, Serialize};
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
            scheduler_url: "http://localhost:8081".to_string(),
            name: "runner".to_string(),
            runner_type: "docker".to_string(),
            capacity: 2,
            heartbeat_interval_secs: 30,
            fetch_interval_secs: 5,
        }
    }
}

/// Job assignment from scheduler
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobAssignment {
    /// Job ID
    pub job_id: String,
    /// Job name
    pub name: String,
    /// Pipeline run ID
    pub pipeline_run_id: String,
    /// Commands to execute
    pub commands: Vec<String>,
    /// Working directory
    pub working_dir: Option<String>,
}

/// Runner agent that fetches and executes jobs
pub struct RunnerAgent {
    config: RunnerConfig,
    client: Client,
    runner: Option<Runner>,
    #[allow(dead_code)]
    sandbox: Arc<DockerSandbox>,
    is_running: Arc<RwLock<bool>>,
}

impl RunnerAgent {
    /// Create a new runner agent
    pub async fn new(config: RunnerConfig) -> Result<Self> {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| Error::internal(format!("failed to create HTTP client: {}", e)))?;

        let sandbox = DockerSandbox::new().await?;

        Ok(Self {
            config,
            client,
            runner: None,
            sandbox: Arc::new(sandbox),
            is_running: Arc::new(RwLock::new(false)),
        })
    }

    /// Register with the scheduler via HTTP
    pub async fn register(&mut self) -> Result<RunnerId> {
        let runner = Runner::new(
            self.config.name.clone(),
            gitforce_db::models::RunnerType::Docker,
            self.config.capacity,
        );

        // Try to register with scheduler via HTTP
        let register_url = format!("{}/runners", self.config.scheduler_url);
        let request = serde_json::json!({
            "name": runner.name,
            "type": runner.runner_type,
            "capacity": runner.capacity,
        });

        match self.client.post(&register_url).json(&request).send().await {
            Ok(response) => {
                if response.status().is_success() {
                    tracing::info!("registered runner {} with scheduler", runner.id);
                } else {
                    tracing::warn!(
                        "scheduler returned {} for registration, running in standalone mode",
                        response.status()
                    );
                }
            }
            Err(e) => {
                tracing::warn!(
                    "failed to register with scheduler: {}. Running in standalone mode.",
                    e
                );
            }
        }

        self.runner = Some(runner.clone());
        Ok(runner.id)
    }

    /// Start the runner agent loop
    pub async fn run(&self) -> Result<()> {
        *self.is_running.write().await = true;

        let runner = self.runner
            .as_ref()
            .ok_or_else(|| Error::internal("runner not registered"))?;

        let runner_id = runner.id;
        tracing::info!("runner {} starting", runner_id);

        // Start heartbeat loop
        let heartbeat_runner_id = runner_id;
        let heartbeat_interval = self.config.heartbeat_interval_secs;
        let heartbeat_client = self.client.clone();
        let heartbeat_url = self.config.scheduler_url.clone();
        tokio::spawn(async move {
            let mut ticker = interval(Duration::from_secs(heartbeat_interval));
            loop {
                ticker.tick().await;
                tracing::debug!("runner {} sending heartbeat", heartbeat_runner_id);
                let url = format!("{}/runners/{}/heartbeat", heartbeat_url, heartbeat_runner_id);
                if let Err(e) = heartbeat_client.post(&url).send().await {
                    tracing::trace!("heartbeat failed: {}", e);
                }
            }
        });

        // Start job fetch loop
        let fetch_interval = self.config.fetch_interval_secs;
        let fetch_client = self.client.clone();
        let fetch_url = self.config.scheduler_url.clone();
        tokio::spawn(async move {
            let mut ticker = interval(Duration::from_secs(fetch_interval));
            loop {
                ticker.tick().await;
                tracing::debug!("runner checking for jobs...");

                let jobs_url = format!("{}/jobs/pending", fetch_url);
                match fetch_client.get(&jobs_url).send().await {
                    Ok(response) => {
                        if response.status().is_success() {
                            if let Ok(jobs) = response.json::<Vec<JobAssignment>>().await {
                                for job in jobs {
                                    tracing::info!(
                                        "received job assignment: {} ({})",
                                        job.name, job.job_id
                                    );
                                    // Job execution would happen here
                                }
                            }
                        }
                    }
                    Err(e) => {
                        tracing::trace!("job fetch failed: {}", e);
                    }
                }
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
        let runner_id = self.runner.as_ref().map(|r| r.id.to_string()).unwrap_or_default();
        tracing::info!("runner {} stopped", runner_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_runner_creation() {
        let config = RunnerConfig::default();
        let agent = RunnerAgent::new(config).await.unwrap();
        assert!(agent.runner.is_none());
    }

    #[tokio::test]
    async fn test_runner_config_default() {
        let config = RunnerConfig::default();
        assert_eq!(config.name, "runner");
        assert_eq!(config.capacity, 2);
        assert_eq!(config.heartbeat_interval_secs, 30);
        assert_eq!(config.fetch_interval_secs, 5);
    }

    #[tokio::test]
    async fn test_runner_register_no_scheduler() {
        // Test that register doesn't panic when scheduler is unavailable
        let mut config = RunnerConfig::default();
        config.scheduler_url = "http://localhost:99999".to_string(); // Invalid URL
        let mut agent = RunnerAgent::new(config).await.unwrap();
        let result = agent.register().await;
        assert!(result.is_ok());
        assert!(agent.runner.is_some());
    }
}

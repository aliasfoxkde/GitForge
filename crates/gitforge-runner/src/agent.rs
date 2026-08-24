//! Runner agent

use crate::executor::{ExecutableJob, JobExecutor, JobStep};
use gitforge_common::{Error, JobId, Result, RunnerId};
use gitforge_db::models::Runner;
use gitforge_sandbox::DockerSandbox;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::env;
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error as ThisError;
use tokio::sync::RwLock;
use tokio::time::interval;

/// Configuration seam errors with actionable messages
#[derive(ThisError, Debug)]
pub enum ConfigError {
    #[error("GITFORGE_SCHEDULER_URL is empty. Set a valid URL (e.g., http://localhost:42781)")]
    EmptySchedulerUrl,

    #[error("GITFORGE_RUNNER_NAME is empty. Set a non-empty runner name via GITFORGE_RUNNER_NAME")]
    EmptyRunnerName,

    #[error("GITFORGE_CAPACITY must be a positive integer. Got '{value}'. Set GITFORGE_CAPACITY to a value between 1 and 100")]
    InvalidCapacity { value: String },

    #[error("GITFORGE_HEARTBEAT_INTERVAL must be a positive integer (seconds). Got '{value}'. Set GITFORGE_HEARTBEAT_INTERVAL to a value between 1 and 3600")]
    InvalidHeartbeatInterval { value: String },

    #[error("GITFORGE_FETCH_INTERVAL must be a positive integer (seconds). Got '{value}'. Set GITFORGE_FETCH_INTERVAL to a value between 1 and 3600")]
    InvalidFetchInterval { value: String },
}

/// Environment variable names for runner configuration
mod env_vars {
    /// Scheduler URL environment variable
    pub const SCHEDULER_URL: &str = "GITFORGE_SCHEDULER_URL";
    /// Runner name environment variable
    pub const RUNNER_NAME: &str = "GITFORGE_RUNNER_NAME";
    /// Runner type environment variable
    pub const RUNNER_TYPE: &str = "GITFORGE_RUNNER_TYPE";
    /// Runner capacity environment variable
    pub const CAPACITY: &str = "GITFORGE_CAPACITY";
    /// Heartbeat interval environment variable (seconds)
    pub const HEARTBEAT_INTERVAL: &str = "GITFORGE_HEARTBEAT_INTERVAL";
    /// Fetch interval environment variable (seconds)
    pub const FETCH_INTERVAL: &str = "GITFORGE_FETCH_INTERVAL";
}

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
            scheduler_url: "http://localhost:42781".to_string(),
            name: "runner".to_string(),
            runner_type: "docker".to_string(),
            capacity: 2,
            heartbeat_interval_secs: 30,
            fetch_interval_secs: 5,
        }
    }
}

impl RunnerConfig {
    /// Build RunnerConfig from environment variables with validation.
    ///
    /// Environment variables (all optional, with sensible defaults):
    /// - GITFORGE_SCHEDULER_URL: Scheduler URL (default: http://localhost:42781)
    /// - GITFORGE_RUNNER_NAME: Runner name (default: runner)
    /// - GITFORGE_RUNNER_TYPE: Runner type (default: docker)
    /// - GITFORGE_CAPACITY: Concurrent job capacity (default: 2, range: 1-100)
    /// - GITFORGE_HEARTBEAT_INTERVAL: Heartbeat interval in seconds (default: 30, range: 1-3600)
    /// - GITFORGE_FETCH_INTERVAL: Job fetch interval in seconds (default: 5, range: 1-3600)
    ///
    /// # Errors
    ///
    /// Returns an actionable ConfigError if:
    /// - GITFORGE_SCHEDULER_URL is set but empty
    /// - GITFORGE_RUNNER_NAME is set but empty
    /// - GITFORGE_CAPACITY is set but not a valid positive integer (1-100)
    /// - GITFORGE_HEARTBEAT_INTERVAL is set but not a valid positive integer (1-3600)
    /// - GITFORGE_FETCH_INTERVAL is set but not a valid positive integer (1-3600)
    pub fn from_env() -> std::result::Result<Self, ConfigError> {
        // Parse string values
        let scheduler_url = match env::var(env_vars::SCHEDULER_URL) {
            Ok(v) if v.trim().is_empty() => return Err(ConfigError::EmptySchedulerUrl),
            Ok(v) => v,
            Err(_) => "http://localhost:42781".to_string(),
        };

        let name = match env::var(env_vars::RUNNER_NAME) {
            Ok(v) if v.trim().is_empty() => return Err(ConfigError::EmptyRunnerName),
            Ok(v) => v,
            Err(_) => "runner".to_string(),
        };

        let runner_type = env::var(env_vars::RUNNER_TYPE).unwrap_or_else(|_| "docker".to_string());

        // Parse numeric values with validation
        let capacity = Self::parse_i32_env(env_vars::CAPACITY, 1, 100, || 2)?;

        let heartbeat_interval_secs =
            Self::parse_u64_env(env_vars::HEARTBEAT_INTERVAL, 1, 3600, || 30)?;

        let fetch_interval_secs = Self::parse_u64_env(env_vars::FETCH_INTERVAL, 1, 3600, || 5)?;

        Ok(Self {
            scheduler_url,
            name,
            runner_type,
            capacity,
            heartbeat_interval_secs,
            fetch_interval_secs,
        })
    }

    /// Parse an optional i32 environment variable with range validation.
    fn parse_i32_env(
        name: &str,
        min: i32,
        max: i32,
        default: impl FnOnce() -> i32,
    ) -> std::result::Result<i32, ConfigError> {
        match env::var(name) {
            Ok(v) => {
                let parsed = v
                    .parse::<i32>()
                    .map_err(|_| ConfigError::InvalidCapacity { value: v.clone() })?;
                if !(min..=max).contains(&parsed) {
                    return Err(ConfigError::InvalidCapacity { value: v });
                }
                Ok(parsed)
            }
            Err(_) => Ok(default()),
        }
    }

    /// Parse an optional u64 environment variable with range validation.
    fn parse_u64_env(
        name: &str,
        min: u64,
        max: u64,
        default: impl FnOnce() -> u64,
    ) -> std::result::Result<u64, ConfigError> {
        match env::var(name) {
            Ok(v) => {
                let parsed = v.parse::<u64>().map_err(|_| match name {
                    env_vars::HEARTBEAT_INTERVAL => {
                        ConfigError::InvalidHeartbeatInterval { value: v.clone() }
                    }
                    env_vars::FETCH_INTERVAL => {
                        ConfigError::InvalidFetchInterval { value: v.clone() }
                    }
                    _ => ConfigError::InvalidCapacity { value: v.clone() },
                })?;
                if !(min..=max).contains(&parsed) {
                    return Err(match name {
                        env_vars::HEARTBEAT_INTERVAL => {
                            ConfigError::InvalidHeartbeatInterval { value: v }
                        }
                        env_vars::FETCH_INTERVAL => ConfigError::InvalidFetchInterval { value: v },
                        _ => ConfigError::InvalidCapacity { value: v },
                    });
                }
                Ok(parsed)
            }
            Err(_) => Ok(default()),
        }
    }

    /// Get scheduler URL, checking environment first.
    pub fn scheduler_url(&self) -> &str {
        &self.scheduler_url
    }

    /// Get runner name, checking environment first.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Get runner type, checking environment first.
    pub fn runner_type(&self) -> &str {
        &self.runner_type
    }

    /// Get capacity, checking environment first.
    pub fn capacity(&self) -> i32 {
        self.capacity
    }

    /// Get heartbeat interval in seconds, checking environment first.
    pub fn heartbeat_interval_secs(&self) -> u64 {
        self.heartbeat_interval_secs
    }

    /// Get fetch interval in seconds, checking environment first.
    pub fn fetch_interval_secs(&self) -> u64 {
        self.fetch_interval_secs
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
#[derive(Clone)]
pub struct RunnerAgent {
    config: RunnerConfig,
    client: Client,
    runner: Option<Runner>,
    #[allow(dead_code)]
    sandbox: Arc<DockerSandbox>,
    executor: Arc<JobExecutor>,
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
        let executor = JobExecutor::new().await?;

        Ok(Self {
            config,
            client,
            runner: None,
            sandbox: Arc::new(sandbox),
            executor: Arc::new(executor),
            is_running: Arc::new(RwLock::new(false)),
        })
    }

    /// Register with the scheduler via HTTP
    pub async fn register(&mut self) -> Result<RunnerId> {
        let mut runner = Runner::new(
            self.config.name.clone(),
            gitforge_db::models::RunnerType::Docker,
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
                    match response.json::<serde_json::Value>().await {
                        Ok(body) => {
                            if let Some(server_id) = body
                                .get("id")
                                .and_then(|value| value.as_str())
                                .and_then(|value| uuid::Uuid::parse_str(value).ok())
                            {
                                runner.id = RunnerId::from(server_id);
                            } else {
                                tracing::warn!(
                                    "scheduler registration response did not contain a valid runner ID"
                                );
                            }
                        }
                        Err(error) => {
                            tracing::warn!(
                                "failed to decode scheduler registration response: {}",
                                error
                            );
                        }
                    }
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

        let runner = self
            .runner
            .as_ref()
            .ok_or_else(|| Error::internal("runner not registered"))?;

        let runner_id = runner.id;
        tracing::info!("runner {} starting", runner_id);

        // Start heartbeat loop
        let heartbeat_runner_id = runner_id;
        let heartbeat_interval = self.config.heartbeat_interval_secs;
        let heartbeat_client = self.client.clone();
        let heartbeat_url = self.config.scheduler_url.clone();
        let is_running = self.is_running.clone();
        tokio::spawn(async move {
            let mut ticker = interval(Duration::from_secs(heartbeat_interval));
            loop {
                ticker.tick().await;
                if !*is_running.read().await {
                    tracing::debug!("heartbeat loop stopping");
                    break;
                }
                tracing::debug!("runner {} sending heartbeat", heartbeat_runner_id);
                let url = format!(
                    "{}/runners/{}/heartbeat",
                    heartbeat_url, heartbeat_runner_id
                );
                if let Err(e) = heartbeat_client.post(&url).send().await {
                    tracing::trace!("heartbeat failed: {}", e);
                }
            }
        });

        // Start job fetch loop
        let fetch_interval = self.config.fetch_interval_secs;
        let fetch_client = self.client.clone();
        let fetch_url = self.config.scheduler_url.clone();
        let is_running = self.is_running.clone();
        let executor = self.executor.clone();
        tokio::spawn(async move {
            let mut ticker = interval(Duration::from_secs(fetch_interval));
            loop {
                ticker.tick().await;
                if !*is_running.read().await {
                    tracing::debug!("job fetch loop stopping");
                    break;
                }
                tracing::debug!("runner checking for jobs...");

                let jobs_url = format!("{}/jobs/pending", fetch_url);
                match fetch_client.get(&jobs_url).send().await {
                    Ok(response) => {
                        if response.status().is_success() {
                            if let Ok(jobs) = response.json::<Vec<JobAssignment>>().await {
                                for job in jobs {
                                    tracing::info!(
                                        "received job assignment: {} ({})",
                                        job.name,
                                        job.job_id
                                    );
                                    // Execute the job
                                    Self::execute_job(&executor, &job, &fetch_client, &fetch_url)
                                        .await;
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
        let runner_id = self
            .runner
            .as_ref()
            .map(|r| r.id.to_string())
            .unwrap_or_default();
        tracing::info!("runner {} stopped", runner_id);
    }

    /// Check if agent is running
    pub async fn is_running(&self) -> bool {
        *self.is_running.read().await
    }

    /// Execute a job assignment
    async fn execute_job(
        executor: &Arc<JobExecutor>,
        assignment: &JobAssignment,
        client: &Client,
        scheduler_url: &str,
    ) {
        let job_id = match uuid::Uuid::parse_str(&assignment.job_id) {
            Ok(id) => JobId::from(id),
            Err(_) => {
                tracing::error!("invalid job_id: {}", assignment.job_id);
                return;
            }
        };

        // Convert assignment to ExecutableJob
        let executable = ExecutableJob {
            job_id,
            image: "rust:latest".to_string(), // Default image - would come from job config
            steps: assignment
                .commands
                .iter()
                .map(|cmd| JobStep {
                    name: "run".to_string(),
                    run: cmd.clone(),
                    env: None,
                    working_directory: assignment.working_dir.clone(),
                })
                .collect(),
            env: std::collections::HashMap::new(),
            working_dir: assignment.working_dir.clone(),
            timeout_secs: 3600,
        };

        tracing::info!("executing job {} in container", assignment.job_id);

        // Execute the job
        let result = executor.execute(executable).await;

        tracing::info!(
            "job {} completed: success={}, exit_code={}",
            assignment.job_id,
            result.success,
            result.exit_code
        );

        // Report completion to scheduler with full results
        let complete_url = format!("{}/jobs/{}/complete", scheduler_url, assignment.job_id);

        // Build step results for reporting
        let step_results_json: Vec<serde_json::Value> = result
            .step_results
            .iter()
            .map(|sr| {
                serde_json::json!({
                    "exit_code": sr.exit_code,
                    "stdout": sr.stdout,
                    "stderr": sr.stderr,
                })
            })
            .collect();

        let complete_request = serde_json::json!({
            "success": result.success,
            "exit_code": result.exit_code,
            "error": result.error,
            "step_results": step_results_json,
        });

        if let Err(e) = client
            .post(&complete_url)
            .json(&complete_request)
            .send()
            .await
        {
            tracing::error!("failed to report job completion: {}", e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

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
        let config = RunnerConfig {
            scheduler_url: "http://localhost:99999".to_string(), // Invalid URL
            ..Default::default()
        };
        let mut agent = RunnerAgent::new(config).await.unwrap();
        let result = agent.register().await;
        assert!(result.is_ok());
        assert!(agent.runner.is_some());
    }

    #[tokio::test]
    async fn test_runner_agent_stop() {
        let config = RunnerConfig::default();
        let agent = RunnerAgent::new(config).await.unwrap();
        agent.stop().await;
        // No panic means success
    }

    #[test]
    fn test_runner_config_custom_values() {
        let config = RunnerConfig {
            scheduler_url: "http://custom:8081".to_string(),
            name: "custom-runner".to_string(),
            runner_type: "kubernetes".to_string(),
            capacity: 5,
            heartbeat_interval_secs: 60,
            fetch_interval_secs: 10,
        };
        assert_eq!(config.name, "custom-runner");
        assert_eq!(config.capacity, 5);
        assert_eq!(config.heartbeat_interval_secs, 60);
        assert_eq!(config.fetch_interval_secs, 10);
    }

    #[test]
    fn test_job_assignment_serialization() {
        let assignment = JobAssignment {
            job_id: "job-123".to_string(),
            name: "build".to_string(),
            pipeline_run_id: "run-456".to_string(),
            commands: vec!["cargo build".to_string(), "cargo test".to_string()],
            working_dir: Some("/workspace".to_string()),
        };

        let json = serde_json::to_string(&assignment).unwrap();
        let deserialized: JobAssignment = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.job_id, "job-123");
        assert_eq!(deserialized.commands.len(), 2);
    }

    #[test]
    fn test_job_assignment_without_working_dir() {
        let assignment = JobAssignment {
            job_id: "job-456".to_string(),
            name: "test".to_string(),
            pipeline_run_id: "run-789".to_string(),
            commands: vec!["cargo test".to_string()],
            working_dir: None,
        };
        assert!(assignment.working_dir.is_none());
    }

    #[test]
    fn test_runner_config_debug() {
        let config = RunnerConfig::default();
        let debug_str = format!("{:?}", config);
        assert!(debug_str.contains("runner"));
    }

    #[test]
    fn test_job_assignment_debug() {
        let assignment = JobAssignment {
            job_id: "job-123".to_string(),
            name: "build".to_string(),
            pipeline_run_id: "run-456".to_string(),
            commands: vec!["cargo build".to_string()],
            working_dir: None,
        };
        let debug_str = format!("{:?}", assignment);
        assert!(debug_str.contains("job-123"));
    }

    #[test]
    fn test_runner_config_clone() {
        let config = RunnerConfig::default();
        let cloned = config.clone();
        assert_eq!(cloned.name, config.name);
        assert_eq!(cloned.capacity, config.capacity);
    }

    #[test]
    fn test_runner_config_partial_eq() {
        let config1 = RunnerConfig::default();
        let config2 = RunnerConfig::default();
        assert_eq!(config1.name, config2.name);
        assert_eq!(config1.capacity, config2.capacity);
    }

    #[test]
    fn test_job_assignment_serde_roundtrip() {
        let assignment = JobAssignment {
            job_id: "job-123".to_string(),
            name: "build".to_string(),
            pipeline_run_id: "run-456".to_string(),
            commands: vec!["cargo build".to_string(), "cargo test".to_string()],
            working_dir: Some("/workspace".to_string()),
        };

        // Test JSON serialization
        let json = serde_json::to_string(&assignment).unwrap();
        assert!(json.contains("job-123"));
        assert!(json.contains("build"));

        // Test deserialization
        let deserialized: JobAssignment = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.job_id, assignment.job_id);
        assert_eq!(deserialized.name, assignment.name);
        assert_eq!(deserialized.commands, assignment.commands);
        assert_eq!(deserialized.working_dir, assignment.working_dir);
    }

    #[test]
    fn test_job_assignment_empty_commands() {
        let assignment = JobAssignment {
            job_id: "job-empty".to_string(),
            name: "noop".to_string(),
            pipeline_run_id: "run-001".to_string(),
            commands: vec![],
            working_dir: None,
        };
        assert!(assignment.commands.is_empty());
        assert!(assignment.working_dir.is_none());
    }

    #[test]
    fn test_job_assignment_multiple_commands() {
        let commands = vec![
            "cargo fetch".to_string(),
            "cargo build --release".to_string(),
            "cargo test".to_string(),
            "cargo clippy".to_string(),
        ];
        let assignment = JobAssignment {
            job_id: "job-multi".to_string(),
            name: "full-pipeline".to_string(),
            pipeline_run_id: "run-002".to_string(),
            commands,
            working_dir: Some("/project".to_string()),
        };
        assert_eq!(assignment.commands.len(), 4);
    }

    #[tokio::test]
    async fn test_runner_agent_not_registered() {
        // Agent without registration should have runner as None
        let config = RunnerConfig::default();
        let agent = RunnerAgent::new(config).await.unwrap();
        assert!(agent.runner.is_none());
    }

    #[tokio::test]
    async fn test_runner_register_sets_runner() {
        let config = RunnerConfig {
            scheduler_url: "http://localhost:99999".to_string(),
            ..Default::default()
        };
        let mut agent = RunnerAgent::new(config).await.unwrap();

        let result = agent.register().await;
        assert!(result.is_ok());
        assert!(agent.runner.is_some());

        // Verify runner has correct properties
        let runner = agent.runner.as_ref().unwrap();
        assert_eq!(runner.name, "runner");
        assert_eq!(runner.capacity, 2);
    }

    #[test]
    fn test_runner_config_all_fields() {
        let config = RunnerConfig {
            scheduler_url: "http://example.com:8081".to_string(),
            name: "test-runner".to_string(),
            runner_type: "firecracker".to_string(),
            capacity: 8,
            heartbeat_interval_secs: 15,
            fetch_interval_secs: 3,
        };

        assert_eq!(config.scheduler_url, "http://example.com:8081");
        assert_eq!(config.name, "test-runner");
        assert_eq!(config.runner_type, "firecracker");
        assert_eq!(config.capacity, 8);
        assert_eq!(config.heartbeat_interval_secs, 15);
        assert_eq!(config.fetch_interval_secs, 3);
    }

    #[test]
    fn test_runner_config_default_is_docker() {
        let config = RunnerConfig::default();
        assert_eq!(config.runner_type, "docker");
    }

    #[test]
    fn test_runner_config_default_heartbeat() {
        let config = RunnerConfig::default();
        // Default heartbeat is 30 seconds
        assert_eq!(config.heartbeat_interval_secs, 30);
        // Default fetch interval is 5 seconds
        assert_eq!(config.fetch_interval_secs, 5);
    }

    #[test]
    fn test_runner_agent_debug() {
        // We can't easily create a running agent for debug test
        // but we can verify the type implements Debug
        let config = RunnerConfig::default();
        assert!(format!("{:?}", config).contains("RunnerConfig"));
    }

    #[test]
    fn test_job_assignment_equality() {
        let assignment1 = JobAssignment {
            job_id: "job-1".to_string(),
            name: "build".to_string(),
            pipeline_run_id: "run-1".to_string(),
            commands: vec!["echo 1".to_string()],
            working_dir: None,
        };
        let assignment2 = JobAssignment {
            job_id: "job-1".to_string(),
            name: "build".to_string(),
            pipeline_run_id: "run-1".to_string(),
            commands: vec!["echo 1".to_string()],
            working_dir: None,
        };
        // JobAssignment should implement PartialEq if we add it
        // For now just verify individual field equality
        assert_eq!(assignment1.job_id, assignment2.job_id);
        assert_eq!(assignment1.name, assignment2.name);
    }

    #[test]
    fn test_job_assignment_serialize_with_minimal_fields() {
        let assignment = JobAssignment {
            job_id: "minimal-job".to_string(),
            name: "test".to_string(),
            pipeline_run_id: "run-min".to_string(),
            commands: vec!["true".to_string()],
            working_dir: None,
        };

        let json = serde_json::to_string(&assignment).unwrap();
        assert!(json.contains("minimal-job"));
        assert!(json.contains("test"));
        assert!(json.contains("minimal-job"));
    }

    #[tokio::test]
    async fn test_runner_stop_when_not_running() {
        let config = RunnerConfig::default();
        let agent = RunnerAgent::new(config).await.unwrap();
        // Stop without running should not panic
        agent.stop().await;
    }

    #[tokio::test]
    async fn test_runner_stop_after_registration() {
        let config = RunnerConfig {
            scheduler_url: "http://localhost:99999".to_string(),
            ..Default::default()
        };
        let mut agent = RunnerAgent::new(config).await.unwrap();
        agent.register().await.unwrap();
        // Stop after registration should not panic
        agent.stop().await;
    }

    #[test]
    fn test_runner_config_all_default_values() {
        let config = RunnerConfig::default();
        // Verify all default values
        assert_eq!(config.scheduler_url, "http://localhost:42781");
        assert_eq!(config.name, "runner");
        assert_eq!(config.runner_type, "docker");
        assert_eq!(config.capacity, 2);
        assert_eq!(config.heartbeat_interval_secs, 30);
        assert_eq!(config.fetch_interval_secs, 5);
    }

    #[test]
    fn test_runner_config_with_zero_capacity() {
        let config = RunnerConfig {
            scheduler_url: "http://localhost:42781".to_string(),
            name: "zero-cap".to_string(),
            runner_type: "docker".to_string(),
            capacity: 0,
            heartbeat_interval_secs: 30,
            fetch_interval_secs: 5,
        };
        assert_eq!(config.capacity, 0);
    }

    #[test]
    fn test_job_assignment_with_many_commands() {
        let commands: Vec<String> = (0..100).map(|i| format!("echo step{}", i)).collect();
        let assignment = JobAssignment {
            job_id: "job-many".to_string(),
            name: "many-steps".to_string(),
            pipeline_run_id: "run-many".to_string(),
            commands,
            working_dir: None,
        };
        assert_eq!(assignment.commands.len(), 100);
    }

    #[test]
    fn test_job_assignment_clone() {
        let assignment = JobAssignment {
            job_id: "clone-test".to_string(),
            name: "test".to_string(),
            pipeline_run_id: "run-1".to_string(),
            commands: vec!["echo clone".to_string()],
            working_dir: None,
        };
        let cloned = assignment.clone();
        assert_eq!(cloned.job_id, assignment.job_id);
        assert_eq!(cloned.commands, assignment.commands);
    }

    #[test]
    fn test_job_assignment_with_unicode_in_name() {
        let assignment = JobAssignment {
            job_id: "job-unicode".to_string(),
            name: "测试任务".to_string(),
            pipeline_run_id: "run-unicode".to_string(),
            commands: vec!["echo 测试".to_string()],
            working_dir: None,
        };
        assert_eq!(assignment.name, "测试任务");
    }

    #[test]
    fn test_job_assignment_with_special_chars_in_commands() {
        let assignment = JobAssignment {
            job_id: "special-cmd".to_string(),
            name: "special".to_string(),
            pipeline_run_id: "run-special".to_string(),
            commands: vec![
                "echo $HOME".to_string(),
                "echo \"quoted\"".to_string(),
                "echo 'single'".to_string(),
            ],
            working_dir: None,
        };
        assert_eq!(assignment.commands.len(), 3);
    }

    #[test]
    fn test_runner_config_with_special_url() {
        let config = RunnerConfig {
            scheduler_url: "http://user:pass@host:9090/path".to_string(),
            name: "special-url-runner".to_string(),
            runner_type: "docker".to_string(),
            capacity: 4,
            heartbeat_interval_secs: 45,
            fetch_interval_secs: 10,
        };
        assert!(config.scheduler_url.contains("user:pass"));
    }

    #[test]
    fn test_job_assignment_deserialize() {
        let json = r#"{
            "job_id": "deserialized-job",
            "name": "deserialized",
            "pipeline_run_id": "run-123",
            "commands": ["cargo build", "cargo test"],
            "working_dir": "/workspace"
        }"#;
        let assignment: JobAssignment = serde_json::from_str(json).unwrap();
        assert_eq!(assignment.job_id, "deserialized-job");
        assert_eq!(assignment.commands.len(), 2);
    }

    #[tokio::test]
    async fn test_runner_agent_with_custom_config() {
        let config = RunnerConfig {
            scheduler_url: "http://custom-scheduler:8081".to_string(),
            name: "custom-runner".to_string(),
            runner_type: "kubernetes".to_string(),
            capacity: 10,
            heartbeat_interval_secs: 60,
            fetch_interval_secs: 15,
        };
        let agent = RunnerAgent::new(config).await.unwrap();
        assert!(agent.runner.is_none());
    }

    #[test]
    fn test_runner_config_clone_is_independent() {
        let config1 = RunnerConfig::default();
        let mut config2 = config1.clone();
        config2.name = "modified".to_string();
        assert_ne!(config1.name, config2.name);
    }

    #[test]
    fn test_job_assignment_with_empty_working_dir() {
        let assignment = JobAssignment {
            job_id: "empty-wd".to_string(),
            name: "test".to_string(),
            pipeline_run_id: "run-1".to_string(),
            commands: vec!["echo test".to_string()],
            working_dir: Some("".to_string()),
        };
        assert!(assignment.working_dir.is_some());
    }

    #[tokio::test]
    async fn test_runner_is_running() {
        let config = RunnerConfig::default();
        let agent = RunnerAgent::new(config).await.unwrap();
        assert!(!agent.is_running().await);
    }

    #[tokio::test]
    async fn test_runner_run_and_stop() {
        let config = RunnerConfig {
            scheduler_url: "http://localhost:99999".to_string(),
            ..Default::default()
        };

        // Create and register agent
        let mut agent = RunnerAgent::new(config.clone()).await.unwrap();
        agent.register().await.unwrap();

        // Clone for use in spawn
        let agent_clone = agent.clone();

        // Start run in background
        let run_handle = tokio::spawn(async move { agent_clone.run().await });

        // Wait for start
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Check is_running
        assert!(agent.is_running().await);

        // Stop it
        agent.stop().await;

        // Give it time to shutdown
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        // Verify run completed
        let result = run_handle.await.unwrap();
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_runner_run_requires_registration() {
        let config = RunnerConfig::default();
        let agent = RunnerAgent::new(config).await.unwrap();

        // Agent is not registered, run should fail
        let result = agent.run().await;
        assert!(result.is_err());
    }

    // ========================================
    // Configuration Seam Tests
    // ========================================

    #[test]
    fn test_config_error_display() {
        let err = ConfigError::EmptySchedulerUrl;
        assert!(err.to_string().contains("GITFORGE_SCHEDULER_URL"));

        let err = ConfigError::EmptyRunnerName;
        assert!(err.to_string().contains("GITFORGE_RUNNER_NAME"));

        let err = ConfigError::InvalidCapacity {
            value: "bad".to_string(),
        };
        assert!(err.to_string().contains("GITFORGE_CAPACITY"));
        assert!(err.to_string().contains("bad"));

        let err = ConfigError::InvalidHeartbeatInterval {
            value: "0".to_string(),
        };
        assert!(err.to_string().contains("GITFORGE_HEARTBEAT_INTERVAL"));

        let err = ConfigError::InvalidFetchInterval {
            value: "-1".to_string(),
        };
        assert!(err.to_string().contains("GITFORGE_FETCH_INTERVAL"));
    }

    #[test]
    #[serial]
    fn test_config_from_env_no_vars() {
        // Clear any existing env vars
        env::remove_var(env_vars::SCHEDULER_URL);
        env::remove_var(env_vars::RUNNER_NAME);
        env::remove_var(env_vars::RUNNER_TYPE);
        env::remove_var(env_vars::CAPACITY);
        env::remove_var(env_vars::HEARTBEAT_INTERVAL);
        env::remove_var(env_vars::FETCH_INTERVAL);

        let config = RunnerConfig::from_env().unwrap();
        assert_eq!(config.scheduler_url, "http://localhost:42781");
        assert_eq!(config.name, "runner");
        assert_eq!(config.runner_type, "docker");
        assert_eq!(config.capacity, 2);
        assert_eq!(config.heartbeat_interval_secs, 30);
        assert_eq!(config.fetch_interval_secs, 5);
    }

    #[test]
    #[serial]
    fn test_config_from_env_with_all_vars() {
        env::set_var(env_vars::SCHEDULER_URL, "http://custom:8081");
        env::set_var(env_vars::RUNNER_NAME, "test-runner");
        env::set_var(env_vars::RUNNER_TYPE, "kubernetes");
        env::set_var(env_vars::CAPACITY, "5");
        env::set_var(env_vars::HEARTBEAT_INTERVAL, "60");
        env::set_var(env_vars::FETCH_INTERVAL, "10");

        let config = RunnerConfig::from_env().unwrap();
        assert_eq!(config.scheduler_url, "http://custom:8081");
        assert_eq!(config.name, "test-runner");
        assert_eq!(config.runner_type, "kubernetes");
        assert_eq!(config.capacity, 5);
        assert_eq!(config.heartbeat_interval_secs, 60);
        assert_eq!(config.fetch_interval_secs, 10);

        // Cleanup
        env::remove_var(env_vars::SCHEDULER_URL);
        env::remove_var(env_vars::RUNNER_NAME);
        env::remove_var(env_vars::RUNNER_TYPE);
        env::remove_var(env_vars::CAPACITY);
        env::remove_var(env_vars::HEARTBEAT_INTERVAL);
        env::remove_var(env_vars::FETCH_INTERVAL);
    }

    #[test]
    #[serial]
    fn test_config_from_env_partial_vars() {
        // Only set some vars
        env::set_var(env_vars::RUNNER_NAME, "partial-runner");
        env::set_var(env_vars::CAPACITY, "8");

        let config = RunnerConfig::from_env().unwrap();
        assert_eq!(config.name, "partial-runner");
        assert_eq!(config.capacity, 8);
        // Others should be defaults
        assert_eq!(config.scheduler_url, "http://localhost:42781");
        assert_eq!(config.runner_type, "docker");
        assert_eq!(config.heartbeat_interval_secs, 30);
        assert_eq!(config.fetch_interval_secs, 5);

        // Cleanup
        env::remove_var(env_vars::RUNNER_NAME);
        env::remove_var(env_vars::CAPACITY);
    }

    #[test]
    #[serial]
    fn test_config_capacity_boundaries() {
        // Test minimum valid
        env::set_var(env_vars::CAPACITY, "1");
        let config = RunnerConfig::from_env().unwrap();
        assert_eq!(config.capacity, 1);
        env::remove_var(env_vars::CAPACITY);

        // Test maximum valid
        env::set_var(env_vars::CAPACITY, "100");
        let config = RunnerConfig::from_env().unwrap();
        assert_eq!(config.capacity, 100);
        env::remove_var(env_vars::CAPACITY);
    }

    #[test]
    #[serial]
    fn test_config_heartbeat_boundaries() {
        // Test minimum valid
        env::set_var(env_vars::HEARTBEAT_INTERVAL, "1");
        let config = RunnerConfig::from_env().unwrap();
        assert_eq!(config.heartbeat_interval_secs, 1);
        env::remove_var(env_vars::HEARTBEAT_INTERVAL);

        // Test maximum valid
        env::set_var(env_vars::HEARTBEAT_INTERVAL, "3600");
        let config = RunnerConfig::from_env().unwrap();
        assert_eq!(config.heartbeat_interval_secs, 3600);
        env::remove_var(env_vars::HEARTBEAT_INTERVAL);
    }

    #[test]
    #[serial]
    fn test_config_fetch_boundaries() {
        // Test minimum valid
        env::set_var(env_vars::FETCH_INTERVAL, "1");
        let config = RunnerConfig::from_env().unwrap();
        assert_eq!(config.fetch_interval_secs, 1);
        env::remove_var(env_vars::FETCH_INTERVAL);

        // Test maximum valid
        env::set_var(env_vars::FETCH_INTERVAL, "3600");
        let config = RunnerConfig::from_env().unwrap();
        assert_eq!(config.fetch_interval_secs, 3600);
        env::remove_var(env_vars::FETCH_INTERVAL);
    }

    #[test]
    fn test_config_getter_methods() {
        let config = RunnerConfig {
            scheduler_url: "http://getter:9090".to_string(),
            name: "getter-test".to_string(),
            runner_type: "firecracker".to_string(),
            capacity: 7,
            heartbeat_interval_secs: 45,
            fetch_interval_secs: 15,
        };

        assert_eq!(config.scheduler_url(), "http://getter:9090");
        assert_eq!(config.name(), "getter-test");
        assert_eq!(config.runner_type(), "firecracker");
        assert_eq!(config.capacity(), 7);
        assert_eq!(config.heartbeat_interval_secs(), 45);
        assert_eq!(config.fetch_interval_secs(), 15);
    }

    #[test]
    #[serial]
    fn test_config_default_preserves_values() {
        // Verify default() produces expected values
        let config = RunnerConfig::default();
        assert_eq!(config.scheduler_url, "http://localhost:42781");
        assert_eq!(config.name, "runner");
        assert_eq!(config.runner_type, "docker");
        assert_eq!(config.capacity, 2);
        assert_eq!(config.heartbeat_interval_secs, 30);
        assert_eq!(config.fetch_interval_secs, 5);

        // Verify from_env() without vars matches default
        env::remove_var(env_vars::SCHEDULER_URL);
        env::remove_var(env_vars::RUNNER_NAME);
        env::remove_var(env_vars::RUNNER_TYPE);
        env::remove_var(env_vars::CAPACITY);
        env::remove_var(env_vars::HEARTBEAT_INTERVAL);
        env::remove_var(env_vars::FETCH_INTERVAL);

        let from_env = RunnerConfig::from_env().unwrap();
        assert_eq!(config.scheduler_url, from_env.scheduler_url);
        assert_eq!(config.name, from_env.name);
        assert_eq!(config.runner_type, from_env.runner_type);
        assert_eq!(config.capacity, from_env.capacity);
        assert_eq!(
            config.heartbeat_interval_secs,
            from_env.heartbeat_interval_secs
        );
        assert_eq!(config.fetch_interval_secs, from_env.fetch_interval_secs);
    }

    #[test]
    fn test_config_env_var_names() {
        assert_eq!(env_vars::SCHEDULER_URL, "GITFORGE_SCHEDULER_URL");
        assert_eq!(env_vars::RUNNER_NAME, "GITFORGE_RUNNER_NAME");
        assert_eq!(env_vars::RUNNER_TYPE, "GITFORGE_RUNNER_TYPE");
        assert_eq!(env_vars::CAPACITY, "GITFORGE_CAPACITY");
        assert_eq!(env_vars::HEARTBEAT_INTERVAL, "GITFORGE_HEARTBEAT_INTERVAL");
        assert_eq!(env_vars::FETCH_INTERVAL, "GITFORGE_FETCH_INTERVAL");
    }

    // ========================================
    // Invalid Value Tests
    // ========================================

    #[test]
    #[serial]
    fn test_config_empty_scheduler_url() {
        env::set_var(env_vars::SCHEDULER_URL, "");
        let result = RunnerConfig::from_env();
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConfigError::EmptySchedulerUrl
        ));
        env::remove_var(env_vars::SCHEDULER_URL);
    }

    #[test]
    #[serial]
    fn test_config_empty_runner_name() {
        env::set_var(env_vars::RUNNER_NAME, "   ");
        let result = RunnerConfig::from_env();
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ConfigError::EmptyRunnerName));
        env::remove_var(env_vars::RUNNER_NAME);
    }

    #[test]
    #[serial]
    fn test_config_invalid_capacity_non_numeric() {
        env::set_var(env_vars::CAPACITY, "abc");
        let result = RunnerConfig::from_env();
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, ConfigError::InvalidCapacity { .. }));
        assert!(err.to_string().contains("abc"));
        env::remove_var(env_vars::CAPACITY);
    }

    #[test]
    #[serial]
    fn test_config_invalid_capacity_zero() {
        env::set_var(env_vars::CAPACITY, "0");
        let result = RunnerConfig::from_env();
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, ConfigError::InvalidCapacity { .. }));
        assert!(err.to_string().contains("0"));
        env::remove_var(env_vars::CAPACITY);
    }

    #[test]
    #[serial]
    fn test_config_invalid_capacity_out_of_range() {
        env::set_var(env_vars::CAPACITY, "101");
        let result = RunnerConfig::from_env();
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, ConfigError::InvalidCapacity { .. }));
        assert!(err.to_string().contains("101"));
        env::remove_var(env_vars::CAPACITY);
    }

    #[test]
    #[serial]
    fn test_config_invalid_heartbeat_non_numeric() {
        env::set_var(env_vars::HEARTBEAT_INTERVAL, "xyz");
        let result = RunnerConfig::from_env();
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, ConfigError::InvalidHeartbeatInterval { .. }));
        assert!(err.to_string().contains("xyz"));
        env::remove_var(env_vars::HEARTBEAT_INTERVAL);
    }

    #[test]
    #[serial]
    fn test_config_invalid_heartbeat_out_of_range() {
        env::set_var(env_vars::HEARTBEAT_INTERVAL, "0");
        let result = RunnerConfig::from_env();
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, ConfigError::InvalidHeartbeatInterval { .. }));
        env::remove_var(env_vars::HEARTBEAT_INTERVAL);
    }

    #[test]
    #[serial]
    fn test_config_invalid_fetch_non_numeric() {
        env::set_var(env_vars::FETCH_INTERVAL, "nan");
        let result = RunnerConfig::from_env();
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, ConfigError::InvalidFetchInterval { .. }));
        assert!(err.to_string().contains("nan"));
        env::remove_var(env_vars::FETCH_INTERVAL);
    }

    #[test]
    #[serial]
    fn test_config_invalid_fetch_out_of_range() {
        env::set_var(env_vars::FETCH_INTERVAL, "99999");
        let result = RunnerConfig::from_env();
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, ConfigError::InvalidFetchInterval { .. }));
        env::remove_var(env_vars::FETCH_INTERVAL);
    }
}

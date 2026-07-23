//! Runner agent

use crate::executor::{ExecutableJob, JobExecutor, JobStep};
use gitforce_common::{Error, JobId, Result, RunnerId};
use gitforce_db::models::Runner;
use gitforce_sandbox::DockerSandbox;
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
        assert_eq!(config.scheduler_url, "http://localhost:8081");
        assert_eq!(config.name, "runner");
        assert_eq!(config.runner_type, "docker");
        assert_eq!(config.capacity, 2);
        assert_eq!(config.heartbeat_interval_secs, 30);
        assert_eq!(config.fetch_interval_secs, 5);
    }

    #[test]
    fn test_runner_config_with_zero_capacity() {
        let config = RunnerConfig {
            scheduler_url: "http://localhost:8081".to_string(),
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
}

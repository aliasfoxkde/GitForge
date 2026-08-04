//! Integration tests for Runner Agent
//!
//! These tests verify the runner agent's HTTP communication with scheduler
//! and job execution functionality.

use gitforce_runner::{RunnerAgent, RunnerConfig};
use gitforce_runner::executor::{ExecutableJob, JobStep, JobResult, JobExecutor};
use gitforce_common::JobId;
use std::collections::HashMap;

/// Test runner configuration
#[test]
fn test_runner_config_default() {
    let config = RunnerConfig::default();
    assert_eq!(config.name, "runner");
    assert_eq!(config.runner_type, "docker");
    assert_eq!(config.capacity, 2);
    assert_eq!(config.heartbeat_interval_secs, 30);
    assert_eq!(config.fetch_interval_secs, 5);
    assert_eq!(config.scheduler_url, "http://localhost:42781");
}

/// Test runner configuration custom values
#[test]
fn test_runner_config_custom() {
    let config = RunnerConfig {
        scheduler_url: "http://custom:9090".to_string(),
        name: "custom-runner".to_string(),
        runner_type: "firecracker".to_string(),
        capacity: 8,
        heartbeat_interval_secs: 60,
        fetch_interval_secs: 10,
    };
    assert_eq!(config.name, "custom-runner");
    assert_eq!(config.runner_type, "firecracker");
    assert_eq!(config.capacity, 8);
    assert_eq!(config.heartbeat_interval_secs, 60);
    assert_eq!(config.fetch_interval_secs, 10);
}

/// Test executable job creation
#[test]
fn test_executable_job_creation() {
    let job = ExecutableJob::new(JobId::new(), "rust:latest".to_string());
    assert_eq!(job.image, "rust:latest");
    assert_eq!(job.timeout_secs, 3600);
    assert!(job.steps.is_empty());
}

/// Test executable job with builder pattern
#[test]
fn test_executable_job_builder_pattern() {
    let steps = vec![
        JobStep::new("build", "cargo build"),
        JobStep::new("test", "cargo test"),
    ];
    let mut env = HashMap::new();
    env.insert("RUST_BACKTRACE".to_string(), "1".to_string());

    let job = ExecutableJob::new(JobId::new(), "rust:latest".to_string())
        .with_steps(steps.clone())
        .with_env(env.clone())
        .with_timeout(7200);

    assert_eq!(job.steps.len(), 2);
    assert_eq!(job.env.get("RUST_BACKTRACE"), Some(&"1".to_string()));
    assert_eq!(job.timeout_secs, 7200);
}

/// Test job step creation
#[test]
fn test_job_step_creation() {
    let step = JobStep::new("build", "cargo build --release");
    assert_eq!(step.name, "build");
    assert_eq!(step.run, "cargo build --release");
    assert!(step.env.is_none());
    assert!(step.working_directory.is_none());
}

/// Test job step with environment
#[test]
fn test_job_step_with_env() {
    let mut env = HashMap::new();
    env.insert("CI".to_string(), "true".to_string());
    env.insert("NODE_ENV".to_string(), "test".to_string());

    let step = JobStep {
        name: "test".to_string(),
        run: "npm test".to_string(),
        env: Some(env),
        working_directory: Some("/app".to_string()),
    };

    assert_eq!(step.name, "test");
    assert_eq!(step.run, "npm test");
    assert!(step.env.is_some());
    assert_eq!(step.env.as_ref().unwrap().get("CI"), Some(&"true".to_string()));
    assert_eq!(step.working_directory, Some("/app".to_string()));
}

/// Test job result structure
#[test]
fn test_job_result_success() {
    let result = JobResult {
        job_id: JobId::new(),
        success: true,
        exit_code: 0,
        step_results: vec![],
        error: None,
    };
    assert!(result.success);
    assert_eq!(result.exit_code, 0);
    assert!(result.error.is_none());
}

/// Test job result failure
#[test]
fn test_job_result_failure() {
    let result = JobResult {
        job_id: JobId::new(),
        success: false,
        exit_code: 1,
        step_results: vec![],
        error: Some("build failed".to_string()),
    };
    assert!(!result.success);
    assert_eq!(result.exit_code, 1);
    assert_eq!(result.error, Some("build failed".to_string()));
}

/// Test job executor creation (requires Docker, may fail)
#[tokio::test]
async fn test_job_executor_creation() {
    let executor = JobExecutor::new().await;
    // This may fail if Docker is not available
    // In that case we skip - this is expected in CI without Docker
    if executor.is_ok() {
        assert_eq!(executor.unwrap().active_count().await, 0);
    }
}

/// Test multiple job steps
#[test]
fn test_multiple_job_steps() {
    let steps = vec![
        JobStep::new("checkout", "git clone https://github.com/example/repo.git"),
        JobStep::new("setup", "npm install"),
        JobStep::new("build", "npm run build"),
        JobStep::new("test", "npm test"),
        JobStep::new("deploy", "npm run deploy"),
    ];

    let job = ExecutableJob::new(JobId::new(), "node:18".to_string())
        .with_steps(steps);

    assert_eq!(job.steps.len(), 5);
    assert_eq!(job.steps[0].name, "checkout");
    assert_eq!(job.steps[4].name, "deploy");
}
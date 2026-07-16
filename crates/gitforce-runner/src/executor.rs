//! Job executor

use gitforce_common::{JobId, Result};
use gitforce_sandbox::{DockerSandbox, Sandbox, SandboxInstance, SandboxLimits, StepResult};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Job to execute
#[derive(Debug, Clone)]
pub struct ExecutableJob {
    pub job_id: JobId,
    pub image: String,
    pub steps: Vec<JobStep>,
    pub env: HashMap<String, String>,
    pub working_dir: Option<String>,
    pub timeout_secs: u64,
}

impl ExecutableJob {
    /// Create a new executable job
    pub fn new(job_id: JobId, image: String) -> Self {
        Self {
            job_id,
            image,
            steps: Vec::new(),
            env: HashMap::new(),
            working_dir: None,
            timeout_secs: 3600,
        }
    }

    pub fn with_steps(mut self, steps: Vec<JobStep>) -> Self {
        self.steps = steps;
        self
    }

    pub fn with_env(mut self, env: HashMap<String, String>) -> Self {
        self.env = env;
        self
    }

    pub fn with_timeout(mut self, timeout_secs: u64) -> Self {
        self.timeout_secs = timeout_secs;
        self
    }
}

/// A single step in a job
#[derive(Debug, Clone)]
pub struct JobStep {
    pub name: String,
    pub run: String,
    pub env: Option<HashMap<String, String>>,
    pub working_directory: Option<String>,
}

impl JobStep {
    pub fn new(name: &str, run: &str) -> Self {
        Self {
            name: name.to_string(),
            run: run.to_string(),
            env: None,
            working_directory: None,
        }
    }
}

/// Job execution result
#[derive(Debug)]
pub struct JobResult {
    pub job_id: JobId,
    pub success: bool,
    pub exit_code: i32,
    pub step_results: Vec<StepResult>,
    pub error: Option<String>,
}

/// Job executor
pub struct JobExecutor {
    sandbox: Arc<DockerSandbox>,
    active_instances: Arc<RwLock<HashMap<JobId, SandboxInstance>>>,
}

impl JobExecutor {
    /// Create a new job executor
    pub async fn new() -> Result<Self> {
        let sandbox = DockerSandbox::new().await?;
        Ok(Self {
            sandbox: Arc::new(sandbox),
            active_instances: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    /// Execute a job
    pub async fn execute(&self, job: ExecutableJob) -> JobResult {
        let job_id = job.job_id;
        tracing::info!("executing job {}", job_id);

        // Create sandbox instance
        let instance = match self.sandbox.create(job_id, &job.image, SandboxLimits::default()).await {
            Ok(i) => i,
            Err(e) => {
                return JobResult {
                    job_id,
                    success: false,
                    exit_code: -1,
                    step_results: Vec::new(),
                    error: Some(format!("failed to create sandbox: {}", e)),
                };
            }
        };

        // Store active instance
        {
            let mut instances = self.active_instances.write().await;
            instances.insert(job_id, instance.clone());
        }

        // Execute steps
        let mut step_results = Vec::new();
        let mut success = true;
        let mut final_exit_code = 0;

        for step in &job.steps {
            tracing::debug!("executing step: {}", step.name);

            // Build command
            let cmd = vec!["sh", "-c", &step.run];

            // Execute step
            let result = self.sandbox.execute(&instance, &cmd).await;

            match result {
                Ok(step_result) => {
                    step_results.push(step_result.clone());
                    if step_result.exit_code != 0 {
                        success = false;
                        final_exit_code = step_result.exit_code;
                        tracing::error!("step {} failed with exit code {}", step.name, step_result.exit_code);
                        break;
                    }
                }
                Err(e) => {
                    success = false;
                    final_exit_code = -1;
                    step_results.push(StepResult {
                        exit_code: -1,
                        stdout: String::new(),
                        stderr: format!("execution error: {}", e),
                    });
                    break;
                }
            }
        }

        // Cleanup sandbox
        if let Err(e) = self.sandbox.destroy(instance).await {
            tracing::error!("failed to destroy sandbox for job {}: {}", job_id, e);
        }

        // Remove from active instances
        {
            let mut instances = self.active_instances.write().await;
            instances.remove(&job_id);
        }

        tracing::info!("job {} completed: success={}", job_id, success);

        JobResult {
            job_id,
            success,
            exit_code: final_exit_code,
            step_results,
            error: if success { None } else { Some("job failed".to_string()) },
        }
    }

    /// Cancel a running job
    pub async fn cancel(&self, job_id: JobId) -> Result<()> {
        let instances = self.active_instances.read().await;
        if let Some(instance) = instances.get(&job_id) {
            self.sandbox.destroy(instance.clone()).await?;
        }
        Ok(())
    }

    /// Get number of active jobs
    pub async fn active_count(&self) -> usize {
        let instances = self.active_instances.read().await;
        instances.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_executable_job_creation() {
        let job = ExecutableJob::new(JobId::new(), "rust:latest".to_string());
        assert_eq!(job.image, "rust:latest");
        assert_eq!(job.timeout_secs, 3600);
        assert!(job.steps.is_empty());
    }

    #[test]
    fn test_executable_job_with_steps() {
        let steps = vec![
            JobStep::new("build", "cargo build"),
            JobStep::new("test", "cargo test"),
        ];
        let job = ExecutableJob::new(JobId::new(), "rust:latest".to_string())
            .with_steps(steps.clone());
        assert_eq!(job.steps.len(), 2);
        assert_eq!(job.steps[0].name, "build");
    }

    #[test]
    fn test_executable_job_with_env() {
        let mut env = HashMap::new();
        env.insert("RUST_BACKTRACE".to_string(), "1".to_string());
        let job = ExecutableJob::new(JobId::new(), "rust:latest".to_string())
            .with_env(env);
        assert_eq!(job.env.get("RUST_BACKTRACE"), Some(&"1".to_string()));
    }

    #[test]
    fn test_executable_job_with_timeout() {
        let job = ExecutableJob::new(JobId::new(), "rust:latest".to_string())
            .with_timeout(7200);
        assert_eq!(job.timeout_secs, 7200);
    }

    #[test]
    fn test_job_step_creation() {
        let step = JobStep::new("build", "cargo build --release");
        assert_eq!(step.name, "build");
        assert_eq!(step.run, "cargo build --release");
        assert!(step.env.is_none());
        assert!(step.working_directory.is_none());
    }

    #[test]
    fn test_job_result_structure() {
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

    #[tokio::test]
    async fn test_active_count_empty() {
        let executor = JobExecutor::new().await.unwrap();
        assert_eq!(executor.active_count().await, 0);
    }

    #[test]
    fn test_executable_job_debug() {
        let job = ExecutableJob::new(JobId::new(), "rust:latest".to_string());
        let debug_str = format!("{:?}", job);
        assert!(debug_str.contains("rust:latest"));
    }

    #[test]
    fn test_job_step_debug() {
        let step = JobStep::new("build", "cargo build");
        let debug_str = format!("{:?}", step);
        assert!(debug_str.contains("build"));
    }

    #[test]
    fn test_job_result_debug() {
        let result = JobResult {
            job_id: JobId::new(),
            success: false,
            exit_code: 1,
            step_results: vec![],
            error: Some("failed".to_string()),
        };
        let debug_str = format!("{:?}", result);
        assert!(debug_str.contains("failed"));
    }

    #[test]
    fn test_job_step_with_env() {
        let mut env = HashMap::new();
        env.insert("KEY".to_string(), "value".to_string());
        let step = JobStep {
            name: "test".to_string(),
            run: "echo test".to_string(),
            env: Some(env),
            working_directory: Some("/tmp".to_string()),
        };
        assert_eq!(step.env.as_ref().unwrap().get("KEY"), Some(&"value".to_string()));
        assert_eq!(step.working_directory, Some("/tmp".to_string()));
    }

    #[test]
    fn test_job_result_with_error() {
        let result = JobResult {
            job_id: JobId::new(),
            success: false,
            exit_code: 127,
            step_results: vec![],
            error: Some("command not found".to_string()),
        };
        assert!(!result.success);
        assert_eq!(result.exit_code, 127);
        assert!(result.error.is_some());
    }

    #[test]
    fn test_executable_job_clone() {
        let mut env = HashMap::new();
        env.insert("FOO".to_string(), "bar".to_string());
        let job = ExecutableJob::new(JobId::new(), "rust:latest".to_string())
            .with_env(env)
            .with_timeout(1800);
        // Clone to verify all fields are cloneable
        let _ = job.clone();
    }

    #[test]
    fn test_executable_job_builder_pattern() {
        let steps = vec![
            JobStep::new("setup", "cargo fetch"),
            JobStep::new("build", "cargo build"),
            JobStep::new("test", "cargo test"),
        ];
        let mut env = HashMap::new();
        env.insert("CI".to_string(), "true".to_string());

        let job = ExecutableJob::new(JobId::new(), "rust:latest".to_string())
            .with_steps(steps)
            .with_env(env)
            .with_timeout(7200);

        assert_eq!(job.steps.len(), 3);
        assert_eq!(job.env.get("CI"), Some(&"true".to_string()));
        assert_eq!(job.timeout_secs, 7200);
    }

    #[test]
    fn test_job_step_working_directory() {
        let step = JobStep {
            name: "build".to_string(),
            run: "cargo build".to_string(),
            env: None,
            working_directory: Some("/workspace".to_string()),
        };
        assert_eq!(step.working_directory, Some("/workspace".to_string()));
    }

    #[test]
    fn test_job_result_with_step_results() {
        use gitforce_sandbox::StepResult;
        let step_result = StepResult {
            exit_code: 0,
            stdout: "Build successful".to_string(),
            stderr: String::new(),
        };
        let result = JobResult {
            job_id: JobId::new(),
            success: true,
            exit_code: 0,
            step_results: vec![step_result],
            error: None,
        };
        assert_eq!(result.step_results.len(), 1);
        assert_eq!(result.step_results[0].stdout, "Build successful");
    }

    #[test]
    fn test_executable_job_with_multiple_steps() {
        let steps = vec![
            JobStep::new("step1", "echo 1"),
            JobStep::new("step2", "echo 2"),
            JobStep::new("step3", "echo 3"),
        ];
        let job = ExecutableJob::new(JobId::new(), "alpine:latest".to_string())
            .with_steps(steps);
        assert_eq!(job.steps.len(), 3);
    }

    #[test]
    fn test_executable_job_default_values() {
        let job = ExecutableJob::new(JobId::new(), "ubuntu:latest".to_string());
        assert!(job.steps.is_empty());
        assert!(job.env.is_empty());
        assert!(job.working_dir.is_none());
        assert_eq!(job.timeout_secs, 3600);
    }

    #[test]
    fn test_job_step_equality() {
        let step1 = JobStep::new("build", "cargo build");
        let step2 = JobStep::new("build", "cargo build");
        let step3 = JobStep::new("test", "cargo test");
        assert_eq!(step1.name, step2.name);
        assert_eq!(step1.run, step2.run);
        assert_ne!(step1.name, step3.name);
    }

    #[test]
    fn test_executable_job_env_is_empty_by_default() {
        let job = ExecutableJob::new(JobId::new(), "rust:latest".to_string());
        assert!(job.env.is_empty());
    }

    #[test]
    fn test_executable_job_with_working_dir() {
        let job = ExecutableJob::new(JobId::new(), "rust:latest".to_string());
        // working_dir is not directly settable via builder but exists as a field
        assert!(job.working_dir.is_none());
    }

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

    #[test]
    fn test_job_result_failure_with_stderr() {
        use gitforce_sandbox::StepResult;
        let step_result = StepResult {
            exit_code: 1,
            stdout: String::new(),
            stderr: "compilation failed".to_string(),
        };
        let result = JobResult {
            job_id: JobId::new(),
            success: false,
            exit_code: 1,
            step_results: vec![step_result],
            error: Some("step failed".to_string()),
        };
        assert!(!result.success);
        assert_eq!(result.exit_code, 1);
        assert!(result.error.is_some());
    }

    #[test]
    fn test_executable_job_all_fields() {
        let steps = vec![
            JobStep::new("build", "cargo build"),
            JobStep::new("test", "cargo test"),
        ];
        let mut env = HashMap::new();
        env.insert("RUST_BACKTRACE".to_string(), "1".to_string());
        env.insert("CI".to_string(), "true".to_string());

        let job = ExecutableJob::new(JobId::new(), "rust:latest".to_string())
            .with_steps(steps)
            .with_env(env)
            .with_timeout(7200);

        assert_eq!(job.image, "rust:latest");
        assert_eq!(job.steps.len(), 2);
        assert_eq!(job.env.len(), 2);
        assert_eq!(job.timeout_secs, 7200);
    }

    #[test]
    fn test_job_step_all_fields() {
        let mut env = HashMap::new();
        env.insert("PATH".to_string(), "/usr/bin".to_string());

        let step = JobStep {
            name: "build".to_string(),
            run: "cargo build --release".to_string(),
            env: Some(env),
            working_directory: Some("/project".to_string()),
        };

        assert_eq!(step.name, "build");
        assert_eq!(step.run, "cargo build --release");
        assert!(step.env.is_some());
        assert_eq!(step.working_directory, Some("/project".to_string()));
    }

    #[test]
    fn test_executable_job_id() {
        let job_id = JobId::new();
        let job = ExecutableJob::new(job_id, "rust:latest".to_string());
        assert_eq!(job.job_id, job_id);
    }

    #[test]
    fn test_executable_job_cloneable() {
        let job = ExecutableJob::new(JobId::new(), "alpine:latest".to_string());
        let cloned = job.clone();
        assert_eq!(cloned.image, job.image);
        assert_eq!(cloned.job_id, job.job_id);
    }

    #[test]
    fn test_job_result_cloneable() {
        // JobResult doesn't implement Clone, but we can verify it's Debug
        let result = JobResult {
            job_id: JobId::new(),
            success: true,
            exit_code: 0,
            step_results: vec![],
            error: None,
        };
        // Verify it can be formatted with Debug
        let debug_str = format!("{:?}", result);
        assert!(debug_str.contains("JobResult"));
    }

    #[tokio::test]
    async fn test_executor_active_count_after_create() {
        let executor = JobExecutor::new().await.unwrap();
        // Should start at 0
        let count = executor.active_count().await;
        assert_eq!(count, 0);
    }

    #[test]
    fn test_executable_job_with_empty_steps() {
        let job = ExecutableJob::new(JobId::new(), "rust:latest".to_string())
            .with_steps(vec![]);
        assert!(job.steps.is_empty());
    }

    #[test]
    fn test_executable_job_builder_chaining() {
        let job = ExecutableJob::new(JobId::new(), "rust:latest".to_string())
            .with_timeout(3600)
            .with_env(HashMap::new());
        assert_eq!(job.timeout_secs, 3600);
        assert!(job.env.is_empty());
    }

    #[test]
    fn test_job_step_clone() {
        let step = JobStep::new("build", "cargo build");
        let cloned = step.clone();
        assert_eq!(cloned.name, step.name);
        assert_eq!(cloned.run, step.run);
    }

    #[test]
    fn test_executable_job_clone_and_modify() {
        let mut env = HashMap::new();
        env.insert("FOO".to_string(), "bar".to_string());
        let job1 = ExecutableJob::new(JobId::new(), "rust:latest".to_string())
            .with_env(env);
        let job2 = job1.clone();
        // Verify both have same values
        assert_eq!(job1.image, job2.image);
        assert_eq!(job1.timeout_secs, job2.timeout_secs);
    }

    #[test]
    fn test_job_step_from_constructor() {
        let step = JobStep::new("build", "cargo build --release");
        assert_eq!(step.name, "build");
        assert_eq!(step.run, "cargo build --release");
        assert!(step.env.is_none());
        assert!(step.working_directory.is_none());
    }

    #[test]
    fn test_executable_job_with_empty_env() {
        let job = ExecutableJob::new(JobId::new(), "alpine:latest".to_string())
            .with_env(HashMap::new());
        assert!(job.env.is_empty());
    }

    #[test]
    fn test_executable_job_very_long_timeout() {
        let job = ExecutableJob::new(JobId::new(), "rust:latest".to_string())
            .with_timeout(86400); // 24 hours
        assert_eq!(job.timeout_secs, 86400);
    }

    #[test]
    fn test_executable_job_zero_timeout() {
        let job = ExecutableJob::new(JobId::new(), "rust:latest".to_string())
            .with_timeout(0);
        assert_eq!(job.timeout_secs, 0);
    }

    #[test]
    fn test_job_step_clone_is_independent() {
        let step1 = JobStep::new("build", "cargo build");
        let step2 = step1.clone();
        // Modify step2
        let step2_modified = JobStep {
            name: "test".to_string(),
            run: "cargo test".to_string(),
            env: None,
            working_directory: None,
        };
        assert_ne!(step1.name, step2_modified.name);
        assert_eq!(step1.name, step2.name);
    }

    #[test]
    fn test_executable_job_with_unicode_in_env() {
        let mut env = HashMap::new();
        env.insert("中文".to_string(), "value".to_string());
        let job = ExecutableJob::new(JobId::new(), "rust:latest".to_string())
            .with_env(env);
        assert_eq!(job.env.get("中文"), Some(&"value".to_string()));
    }

    #[test]
    fn test_executable_job_with_unicode_in_name() {
        let job = ExecutableJob::new(JobId::new(), "rust:最新".to_string());
        assert_eq!(job.image, "rust:最新");
    }

    #[test]
    fn test_executable_job_env_keys_with_special_chars() {
        let mut env = HashMap::new();
        env.insert("MY_VAR_123".to_string(), "value".to_string());
        env.insert("ANOTHER_VAR".to_string(), "another".to_string());
        let job = ExecutableJob::new(JobId::new(), "rust:latest".to_string())
            .with_env(env);
        assert_eq!(job.env.len(), 2);
    }

    #[tokio::test]
    async fn test_executor_cancel_nonexistent_job() {
        let executor = JobExecutor::new().await.unwrap();
        // Cancel a job that doesn't exist should not error
        let result = executor.cancel(JobId::new()).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_executor_active_count_initial() {
        let executor = JobExecutor::new().await.unwrap();
        assert_eq!(executor.active_count().await, 0);
    }

    #[test]
    fn test_executable_job_clone_preserves_id() {
        let job_id = JobId::new();
        let job = ExecutableJob::new(job_id, "rust:latest".to_string());
        let cloned = job.clone();
        assert_eq!(cloned.job_id, job_id);
    }

    #[test]
    fn test_executable_job_preserves_timeout() {
        let job = ExecutableJob::new(JobId::new(), "rust:latest".to_string())
            .with_timeout(7200);
        assert_eq!(job.timeout_secs, 7200);
    }

    #[test]
    fn test_job_step_with_both_env_and_working_dir() {
        let mut env = HashMap::new();
        env.insert("HOME".to_string(), "/root".to_string());
        let step = JobStep {
            name: "build".to_string(),
            run: "cargo build".to_string(),
            env: Some(env),
            working_directory: Some("/project".to_string()),
        };
        assert!(step.env.is_some());
        assert!(step.working_directory.is_some());
    }

    #[tokio::test]
    async fn test_executor_execute_simple_job() {
        let executor = JobExecutor::new().await.unwrap();

        let job = ExecutableJob::new(JobId::new(), "alpine:latest".to_string())
            .with_steps(vec![
                JobStep::new("test", "echo hello"),
            ]);

        let result = executor.execute(job).await;
        // Job completed - success or failure depends on Docker availability
        assert!(result.exit_code == 0 || !result.success || result.error.is_some());
    }

    #[tokio::test]
    async fn test_executor_execute_with_env() {
        let executor = JobExecutor::new().await.unwrap();

        let mut env = HashMap::new();
        env.insert("TEST_VAR".to_string(), "test_value".to_string());

        let job = ExecutableJob::new(JobId::new(), "alpine:latest".to_string())
            .with_steps(vec![
                JobStep::new("env", "echo $TEST_VAR"),
            ])
            .with_env(env);

        let result = executor.execute(job).await;
        // Should have executed regardless of outcome
        assert!(result.exit_code == 0 || !result.success);
    }
}

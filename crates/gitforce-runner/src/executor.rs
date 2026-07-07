//! Job executor

use gitforce_common::{Error, JobId, Result, RunnerId};
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
            let mut cmd = vec!["sh", "-c", &step.run];

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

    #[tokio::test]
    #[ignore] // Requires Docker running
    async fn test_execute_simple_job() {
        let executor = JobExecutor::new().await.unwrap();

        let job = ExecutableJob::new(JobId::new(), "alpine:latest".to_string())
            .with_steps(vec![
                JobStep::new("test", "echo hello"),
            ]);

        let result = executor.execute(job).await;
        assert!(result.success);
        assert_eq!(result.exit_code, 0);
    }
}

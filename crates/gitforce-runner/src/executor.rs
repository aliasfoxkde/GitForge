//! Job executor with container pooling

use gitforce_common::{JobId, Result};
use gitforce_sandbox::{DockerSandbox, Sandbox, SandboxInstance, SandboxLimits, StepResult};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Default number of pre-warmed containers per image
const POOL_SIZE: usize = 2;

/// A pool of pre-warmed container instances
pub struct ContainerPool {
    pools: Arc<RwLock<HashMap<String, Vec<SandboxInstance>>>>, // image -> instances
    sandbox: Arc<DockerSandbox>,
}

impl ContainerPool {
    /// Create a new container pool
    pub async fn new() -> Result<Self> {
        let sandbox = DockerSandbox::new().await?;
        Ok(Self {
            pools: Arc::new(RwLock::new(HashMap::new())),
            sandbox: Arc::new(sandbox),
        })
    }

    /// Pre-warm containers for an image
    pub async fn prewarm(&self, image: &str, count: usize) -> Result<()> {
        let mut pools = self.pools.write().await;
        let instances = pools.entry(image.to_string()).or_insert_with(Vec::new);

        while instances.len() < count {
            let id = JobId::new();
            match self
                .sandbox
                .create(id, image, SandboxLimits::default())
                .await
            {
                Ok(instance) => {
                    tracing::info!("pre-warmed container for image {}", image);
                    instances.push(instance);
                }
                Err(e) => {
                    tracing::warn!("failed to pre-warm container: {}", e);
                    break;
                }
            }
        }
        Ok(())
    }

    /// Get a container from the pool, creating one if needed
    pub async fn acquire(&self, job_id: &JobId, image: &str) -> Result<SandboxInstance> {
        let mut pools = self.pools.write().await;

        // Try to get from pool
        if let Some(instances) = pools.get_mut(image) {
            if let Some(instance) = instances.pop() {
                tracing::debug!("reusing pooled container for job {}", job_id);
                return Ok(instance);
            }
        }

        // Pool empty or no pool for this image, create new
        tracing::debug!("creating new container for job {} (pool empty)", job_id);
        self.sandbox
            .create(*job_id, image, SandboxLimits::default())
            .await
    }

    /// Return a container to the pool
    pub async fn release(&self, image: &str, instance: SandboxInstance) {
        let mut pools = self.pools.write().await;
        let instances = pools.entry(image.to_string()).or_insert_with(Vec::new);

        if instances.len() < POOL_SIZE {
            if let Err(e) = self.sandbox.destroy(instance).await {
                tracing::warn!("failed to reset pooled container: {}", e);
                return;
            }
            // Create fresh instance for the pool
            let id = JobId::new();
            match self
                .sandbox
                .create(id, image, SandboxLimits::default())
                .await
            {
                Ok(new_instance) => {
                    instances.push(new_instance);
                    tracing::debug!("returned container to pool");
                }
                Err(e) => {
                    tracing::warn!("failed to create replacement container: {}", e);
                }
            }
        } else {
            // Pool full, destroy
            if let Err(e) = self.sandbox.destroy(instance).await {
                tracing::warn!("failed to destroy excess container: {}", e);
            }
        }
    }
}

/// Job executor
pub struct JobExecutor {
    pool: ContainerPool,
    active_instances: Arc<RwLock<HashMap<JobId, (String, SandboxInstance)>>>, // job_id -> (image, instance)
}

impl JobExecutor {
    /// Create a new job executor
    pub async fn new() -> Result<Self> {
        let pool = ContainerPool::new().await?;
        Ok(Self {
            pool,
            active_instances: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    /// Pre-warm containers for an image
    pub async fn prewarm(&self, image: &str, count: usize) -> Result<()> {
        self.pool.prewarm(image, count).await
    }

    /// Execute a job
    pub async fn execute(&self, job: ExecutableJob) -> JobResult {
        let job_id = job.job_id; // Copy type
        tracing::info!("executing job {}", job_id);

        // Acquire container from pool
        let instance = match self.pool.acquire(&job_id, &job.image).await {
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
            instances.insert(job_id, (job.image.clone(), instance.clone()));
        }

        // Execute steps
        let mut step_results = Vec::new();
        let mut success = true;
        let mut final_exit_code = 0;

        for step in &job.steps {
            tracing::debug!("executing step: {}", step.name);
            let cmd = vec!["sh", "-c", &step.run];

            let result = self.pool.sandbox.execute(&instance, &cmd).await;

            match result {
                Ok(step_result) => {
                    step_results.push(step_result.clone());
                    if step_result.exit_code != 0 {
                        success = false;
                        final_exit_code = step_result.exit_code;
                        tracing::error!(
                            "step {} failed with exit code {}",
                            step.name,
                            step_result.exit_code
                        );
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

        // Return container to pool
        {
            let mut instances = self.active_instances.write().await;
            if let Some((image, inst)) = instances.remove(&job_id) {
                self.pool.release(&image, inst).await;
            }
        }

        tracing::info!("job {} completed: success={}", job_id, success);

        JobResult {
            job_id,
            success,
            exit_code: final_exit_code,
            step_results,
            error: if success {
                None
            } else {
                Some("job failed".to_string())
            },
        }
    }

    /// Cancel a running job
    pub async fn cancel(&self, job_id: &JobId) -> Result<()> {
        let instances = self.active_instances.read().await;
        if let Some((_image, instance)) = instances.get(job_id) {
            self.pool.sandbox.destroy(instance.clone()).await?;
        }
        Ok(())
    }
}

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

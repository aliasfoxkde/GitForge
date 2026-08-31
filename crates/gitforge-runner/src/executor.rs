//! Job executor with container pooling

use gitforge_common::{JobId, PipelineRunId, RepoId, Result};
use gitforge_sandbox::{
    DockerSandbox, OutputSink, Sandbox, SandboxInstance, SandboxLimits, StepResult,
};
use gitforge_storage::{
    Artifact, ArtifactReceipt, ArtifactStore, FileJobLogStore, FileStorage, JobReceipt, LogReceipt,
    ReceiptStatus, MAX_LOG_BYTES, RECEIPT_VERSION,
};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::fs;
use tokio::sync::RwLock;
use tokio::time::{timeout, Duration, Instant};

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
        let sandbox = DockerSandbox::connect_required().await?;
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
    pub async fn acquire(
        &self,
        job_id: &JobId,
        image: &str,
        workspace_path: Option<&str>,
    ) -> Result<SandboxInstance> {
        if workspace_path.is_some() {
            return self
                .sandbox
                .create_with_workspace(*job_id, image, SandboxLimits::default(), workspace_path)
                .await;
        }
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
    pub async fn release(
        &self,
        image: &str,
        instance: SandboxInstance,
        workspace_path: Option<&str>,
    ) {
        if workspace_path.is_some() {
            if let Err(error) = self.sandbox.destroy(instance).await {
                tracing::warn!("failed to destroy workspace container: {}", error);
            }
            return;
        }
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
#[allow(clippy::type_complexity)]
pub struct JobExecutor {
    pool: ContainerPool,
    active_job_count: Arc<RwLock<usize>>, // number of jobs currently executing
    active_instances: Arc<RwLock<HashMap<JobId, (String, Option<String>, SandboxInstance)>>>, // job_id -> (image, workspace, instance)
    artifact_storage: Arc<FileStorage>,
    log_store: Arc<FileJobLogStore>,
}

impl JobExecutor {
    /// Create a new job executor
    pub async fn new() -> Result<Self> {
        let pool = ContainerPool::new().await?;
        let storage_root = std::env::var("GITFORGE_ARTIFACT_ROOT")
            .unwrap_or_else(|_| "target/gitforge-artifacts".to_string());
        let artifact_storage = FileStorage::new(storage_root.clone()).await?;
        let log_store = FileJobLogStore::new(&storage_root).await?;
        Ok(Self {
            pool,
            active_instances: Arc::new(RwLock::new(HashMap::new())),
            active_job_count: Arc::new(RwLock::new(0)),
            artifact_storage: Arc::new(artifact_storage),
            log_store: Arc::new(log_store),
        })
    }

    async fn collect_artifacts(
        &self,
        job_id: JobId,
        workspace_path: Option<&str>,
    ) -> Vec<ArtifactReceipt> {
        let Some(workspace_path) = workspace_path else {
            return Vec::new();
        };
        let artifact_dir = Path::new(workspace_path).join("artifacts");
        let mut entries = match fs::read_dir(&artifact_dir).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
            Err(error) => {
                tracing::warn!(%error, path = %artifact_dir.display(), "failed to read artifact directory");
                return Vec::new();
            }
        };
        let mut receipts = Vec::new();
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path: PathBuf = entry.path();
            let Ok(metadata) = entry.metadata().await else {
                continue;
            };
            if !metadata.is_file() || metadata.len() > gitforge_storage::receipt::MAX_ARTIFACT_BYTES
            {
                tracing::warn!(path = %path.display(), "skipping invalid or oversized artifact");
                continue;
            }
            let name = path
                .strip_prefix(&artifact_dir)
                .unwrap_or(&path)
                .to_string_lossy()
                .to_string();
            let Ok(mut artifact) = Artifact::from_file(job_id, name.clone(), &path).await else {
                continue;
            };
            let Ok(data) = fs::read(&path).await else {
                continue;
            };
            artifact.path = path.to_string_lossy().to_string();
            if let Err(error) = self.artifact_storage.put(&artifact, &data).await {
                tracing::warn!(%error, path = %path.display(), "failed to persist artifact");
                continue;
            }
            receipts.push(ArtifactReceipt {
                name,
                uri: format!("gitforge://artifact/{}", artifact.id),
                sha256: artifact.checksum,
                bytes: artifact.size_bytes,
                media_type: artifact.content_type,
            });
        }
        receipts
    }

    /// Collect stdout/stderr from step results, bound to max_size, and store as LogReceipt.
    async fn collect_logs(&self, job_id: JobId, step_results: &[StepResult]) -> Option<LogReceipt> {
        // Concatenate all stdout and stderr
        let mut combined = String::new();
        for (i, sr) in step_results.iter().enumerate() {
            if !sr.stdout.is_empty() {
                combined.push_str(&format!("[step {} stdout]\n{}\n", i, sr.stdout));
            }
            if !sr.stderr.is_empty() {
                combined.push_str(&format!("[step {} stderr]\n{}\n", i, sr.stderr));
            }
        }

        let data = combined.into_bytes();
        match self
            .log_store
            .bounded_put(job_id, data, MAX_LOG_BYTES)
            .await
        {
            Ok(receipt) => {
                tracing::debug!("stored {} byte log for job {}", receipt.bytes, job_id);
                Some(receipt)
            }
            Err(e) => {
                tracing::warn!("failed to store job log: {}", e);
                None
            }
        }
    }

    /// Pre-warm containers for an image
    pub async fn prewarm(&self, image: &str, count: usize) -> Result<()> {
        self.pool.prewarm(image, count).await
    }

    /// Execute a job
    pub async fn execute(&self, job: ExecutableJob) -> JobResult {
        self.execute_with_output(job, None).await
    }

    /// Best-effort removal of containers left by a failed acquisition.
    ///
    /// When acquisition is abandoned (timeout or creation error) the Docker
    /// daemon may still have materialized the container server-side; without
    /// reaping, every slow acquisition leaks a container that never starts.
    async fn reap_attempt_containers(&self, job_id: JobId) {
        if let Err(error) = self.pool.sandbox.remove_job_containers(job_id).await {
            tracing::warn!(%job_id, %error, "failed to reap containers after failed acquisition");
        }
    }

    /// Execute a job and forward sandbox output while each step is running.
    /// The sink is optional so existing callers and local tests retain the
    /// original accumulated-result behavior.
    pub async fn execute_with_output(
        &self,
        job: ExecutableJob,
        output_sink: Option<Arc<dyn OutputSink>>,
    ) -> JobResult {
        let job_id = job.job_id; // Copy type
        let started_at = chrono::Utc::now();
        let job_timeout = Duration::from_secs(job.timeout_secs.clamp(5, 24 * 60 * 60));
        let deadline = Instant::now() + job_timeout;
        tracing::info!("executing job {}", job_id);

        // Acquire container from pool
        let acquire_timeout = job_timeout.min(Duration::from_secs(60));
        let instance = match timeout(
            acquire_timeout,
            self.pool
                .acquire(&job_id, &job.image, job.working_dir.as_deref()),
        )
        .await
        {
            Err(_) => {
                self.reap_attempt_containers(job_id).await;
                let completed_at = chrono::Utc::now();
                return JobResult {
                    job_id,
                    success: false,
                    exit_code: -1,
                    step_results: Vec::new(),
                    artifacts: Vec::new(),
                    logs: None,
                    started_at,
                    completed_at,
                    error: Some(format!(
                        "failed to create sandbox: acquisition timed out after {} seconds",
                        acquire_timeout.as_secs()
                    )),
                    workspace_path: job.working_dir.clone(),
                };
            }
            Ok(Err(e)) => {
                self.reap_attempt_containers(job_id).await;
                let completed_at = chrono::Utc::now();
                return JobResult {
                    job_id,
                    success: false,
                    exit_code: -1,
                    step_results: Vec::new(),
                    artifacts: Vec::new(),
                    logs: None,
                    started_at,
                    completed_at,
                    error: Some(format!("failed to create sandbox: {}", e)),
                    workspace_path: job.working_dir.clone(),
                };
            }
            Ok(Ok(instance)) => instance,
        };

        // Increment active job count
        {
            let mut count = self.active_job_count.write().await;
            *count += 1;
        }

        // Store active instance
        {
            let mut instances = self.active_instances.write().await;
            instances.insert(
                job_id,
                (job.image.clone(), job.working_dir.clone(), instance.clone()),
            );
        }

        // Execute steps
        let mut step_results = Vec::new();
        let mut success = true;
        let mut final_exit_code = 0;
        let mut timed_out = false;

        for step in &job.steps {
            tracing::debug!("executing step: {}", step.name);
            let cmd = vec!["sh", "-c", &step.run];

            let remaining = deadline.saturating_duration_since(Instant::now());
            let result = if remaining.is_zero() {
                Err(gitforge_common::Error::timeout(
                    "job timed out before step started",
                ))
            } else {
                timeout(
                    remaining,
                    self.pool
                        .sandbox
                        .execute_with_output(&instance, &cmd, output_sink.clone()),
                )
                .await
                .map_err(|_| gitforge_common::Error::timeout("job timed out"))
                .and_then(|result| result)
            };

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
                    timed_out = deadline <= Instant::now();
                    step_results.push(StepResult {
                        exit_code: -1,
                        stdout: String::new(),
                        stderr: format!("execution error: {}", e),
                    });
                    break;
                }
            }
        }

        // A dropped Docker exec stream does not guarantee that the process
        // inside the container has exited.  Tear down the exact container
        // immediately on timeout, before artifact/log collection, so timed-out
        // jobs cannot leave an active exec or conmon helper behind.
        if timed_out {
            if timeout(
                Duration::from_secs(15),
                self.pool.sandbox.destroy(instance.clone()),
            )
            .await
            .is_err()
            {
                tracing::error!(%job_id, "timed-out sandbox teardown exceeded 15 seconds");
            }
        }

        // Collect artifacts
        let artifacts = self
            .collect_artifacts(job_id, job.working_dir.as_deref())
            .await;

        // Collect logs
        let logs = self.collect_logs(job_id, &step_results).await;

        // Return container to pool
        let released = {
            let mut instances = self.active_instances.write().await;
            instances.remove(&job_id)
        };
        if let Some((image, workspace, inst)) = released {
            if !timed_out {
                if timeout(
                    Duration::from_secs(30),
                    self.pool.release(&image, inst, workspace.as_deref()),
                )
                .await
                .is_err()
                {
                    tracing::warn!("timed out cleaning up sandbox for job {}", job_id);
                }
            }
        }

        // Decrement active job count
        {
            let mut count = self.active_job_count.write().await;
            *count = count.saturating_sub(1);
        }

        let completed_at = chrono::Utc::now();
        tracing::info!("job {} completed: success={}", job_id, success);

        JobResult {
            job_id,
            success,
            exit_code: final_exit_code,
            step_results,
            artifacts,
            logs,
            started_at,
            completed_at,
            error: if success {
                None
            } else {
                Some("job failed".to_string())
            },
            workspace_path: job.working_dir.clone(),
        }
    }

    /// Cancel a running job
    pub async fn cancel(&self, job_id: &JobId) -> Result<()> {
        let instance = {
            let mut instances = self.active_instances.write().await;
            instances.remove(job_id)
        };
        if let Some((_image, _workspace, inst)) = instance {
            self.pool.sandbox.destroy(inst).await?;
        }
        Ok(())
    }

    /// Get the number of active jobs
    pub async fn active_job_count(&self) -> usize {
        *self.active_job_count.read().await
    }

    /// Wait for all active jobs to complete
    pub async fn wait_for_jobs_complete(&self, timeout_duration: Duration) -> bool {
        let start = tokio::time::Instant::now();
        while *self.active_job_count.read().await > 0 {
            if start.elapsed() >= timeout_duration {
                tracing::warn!(
                    "timeout waiting for {} active jobs to complete",
                    *self.active_job_count.read().await
                );
                return false;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        true
    }

    /// Cancel all active jobs and drain active_instances
    pub async fn cancel_all_jobs(&self) {
        let instances = {
            let mut instances = self.active_instances.write().await;
            std::mem::take(&mut *instances)
        };

        for (_job_id, (_image, _workspace, instance)) in instances {
            if let Err(e) = self.pool.sandbox.destroy(instance).await {
                tracing::warn!("failed to destroy sandbox during cancel_all_jobs: {}", e);
            }
        }

        let mut count = self.active_job_count.write().await;
        *count = 0;
    }
}

/// Job to execute
#[derive(Debug, Clone)]
pub struct ExecutableJob {
    pub job_id: JobId,
    pub pipeline_run_id: PipelineRunId,
    pub repository_id: Option<RepoId>,
    pub base_sha: Option<String>,
    pub image: String,
    pub steps: Vec<JobStep>,
    pub env: HashMap<String, String>,
    pub working_dir: Option<String>,
    pub timeout_secs: u64,
}

impl ExecutableJob {
    /// Create a new executable job
    pub fn new(job_id: JobId, pipeline_run_id: PipelineRunId, image: String) -> Self {
        Self {
            job_id,
            pipeline_run_id,
            repository_id: None,
            base_sha: None,
            image,
            steps: Vec::new(),
            env: HashMap::new(),
            working_dir: None,
            timeout_secs: 300,
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
    pub artifacts: Vec<ArtifactReceipt>,
    pub logs: Option<LogReceipt>,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub completed_at: chrono::DateTime<chrono::Utc>,
    pub error: Option<String>,
    /// Workspace path where the job executed
    pub workspace_path: Option<String>,
}

impl JobResult {
    /// Determine receipt status from job result
    fn status(&self) -> ReceiptStatus {
        if self.success {
            ReceiptStatus::Succeeded
        } else if self
            .error
            .as_ref()
            .map(|e| e.contains("timeout"))
            .unwrap_or(false)
        {
            ReceiptStatus::TimedOut
        } else {
            ReceiptStatus::Failed
        }
    }
}

impl JobResult {
    /// Build a `JobReceipt` from this result, using metadata from `job`.
    ///
    /// Returns `None` if the receipt fails validation.
    pub fn receipt(&self, job: &ExecutableJob) -> Option<JobReceipt> {
        let status = self.status();

        let commands: Vec<String> = job.steps.iter().map(|s| s.run.clone()).collect();
        let working_directory = job.working_dir.clone();
        let (output_sha, output_bytes) = Self::compute_output_sha(&self.artifacts);

        // Build log and artifact URI lists from receipts
        let log_uri: Vec<String> = self.logs.iter().map(|l| l.uri.clone()).collect();
        let artifact_uri: Vec<String> = self.artifacts.iter().map(|a| a.uri.clone()).collect();

        let mut receipt = JobReceipt {
            receipt_version: RECEIPT_VERSION,
            work_request_id: None,
            pipeline_run_id: job.pipeline_run_id,
            job_id: self.job_id,
            repository_id: job.repository_id,
            base_sha: job.base_sha.clone(),
            head_sha: job.base_sha.clone(), // Head SHA same as base for single-commit jobs
            workspace_path: self.workspace_path.clone(),
            run_id: Some(format!("run-{}", self.job_id)), // Generate run ID from job ID
            status,
            commands,
            working_directory,
            exit_code: Some(self.exit_code),
            changed_paths: Vec::new(),
            started_at: self.started_at,
            completed_at: self.completed_at,
            output_sha,
            output_bytes,
            stable_uri: format!("gitforge://job/{}", self.job_id),
            log_uri,
            artifact_uri,
            logs: self.logs.clone(),
            artifacts: self.artifacts.clone(),
            error: self.error.clone(),
            receipt_signature: None,
        };

        // Sign the receipt for integrity verification
        receipt.receipt_signature = Some(receipt.compute_signature());

        // Validate before returning
        receipt.validate().ok()?;
        Some(receipt)
    }

    /// Compute the aggregate output SHA from a list of artifact receipts.
    fn compute_output_sha(artifacts: &[ArtifactReceipt]) -> (String, u64) {
        if artifacts.is_empty() {
            return (String::new(), 0);
        }
        let mut hasher = Sha256::new();
        let mut total_bytes = 0u64;
        for ar in artifacts {
            hasher.update(ar.sha256.as_bytes());
            total_bytes += ar.bytes;
        }
        (hex::encode(hasher.finalize()), total_bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gitforge_sandbox::StepResult;

    #[test]
    fn test_executable_job_builder() {
        let job_id = JobId::new();
        let pipeline_run_id = PipelineRunId::new();
        let job = ExecutableJob::new(job_id, pipeline_run_id, "rust:latest".to_string())
            .with_steps(vec![
                JobStep::new("build", "cargo build"),
                JobStep::new("test", "cargo test"),
            ])
            .with_env(
                [("RUST_BACKTRACE".to_string(), "1".to_string())]
                    .into_iter()
                    .collect(),
            )
            .with_timeout(600);

        assert_eq!(job.image, "rust:latest");
        assert_eq!(job.timeout_secs, 600);
        assert_eq!(job.steps.len(), 2);
        assert_eq!(job.env.get("RUST_BACKTRACE"), Some(&"1".to_string()));
    }

    #[test]
    fn test_executable_job_defaults() {
        let job_id = JobId::new();
        let pipeline_run_id = PipelineRunId::new();
        let job = ExecutableJob::new(job_id, pipeline_run_id, "alpine:latest".to_string());

        assert!(job.repository_id.is_none());
        assert!(job.base_sha.is_none());
        assert!(job.working_dir.is_none());
        assert_eq!(job.timeout_secs, 300);
        assert!(job.steps.is_empty());
        assert!(job.env.is_empty());
    }

    #[test]
    fn test_job_step_new() {
        let step = JobStep::new("lint", "cargo clippy");

        assert_eq!(step.name, "lint");
        assert_eq!(step.run, "cargo clippy");
        assert!(step.env.is_none());
        assert!(step.working_directory.is_none());
    }

    #[test]
    fn test_job_result_status() {
        let started = chrono::Utc::now();
        let completed = started + chrono::Duration::seconds(30);

        // Test success status
        let success_result = JobResult {
            job_id: JobId::new(),
            success: true,
            exit_code: 0,
            step_results: vec![StepResult {
                exit_code: 0,
                stdout: "OK".to_string(),
                stderr: String::new(),
            }],
            artifacts: vec![],
            logs: None,
            started_at: started,
            completed_at: completed,
            error: None,
            workspace_path: None,
        };
        assert_eq!(success_result.status(), ReceiptStatus::Succeeded);

        // Test failure status
        let failure_result = JobResult {
            job_id: JobId::new(),
            success: false,
            exit_code: 1,
            step_results: vec![],
            artifacts: vec![],
            logs: None,
            started_at: started,
            completed_at: completed,
            error: Some("build failed".to_string()),
            workspace_path: None,
        };
        assert_eq!(failure_result.status(), ReceiptStatus::Failed);

        // Test timeout status
        let timeout_result = JobResult {
            job_id: JobId::new(),
            success: false,
            exit_code: -1,
            step_results: vec![],
            artifacts: vec![],
            logs: None,
            started_at: started,
            completed_at: completed,
            error: Some("operation timeout exceeded".to_string()),
            workspace_path: None,
        };
        assert_eq!(timeout_result.status(), ReceiptStatus::TimedOut);
    }

    #[test]
    fn test_compute_output_sha_empty() {
        let (sha, bytes) = JobResult::compute_output_sha(&[]);
        assert!(sha.is_empty());
        assert_eq!(bytes, 0);
    }

    #[test]
    fn test_compute_output_sha_single_artifact() {
        let artifacts = vec![ArtifactReceipt {
            name: "binary".to_string(),
            uri: "gitforge://artifact/123".to_string(),
            sha256: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".to_string(),
            bytes: 1024,
            media_type: Some("application/octet-stream".to_string()),
        }];

        let (sha, bytes) = JobResult::compute_output_sha(&artifacts);
        assert!(!sha.is_empty());
        assert_eq!(bytes, 1024);
    }

    #[test]
    fn test_compute_output_sha_multiple_artifacts() {
        let artifacts = vec![
            ArtifactReceipt {
                name: "a.out".to_string(),
                uri: "gitforge://artifact/1".to_string(),
                sha256: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
                    .to_string(),
                bytes: 100,
                media_type: None,
            },
            ArtifactReceipt {
                name: "b.out".to_string(),
                uri: "gitforge://artifact/2".to_string(),
                sha256: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
                    .to_string(),
                bytes: 200,
                media_type: None,
            },
        ];

        let (sha, bytes) = JobResult::compute_output_sha(&artifacts);
        assert!(!sha.is_empty());
        assert_eq!(bytes, 300); // 100 + 200
    }
}

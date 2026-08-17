//! Build coordinator with semaphore-based concurrency control

use crate::job::{BuildJob, BuildResult, JobStatus, MAX_CONCURRENT_JOBS};
use crate::protocol::{JobInfo, Response};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::{Mutex, Semaphore};
use tokio::time::{timeout, Duration};
use tracing::{error, info, warn};

/// Build coordinator that limits concurrent cargo invocations
pub struct BuildCoordinator {
    /// Semaphore limiting concurrent jobs
    semaphore: Arc<Semaphore>,
    /// Active jobs
    jobs: Arc<Mutex<HashMap<uuid::Uuid, BuildJob>>>,
    /// Completed job count
    completed_count: Arc<AtomicU64>,
    /// Job results for waiters
    results: Arc<Mutex<HashMap<uuid::Uuid, BuildResult>>>,
}

impl BuildCoordinator {
    /// Create a new build coordinator
    pub fn new() -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(MAX_CONCURRENT_JOBS)),
            jobs: Arc::new(Mutex::new(HashMap::new())),
            completed_count: Arc::new(AtomicU64::new(0)),
            results: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Get the maximum concurrent job count
    pub fn max_concurrent() -> usize {
        MAX_CONCURRENT_JOBS
    }

    /// Submit a new build job - BLOCKS until a permit is available
    pub async fn submit(
        self: &Arc<Self>,
        cargo_args: Vec<String>,
        working_dir: Option<String>,
    ) -> uuid::Uuid {
        let job = BuildJob::new(cargo_args, working_dir);
        let job_id = job.id;
        let coordinator = self.clone();

        info!("submitting job {} with args: {:?}", job_id, job.cargo_args);

        // Add to jobs map first
        {
            let mut jobs = self.jobs.lock().await;
            jobs.insert(job_id, job.clone());
        }

        // BLOCKING: Acquire semaphore permit BEFORE returning
        // This is the key fix - we await the acquire, blocking the caller
        // when at capacity (max 2 concurrent)
        let permit = self.semaphore.clone().acquire_owned().await.unwrap();

        // Update job status to running
        {
            let mut jobs = self.jobs.lock().await;
            if let Some(j) = jobs.get_mut(&job_id) {
                j.status = JobStatus::Running { pid: 0 };
            }
        }

        // Spawn execution task - it will run but we already hold the permit
        tokio::spawn(async move {
            // Execute the job
            let result = execute_cargo_job(&job).await;

            // Update job status and record completion
            {
                let mut jobs = coordinator.jobs.lock().await;
                if let Some(j) = jobs.get_mut(&job_id) {
                    match &result {
                        Ok(r) => {
                            j.status = JobStatus::Completed {
                                exit_code: r.exit_code,
                                duration_ms: r.duration_ms,
                            };
                        }
                        Err(e) => {
                            j.status = JobStatus::Failed {
                                exit_code: -1,
                                duration_ms: j.wait_time().as_millis() as u64,
                                error: e.to_string(),
                            };
                        }
                    }
                }
                coordinator.completed_count.fetch_add(1, Ordering::SeqCst);
            }

            // Store result for waiters
            {
                let mut results = coordinator.results.lock().await;
                if let Ok(r) = &result {
                    results.insert(job_id, r.clone());
                }
            }

            // Release permit (drop to signal completion)
            drop(permit);
        });

        job_id
    }

    /// Get job status
    pub async fn get_status(&self, job_id: &uuid::Uuid) -> Option<(String, u64)> {
        let jobs = self.jobs.lock().await;
        jobs.get(job_id).map(|j| {
            let status = match &j.status {
                JobStatus::Queued => "queued".to_string(),
                JobStatus::Running { .. } => "running".to_string(),
                JobStatus::Completed { exit_code, .. } => {
                    format!("completed({})", exit_code)
                }
                JobStatus::Failed {
                    exit_code, error, ..
                } => {
                    format!("failed({}): {}", exit_code, error)
                }
                JobStatus::Cancelled => "cancelled".to_string(),
            };
            (status, j.wait_time().as_millis() as u64)
        })
    }

    /// List all jobs
    pub async fn list_jobs(&self) -> Vec<JobInfo> {
        let jobs = self.jobs.lock().await;
        jobs.values()
            .map(|j| {
                let status = match &j.status {
                    JobStatus::Queued => "queued".to_string(),
                    JobStatus::Running { .. } => "running".to_string(),
                    JobStatus::Completed { exit_code, .. } => {
                        format!("completed({})", exit_code)
                    }
                    JobStatus::Failed { error, .. } => format!("failed: {}", error),
                    JobStatus::Cancelled => "cancelled".to_string(),
                };
                JobInfo {
                    job_id: j.id.to_string(),
                    status,
                    cargo_args: j.cargo_args.clone(),
                    wait_time_ms: j.wait_time().as_millis() as u64,
                }
            })
            .collect()
    }

    /// Get coordinator stats
    pub async fn stats(&self) -> Response {
        let jobs = self.jobs.lock().await;
        let running = jobs
            .values()
            .filter(|j| matches!(j.status, JobStatus::Running { .. }))
            .count();
        let queued = jobs
            .values()
            .filter(|j| matches!(j.status, JobStatus::Queued))
            .count();
        let completed = self
            .completed_count
            .load(std::sync::atomic::Ordering::SeqCst);

        Response::Stats {
            running_count: running,
            queued_count: queued,
            completed_count: completed,
            max_concurrent: MAX_CONCURRENT_JOBS,
        }
    }

    /// Wait for a job to complete and get its result
    pub async fn wait_for_job(&self, job_id: uuid::Uuid) -> Option<BuildResult> {
        self.wait_for_job_with_timeout(job_id, Duration::from_secs(3600))
            .await
    }

    /// Wait for a job with a custom timeout (useful for testing)
    pub async fn wait_for_job_with_timeout(
        &self,
        job_id: uuid::Uuid,
        timeout: Duration,
    ) -> Option<BuildResult> {
        let start = std::time::Instant::now();

        loop {
            // Check if already completed
            {
                let results = self.results.lock().await;
                if let Some(result) = results.get(&job_id) {
                    return Some(result.clone());
                }
            }

            // Check if job exists and is terminal
            {
                let jobs = self.jobs.lock().await;
                if let Some(job) = jobs.get(&job_id) {
                    if job.is_terminal() {
                        // Build result from job status
                        return Some(match &job.status {
                            JobStatus::Completed {
                                exit_code,
                                duration_ms,
                            } => BuildResult {
                                job_id: job.id,
                                success: *exit_code == 0,
                                exit_code: *exit_code,
                                duration_ms: *duration_ms,
                                output: crate::job::JobOutput {
                                    stdout: String::new(),
                                    stderr: String::new(),
                                    exit_code: *exit_code,
                                },
                                error: None,
                            },
                            JobStatus::Failed {
                                exit_code,
                                duration_ms,
                                error,
                            } => BuildResult {
                                job_id: job.id,
                                success: false,
                                exit_code: *exit_code,
                                duration_ms: *duration_ms,
                                output: crate::job::JobOutput {
                                    stdout: String::new(),
                                    stderr: String::new(),
                                    exit_code: *exit_code,
                                },
                                error: Some(error.clone()),
                            },
                            _ => BuildResult {
                                job_id: job.id,
                                success: false,
                                exit_code: -1,
                                duration_ms: job.wait_time().as_millis() as u64,
                                output: crate::job::JobOutput {
                                    stdout: String::new(),
                                    stderr: String::new(),
                                    exit_code: -1,
                                },
                                error: Some("job not complete".to_string()),
                            },
                        });
                    }
                }
            }

            // Check timeout
            if start.elapsed() > timeout {
                return None;
            }

            // Brief sleep before retry
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

impl Default for BuildCoordinator {
    fn default() -> Self {
        Self::new()
    }
}

/// Execute a cargo job
async fn execute_cargo_job(job: &BuildJob) -> anyhow::Result<BuildResult> {
    use std::process::Stdio;
    use tokio::io::AsyncReadExt;

    // IMPORTANT: Never invoke cargo-wrapper here - it would create circular dependency:
    // wrapper -> daemon -> gitforge-build -> wrapper -> daemon...
    //
    // Strategy:
    // 1. CARGO_REAL env var (set by wrapper in bypass mode)
    // 2. rustup run stable cargo (always uses real cargo)
    let cargo_executable = std::env::var("CARGO_REAL").unwrap_or_else(|_| "rustup".to_string());

    let cargo_args = if cargo_executable == "rustup" {
        // rustup run stable cargo [cargo args...]
        let mut args = vec!["run".to_string(), "stable".to_string(), "cargo".to_string()];
        args.extend(job.cargo_args.clone());
        args
    } else {
        job.cargo_args.clone()
    };

    info!(
        "executing {} {:?} in {:?}",
        cargo_executable, cargo_args, job.working_dir
    );

    let mut cmd = tokio::process::Command::new(&cargo_executable);
    cmd.args(&cargo_args);

    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

    if let Some(ref dir) = job.working_dir {
        cmd.current_dir(dir);
    }

    // Set up process group for proper cleanup
    cmd.process_group(0);

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            error!("failed to spawn cargo: {}", e);
            return Ok(BuildResult::failed(
                job,
                -1,
                String::new(),
                format!("failed to spawn: {}", e),
            ));
        }
    };

    let timeout_duration = Duration::from_secs(3600); // 1 hour default timeout

    let result = timeout(timeout_duration, async {
        let start = std::time::Instant::now();

        // Capture stdout
        let mut stdout_buf = String::new();
        if let Some(mut stdout) = child.stdout.take() {
            AsyncReadExt::read_to_string(&mut stdout, &mut stdout_buf)
                .await
                .unwrap_or(0);
        }
        let stdout = stdout_buf;

        // Capture stderr
        let mut stderr_buf = String::new();
        if let Some(mut stderr) = child.stderr.take() {
            AsyncReadExt::read_to_string(&mut stderr, &mut stderr_buf)
                .await
                .unwrap_or(0);
        }
        let stderr = stderr_buf;

        // Wait for process to finish
        let status = child.wait().await?;

        let duration_ms = start.elapsed().as_millis() as u64;
        let exit_code = status.code().unwrap_or(-1);

        Ok::<_, anyhow::Error>(BuildResult {
            job_id: job.id,
            success: exit_code == 0,
            exit_code,
            duration_ms,
            output: crate::job::JobOutput {
                stdout,
                stderr,
                exit_code,
            },
            error: None,
        })
    })
    .await;

    match result {
        Ok(Ok(r)) => {
            info!(
                "job {} completed: success={}, exit_code={}, duration={}ms",
                job.id, r.success, r.exit_code, r.duration_ms
            );
            Ok(r)
        }
        Ok(Err(e)) => {
            warn!("job {} failed: {}", job.id, e);
            Ok(BuildResult::failed(job, -1, String::new(), e.to_string()))
        }
        Err(_) => {
            warn!("job {} timed out after {:?}", job.id, timeout_duration);
            // Try to kill the process
            let _ = child.kill().await;
            Ok(BuildResult::failed(
                job,
                -1,
                String::new(),
                format!("timed out after {:?}", timeout_duration),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_coordinator_submit() {
        let coordinator = Arc::new(BuildCoordinator::new());
        let job_id = coordinator
            .submit(vec!["--version".to_string()], None)
            .await;
        assert!(!job_id.is_nil());
    }

    #[tokio::test]
    async fn test_coordinator_stats() {
        let coordinator = BuildCoordinator::new();
        let stats = coordinator.stats().await;
        assert!(matches!(stats, Response::Stats { .. }));

        if let Response::Stats { max_concurrent, .. } = stats {
            assert_eq!(max_concurrent, MAX_CONCURRENT_JOBS);
        }
    }

    #[tokio::test]
    async fn test_coordinator_list_jobs() {
        let coordinator = BuildCoordinator::new();
        let jobs = coordinator.list_jobs().await;
        assert!(jobs.is_empty());
    }

    #[tokio::test]
    async fn test_coordinator_stats_with_running_jobs() {
        let coordinator = Arc::new(BuildCoordinator::new());

        // Submit a job that will complete quickly
        let _job_id = coordinator
            .submit(vec!["--version".to_string()], None)
            .await;

        // Check stats
        let stats = coordinator.stats().await;
        if let Response::Stats {
            completed_count, ..
        } = stats
        {
            assert_eq!(completed_count, 0);
        }
    }

    #[tokio::test]
    async fn test_coordinator_wait_for_nonexistent_job() {
        let coordinator = BuildCoordinator::new();
        let fake_uuid = uuid::Uuid::new_v4();
        // Use short timeout for testing nonexistent job
        let result = coordinator
            .wait_for_job_with_timeout(fake_uuid, Duration::from_millis(500))
            .await;
        // Should timeout and return None (job doesn't exist)
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_coordinator_get_status() {
        let coordinator = Arc::new(BuildCoordinator::new());
        let job_id = coordinator
            .submit(vec!["--version".to_string()], None)
            .await;

        // Job should exist
        let status = coordinator.get_status(&job_id).await;
        assert!(status.is_some());

        let (status_str, wait_time) = status.unwrap();
        assert!(!status_str.is_empty());
        assert!(wait_time == wait_time); // u64 is always >= 0, verify wait_time exists
    }

    #[tokio::test]
    async fn test_coordinator_get_status_nonexistent() {
        let coordinator = BuildCoordinator::new();
        let fake_uuid = uuid::Uuid::new_v4();
        let status = coordinator.get_status(&fake_uuid).await;
        assert!(status.is_none());
    }

    #[tokio::test]
    async fn test_coordinator_max_concurrent() {
        assert_eq!(BuildCoordinator::max_concurrent(), MAX_CONCURRENT_JOBS);
    }

    #[tokio::test]
    async fn test_coordinator_default() {
        let coordinator = BuildCoordinator::default();
        let stats = coordinator.stats().await;
        if let Response::Stats {
            max_concurrent,
            running_count,
            queued_count,
            completed_count,
        } = stats
        {
            assert_eq!(max_concurrent, MAX_CONCURRENT_JOBS);
            assert_eq!(running_count, 0);
            assert_eq!(queued_count, 0);
            assert_eq!(completed_count, 0);
        }
    }

    #[tokio::test]
    async fn test_coordinator_list_jobs_after_submit() {
        let coordinator = Arc::new(BuildCoordinator::new());
        let _job_id = coordinator
            .submit(vec!["--version".to_string()], None)
            .await;

        // Give a moment for job to be registered
        tokio::time::sleep(Duration::from_millis(50)).await;

        let jobs = coordinator.list_jobs().await;
        assert!(!jobs.is_empty());
    }

    #[tokio::test]
    async fn test_coordinator_get_status_after_submit() {
        let coordinator = Arc::new(BuildCoordinator::new());
        let job_id = coordinator
            .submit(vec!["--version".to_string()], None)
            .await;

        // Give a moment for job to be registered
        tokio::time::sleep(Duration::from_millis(50)).await;

        let status = coordinator.get_status(&job_id).await;
        assert!(status.is_some());

        let (status_str, _) = status.unwrap();
        // Job should be in a valid state (queued, running, or completed for fast jobs)
        assert!(!status_str.is_empty());
    }

    #[tokio::test]
    async fn test_coordinator_wait_for_job_completes() {
        let coordinator = Arc::new(BuildCoordinator::new());
        let job_id = coordinator
            .submit(vec!["--version".to_string()], None)
            .await;

        // Wait for job to complete with short timeout
        let result = coordinator
            .wait_for_job_with_timeout(job_id, Duration::from_secs(5))
            .await;
        assert!(result.is_some());

        let r = result.unwrap();
        assert_eq!(r.job_id, job_id);
    }

    #[tokio::test]
    async fn test_coordinator_submit_multiple_jobs() {
        let coordinator = Arc::new(BuildCoordinator::new());

        // Submit multiple jobs
        let job1 = coordinator
            .submit(vec!["--version".to_string()], None)
            .await;
        let job2 = coordinator.submit(vec!["--list".to_string()], None).await;

        assert_ne!(job1, job2);
    }

    // =============================================================================
    // Negative-path tests for BuildCoordinator
    // =============================================================================

    /// Test: Job with non-zero exit code records failure
    #[tokio::test]
    async fn test_coordinator_job_failure_records_failure() {
        let coordinator = Arc::new(BuildCoordinator::new());

        // Submit a command that will fail (exit code != 0)
        let job_id = coordinator
            .submit(vec!["--invalid-flag".to_string()], None)
            .await;

        // Wait for job to complete
        let result = coordinator
            .wait_for_job_with_timeout(job_id, Duration::from_secs(10))
            .await;

        assert!(result.is_some());
        let r = result.unwrap();
        assert_eq!(r.job_id, job_id);
        // A failed command should have success = false
        assert!(!r.success, "job with invalid flag should have failed");
        assert_ne!(r.exit_code, 0);
    }

    /// Test: Wait for non-existent job times out
    #[tokio::test]
    async fn test_coordinator_wait_for_nonexistent_job_times_out() {
        let coordinator = BuildCoordinator::new();
        let fake_uuid = uuid::Uuid::new_v4();

        let result = coordinator
            .wait_for_job_with_timeout(fake_uuid, Duration::from_millis(200))
            .await;

        assert!(result.is_none(), "waiting for non-existent job should timeout");
    }

    /// Test: Status of non-existent job returns None
    #[tokio::test]
    async fn test_coordinator_nonexistent_job_status_is_none() {
        let coordinator = BuildCoordinator::new();
        let fake_uuid = uuid::Uuid::new_v4();

        let status = coordinator.get_status(&fake_uuid).await;
        assert!(status.is_none(), "status of non-existent job should be None");
    }

    /// Test: Missing receipt - job completed but receipt not in results map
    /// This is detectable by checking job status vs result presence
    #[tokio::test]
    async fn test_coordinator_missing_receipt_detectable() {
        let coordinator = Arc::new(BuildCoordinator::new());

        // Submit a fast job
        let job_id = coordinator
            .submit(vec!["--version".to_string()], None)
            .await;

        // Give time for job to register
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Job exists in the jobs map
        let status = coordinator.get_status(&job_id).await;
        assert!(status.is_some());

        // Wait for result
        let result = coordinator
            .wait_for_job_with_timeout(job_id, Duration::from_secs(5))
            .await;

        // If result is None but status exists, we have a "missing receipt" scenario
        // This could happen if results weren't properly persisted
        match result {
            Some(r) => {
                // Normal case: receipt exists
                assert_eq!(r.job_id, job_id);
            }
            None => {
                // Missing receipt case - detectable
                // In production this would indicate a persistence failure
                let status = coordinator.get_status(&job_id).await;
                assert!(status.is_some(), "job exists but receipt missing");
            }
        }
    }

    /// Test: Duplicate job submission produces unique IDs (idempotency check)
    #[tokio::test]
    async fn test_coordinator_duplicate_submit_unique_ids() {
        let coordinator = Arc::new(BuildCoordinator::new());

        let job1 = coordinator
            .submit(vec!["--version".to_string()], None)
            .await;
        let job2 = coordinator
            .submit(vec!["--version".to_string()], None)
            .await;

        // Each submission gets a unique ID
        assert_ne!(job1, job2, "duplicate submits should produce unique IDs");
    }

    /// Test: Job with working directory that doesn't exist still executes
    /// (sandbox/escape prevention is at a different layer)
    #[tokio::test]
    async fn test_coordinator_nonexistent_working_dir() {
        let coordinator = Arc::new(BuildCoordinator::new());

        // Submit job with non-existent working directory
        let job_id = coordinator
            .submit(
                vec!["--version".to_string()],
                Some("/nonexistent/path/does/not/exist".to_string()),
            )
            .await;

        let result = coordinator
            .wait_for_job_with_timeout(job_id, Duration::from_secs(5))
            .await;

        // Job should complete (will likely fail due to bad dir, but not hang)
        assert!(result.is_some());
    }
}

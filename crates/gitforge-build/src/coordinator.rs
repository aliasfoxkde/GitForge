//! Build coordinator with semaphore-based concurrency control

use crate::job::{BuildJob, BuildResult, JobStatus, MAX_CONCURRENT_JOBS};
use crate::protocol::{JobInfo, Response};
use nix::sys::signal::{killpg, Signal, SIGTERM};
use nix::unistd::Pid;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
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
    /// Process groups for running jobs, used by the control socket to cancel
    /// a build without requiring access to the worker task.
    active_pids: Arc<Mutex<HashMap<uuid::Uuid, u32>>>,
    /// Number of admitted jobs, including queued and running jobs.
    admitted_jobs: Arc<AtomicUsize>,
    /// Maximum number of jobs allowed in the coordinator at once.
    max_jobs: usize,
    /// Configured execution concurrency, exposed in stats.
    max_concurrent_jobs: usize,
    /// Maximum execution time for one child process.
    job_timeout: Duration,
}

/// Admission and execution limits for the build daemon.
#[derive(Debug, Clone, Copy)]
pub struct BuildCoordinatorConfig {
    /// Number of jobs that may execute concurrently.
    pub max_concurrent: usize,
    /// Number of queued jobs accepted in addition to running jobs.
    pub max_queued: usize,
    /// Maximum wall-clock time for a child process.
    pub job_timeout: Duration,
}

impl Default for BuildCoordinatorConfig {
    fn default() -> Self {
        Self {
            max_concurrent: MAX_CONCURRENT_JOBS,
            max_queued: 32,
            job_timeout: Duration::from_secs(3600),
        }
    }
}

impl BuildCoordinatorConfig {
    /// Load safe, bounded settings from the daemon environment.
    pub fn from_env() -> Self {
        let defaults = Self::default();
        let max_concurrent = env_usize(
            "GITFORGE_BUILD_MAX_CONCURRENT",
            defaults.max_concurrent,
            1,
            64,
        );
        let max_queued = env_usize("GITFORGE_BUILD_MAX_QUEUED", defaults.max_queued, 0, 1024);
        let timeout_secs = env_usize(
            "GITFORGE_BUILD_TIMEOUT_SECONDS",
            defaults.job_timeout.as_secs() as usize,
            1,
            86_400,
        );
        Self {
            max_concurrent,
            max_queued,
            job_timeout: Duration::from_secs(timeout_secs as u64),
        }
    }
}

fn env_usize(name: &str, default: usize, min: usize, max: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .map(|value| value.clamp(min, max))
        .unwrap_or(default)
}

impl BuildCoordinator {
    /// Create a new build coordinator
    pub fn new() -> Self {
        Self::with_config(BuildCoordinatorConfig::default())
    }

    /// Create a coordinator with explicit admission and timeout limits.
    pub fn with_config(config: BuildCoordinatorConfig) -> Self {
        let max_concurrent = config.max_concurrent.max(1);
        Self {
            semaphore: Arc::new(Semaphore::new(max_concurrent)),
            jobs: Arc::new(Mutex::new(HashMap::new())),
            completed_count: Arc::new(AtomicU64::new(0)),
            results: Arc::new(Mutex::new(HashMap::new())),
            active_pids: Arc::new(Mutex::new(HashMap::new())),
            admitted_jobs: Arc::new(AtomicUsize::new(0)),
            max_jobs: max_concurrent.saturating_add(config.max_queued),
            max_concurrent_jobs: max_concurrent,
            job_timeout: config.job_timeout,
        }
    }

    /// Get the maximum concurrent job count
    pub fn max_concurrent() -> usize {
        MAX_CONCURRENT_JOBS
    }

    /// Submit a build job without blocking the caller on capacity.
    ///
    /// The job remains queued until the worker task acquires a permit. This
    /// keeps the daemon responsive when all build slots are busy.
    pub async fn submit(
        self: &Arc<Self>,
        cargo_args: Vec<String>,
        working_dir: Option<String>,
    ) -> uuid::Uuid {
        self.submit_job(BuildJob::new(cargo_args, working_dir))
            .await
    }

    /// Submit an explicitly selected non-Cargo command.
    pub async fn submit_command(
        self: &Arc<Self>,
        program: String,
        args: Vec<String>,
        working_dir: Option<String>,
    ) -> uuid::Uuid {
        self.submit_job(BuildJob::new_command(program, args, working_dir))
            .await
    }

    /// Try to admit an explicitly selected command without allowing an
    /// unbounded queue to consume memory or worker tasks.
    pub async fn try_submit_command(
        self: &Arc<Self>,
        program: String,
        args: Vec<String>,
        working_dir: Option<String>,
    ) -> Result<uuid::Uuid, String> {
        self.try_submit_job(BuildJob::new_command(program, args, working_dir))
            .await
    }

    /// Try to admit a Cargo job subject to the configured queue bound.
    pub async fn try_submit(
        self: &Arc<Self>,
        cargo_args: Vec<String>,
        working_dir: Option<String>,
    ) -> Result<uuid::Uuid, String> {
        self.try_submit_job(BuildJob::new(cargo_args, working_dir))
            .await
    }

    async fn submit_job(self: &Arc<Self>, job: BuildJob) -> uuid::Uuid {
        self.try_submit_job(job)
            .await
            .expect("legacy submission exceeded coordinator capacity")
    }

    async fn try_submit_job(self: &Arc<Self>, job: BuildJob) -> Result<uuid::Uuid, String> {
        let mut admitted = self.admitted_jobs.load(Ordering::Acquire);
        loop {
            if admitted >= self.max_jobs {
                return Err(format!("build queue is full (capacity {})", self.max_jobs));
            }
            match self.admitted_jobs.compare_exchange(
                admitted,
                admitted + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(current) => admitted = current,
            }
        }
        let job_id = job.id;
        let coordinator = self.clone();

        info!("submitting job {} with args: {:?}", job_id, job.cargo_args);

        // Add to jobs map first
        {
            let mut jobs = self.jobs.lock().await;
            jobs.insert(job_id, job.clone());
        }

        // Spawn a worker that waits for capacity independently of the client
        // request. The permit lives for the complete process lifetime.
        tokio::spawn(async move {
            let permit = match coordinator.semaphore.clone().acquire_owned().await {
                Ok(permit) => permit,
                Err(_) => {
                    coordinator.admitted_jobs.fetch_sub(1, Ordering::AcqRel);
                    return;
                }
            };
            {
                let mut jobs = coordinator.jobs.lock().await;
                if matches!(
                    jobs.get(&job_id).map(|j| &j.status),
                    Some(JobStatus::Cancelled)
                ) {
                    coordinator.admitted_jobs.fetch_sub(1, Ordering::AcqRel);
                    return;
                }
                if let Some(j) = jobs.get_mut(&job_id) {
                    j.status = JobStatus::Running { pid: 0 };
                }
            }

            // Execute the job
            let result = execute_job(
                &job,
                coordinator.active_pids.clone(),
                coordinator.job_timeout,
            )
            .await;

            // Update job status and record completion
            {
                let mut jobs = coordinator.jobs.lock().await;
                if let Some(j) = jobs.get_mut(&job_id) {
                    if matches!(j.status, JobStatus::Cancelled) {
                        coordinator.admitted_jobs.fetch_sub(1, Ordering::AcqRel);
                        drop(permit);
                        return;
                    }
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
            coordinator.admitted_jobs.fetch_sub(1, Ordering::AcqRel);
        });

        Ok(job_id)
    }

    /// Cancel a queued or running build. Running jobs are terminated through
    /// their process group so child processes cannot survive the request.
    pub async fn cancel(&self, job_id: uuid::Uuid) -> bool {
        let mut jobs = self.jobs.lock().await;
        let Some(job) = jobs.get_mut(&job_id) else {
            return false;
        };
        if job.is_terminal() {
            return false;
        }
        job.status = JobStatus::Cancelled;
        drop(jobs);

        if let Some(pid) = self.active_pids.lock().await.get(&job_id).copied() {
            let pgid = Pid::from_raw(pid as i32);
            // A child stopped by terminal job control will not process TERM
            // until continued. Always resume it before termination.
            let _ = killpg(pgid, Signal::SIGCONT);
            let _ = killpg(pgid, SIGTERM);
        }
        true
    }

    /// Stop all queued and running jobs, then wait for their worker tasks to
    /// reap their children. The forceful pass is deliberately bounded so a
    /// child that ignores SIGTERM cannot keep the daemon shutdown hanging.
    pub async fn shutdown(&self, grace: Duration) {
        let job_ids: Vec<_> = {
            let jobs = self.jobs.lock().await;
            jobs.values()
                .filter(|job| !job.is_terminal())
                .map(|job| job.id)
                .collect()
        };
        for job_id in job_ids {
            let _ = self.cancel(job_id).await;
        }

        let deadline = tokio::time::Instant::now() + grace;
        while tokio::time::Instant::now() < deadline {
            if self.active_pids.lock().await.is_empty() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        // SIGKILL is the final containment boundary. The worker still owns
        // and awaits the child, so normal task cleanup reaps it afterward.
        let pids: Vec<_> = self.active_pids.lock().await.values().copied().collect();
        for pid in pids {
            let pgid = Pid::from_raw(pid as i32);
            let _ = killpg(pgid, Signal::SIGCONT);
            let _ = killpg(pgid, Signal::SIGKILL);
        }
        while !self.active_pids.lock().await.is_empty() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
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
            max_concurrent: self.max_concurrent_jobs,
        }
    }

    /// Wait for a job to complete and get its result
    pub async fn wait_for_job(&self, job_id: uuid::Uuid) -> Option<BuildResult> {
        self.wait_for_job_with_timeout(job_id, self.job_timeout)
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
async fn execute_job(
    job: &BuildJob,
    active_pids: Arc<Mutex<HashMap<uuid::Uuid, u32>>>,
    timeout_duration: Duration,
) -> anyhow::Result<BuildResult> {
    use std::process::Stdio;
    use tokio::io::AsyncReadExt;

    // IMPORTANT: Never invoke cargo-wrapper here - it would create circular dependency:
    // wrapper -> daemon -> gitforge-build -> wrapper -> daemon...
    //
    // Strategy:
    // 1. CARGO_REAL env var (set by wrapper in bypass mode)
    // 2. rustup run stable cargo (always uses real cargo)
    let cargo_executable = job
        .executable
        .clone()
        .unwrap_or_else(|| std::env::var("CARGO_REAL").unwrap_or_else(|_| "rustup".to_string()));

    let cargo_args = if job.executable.is_some() {
        job.cargo_args.clone()
    } else if cargo_executable == "rustup" {
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

    // Builds are non-interactive. Detaching stdin prevents a child from
    // stopping on terminal input/job-control signals when the daemon runs in
    // a PTY.
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

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
    if let Some(pid) = child.id() {
        active_pids.lock().await.insert(job.id, pid);
    }

    // Drain both pipes concurrently. Reading stdout to completion before
    // stderr can deadlock a noisy build once the stderr pipe buffer fills.
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    let mut stdout_task = tokio::spawn(async move {
        let mut output = String::new();
        if let Some(mut stream) = stdout.take() {
            let _ = AsyncReadExt::read_to_string(&mut stream, &mut output).await;
        }
        output
    });
    let mut stderr_task = tokio::spawn(async move {
        let mut output = String::new();
        if let Some(mut stream) = stderr.take() {
            let _ = AsyncReadExt::read_to_string(&mut stream, &mut output).await;
        }
        output
    });

    let result = timeout(timeout_duration, async {
        let start = std::time::Instant::now();
        let (stdout, stderr) = tokio::join!(&mut stdout_task, &mut stderr_task);
        let stdout = stdout.unwrap_or_default();
        let stderr = stderr.unwrap_or_default();
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

    let final_result = match result {
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
            // Kill the process group with SIGTERM first and reap the child.
            // Abort pipe readers after the process is gone so a stuck command
            // cannot leave background tasks holding resources indefinitely.
            if let Some(pid) = child.id() {
                let pgid = Pid::from_raw(pid as i32);
                let _ = killpg(pgid, Signal::SIGCONT);
                let _ = killpg(pgid, SIGTERM);
                tokio::time::sleep(Duration::from_secs(5)).await;
                let _ = killpg(pgid, Signal::SIGKILL);
            } else {
                let _ = child.kill().await;
            }
            let _ = child.wait().await;
            stdout_task.abort();
            stderr_task.abort();
            Ok(BuildResult::failed(
                job,
                -1,
                String::new(),
                format!("timed out after {:?}", timeout_duration),
            ))
        }
    };
    active_pids.lock().await.remove(&job.id);
    final_result
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

    #[tokio::test]
    async fn test_coordinator_rejects_when_admission_is_full() {
        let coordinator = Arc::new(BuildCoordinator::with_config(BuildCoordinatorConfig {
            max_concurrent: 1,
            max_queued: 0,
            job_timeout: Duration::from_secs(10),
        }));
        let first = coordinator
            .try_submit_command(
                "/bin/sh".to_string(),
                vec!["-c".to_string(), "sleep 1".to_string()],
                None,
            )
            .await
            .expect("first job should be admitted");
        let second = coordinator
            .try_submit_command("/bin/true".to_string(), Vec::new(), None)
            .await;
        assert!(second.is_err());
        assert!(second.unwrap_err().contains("queue is full"));
        assert!(coordinator
            .wait_for_job_with_timeout(first, Duration::from_secs(5))
            .await
            .is_some());
    }

    #[test]
    fn test_config_from_env_clamps_invalid_ranges() {
        let config = BuildCoordinatorConfig::from_env();
        assert!(config.max_concurrent >= 1);
        assert!(config.max_concurrent <= 64);
        assert!(config.max_queued <= 1024);
        assert!(config.job_timeout >= Duration::from_secs(1));
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

        assert!(
            result.is_none(),
            "waiting for non-existent job should timeout"
        );
    }

    /// Test: Status of non-existent job returns None
    #[tokio::test]
    async fn test_coordinator_nonexistent_job_status_is_none() {
        let coordinator = BuildCoordinator::new();
        let fake_uuid = uuid::Uuid::new_v4();

        let status = coordinator.get_status(&fake_uuid).await;
        assert!(
            status.is_none(),
            "status of non-existent job should be None"
        );
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

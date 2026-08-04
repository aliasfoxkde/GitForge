//! Build job representation

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Maximum concurrent build jobs
pub const MAX_CONCURRENT_JOBS: usize = 2;

/// Job weight for resource accounting
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum JobWeight {
    /// Lightweight job - compile only
    Light = 1,
    /// Medium job - test, check
    #[default]
    Medium = 2,
    /// Heavy job - coverage, full test suite
    Heavy = 3,
}

impl JobWeight {
    /// Determine job weight from cargo subcommand
    pub fn from_cargo_cmd(cmd: &str) -> Self {
        match cmd {
            "build" | "check" | "clippy" | "fmt" => JobWeight::Light,
            "test" | " bench" => JobWeight::Medium,
            "llvm-cov" | "tarpaulin" | "miri" => JobWeight::Heavy,
            _ => JobWeight::Medium,
        }
    }
}

/// Build job status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum JobStatus {
    /// Job is queued, waiting for a slot
    Queued,
    /// Job is running
    Running { pid: u32 },
    /// Job completed successfully
    Completed { exit_code: i32, duration_ms: u64 },
    /// Job failed
    Failed {
        exit_code: i32,
        duration_ms: u64,
        error: String,
    },
    /// Job was cancelled
    Cancelled,
}

/// A build job to be executed
#[derive(Debug, Clone)]
pub struct BuildJob {
    /// Unique job ID
    pub id: uuid::Uuid,
    /// Cargo command and arguments (e.g., ["test", "--workspace"])
    pub cargo_args: Vec<String>,
    /// Working directory
    pub working_dir: Option<String>,
    /// Job weight (affects resource allocation)
    pub weight: JobWeight,
    /// Job status
    pub status: JobStatus,
    /// When the job was submitted
    pub submitted_at: std::time::Instant,
}

impl BuildJob {
    /// Create a new build job from cargo arguments
    pub fn new(cargo_args: Vec<String>, working_dir: Option<String>) -> Self {
        let cmd = cargo_args.first().map(|s| s.as_str()).unwrap_or("test");
        let weight = JobWeight::from_cargo_cmd(cmd);
        Self {
            id: uuid::Uuid::new_v4(),
            cargo_args,
            working_dir,
            weight,
            status: JobStatus::Queued,
            submitted_at: std::time::Instant::now(),
        }
    }

    /// Get the duration since submission
    pub fn wait_time(&self) -> Duration {
        self.submitted_at.elapsed()
    }

    /// Check if job is terminal (completed, failed, or cancelled)
    pub fn is_terminal(&self) -> bool {
        matches!(
            self.status,
            JobStatus::Completed { .. } | JobStatus::Failed { .. } | JobStatus::Cancelled
        )
    }
}

/// Output from a job step
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobOutput {
    /// Standard output
    pub stdout: String,
    /// Standard error
    pub stderr: String,
    /// Exit code
    pub exit_code: i32,
}

/// Result of a completed build job
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildResult {
    /// Job ID
    pub job_id: uuid::Uuid,
    /// Whether the build succeeded
    pub success: bool,
    /// Exit code
    pub exit_code: i32,
    /// Duration in milliseconds
    pub duration_ms: u64,
    /// Job output (stdout/stderr)
    pub output: JobOutput,
    /// Error message if failed
    pub error: Option<String>,
}

impl BuildResult {
    /// Create a result from a completed job
    pub fn from_job(job: &BuildJob, exit_code: i32, stdout: String, stderr: String) -> Self {
        let duration_ms = job.wait_time().as_millis() as u64;
        let success = exit_code == 0;
        Self {
            job_id: job.id,
            success,
            exit_code,
            duration_ms,
            output: JobOutput {
                stdout,
                stderr,
                exit_code,
            },
            error: None,
        }
    }

    /// Create a failure result
    pub fn failed(job: &BuildJob, exit_code: i32, stderr: String, error: String) -> Self {
        let duration_ms = job.wait_time().as_millis() as u64;
        Self {
            job_id: job.id,
            success: false,
            exit_code,
            duration_ms,
            output: JobOutput {
                stdout: String::new(),
                stderr,
                exit_code,
            },
            error: Some(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_job_weight_from_cargo_cmd() {
        assert_eq!(JobWeight::from_cargo_cmd("build"), JobWeight::Light);
        assert_eq!(JobWeight::from_cargo_cmd("check"), JobWeight::Light);
        assert_eq!(JobWeight::from_cargo_cmd("clippy"), JobWeight::Light);
        assert_eq!(JobWeight::from_cargo_cmd("fmt"), JobWeight::Light);
        assert_eq!(JobWeight::from_cargo_cmd("test"), JobWeight::Medium);
        assert_eq!(JobWeight::from_cargo_cmd("bench"), JobWeight::Medium);
        assert_eq!(JobWeight::from_cargo_cmd("llvm-cov"), JobWeight::Heavy);
        assert_eq!(JobWeight::from_cargo_cmd("tarpaulin"), JobWeight::Heavy);
        assert_eq!(JobWeight::from_cargo_cmd("miri"), JobWeight::Heavy);
        // Unknown defaults to Medium
        assert_eq!(JobWeight::from_cargo_cmd("unknown"), JobWeight::Medium);
    }

    #[test]
    fn test_build_job_new() {
        let job = BuildJob::new(
            vec!["test".to_string(), "--workspace".to_string()],
            Some("/path/to/project".to_string()),
        );
        assert_eq!(job.weight, JobWeight::Medium);
        assert!(matches!(job.status, JobStatus::Queued));
        assert!(!job.id.is_nil());
        assert!(job.wait_time().as_millis() == job.wait_time().as_millis());
    }

    #[test]
    fn test_build_job_new_no_working_dir() {
        let job = BuildJob::new(vec!["build".to_string()], None);
        assert_eq!(job.working_dir, None);
        assert_eq!(job.weight, JobWeight::Light);
    }

    #[test]
    fn test_job_is_terminal() {
        let job = BuildJob::new(vec!["build".to_string()], None);
        assert!(!job.is_terminal());
    }

    #[test]
    fn test_job_status_completed() {
        let mut job = BuildJob::new(vec!["build".to_string()], None);
        job.status = JobStatus::Completed {
            exit_code: 0,
            duration_ms: 100,
        };
        assert!(job.is_terminal());
    }

    #[test]
    fn test_job_status_failed() {
        let mut job = BuildJob::new(vec!["build".to_string()], None);
        job.status = JobStatus::Failed {
            exit_code: 1,
            duration_ms: 50,
            error: "test error".to_string(),
        };
        assert!(job.is_terminal());
    }

    #[test]
    fn test_job_status_cancelled() {
        let mut job = BuildJob::new(vec!["build".to_string()], None);
        job.status = JobStatus::Cancelled;
        assert!(job.is_terminal());
    }

    #[test]
    fn test_job_status_running() {
        let mut job = BuildJob::new(vec!["build".to_string()], None);
        job.status = JobStatus::Running { pid: 12345 };
        assert!(!job.is_terminal());
    }

    #[test]
    fn test_build_result_from_job_success() {
        let job = BuildJob::new(vec!["build".to_string()], None);
        let result = BuildResult::from_job(&job, 0, "output".to_string(), "".to_string());
        assert!(result.success);
        assert_eq!(result.exit_code, 0);
    }

    #[test]
    fn test_build_result_from_job_failure() {
        let job = BuildJob::new(vec!["build".to_string()], None);
        let result = BuildResult::from_job(&job, 1, "".to_string(), "error".to_string());
        assert!(!result.success);
        assert_eq!(result.exit_code, 1);
    }

    #[test]
    fn test_build_result_failed() {
        let job = BuildJob::new(vec!["build".to_string()], None);
        let result =
            BuildResult::failed(&job, -1, "stderr".to_string(), "spawn failed".to_string());
        assert!(!result.success);
        assert_eq!(result.exit_code, -1);
        assert!(result.error.is_some());
    }
}

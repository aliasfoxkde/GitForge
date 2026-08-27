//! GitForge Build CLI
//!
//! Client for submitting build jobs to the gitforge-buildd daemon.

use anyhow::Result;
use clap::Parser;

use gitforge_build::client::{JobSubmitter, UnixSocketClient, DEFAULT_SOCKET};
use gitforge_build::Response;

#[derive(Parser, Debug)]
#[command(
    name = "gitforge-build",
    about = "GitForge build coordinator CLI",
    long_about = None,
)]
struct Cli {
    /// Socket path (default: /tmp/gitforge-build.sock)
    #[arg(long)]
    socket: Option<String>,

    /// Working directory
    #[arg(short, long)]
    dir: Option<String>,

    /// Just submit and don't wait for result
    #[arg(short = 'n', long)]
    no_wait: bool,

    /// List all jobs
    #[arg(short, long)]
    list: bool,

    /// Show daemon stats
    #[arg(long)]
    stats: bool,

    /// Cancel a queued or running job
    #[arg(long, value_name = "JOB_ID")]
    cancel: Option<String>,

    /// Gracefully stop the build daemon
    #[arg(long)]
    shutdown: bool,

    /// cargo command and arguments (e.g., "test --workspace"); options after
    /// the cargo command are forwarded without requiring a second `--`.
    #[arg(trailing_var_arg = true)]
    cargo_args: Vec<String>,
}

/// Main entry point - uses real UnixSocketClient
#[tokio::main]
async fn main() -> Result<()> {
    run_with_client(&UnixSocketClient::new()).await
}

/// Run the CLI with a given client implementation (allows mocking in tests)
pub async fn run_with_client<C: JobSubmitter>(client: &C) -> Result<()> {
    let cli = Cli::parse();
    let socket_path = cli.socket.unwrap_or_else(|| DEFAULT_SOCKET.to_string());

    // If just listing or showing stats, handle specially
    if cli.list {
        return list_jobs_cmd(client, &socket_path).await;
    }

    if cli.stats {
        return stats_cmd(client, &socket_path).await;
    }

    if let Some(job_id) = cli.cancel {
        return cancel_cmd(client, &socket_path, job_id).await;
    }

    if cli.shutdown {
        return shutdown_cmd(client, &socket_path).await;
    }

    // Need at least one cargo arg
    if cli.cargo_args.is_empty() {
        anyhow::bail!("no cargo command specified. Usage: gitforge-build <cargo args...>");
    }

    // Determine working directory
    let working_dir = if let Some(ref dir) = cli.dir {
        Some(dir.clone())
    } else {
        std::env::current_dir()
            .ok()
            .map(|p| p.to_string_lossy().to_string())
    };

    // Submit job
    let response = client
        .submit_job(&socket_path, cli.cargo_args.clone(), working_dir)
        .await?;

    match response {
        Response::Submitted { job_id } => {
            println!("submitted job: {}", job_id);

            if cli.no_wait {
                return Ok(());
            }

            // Wait for job to complete
            println!("waiting for job to complete...");
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                match client.get_status(&socket_path, job_id.clone()).await? {
                    Response::Status { status, .. } => {
                        println!("status: {}", status);
                        if status.starts_with("completed(")
                            || status.starts_with("failed")
                            || status == "cancelled"
                        {
                            if status.starts_with("failed") || status == "cancelled" {
                                anyhow::bail!("job {}", status);
                            }
                            return Ok(());
                        }
                    }
                    Response::Error { message } => anyhow::bail!("error: {}", message),
                    response => anyhow::bail!("unexpected response: {:?}", response),
                }
            }
        }
        Response::Error { message } => {
            anyhow::bail!("error: {}", message);
        }
        _ => {
            anyhow::bail!("unexpected response: {:?}", response);
        }
    }
}

async fn cancel_cmd<C: JobSubmitter>(client: &C, socket_path: &str, job_id: String) -> Result<()> {
    match client.cancel_job(socket_path, job_id).await? {
        Response::Status { status, .. } => {
            println!("{}", status);
            Ok(())
        }
        Response::Error { message } => anyhow::bail!("error: {}", message),
        response => anyhow::bail!("unexpected response: {:?}", response),
    }
}

async fn shutdown_cmd<C: JobSubmitter>(client: &C, socket_path: &str) -> Result<()> {
    match client.shutdown(socket_path).await? {
        Response::Shutdown => {
            println!("shutdown requested");
            Ok(())
        }
        Response::Error { message } => anyhow::bail!("error: {}", message),
        response => anyhow::bail!("unexpected response: {:?}", response),
    }
}

async fn list_jobs_cmd<C: JobSubmitter>(client: &C, socket_path: &str) -> Result<()> {
    let response = client.list_jobs(socket_path).await?;

    match response {
        Response::JobList { jobs } => {
            if jobs.is_empty() {
                println!("no jobs");
            } else {
                for job in jobs {
                    println!(
                        "{} {:15} {:8} (wait: {}ms) {:?}",
                        job.job_id, job.status, "", job.wait_time_ms, job.cargo_args
                    );
                }
            }
            Ok(())
        }
        Response::Error { message } => {
            if message.contains("No such file") {
                anyhow::bail!("daemon not running. Start with: gitforge-buildd");
            }
            anyhow::bail!("error: {}", message);
        }
        _ => anyhow::bail!("unexpected response"),
    }
}

async fn stats_cmd<C: JobSubmitter>(client: &C, socket_path: &str) -> Result<()> {
    let response = client.get_stats(socket_path).await?;

    match response {
        Response::Stats {
            running_count,
            queued_count,
            completed_count,
            max_concurrent,
        } => {
            println!("GitForge Build Daemon");
            println!("====================");
            println!("max concurrent: {}", max_concurrent);
            println!("running:        {}", running_count);
            println!("queued:         {}", queued_count);
            println!("completed:      {}", completed_count);
            Ok(())
        }
        Response::Error { message } => {
            if message.contains("No such file") {
                anyhow::bail!("daemon not running. Start with: gitforge-buildd");
            }
            anyhow::bail!("error: {}", message);
        }
        _ => anyhow::bail!("unexpected response"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gitforge_build::client::{JobSubmitter, MockClient};
    use gitforge_build::{JobInfo, Response};

    #[tokio::test]
    async fn test_run_with_client_submit_success() {
        let client = MockClient::with_submit_response(Response::Submitted {
            job_id: "test-job-123".to_string(),
        });

        let result = client
            .submit_job(
                "/fake/socket",
                vec!["build".to_string()],
                Some("/test/dir".to_string()),
            )
            .await;
        assert!(result.is_ok());

        if let Ok(Response::Submitted { job_id }) = result {
            assert_eq!(job_id, "test-job-123");
        }
    }

    #[tokio::test]
    async fn test_run_with_client_submit_with_error() {
        let client = MockClient::with_submit_response(Response::Error {
            message: "build failed".to_string(),
        });

        let result = client
            .submit_job("/fake/socket", vec!["build".to_string()], None)
            .await;
        assert!(result.is_ok());

        if let Ok(Response::Error { message }) = result {
            assert_eq!(message, "build failed");
        }
    }

    #[tokio::test]
    async fn test_run_with_client_list_empty() {
        let client = MockClient::with_submit_response(Response::Submitted {
            job_id: "test".to_string(),
        });

        let result = client.list_jobs("/fake/socket").await;
        assert!(result.is_ok());

        if let Ok(Response::JobList { jobs }) = result {
            assert!(jobs.is_empty());
        }
    }

    #[tokio::test]
    async fn test_run_with_client_list_with_jobs() {
        let client = MockClient::new(
            Response::Submitted {
                job_id: "test".to_string(),
            },
            Response::JobList {
                jobs: vec![
                    JobInfo {
                        job_id: "job-1".to_string(),
                        status: "running".to_string(),
                        cargo_args: vec!["build".to_string()],
                        wait_time_ms: 100,
                    },
                    JobInfo {
                        job_id: "job-2".to_string(),
                        status: "queued".to_string(),
                        cargo_args: vec!["test".to_string()],
                        wait_time_ms: 500,
                    },
                ],
            },
            Response::Stats {
                running_count: 1,
                queued_count: 1,
                completed_count: 0,
                max_concurrent: 2,
            },
        );

        let result = client.list_jobs("/fake/socket").await;
        assert!(result.is_ok());

        if let Ok(Response::JobList { jobs }) = result {
            assert_eq!(jobs.len(), 2);
        }
    }

    #[tokio::test]
    async fn test_run_with_client_stats() {
        let client = MockClient::new(
            Response::Submitted {
                job_id: "test".to_string(),
            },
            Response::JobList { jobs: vec![] },
            Response::Stats {
                running_count: 2,
                queued_count: 5,
                completed_count: 100,
                max_concurrent: 4,
            },
        );

        let result = client.get_stats("/fake/socket").await;
        assert!(result.is_ok());

        if let Ok(Response::Stats {
            running_count,
            queued_count,
            completed_count,
            max_concurrent,
        }) = result
        {
            assert_eq!(running_count, 2);
            assert_eq!(queued_count, 5);
            assert_eq!(completed_count, 100);
            assert_eq!(max_concurrent, 4);
        }
    }

    #[tokio::test]
    async fn test_run_with_client_list_error() {
        let client = MockClient::new(
            Response::Submitted {
                job_id: "test".to_string(),
            },
            Response::Error {
                message: "connection lost".to_string(),
            },
            Response::Stats {
                running_count: 0,
                queued_count: 0,
                completed_count: 0,
                max_concurrent: 0,
            },
        );

        let result = client.list_jobs("/fake/socket").await;
        assert!(result.is_ok());

        if let Ok(Response::Error { message }) = result {
            assert_eq!(message, "connection lost");
        }
    }

    #[tokio::test]
    async fn test_run_with_client_stats_error() {
        let client = MockClient::new(
            Response::Submitted {
                job_id: "test".to_string(),
            },
            Response::JobList { jobs: vec![] },
            Response::Error {
                message: "socket closed".to_string(),
            },
        );

        let result = client.get_stats("/fake/socket").await;
        assert!(result.is_ok());

        if let Ok(Response::Error { message }) = result {
            assert_eq!(message, "socket closed");
        }
    }

    #[tokio::test]
    async fn test_run_with_client_completed_response() {
        let client = MockClient::with_submit_response(Response::Completed {
            job_id: "completed-job".to_string(),
            success: true,
            exit_code: 0,
            duration_ms: 5000,
            stdout: "Build successful".to_string(),
            stderr: "".to_string(),
        });

        let result = client.submit_job("/fake", vec![], None).await;
        assert!(result.is_ok());

        if let Ok(Response::Completed {
            job_id,
            success,
            exit_code,
            ..
        }) = result
        {
            assert_eq!(job_id, "completed-job");
            assert!(success);
            assert_eq!(exit_code, 0);
        }
    }

    #[test]
    fn test_cli_parse_submit() {
        let cli = Cli::try_parse_from(["gitforge-build", "--", "build", "--release"]).unwrap();
        assert!(!cli.list);
        assert!(!cli.stats);
        assert_eq!(cli.cargo_args, vec!["build", "--release"]);
    }

    #[test]
    fn test_cli_parse_list() {
        let cli = Cli::try_parse_from(["gitforge-build", "-l"]).unwrap();
        assert!(cli.list);
        assert!(!cli.stats);
        assert!(cli.cargo_args.is_empty());
    }

    #[test]
    fn test_cli_parse_stats() {
        let cli = Cli::try_parse_from(["gitforge-build", "--stats"]).unwrap();
        assert!(!cli.list);
        assert!(cli.stats);
        assert!(cli.cargo_args.is_empty());
    }

    #[test]
    fn test_cli_parse_with_socket() {
        let cli =
            Cli::try_parse_from(["gitforge-build", "--socket", "/custom/sock", "-l"]).unwrap();
        assert_eq!(cli.socket, Some("/custom/sock".to_string()));
    }

    #[test]
    fn test_cli_parse_with_dir() {
        let cli = Cli::try_parse_from(["gitforge-build", "-d", "/work/dir", "--", "test"]).unwrap();
        assert_eq!(cli.dir, Some("/work/dir".to_string()));
        assert_eq!(cli.cargo_args, vec!["test"]);
    }

    #[test]
    fn test_cli_parse_no_wait() {
        let cli = Cli::try_parse_from(["gitforge-build", "-n", "--", "build"]).unwrap();
        assert!(cli.no_wait);
    }

    #[test]
    fn test_cli_cargo_args_multiple() {
        let cli = Cli::try_parse_from([
            "gitforge-build",
            "--",
            "check",
            "-p",
            "foo",
            "--all-targets",
        ])
        .unwrap();
        assert_eq!(cli.cargo_args, vec!["check", "-p", "foo", "--all-targets"]);
    }

    #[test]
    fn test_cli_forwards_cargo_options_without_separator() {
        let cli = Cli::try_parse_from(["gitforge-build", "test", "--workspace"]).unwrap();
        assert_eq!(cli.cargo_args, vec!["test", "--workspace"]);
    }

    #[test]
    fn test_cli_parse_all_flags() {
        let cli = Cli::try_parse_from([
            "gitforge-build",
            "--socket",
            "/my/sock",
            "-d",
            "/work",
            "-n",
            "-l",
            "--",
            "build",
        ])
        .unwrap();
        assert_eq!(cli.socket, Some("/my/sock".to_string()));
        assert_eq!(cli.dir, Some("/work".to_string()));
        assert!(cli.no_wait);
        assert!(cli.list);
        assert_eq!(cli.cargo_args, vec!["build"]);
    }
}

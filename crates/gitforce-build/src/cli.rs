//! GitForge Build CLI
//!
//! Client for submitting build jobs to the gitforge-buildd daemon.

use anyhow::Result;
use clap::Parser;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

use gitforce_build::{encode_request, Request, Response};

const DEFAULT_SOCKET: &str = "/tmp/gitforge-build.sock";

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

    /// cargo command and arguments (e.g., "test --workspace")
    cargo_args: Vec<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let socket_path = cli.socket.unwrap_or_else(|| DEFAULT_SOCKET.to_string());

    // If just listing or showing stats, handle specially
    if cli.list {
        return list_jobs(&socket_path).await;
    }

    if cli.stats {
        return show_stats(&socket_path).await;
    }

    // Need at least one cargo arg
    if cli.cargo_args.is_empty() {
        anyhow::bail!("no cargo command specified. Usage: gitforge-build <cargo args...>");
    }

    // Connect to daemon
    let mut stream = UnixStream::connect(&socket_path).await?;

    // Determine working directory
    let working_dir = if let Some(ref dir) = cli.dir {
        Some(dir.clone())
    } else {
        std::env::current_dir().ok().map(|p| p.to_string_lossy().to_string())
    };

    // Build submit request
    let request = Request::Submit {
        cargo_args: cli.cargo_args.clone(),
        working_dir,
    };

    // Send request
    let request_bytes = encode_request(&request)?;
    stream.write_all(&request_bytes).await?;
    // Shutdown write side to signal we're done sending
    stream.shutdown().await?;

    // Read response - first 4 bytes are length prefix
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).await?;
    let len = u32::from_le_bytes(len_buf) as usize;
    let mut response_bytes = vec![0u8; len];
    stream.read_exact(&mut response_bytes).await?;

    // Decode response - response_bytes is already the JSON (length was read above)
    let response: Response = serde_json::from_slice(&response_bytes)?;

    match response {
        Response::Submitted { job_id } => {
            println!("submitted job: {}", job_id);

            if cli.no_wait {
                return Ok(());
            }

            // Wait for job to complete
            println!("waiting for job to complete...");
            // For now just print that it was submitted
            // In full implementation, would poll for status
            Ok(())
        }
        Response::Error { message } => {
            anyhow::bail!("error: {}", message);
        }
        _ => {
            anyhow::bail!("unexpected response: {:?}", response);
        }
    }
}

async fn list_jobs(socket_path: &str) -> Result<()> {
    let mut stream = UnixStream::connect(socket_path).await?;

    let request = Request::List;
    let request_bytes = encode_request(&request)?;
    stream.write_all(&request_bytes).await?;
    stream.shutdown().await?;

    // Read response - first 4 bytes are length prefix
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).await?;
    let len = u32::from_le_bytes(len_buf) as usize;
    let mut response_bytes = vec![0u8; len];
    stream.read_exact(&mut response_bytes).await?;

    let response: Response = serde_json::from_slice(&response_bytes)?;

    match response {
        Response::JobList { jobs } => {
            if jobs.is_empty() {
                println!("no jobs");
            } else {
                for job in jobs {
                    println!(
                        "{} {:15} {:8} (wait: {}ms) {:?}",
                        job.job_id,
                        job.status,
                        "",
                        job.wait_time_ms,
                        job.cargo_args
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

async fn show_stats(socket_path: &str) -> Result<()> {
    let mut stream = UnixStream::connect(socket_path).await?;

    let request = Request::Stats;
    let request_bytes = encode_request(&request)?;
    stream.write_all(&request_bytes).await?;
    stream.shutdown().await?;

    // Read response - first 4 bytes are length prefix
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).await?;
    let len = u32::from_le_bytes(len_buf) as usize;
    let mut response_bytes = vec![0u8; len];
    stream.read_exact(&mut response_bytes).await?;

    let response: Response = serde_json::from_slice(&response_bytes)?;

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

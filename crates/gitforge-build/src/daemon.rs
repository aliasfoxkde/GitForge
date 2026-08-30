//! GitForge Build Daemon
//!
//! Unix socket-based daemon that coordinates cargo builds with semaphore limiting.

use anyhow::Result;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tracing::{error, info, warn};

use gitforge_build::{
    configured_socket_path, encode_response, BuildCoordinator, Request, Response,
    MAX_CONCURRENT_JOBS, MAX_MESSAGE_SIZE,
};

/// Create a shutdown flag
pub fn create_shutdown_flag() -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(false))
}

/// Create the shutdown future that waits for shutdown signal
pub async fn create_shutdown_future(shutdown: Arc<AtomicBool>) {
    while !shutdown.load(Ordering::SeqCst) {
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }
}

fn handle_cli_args() -> bool {
    let mut args = std::env::args().skip(1);
    let Some(arg) = args.next() else {
        return false;
    };

    match arg.as_str() {
        "--help" | "-h" => {
            println!("GitForge build daemon\\n\\nUsage: gitforge-buildd\\n\\nOptions:\\n  -h, --help       Print help\\n  -V, --version    Print version");
            true
        }
        "--version" | "-V" => {
            println!("gitforge-buildd {}", env!("CARGO_PKG_VERSION"));
            true
        }
        _ => false,
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    if handle_cli_args() {
        return Ok(());
    }

    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .init();

    info!("starting gitforge-buildd daemon");
    info!("max concurrent jobs: {}", MAX_CONCURRENT_JOBS);

    // Initialize process supervision (subreaper + SIGCHLD)
    if let Err(e) = gitforge_process::init_without_sigchld_reaper() {
        warn!("failed to initialize process supervision: {}", e);
    }

    // Create build coordinator
    let coordinator = Arc::new(BuildCoordinator::new());

    let socket_path = configured_socket_path();

    // Clean up old socket
    if Path::new(&socket_path).exists() {
        std::fs::remove_file(&socket_path)?;
    }

    // Create Unix socket listener
    let listener = UnixListener::bind(&socket_path)?;

    // Set socket permissions (owner only)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600))?;
    }

    info!("listening on {}", socket_path);

    // Handle connections. The socket shutdown request and OS signal share the
    // same flag so operators can use the manager to stop the daemon without
    // needing a second process-control channel.
    let shutdown_flag = Arc::new(AtomicBool::new(false));
    let signal_shutdown = shutdown_flag.clone();
    let shutdown_signal = async {
        tokio::signal::ctrl_c().await.ok();
        signal_shutdown.store(true, Ordering::SeqCst);
        info!("ctrl-c received, shutting down");
    };

    tokio::pin!(shutdown_signal);

    loop {
        tokio::select! {
            result = listener.accept() => {
                match result {
                    Ok((stream, _)) => {
                        let coordinator = coordinator.clone();
                        let shutdown = shutdown_flag.clone();
                        tokio::spawn(handle_connection(stream, coordinator, shutdown));
                    }
                    Err(e) => {
                        error!("accept error: {}", e);
                        break;
                    }
                }
            }
            _ = &mut shutdown_signal => {
                break;
            }
            _ = create_shutdown_future(shutdown_flag.clone()) => {
                break;
            }
        }
    }

    // Close socket
    drop(listener);
    let _ = std::fs::remove_file(&socket_path);

    // Do not leave cargo, rustc, or test descendants orphaned when the
    // control daemon is stopped. Give cooperative cancellation a short
    // window, then let the coordinator force-kill and reap survivors.
    coordinator
        .shutdown(tokio::time::Duration::from_secs(5))
        .await;

    info!("gitforge-buildd stopped");
    Ok(())
}

/// Handle a single connection
async fn handle_connection(
    stream: UnixStream,
    coordinator: Arc<BuildCoordinator>,
    shutdown: Arc<AtomicBool>,
) {
    let (mut reader, mut writer) = tokio::io::split(stream);

    // Read request - first 4 bytes are length prefix
    let mut len_buf = [0u8; 4];
    if let Err(e) = reader.read_exact(&mut len_buf).await {
        error!("read error: {}", e);
        return;
    }
    let len = u32::from_le_bytes(len_buf) as usize;
    if len > MAX_MESSAGE_SIZE {
        error!("request exceeds maximum size: {} bytes", len);
        return;
    }
    let mut bytes = vec![0u8; len];
    if let Err(e) = reader.read_exact(&mut bytes).await {
        error!("read error: {}", e);
        return;
    }

    // Decode request - bytes already contains the JSON (after length prefix)
    let request: Request = match serde_json::from_slice(&bytes) {
        Ok(req) => req,
        Err(e) => {
            error!("decode error: {}", e);
            return;
        }
    };

    info!("received request: {}", request.name());

    let response: Response = match request {
        Request::Submit {
            cargo_args,
            working_dir,
        } => {
            let job_id = coordinator.submit(cargo_args, working_dir).await;
            Response::Submitted {
                job_id: job_id.to_string(),
            }
        }
        Request::Status { job_id } => {
            if let Ok(uuid) = uuid::Uuid::parse_str(&job_id) {
                if let Some((status, wait_time_ms)) = coordinator.get_status(&uuid).await {
                    Response::Status {
                        job_id,
                        status,
                        wait_time_ms,
                    }
                } else {
                    Response::Error {
                        message: "job not found".to_string(),
                    }
                }
            } else {
                Response::Error {
                    message: "invalid job id".to_string(),
                }
            }
        }
        Request::Cancel { job_id } => match uuid::Uuid::parse_str(&job_id) {
            Ok(job_id) if coordinator.cancel(job_id).await => Response::Status {
                job_id: job_id.to_string(),
                status: "cancelled".to_string(),
                wait_time_ms: 0,
            },
            Ok(_) => Response::Error {
                message: "job not found or already terminal".to_string(),
            },
            Err(_) => Response::Error {
                message: "invalid job id".to_string(),
            },
        },
        Request::List => {
            let jobs = coordinator.list_jobs().await;
            Response::JobList { jobs }
        }
        Request::Stats => coordinator.stats().await,
        Request::Shutdown => {
            info!("shutdown requested via socket");
            shutdown.store(true, Ordering::SeqCst);
            Response::Shutdown
        }
    };

    // Send response
    let response_bytes = match encode_response(&response) {
        Ok(b) => b,
        Err(e) => {
            error!("encode error: {}", e);
            return;
        }
    };

    if let Err(e) = writer.write_all(&response_bytes).await {
        error!("write error: {}", e);
    }
    // Close write half to signal end of response
    drop(writer);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_shutdown_flag_initial_state() {
        let flag = create_shutdown_flag();
        assert!(!flag.load(Ordering::SeqCst));
    }

    #[test]
    fn test_create_shutdown_flag_clone() {
        let flag1 = create_shutdown_flag();
        let flag2 = flag1.clone();
        flag1.store(true, Ordering::SeqCst);
        assert!(flag2.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn test_create_shutdown_future() {
        let shutdown = create_shutdown_flag();
        let shutdown_flag = shutdown.clone();

        // Set shutdown after a short delay
        tokio::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
            shutdown_flag.store(true, Ordering::SeqCst);
        });

        create_shutdown_future(shutdown).await;
    }

    #[test]
    fn test_shutdown_flag_is_atomic() {
        let flag = create_shutdown_flag();
        assert!(!flag.load(Ordering::SeqCst));
        flag.store(true, Ordering::SeqCst);
        assert!(flag.load(Ordering::SeqCst));
    }
}

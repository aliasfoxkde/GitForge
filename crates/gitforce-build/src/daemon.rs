//! GitForge Build Daemon
//!
//! Unix socket-based daemon that coordinates cargo builds with semaphore limiting.

use anyhow::Result;
use std::path::Path;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tracing::{error, info, warn};

use gitforce_build::{encode_response, BuildCoordinator, Request, Response, MAX_CONCURRENT_JOBS};

/// Socket path for the daemon
const SOCKET_PATH: &str = "/tmp/gitforge-build.sock";

#[tokio::main]
async fn main() -> Result<()> {
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
    if let Err(e) = gitforce_process::init() {
        warn!("failed to initialize process supervision: {}", e);
    }

    // Create build coordinator
    let coordinator = Arc::new(BuildCoordinator::new());

    // Clean up old socket
    if Path::new(SOCKET_PATH).exists() {
        std::fs::remove_file(SOCKET_PATH)?;
    }

    // Create Unix socket listener
    let listener = UnixListener::bind(SOCKET_PATH)?;

    // Set socket permissions (owner only)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(SOCKET_PATH, std::fs::Permissions::from_mode(0o600))?;
    }

    info!("listening on {}", SOCKET_PATH);

    // Handle connections
    let shutdown = async {
        tokio::signal::ctrl_c().await.ok();
        info!("ctrl-c received, shutting down");
    };

    tokio::pin!(shutdown);

    loop {
        tokio::select! {
            result = listener.accept() => {
                match result {
                    Ok((stream, _)) => {
                        let coordinator = coordinator.clone();
                        tokio::spawn(handle_connection(stream, coordinator));
                    }
                    Err(e) => {
                        error!("accept error: {}", e);
                        break;
                    }
                }
            }
            _ = &mut shutdown => {
                break;
            }
        }
    }

    // Close socket
    drop(listener);
    let _ = std::fs::remove_file(SOCKET_PATH);

    info!("gitforge-buildd stopped");
    Ok(())
}

/// Handle a single connection
async fn handle_connection(stream: UnixStream, coordinator: Arc<BuildCoordinator>) {
    let (mut reader, mut writer) = tokio::io::split(stream);

    // Read request - first 4 bytes are length prefix
    let mut len_buf = [0u8; 4];
    if let Err(e) = reader.read_exact(&mut len_buf).await {
        error!("read error: {}", e);
        return;
    }
    let len = u32::from_le_bytes(len_buf) as usize;
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
        Request::Cancel { job_id: _ } => {
            Response::Error {
                message: "cancel not implemented".to_string(),
            }
        }
        Request::List => {
            let jobs = coordinator.list_jobs().await;
            Response::JobList { jobs }
        }
        Request::Stats => coordinator.stats().await,
        Request::Shutdown => {
            info!("shutdown requested via socket");
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

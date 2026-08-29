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
    encode_response, BuildCoordinator, Request, Response, DEFAULT_SOCKET, MAX_MESSAGE_SIZE,
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
    let config = gitforge_build::coordinator::BuildCoordinatorConfig::from_env();
    info!(
        "build limits: max_concurrent={}, max_queued={}, timeout={:?}",
        config.max_concurrent, config.max_queued, config.job_timeout
    );
    let socket_path =
        std::env::var("GITFORGE_BUILD_SOCKET").unwrap_or_else(|_| DEFAULT_SOCKET.to_string());

    // Initialize process supervision (subreaper + SIGCHLD)
    if let Err(e) = gitforge_process::init_without_sigchld_reaper() {
        warn!("failed to initialize process supervision: {}", e);
    }

    // Create build coordinator
    let coordinator = if let Some(path) = config.journal_path.clone() {
        BuildCoordinator::with_journal(config, path).await?
    } else {
        Arc::new(BuildCoordinator::with_config(config))
    };

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
        } => match coordinator.try_submit(cargo_args, working_dir).await {
            Ok(job_id) => Response::Submitted {
                job_id: job_id.to_string(),
            },
            Err(message) => Response::Error { message },
        },
        Request::Exec {
            program,
            args,
            working_dir,
        } => {
            match coordinator
                .try_submit_command(program, args, working_dir)
                .await
            {
                Ok(job_id) => Response::Submitted {
                    job_id: job_id.to_string(),
                },
                Err(message) => Response::Error { message },
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

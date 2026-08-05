//! Signal handling for process supervision
//!
//! This module provides proper SIGCHLD handling to prevent zombie processes
//! by actively reaping terminated child processes.

#[cfg(target_os = "linux")]
use libc::{waitpid, WEXITSTATUS, WIFEXITED, WIFSIGNALED, WNOHANG, WTERMSIG};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

static SIGCHLD_HANDLER_STARTED: AtomicBool = AtomicBool::new(false);

/// Spawn a shutdown signal handler that waits for SIGINT (Ctrl+C) or SIGTERM
/// and sets the shutdown flag when received.
///
/// This function properly handles signal registration errors instead of panicking.
///
/// # Arguments
/// * `shutdown_flag` - An atomic boolean that will be set to true when shutdown is requested
///
/// # Example
/// ```ignore
/// use std::sync::atomic::{AtomicBool, Ordering};
/// use std::sync::Arc;
///
/// let shutdown_flag = Arc::new(AtomicBool::new(false));
/// gitforge_process::signal::spawn_shutdown_handler(shutdown_flag.clone());
/// ```
#[cfg(unix)]
pub fn spawn_shutdown_handler(shutdown_flag: Arc<AtomicBool>) {
    tokio::spawn(async move {
        // Use ctrl_c() for SIGINT which handles Ctrl+C gracefully
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("received Ctrl+C, initiating graceful shutdown...");
            }
            _ = async {
                // For SIGTERM, we need to use the signal crate directly
                // which can fail if signal is already used by another handler
                match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                    Ok(mut sigterm) => sigterm.recv().await,
                    Err(e) => {
                        tracing::error!("failed to register SIGTERM handler: {}, continuing without SIGTERM support", e);
                        // Wait forever by sleeping - caller should use other means to shutdown
                        loop {
                            tokio::time::sleep(Duration::MAX).await;
                        }
                    }
                }
            } => {
                tracing::info!("received SIGTERM, initiating graceful shutdown...");
            }
        }
        shutdown_flag.store(true, Ordering::SeqCst);
    });
}

/// Spawn a shutdown signal handler for Windows (no-op since signals work differently)
#[cfg(windows)]
pub fn spawn_shutdown_handler(_shutdown_flag: Arc<AtomicBool>) {
    // On Windows, we don't have meaningful signal handling for graceful shutdown
    // The process will be terminated by the OS
    tracing::debug!("shutdown signal handler not available on Windows");
}

/// Create a shutdown future that waits for shutdown signal to be set
pub async fn wait_for_shutdown(shutdown: Arc<AtomicBool>) {
    while !shutdown.load(Ordering::SeqCst) {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    tracing::info!("shutdown signal detected");
}

/// Create a shutdown flag for signaling graceful shutdown
pub fn create_shutdown_flag() -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(false))
}

/// Install a SIGCHLD handler that actively reaps child processes.
///
/// This spawns a background thread that waits for child processes,
/// preventing them from becoming zombies.
///
/// On non-Linux platforms, this is a no-op since waitpid is not available.
///
/// # Example
/// ```
/// gitforge_process::install_sigchld_handler();
/// ```
#[cfg(target_os = "linux")]
pub fn install_sigchld_handler() {
    if SIGCHLD_HANDLER_STARTED.swap(true, Ordering::SeqCst) {
        tracing::warn!("SIGCHLD handler already installed");
        return;
    }

    thread::spawn(|| {
        tracing::info!("SIGCHLD handler thread started");
        loop {
            // Use waitpid in a loop to reap all available children
            let mut status: i32 = 0;
            loop {
                // waitpid(-1, &mut status, WNOHANG) reaps any available child
                let result = unsafe { waitpid(-1, &mut status, WNOHANG) };
                if result == 0 {
                    // No more children to reap
                    break;
                } else if result < 0 {
                    // Error - likely ECHILD (no children)
                    if std::io::Error::last_os_error().raw_os_error() != Some(libc::ECHILD) {
                        tracing::debug!("waitpid error: {}", std::io::Error::last_os_error());
                    }
                    break;
                } else {
                    // Child reaped (result is pid)
                    if WIFEXITED(status) {
                        tracing::trace!(
                            "child {} exited normally with code: {}",
                            result,
                            WEXITSTATUS(status)
                        );
                    } else if WIFSIGNALED(status) {
                        tracing::trace!("child {} killed by signal: {}", result, WTERMSIG(status));
                    }
                }
            }
            thread::sleep(Duration::from_millis(100));
        }
    });
}

#[cfg(not(target_os = "linux"))]
pub fn install_sigchld_handler() {
    if SIGCHLD_HANDLER_STARTED.swap(true, Ordering::SeqCst) {
        tracing::warn!("SIGCHLD handler already installed");
        return;
    }
    tracing::debug!("SIGCHLD handler not available on this platform");
}

/// Check if the SIGCHLD handler is running
pub fn is_handler_running() -> bool {
    SIGCHLD_HANDLER_STARTED.load(Ordering::SeqCst)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_install_handler() {
        install_sigchld_handler();
        assert!(is_handler_running());
    }

    #[test]
    fn test_install_handler_idempotent() {
        // Calling twice should not panic
        install_sigchld_handler();
        install_sigchld_handler();
        // Handler should still be running
        assert!(is_handler_running());
    }
}

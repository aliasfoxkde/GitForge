//! Signal handling for process supervision
//!
//! This module provides proper SIGCHLD handling to prevent zombie processes
//! by actively reaping terminated child processes.

#[cfg(target_os = "linux")]
use libc::{waitpid, WEXITSTATUS, WIFEXITED, WIFSIGNALED, WNOHANG, WTERMSIG};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

static SIGCHLD_HANDLER_STARTED: AtomicBool = AtomicBool::new(false);

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

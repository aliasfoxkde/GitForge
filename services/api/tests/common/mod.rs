//! Helpers shared by the ci service integration tests.

use std::process::Command;
use std::time::{Duration, Instant};

/// Terminate a spawned ci process with SIGTERM and wait for it to exit.
///
/// The service traps SIGTERM for graceful shutdown, so this exercises the
/// real shutdown path instead of killing the process mid-flight. The clean
/// exit also matters for coverage measurement: the child's LLVM profile is
/// only written on a normal exit, so `cargo llvm-cov` only counts the code
/// these suites exercise when they stop the service this way rather than
/// with SIGKILL.
pub async fn shutdown_gracefully(child: &mut tokio::process::Child) {
    if child.id().is_none() {
        return; // Already exited.
    }

    #[cfg(unix)]
    {
        // Route through kill(1) rather than unsafe libc calls in tests.
        let pid = child.id().unwrap_or_default().to_string();
        let _ = Command::new("kill").args(["-s", "TERM", &pid]).status();
    }
    #[cfg(not(unix))]
    let _ = child.start_kill();

    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        match child.try_wait() {
            Ok(Some(_status)) => return,
            Ok(None) => tokio::time::sleep(Duration::from_millis(100)).await,
            Err(_) => return,
        }
    }

    // The service did not exit in time; fall back so tests never hang.
    let _ = child.start_kill();
    let _ = child.wait().await;
}

//! Helpers shared by the git-server protocol test suites.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Run a real `git` command and panic with its stderr on failure.
///
/// Not every suite in this directory uses every helper; the shared
/// module is compiled per test binary, so unused ones are expected.
#[allow(dead_code)]
pub fn run_git(
    args: &[&str],
    cwd: &std::path::Path,
    envs: &[(&str, &str)],
) -> std::process::Output {
    let mut command = Command::new("git");
    command
        .args(args)
        .current_dir(cwd)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdin(Stdio::null());
    for (key, value) in envs {
        command.env(key, value);
    }
    let output = command.output().expect("spawn git");
    if !output.status.success() {
        panic!(
            "git {} failed ({}): {}",
            args.join(" "),
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    output
}

/// Bind an ephemeral TCP port and return it.
#[allow(dead_code)]
pub fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind ephemeral port")
        .local_addr()
        .expect("local addr")
        .port()
}

/// Terminate a spawned git-server with SIGTERM and wait for it to exit.
///
/// The server traps SIGTERM for graceful shutdown, so this exercises the
/// real shutdown path instead of killing the process mid-flight. The clean
/// exit also matters for coverage measurement: the child's LLVM profile is
/// only written on a normal exit, so `cargo llvm-cov` only counts the code
/// these suites exercise when they stop the server this way rather than
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

    // The server did not exit in time; fall back so tests never hang.
    let _ = child.start_kill();
    let _ = child.wait().await;
}

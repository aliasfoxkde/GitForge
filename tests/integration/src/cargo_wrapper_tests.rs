//! Integration tests for cargo-wrapper enforcement
//!
//! These tests verify that the cargo-wrapper properly routes cargo commands
//! through the gitforge-buildd daemon for concurrency control.

use std::process::Command;
use std::time::Duration;
use std::thread;

/// Strip ANSI color codes from string
fn strip_ansi(s: &str) -> String {
    let mut result = s.to_string();
    // Remove common ANSI escape sequences
    let patterns = ["[0;32m", "[0;31m", "[1;33m", "[0m", "\u{1b}[0m", "\u{1b}"];
    for p in patterns {
        result = result.replace(p, "");
    }
    // Remove any remaining CSI sequences
    result = result.replace("\u{1b}[", "[");
    result
}

/// Test that cargo-wrapper status command shows daemon status
#[test]
fn test_cargo_wrapper_status_shows_daemon() {
    // The wrapper status command should work even without daemon
    let output = Command::new("/home/mkinney/.cargo/bin/cargo-wrapper")
        .args(["--wrapper-status"])
        .output()
        .expect("failed to execute cargo-wrapper");

    let stdout = strip_ansi(&String::from_utf8_lossy(&output.stdout));

    // Should indicate daemon is running or not
    assert!(
        stdout.contains("running") || stdout.contains("NOT running") || stdout.contains("gitforge-build"),
        "status should indicate daemon state, got: {}",
        stdout
    );
}

/// Test that cargo-wrapper --wrapper-help works
#[test]
fn test_cargo_wrapper_help() {
    let output = Command::new("/home/mkinney/.cargo/bin/cargo-wrapper")
        .args(["--wrapper-help"])
        .output()
        .expect("failed to execute cargo-wrapper");

    let stdout = strip_ansi(&String::from_utf8_lossy(&output.stdout));
    assert!(stdout.contains("cargo-wrapper"), "help should show wrapper name, got: {}", stdout);
    assert!(stdout.contains("gitforge-buildd") || stdout.contains("Routes"),
            "help should describe routing, got: {}", stdout);
}

/// Test that cargo-wrapper falls back to real cargo when daemon not running
#[test]
fn test_cargo_wrapper_fallback_no_daemon() {
    // Kill any running daemon temporarily
    let daemon_running = is_daemon_running();

    if daemon_running {
        // Daemon is running, skip this test
        // (we'll test fallback in a separate scenario)
        return;
    }

    // When daemon not running, wrapper should fallback to real cargo
    let output = Command::new("bash")
        .args(["-c", "source /home/mkinney/.cargo/bin/cargo-wrapper && cargo-wrapper -- cargo --version"])
        .output()
        .expect("failed to execute cargo-wrapper");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("cargo"), "fallback should invoke real cargo");
}

/// Test direct cargo version through daemon (when running)
#[test]
fn test_gitforge_build_routes_cargo_version() {
    // This test requires the daemon to be running
    if !is_daemon_running() {
        println!("daemon not running, skipping test");
        return;
    }

    // Use gitforge-build CLI to submit a cargo version job
    let output = Command::new("/nas/Temp/repos/GitForge/target/debug/gitforge-build")
        .args(["--", "--version"])
        .output()
        .expect("failed to execute gitforge-build");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Job should be submitted
    assert!(
        stdout.contains("submitted job") || stderr.contains("submitted job"),
        "should submit job, got stdout: {}, stderr: {}",
        stdout,
        stderr
    );
}

/// Test that daemon enforces concurrency limit
#[test]
fn test_gitforge_buildd_concurrency_limit() {
    if !is_daemon_running() {
        println!("daemon not running, skipping test");
        return;
    }

    // Get initial stats
    let initial_stats = get_daemon_stats();
    println!("Initial stats: {:?}", initial_stats);

    // Submit multiple quick jobs
    let num_jobs = 4;
    let mut job_ids = Vec::new();

    for i in 0..num_jobs {
        let output = Command::new("/nas/Temp/repos/GitForge/target/debug/gitforge-build")
            .args(["--", format!("echo job{}", i).as_str()])
            .output()
            .expect("failed to execute gitforge-build");

        let stdout = String::from_utf8_lossy(&output.stdout);
        if stdout.contains("submitted job:") {
            // Extract job ID
            if let Some(id) = stdout.lines().find(|l| l.contains("submitted job:"))
                .and_then(|l| l.split(':').next_back())
            {
                job_ids.push(id.trim().to_string());
            }
        }
    }

    // With max 2 concurrent, we shouldn't see more than 2 running at once
    // Check stats
    thread::sleep(Duration::from_millis(500));
    let stats = get_daemon_stats();

    println!("After submitting {} jobs: running={}, queued={}",
             num_jobs, stats.running, stats.queued);

    // Running should be at most 2 (the concurrency limit)
    assert!(
        stats.running <= 2,
        "should respect max concurrent limit of 2, got {} running",
        stats.running
    );
}

/// Test that daemon stats command works
#[test]
fn test_gitforge_buildd_stats() {
    if !is_daemon_running() {
        println!("daemon not running, skipping test");
        return;
    }

    let stats = get_daemon_stats();

    assert!(stats.max_concurrent > 0, "max_concurrent should be set");
    assert_eq!(stats.max_concurrent, 2, "max_concurrent should be 2");
}

/// Test that daemon handles list command
#[test]
fn test_gitforge_buildd_list_jobs() {
    if !is_daemon_running() {
        println!("daemon not running, skipping test");
        return;
    }

    let output = Command::new("/nas/Temp/repos/GitForge/target/debug/gitforge-build")
        .args(["--list"])
        .output()
        .expect("failed to execute gitforge-build");

    // Should not panic or error
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("error") || output.status.success(),
        "list command should work, got stderr: {}",
        stderr
    );
}

/// Helper: Check if daemon is running
fn is_daemon_running() -> bool {
    Command::new("pgrep")
        .args(["-f", "gitforge-buildd"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Helper: Get daemon stats
#[derive(Debug)]
struct DaemonStats {
    running: usize,
    queued: usize,
    completed: usize,
    max_concurrent: usize,
}

fn get_daemon_stats() -> DaemonStats {
    let output = Command::new("/nas/Temp/repos/GitForge/target/debug/gitforge-build")
        .args(["--stats"])
        .output()
        .expect("failed to execute gitforge-build --stats");

    let stdout = String::from_utf8_lossy(&output.stdout);

    let running = extract_stat(&stdout, "running:");
    let queued = extract_stat(&stdout, "queued:");
    let completed = extract_stat(&stdout, "completed:");
    let max_concurrent = extract_stat(&stdout, "max concurrent:");

    DaemonStats {
        running,
        queued,
        completed,
        max_concurrent,
    }
}

fn extract_stat(output: &str, key: &str) -> usize {
    output
        .lines()
        .find(|l| l.contains(key))
        .and_then(|l| {
            l.split(':')
                .nth(1)
                .and_then(|s| s.trim().parse::<usize>().ok())
        })
        .unwrap_or(0)
}

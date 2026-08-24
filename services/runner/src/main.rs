//! GitForce Runner Agent
//!
//! Main entry point for the runner agent service.

use gitforge_process::{create_shutdown_flag, spawn_shutdown_handler, wait_for_shutdown};
use gitforge_runner::{RunnerAgent, RunnerConfig};
#[allow(unused_imports)]
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .init();

    tracing::info!("starting GitForce Runner Agent");

    // Initialize process supervision (subreaper + SIGCHLD) to prevent zombies
    if let Err(e) = gitforge_process::init() {
        tracing::warn!("failed to initialize process supervision: {}", e);
    }

    // Load runner configuration from the environment, retaining development
    // defaults but failing closed on malformed deployment values.
    let config = RunnerConfig::from_env()?;

    // Create runner agent
    let mut agent = RunnerAgent::new(config).await?;

    // Register with scheduler
    let runner_id = agent.register().await?;
    tracing::info!("runner registered with ID: {}", runner_id);

    tracing::info!("Runner Agent initialized successfully");

    // Set up shutdown handling
    let shutdown = create_shutdown_flag();
    let shutdown_flag = shutdown.clone();

    // Spawn graceful shutdown handler
    spawn_shutdown_handler(shutdown_flag);

    // Start the registered agent's heartbeat and job-fetch loops. The agent
    // owns its executor; keep the loop under a task handle so shutdown can
    // stop it cleanly and propagate runtime failures to the service.
    let agent_loop = agent.clone();
    let runner_task = tokio::spawn(async move { agent_loop.run().await });

    // Wait for shutdown signal
    let shutdown_future = create_shutdown_future(shutdown.clone());
    tracing::info!("Runner Agent running, press Ctrl+C to stop");

    // Wait for shutdown signal
    timeout(Duration::MAX, shutdown_future).await.ok();

    tracing::info!("shutting down Runner Agent");

    // Stop the agent gracefully (force=false to wait for jobs)
    agent.stop(false).await;

    // Wait for active jobs to complete with a timeout
    const JOB_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(30);
    if !agent.wait_for_jobs_complete(JOB_SHUTDOWN_TIMEOUT).await {
        tracing::warn!("jobs did not complete in time, force cancelling");
        agent.stop(true).await;
    }

    runner_task
        .await
        .map_err(|e| anyhow::anyhow!("runner task join failed: {}", e))??;

    // Graceful shutdown delay
    graceful_shutdown_delay().await;

    tracing::info!("Runner Agent stopped");
    Ok(())
}

/// Create the shutdown future that waits for shutdown signal
pub async fn create_shutdown_future(shutdown: Arc<AtomicBool>) {
    wait_for_shutdown(shutdown).await;
}

/// Perform graceful shutdown delay
pub async fn graceful_shutdown_delay() {
    timeout(Duration::from_secs(2), async {
        tokio::time::sleep(Duration::from_secs(1)).await;
    })
    .await
    .ok();
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

    #[test]
    fn test_graceful_shutdown_delay_does_not_panic() {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                graceful_shutdown_delay().await;
            });
    }

    #[tokio::test]
    async fn test_create_shutdown_future() {
        let shutdown = create_shutdown_flag();
        let shutdown_flag = shutdown.clone();

        // Set shutdown after a short delay
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            shutdown_flag.store(true, Ordering::SeqCst);
        });

        create_shutdown_future(shutdown).await;
    }

    #[tokio::test]
    async fn test_spawn_shutdown_handler_does_not_panic() {
        let flag = create_shutdown_flag();
        // Just verify the function doesn't panic when called
        spawn_shutdown_handler(flag);
    }

    #[test]
    fn test_shutdown_flag_is_atomic() {
        let flag = create_shutdown_flag();
        // Verify atomic operations work
        assert!(!flag.load(Ordering::SeqCst));
        flag.store(true, Ordering::SeqCst);
        assert!(flag.load(Ordering::SeqCst));
    }

    #[test]
    fn test_shutdown_flag_load_ordering() {
        // Verify SeqCst ordering is used
        let flag = create_shutdown_flag();
        let value = flag.load(Ordering::SeqCst);
        assert!(!value);
    }

    #[test]
    fn test_graceful_shutdown_delay_completes() {
        // Test that the delay actually waits
        let start = std::time::Instant::now();
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                graceful_shutdown_delay().await;
            });
        // Should have taken at least 1 second
        assert!(start.elapsed().as_secs() >= 1);
    }

    #[test]
    fn test_runner_service_config_defaults() {
        // Test that RunnerConfig::default() works - just verify it doesn't panic
        let _config = gitforge_runner::RunnerConfig::default();
    }
}

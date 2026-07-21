//! GitForce Runner Agent
//!
//! Main entry point for the runner agent service.

use gitforce_runner::{JobExecutor, RunnerAgent, RunnerConfig};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::signal;
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

    // Load runner configuration
    let config = RunnerConfig::default();

    // Create runner agent
    let mut agent = RunnerAgent::new(config).await?;

    // Register with scheduler
    let runner_id = agent.register().await?;
    tracing::info!("runner registered with ID: {}", runner_id);

    // Create job executor
    let _executor = Arc::new(JobExecutor::new().await?);

    tracing::info!("Runner Agent initialized successfully");

    // Set up shutdown handling
    let shutdown = create_shutdown_flag();
    let shutdown_flag = shutdown.clone();

    // Spawn graceful shutdown handler
    spawn_shutdown_handler(shutdown_flag);

    // Wait for shutdown signal
    let shutdown_future = create_shutdown_future(shutdown.clone());
    tracing::info!("Runner Agent running, press Ctrl+C to stop");

    // Wait for shutdown signal
    timeout(Duration::MAX, shutdown_future).await.ok();

    tracing::info!("shutting down Runner Agent");

    // Stop the agent gracefully
    agent.stop().await;

    // Graceful shutdown delay
    graceful_shutdown_delay().await;

    tracing::info!("Runner Agent stopped");
    Ok(())
}

/// Create a shutdown flag
pub fn create_shutdown_flag() -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(false))
}

/// Spawn the shutdown signal handler
pub fn spawn_shutdown_handler(shutdown_flag: Arc<AtomicBool>) {
    tokio::spawn(async move {
        let mut sigterm = signal::unix::signal(signal::unix::SignalKind::terminate()).unwrap();
        let mut sigint = signal::unix::signal(signal::unix::SignalKind::interrupt()).unwrap();

        tokio::select! {
            _ = sigterm.recv() => {
                tracing::info!("received SIGTERM, initiating graceful shutdown...");
            }
            _ = sigint.recv() => {
                tracing::info!("received SIGINT, initiating graceful shutdown...");
            }
        }
        shutdown_flag.store(true, Ordering::SeqCst);
    });
}

/// Create the shutdown future that waits for shutdown signal
pub async fn create_shutdown_future(shutdown: Arc<AtomicBool>) {
    while !shutdown.load(Ordering::SeqCst) {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
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
}

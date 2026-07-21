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

    // Shared shutdown flag
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_flag = shutdown.clone();

    // Spawn graceful shutdown handler
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

    // Wait for shutdown signal
    let shutdown_future = async {
        while !shutdown.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    };

    tracing::info!("Runner Agent running, press Ctrl+C to stop");

    // Wait for shutdown signal
    timeout(Duration::MAX, shutdown_future).await.ok();

    tracing::info!("shutting down Runner Agent");

    // Stop the agent gracefully
    agent.stop().await;

    // Graceful shutdown delay
    timeout(Duration::from_secs(2), async {
        tokio::time::sleep(Duration::from_secs(1)).await;
    })
    .await
    .ok();

    tracing::info!("Runner Agent stopped");
    Ok(())
}

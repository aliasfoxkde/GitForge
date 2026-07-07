//! GitForce Runner Agent
//!
//! Main entry point for the runner agent service.

use gitforce_runner::{JobExecutor, RunnerAgent, RunnerConfig};
use std::sync::Arc;
use tokio::signal;

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
    let executor = Arc::new(JobExecutor::new().await?);

    // Start agent loop
    // In production, this would:
    // 1. Register with the scheduler
    // 2. Poll for jobs
    // 3. Execute jobs using the executor
    // 4. Report status back to scheduler

    tracing::info!("Runner Agent initialized successfully");

    // Wait for shutdown signal
    signal::ctrl_c().await?;

    tracing::info!("shutting down Runner Agent");
    agent.stop().await;

    Ok(())
}

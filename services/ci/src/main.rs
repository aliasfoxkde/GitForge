//! GitForce CI Orchestrator
//!
//! Main entry point for the CI orchestration service.

use gitforce_ci::{CiEngine, PipelineDefinition, PipelineTriggerEvent, TriggerType};
use gitforce_events::{EventBus, EventEnvelope, EventType, InMemoryEventBus};
use gitforce_scheduler::Scheduler;
use std::sync::Arc;
use tokio::signal;
use tokio::time::{interval, Duration};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .init();

    tracing::info!("starting GitForce CI Orchestrator");

    // Initialize event bus
    let event_bus: Arc<dyn EventBus> = Arc::new(InMemoryEventBus::new());

    // Initialize scheduler
    let scheduler = Arc::new(Scheduler::new());

    // Start event consumer loop
    let _event_bus_clone = event_bus.clone();
    tokio::spawn(async move {
        tracing::info!("starting event consumer loop");
        // In production, subscribe to push events and trigger pipelines
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });

    // Start scheduler loop
    let scheduler_clone = scheduler.clone();
    tokio::spawn(async move {
        let mut ticker = interval(Duration::from_secs(5));
        loop {
            ticker.tick().await;
            scheduler_clone.process_queue().await;
        }
    });

    tracing::info!("CI Orchestrator initialized successfully");

    // Wait for shutdown signal
    signal::ctrl_c().await?;

    tracing::info!("shutting down CI Orchestrator");
    Ok(())
}

//! GitForce Git Server
//!
//! Main entry point for the Git SSH/HTTP server.

use gitforce_core::{FileStorageBackend, HookManager, RepoService};
use gitforce_events::{EventBus, InMemoryEventBus};
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

    tracing::info!("starting GitForce Git Server");

    // Get git root from environment
    let git_root = std::env::var("GIT_ROOT").unwrap_or_else(|_| "/var/lib/gitforge/repos".to_string());
    tracing::info!("using git root: {}", git_root);

    // Initialize storage
    let storage = FileStorageBackend::new(&git_root);
    storage.ensure_root().await?;

    // Initialize repository service
    let _repo_service = RepoService::new(storage);

    // Initialize event bus
    let _event_bus: Arc<dyn EventBus> = Arc::new(InMemoryEventBus::new());

    // Initialize hook manager
    let _hook_manager = HookManager::new();

    tracing::info!("Git Server initialized successfully");

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

    tracing::info!("Git Server running, press Ctrl+C to stop");

    // Wait for shutdown signal
    timeout(Duration::MAX, shutdown_future).await.ok();

    tracing::info!("shutting down Git Server");

    // Graceful shutdown delay
    timeout(Duration::from_secs(2), async {
        tokio::time::sleep(Duration::from_secs(1)).await;
    })
    .await
    .ok();

    tracing::info!("Git Server stopped");
    Ok(())
}

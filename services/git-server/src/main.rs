//! GitForce Git Server
//!
//! Main entry point for the Git SSH/HTTP server.

use gitforce_core::{FileStorageBackend, HookManager, RepoService};
use gitforce_events::{EventBus, InMemoryEventBus};
use std::sync::Arc;
use tokio::net::TcpListener;
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

    tracing::info!("starting GitForce Git Server");

    // Initialize storage
    let storage = FileStorageBackend::new("/var/lib/gitforce/repos");
    storage.ensure_root().await?;

    // Initialize repository service
    let repo_service = RepoService::new(storage);

    // Initialize event bus
    let event_bus: Arc<dyn EventBus> = Arc::new(InMemoryEventBus::new());

    // Initialize hook manager
    let hook_manager = HookManager::new();

    // In production, we would:
    // 1. Start SSH server on port 22
    // 2. Start HTTP server on port 80/443
    // 3. Wire up post-receive hooks to emit events

    tracing::info!("Git Server initialized successfully");

    // Wait for shutdown signal
    signal::ctrl_c().await?;

    tracing::info!("shutting down Git Server");
    Ok(())
}

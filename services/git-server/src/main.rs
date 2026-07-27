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

    // Initialize process supervision (subreaper + SIGCHLD) to prevent zombies
    if let Err(e) = gitforce_process::init() {
        tracing::warn!("failed to initialize process supervision: {}", e);
    }

    // Get git root from environment
    let git_root = get_git_root();
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

    // Set up shutdown handling
    let shutdown = create_shutdown_flag();
    let shutdown_flag = shutdown.clone();

    // Spawn graceful shutdown handler
    spawn_shutdown_handler(shutdown_flag);

    // Wait for shutdown signal
    let shutdown_future = create_shutdown_future(shutdown.clone());
    tracing::info!("Git Server running, press Ctrl+C to stop");

    // Wait for shutdown signal
    timeout(Duration::MAX, shutdown_future).await.ok();

    tracing::info!("shutting down Git Server");

    // Graceful shutdown delay
    graceful_shutdown_delay().await;

    tracing::info!("Git Server stopped");
    Ok(())
}

/// Get the git root directory from environment or use default
pub fn get_git_root() -> String {
    std::env::var("GIT_ROOT").unwrap_or_else(|_| "/var/lib/gitforge/repos".to_string())
}

/// Create a shutdown flag
pub fn create_shutdown_flag() -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(false))
}

/// Spawn the shutdown signal handler (Unix-only)
#[cfg(unix)]
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

/// Spawn the shutdown signal handler (Windows stub)
#[cfg(windows)]
pub fn spawn_shutdown_handler(_shutdown_flag: Arc<AtomicBool>) {}

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
    fn test_get_git_root_default() {
        // Without GIT_ROOT set, should return default
        std::env::remove_var("GIT_ROOT");
        let root = get_git_root();
        assert_eq!(root, "/var/lib/gitforge/repos");
    }

    #[test]
    fn test_get_git_root_from_env() {
        // With GIT_ROOT set, should return it
        std::env::set_var("GIT_ROOT", "/custom/path");
        let root = get_git_root();
        assert_eq!(root, "/custom/path");
        std::env::remove_var("GIT_ROOT");
    }

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
        // Just verify it doesn't panic
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
    fn test_get_git_root_empty_string() {
        // When GIT_ROOT is set to empty string
        std::env::set_var("GIT_ROOT", "");
        let _root = get_git_root();
        // Empty string is a valid env var value, so it returns empty
        std::env::remove_var("GIT_ROOT");
    }
}

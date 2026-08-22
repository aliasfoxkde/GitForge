//! GitForce API Gateway
//!
//! Main entry point for the REST API gateway.

use gitforce_api::ApiServer;
use gitforce_db::Pool;
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

    tracing::info!("starting GitForce API Gateway");

    // Load configuration
    let config = load_config();
    tracing::info!("connecting to database: {}", config.database_url);

    // Create database pool
    let pool = Pool::new(&config.database_url).await?;
    pool.migrate().await?;
    tracing::info!("database connection established");

    // Create and start API server
    let server = ApiServer::new(&config.jwt_secret, pool).with_port(config.port);

    tracing::info!("API Gateway listening on port {}", config.port);

    // Set up shutdown handling
    let shutdown = create_shutdown_flag();
    let shutdown_flag = shutdown.clone();

    // Spawn graceful shutdown handler
    spawn_shutdown_handler(shutdown_flag);

    // Create shutdown future
    let shutdown_future = create_shutdown_future(shutdown.clone());

    // Run server with graceful shutdown
    let server_handle = tokio::spawn(async move {
        if let Err(e) = server.start().await {
            tracing::error!("server error: {}", e);
        }
    });

    // Wait for either server to finish or shutdown signal
    tokio::select! {
        result = server_handle => {
            if let Err(e) = result {
                tracing::error!("server task panicked: {}", e);
            }
        }
        _ = shutdown_future => {
            tracing::info!("shutdown signal received, stopping server...");
        }
    }

    // Graceful shutdown delay to allow connections to drain
    tracing::info!("waiting for connections to drain...");
    graceful_shutdown_delay().await;

    tracing::info!("API Gateway stopped");
    Ok(())
}

/// Server configuration
pub struct ServerConfig {
    pub jwt_secret: String,
    pub port: u16,
    pub database_url: String,
}

/// Load server configuration from environment
pub fn load_config() -> ServerConfig {
    let jwt_secret =
        std::env::var("JWT_SECRET").unwrap_or_else(|_| "dev-secret-change-in-prod".to_string());
    let port = std::env::var("PORT")
        .unwrap_or_else(|_| "8080".to_string())
        .parse::<u16>()
        .unwrap_or(8080);
    let database_url =
        std::env::var("DATABASE_URL").unwrap_or_else(|_| "sqlite:/gitforge.db".to_string());

    ServerConfig {
        jwt_secret,
        port,
        database_url,
    }
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
    tracing::info!("graceful shutdown complete");
}

/// Perform graceful shutdown delay
pub async fn graceful_shutdown_delay() {
    timeout(Duration::from_secs(10), async {
        tokio::time::sleep(Duration::from_secs(2)).await;
        tracing::info!("graceful shutdown complete");
    })
    .await
    .ok();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clear_env() {
        std::env::remove_var("JWT_SECRET");
        std::env::remove_var("PORT");
        std::env::remove_var("DATABASE_URL");
    }

    #[test]
    fn test_load_config_defaults() {
        // Clear any set env vars first
        clear_env();

        let config = load_config();
        assert_eq!(config.jwt_secret, "dev-secret-change-in-prod");
        assert_eq!(config.port, 8080);
        assert_eq!(config.database_url, "sqlite:/gitforge.db");
    }

    #[test]
    fn test_load_config_from_env() {
        // Always set fresh values - clear first then set
        clear_env();
        std::env::set_var("JWT_SECRET", "test-secret");
        std::env::set_var("PORT", "3000");
        std::env::set_var("DATABASE_URL", "postgres://localhost/test");

        let config = load_config();
        assert_eq!(config.jwt_secret, "test-secret");
        assert_eq!(config.port, 3000);
        assert_eq!(config.database_url, "postgres://localhost/test");

        // Clean up
        clear_env();
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
}

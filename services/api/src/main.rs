//! GitForce API Gateway
//!
//! Main entry point for the REST API gateway.

use gitforge_api::ApiServer;
use gitforge_db::Pool;
use gitforge_process::{create_shutdown_flag, spawn_shutdown_handler, wait_for_shutdown};
use gitforge_storage::FileStorage;
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

    tracing::info!("starting GitForce API Gateway");

    // Initialize process supervision (subreaper + SIGCHLD) to prevent zombies
    if let Err(e) = gitforge_process::init() {
        tracing::warn!("failed to initialize process supervision: {}", e);
    }

    // Load configuration
    let config = load_config();
    tracing::info!("connecting to database: {}", config.database_url);

    // Create database pool
    let pool = Pool::new(&config.database_url).await?;
    pool.migrate().await?;
    tracing::info!("database connection established");

    // Create and start API server. Runner and API share this LAN-local
    // filesystem root for bounded artifact metadata and content.
    let artifact_root = std::env::var("GITFORGE_ARTIFACT_ROOT")
        .unwrap_or_else(|_| "/tmp/gitforge-artifacts".to_string());
    let storage = FileStorage::new(artifact_root).await?;
    let server = ApiServer::new(&config.jwt_secret, pool)
        .with_storage_extension(Arc::new(storage))
        .with_port(config.port);

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
#[derive(Debug)]
pub struct ServerConfig {
    pub jwt_secret: String,
    pub port: u16,
    pub database_url: String,
}

/// Load server configuration from environment
/// Fails if JWT_SECRET is not set (no dev fallback in production)
pub fn load_config() -> ServerConfig {
    let jwt_secret = std::env::var("JWT_SECRET")
        .expect("JWT_SECRET environment variable must be set - no dev fallback in production");

    let port = std::env::var("PORT")
        .unwrap_or_else(|_| "42780".to_string())
        .parse::<u16>()
        .unwrap_or(42780);

    let database_url =
        std::env::var("DATABASE_URL").unwrap_or_else(|_| "sqlite:/gitforge.db".to_string());

    ServerConfig {
        jwt_secret,
        port,
        database_url,
    }
}

/// Load configuration for testing (allows env var defaults)
#[cfg(test)]
pub fn load_config_test() -> ServerConfig {
    // Use ONLY defaults for testing - ignore any environment variables
    // This ensures test isolation regardless of parallel test execution
    ServerConfig {
        jwt_secret: "test-secret".to_string(),
        port: 42780,
        database_url: "sqlite:/gitforge.db".to_string(),
    }
}

/// Create the shutdown future that waits for shutdown signal
pub async fn create_shutdown_future(shutdown: Arc<AtomicBool>) {
    wait_for_shutdown(shutdown).await;
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
    fn test_load_config_requires_jwt_secret() {
        clear_env();
        std::env::remove_var("JWT_SECRET");

        // Should panic because JWT_SECRET is not set
        let result = std::panic::catch_unwind(|| {
            load_config();
        });
        assert!(
            result.is_err(),
            "load_config should panic without JWT_SECRET"
        );
    }

    #[test]
    fn test_load_config_with_env() {
        clear_env();
        std::env::set_var("JWT_SECRET", "production-secret-32chars!!");
        std::env::set_var("PORT", "3000");
        std::env::set_var("DATABASE_URL", "postgres://localhost/test");

        let config = load_config();
        assert_eq!(config.jwt_secret, "production-secret-32chars!!");
        assert_eq!(config.port, 3000);
        assert_eq!(config.database_url, "postgres://localhost/test");

        clear_env();
    }

    #[test]
    fn test_load_config_test_defaults() {
        clear_env();
        // Explicitly remove DATABASE_URL to ensure clean state
        std::env::remove_var("DATABASE_URL");
        // load_config_test should use defaults when env vars not set
        let config = load_config_test();
        assert_eq!(config.jwt_secret, "test-secret");
        assert_eq!(config.port, 42780);
        assert_eq!(config.database_url, "sqlite:/gitforge.db");
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
    fn test_server_config_debug() {
        let config = ServerConfig {
            jwt_secret: "test-secret".to_string(),
            port: 42780,
            database_url: "sqlite::memory:".to_string(),
        };
        let debug_str = format!("{:?}", config);
        assert!(debug_str.contains("jwt_secret"));
        assert!(debug_str.contains("42780"));
    }

    #[test]
    fn test_load_config_port_parsing() {
        clear_env();
        std::env::set_var("JWT_SECRET", "test-secret");
        std::env::set_var("PORT", "9000");

        let config = load_config();
        assert_eq!(config.port, 9000);

        clear_env();
    }

    #[test]
    fn test_load_config_invalid_port_uses_default() {
        clear_env();
        std::env::set_var("JWT_SECRET", "test-secret");
        std::env::set_var("PORT", "invalid");

        let config = load_config();
        // Invalid port should fallback to 42780
        assert_eq!(config.port, 42780);

        clear_env();
    }

    #[test]
    fn test_load_config_port_zero_is_valid() {
        clear_env();
        std::env::set_var("JWT_SECRET", "test-secret");
        std::env::set_var("PORT", "0");

        let config = load_config();
        // Port 0 is actually valid (though it means "bind to any available port")
        assert_eq!(config.port, 0);

        clear_env();
    }

    #[test]
    fn test_load_config_port_max_u16() {
        clear_env();
        std::env::set_var("JWT_SECRET", "test-secret");
        std::env::set_var("PORT", "65535");

        let config = load_config();
        assert_eq!(config.port, 65535);

        clear_env();
    }

    #[test]
    fn test_create_shutdown_flag_load_ordering() {
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
        // Should have taken at least 2 seconds
        assert!(start.elapsed().as_secs() >= 2);
    }

    #[test]
    fn test_clear_env_removes_all_vars() {
        // Set then clear
        std::env::set_var("JWT_SECRET", "test");
        std::env::set_var("PORT", "1234");
        clear_env();
        // After clear, these should not be set
        assert!(std::env::var("JWT_SECRET").is_err());
        assert!(std::env::var("PORT").is_err());
    }
}

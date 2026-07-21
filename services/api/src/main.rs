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

    // Load configuration (from environment or config file)
    let jwt_secret = std::env::var("JWT_SECRET").unwrap_or_else(|_| "dev-secret-change-in-prod".to_string());
    let port = std::env::var("PORT")
        .unwrap_or_else(|_| "8080".to_string())
        .parse::<u16>()
        .unwrap_or(8080);
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "sqlite:/gitforge.db".to_string());

    // Create database pool
    tracing::info!("connecting to database: {}", database_url);
    let pool = Pool::new(&database_url).await?;
    pool.migrate().await?;
    tracing::info!("database connection established");

    // Create and start API server
    let server = ApiServer::new(&jwt_secret, pool).with_port(port);

    tracing::info!("API Gateway listening on port {}", port);

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

    // Create shutdown future
    let shutdown_future = async {
        while !shutdown.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        tracing::info!("graceful shutdown complete");
    };

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
            // In a real implementation, we'd call server.shutdown() here
            // For now, the server will naturally stop when the handle completes
        }
    }

    // Graceful shutdown delay to allow connections to drain
    tracing::info!("waiting for connections to drain...");
    timeout(Duration::from_secs(10), async {
        // Wait a bit for connections to drain
        tokio::time::sleep(Duration::from_secs(2)).await;
        tracing::info!("graceful shutdown complete");
    })
    .await
    .ok();

    tracing::info!("API Gateway stopped");
    Ok(())
}

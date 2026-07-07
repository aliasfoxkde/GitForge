//! GitForce API Gateway
//!
//! Main entry point for the REST API gateway.

use gitforce_api::ApiServer;
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

    tracing::info!("starting GitForce API Gateway");

    // Load configuration (from environment or config file)
    let jwt_secret = std::env::var("JWT_SECRET").unwrap_or_else(|_| "dev-secret-change-in-prod".to_string());
    let port = std::env::var("PORT")
        .unwrap_or_else(|_| "8080".to_string())
        .parse::<u16>()
        .unwrap_or(8080);

    // Create and start API server
    let server = ApiServer::new(&jwt_secret).with_port(port);

    tracing::info!("API Gateway listening on port {}", port);

    // Run the server
    server.start().await?;

    // Wait for shutdown signal
    signal::ctrl_c().await?;

    tracing::info!("shutting down API Gateway");

    Ok(())
}

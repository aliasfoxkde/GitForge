//! GitForce scheduler HTTP service.

use axum::{routing::get, Json};
use gitforce_scheduler::{create_state, scheduler_routes, Scheduler};
use serde_json::json;
use std::net::SocketAddr;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .init();

    let port = std::env::var("PORT")
        .or_else(|_| std::env::var("SCHEDULER_PORT"))
        .unwrap_or_else(|_| "8081".to_string())
        .parse::<u16>()
        .map_err(|error| anyhow::anyhow!("invalid scheduler port: {error}"))?;

    let state = create_state(Scheduler::new());
    let app = scheduler_routes::<()>(state).route(
        "/health",
        get(|| async { Json(json!({ "status": "healthy" })) }),
    );
    let address = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(address).await?;
    tracing::info!(%address, "GitForce scheduler listening");

    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;

    Ok(())
}

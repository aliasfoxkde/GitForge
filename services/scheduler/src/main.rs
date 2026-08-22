//! GitForce scheduler HTTP service.

use axum::{routing::get, Json};
use gitforce_db::Pool;
use gitforce_scheduler::{create_state_with_pool, scheduler_routes, Scheduler};
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

    let database_url = std::env::var("SCHEDULER_DATABASE_URL").unwrap_or_else(|_| {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        format!("sqlite:{home}/.local/share/gitforge/scheduler.db?mode=rwc")
    });
    let pool = Pool::new(&database_url)
        .await
        .map_err(|error| anyhow::anyhow!("failed to open scheduler database: {error}"))?;
    pool.migrate()
        .await
        .map_err(|error| anyhow::anyhow!("failed to migrate scheduler database: {error}"))?;
    let state = create_state_with_pool(Scheduler::new(), pool)
        .await
        .map_err(|error| anyhow::anyhow!("failed to initialize scheduler state: {error}"))?;
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

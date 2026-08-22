//! GitForce scheduler HTTP service.

use axum::{routing::get, Json};
use gitforce_db::Pool;
use gitforce_scheduler::{create_state_with_pool, scheduler_routes, start_claim_reaper, Scheduler};
use serde_json::json;
use std::net::SocketAddr;
use std::time::Duration;

/// Parse a strictly positive i64 environment variable, or return a default.
///
/// Rejects zero, negative, and non-numeric values with a descriptive error.
fn parse_claim_env_i64(name: &str, default: i64) -> anyhow::Result<i64> {
    let var = std::env::var(name).unwrap_or_else(|_| default.to_string());
    let value: i64 = var
        .parse()
        .map_err(|_| anyhow::anyhow!("{name} must be an integer"))?;
    if value <= 0 {
        anyhow::bail!("{name} must be a positive integer (got {value})");
    }
    Ok(value)
}

/// Parse a strictly positive i32 environment variable, or return a default.
fn parse_claim_env_i32(name: &str, default: i32) -> anyhow::Result<i32> {
    let var = std::env::var(name).unwrap_or_else(|_| default.to_string());
    let value: i32 = var
        .parse()
        .map_err(|_| anyhow::anyhow!("{name} must be an integer"))?;
    if value <= 0 {
        anyhow::bail!("{name} must be a positive integer (got {value})");
    }
    Ok(value)
}

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

    // Parse claim reaper configuration from environment variables.
    let claim_lease_secs = parse_claim_env_i64("SCHEDULER_CLAIM_LEASE_SECS", 300)?;
    let claim_max_retries = parse_claim_env_i32("SCHEDULER_CLAIM_MAX_RETRIES", 3)?;
    let claim_reaper_interval_secs =
        parse_claim_env_i64("SCHEDULER_CLAIM_REAPER_INTERVAL_SECS", 30)?;

    // Start the durable claim reaper background task.
    let reaper_handle = start_claim_reaper(
        &state,
        claim_lease_secs,
        claim_max_retries,
        Duration::from_secs(claim_reaper_interval_secs as u64),
    )
    .ok_or_else(|| anyhow::anyhow!("claim reaper requires durable scheduler state (no pool)"))?;

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

    // Shut down the claim reaper background task.
    reaper_handle.abort();

    Ok(())
}

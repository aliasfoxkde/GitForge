//! GitForce scheduler HTTP service.

use axum::{routing::get, Json, Router};
use gitforce_db::Pool;
use gitforce_scheduler::{
    create_state_with_pool, scheduler_routes, start_claim_reaper, with_auth, Scheduler,
    SchedulerAuthState,
};
use serde_json::json;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::str::FromStr;
use std::time::Duration;

/// Parse a valid IPv4 address from an environment variable, or return a default.
///
/// Rejects invalid IP strings. This enforces that the scheduler bind address is
/// a valid, contained IP — never 0.0.0.0 when exposed outside a trusted network.
fn parse_scheduler_bind(name: &str, default: Ipv4Addr) -> anyhow::Result<Ipv4Addr> {
    let var = std::env::var(name).unwrap_or_else(|_| default.to_string());
    let ip = IpAddr::from_str(&var)
        .map_err(|_| anyhow::anyhow!("{name} must be a valid IPv4 address (got: {var})"))?;
    match ip {
        IpAddr::V4(v4) => Ok(v4),
        IpAddr::V6(_) => anyhow::bail!("{name} must be IPv4 (IPv6 not supported: {var})"),
    }
}

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
    let state = create_state_with_pool(Scheduler::new(), pool, SchedulerAuthState::from_env())
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

    // Build the scheduler router and apply auth middleware when a secret is configured.
    // Routes added AFTER with_auth (e.g. /health) remain unauthenticated.
    let scheduler_router = scheduler_routes::<()>(state.clone());
    let app: Router = with_auth(state.auth, scheduler_router).route(
        "/health",
        get(|| async { Json(json!({ "status": "healthy" })) }),
    );
    let bind_ip = parse_scheduler_bind("SCHEDULER_BIND", Ipv4Addr::new(127, 0, 0, 1))?;
    let address = SocketAddr::from((bind_ip, port));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_scheduler_bind_valid_ipv4() {
        // Valid localhost
        std::env::set_var("SCHEDULER_BIND", "127.0.0.1");
        let result = parse_scheduler_bind("SCHEDULER_BIND", Ipv4Addr::new(127, 0, 0, 1));
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Ipv4Addr::new(127, 0, 0, 1));
        std::env::remove_var("SCHEDULER_BIND");

        // Valid private IP
        std::env::set_var("SCHEDULER_BIND", "10.0.0.1");
        let result = parse_scheduler_bind("SCHEDULER_BIND", Ipv4Addr::new(127, 0, 0, 1));
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Ipv4Addr::new(10, 0, 0, 1));
        std::env::remove_var("SCHEDULER_BIND");

        // Valid 0.0.0.0 (explicit)
        std::env::set_var("SCHEDULER_BIND", "0.0.0.0");
        let result = parse_scheduler_bind("SCHEDULER_BIND", Ipv4Addr::new(127, 0, 0, 1));
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Ipv4Addr::new(0, 0, 0, 0));
        std::env::remove_var("SCHEDULER_BIND");
    }

    #[test]
    fn test_parse_scheduler_bind_default_on_missing() {
        std::env::remove_var("SCHEDULER_BIND");
        let result = parse_scheduler_bind("SCHEDULER_BIND", Ipv4Addr::new(127, 0, 0, 1));
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Ipv4Addr::new(127, 0, 0, 1));
    }

    #[test]
    fn test_parse_scheduler_bind_rejects_invalid() {
        // Invalid: not an IP address
        std::env::set_var("SCHEDULER_BIND", "not-an-ip");
        let result = parse_scheduler_bind("SCHEDULER_BIND", Ipv4Addr::new(127, 0, 0, 1));
        assert!(result.is_err());
        std::env::remove_var("SCHEDULER_BIND");

        // Invalid: out of range
        std::env::set_var("SCHEDULER_BIND", "256.0.0.1");
        let result = parse_scheduler_bind("SCHEDULER_BIND", Ipv4Addr::new(127, 0, 0, 1));
        assert!(result.is_err());
        std::env::remove_var("SCHEDULER_BIND");

        // Invalid: empty
        std::env::set_var("SCHEDULER_BIND", "");
        let result = parse_scheduler_bind("SCHEDULER_BIND", Ipv4Addr::new(127, 0, 0, 1));
        assert!(result.is_err());
        std::env::remove_var("SCHEDULER_BIND");
    }

    #[test]
    fn test_parse_scheduler_bind_rejects_ipv6() {
        std::env::set_var("SCHEDULER_BIND", "::1");
        let result = parse_scheduler_bind("SCHEDULER_BIND", Ipv4Addr::new(127, 0, 0, 1));
        assert!(result.is_err());
        std::env::remove_var("SCHEDULER_BIND");

        std::env::set_var("SCHEDULER_BIND", "fe80::1");
        let result = parse_scheduler_bind("SCHEDULER_BIND", Ipv4Addr::new(127, 0, 0, 1));
        assert!(result.is_err());
        std::env::remove_var("SCHEDULER_BIND");
    }
}

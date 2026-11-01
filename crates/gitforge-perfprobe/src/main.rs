//! GitForge performance probe (M8 characterization harness).
//!
//! Drives characterized load against a running GitForge gateway (API) and
//! reports per-endpoint latency percentiles. Read-only probes need no
//! credentials; authed probes log in via `/auth/login`.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};

#[derive(Parser)]
#[command(
    name = "perfprobe",
    about = "GitForge performance characterization harness"
)]
struct Cli {
    /// Gateway base URL
    #[arg(long, default_value = "http://127.0.0.1:42780")]
    base_url: String,

    /// Requests per endpoint (mixed workload scales this per weight)
    #[arg(long, default_value_t = 50)]
    requests: u32,

    /// Concurrent workers issuing requests
    #[arg(long, default_value_t = 4)]
    concurrency: u32,

    /// Login as this user for authed probes
    #[arg(long)]
    username: Option<String>,

    /// Password for authed probes (omit to run unauthenticated probes only)
    #[arg(long)]
    password: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Hit a single endpoint repeatedly
    Probe {
        /// Path to probe, e.g. /health or /api/repos
        path: String,
    },
    /// Run the standard characterization workload (health, metrics, repos, pipelines)
    Baseline,
}

#[derive(Debug, Clone, Serialize)]
struct Sample {
    endpoint: String,
    status: u16,
    latency_ms: f64,
}

#[derive(Debug, Clone, Serialize)]
struct ProbeBatch {
    samples: Vec<Sample>,
    elapsed_secs: f64,
}

#[derive(Debug, Serialize)]
struct Report {
    base_url: String,
    started_at: String,
    requests_per_endpoint: u32,
    concurrency: u32,
    endpoints: Vec<EndpointReport>,
}

#[derive(Debug, Serialize)]
struct EndpointReport {
    endpoint: String,
    requests: usize,
    errors: usize,
    p50_ms: f64,
    p95_ms: f64,
    p99_ms: f64,
    max_ms: f64,
    throughput_rps: f64,
}

#[derive(Deserialize)]
struct LoginResponse {
    token: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "perfprobe=info".into()),
        )
        .init();

    let cli = Cli::parse();
    let base = cli.base_url.trim_end_matches('/').to_string();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;

    let token = match (&cli.username, &cli.password) {
        (Some(user), Some(pass)) => Some(login(&client, &base, user, pass).await?),
        _ => None,
    };

    let endpoints: Vec<(String, u32)> = match &cli.command {
        Command::Probe { path } => vec![(path.clone(), cli.requests)],
        Command::Baseline => vec![
            ("/health".to_string(), cli.requests),
            ("/metrics".to_string(), cli.requests),
            ("/api/repos".to_string(), cli.requests),
            ("/api/pipeline-runs".to_string(), cli.requests),
        ],
    };

    let started_at = chrono::Utc::now().to_rfc3339();
    let concurrency = cli.concurrency.max(1);
    let mut samples: Vec<ProbeBatch> = Vec::new();

    for (endpoint, count) in &endpoints {
        let batch = probe_endpoint(
            &client,
            &base,
            endpoint,
            *count,
            concurrency,
            token.as_deref(),
        )
        .await?;
        samples.push(batch);
    }

    let report = summarize(&base, &started_at, cli.requests, concurrency, &samples);
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

async fn login(
    client: &reqwest::Client,
    base: &str,
    username: &str,
    password: &str,
) -> anyhow::Result<String> {
    let response = client
        .post(format!("{base}/auth/login"))
        .json(&serde_json::json!({ "username": username, "password": password }))
        .send()
        .await?;
    if !response.status().is_success() {
        anyhow::bail!(
            "login failed with {}: check --username/--password",
            response.status()
        );
    }
    let body: LoginResponse = response.json().await?;
    Ok(body.token)
}

async fn probe_endpoint(
    client: &reqwest::Client,
    base: &str,
    endpoint: &str,
    count: u32,
    concurrency: u32,
    token: Option<&str>,
) -> anyhow::Result<ProbeBatch> {
    let url = format!("{base}{endpoint}");
    tracing::info!(endpoint, requests = count, "probing");
    let batch_start = Instant::now();
    let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(concurrency as usize));
    let mut handles = Vec::with_capacity(count as usize);

    for _ in 0..count {
        let permit = semaphore.clone().acquire_owned().await?;
        let client = client.clone();
        let url = url.clone();
        let token = token.map(str::to_string);
        let endpoint = endpoint.to_string();
        handles.push(tokio::spawn(async move {
            let start = Instant::now();
            let mut request = client.get(&url);
            if let Some(token) = &token {
                request = request.bearer_auth(token);
            }
            let status = match request.send().await {
                Ok(response) => response.status().as_u16(),
                Err(_) => 0,
            };
            let latency_ms = start.elapsed().as_secs_f64() * 1000.0;
            drop(permit);
            Sample {
                endpoint,
                status,
                latency_ms,
            }
        }));
    }

    let mut samples = Vec::with_capacity(count as usize);
    for handle in handles {
        samples.push(handle.await?);
    }
    Ok(ProbeBatch {
        samples,
        elapsed_secs: batch_start.elapsed().as_secs_f64(),
    })
}

fn summarize(
    base_url: &str,
    started_at: &str,
    requests: u32,
    concurrency: u32,
    samples: &[ProbeBatch],
) -> Report {
    let mut by_endpoint: BTreeMap<&str, &ProbeBatch> = BTreeMap::new();
    for batch in samples {
        if let Some(first) = batch.samples.first() {
            by_endpoint.insert(first.endpoint.as_str(), batch);
        }
    }

    let endpoints = by_endpoint
        .into_iter()
        .map(|(endpoint, batch)| {
            let mut latencies: Vec<f64> = batch.samples.iter().map(|s| s.latency_ms).collect();
            latencies.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let errors = batch
                .samples
                .iter()
                .filter(|s| s.status == 0 || s.status >= 500)
                .count();
            let window = batch.elapsed_secs.max(0.001);
            EndpointReport {
                endpoint: endpoint.to_string(),
                requests: latencies.len(),
                errors,
                p50_ms: percentile(&latencies, 0.50),
                p95_ms: percentile(&latencies, 0.95),
                p99_ms: percentile(&latencies, 0.99),
                max_ms: latencies.last().copied().unwrap_or(0.0),
                throughput_rps: latencies.len() as f64 / window,
            }
        })
        .collect();

    Report {
        base_url: base_url.to_string(),
        started_at: started_at.to_string(),
        requests_per_endpoint: requests,
        concurrency,
        endpoints,
    }
}

fn percentile(sorted: &[f64], q: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let index = ((sorted.len() as f64 - 1.0) * q).round() as usize;
    sorted[index.min(sorted.len() - 1)]
}

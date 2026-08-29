//! Runner agent

use crate::executor::{ExecutableJob, JobExecutor, JobStep};
use gitforge_common::{Error, JobId, PipelineRunId, Result, RunnerId};
use gitforge_db::models::Runner;
use gitforge_sandbox::{DockerSandbox, OutputSink, OutputStream, StepResult};
use gitforge_storage::ArtifactReceipt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fmt;
use std::path::Path;
use std::sync::Arc;
use tokio::fs;
use tokio::sync::{Mutex, RwLock};
use tokio::time::{interval, Duration};

/// Runner configuration
#[derive(Clone)]
pub struct RunnerConfig {
    /// Scheduler URL for job fetching
    pub scheduler_url: String,
    /// Runner name
    pub name: String,
    /// Runner type
    pub runner_type: String,
    /// Capacity (number of concurrent jobs)
    pub capacity: i32,
    /// Heartbeat interval in seconds
    pub heartbeat_interval_secs: u64,
    /// Job fetch interval in seconds
    pub fetch_interval_secs: u64,
    /// Bearer token used for scheduler service authentication.
    pub scheduler_token: Option<String>,
}

impl fmt::Debug for RunnerConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RunnerConfig")
            .field("scheduler_url", &self.scheduler_url)
            .field("name", &self.name)
            .field("runner_type", &self.runner_type)
            .field("capacity", &self.capacity)
            .field("heartbeat_interval_secs", &self.heartbeat_interval_secs)
            .field("fetch_interval_secs", &self.fetch_interval_secs)
            .field(
                "scheduler_token",
                &self.scheduler_token.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

impl Default for RunnerConfig {
    fn default() -> Self {
        Self {
            scheduler_url: std::env::var("GITFORGE_SCHEDULER_URL")
                .unwrap_or_else(|_| "http://localhost:42781".to_string()),
            name: "runner".to_string(),
            runner_type: "docker".to_string(),
            capacity: 2,
            heartbeat_interval_secs: 30,
            fetch_interval_secs: 5,
            scheduler_token: std::env::var("GITFORGE_SCHEDULER_TOKEN").ok(),
        }
    }
}

/// Job assignment from scheduler
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobAssignment {
    /// Job ID
    pub job_id: String,
    /// Job name
    pub name: String,
    /// Pipeline run ID
    pub pipeline_run_id: String,
    /// Commands to execute
    pub commands: Vec<String>,
    #[serde(default = "default_job_image")]
    pub image: String,
    /// Working directory
    pub working_dir: Option<String>,
}

fn default_job_image() -> String {
    "rust:latest".to_string()
}

/// Runner agent that fetches and executes jobs
#[derive(Clone)]
pub struct RunnerAgent {
    config: RunnerConfig,
    client: Client,
    runner: Option<Runner>,
    #[allow(dead_code)]
    sandbox: Arc<DockerSandbox>,
    executor: Arc<JobExecutor>,
    is_running: Arc<RwLock<bool>>,
}

impl RunnerAgent {
    /// Create a new runner agent
    pub async fn new(config: RunnerConfig) -> Result<Self> {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| Error::internal(format!("failed to create HTTP client: {}", e)))?;

        let sandbox = DockerSandbox::connect_required().await?;
        let executor = JobExecutor::new().await?;

        Ok(Self {
            config,
            client,
            runner: None,
            sandbox: Arc::new(sandbox),
            executor: Arc::new(executor),
            is_running: Arc::new(RwLock::new(false)),
        })
    }

    /// Register with the scheduler via HTTP
    pub async fn register(&mut self) -> Result<RunnerId> {
        let mut runner = Runner::new(
            self.config.name.clone(),
            gitforge_db::models::RunnerType::Docker,
            self.config.capacity,
        );

        // Try to register with scheduler via HTTP
        let register_url = format!("{}/runners", self.config.scheduler_url);
        let request = serde_json::json!({
            "name": runner.name,
            "type": runner.runner_type,
            "capacity": runner.capacity,
        });

        let mut register_request = self.client.post(&register_url).json(&request);
        if let Some(token) = &self.config.scheduler_token {
            register_request = register_request.bearer_auth(token);
        }
        match register_request.send().await {
            Ok(response) => {
                if response.status().is_success() {
                    if let Ok(payload) = response.json::<serde_json::Value>().await {
                        if let Some(id) = payload["id"]
                            .as_str()
                            .and_then(|value| uuid::Uuid::parse_str(value).ok())
                        {
                            runner.id = RunnerId::from(id);
                        }
                    }
                    tracing::info!("registered runner {} with scheduler", runner.id);
                } else {
                    if response.status() == reqwest::StatusCode::SERVICE_UNAVAILABLE
                        || response.status() == reqwest::StatusCode::UNAUTHORIZED
                    {
                        return Err(Error::internal(format!(
                            "scheduler authentication rejected registration: {}",
                            response.status()
                        )));
                    }
                    tracing::warn!(
                        "scheduler returned {} for registration, running in standalone mode",
                        response.status()
                    );
                }
            }
            Err(e) => {
                tracing::warn!(
                    "failed to register with scheduler: {}. Running in standalone mode.",
                    e
                );
            }
        }

        self.runner = Some(runner.clone());
        Ok(runner.id)
    }

    /// Start the runner agent loop
    pub async fn run(&self) -> Result<()> {
        *self.is_running.write().await = true;

        let runner = self
            .runner
            .as_ref()
            .ok_or_else(|| Error::internal("runner not registered"))?;

        let runner_id = runner.id;
        tracing::info!("runner {} starting", runner_id);

        // Start heartbeat loop
        let heartbeat_runner_id = runner_id;
        let heartbeat_interval = self.config.heartbeat_interval_secs;
        let heartbeat_client = self.client.clone();
        let heartbeat_url = self.config.scheduler_url.clone();
        let heartbeat_token = self.config.scheduler_token.clone();
        let is_running = self.is_running.clone();
        tokio::spawn(async move {
            let mut ticker = interval(Duration::from_secs(heartbeat_interval));
            loop {
                ticker.tick().await;
                if !*is_running.read().await {
                    tracing::debug!("heartbeat loop stopping");
                    break;
                }
                tracing::debug!("runner {} sending heartbeat", heartbeat_runner_id);
                let url = format!(
                    "{}/runners/{}/heartbeat",
                    heartbeat_url, heartbeat_runner_id
                );
                let mut heartbeat_request = heartbeat_client.post(&url);
                if let Some(token) = &heartbeat_token {
                    heartbeat_request = heartbeat_request.bearer_auth(token);
                }
                if let Err(e) = heartbeat_request.send().await {
                    tracing::trace!("heartbeat failed: {}", e);
                }
            }
        });

        // Start job fetch loop
        let fetch_interval = self.config.fetch_interval_secs;
        let fetch_client = self.client.clone();
        let fetch_url = self.config.scheduler_url.clone();
        let fetch_runner_id = runner_id;
        let fetch_token = self.config.scheduler_token.clone();
        let is_running = self.is_running.clone();
        let executor = self.executor.clone();
        let active_jobs: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));
        let active_jobs_for_loop = active_jobs.clone();
        tokio::spawn(async move {
            let mut ticker = interval(Duration::from_secs(fetch_interval));
            loop {
                ticker.tick().await;
                if !*is_running.read().await {
                    tracing::debug!("job fetch loop stopping");
                    break;
                }
                tracing::debug!("runner checking for jobs...");

                let jobs_url = format!("{}/jobs/pending?runner_id={}", fetch_url, fetch_runner_id);
                let mut fetch_request = fetch_client.get(&jobs_url);
                if let Some(token) = &fetch_token {
                    fetch_request = fetch_request.bearer_auth(token);
                }
                match fetch_request.send().await {
                    Ok(response) => {
                        if response.status().is_success() {
                            if let Ok(jobs) = response.json::<Vec<JobAssignment>>().await {
                                for job in jobs {
                                    tracing::info!(
                                        "received job assignment: {} ({})",
                                        job.name,
                                        job.job_id
                                    );
                                    let Some(lease_token) = Self::claim_job(
                                        &fetch_client,
                                        &fetch_url,
                                        &job.job_id,
                                        fetch_runner_id,
                                        fetch_token.as_deref(),
                                    )
                                    .await
                                    else {
                                        tracing::warn!("unable to claim job {}", job.job_id);
                                        continue;
                                    };
                                    {
                                        let mut active = active_jobs_for_loop.lock().await;
                                        if !active.insert(job.job_id.clone()) {
                                            tracing::warn!("job {} is already executing locally; skipping duplicate assignment", job.job_id);
                                            continue;
                                        }
                                    }
                                    // Execute concurrently so the fetch loop
                                    // remains responsive and cancellation can
                                    // be observed while the sandbox runs.
                                    let executor = executor.clone();
                                    let client = fetch_client.clone();
                                    let url = fetch_url.clone();
                                    let token = fetch_token.clone();
                                    let active_jobs = active_jobs_for_loop.clone();
                                    let active_job_id = job.job_id.clone();
                                    tokio::spawn(async move {
                                        Self::execute_job(
                                            &executor,
                                            &job,
                                            &client,
                                            &url,
                                            fetch_runner_id,
                                            &lease_token,
                                            token.as_deref(),
                                        )
                                        .await;
                                        active_jobs.lock().await.remove(&active_job_id);
                                    });
                                }
                            }
                        }
                    }
                    Err(e) => {
                        tracing::trace!("job fetch failed: {}", e);
                    }
                }
            }
        });

        // Keep running until stopped
        while *self.is_running.read().await {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }

        Ok(())
    }

    /// Stop the runner agent
    /// If force is true, cancel all running jobs immediately.
    /// Otherwise, wait for jobs to complete gracefully.
    pub async fn stop(&self, force: bool) {
        *self.is_running.write().await = false;

        if force {
            tracing::info!("force stopping - cancelling all active jobs");
            self.executor.cancel_all_jobs().await;
        }

        let runner_id = self
            .runner
            .as_ref()
            .map(|r| r.id.to_string())
            .unwrap_or_default();
        tracing::info!("runner {} stopped", runner_id);
    }

    /// Wait for all active jobs to complete within the given timeout
    pub async fn wait_for_jobs_complete(&self, timeout_duration: tokio::time::Duration) -> bool {
        self.executor.wait_for_jobs_complete(timeout_duration).await
    }

    /// Check if agent is running
    pub async fn is_running(&self) -> bool {
        *self.is_running.read().await
    }

    /// Execute a job assignment
    async fn claim_job(
        client: &Client,
        scheduler_url: &str,
        job_id: &str,
        runner_id: RunnerId,
        scheduler_token: Option<&str>,
    ) -> Option<String> {
        let url = format!("{}/jobs/{}/claim", scheduler_url, job_id);
        let mut request = client
            .post(url)
            .json(&serde_json::json!({"runner_id": runner_id.to_string()}));
        if let Some(token) = scheduler_token {
            request = request.bearer_auth(token);
        }
        let response = request.send().await.ok()?;
        if !response.status().is_success() {
            return None;
        }
        response
            .json::<serde_json::Value>()
            .await
            .ok()?
            .get("lease_token")
            .and_then(|token| token.as_str())
            .map(ToOwned::to_owned)
    }

    /// Execute a job assignment
    async fn execute_job(
        executor: &Arc<JobExecutor>,
        assignment: &JobAssignment,
        client: &Client,
        scheduler_url: &str,
        runner_id: RunnerId,
        lease_token: &str,
        scheduler_token: Option<&str>,
    ) {
        let job_id = match uuid::Uuid::parse_str(&assignment.job_id) {
            Ok(id) => JobId::from(id),
            Err(_) => {
                tracing::error!("invalid job_id: {}", assignment.job_id);
                return;
            }
        };

        // Convert assignment to ExecutableJob
        let pipeline_run_id = uuid::Uuid::parse_str(&assignment.pipeline_run_id)
            .map(PipelineRunId::from)
            .unwrap_or_else(|_| PipelineRunId::new());

        let executable = ExecutableJob {
            job_id,
            pipeline_run_id,
            repository_id: None,
            base_sha: None,
            image: assignment.image.clone(),
            steps: assignment
                .commands
                .iter()
                .map(|cmd| JobStep {
                    name: "run".to_string(),
                    run: cmd.clone(),
                    env: None,
                    working_directory: assignment.working_dir.clone(),
                })
                .collect(),
            env: std::collections::HashMap::new(),
            working_dir: assignment.working_dir.clone(),
            timeout_secs: 300,
        };

        tracing::info!("executing job {} in container", assignment.job_id);

        let started_url = format!("{}/jobs/{}/started", scheduler_url, assignment.job_id);
        let mut started_request = client.post(&started_url).json(&serde_json::json!({
            "runner_id": runner_id.to_string(),
            "lease_token": lease_token,
        }));
        if let Some(token) = scheduler_token {
            started_request = started_request.bearer_auth(token);
        }
        let started = started_request
            .send()
            .await
            .map(|response| response.status().is_success())
            .unwrap_or(false);
        if !started {
            tracing::error!("failed to mark job {} started", assignment.job_id);
            return;
        }

        let cancellation_client = client.clone();
        let cancellation_url = scheduler_url.to_string();
        let cancellation_job_id = assignment.job_id.clone();
        let cancellation_executor = executor.clone();
        let cancellation_token = scheduler_token.map(ToOwned::to_owned);
        let cancellation_watch = tokio::spawn(async move {
            let endpoint = format!(
                "{}/jobs/{}/cancelled",
                cancellation_url, cancellation_job_id
            );
            let mut probe_failures = 0u8;
            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
                let mut request = cancellation_client.get(&endpoint);
                if let Some(token) = &cancellation_token {
                    request = request.bearer_auth(token);
                }
                match request.send().await {
                    Ok(response) if response.status().is_success() => {
                        probe_failures = 0;
                        let cancelled = response
                            .json::<serde_json::Value>()
                            .await
                            .ok()
                            .and_then(|payload| payload["cancelled"].as_bool())
                            .unwrap_or(false);
                        if cancelled {
                            if let Ok(job_id) = uuid::Uuid::parse_str(&cancellation_job_id) {
                                let job_id = JobId::from(job_id);
                                if let Err(error) = cancellation_executor.cancel(&job_id).await {
                                    tracing::warn!(%error, %job_id, "failed to destroy cancelled sandbox");
                                }
                            }
                            break;
                        }
                    }
                    Ok(response) => {
                        probe_failures = probe_failures.saturating_add(1);
                        tracing::warn!(status = %response.status(), attempt = probe_failures, "job cancellation probe rejected");
                    }
                    Err(error) => {
                        probe_failures = probe_failures.saturating_add(1);
                        tracing::warn!(%error, attempt = probe_failures, "job cancellation probe failed");
                    }
                }
                if probe_failures >= 3 {
                    tracing::error!(
                        "cancellation probe unavailable repeatedly; stopping local job sandbox"
                    );
                    if let Ok(job_id) = uuid::Uuid::parse_str(&cancellation_job_id) {
                        let _ = cancellation_executor.cancel(&JobId::from(job_id)).await;
                    }
                    break;
                }
            }
        });

        // Execute the job. Output is sent to the scheduler while the sandbox
        // is running; the bounded sink applies network backpressure and never
        // changes the job's success result if observability is degraded.
        let live_logs = Arc::new(LiveLogSink::new(
            client,
            scheduler_url,
            &assignment.job_id,
            runner_id,
            lease_token,
            scheduler_token,
        ));
        let result = executor
            .execute_with_output(executable, Some(live_logs.clone()))
            .await;
        cancellation_watch.abort();

        tracing::info!(
            "job {} completed: success={}, exit_code={}",
            assignment.job_id,
            result.success,
            result.exit_code
        );

        let protocol = RunnerProtocol {
            client,
            scheduler_url,
            job_id: &assignment.job_id,
            runner_id,
            lease_token,
            scheduler_token,
        };
        if !live_logs.sent_any() {
            if let Err(error) = report_log_chunks(&protocol, &result.step_results).await {
                tracing::warn!(%error, job_id = %assignment.job_id, "failed to stream job logs");
            }
        } else if live_logs.failed() {
            tracing::warn!(job_id = %assignment.job_id, "live log delivery was degraded");
        }

        let uploaded_artifacts = match report_artifacts(
            &protocol,
            result.workspace_path.as_deref(),
            &result.artifacts,
        )
        .await
        {
            Ok(artifacts) => artifacts,
            Err(error) => {
                tracing::warn!(%error, job_id = %assignment.job_id, "failed to upload job artifacts");
                result
                    .artifacts
                    .iter()
                    .filter_map(|artifact| serde_json::to_value(artifact).ok())
                    .collect()
            }
        };

        // Report completion to scheduler with full results
        let complete_url = format!("{}/jobs/{}/complete", scheduler_url, assignment.job_id);

        // Build step results for reporting
        let step_results_json: Vec<serde_json::Value> = result
            .step_results
            .iter()
            .map(|sr| {
                serde_json::json!({
                    "exit_code": sr.exit_code,
                    "stdout": sr.stdout,
                    "stderr": sr.stderr,
                })
            })
            .collect();

        let complete_request = serde_json::json!({
            "contract_version": "harness.job.v1",
            "runner_id": runner_id.to_string(),
            "lease_token": lease_token,
            "success": result.success,
            "exit_code": result.exit_code,
            "error": result.error,
            "step_results": step_results_json,
            "artifacts": uploaded_artifacts,
        });

        let mut complete_request_builder = client.post(&complete_url).json(&complete_request);
        if let Some(token) = scheduler_token {
            complete_request_builder = complete_request_builder.bearer_auth(token);
        }
        match complete_request_builder.send().await {
            Ok(response) if response.status().is_success() => {}
            Ok(response) => {
                tracing::error!("scheduler rejected job completion: {}", response.status())
            }
            Err(error) => tracing::error!("failed to report job completion: {}", error),
        }
    }
}

struct LiveLogSink {
    client: Client,
    endpoint: String,
    runner_id: RunnerId,
    lease_token: String,
    scheduler_token: Option<String>,
    sent_chunks: std::sync::atomic::AtomicUsize,
    failed_delivery: std::sync::atomic::AtomicBool,
}

impl LiveLogSink {
    fn new(
        client: &Client,
        scheduler_url: &str,
        job_id: &str,
        runner_id: RunnerId,
        lease_token: &str,
        scheduler_token: Option<&str>,
    ) -> Self {
        Self {
            client: client.clone(),
            endpoint: format!("{scheduler_url}/jobs/{job_id}/logs"),
            runner_id,
            lease_token: lease_token.to_string(),
            scheduler_token: scheduler_token.map(ToOwned::to_owned),
            sent_chunks: std::sync::atomic::AtomicUsize::new(0),
            failed_delivery: std::sync::atomic::AtomicBool::new(false),
        }
    }

    fn sent_any(&self) -> bool {
        self.sent_chunks.load(std::sync::atomic::Ordering::Relaxed) > 0
    }

    fn failed(&self) -> bool {
        self.failed_delivery
            .load(std::sync::atomic::Ordering::Relaxed)
    }
}

#[async_trait::async_trait]
impl OutputSink for LiveLogSink {
    async fn on_output(&self, stream: OutputStream, chunk: Vec<u8>) -> gitforge_common::Result<()> {
        let label = match stream {
            OutputStream::Stdout => "stdout",
            OutputStream::Stderr => "stderr",
        };
        let text = String::from_utf8_lossy(&chunk);
        for part in utf8_chunks(&text, 60 * 1024) {
            let mut request = self.client.post(&self.endpoint).json(&serde_json::json!({
                "contract_version": "harness.job.v1",
                "runner_id": self.runner_id.to_string(),
                "lease_token": self.lease_token,
                "chunk": format!("[{label}]\n{part}"),
            }));
            if let Some(token) = &self.scheduler_token {
                request = request.bearer_auth(token);
            }
            match request.send().await {
                Ok(response) if response.status().is_success() => {
                    self.sent_chunks
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
                Ok(response) => {
                    self.failed_delivery
                        .store(true, std::sync::atomic::Ordering::Relaxed);
                    tracing::warn!(status = %response.status(), "scheduler rejected live log chunk");
                }
                Err(error) => {
                    self.failed_delivery
                        .store(true, std::sync::atomic::Ordering::Relaxed);
                    tracing::warn!(%error, "live log delivery failed");
                }
            }
        }
        Ok(())
    }
}

/// Upload final step output in bounded, UTF-8-safe chunks before completion.
/// The scheduler persists each chunk under the runner lease; this provides a
/// durable tail immediately before the terminal receipt while the sandbox
/// streaming interface is still being expanded.
struct RunnerProtocol<'a> {
    client: &'a Client,
    scheduler_url: &'a str,
    job_id: &'a str,
    runner_id: RunnerId,
    lease_token: &'a str,
    scheduler_token: Option<&'a str>,
}

async fn report_log_chunks(
    protocol: &RunnerProtocol<'_>,
    step_results: &[StepResult],
) -> anyhow::Result<()> {
    let endpoint = format!("{}/jobs/{}/logs", protocol.scheduler_url, protocol.job_id);
    for (index, result) in step_results.iter().enumerate() {
        let mut output = String::new();
        if !result.stdout.is_empty() {
            output.push_str(&format!("[step {index} stdout]\n{}\n", result.stdout));
        }
        if !result.stderr.is_empty() {
            output.push_str(&format!("[step {index} stderr]\n{}\n", result.stderr));
        }
        for chunk in utf8_chunks(&output, 60 * 1024) {
            let mut request = protocol.client.post(&endpoint).json(&serde_json::json!({
                "contract_version": "harness.job.v1",
                "runner_id": protocol.runner_id.to_string(),
                "lease_token": protocol.lease_token,
                "chunk": chunk,
            }));
            if let Some(token) = protocol.scheduler_token {
                request = request.bearer_auth(token);
            }
            let response = request.send().await?;
            if !response.status().is_success() {
                anyhow::bail!("scheduler rejected log append: {}", response.status());
            }
        }
    }
    Ok(())
}

fn utf8_chunks(value: &str, max_bytes: usize) -> Vec<&str> {
    if value.is_empty() {
        return Vec::new();
    }
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < value.len() {
        let mut end = (start + max_bytes).min(value.len());
        while end > start && !value.is_char_boundary(end) {
            end -= 1;
        }
        if end == start {
            end = value[start..]
                .char_indices()
                .nth(1)
                .map(|(offset, _)| start + offset)
                .unwrap_or(value.len());
        }
        chunks.push(&value[start..end]);
        start = end;
    }
    chunks
}

async fn report_artifacts(
    protocol: &RunnerProtocol<'_>,
    workspace_path: Option<&str>,
    artifacts: &[ArtifactReceipt],
) -> anyhow::Result<Vec<serde_json::Value>> {
    let Some(workspace_path) = workspace_path else {
        return Ok(Vec::new());
    };
    if artifacts.is_empty() {
        return Ok(Vec::new());
    }
    let artifact_root = fs::canonicalize(Path::new(workspace_path).join("artifacts")).await?;
    let endpoint = format!(
        "{}/jobs/{}/artifacts",
        protocol.scheduler_url, protocol.job_id
    );
    let mut uploaded = Vec::with_capacity(artifacts.len());
    for artifact in artifacts {
        let path = artifact_root.join(&artifact.name);
        let canonical = fs::canonicalize(&path).await?;
        if !canonical.starts_with(&artifact_root) {
            anyhow::bail!("artifact path escapes artifact directory");
        }
        let data = fs::read(&canonical).await?;
        let checksum = sha256_hex(&data);
        if checksum != artifact.sha256 {
            anyhow::bail!("artifact checksum changed before upload: {}", artifact.name);
        }
        let mut request = protocol
            .client
            .post(&endpoint)
            .header("x-runner-id", protocol.runner_id.to_string())
            .header("x-lease-token", protocol.lease_token)
            .header("x-artifact-name", &artifact.name)
            .header("x-artifact-sha256", &checksum)
            .body(data);
        if let Some(token) = protocol.scheduler_token {
            request = request.bearer_auth(token);
        }
        let response = request.send().await?;
        if !response.status().is_success() {
            anyhow::bail!("scheduler rejected artifact upload: {}", response.status());
        }
        uploaded.push(response.json::<serde_json::Value>().await?);
    }
    Ok(uploaded)
}

fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_utf8_chunks_preserve_boundaries() {
        let value = "ééé";
        let chunks = utf8_chunks(value, 3);
        assert_eq!(chunks.concat(), value);
        assert!(chunks.iter().all(|chunk| chunk.len() <= 3));
    }

    #[tokio::test]
    async fn test_runner_creation() {
        let config = RunnerConfig::default();
        let agent = RunnerAgent::new(config).await.unwrap();
        assert!(agent.runner.is_none());
    }

    #[tokio::test]
    async fn test_runner_config_default() {
        let config = RunnerConfig::default();
        assert_eq!(config.name, "runner");
        assert_eq!(config.capacity, 2);
        assert_eq!(config.heartbeat_interval_secs, 30);
        assert_eq!(config.fetch_interval_secs, 5);
    }

    #[tokio::test]
    async fn test_runner_register_no_scheduler() {
        // Test that register doesn't panic when scheduler is unavailable
        let config = RunnerConfig {
            scheduler_url: "http://localhost:99999".to_string(), // Invalid URL
            ..Default::default()
        };
        let mut agent = RunnerAgent::new(config).await.unwrap();
        let result = agent.register().await;
        assert!(result.is_ok());
        assert!(agent.runner.is_some());
    }

    #[tokio::test]
    async fn test_runner_agent_stop() {
        let config = RunnerConfig::default();
        let agent = RunnerAgent::new(config).await.unwrap();
        agent.stop(false).await;
        // No panic means success
    }

    #[test]
    fn test_runner_config_custom_values() {
        let config = RunnerConfig {
            scheduler_url: "http://custom:8081".to_string(),
            name: "custom-runner".to_string(),
            runner_type: "kubernetes".to_string(),
            capacity: 5,
            heartbeat_interval_secs: 60,
            fetch_interval_secs: 10,
            scheduler_token: None,
        };
        assert_eq!(config.name, "custom-runner");
        assert_eq!(config.capacity, 5);
        assert_eq!(config.heartbeat_interval_secs, 60);
        assert_eq!(config.fetch_interval_secs, 10);
    }

    #[test]
    fn test_job_assignment_serialization() {
        let assignment = JobAssignment {
            job_id: "job-123".to_string(),
            name: "build".to_string(),
            pipeline_run_id: "run-456".to_string(),
            commands: vec!["cargo build".to_string(), "cargo test".to_string()],
            image: "rust:latest".to_string(),
            working_dir: Some("/workspace".to_string()),
        };

        let json = serde_json::to_string(&assignment).unwrap();
        let deserialized: JobAssignment = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.job_id, "job-123");
        assert_eq!(deserialized.commands.len(), 2);
    }

    #[test]
    fn test_job_assignment_without_working_dir() {
        let assignment = JobAssignment {
            job_id: "job-456".to_string(),
            name: "test".to_string(),
            pipeline_run_id: "run-789".to_string(),
            commands: vec!["cargo test".to_string()],
            image: "rust:latest".to_string(),
            working_dir: None,
        };
        assert!(assignment.working_dir.is_none());
    }

    #[test]
    fn test_runner_config_debug() {
        let config = RunnerConfig::default();
        let debug_str = format!("{:?}", config);
        assert!(debug_str.contains("runner"));
    }

    #[test]
    fn test_job_assignment_debug() {
        let assignment = JobAssignment {
            job_id: "job-123".to_string(),
            name: "build".to_string(),
            pipeline_run_id: "run-456".to_string(),
            commands: vec!["cargo build".to_string()],
            image: "rust:latest".to_string(),
            working_dir: None,
        };
        let debug_str = format!("{:?}", assignment);
        assert!(debug_str.contains("job-123"));
    }

    #[test]
    fn test_runner_config_clone() {
        let config = RunnerConfig::default();
        let cloned = config.clone();
        assert_eq!(cloned.name, config.name);
        assert_eq!(cloned.capacity, config.capacity);
    }

    #[test]
    fn test_runner_config_partial_eq() {
        let config1 = RunnerConfig::default();
        let config2 = RunnerConfig::default();
        assert_eq!(config1.name, config2.name);
        assert_eq!(config1.capacity, config2.capacity);
    }

    #[test]
    fn test_job_assignment_serde_roundtrip() {
        let assignment = JobAssignment {
            job_id: "job-123".to_string(),
            name: "build".to_string(),
            pipeline_run_id: "run-456".to_string(),
            commands: vec!["cargo build".to_string(), "cargo test".to_string()],
            image: "rust:latest".to_string(),
            working_dir: Some("/workspace".to_string()),
        };

        // Test JSON serialization
        let json = serde_json::to_string(&assignment).unwrap();
        assert!(json.contains("job-123"));
        assert!(json.contains("build"));

        // Test deserialization
        let deserialized: JobAssignment = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.job_id, assignment.job_id);
        assert_eq!(deserialized.name, assignment.name);
        assert_eq!(deserialized.commands, assignment.commands);
        assert_eq!(deserialized.working_dir, assignment.working_dir);
    }

    #[test]
    fn test_job_assignment_empty_commands() {
        let assignment = JobAssignment {
            job_id: "job-empty".to_string(),
            name: "noop".to_string(),
            pipeline_run_id: "run-001".to_string(),
            commands: vec![],
            image: "rust:latest".to_string(),
            working_dir: None,
        };
        assert!(assignment.commands.is_empty());
        assert!(assignment.working_dir.is_none());
    }

    #[test]
    fn test_job_assignment_multiple_commands() {
        let commands = vec![
            "cargo fetch".to_string(),
            "cargo build --release".to_string(),
            "cargo test".to_string(),
            "cargo clippy".to_string(),
        ];
        let assignment = JobAssignment {
            job_id: "job-multi".to_string(),
            name: "full-pipeline".to_string(),
            pipeline_run_id: "run-002".to_string(),
            commands,
            working_dir: Some("/project".to_string()),
        };
        assert_eq!(assignment.commands.len(), 4);
    }

    #[tokio::test]
    async fn test_runner_agent_not_registered() {
        // Agent without registration should have runner as None
        let config = RunnerConfig::default();
        let agent = RunnerAgent::new(config).await.unwrap();
        assert!(agent.runner.is_none());
    }

    #[tokio::test]
    async fn test_runner_register_sets_runner() {
        let config = RunnerConfig {
            scheduler_url: "http://localhost:99999".to_string(),
            ..Default::default()
        };
        let mut agent = RunnerAgent::new(config).await.unwrap();

        let result = agent.register().await;
        assert!(result.is_ok());
        assert!(agent.runner.is_some());

        // Verify runner has correct properties
        let runner = agent.runner.as_ref().unwrap();
        assert_eq!(runner.name, "runner");
        assert_eq!(runner.capacity, 2);
    }

    #[test]
    fn test_runner_config_all_fields() {
        let config = RunnerConfig {
            scheduler_url: "http://example.com:8081".to_string(),
            name: "test-runner".to_string(),
            runner_type: "firecracker".to_string(),
            capacity: 8,
            heartbeat_interval_secs: 15,
            fetch_interval_secs: 3,
            scheduler_token: None,
        };

        assert_eq!(config.scheduler_url, "http://example.com:8081");
        assert_eq!(config.name, "test-runner");
        assert_eq!(config.runner_type, "firecracker");
        assert_eq!(config.capacity, 8);
        assert_eq!(config.heartbeat_interval_secs, 15);
        assert_eq!(config.fetch_interval_secs, 3);
    }

    #[test]
    fn test_runner_config_default_is_docker() {
        let config = RunnerConfig::default();
        assert_eq!(config.runner_type, "docker");
    }

    #[test]
    fn test_runner_config_default_heartbeat() {
        let config = RunnerConfig::default();
        // Default heartbeat is 30 seconds
        assert_eq!(config.heartbeat_interval_secs, 30);
        // Default fetch interval is 5 seconds
        assert_eq!(config.fetch_interval_secs, 5);
    }

    #[test]
    fn test_runner_agent_debug() {
        // We can't easily create a running agent for debug test
        // but we can verify the type implements Debug
        let config = RunnerConfig::default();
        assert!(format!("{:?}", config).contains("RunnerConfig"));
    }

    #[test]
    fn test_job_assignment_equality() {
        let assignment1 = JobAssignment {
            job_id: "job-1".to_string(),
            name: "build".to_string(),
            pipeline_run_id: "run-1".to_string(),
            commands: vec!["echo 1".to_string()],
            image: "rust:latest".to_string(),
            working_dir: None,
        };
        let assignment2 = JobAssignment {
            job_id: "job-1".to_string(),
            name: "build".to_string(),
            pipeline_run_id: "run-1".to_string(),
            commands: vec!["echo 1".to_string()],
            image: "rust:latest".to_string(),
            working_dir: None,
        };
        // JobAssignment should implement PartialEq if we add it
        // For now just verify individual field equality
        assert_eq!(assignment1.job_id, assignment2.job_id);
        assert_eq!(assignment1.name, assignment2.name);
    }

    #[test]
    fn test_job_assignment_serialize_with_minimal_fields() {
        let assignment = JobAssignment {
            job_id: "minimal-job".to_string(),
            name: "test".to_string(),
            pipeline_run_id: "run-min".to_string(),
            commands: vec!["true".to_string()],
            image: "rust:latest".to_string(),
            working_dir: None,
        };

        let json = serde_json::to_string(&assignment).unwrap();
        assert!(json.contains("minimal-job"));
        assert!(json.contains("test"));
        assert!(json.contains("minimal-job"));
    }

    #[tokio::test]
    async fn test_runner_stop_when_not_running() {
        let config = RunnerConfig::default();
        let agent = RunnerAgent::new(config).await.unwrap();
        // Stop without running should not panic
        agent.stop(false).await;
    }

    #[tokio::test]
    async fn test_runner_stop_after_registration() {
        let config = RunnerConfig {
            scheduler_url: "http://localhost:99999".to_string(),
            ..Default::default()
        };
        let mut agent = RunnerAgent::new(config).await.unwrap();
        agent.register().await.unwrap();
        // Stop after registration should not panic
        agent.stop(false).await;
    }

    #[test]
    fn test_runner_config_all_default_values() {
        let config = RunnerConfig::default();
        // Verify all default values
        assert_eq!(config.scheduler_url, "http://localhost:42781");
        assert_eq!(config.name, "runner");
        assert_eq!(config.runner_type, "docker");
        assert_eq!(config.capacity, 2);
        assert_eq!(config.heartbeat_interval_secs, 30);
        assert_eq!(config.fetch_interval_secs, 5);
    }

    #[test]
    fn test_runner_config_with_zero_capacity() {
        let config = RunnerConfig {
            scheduler_url: "http://localhost:42781".to_string(),
            name: "zero-cap".to_string(),
            runner_type: "docker".to_string(),
            capacity: 0,
            heartbeat_interval_secs: 30,
            fetch_interval_secs: 5,
            scheduler_token: None,
        };
        assert_eq!(config.capacity, 0);
    }

    #[test]
    fn test_job_assignment_with_many_commands() {
        let commands: Vec<String> = (0..100).map(|i| format!("echo step{}", i)).collect();
        let assignment = JobAssignment {
            job_id: "job-many".to_string(),
            name: "many-steps".to_string(),
            pipeline_run_id: "run-many".to_string(),
            commands,
            image: "rust:latest".to_string(),
            working_dir: None,
        };
        assert_eq!(assignment.commands.len(), 100);
    }

    #[test]
    fn test_job_assignment_clone() {
        let assignment = JobAssignment {
            job_id: "clone-test".to_string(),
            name: "test".to_string(),
            pipeline_run_id: "run-1".to_string(),
            commands: vec!["echo clone".to_string()],
            image: "rust:latest".to_string(),
            working_dir: None,
        };
        let cloned = assignment.clone();
        assert_eq!(cloned.job_id, assignment.job_id);
        assert_eq!(cloned.commands, assignment.commands);
    }

    #[test]
    fn test_job_assignment_with_unicode_in_name() {
        let assignment = JobAssignment {
            job_id: "job-unicode".to_string(),
            name: "测试任务".to_string(),
            pipeline_run_id: "run-unicode".to_string(),
            commands: vec!["echo 测试".to_string()],
            image: "rust:latest".to_string(),
            working_dir: None,
        };
        assert_eq!(assignment.name, "测试任务");
    }

    #[test]
    fn test_job_assignment_with_special_chars_in_commands() {
        let assignment = JobAssignment {
            job_id: "special-cmd".to_string(),
            name: "special".to_string(),
            pipeline_run_id: "run-special".to_string(),
            commands: vec![
                "echo $HOME".to_string(),
                "echo \"quoted\"".to_string(),
                "echo 'single'".to_string(),
            ],
            image: "rust:latest".to_string(),
            working_dir: None,
        };
        assert_eq!(assignment.commands.len(), 3);
    }

    #[test]
    fn test_runner_config_with_special_url() {
        let config = RunnerConfig {
            scheduler_url: "http://user:pass@host:9090/path".to_string(),
            name: "special-url-runner".to_string(),
            runner_type: "docker".to_string(),
            capacity: 4,
            heartbeat_interval_secs: 45,
            fetch_interval_secs: 10,
            scheduler_token: None,
        };
        assert!(config.scheduler_url.contains("user:pass"));
    }

    #[test]
    fn test_job_assignment_deserialize() {
        let json = r#"{
            "job_id": "deserialized-job",
            "name": "deserialized",
            "pipeline_run_id": "run-123",
            "commands": ["cargo build", "cargo test"],
            "working_dir": "/workspace"
        }"#;
        let assignment: JobAssignment = serde_json::from_str(json).unwrap();
        assert_eq!(assignment.job_id, "deserialized-job");
        assert_eq!(assignment.commands.len(), 2);
    }

    #[tokio::test]
    async fn test_runner_agent_with_custom_config() {
        let config = RunnerConfig {
            scheduler_url: "http://custom-scheduler:8081".to_string(),
            name: "custom-runner".to_string(),
            runner_type: "kubernetes".to_string(),
            capacity: 10,
            heartbeat_interval_secs: 60,
            fetch_interval_secs: 15,
            scheduler_token: None,
        };
        let agent = RunnerAgent::new(config).await.unwrap();
        assert!(agent.runner.is_none());
    }

    #[test]
    fn test_runner_config_clone_is_independent() {
        let config1 = RunnerConfig::default();
        let mut config2 = config1.clone();
        config2.name = "modified".to_string();
        assert_ne!(config1.name, config2.name);
    }

    #[test]
    fn test_job_assignment_with_empty_working_dir() {
        let assignment = JobAssignment {
            job_id: "empty-wd".to_string(),
            name: "test".to_string(),
            pipeline_run_id: "run-1".to_string(),
            commands: vec!["echo test".to_string()],
            image: "rust:latest".to_string(),
            working_dir: Some("".to_string()),
        };
        assert!(assignment.working_dir.is_some());
    }

    #[tokio::test]
    async fn test_runner_is_running() {
        let config = RunnerConfig::default();
        let agent = RunnerAgent::new(config).await.unwrap();
        assert!(!agent.is_running().await);
    }

    #[tokio::test]
    async fn test_runner_run_and_stop() {
        let config = RunnerConfig {
            scheduler_url: "http://localhost:99999".to_string(),
            ..Default::default()
        };

        // Create and register agent
        let mut agent = RunnerAgent::new(config.clone()).await.unwrap();
        agent.register().await.unwrap();

        // Clone for use in spawn
        let agent_clone = agent.clone();

        // Start run in background
        let run_handle = tokio::spawn(async move { agent_clone.run().await });

        // Wait for start
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Check is_running
        assert!(agent.is_running().await);

        // Stop it
        agent.stop(false).await;

        // Give it time to shutdown
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        // Verify run completed
        let result = run_handle.await.unwrap();
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_runner_run_requires_registration() {
        let config = RunnerConfig::default();
        let agent = RunnerAgent::new(config).await.unwrap();

        // Agent is not registered, run should fail
        let result = agent.run().await;
        assert!(result.is_err());
    }
}

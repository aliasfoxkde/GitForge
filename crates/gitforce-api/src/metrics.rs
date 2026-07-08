//! Prometheus metrics for GitForge API
//!
//! Provides observability into API operations, CI/CD pipelines, and runner status.

use prometheus::{
    Counter, Histogram, HistogramOpts, HistogramVec, IntCounterVec, IntGauge, Opts, Registry,
};

/// GitForge metrics collector
pub struct Metrics {
    pub registry: Registry,

    // HTTP metrics
    pub http_requests_total: IntCounterVec,
    pub http_request_duration_seconds: HistogramVec,

    // Repository metrics
    pub repos_total: IntGauge,
    pub repo_operations_total: IntCounterVec,

    // CI/CD metrics
    pub pipelines_total: IntGauge,
    pub pipeline_runs_total: IntCounterVec,
    pub jobs_total: IntGauge,
    pub job_duration_seconds: Histogram,

    // Runner metrics
    pub runners_online: IntGauge,
    pub runners_busy: IntGauge,
    pub runners_offline: IntGauge,
    pub job_assignments_total: Counter,

    // Event metrics
    pub events_published_total: IntCounterVec,
    pub events_consumed_total: IntCounterVec,

    // Storage metrics
    pub artifacts_total: IntGauge,
    pub artifact_size_bytes: Histogram,
}

impl Metrics {
    /// Create a new metrics collector
    pub fn new() -> Self {
        let registry = Registry::new();

        // HTTP metrics
        let http_requests_total = IntCounterVec::new(
            Opts::new("http_requests_total", "Total HTTP requests"),
            &["method", "endpoint", "status"],
        )
        .expect("failed to create http_requests_total counter");
        registry
            .register(Box::new(http_requests_total.clone()))
            .expect("failed to register http_requests_total");

        let http_request_duration_seconds = HistogramVec::new(
            HistogramOpts::new(
                "http_request_duration_seconds",
                "HTTP request duration in seconds",
            )
            .buckets(vec![0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0]),
            &["method", "endpoint"],
        )
        .expect("failed to create http_request_duration_seconds histogram");
        registry
            .register(Box::new(http_request_duration_seconds.clone()))
            .expect("failed to register http_request_duration_seconds");

        // Repository metrics
        let repos_total = IntGauge::new("gitforge_repos_total", "Total number of repositories")
            .expect("failed to create repos_total gauge");
        registry
            .register(Box::new(repos_total.clone()))
            .expect("failed to register repos_total");

        let repo_operations_total = IntCounterVec::new(
            Opts::new("gitforge_repo_operations_total", "Total repository operations"),
            &["operation"],
        )
        .expect("failed to create repo_operations_total counter");
        registry
            .register(Box::new(repo_operations_total.clone()))
            .expect("failed to register repo_operations_total");

        // CI/CD metrics
        let pipelines_total = IntGauge::new("gitforge_pipelines_total", "Total number of pipelines")
            .expect("failed to create pipelines_total gauge");
        registry
            .register(Box::new(pipelines_total.clone()))
            .expect("failed to register pipelines_total");

        let pipeline_runs_total = IntCounterVec::new(
            Opts::new("gitforge_pipeline_runs_total", "Total pipeline runs"),
            &["status"],
        )
        .expect("failed to create pipeline_runs_total counter");
        registry
            .register(Box::new(pipeline_runs_total.clone()))
            .expect("failed to register pipeline_runs_total");

        let jobs_total = IntGauge::new("gitforge_jobs_total", "Total number of jobs")
            .expect("failed to create jobs_total gauge");
        registry
            .register(Box::new(jobs_total.clone()))
            .expect("failed to register jobs_total");

        let job_duration_seconds = Histogram::with_opts(
            HistogramOpts::new("gitforge_job_duration_seconds", "Job duration in seconds")
                .buckets(vec![1.0, 5.0, 10.0, 30.0, 60.0, 120.0, 300.0, 600.0, 1800.0]),
        )
        .expect("failed to create job_duration_seconds histogram");
        registry
            .register(Box::new(job_duration_seconds.clone()))
            .expect("failed to register job_duration_seconds");

        // Runner metrics
        let runners_online = IntGauge::new("gitforge_runners_online", "Number of online runners")
            .expect("failed to create runners_online gauge");
        registry
            .register(Box::new(runners_online.clone()))
            .expect("failed to register runners_online");

        let runners_busy = IntGauge::new("gitforge_runners_busy", "Number of busy runners")
            .expect("failed to create runners_busy gauge");
        registry
            .register(Box::new(runners_busy.clone()))
            .expect("failed to register runners_busy");

        let runners_offline = IntGauge::new("gitforge_runners_offline", "Number of offline runners")
            .expect("failed to create runners_offline gauge");
        registry
            .register(Box::new(runners_offline.clone()))
            .expect("failed to register runners_offline");

        let job_assignments_total =
            Counter::new("gitforge_job_assignments_total", "Total job assignments")
                .expect("failed to create job_assignments_total counter");
        registry
            .register(Box::new(job_assignments_total.clone()))
            .expect("failed to register job_assignments_total");

        // Event metrics
        let events_published_total = IntCounterVec::new(
            Opts::new("gitforge_events_published_total", "Total events published"),
            &["event_type"],
        )
        .expect("failed to create events_published_total counter");
        registry
            .register(Box::new(events_published_total.clone()))
            .expect("failed to register events_published_total");

        let events_consumed_total = IntCounterVec::new(
            Opts::new("gitforge_events_consumed_total", "Total events consumed"),
            &["event_type"],
        )
        .expect("failed to create events_consumed_total counter");
        registry
            .register(Box::new(events_consumed_total.clone()))
            .expect("failed to register events_consumed_total");

        // Storage metrics
        let artifacts_total = IntGauge::new("gitforge_artifacts_total", "Total number of artifacts")
            .expect("failed to create artifacts_total gauge");
        registry
            .register(Box::new(artifacts_total.clone()))
            .expect("failed to register artifacts_total");

        let artifact_size_bytes = Histogram::with_opts(
            HistogramOpts::new("gitforge_artifact_size_bytes", "Artifact size in bytes")
                .buckets(vec![1024.0, 10240.0, 102400.0, 1048576.0, 10485760.0, 104857600.0]),
        )
        .expect("failed to create artifact_size_bytes histogram");
        registry
            .register(Box::new(artifact_size_bytes.clone()))
            .expect("failed to register artifact_size_bytes");

        Self {
            registry,
            http_requests_total,
            http_request_duration_seconds,
            repos_total,
            repo_operations_total,
            pipelines_total,
            pipeline_runs_total,
            jobs_total,
            job_duration_seconds,
            runners_online,
            runners_busy,
            runners_offline,
            job_assignments_total,
            events_published_total,
            events_consumed_total,
            artifacts_total,
            artifact_size_bytes,
        }
    }

    /// Increment HTTP request counter
    pub fn record_http_request(&self, method: &str, endpoint: &str, status: u16) {
        self.http_requests_total
            .with_label_values(&[method, endpoint, &status.to_string()])
            .inc();
    }

    /// Record HTTP request duration
    pub fn record_http_duration(&self, method: &str, endpoint: &str, duration_secs: f64) {
        self.http_request_duration_seconds
            .with_label_values(&[method, endpoint])
            .observe(duration_secs);
    }

    /// Increment repository operation counter
    pub fn record_repo_operation(&self, operation: &str) {
        self.repo_operations_total
            .with_label_values(&[operation])
            .inc();
    }

    /// Increment pipeline run counter
    pub fn record_pipeline_run(&self, status: &str) {
        self.pipeline_runs_total
            .with_label_values(&[status])
            .inc();
    }

    /// Record job duration
    pub fn record_job_duration(&self, duration_secs: f64) {
        self.job_duration_seconds.observe(duration_secs);
    }

    /// Record job assignment
    pub fn record_job_assignment(&self) {
        self.job_assignments_total.inc();
    }

    /// Increment event published counter
    pub fn record_event_published(&self, event_type: &str) {
        self.events_published_total
            .with_label_values(&[event_type])
            .inc();
    }

    /// Increment event consumed counter
    pub fn record_event_consumed(&self, event_type: &str) {
        self.events_consumed_total
            .with_label_values(&[event_type])
            .inc();
    }

    /// Record artifact size
    pub fn record_artifact_size(&self, size_bytes: u64) {
        self.artifact_size_bytes.observe(size_bytes as f64);
    }
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics_creation() {
        let metrics = Metrics::new();
        // Verify metrics were created by recording and gathering
        metrics.record_http_request("GET", "/health", 200);
        let families = metrics.registry.gather();
        assert!(!families.is_empty());
    }

    #[test]
    fn test_record_http_request() {
        let metrics = Metrics::new();
        metrics.record_http_request("GET", "/health", 200);
        metrics.record_http_request("POST", "/repos", 201);
    }

    #[test]
    fn test_record_repo_operation() {
        let metrics = Metrics::new();
        metrics.record_repo_operation("create");
        metrics.record_repo_operation("delete");
    }

    #[test]
    fn test_record_pipeline_run() {
        let metrics = Metrics::new();
        metrics.record_pipeline_run("succeeded");
        metrics.record_pipeline_run("failed");
    }

    #[test]
    fn test_record_job_duration() {
        let metrics = Metrics::new();
        metrics.record_job_duration(5.0);
        metrics.record_job_duration(120.5);
    }

    #[test]
    fn test_record_event() {
        let metrics = Metrics::new();
        metrics.record_event_published("push.received");
        metrics.record_event_consumed("push.received");
    }

    #[test]
    fn test_record_artifact_size() {
        let metrics = Metrics::new();
        metrics.record_artifact_size(1024);
        metrics.record_artifact_size(10485760);
    }
}

    #[test]
    fn test_record_runner_counts() {
        let metrics = Metrics::new();
        metrics.record_http_request("GET", "/runners", 200);
        metrics.record_http_request("POST", "/runners", 201);
    }

    #[test]
    fn test_record_artifact_operations() {
        let metrics = Metrics::new();
        metrics.record_http_request("GET", "/artifacts", 200);
        metrics.record_http_request("DELETE", "/artifacts/123", 204);
    }

    #[test]
    fn test_record_event_publishing() {
        let metrics = Metrics::new();
        metrics.record_event_published("push.received");
        metrics.record_event_published("pipeline.triggered");
        metrics.record_event_published("job.started");
        metrics.record_event_published("job.finished");
    }

    #[test]
    fn test_record_event_consumption() {
        let metrics = Metrics::new();
        metrics.record_event_consumed("push.received");
        metrics.record_event_consumed("pipeline.triggered");
    }

    #[test]
    fn test_record_http_duration() {
        let metrics = Metrics::new();
        metrics.record_http_duration("GET", "/health", 0.001);
        metrics.record_http_duration("POST", "/repos", 0.05);
        metrics.record_http_duration("GET", "/pipelines", 0.025);
    }

    #[test]
    fn test_record_multiple_job_assignments() {
        let metrics = Metrics::new();
        metrics.record_job_assignment();
        metrics.record_job_assignment();
        metrics.record_job_assignment();
    }

    #[test]
    fn test_metrics_all_counters() {
        let metrics = Metrics::new();
        metrics.record_http_request("GET", "/health", 200);
        metrics.record_http_request("POST", "/repos", 201);
        metrics.record_http_request("GET", "/repos", 200);
        metrics.record_http_request("DELETE", "/repos/123", 204);
        metrics.record_repo_operation("create");
        metrics.record_repo_operation("get");
        metrics.record_repo_operation("delete");
        metrics.record_pipeline_run("pending");
        metrics.record_pipeline_run("running");
        metrics.record_pipeline_run("succeeded");
        metrics.record_pipeline_run("failed");
        metrics.record_job_duration(1.5);
        metrics.record_job_duration(30.0);
        metrics.record_job_duration(120.5);
        metrics.record_event_published("push");
        metrics.record_event_consumed("push");
        metrics.record_artifact_size(1024);
        metrics.record_artifact_size(1048576);
    }

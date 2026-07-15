//! Integration tests for GitForge API
//!
//! These tests verify API functionality.

use gitforce_api::metrics::Metrics;

#[test]
fn test_metrics_creation() {
    let _metrics = Metrics::new();
}

#[test]
fn test_metrics_record_http_request() {
    let metrics = Metrics::new();
    metrics.record_http_request("GET", "/health", 200);
    metrics.record_http_request("POST", "/api/repos", 201);
}

#[test]
fn test_metrics_record_http_duration() {
    let metrics = Metrics::new();
    metrics.record_http_duration("GET", "/api/repos", 0.05);
    metrics.record_http_duration("POST", "/api/repos", 0.123);
}

#[test]
fn test_metrics_record_job_assignment() {
    let metrics = Metrics::new();
    metrics.record_job_assignment();
    metrics.record_job_assignment();
}

#[test]
fn test_metrics_record_job_duration() {
    let metrics = Metrics::new();
    metrics.record_job_duration(5.0);
    metrics.record_job_duration(120.5);
}

#[test]
fn test_metrics_record_pipeline_run() {
    let metrics = Metrics::new();
    metrics.record_pipeline_run("succeeded");
    metrics.record_pipeline_run("failed");
}

#[test]
fn test_metrics_record_repo_operation() {
    let metrics = Metrics::new();
    metrics.record_repo_operation("create");
    metrics.record_repo_operation("delete");
}

#[test]
fn test_metrics_record_artifact_size() {
    let metrics = Metrics::new();
    metrics.record_artifact_size(1024);
    metrics.record_artifact_size(10485760);
}

#[test]
fn test_metrics_record_event() {
    let metrics = Metrics::new();
    metrics.record_event_published("push.received");
    metrics.record_event_consumed("push.received");
}

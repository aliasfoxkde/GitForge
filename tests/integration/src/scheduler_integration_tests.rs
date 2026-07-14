//! Integration tests for the Scheduler HTTP API
//!
//! These tests verify the scheduler's HTTP endpoints including:
//! - Runner registration
//! - Runner heartbeat
//! - Job assignment
//! - Job completion

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use tower::util::ServiceExt;
use gitforce_scheduler::{Scheduler, create_state, scheduler_routes};
use crate::integration_test_helpers::*;

/// Helper to create a test scheduler app
fn create_test_app() -> (Scheduler, axum::Router) {
    let scheduler = Scheduler::new();
    let state = create_state(scheduler.clone());
    let app = scheduler_routes(state);
    (scheduler, app)
}

#[tokio::test]
async fn test_register_runner_endpoint() {
    let (_scheduler, app) = create_test_app();

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/runners")
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"name":"test-runner","type":"docker","capacity":4}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CREATED);
}

#[tokio::test]
async fn test_register_runner_firecracker_type() {
    let (_scheduler, app) = create_test_app();

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/runners")
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"name":"firecracker-runner","type":"firecracker","capacity":2}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CREATED);
}

#[tokio::test]
async fn test_register_runner_bare_metal_type() {
    let (_scheduler, app) = create_test_app();

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/runners")
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"name":"baremetal-runner","type":"bare-metal","capacity":8}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CREATED);
}

#[tokio::test]
async fn test_register_runner_default_type() {
    let (_scheduler, app) = create_test_app();

    // Unknown type should default to docker
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/runners")
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"name":"unknown-type-runner","type":"unknown","capacity":1}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CREATED);
}

#[tokio::test]
async fn test_runner_heartbeat_invalid_id() {
    let (_scheduler, app) = create_test_app();

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/runners/not-a-valid-uuid/heartbeat")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_get_pending_jobs() {
    let (_scheduler, app) = create_test_app();

    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/jobs/pending")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_assign_job_endpoint() {
    let (_scheduler, app) = create_test_app();

    let job_id = uuid::Uuid::new_v4();
    let runner_id = uuid::Uuid::new_v4();

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&format!("/jobs/{}/assign", job_id))
                .header("Content-Type", "application/json")
                .body(Body::from(format!(r#"{{"runner_id":"{}"}}"#, runner_id)))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_assign_job_invalid_job_id() {
    let (_scheduler, app) = create_test_app();

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/jobs/not-a-uuid/assign")
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"runner_id":"550e8400-e29b-41d4-a716-446655440000"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_complete_job_success() {
    let (_scheduler, app) = create_test_app();

    let job_id = uuid::Uuid::new_v4();

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&format!("/jobs/{}/complete", job_id))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"success":true}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_complete_job_failure() {
    let (_scheduler, app) = create_test_app();

    let job_id = uuid::Uuid::new_v4();

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&format!("/jobs/{}/complete", job_id))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"success":false}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_complete_job_default_success() {
    let (_scheduler, app) = create_test_app();

    let job_id = uuid::Uuid::new_v4();

    // Missing success field should default to false
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&format!("/jobs/{}/complete", job_id))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"other":"field"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_complete_job_invalid_job_id() {
    let (_scheduler, app) = create_test_app();

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/jobs/invalid-uuid/complete")
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"success":true}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}
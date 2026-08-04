//! Integration tests for the HTTP API.

use axum::{
    body::Body,
    http::StatusCode,
};
use axum::extract::Request;
use tower::util::ServiceExt;
use project_name::api::handlers::create_router;

#[tokio::test]
#[ignore = "integration test - run with `cargo test --test integration`"]
async fn health_returns_ok() {
    let app = create_router();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
#[ignore = "integration test - run with `cargo test --test integration`"]
async fn hello_returns_greeting() {
    let app = create_router();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/hello")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
#[ignore = "integration test - run with `cargo test --test integration`"]
async fn echo_returns_body() {
    let app = create_router();
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/echo")
                .header("Content-Type", "text/plain")
                .body(Body::from("test message"))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

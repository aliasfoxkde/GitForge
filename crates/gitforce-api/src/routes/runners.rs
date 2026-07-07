//! Runner API routes

use axum::{
    extract::Path,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use gitforce_common::RunnerId;
use serde::{Deserialize, Serialize};

/// Runner response
#[derive(Debug, Serialize, Deserialize)]
pub struct RunnerResponse {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub runner_type: String,
    pub status: String,
    pub capacity: i32,
    pub last_heartbeat: Option<String>,
}

/// Runner routes
pub fn runner_routes<S: Clone + Send + Sync + 'static>() -> Router<S> {
    Router::new()
        .route("/runners", get(list_runners))
        .route("/runners", post(register_runner))
        .route("/runners/:id", get(get_runner))
}

/// List runners
async fn list_runners() -> impl IntoResponse {
    Json(serde_json::Value::Array(vec![]))
}

/// Register a runner
async fn register_runner(Json(payload): Json<serde_json::Value>) -> impl IntoResponse {
    tracing::debug!("register runner: {:?}", payload);
    (StatusCode::CREATED, Json(RunnerResponse {
        id: RunnerId::new().to_string(),
        name: payload["name"].as_str().unwrap_or("runner").to_string(),
        runner_type: payload["type"].as_str().unwrap_or("docker").to_string(),
        status: "online".to_string(),
        capacity: payload["capacity"].as_i64().unwrap_or(1) as i32,
        last_heartbeat: Some(chrono::Utc::now().to_rfc3339()),
    }))
}

/// Get runner
async fn get_runner(Path(id): Path<String>) -> impl IntoResponse {
    tracing::debug!("get runner: {}", id);
    (StatusCode::OK, Json(RunnerResponse {
        id,
        name: "test-runner".to_string(),
        runner_type: "docker".to_string(),
        status: "online".to_string(),
        capacity: 2,
        last_heartbeat: Some(chrono::Utc::now().to_rfc3339()),
    }))
}

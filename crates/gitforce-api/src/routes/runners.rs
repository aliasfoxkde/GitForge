//! Runner API routes

use axum::{
    extract::{Extension, Path},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use gitforce_common::RunnerId;
use gitforce_db::{Pool, queries::RunnerQueries, models::RunnerType};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

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
        .route("/runners", get(list_runners).post(register_runner))
        .route("/runners/{id}", get(get_runner))
}

/// List runners
async fn list_runners(
    Extension(pool): Extension<Arc<Pool>>,
) -> impl IntoResponse {
    match RunnerQueries::list(&pool).await {
        Ok(runners) => {
            let response: Vec<RunnerResponse> = runners.into_iter().map(|r| RunnerResponse {
                id: r.id.to_string(),
                name: r.name,
                runner_type: r.runner_type,
                status: r.status,
                capacity: r.capacity,
                last_heartbeat: r.last_heartbeat.map(|dt| dt.to_rfc3339()),
            }).collect();
            return Json(response).into_response();
        }
        Err(e) => {
            tracing::error!("failed to list runners: {}", e);
        }
    }
    Json(serde_json::Value::Array(vec![])).into_response()
}

/// Register a runner
async fn register_runner(
    Extension(pool): Extension<Arc<Pool>>,
    Json(payload): Json<serde_json::Value>,
) -> impl IntoResponse {
    tracing::debug!("register runner: {:?}", payload);

    let name = payload["name"].as_str().unwrap_or("runner").to_string();
    let runner_type = payload["type"].as_str().unwrap_or("docker").to_string();
    let capacity = payload["capacity"].as_i64().unwrap_or(1) as i32;

    let rt = match runner_type.as_str() {
        "docker" => RunnerType::Docker,
        "firecracker" => RunnerType::Firecracker,
        "bare_metal" | "baremetal" => RunnerType::BareMetal,
        _ => RunnerType::Docker,
    };

    let runner = gitforce_db::models::Runner::new(name, rt, capacity);

    match RunnerQueries::create(&pool, &runner).await {
        Ok(_) => {
            let response = RunnerResponse {
                id: runner.id.to_string(),
                name: runner.name,
                runner_type: runner.runner_type,
                status: runner.status,
                capacity: runner.capacity,
                last_heartbeat: runner.last_heartbeat.map(|dt| dt.to_rfc3339()),
            };
            return (StatusCode::CREATED, Json(response)).into_response();
        }
        Err(e) => {
            tracing::error!("failed to register runner: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({
                "error": "database_error",
                "message": format!("failed to register runner: {}", e)
            }))).into_response();
        }
    }
}

/// Get runner
async fn get_runner(
    Extension(pool): Extension<Arc<Pool>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    tracing::debug!("get runner: {}", id);

    let response = match Uuid::parse_str(&id) {
        Ok(uuid) => {
            let runner_id = RunnerId::from(uuid);
            match RunnerQueries::get(&pool, runner_id).await {
                Ok(Some(runner)) => {
                    return (StatusCode::OK, Json(RunnerResponse {
                        id: runner.id.to_string(),
                        name: runner.name,
                        runner_type: runner.runner_type,
                        status: runner.status,
                        capacity: runner.capacity,
                        last_heartbeat: runner.last_heartbeat.map(|dt| dt.to_rfc3339()),
                    })).into_response();
                }
                Ok(None) => {
                    serde_json::json!({
                        "error": "not_found",
                        "message": "Runner not found"
                    })
                }
                Err(e) => {
                    tracing::error!("failed to get runner: {}", e);
                    serde_json::json!({
                        "error": "database_error",
                        "message": format!("failed to get runner: {}", e)
                    })
                }
            }
        }
        Err(_) => {
            serde_json::json!({
                "error": "invalid_id",
                "message": "Invalid runner ID format"
            })
        }
    };
    (StatusCode::BAD_REQUEST, Json(response)).into_response()
}

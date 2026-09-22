//! Runner API routes

use crate::middleware::AuthenticatedUser;
use axum::{
    extract::{Extension, Path},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use gitforge_common::RunnerId;
use gitforge_db::{
    models::RunnerType,
    queries::{RunnerQueries, RunnerRegistration, RunnerRetirement},
    Pool,
};
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

impl From<gitforge_db::models::Runner> for RunnerResponse {
    fn from(runner: gitforge_db::models::Runner) -> Self {
        Self {
            id: runner.id.to_string(),
            name: runner.name,
            runner_type: runner.runner_type,
            status: runner.status,
            capacity: runner.capacity,
            last_heartbeat: runner.last_heartbeat.map(|dt| dt.to_rfc3339()),
        }
    }
}

/// Runner routes
pub fn runner_routes<S: Clone + Send + Sync + 'static>() -> Router<S> {
    Router::new()
        .route("/runners", get(list_runners))
        .route("/runners/{id}", get(get_runner).delete(retire_runner))
}

/// Runner registration is a bootstrap endpoint used before a runner has a
/// user JWT. It is mounted separately from protected runner administration.
pub fn public_runner_routes<S: Clone + Send + Sync + 'static>() -> Router<S> {
    Router::new().route("/runners", axum::routing::post(register_runner))
}

/// List runners
async fn list_runners(
    _user: AuthenticatedUser,
    Extension(pool): Extension<Arc<Pool>>,
) -> impl IntoResponse {
    match RunnerQueries::list(&pool).await {
        Ok(runners) => {
            let response: Vec<RunnerResponse> =
                runners.into_iter().map(RunnerResponse::from).collect();
            Json(response).into_response()
        }
        Err(e) => {
            tracing::error!("failed to list runners: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": "database_error",
                    "message": "failed to list runners"
                })),
            )
                .into_response()
        }
    }
}

/// Register a runner (also used by runner agent - allows unauthenticated)
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

    let runner = gitforge_db::models::Runner::new(name, rt, capacity);

    // Registration keys on the runner's stable name: a restart adopts the
    // existing row (200 with the adopted id) instead of minting a duplicate
    // identity, so heartbeats keep landing on one registry entry.
    match RunnerQueries::register_or_refresh(&pool, &runner).await {
        Ok((adopted, RunnerRegistration::Created)) => {
            (StatusCode::CREATED, Json(RunnerResponse::from(adopted))).into_response()
        }
        Ok((adopted, RunnerRegistration::Refreshed)) => {
            (StatusCode::OK, Json(RunnerResponse::from(adopted))).into_response()
        }
        Err(e) => {
            tracing::error!("failed to register runner: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": "database_error",
                    "message": format!("failed to register runner: {}", e)
                })),
            )
                .into_response()
        }
    }
}

/// Get runner
async fn get_runner(
    _user: AuthenticatedUser,
    Extension(pool): Extension<Arc<Pool>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    tracing::debug!("get runner: {}", id);

    match Uuid::parse_str(&id) {
        Ok(uuid) => match RunnerQueries::get(&pool, RunnerId::from(uuid)).await {
            Ok(Some(runner)) => {
                (StatusCode::OK, Json(RunnerResponse::from(runner))).into_response()
            }
            Ok(None) => (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({
                    "error": "not_found",
                    "message": "Runner not found"
                })),
            )
                .into_response(),
            Err(e) => {
                tracing::error!("failed to get runner: {}", e);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({
                        "error": "database_error",
                        "message": format!("failed to get runner: {}", e)
                    })),
                )
                    .into_response()
            }
        },
        Err(_) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "invalid_id",
                "message": "Invalid runner ID format"
            })),
        )
            .into_response(),
    }
}

/// Retire an idle runner while preserving its database record for audit and
/// historical pipeline visibility. Only administrators and maintainers may
/// perform this operation.
async fn retire_runner(
    user: AuthenticatedUser,
    Extension(pool): Extension<Arc<Pool>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if !matches!(user.claims.role.as_str(), "admin" | "maintainer") {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({
                "error": "forbidden",
                "message": "Only administrators and maintainers may retire runners"
            })),
        )
            .into_response();
    }

    let runner_id = match Uuid::parse_str(&id) {
        Ok(uuid) => RunnerId::from(uuid),
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": "invalid_id",
                    "message": "Invalid runner ID format"
                })),
            )
                .into_response();
        }
    };

    match RunnerQueries::retire_if_idle(&pool, runner_id).await {
        Ok(RunnerRetirement::Retired | RunnerRetirement::AlreadyRetired) => {
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(RunnerRetirement::ActiveJobs(count)) => (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": "runner_busy",
                "message": "Runner has active jobs; wait for completion before retiring it",
                "active_jobs": count
            })),
        )
            .into_response(),
        Ok(RunnerRetirement::NotFound) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": "not_found",
                "message": "Runner not found"
            })),
        )
            .into_response(),
        Err(error) => {
            tracing::error!(%error, %runner_id, "failed to retire runner");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": "database_error",
                    "message": "Failed to retire runner"
                })),
            )
                .into_response()
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    /// The runner listing wire contract: exact field names (note the
    /// `type` rename) and the heartbeat timestamp present only when the
    /// runner has reported one.
    #[test]
    fn runner_response_wire_contract() {
        let json = serde_json::to_value(RunnerResponse {
            id: "runner-1".to_string(),
            name: "edge-runner".to_string(),
            runner_type: "docker".to_string(),
            status: "online".to_string(),
            capacity: 4,
            last_heartbeat: Some("2026-01-01T00:00:00Z".to_string()),
        })
        .unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "id": "runner-1",
                "name": "edge-runner",
                "type": "docker",
                "status": "online",
                "capacity": 4,
                "last_heartbeat": "2026-01-01T00:00:00Z"
            })
        );

        let idle = serde_json::to_value(RunnerResponse {
            last_heartbeat: None,
            ..serde_json::from_value::<RunnerResponse>(json).unwrap()
        })
        .unwrap();
        assert!(idle["last_heartbeat"].is_null());
    }
}

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
use gitforge_db::{models::RunnerType, queries::RunnerQueries, Pool};
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
        .route("/runners", get(list_runners))
        .route("/runners/{id}", get(get_runner))
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
            let response: Vec<RunnerResponse> = runners
                .into_iter()
                .map(|r| RunnerResponse {
                    id: r.id.to_string(),
                    name: r.name,
                    runner_type: r.runner_type,
                    status: r.status,
                    capacity: r.capacity,
                    last_heartbeat: r.last_heartbeat.map(|dt| dt.to_rfc3339()),
                })
                .collect();
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
            (StatusCode::CREATED, Json(response)).into_response()
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
            Ok(Some(runner)) => (
                StatusCode::OK,
                Json(RunnerResponse {
                    id: runner.id.to_string(),
                    name: runner.name,
                    runner_type: runner.runner_type,
                    status: runner.status,
                    capacity: runner.capacity,
                    last_heartbeat: runner.last_heartbeat.map(|dt| dt.to_rfc3339()),
                }),
            )
                .into_response(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_runner_response_serialization() {
        let response = RunnerResponse {
            id: "runner-123".to_string(),
            name: "docker-runner-1".to_string(),
            runner_type: "docker".to_string(),
            status: "online".to_string(),
            capacity: 4,
            last_heartbeat: Some("2024-01-01T00:00:00Z".to_string()),
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("runner-123"));
        assert!(json.contains("docker"));
        assert!(json.contains("4"));
    }

    #[test]
    fn test_runner_response_deserialization() {
        let json = r#"{
            "id": "runner-456",
            "name": "firecracker-runner",
            "type": "firecracker",
            "status": "busy",
            "capacity": 8,
            "last_heartbeat": "2024-01-01T12:30:00Z"
        }"#;
        let response: RunnerResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.id, "runner-456");
        assert_eq!(response.name, "firecracker-runner");
        assert_eq!(response.runner_type, "firecracker");
        assert_eq!(response.capacity, 8);
    }

    #[test]
    fn test_runner_response_without_heartbeat() {
        let response = RunnerResponse {
            id: "runner-new".to_string(),
            name: "new-runner".to_string(),
            runner_type: "docker".to_string(),
            status: "online".to_string(),
            capacity: 2,
            last_heartbeat: None,
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("runner-new"));
        // The field is still present in JSON but as null
        assert!(json.contains("null") || !json.contains("last_heartbeat"));
    }

    #[test]
    fn test_runner_response_all_statuses() {
        for status in &["online", "offline", "busy"] {
            let response = RunnerResponse {
                id: "runner-1".to_string(),
                name: "test".to_string(),
                runner_type: "docker".to_string(),
                status: status.to_string(),
                capacity: 1,
                last_heartbeat: None,
            };
            let json = serde_json::to_string(&response).unwrap();
            assert!(json.contains(status));
        }
    }

    #[test]
    fn test_runner_response_different_types() {
        for rt in &["docker", "firecracker", "bare_metal"] {
            let response = RunnerResponse {
                id: "runner-1".to_string(),
                name: "test".to_string(),
                runner_type: rt.to_string(),
                status: "online".to_string(),
                capacity: 4,
                last_heartbeat: None,
            };
            let json = serde_json::to_string(&response).unwrap();
            assert!(json.contains(rt));
        }
    }

    #[test]
    fn test_runner_response_debug() {
        let response = RunnerResponse {
            id: "runner-debug".to_string(),
            name: "debug-runner".to_string(),
            runner_type: "docker".to_string(),
            status: "online".to_string(),
            capacity: 4,
            last_heartbeat: Some("2024-01-01T00:00:00Z".to_string()),
        };
        let debug_str = format!("{:?}", response);
        assert!(debug_str.contains("runner-debug"));
    }

    #[test]
    fn test_runner_response_all_capacities() {
        for capacity in &[1, 2, 4, 8, 16, 32] {
            let response = RunnerResponse {
                id: "runner-cap".to_string(),
                name: "capacity-test".to_string(),
                runner_type: "docker".to_string(),
                status: "online".to_string(),
                capacity: *capacity,
                last_heartbeat: None,
            };
            assert_eq!(response.capacity, *capacity);
        }
    }

    #[test]
    fn test_runner_response_with_special_heartbeat() {
        let response = RunnerResponse {
            id: "runner-hb".to_string(),
            name: "heartbeat-test".to_string(),
            runner_type: "firecracker".to_string(),
            status: "busy".to_string(),
            capacity: 4,
            last_heartbeat: Some("2026-07-16T22:00:00Z".to_string()),
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("2026-07-16T22:00:00Z"));
    }

    #[test]
    fn test_runner_response_all_runner_types() {
        for rt in &["docker", "firecracker", "bare_metal", "kubernetes"] {
            let response = RunnerResponse {
                id: "runner-rt".to_string(),
                name: "type-test".to_string(),
                runner_type: rt.to_string(),
                status: "online".to_string(),
                capacity: 2,
                last_heartbeat: None,
            };
            let json = serde_json::to_string(&response).unwrap();
            assert!(json.contains(rt));
        }
    }

    #[test]
    fn test_runner_response_all_capacities_values() {
        for cap in &[0, 1, 2, 4, 8, 16, 32, 64] {
            let response = RunnerResponse {
                id: "runner-cap".to_string(),
                name: "capacity-test".to_string(),
                runner_type: "docker".to_string(),
                status: "online".to_string(),
                capacity: *cap,
                last_heartbeat: None,
            };
            assert_eq!(response.capacity, *cap);
        }
    }

    #[test]
    fn test_runner_response_special_characters_in_name() {
        let response = RunnerResponse {
            id: "runner-special".to_string(),
            name: "runner-with-dashes_and_underscores".to_string(),
            runner_type: "docker".to_string(),
            status: "online".to_string(),
            capacity: 2,
            last_heartbeat: None,
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("runner-with-dashes_and_underscores"));
    }

    #[test]
    fn test_runner_response_timestamp_formats() {
        let timestamps = [
            "2024-01-01T00:00:00Z",
            "2024-12-31T23:59:59Z",
            "2026-07-23T12:00:00Z",
        ];
        for ts in &timestamps {
            let response = RunnerResponse {
                id: "runner-ts".to_string(),
                name: "timestamp-test".to_string(),
                runner_type: "firecracker".to_string(),
                status: "busy".to_string(),
                capacity: 4,
                last_heartbeat: Some(ts.to_string()),
            };
            let json = serde_json::to_string(&response).unwrap();
            assert!(json.contains(ts));
        }
    }

    #[test]
    fn test_runner_response_id_formats() {
        // UUID format
        let response = RunnerResponse {
            id: "550e8400-e29b-41d4-a716-446655440000".to_string(),
            name: "uuid-runner".to_string(),
            runner_type: "docker".to_string(),
            status: "online".to_string(),
            capacity: 2,
            last_heartbeat: None,
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("550e8400-e29b-41d4-a716-446655440000"));
    }

    #[test]
    fn test_runner_response_statuses() {
        let statuses = ["online", "offline", "busy", "draining", "unknown"];
        for status in &statuses {
            let response = RunnerResponse {
                id: "runner-status".to_string(),
                name: "status-test".to_string(),
                runner_type: "docker".to_string(),
                status: status.to_string(),
                capacity: 1,
                last_heartbeat: None,
            };
            assert_eq!(response.status, *status);
        }
    }

    #[test]
    fn test_runner_response_debug_format() {
        let response = RunnerResponse {
            id: "runner-debug".to_string(),
            name: "debug-runner".to_string(),
            runner_type: "docker".to_string(),
            status: "online".to_string(),
            capacity: 4,
            last_heartbeat: None,
        };
        let debug_str = format!("{:?}", response);
        assert!(debug_str.contains("runner-debug"));
        assert!(debug_str.contains("debug-runner"));
    }

    #[test]
    fn test_runner_response_id_various_formats() {
        // Test various ID formats
        let ids = vec![
            "simple",
            "with-dashes",
            "with_underscores",
            "with.dots",
            "UPPERCASE",
            "MixedCase123",
        ];
        for id in ids {
            let response = RunnerResponse {
                id: id.to_string(),
                name: "test".to_string(),
                runner_type: "docker".to_string(),
                status: "online".to_string(),
                capacity: 1,
                last_heartbeat: None,
            };
            assert_eq!(response.id, id);
        }
    }

    #[test]
    fn test_runner_response_name_with_spaces() {
        let response = RunnerResponse {
            id: "runner-1".to_string(),
            name: "My Docker Runner".to_string(),
            runner_type: "docker".to_string(),
            status: "online".to_string(),
            capacity: 2,
            last_heartbeat: None,
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("My Docker Runner"));
    }
}

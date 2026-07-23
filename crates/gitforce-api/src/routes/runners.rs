//! Runner API routes

use crate::auth::ApiAuth;
use axum::{
    extract::{Extension, Path},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use gitforce_common::RunnerId;
use gitforce_db::{models::RunnerType, queries::RunnerQueries, Pool};
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

/// Helper to extract and validate user from headers
fn extract_user(auth: &ApiAuth, headers: &HeaderMap) -> Result<(), StatusCode> {
    let auth_header = headers.get("Authorization").and_then(|v| v.to_str().ok());

    let token = auth_header
        .and_then(|h| ApiAuth::extract_token(h))
        .ok_or(StatusCode::UNAUTHORIZED)?;

    auth.validate_token(token)
        .map_err(|_| StatusCode::UNAUTHORIZED)?;

    Ok(())
}

/// List runners
async fn list_runners(
    Extension(pool): Extension<Arc<Pool>>,
    Extension(auth): Extension<Arc<ApiAuth>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    match extract_user(&auth, &headers) {
        Err(e) => e.into_response(),
        Ok(_) => match RunnerQueries::list(&pool).await {
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
                Json(serde_json::Value::Array(vec![])).into_response()
            }
        },
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
    Extension(pool): Extension<Arc<Pool>>,
    Extension(auth): Extension<Arc<ApiAuth>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match extract_user(&auth, &headers) {
        Err(e) => e.into_response(),
        Ok(_) => {
            tracing::debug!("get runner: {}", id);

            match Uuid::parse_str(&id) {
                Ok(uuid) => {
                    let runner_id = RunnerId::from(uuid);
                    match RunnerQueries::get(&pool, runner_id).await {
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
                    }
                }
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
    fn test_extract_user_without_auth_header() {
        use crate::auth::ApiAuth;

        let auth = ApiAuth::new("test-secret");
        let headers = HeaderMap::new();
        let result = extract_user(&auth, &headers);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn test_extract_user_with_invalid_token() {
        use crate::auth::ApiAuth;

        let auth = ApiAuth::new("test-secret");
        let mut headers = HeaderMap::new();
        headers.insert("Authorization", "Bearer invalid-token".parse().unwrap());
        let result = extract_user(&auth, &headers);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn test_extract_user_with_valid_token() {
        use crate::auth::ApiAuth;
        use gitforce_common::UserId;

        let auth = ApiAuth::new("test-secret");
        let user_id = UserId::new();
        let token = auth.generate_token(user_id, "testuser", "user").unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(
            "Authorization",
            format!("Bearer {}", token).parse().unwrap(),
        );
        let result = extract_user(&auth, &headers);
        assert!(result.is_ok());
    }
}

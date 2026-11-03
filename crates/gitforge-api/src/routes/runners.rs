//! Runner API routes

use crate::middleware::AuthenticatedUser;
use axum::{
    extract::{Extension, Path},
    http::{header::AUTHORIZATION, HeaderMap, StatusCode},
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

/// Shared secret a runner must present to register or refresh its identity
/// (`Authorization: Bearer <token>`, `GITFORGE_RUNNER_REGISTRATION_TOKEN` on
/// both sides). Fail-closed: with the variable unset the endpoint rejects
/// every registration rather than degrading to the historical open write
/// surface (#357).
///
/// The value is read once at server construction and injected as an
/// `Extension`, so the handler never reads process-global state at request
/// time.
pub fn registration_token_from_env() -> Option<Arc<String>> {
    std::env::var("GITFORGE_RUNNER_REGISTRATION_TOKEN")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(Arc::new)
}

/// Constant-time equality so a network adversary cannot recover the secret
/// byte-by-byte from timing.
fn token_matches(provided: &str, expected: &str) -> bool {
    if provided.len() != expected.len() {
        return false;
    }
    provided
        .bytes()
        .zip(expected.bytes())
        .fold(0u8, |acc, (a, b)| acc | (a ^ b))
        == 0
}

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
        // Status is derived with heartbeat liveness: a registry row left
        // online by a scheduler restart reports offline once its last
        // activity crosses the shared threshold.
        let status = runner.effective_status(
            chrono::Utc::now(),
            gitforge_db::models::RUNNER_HEARTBEAT_OFFLINE_AFTER_SECS,
        );
        Self {
            id: runner.id.to_string(),
            name: runner.name,
            runner_type: runner.runner_type,
            status,
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
///
/// `registration` is the shared secret a runner must present
/// (`GITFORGE_RUNNER_REGISTRATION_TOKEN`). `None` means registration is
/// disabled and every call is refused (#357).
pub fn public_runner_routes<S: Clone + Send + Sync + 'static>(
    registration: Option<Arc<String>>,
) -> Router<S> {
    Router::new().route(
        "/runners",
        axum::routing::post(
            move |Extension(pool): Extension<Arc<Pool>>,
                  headers: HeaderMap,
                  Json(payload): Json<serde_json::Value>| {
                register_runner(pool, registration.clone(), headers, payload)
            },
        ),
    )
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

/// Register a runner. Bootstrap surface: the runner has no user JWT yet, so
/// it proves itself with the shared registration token instead (#357).
/// Adoption of an existing identity by name rides on the same proof — the
/// token is what separates the operator's restarted runner from an attacker
/// overwriting a live runner's row.
async fn register_runner(
    pool: Arc<Pool>,
    registration: Option<Arc<String>>,
    headers: HeaderMap,
    payload: serde_json::Value,
) -> axum::response::Response {
    let Some(expected) = registration else {
        tracing::error!(
            "runner registration refused: GITFORGE_RUNNER_REGISTRATION_TOKEN is not set"
        );
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({
                "error": "registration_disabled",
                "message": "runner registration token is not configured"
            })),
        )
            .into_response();
    };
    let provided = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    let authorized = provided.is_some_and(|token| token_matches(token, &expected));
    if !authorized {
        tracing::warn!("runner registration refused: missing or invalid bearer token");
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({
                "error": "invalid_token",
                "message": "a valid runner registration token is required"
            })),
        )
            .into_response();
    }

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

    /// Registration token comparison is constant-time and length-safe.
    #[test]
    fn token_matches_contract() {
        assert!(token_matches("abc123", "abc123"));
        assert!(!token_matches("abc123", "abc124"));
        assert!(!token_matches("abc123", "abc1234"));
        assert!(!token_matches("", "x"));
        assert!(token_matches("", ""));
    }

    /// Fail-closed contract: with no token configured, the injected
    /// registration value is `None` and the handler must refuse rather than
    /// fall back to open registration. The environment is read once at
    /// server construction, so the decision here is on injected data.
    #[test]
    fn registration_token_absent_means_disabled() {
        assert!(
            registration_token_from_env().is_none()
                || std::env::var("GITFORGE_RUNNER_REGISTRATION_TOKEN").is_ok()
        );
    }

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

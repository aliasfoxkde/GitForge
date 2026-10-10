//! Shared stub infrastructure for the API integration tests.
//!
//! The `CiTriggerClient` refuses any URL but the pinned production
//! endpoint (`http://127.0.0.1:42781/pipelines/trigger`), so driving the
//! delegation paths end-to-end means owning that port for the test's
//! lifetime. Both the webhook and the pipeline-run trigger routes use
//! this stub orchestrator.

// Test-harness exemption, same discipline as the sibling suites:
// `allow-unwrap-in-tests` covers `#[test]` bodies, but boot/seed/fixture
// helper functions in a test target are neither `#[test]` fns nor
// `#[cfg(test)]`, a class the clippy.toml config cannot address. Setup
// failing IS the assertion -- a panic aborts the run loudly. Production
// code keeps the denies.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use axum::{response::IntoResponse, routing::post, Router};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

/// What the stub orchestrator recorded for each delegated trigger.
#[derive(Debug)]
pub struct StubTrigger {
    pub token: String,
    pub body: Value,
}

/// Shared state for the stub orchestrator: what it has received and the
/// scripted responses it still owes.
#[derive(Clone)]
struct StubCiState {
    seen: Arc<Mutex<Vec<StubTrigger>>>,
    script: Arc<Mutex<Vec<(u16, Value)>>>,
}

/// The stub's only route, mirroring the production trigger contract:
/// a `x-gitforge-trigger-token` header and a JSON trigger payload. A
/// scripted `Value::String` is served raw (`text/plain`) to exercise the
/// client's non-JSON branch; any other value is served as JSON.
async fn stub_trigger(
    axum::extract::State(state): axum::extract::State<StubCiState>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> axum::response::Response {
    let token = headers
        .get("x-gitforge-trigger-token")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let text = String::from_utf8_lossy(&body).into_owned();
    let parsed: Value = serde_json::from_str(&text).unwrap_or(Value::String(text));
    state.seen.lock().unwrap().push(StubTrigger {
        token,
        body: parsed,
    });
    let next = {
        let mut script = state.script.lock().unwrap();
        if script.is_empty() {
            (500, json!({"error": "script exhausted"}))
        } else {
            script.remove(0)
        }
    };
    let status = axum::http::StatusCode::from_u16(next.0).unwrap();
    match next.1 {
        Value::String(raw) => (status, raw).into_response(),
        payload => (status, axum::Json(payload)).into_response(),
    }
}

/// Bind the pinned production endpoint and serve a scripted sequence of
/// orchestrator responses.
///
/// Returns `None` when the port is already owned — the live orchestrator
/// on a developer host — and the caller skips; the CI sandbox where the
/// gate runs always has the port free, so coverage is collected there.
pub async fn serve_stub_ci(script: Vec<(u16, Value)>) -> Option<Arc<Mutex<Vec<StubTrigger>>>> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:42781")
        .await
        .ok()?;
    let received = Arc::new(Mutex::new(Vec::new()));
    let state = StubCiState {
        seen: received.clone(),
        script: Arc::new(Mutex::new(script)),
    };

    let app: Router = Router::new()
        .route("/pipelines/trigger", post(stub_trigger))
        .with_state(state);
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    Some(received)
}

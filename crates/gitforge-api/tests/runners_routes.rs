//! Integration tests for the runner registry routes.
//!
//! Registration is the bootstrap endpoint a runner calls before it has a
//! user JWT, so it is unauthenticated and keys on the runner's stable name
//! (restart adopts, never duplicates). Administration (list/get/retire)
//! sits behind the protected router with the admin-or-maintainer retire
//! policy. These tests drive the real router with an in-memory database.

use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Router,
};
use gitforge_api::{ApiAuth, ApiServer};
use gitforge_common::{PipelineId, RunnerId};
use gitforge_db::{
    models::{Job, JobStatus, Pipeline, PipelineRun, Repository, User},
    queries::{
        JobQueries, PipelineQueries, PipelineRunQueries, RepoQueries, RunnerQueries, UserQueries,
    },
    Pool,
};
use serde_json::{json, Value};
use tower::ServiceExt;

struct Fixture {
    app: Router,
    pool: Pool,
    admin_token: String,
    developer_token: String,
    repo_id: gitforge_common::RepoId,
    pipeline_id: PipelineId,
}

async fn seed() -> Fixture {
    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();

    let admin = User::new(
        "runner-admin".to_string(),
        "runner-admin@example.com".to_string(),
        "hash".to_string(),
    );
    let developer = User::new(
        "runner-dev".to_string(),
        "runner-dev@example.com".to_string(),
        "hash".to_string(),
    );
    for user in [&admin, &developer] {
        UserQueries::create(&pool, user).await.unwrap();
    }
    assert!(UserQueries::set_role(&pool, admin.id, "admin")
        .await
        .unwrap());

    let repo = Repository::new(
        "runner-routes-repo".to_string(),
        admin.id,
        "/git/runner-routes-repo".to_string(),
    );
    let repo_id = repo.id;
    RepoQueries::create(&pool, &repo).await.unwrap();

    let pipeline = Pipeline {
        id: PipelineId::new(),
        repo_id,
        name: "runner-routes-pipeline".to_string(),
        trigger_type: "push".to_string(),
        config: json!({"name": "runner-routes-pipeline", "version": "1.0", "jobs": []}),
        created_at: chrono::Utc::now(),
    };
    let pipeline_id = pipeline.id;
    PipelineQueries::create(&pool, &pipeline).await.unwrap();

    let auth = ApiAuth::new("test-secret");
    let token_for =
        |user: &User, role: &str| auth.generate_token(user.id, &user.username, role).unwrap();
    let app = ApiServer::new("test-secret", pool.clone()).into_router();
    Fixture {
        app,
        pool,
        admin_token: token_for(&admin, "admin"),
        developer_token: token_for(&developer, "developer"),
        repo_id,
        pipeline_id,
    }
}

async fn request_json(
    app: Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("Content-Type", "application/json");
    if let Some(token) = token {
        builder = builder.header("Authorization", format!("Bearer {token}"));
    }
    let response = app
        .oneshot(
            builder
                .body(Body::from(body.map(|v| v.to_string()).unwrap_or_default()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes)
            .unwrap_or(Value::String(String::from_utf8_lossy(&bytes).into_owned()))
    };
    (status, parsed)
}

/// Register a runner and return its status code and response body.
async fn register(app: Router, body: Value) -> (StatusCode, Value) {
    request_json(app, "POST", "/api/runners", None, Some(body)).await
}

#[tokio::test]
async fn register_creates_then_adopts_by_stable_name() {
    let f = seed().await;
    let payload = json!({"name": "edge-runner", "type": "docker", "capacity": 3});

    let (status, created) = register(f.app.clone(), payload.clone()).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["name"], "edge-runner");
    assert_eq!(created["type"], "docker");
    assert_eq!(created["capacity"], 3);
    let adopted_id = created["id"].as_str().unwrap().to_string();

    // A restart re-registers under the same name: same identity, refreshed
    // state — a duplicate row would strand future heartbeats.
    let (status, refreshed) = register(f.app.clone(), payload).await;
    assert_eq!(status, StatusCode::OK, "{refreshed}");
    assert_eq!(refreshed["id"].as_str().unwrap(), adopted_id);

    let fleet = RunnerQueries::list(&f.pool).await.unwrap();
    assert_eq!(fleet.len(), 1, "one name maps to exactly one registry row");
}

#[tokio::test]
async fn register_maps_type_aliases_and_defaults() {
    let f = seed().await;
    for (submitted, expected) in [
        ("firecracker", "firecracker"),
        ("bare_metal", "bare_metal"),
        ("baremetal", "bare_metal"),
        ("quantum", "docker"),
    ] {
        let name = format!("runner-{submitted}");
        let (status, body) =
            register(f.app.clone(), json!({"name": name, "type": submitted})).await;
        assert_eq!(status, StatusCode::CREATED, "{submitted}: {body}");
        assert_eq!(body["type"], expected, "{submitted}");
    }

    // Omitted type and capacity fall back to a docker runner of one.
    let (status, body) = register(f.app.clone(), json!({"name": "runner-default"})).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["type"], "docker");
    assert_eq!(body["capacity"], 1);
}

#[tokio::test]
async fn get_runner_returns_wire_contract_and_404s() {
    let f = seed().await;
    let (status, created) = register(
        f.app.clone(),
        json!({"name": "wire-runner", "type": "docker", "capacity": 2}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id = created["id"].as_str().unwrap().to_string();

    let (status, body) = request_json(
        f.app.clone(),
        "GET",
        &format!("/api/runners/{id}"),
        Some(&f.developer_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["id"], id);
    assert_eq!(body["name"], "wire-runner");
    assert_eq!(body["type"], "docker");
    assert_eq!(body["capacity"], 2);
    // Registration stamps an initial heartbeat, so the field is present
    // (rfc3339) from the first response — it is never reported for a
    // registry row that has never reported activity.
    assert!(body["last_heartbeat"].is_string(), "{body}");

    let (status, body) = request_json(
        f.app.clone(),
        "GET",
        &format!("/api/runners/{}", uuid::Uuid::new_v4()),
        Some(&f.developer_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["error"], "not_found");

    let (status, body) = request_json(
        f.app.clone(),
        "GET",
        "/api/runners/not-a-uuid",
        Some(&f.developer_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"], "invalid_id");
}

#[tokio::test]
async fn retire_requires_admin_or_maintainer() {
    let f = seed().await;
    let (status, created) = register(f.app.clone(), json!({"name": "guarded-runner"})).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id = created["id"].as_str().unwrap().to_string();

    let (status, body) = request_json(
        f.app.clone(),
        "DELETE",
        &format!("/api/runners/{id}"),
        Some(&f.developer_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"], "forbidden");

    // The runner must be untouched by the denied retirement.
    let fleet = RunnerQueries::list(&f.pool).await.unwrap();
    assert_eq!(fleet[0].status, "online");
}

#[tokio::test]
async fn retire_is_idempotent_for_idle_runners() {
    let f = seed().await;
    let (status, created) = register(f.app.clone(), json!({"name": "idle-runner"})).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id = created["id"].as_str().unwrap().to_string();

    let (status, body) = request_json(
        f.app.clone(),
        "DELETE",
        &format!("/api/runners/{id}"),
        Some(&f.admin_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    // Retiring an already-retired runner stays 204: the record survives for
    // audit, and the second call must not error or duplicate work.
    let (status, body) = request_json(
        f.app.clone(),
        "DELETE",
        &format!("/api/runners/{id}"),
        Some(&f.admin_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    let fleet = RunnerQueries::list(&f.pool).await.unwrap();
    assert_eq!(fleet[0].status, "retired");
}

#[tokio::test]
async fn retire_conflicts_while_jobs_are_active_then_succeeds_when_idle() {
    let f = seed().await;
    let (status, created) = register(f.app.clone(), json!({"name": "busy-runner"})).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let runner_id: RunnerId = uuid::Uuid::parse_str(created["id"].as_str().unwrap())
        .unwrap()
        .into();

    let run = PipelineRun::new(
        f.pipeline_id,
        f.repo_id,
        "webhook".to_string(),
        "0".repeat(40),
    );
    PipelineRunQueries::create(&f.pool, &run).await.unwrap();
    let mut job = Job::new(run.id, "busy-job".to_string());
    job.runner_id = Some(runner_id);
    job.status = JobStatus::Running.as_str().to_string();
    let job_id = job.id;
    JobQueries::create(&f.pool, &job).await.unwrap();

    let (status, body) = request_json(
        f.app.clone(),
        "DELETE",
        &format!("/api/runners/{runner_id}"),
        Some(&f.admin_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"], "runner_busy");
    assert_eq!(body["active_jobs"], 1);

    // Once the job reaches a terminal state the same retirement succeeds.
    JobQueries::complete(&f.pool, job_id, "succeeded", "{\"status\":\"succeeded\"}")
        .await
        .unwrap();
    let (status, body) = request_json(
        f.app.clone(),
        "DELETE",
        &format!("/api/runners/{runner_id}"),
        Some(&f.admin_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
}

#[tokio::test]
async fn retire_rejects_unknown_and_malformed_ids() {
    let f = seed().await;

    let (status, body) = request_json(
        f.app.clone(),
        "DELETE",
        &format!("/api/runners/{}", uuid::Uuid::new_v4()),
        Some(&f.admin_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["error"], "not_found");

    let (status, body) = request_json(
        f.app.clone(),
        "DELETE",
        "/api/runners/not-a-uuid",
        Some(&f.admin_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"], "invalid_id");
}

#[tokio::test]
async fn list_runners_exposes_the_registered_fleet() {
    let f = seed().await;
    for name in ["fleet-a", "fleet-b"] {
        let (status, _) = register(f.app.clone(), json!({"name": name, "capacity": 2})).await;
        assert_eq!(status, StatusCode::CREATED);
    }

    let (status, fleet) = request_json(
        f.app.clone(),
        "GET",
        "/api/runners",
        Some(&f.developer_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{fleet}");
    let fleet = fleet.as_array().unwrap();
    assert_eq!(fleet.len(), 2);
    let mut names: Vec<&str> = fleet.iter().map(|r| r["name"].as_str().unwrap()).collect();
    names.sort_unstable();
    assert_eq!(names, ["fleet-a", "fleet-b"]);
    for runner in fleet {
        assert!(runner["id"].is_string());
        assert!(runner["status"].is_string());
        assert_eq!(runner["capacity"], 2);
    }
}

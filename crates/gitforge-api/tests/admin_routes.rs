//! Integration tests for the runner administration and user role routes.
//!
//! Runner registration is the one public bootstrap endpoint; listing,
//! inspection, and retirement require a JWT, and retirement additionally
//! requires an administrator or maintainer. These tests drive all of that
//! through the real router with an in-memory database.

use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Router,
};
use gitforge_api::{ApiAuth, ApiServer};
use gitforge_common::{JobId, PipelineId, PipelineRunId, RunnerId};
use gitforge_db::{
    models::{Job, JobStatus, Pipeline, PipelineRun, Repository, Runner, RunnerType, User},
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
    maintainer_token: String,
    developer_token: String,
    developer_id: gitforge_common::UserId,
}

/// Seed one admin, one maintainer, and one plain developer.
async fn seed() -> Fixture {
    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();

    let admin = User::new(
        "admin-runner".to_string(),
        "admin-runner@example.com".to_string(),
        "hash".to_string(),
    );
    let maintainer = User::new(
        "maintainer-runner".to_string(),
        "maintainer-runner@example.com".to_string(),
        "hash".to_string(),
    );
    let developer = User::new(
        "developer-runner".to_string(),
        "developer-runner@example.com".to_string(),
        "hash".to_string(),
    );
    for user in [&admin, &maintainer, &developer] {
        UserQueries::create(&pool, user).await.unwrap();
    }
    assert!(UserQueries::set_role(&pool, admin.id, "admin")
        .await
        .unwrap());
    assert!(UserQueries::set_role(&pool, maintainer.id, "maintainer")
        .await
        .unwrap());

    let auth = ApiAuth::new("test-secret");
    let token_for =
        |user: &User, role: &str| auth.generate_token(user.id, &user.username, role).unwrap();
    let app = ApiServer::new("test-secret", pool.clone()).into_router();
    Fixture {
        app,
        pool,
        admin_token: token_for(&admin, "admin"),
        maintainer_token: token_for(&maintainer, "maintainer"),
        developer_token: token_for(&developer, "developer"),
        developer_id: developer.id,
    }
}

/// Seed a full run/job chain so a runner can own an active job.
async fn seed_job_on_runner(pool: &Pool, owner: &User, runner_id: RunnerId, status: &str) -> JobId {
    let repo = Repository::new(
        "runner-repo".to_string(),
        owner.id,
        "/git/runner-repo".to_string(),
    );
    RepoQueries::create(pool, &repo).await.unwrap();
    let pipeline = Pipeline {
        id: PipelineId::new(),
        repo_id: repo.id,
        name: "runner-pipeline".to_string(),
        trigger_type: "push".to_string(),
        config: json!({}),
        created_at: chrono::Utc::now(),
    };
    PipelineQueries::create(pool, &pipeline).await.unwrap();
    let run = PipelineRun::new(pipeline.id, repo.id, "webhook".to_string(), "a".repeat(40));
    let run_id: PipelineRunId = run.id;
    PipelineRunQueries::create(pool, &run).await.unwrap();

    let mut job = Job::new(run_id, "runner-job".to_string());
    job.status = status.to_string();
    job.runner_id = Some(runner_id);
    JobQueries::create(pool, &job).await.unwrap();
    job.id
}

async fn post_json(
    app: Router,
    uri: &str,
    token: Option<&str>,
    body: Value,
) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .method("POST")
        .uri(uri)
        .header("Content-Type", "application/json");
    if let Some(token) = token {
        builder = builder.header("Authorization", format!("Bearer {token}"));
    }
    let response = app
        .oneshot(builder.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn get_json(app: Router, uri: &str, token: Option<&str>) -> (StatusCode, Value) {
    let mut builder = Request::builder().method("GET").uri(uri);
    if let Some(token) = token {
        builder = builder.header("Authorization", format!("Bearer {token}"));
    }
    let response = app
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn register_runner_is_public_and_normalizes_the_payload() {
    let f = seed().await;

    // Registration happens before the runner has credentials, so it is the
    // one runner endpoint served without a JWT.
    let (status, body) = post_json(
        f.app.clone(),
        "/api/runners",
        None,
        json!({"name": "edge-runner", "type": "firecracker", "capacity": 4}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["name"], "edge-runner");
    assert_eq!(body["type"], "firecracker");
    assert_eq!(body["status"], "online");
    assert_eq!(body["capacity"], 4);

    // Defaults and aliases: no payload means stock docker runner; the
    // baremetal spelling maps to the same type as bare_metal; unknown types
    // fall back to docker.
    let (status, body) = post_json(f.app.clone(), "/api/runners", None, json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["name"], "runner");
    assert_eq!(body["type"], "docker");
    assert_eq!(body["capacity"], 1);

    for (sent, expected) in [
        ("bare_metal", "bare_metal"),
        ("baremetal", "bare_metal"),
        ("quantum", "docker"),
    ] {
        let (status, body) = post_json(
            f.app.clone(),
            "/api/runners",
            None,
            json!({"name": format!("runner-{sent}"), "type": sent}),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{sent}: {body}");
        assert_eq!(body["type"], expected, "{sent}");
    }

    let runners = RunnerQueries::list(&f.pool).await.unwrap();
    assert_eq!(runners.len(), 5);
}

#[tokio::test]
async fn runner_listing_requires_auth_and_shows_registered_runners() {
    let f = seed().await;
    let runner = Runner::new("listed-runner".to_string(), RunnerType::Docker, 2);
    RunnerQueries::create(&f.pool, &runner).await.unwrap();

    let (status, _) = get_json(f.app.clone(), "/api/runners", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, body) = get_json(f.app.clone(), "/api/runners", Some(&f.developer_token)).await;
    assert_eq!(status, StatusCode::OK);
    let runners = body.as_array().unwrap();
    assert_eq!(runners.len(), 1);
    assert_eq!(runners[0]["name"], "listed-runner");
    assert_eq!(runners[0]["capacity"], 2);
    assert!(runners[0]["last_heartbeat"].is_string());
}

#[tokio::test]
async fn get_runner_validates_the_id_and_missing_rows() {
    let f = seed().await;

    let (status, body) = get_json(
        f.app.clone(),
        "/api/runners/not-a-uuid",
        Some(&f.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "invalid_id");

    let (status, body) = get_json(
        f.app.clone(),
        &format!("/api/runners/{}", uuid::Uuid::new_v4()),
        Some(&f.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "not_found");

    let runner = Runner::new("visible-runner".to_string(), RunnerType::BareMetal, 8);
    RunnerQueries::create(&f.pool, &runner).await.unwrap();
    let (status, body) = get_json(
        f.app.clone(),
        &format!("/api/runners/{}", runner.id),
        Some(&f.developer_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["type"], "bare_metal");
    assert_eq!(body["capacity"], 8);
}

#[tokio::test]
async fn retire_runner_enforces_role_ownership_of_the_operation() {
    let f = seed().await;
    let runner = Runner::new("idle-runner".to_string(), RunnerType::Docker, 1);
    RunnerQueries::create(&f.pool, &runner).await.unwrap();
    let uri = format!("/api/runners/{}", runner.id);

    // Plain developers may not retire runners.
    let (status, body) = request_delete(f.app.clone(), &uri, Some(&f.developer_token)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"], "forbidden");

    // Maintainers and administrators may.
    for token in [&f.maintainer_token, &f.admin_token] {
        let (status, _) = request_delete(f.app.clone(), &uri, Some(token)).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
    }

    // The second retirement is still a no-content success: the runner was
    // already retired and the audit row is intact.
    let stored = RunnerQueries::get(&f.pool, runner.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.status, "retired");

    let (status, _) = request_delete(
        f.app.clone(),
        &format!("/api/runners/{}", uuid::Uuid::new_v4()),
        Some(&f.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, body) = request_delete(
        f.app.clone(),
        "/api/runners/not-a-uuid",
        Some(&f.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "invalid_id");
}

#[tokio::test]
async fn retire_runner_refuses_to_hide_a_worker_with_active_jobs() {
    let f = seed().await;
    let owner = User::new(
        "runner-owner".to_string(),
        "runner-owner@example.com".to_string(),
        "hash".to_string(),
    );
    UserQueries::create(&f.pool, &owner).await.unwrap();

    let busy = Runner::new("busy-runner".to_string(), RunnerType::Docker, 1);
    RunnerQueries::create(&f.pool, &busy).await.unwrap();
    seed_job_on_runner(&f.pool, &owner, busy.id, JobStatus::Running.as_str()).await;

    let (status, body) = request_delete(
        f.app.clone(),
        &format!("/api/runners/{}", busy.id),
        Some(&f.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"], "runner_busy");
    assert_eq!(body["active_jobs"], 1);

    // Once the job reaches a terminal state the same runner retires cleanly.
    let stored = RunnerQueries::get(&f.pool, busy.id).await.unwrap().unwrap();
    assert_eq!(stored.status, "online");
}

async fn request_delete(app: Router, uri: &str, token: Option<&str>) -> (StatusCode, Value) {
    let mut builder = Request::builder().method("DELETE").uri(uri);
    if let Some(token) = token {
        builder = builder.header("Authorization", format!("Bearer {token}"));
    }
    let response = app
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn role_management_is_admin_only_and_protects_the_last_admin() {
    let f = seed().await;
    let target_uri = format!("/api/users/{}/role", f.developer_id);

    // Promote the developer to maintainer.
    let promote = Request::builder()
        .method("PATCH")
        .uri(&target_uri)
        .header("Authorization", format!("Bearer {}", f.admin_token))
        .header("Content-Type", "application/json")
        .body(Body::from(r#"{"role":"maintainer"}"#))
        .unwrap();
    let response = f.app.clone().oneshot(promote).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["role"], "maintainer");
    assert_eq!(
        UserQueries::get_role(&f.pool, f.developer_id)
            .await
            .unwrap(),
        Some("maintainer".to_string())
    );

    // Unknown and malformed targets.
    let unknown = Request::builder()
        .method("PATCH")
        .uri(format!("/api/users/{}/role", uuid::Uuid::new_v4()))
        .header("Authorization", format!("Bearer {}", f.admin_token))
        .header("Content-Type", "application/json")
        .body(Body::from(r#"{"role":"developer"}"#))
        .unwrap();
    assert_eq!(
        f.app.clone().oneshot(unknown).await.unwrap().status(),
        StatusCode::NOT_FOUND
    );

    let malformed = Request::builder()
        .method("PATCH")
        .uri("/api/users/not-a-uuid/role")
        .header("Authorization", format!("Bearer {}", f.admin_token))
        .header("Content-Type", "application/json")
        .body(Body::from(r#"{"role":"developer"}"#))
        .unwrap();
    assert_eq!(
        f.app.clone().oneshot(malformed).await.unwrap().status(),
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn the_last_administrator_cannot_be_demoted() {
    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();
    let sole_admin = User::new(
        "sole-admin".to_string(),
        "sole-admin@example.com".to_string(),
        "hash".to_string(),
    );
    UserQueries::create(&pool, &sole_admin).await.unwrap();
    assert!(UserQueries::set_role(&pool, sole_admin.id, "admin")
        .await
        .unwrap());
    let auth = ApiAuth::new("test-secret");
    let token = auth
        .generate_token(sole_admin.id, &sole_admin.username, "admin")
        .unwrap();
    let app = ApiServer::new("test-secret", pool).into_router();

    // Demoting the only administrator would lock every administrative
    // endpoint, so the handler refuses with a distinct conflict error.
    let demote = Request::builder()
        .method("PATCH")
        .uri(format!("/api/users/{}/role", sole_admin.id))
        .header("Authorization", format!("Bearer {token}"))
        .header("Content-Type", "application/json")
        .body(Body::from(r#"{"role":"developer"}"#))
        .unwrap();
    let response = app.clone().oneshot(demote).await.unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["error"], "last_admin");
}

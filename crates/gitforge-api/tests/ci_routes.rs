//! Integration tests for the CI route handlers.
//!
//! These drive the real router with an in-memory database and cover the
//! authorization matrix (owner / admin / unrelated developer) plus the
//! durable job-submission contract: payload validation, idempotent replay,
//! and the cancel lifecycle.

use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Router,
};
use gitforge_api::{ApiAuth, ApiServer, CiTriggerClient};
use gitforge_ci::{JobDefinition, PipelineDefinition, StepDefinition, TriggerType};
use gitforge_common::{PipelineId, PipelineRunId, RepoId, RunnerId};
use gitforge_db::{
    models::{Job, JobStatus, Pipeline, PipelineRun, Repository, Runner, RunnerType, User},
    queries::{
        JobQueries, PipelineQueries, PipelineRunQueries, RepoQueries, RunnerQueries, UserQueries,
    },
    Pool,
};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

/// Seed users, a repository with one pipeline and one pending run, and the
/// matching auth tokens for each role.
struct Fixture {
    app: Router,
    pool: Pool,
    owner_token: String,
    admin_token: String,
    intruder_token: String,
    repo_id: RepoId,
    pipeline_id: PipelineId,
    run_id: PipelineRunId,
}

async fn seed() -> Fixture {
    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();

    let owner = User::new(
        "ci-owner".to_string(),
        "ci-owner@example.com".to_string(),
        "hash".to_string(),
    );
    let admin = User::new(
        "ci-admin".to_string(),
        "ci-admin@example.com".to_string(),
        "hash".to_string(),
    );
    let intruder = User::new(
        "ci-intruder".to_string(),
        "ci-intruder@example.com".to_string(),
        "hash".to_string(),
    );
    for user in [&owner, &admin, &intruder] {
        UserQueries::create(&pool, user).await.unwrap();
    }
    // The middleware resolves the persisted role over the JWT claim, so the
    // admin's database row must carry the admin role.
    assert!(UserQueries::set_role(&pool, admin.id, "admin")
        .await
        .unwrap());

    let repo = Repository::new(
        "ci-routes-repo".to_string(),
        owner.id,
        "/git/ci-routes-repo".to_string(),
    );
    let repo_id = repo.id;
    RepoQueries::create(&pool, &repo).await.unwrap();

    let pipeline = Pipeline {
        id: PipelineId::new(),
        repo_id,
        name: "ci-routes-pipeline".to_string(),
        trigger_type: "push".to_string(),
        config: json!({"name": "ci-routes-pipeline", "version": "1.0", "jobs": []}),
        created_at: chrono::Utc::now(),
    };
    let pipeline_id = pipeline.id;
    PipelineQueries::create(&pool, &pipeline).await.unwrap();

    let run = PipelineRun::new(pipeline_id, repo_id, "webhook".to_string(), "0".repeat(40));
    let run_id = run.id;
    PipelineRunQueries::create(&pool, &run).await.unwrap();

    let auth = ApiAuth::new("test-secret");
    let token_for =
        |user: &User, role: &str| auth.generate_token(user.id, &user.username, role).unwrap();
    let app = ApiServer::new("test-secret", pool.clone()).into_router();
    Fixture {
        app,
        pool,
        owner_token: token_for(&owner, "developer"),
        admin_token: token_for(&admin, "admin"),
        intruder_token: token_for(&intruder, "developer"),
        repo_id,
        pipeline_id,
        run_id,
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

fn submit_body(run_id: &PipelineRunId, commands: Vec<&str>) -> Value {
    json!({
        "pipeline_run_id": run_id.to_string(),
        "name": "user-submitted-job",
        "commands": commands,
        "working_dir": null,
        "idempotency_key": "ci-routes-key-1",
        "timeout": "45m"
    })
}

#[tokio::test]
async fn pipeline_list_is_scoped_to_authorized_repositories() {
    let f = seed().await;
    let uri = "/api/pipelines";

    let (status, owner_view) =
        request_json(f.app.clone(), "GET", uri, Some(&f.owner_token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(owner_view.as_array().unwrap().len(), 1);
    assert_eq!(owner_view[0]["name"], "ci-routes-pipeline");
    assert_eq!(owner_view[0]["repo_id"], f.repo_id.to_string());

    // An unrelated developer must not even see the pipeline's existence.
    let (status, intruder_view) =
        request_json(f.app.clone(), "GET", uri, Some(&f.intruder_token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(intruder_view.as_array().unwrap().is_empty());

    // Administrators bypass repository ownership.
    let (status, admin_view) =
        request_json(f.app.clone(), "GET", uri, Some(&f.admin_token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(admin_view.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn get_pipeline_enforces_ownership_with_admin_override() {
    let f = seed().await;
    let uri = format!("/api/pipelines/{}", f.pipeline_id);

    let (status, body) = request_json(f.app.clone(), "GET", &uri, Some(&f.owner_token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["name"], "ci-routes-pipeline");
    assert_eq!(body["trigger_type"], "push");

    let (status, body) =
        request_json(f.app.clone(), "GET", &uri, Some(&f.intruder_token), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"], "forbidden");

    let (status, _) = request_json(f.app.clone(), "GET", &uri, Some(&f.admin_token), None).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn pipeline_run_endpoints_scoped_and_expose_jobs_with_receipts() {
    let f = seed().await;

    let mut job = Job::new(f.run_id, "seeded-job".to_string());
    job.commands = vec!["echo seeded".to_string()];
    job.status = JobStatus::Queued.as_str().to_string();
    let job_id = job.id;
    JobQueries::create(&f.pool, &job).await.unwrap();
    let receipt = json!({"status": "succeeded", "exit_code": 0}).to_string();
    JobQueries::complete(&f.pool, job_id, "succeeded", &receipt)
        .await
        .unwrap();

    let (status, runs) = request_json(
        f.app.clone(),
        "GET",
        "/api/pipeline-runs",
        Some(&f.owner_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(runs.as_array().unwrap().len(), 1);
    assert_eq!(runs[0]["id"], f.run_id.to_string());

    let (status, intruder_runs) = request_json(
        f.app.clone(),
        "GET",
        "/api/pipeline-runs",
        Some(&f.intruder_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(intruder_runs.as_array().unwrap().is_empty());

    let run_uri = format!("/api/pipeline-runs/{}", f.run_id);
    let (status, body) =
        request_json(f.app.clone(), "GET", &run_uri, Some(&f.owner_token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["pipeline_id"], f.pipeline_id.to_string());
    assert_eq!(body["triggered_by"], "webhook");

    let (status, _) = request_json(
        f.app.clone(),
        "GET",
        &run_uri,
        Some(&f.intruder_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // The run's jobs endpoint surfaces the persisted completion receipt.
    let jobs_uri = format!("/api/pipeline-runs/{}/jobs", f.run_id);
    let (status, jobs) =
        request_json(f.app.clone(), "GET", &jobs_uri, Some(&f.owner_token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(jobs.as_array().unwrap().len(), 1);
    assert_eq!(jobs[0]["id"], job_id.to_string());
    assert_eq!(jobs[0]["status"], "succeeded");
    assert_eq!(jobs[0]["receipt"]["status"], "succeeded");
    assert_eq!(jobs[0]["receipt"]["exit_code"], 0);

    let (status, _) = request_json(
        f.app.clone(),
        "GET",
        &jobs_uri,
        Some(&f.intruder_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, missing) = request_json(
        f.app.clone(),
        "GET",
        &format!("/api/pipeline-runs/{}/jobs", uuid::Uuid::new_v4()),
        Some(&f.owner_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(missing["error"], "not_found");
}

#[tokio::test]
async fn submit_job_rejects_invalid_payloads() {
    let f = seed().await;
    let uri = "/api/jobs";
    let token = f.owner_token.as_str();

    let bad_timeout = json!({
        "pipeline_run_id": f.run_id.to_string(),
        "name": "job",
        "commands": ["echo hi"],
        "working_dir": null,
        "idempotency_key": "key-timeout",
        "timeout": "10x"
    });
    let (status, body) =
        request_json(f.app.clone(), "POST", uri, Some(token), Some(bad_timeout)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "invalid_timeout");

    // Each case violates exactly one submission bound. The idempotency keys
    // are unique per case so a payload that slips through validation would
    // be visible as a created job rather than an idempotency replay.
    let mut cases: Vec<(&str, Value)> = vec![
        (
            "empty-name",
            json!({"pipeline_run_id": f.run_id.to_string(), "name": "", "commands": ["echo"], "working_dir": null, "idempotency_key": "k-empty-name"}),
        ),
        (
            "long-name",
            json!({"pipeline_run_id": f.run_id.to_string(), "name": "j".repeat(129), "commands": ["echo"], "working_dir": null, "idempotency_key": "k-long-name"}),
        ),
        (
            "empty-idempotency-key",
            json!({"pipeline_run_id": f.run_id.to_string(), "name": "job", "commands": ["echo"], "working_dir": null, "idempotency_key": ""}),
        ),
        (
            "no-commands",
            json!({"pipeline_run_id": f.run_id.to_string(), "name": "job", "commands": Vec::<String>::new(), "working_dir": null, "idempotency_key": "k-no-commands"}),
        ),
        (
            "too-many-commands",
            json!({"pipeline_run_id": f.run_id.to_string(), "name": "job", "commands": vec!["echo"; 65], "working_dir": null, "idempotency_key": "k-too-many-commands"}),
        ),
        (
            "long-command",
            json!({"pipeline_run_id": f.run_id.to_string(), "name": "job", "commands": ["e".repeat(17 * 1024)], "working_dir": null, "idempotency_key": "k-long-command"}),
        ),
        (
            "empty-working-dir",
            json!({"pipeline_run_id": f.run_id.to_string(), "name": "job", "commands": ["echo"], "working_dir": "", "idempotency_key": "k-empty-dir"}),
        ),
    ];
    for (label, payload) in cases.drain(..) {
        let (status, body) =
            request_json(f.app.clone(), "POST", uri, Some(token), Some(payload)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "case {label}");
        assert_eq!(body["error"], "invalid_job_submission", "case {label}");
    }

    let bad_run = json!({
        "pipeline_run_id": "not-a-uuid",
        "name": "job",
        "commands": ["echo"],
        "working_dir": null,
        "idempotency_key": "k-bad-run"
    });
    let (status, body) = request_json(f.app.clone(), "POST", uri, Some(token), Some(bad_run)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "invalid_pipeline_run_id");

    let unknown_run = json!({
        "pipeline_run_id": uuid::Uuid::new_v4().to_string(),
        "name": "job",
        "commands": ["echo"],
        "working_dir": null,
        "idempotency_key": "k-unknown-run"
    });
    let (status, body) =
        request_json(f.app.clone(), "POST", uri, Some(token), Some(unknown_run)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "pipeline_run_not_found");
}

#[tokio::test]
async fn submit_job_enforces_ownership_and_terminal_runs() {
    let f = seed().await;

    // A developer without repository access may not enqueue work on it.
    let (status, body) = request_json(
        f.app.clone(),
        "POST",
        "/api/jobs",
        Some(&f.intruder_token),
        Some(submit_body(&f.run_id, vec!["echo nope"])),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"], "forbidden");

    // Administrators bypass ownership and may submit on any repository.
    let (status, body) = request_json(
        f.app.clone(),
        "POST",
        "/api/jobs",
        Some(&f.admin_token),
        Some(submit_body(&f.run_id, vec!["echo admin"])),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["status"], "queued");

    // Terminal runs refuse new submissions.
    let terminal_run = PipelineRun::new(
        f.pipeline_id,
        f.repo_id,
        "webhook".to_string(),
        "f".repeat(40),
    );
    PipelineRunQueries::create(&f.pool, &terminal_run)
        .await
        .unwrap();
    PipelineRunQueries::update_status(&f.pool, terminal_run.id, "failed")
        .await
        .unwrap();
    let mut payload = submit_body(&terminal_run.id, vec!["echo late"]);
    payload["idempotency_key"] = json!("k-terminal");
    let (status, body) = request_json(
        f.app.clone(),
        "POST",
        "/api/jobs",
        Some(&f.owner_token),
        Some(payload),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"], "pipeline_run_already_terminal");
}

#[tokio::test]
async fn submit_job_persists_the_durable_definition_and_replays_idempotently() {
    let f = seed().await;

    let (status, first) = request_json(
        f.app.clone(),
        "POST",
        "/api/jobs",
        Some(&f.owner_token),
        Some(submit_body(&f.run_id, vec!["echo one", "echo two"])),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{first}");
    assert_eq!(first["status"], "queued");
    let job_id = gitforge_common::JobId::from(
        uuid::Uuid::parse_str(first["job_id"].as_str().unwrap()).unwrap(),
    );

    // The queued row carries the submitted commands and bounded timeout so
    // the scheduler can hand it to a runner without extra lookups.
    let stored = JobQueries::get(&f.pool, job_id).await.unwrap().unwrap();
    assert_eq!(stored.status, JobStatus::Queued.as_str());
    assert_eq!(stored.commands, vec!["echo one", "echo two"]);
    assert_eq!(stored.timeout_secs, 45 * 60);

    // Retrying the identical submission returns the same job, not new work.
    let (status, replay) = request_json(
        f.app.clone(),
        "POST",
        "/api/jobs",
        Some(&f.owner_token),
        Some(submit_body(&f.run_id, vec!["echo one", "echo two"])),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{replay}");
    assert_eq!(replay["status"], "already_queued");
    assert_eq!(replay["job_id"], first["job_id"]);

    // The same idempotency key with a different request is a client bug.
    let mut changed = submit_body(&f.run_id, vec!["echo different"]);
    changed["idempotency_key"] = json!("ci-routes-key-1");
    let (status, body) = request_json(
        f.app.clone(),
        "POST",
        "/api/jobs",
        Some(&f.owner_token),
        Some(changed),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(
        body["error"],
        "idempotency_key_reused_with_different_request"
    );

    // Distinct keys create distinct work.
    let mut second = submit_body(&f.run_id, vec!["echo second"]);
    second["idempotency_key"] = json!("ci-routes-key-2");
    let (status, body) = request_json(
        f.app.clone(),
        "POST",
        "/api/jobs",
        Some(&f.owner_token),
        Some(second),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_ne!(body["job_id"], first["job_id"]);
}

#[tokio::test]
async fn get_job_surfaces_receipt_and_enforces_access() {
    let f = seed().await;
    let mut job = Job::new(f.run_id, "seeded-job".to_string());
    job.status = JobStatus::Queued.as_str().to_string();
    let job_id = job.id;
    JobQueries::create(&f.pool, &job).await.unwrap();

    let uri = format!("/api/jobs/{job_id}");
    let (status, body) =
        request_json(f.app.clone(), "GET", &uri, Some(&f.intruder_token), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"], "forbidden");

    let (status, body) = request_json(f.app.clone(), "GET", &uri, Some(&f.owner_token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["name"], "seeded-job");
    assert_eq!(body["status"], "queued");
    assert!(body["receipt"].is_null());

    let (status, _) = request_json(f.app.clone(), "GET", &uri, Some(&f.admin_token), None).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn job_logs_are_lease_scoped_and_readable_through_the_api() {
    let f = seed().await;

    // A runner with an active lease appends chunks; the API reads them back.
    let runner = Runner::new("logs-runner".to_string(), RunnerType::Docker, 1);
    RunnerQueries::create(&f.pool, &runner).await.unwrap();
    let mut job = Job::new(f.run_id, "logging-job".to_string());
    job.status = JobStatus::Queued.as_str().to_string();
    let job_id = job.id;
    JobQueries::create(&f.pool, &job).await.unwrap();
    assert!(JobQueries::assign_with_lease(
        &f.pool,
        job_id,
        runner.id,
        "lease-token-1",
    )
    .await
    .unwrap());
    JobQueries::append_log_with_lease(
        &f.pool,
        job_id,
        runner.id,
        "lease-token-1",
        "[stdout] building\n",
    )
    .await
    .unwrap()
    .unwrap();

    // Unrelated developers may not read the log stream.
    let uri = format!("/api/jobs/{job_id}/logs");
    let (status, _) = request_json(f.app.clone(), "GET", &uri, Some(&f.intruder_token), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, body) = request_json(f.app.clone(), "GET", &uri, Some(&f.owner_token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["job_id"], job_id.to_string());
    let logs = body["logs"].as_array().unwrap();
    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0]["chunk"], "[stdout] building\n");
    assert!(body["receipt"].is_null());
}

#[tokio::test]
async fn cancel_job_lifecycle_is_owned_idempotent_and_final() {
    let f = seed().await;
    let mut job = Job::new(f.run_id, "cancellable-job".to_string());
    job.status = JobStatus::Queued.as_str().to_string();
    let job_id = job.id;
    JobQueries::create(&f.pool, &job).await.unwrap();

    let uri = format!("/api/jobs/{job_id}/cancel");

    // Repository outsiders may not cancel someone else's job.
    let (status, body) =
        request_json(f.app.clone(), "POST", &uri, Some(&f.intruder_token), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"], "forbidden");

    // The owner cancels; the durable row records the cancellation receipt.
    let (status, body) =
        request_json(f.app.clone(), "POST", &uri, Some(&f.owner_token), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "cancelled");
    assert_eq!(body["contract_version"], "harness.job.v1");

    let stored = JobQueries::get(&f.pool, job_id).await.unwrap().unwrap();
    assert_eq!(stored.status, JobStatus::Cancelled.as_str());
    let receipt: Value = serde_json::from_str(stored.result_json.as_deref().unwrap()).unwrap();
    assert_eq!(receipt["status"], "cancelled");
    assert_eq!(receipt["reason"], "api operator requested cancellation");

    // Repeated cancellation of the same job stays successful (idempotent).
    let (status, body) =
        request_json(f.app.clone(), "POST", &uri, Some(&f.owner_token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "cancelled");

    // A job that finished on its own can no longer be cancelled.
    let mut finished = Job::new(f.run_id, "finished-job".to_string());
    finished.status = JobStatus::Succeeded.as_str().to_string();
    let finished_id = finished.id;
    JobQueries::create(&f.pool, &finished).await.unwrap();
    let (status, body) = request_json(
        f.app.clone(),
        "POST",
        &format!("/api/jobs/{finished_id}/cancel"),
        Some(&f.owner_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"], "job_already_terminal");
    assert_eq!(body["status"], "succeeded");

    // The receipt is visible through the job endpoint after cancellation.
    let (status, body) = request_json(
        f.app.clone(),
        "GET",
        &format!("/api/jobs/{job_id}"),
        Some(&f.owner_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["receipt"]["status"], "cancelled");
    assert_eq!(body["status"], "cancelled");
}

#[tokio::test]
async fn job_and_run_routes_reject_malformed_ids() {
    let f = seed().await;

    for uri in [
        "/api/jobs/not-a-uuid".to_string(),
        "/api/jobs/not-a-uuid/logs".to_string(),
        "/api/pipeline-runs/not-a-uuid".to_string(),
        "/api/pipeline-runs/not-a-uuid/jobs".to_string(),
    ] {
        let (status, _) =
            request_json(f.app.clone(), "GET", &uri, Some(&f.owner_token), None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri}");
    }

    // Cancellation is a POST-only control-plane action.
    let (status, _) = request_json(
        f.app.clone(),
        "POST",
        "/api/jobs/not-a-uuid/cancel",
        Some(&f.owner_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

/// A pipeline definition with one runnable entry job, serialized exactly the
/// way `.gitforge-ci.yaml` ingestion stores it.
fn entry_job_definition() -> Value {
    serde_json::to_value(PipelineDefinition {
        name: "webhook-replay-pipeline".to_string(),
        version: "1.0".to_string(),
        trigger_on: vec![TriggerType::Push],
        environment: std::collections::HashMap::new(),
        jobs: vec![JobDefinition {
            name: "webhook-job".to_string(),
            image: "alpine:latest".to_string(),
            needs: vec![],
            env: std::collections::HashMap::new(),
            steps: vec![StepDefinition {
                name: "smoke".to_string(),
                run: "printf webhook-replay".to_string(),
                env: None,
                working_directory: None,
                condition: None,
            }],
            timeout: Some("30s".to_string()),
            retry: None,
        }],
    })
    .unwrap()
}

#[tokio::test]
async fn webhook_replays_are_idempotent_and_map_to_one_durable_job() {
    let f = seed().await;
    let pipeline = Pipeline {
        id: PipelineId::new(),
        repo_id: f.repo_id,
        name: "webhook-replay-pipeline".to_string(),
        trigger_type: "push".to_string(),
        config: entry_job_definition(),
        created_at: chrono::Utc::now(),
    };
    PipelineQueries::create(&f.pool, &pipeline).await.unwrap();

    let payload = json!({
        "repo_id": f.repo_id.to_string(),
        "commit_hash": "a".repeat(40),
        "branch": "main"
    });

    // First delivery queues exactly one executable job.
    let (status, body) = request_json(
        f.app.clone(),
        "POST",
        &format!("/api/webhook/trigger/{}", pipeline.id),
        Some(&f.owner_token),
        Some(payload.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["success"], true);
    assert_eq!(body["pipeline_id"], pipeline.id.to_string());

    let scope = format!("webhook:{}", pipeline.id);
    let key = format!("{scope}:{}", payload["commit_hash"].as_str().unwrap());
    let (job_id, fingerprint) = JobQueries::get_idempotency(&f.pool, &scope, &key)
        .await
        .unwrap()
        .expect("first delivery must reserve the idempotency key");
    let fingerprint: Value = serde_json::from_str(&fingerprint).unwrap();
    assert_eq!(fingerprint["name"], "webhook-job");
    assert_eq!(fingerprint["image"], "alpine:latest");
    assert_eq!(fingerprint["timeout_secs"], 30);
    assert_eq!(fingerprint["commands"], json!(["printf webhook-replay"]));

    let runs_after_first = PipelineRunQueries::list(&f.pool).await.unwrap();
    assert_eq!(runs_after_first.len(), 2);
    let queued_run = runs_after_first
        .iter()
        .find(|run| run.id != f.run_id)
        .unwrap();
    assert_eq!(queued_run.status, "pending");
    let jobs = JobQueries::list_by_run(&f.pool, queued_run.id)
        .await
        .unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].id, job_id);
    assert_eq!(jobs[0].status, JobStatus::Queued.as_str());

    // A redelivered webhook for the same commit is accepted but collapses to
    // the existing job: a new run is recorded and immediately cancelled, and
    // no second job ever reaches the queue.
    let (status, body) = request_json(
        f.app.clone(),
        "POST",
        &format!("/api/webhook/trigger/{}", pipeline.id),
        Some(&f.owner_token),
        Some(payload),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["success"], true);

    let (existing_job_id, _) = JobQueries::get_idempotency(&f.pool, &scope, &key)
        .await
        .unwrap()
        .expect("replay must keep the original key");
    assert_eq!(existing_job_id, job_id);

    let runs_after_replay = PipelineRunQueries::list(&f.pool).await.unwrap();
    assert_eq!(
        runs_after_replay.len(),
        3,
        "replay still records its own run row"
    );
    let replay_run = runs_after_replay
        .iter()
        .find(|run| run.id != f.run_id && run.id != queued_run.id)
        .unwrap();
    assert_eq!(
        replay_run.status, "cancelled",
        "the duplicate delivery's run is cancelled"
    );
    assert!(
        JobQueries::list_by_run(&f.pool, replay_run.id)
            .await
            .unwrap()
            .is_empty(),
        "a replay never enqueues a second job"
    );
    // The original delivery keeps its queued job.
    assert_eq!(
        JobQueries::list_by_run(&f.pool, queued_run.id)
            .await
            .unwrap()
            .len(),
        1,
        "one idempotency key maps to exactly one job"
    );
}

/// The API gateway delegates webhook execution to the loopback CI
/// orchestrator whenever the trigger client is configured. This deployment
/// has no CI endpoint answering under the test's identity, so the gateway
/// must fail closed with a 502 instead of silently dropping the delivery.
#[tokio::test]
async fn webhook_delegation_fails_closed_when_ci_cannot_accept_the_trigger() {
    let f = seed().await;
    let client = CiTriggerClient::new(
        "http://127.0.0.1:42781/pipelines/trigger",
        "not-a-real-token",
    )
    .unwrap();
    let app = ApiServer::new("test-secret", f.pool.clone())
        .with_ci_trigger_client(Arc::new(client))
        .into_router();

    let (status, body) = request_json(
        app,
        "POST",
        &format!("/api/webhook/trigger/{}", f.pipeline_id),
        Some(&f.owner_token),
        Some(json!({
            "repo_id": f.repo_id.to_string(),
            "commit_hash": "b".repeat(40),
            "branch": "main"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
    assert_eq!(body["success"], false);
    assert_eq!(body["message"], "CI trigger unavailable");
    assert!(body["pipeline_id"].is_null());

    // The failed delegation must not leave a phantom job behind.
    let runs = PipelineRunQueries::list(&f.pool).await.unwrap();
    assert_eq!(
        runs.len(),
        1,
        "delegation happens before any run is persisted"
    );
}

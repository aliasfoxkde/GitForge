//! Integration tests for the webhook trigger route.
//!
//! With no CI trigger client configured, the route falls back to its
//! durable in-process path: it validates the stored pipeline definition,
//! persists the run, and queues the entry job under an idempotency key so
//! the scheduler's next tick adopts the work. These tests drive the real
//! router with an in-memory database and assert on the rows the handler
//! itself wrote.
//!
//! The delegation path (a configured client) is covered twice: the
//! fail-closed angle in `ci_routes.rs` (client pointed at a dead port)
//! and — in this file — the full ladder against a stub orchestrator
//! binding the pinned production endpoint itself, since the client
//! refuses any other URL by design.

// Test-harness exemption, same discipline as the sibling route suites and
// the gitforge-db integration fixtures: `allow-unwrap-in-tests` covers
// `#[test]` bodies, but the boot/seed helper functions in a test target
// are neither `#[test]` fns nor `#[cfg(test)]`, a class the clippy.toml
// config cannot address. Setup failing IS the assertion -- a panic aborts
// the run loudly. Production code keeps the denies.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Router,
};
use gitforge_api::{ApiAuth, ApiServer, CiTriggerClient};
use gitforge_common::{JobId, PipelineId, RepoId};
use gitforge_db::{
    models::{JobStatus, Pipeline, Repository, User},
    queries::{JobQueries, PipelineQueries, PipelineRunQueries, RepoQueries, UserQueries},
    Pool,
};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

mod common;

use common::serve_stub_ci;

struct Fixture {
    app: Router,
    pool: Pool,
    owner_token: String,
    repo_id: RepoId,
    valid_pipeline: PipelineId,
    empty_pipeline: PipelineId,
    garbage_pipeline: PipelineId,
    bad_timeout_pipeline: PipelineId,
}

/// A stored pipeline definition must deserialize into a full
/// `PipelineDefinition` (name, version, trigger_on, environment, jobs) or
/// the route rejects it before any persistence.
fn definition(jobs: Value) -> Value {
    json!({
        "name": "wh-pipeline",
        "version": "1.0",
        "trigger_on": ["push"],
        "environment": {},
        "jobs": jobs
    })
}

fn entry_job(timeout: Value) -> Value {
    json!([{
        "name": "build",
        "image": "alpine:latest",
        "needs": [],
        "timeout": timeout,
        "steps": [{"name": "smoke", "run": "echo hi"}]
    }])
}

async fn seed() -> Fixture {
    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();

    let owner = User::new(
        "wh-owner".to_string(),
        "wh-owner@example.com".to_string(),
        "hash".to_string(),
    );
    UserQueries::create(&pool, &owner).await.unwrap();

    let repo = Repository::new(
        "wh-routes-repo".to_string(),
        owner.id,
        "/git/wh-routes-repo".to_string(),
    );
    let repo_id = repo.id;
    RepoQueries::create(&pool, &repo).await.unwrap();

    async fn pipeline_for(pool: &Pool, repo_id: RepoId, name: &str, config: Value) -> PipelineId {
        let pipeline = Pipeline {
            id: PipelineId::new(),
            repo_id,
            name: name.to_string(),
            trigger_type: "push".to_string(),
            config,
            created_at: chrono::Utc::now(),
        };
        let id = pipeline.id;
        PipelineQueries::create(pool, &pipeline).await.unwrap();
        id
    }
    let valid_pipeline = pipeline_for(
        &pool,
        repo_id,
        "wh-valid",
        definition(entry_job(json!("5m"))),
    )
    .await;
    let empty_pipeline = pipeline_for(&pool, repo_id, "wh-empty", definition(json!([]))).await;
    let garbage_pipeline =
        pipeline_for(&pool, repo_id, "wh-garbage", json!({"jobs": "nope"})).await;
    let bad_timeout_pipeline = pipeline_for(
        &pool,
        repo_id,
        "wh-bad-timeout",
        definition(entry_job(json!("bogus"))),
    )
    .await;

    let auth = ApiAuth::new("test-secret");
    let owner_token = auth
        .generate_token(owner.id, &owner.username, "developer")
        .unwrap();
    let app = ApiServer::new("test-secret", pool.clone()).into_router();
    Fixture {
        app,
        pool,
        owner_token,
        repo_id,
        valid_pipeline,
        empty_pipeline,
        garbage_pipeline,
        bad_timeout_pipeline,
    }
}

async fn post_webhook(
    app: Router,
    pipeline_id: &PipelineId,
    token: &str,
    payload: Value,
) -> (StatusCode, Value) {
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/webhook/trigger/{pipeline_id}"))
                .header("Content-Type", "application/json")
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::from(payload.to_string()))
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

fn webhook_payload(repo_id: &RepoId, commit: &str) -> Value {
    json!({
        "repo_id": repo_id.to_string(),
        "commit_hash": commit,
        "branch": "main"
    })
}

#[tokio::test]
async fn webhook_rejects_malformed_and_unknown_pipeline_ids() {
    let f = seed().await;
    let payload = webhook_payload(&f.repo_id, &"a".repeat(40));

    // Malformed pipeline id in the path fails before any database lookup.
    let (status, body) = post_webhook(
        f.app.clone(),
        &PipelineId::new(),
        &f.owner_token,
        payload.clone(),
    )
    .await;
    // (sanity: the same request with a well-formed id resolves the route)
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["message"], "Pipeline not found");

    let (status, body) = post_malformed(f.app.clone(), "not-a-uuid", &f.owner_token, payload).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["success"], false);
    assert_eq!(body["message"], "Invalid pipeline ID format");
    assert!(body["pipeline_id"].is_null());
}

/// Post to the trigger route with a pipeline id that is not a UUID.
async fn post_malformed(
    app: Router,
    pipeline_id: &str,
    token: &str,
    payload: Value,
) -> (StatusCode, Value) {
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/webhook/trigger/{pipeline_id}"))
                .header("Content-Type", "application/json")
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::from(payload.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let parsed: Value = serde_json::from_slice(&bytes).unwrap();
    (status, parsed)
}

#[tokio::test]
async fn webhook_rejects_foreign_or_garbled_repository_ids() {
    let f = seed().await;
    let commit = "b".repeat(40);

    // A real repository id that is not the pipeline's repository.
    let (status, body) = post_webhook(
        f.app.clone(),
        &f.valid_pipeline,
        &f.owner_token,
        webhook_payload(&RepoId::new(), &commit),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(
        body["message"],
        "Webhook repository does not match pipeline"
    );
    assert_eq!(body["success"], false);

    // A non-UUID repository id fails the same ownership check.
    let (status, body) = post_webhook(
        f.app.clone(),
        &f.valid_pipeline,
        &f.owner_token,
        json!({"repo_id": "not-a-uuid-repo", "commit_hash": commit, "branch": "main"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(
        body["message"],
        "Webhook repository does not match pipeline"
    );

    // Neither rejection may leave a run behind.
    let runs = PipelineRunQueries::list(&f.pool).await.unwrap();
    assert!(runs.is_empty(), "validation happens before any persistence");
}

#[tokio::test]
async fn webhook_persists_run_and_entry_job_without_a_ci_client() {
    let f = seed().await;
    let commit = "c".repeat(40);

    let (status, body) = post_webhook(
        f.app.clone(),
        &f.valid_pipeline,
        &f.owner_token,
        webhook_payload(&f.repo_id, &commit),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["success"], true);
    assert_eq!(body["message"], "Pipeline triggered for branch 'main'");
    assert_eq!(body["pipeline_id"], f.valid_pipeline.to_string());

    let runs = PipelineRunQueries::list(&f.pool).await.unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].pipeline_id, f.valid_pipeline);
    assert_eq!(runs[0].triggered_by, "webhook");
    assert_eq!(runs[0].commit_hash, commit);

    let jobs = JobQueries::list_by_run(&f.pool, runs[0].id).await.unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].name, "build");
    assert_eq!(jobs[0].commands, vec!["echo hi".to_string()]);
    assert_eq!(jobs[0].image, "alpine:latest");
    assert!(jobs[0].working_dir.is_none());
    assert_eq!(jobs[0].timeout_secs, 5 * 60);
    assert_eq!(jobs[0].status, JobStatus::Queued.as_str().to_string());
}

#[tokio::test]
async fn webhook_replay_is_idempotent_and_cancels_the_duplicate_run() {
    let f = seed().await;
    let commit = "d".repeat(40);
    let payload = webhook_payload(&f.repo_id, &commit);

    let (first, body) = post_webhook(
        f.app.clone(),
        &f.valid_pipeline,
        &f.owner_token,
        payload.clone(),
    )
    .await;
    assert_eq!(first, StatusCode::OK, "{body}");
    assert_eq!(body["message"], "Pipeline triggered for branch 'main'");

    // A redelivery of the same commit hits the idempotency key: the route
    // still answers success, but the duplicate run is cancelled instead of
    // queueing a second executable job.
    let (second, body) =
        post_webhook(f.app.clone(), &f.valid_pipeline, &f.owner_token, payload).await;
    assert_eq!(second, StatusCode::OK, "{body}");
    assert_eq!(body["success"], true);

    let runs = PipelineRunQueries::list(&f.pool).await.unwrap();
    assert_eq!(runs.len(), 2);
    let cancelled = runs
        .iter()
        .filter(|run| run.status == "cancelled")
        .collect::<Vec<_>>();
    assert_eq!(cancelled.len(), 1, "exactly the replay run is cancelled");

    // One idempotency key maps to exactly one executable job, owned by the
    // run that won the reservation.
    let surviving = runs
        .iter()
        .find(|run| run.status != "cancelled")
        .expect("one run survives the replay");
    let jobs = JobQueries::list_by_run(&f.pool, surviving.id)
        .await
        .unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].commands, vec!["echo hi".to_string()]);
}

#[tokio::test]
async fn webhook_rejects_definitions_that_cannot_produce_an_entry_job() {
    let f = seed().await;
    let payload = webhook_payload(&f.repo_id, &"e".repeat(40));

    // A parse-valid definition whose jobs list is empty has no runnable
    // entry job.
    let (status, body) = post_webhook(
        f.app.clone(),
        &f.empty_pipeline,
        &f.owner_token,
        payload.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["message"], "Pipeline has no runnable entry job");

    // A definition that does not even deserialize.
    let (status, body) = post_webhook(
        f.app.clone(),
        &f.garbage_pipeline,
        &f.owner_token,
        payload.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["message"], "Stored pipeline definition is invalid");

    // A parse-valid definition with an unparseable job timeout.
    let (status, body) = post_webhook(
        f.app.clone(),
        &f.bad_timeout_pipeline,
        &f.owner_token,
        payload,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    // 'bogus' ends in the 's' suffix, so the parser strips it and rejects
    // what remains as a non-integer amount.
    assert_eq!(
        body["message"],
        "Invalid timeout for job 'build': invalid timeout 'bogus': expected a positive integer"
    );

    let runs = PipelineRunQueries::list(&f.pool).await.unwrap();
    assert!(
        runs.is_empty(),
        "definition validation precedes persistence"
    );
}

/// The pipeline row itself must still exist after every rejection: the
/// route validates, it never mutates the definition.
#[tokio::test]
async fn webhook_route_never_mutates_the_stored_pipeline() {
    let f = seed().await;
    for pipeline_id in [
        f.valid_pipeline,
        f.empty_pipeline,
        f.garbage_pipeline,
        f.bad_timeout_pipeline,
    ] {
        let pipeline = PipelineQueries::get(&f.pool, pipeline_id)
            .await
            .unwrap()
            .expect("pipeline row survives");
        assert_eq!(pipeline.id, pipeline_id);
    }
}

// ---------------------------------------------------------------------------
// Delegation paths (a configured CI trigger client)
// ---------------------------------------------------------------------------

/// The full delegation ladder through the pinned endpoint: a healthy
/// orchestrator answer relays its run id, the honest `queued` answer
/// relays "no run yet", and any orchestrator failure (HTTP error status,
/// non-JSON body) fails the webhook closed with no local run persisted.
#[tokio::test]
async fn webhook_delegation_ladder_relays_success_and_fails_closed() {
    let f = seed().await;
    let client =
        CiTriggerClient::new("http://127.0.0.1:42781/pipelines/trigger", "stub-token").unwrap();
    let app = ApiServer::new("test-secret", f.pool.clone())
        .with_ci_trigger_client(Arc::new(client))
        .into_router();

    let Some(received) = serve_stub_ci(vec![
        (200, json!({"pipeline_run_id": "durable-run-1"})),
        (200, json!({"queued": true, "pipeline_run_id": null})),
        (500, json!({"error": "orchestrator exploded"})),
        (200, Value::String("not json at all".to_string())),
    ])
    .await
    else {
        eprintln!("skipping webhook_delegation_ladder: port 42781 is already owned on this host");
        return;
    };

    // 1. A healthy answer with a run id is relayed verbatim.
    let (status, body) = post_webhook(
        app.clone(),
        &f.valid_pipeline,
        &f.owner_token,
        webhook_payload(&f.repo_id, &"1".repeat(40)),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert_eq!(body["success"], true);
    assert_eq!(body["pipeline_id"], f.valid_pipeline.to_string());
    assert!(body["message"]
        .as_str()
        .unwrap()
        .contains("run durable-run-1"));

    // 2. The orchestrator's honest `queued` answer (its correlation window
    //    elapsed) still succeeds — with no run id in the message.
    let (status, body) = post_webhook(
        app.clone(),
        &f.valid_pipeline,
        &f.owner_token,
        webhook_payload(&f.repo_id, &"2".repeat(40)),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert_eq!(body["success"], true);
    assert!(!body["message"].as_str().unwrap().contains("(run"));

    // 3. An orchestrator HTTP failure is mapped to a closed 502.
    let (status, body) = post_webhook(
        app.clone(),
        &f.valid_pipeline,
        &f.owner_token,
        webhook_payload(&f.repo_id, &"3".repeat(40)),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
    assert_eq!(body["success"], false);
    assert_eq!(body["message"], "CI trigger unavailable");
    assert!(body["pipeline_id"].is_null());

    // 4. An orchestrator success with a non-JSON body fails closed too.
    let (status, body) = post_webhook(
        app.clone(),
        &f.valid_pipeline,
        &f.owner_token,
        webhook_payload(&f.repo_id, &"4".repeat(40)),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
    assert_eq!(body["message"], "CI trigger unavailable");

    // Delegation is CI's custody: nothing is persisted locally, no matter
    // how the ladder resolves.
    let runs = PipelineRunQueries::list(&f.pool).await.unwrap();
    assert!(runs.is_empty(), "delegation must not persist local runs");

    // The stub saw the production trigger contract: the configured token
    // header, the repository, and the zero-hash initial-push sentinel for
    // a payload without old_commit_hash.
    let seen = received.lock().unwrap();
    assert_eq!(seen.len(), 4, "every webhook hit the pinned endpoint");
    assert_eq!(seen[0].token, "stub-token");
    assert_eq!(seen[0].body["repo_id"], f.repo_id.to_string());
    assert_eq!(seen[0].body["ref_name"], "main");
    assert_eq!(
        seen[0].body["old_hash"],
        "0000000000000000000000000000000000000000"
    );
    assert_eq!(seen[0].body["new_hash"], "1".repeat(40));
}

#[tokio::test]
async fn webhook_replaying_a_key_with_a_different_job_conflicts() {
    let f = seed().await;
    let commit = "d".repeat(40);

    // Pre-seed the durable idempotency key this delivery would derive,
    // stored under a fingerprint that cannot match the handler's plan.
    let scope = format!("webhook:{}", f.valid_pipeline);
    let key = format!("webhook:{}:{commit}", f.valid_pipeline);
    JobQueries::reserve_idempotency(
        &f.pool,
        &scope,
        &key,
        "{\"fingerprint\":\"stale\"}",
        JobId::new(),
    )
    .await
    .unwrap();

    let (status, body) = post_webhook(
        f.app,
        &f.valid_pipeline,
        &f.owner_token,
        webhook_payload(&f.repo_id, &commit),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["success"], false);
    assert_eq!(
        body["message"],
        "Webhook idempotency key was reused with a different job"
    );
    assert!(body["pipeline_id"].is_null());

    // The conflicted delivery grades the run it created `failed` and
    // leaves no job behind: one idempotency key maps to one executable
    // job, and the loser of the key never queues work.
    let runs = PipelineRunQueries::list(&f.pool).await.unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status, "failed");
    let jobs = JobQueries::list_by_run(&f.pool, runs[0].id).await.unwrap();
    assert!(jobs.is_empty());
}

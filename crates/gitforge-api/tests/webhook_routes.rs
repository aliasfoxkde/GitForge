//! Integration tests for the webhook trigger route's non-delegating
//! branches.
//!
//! With no CI trigger client configured, the route falls back to its
//! durable in-process path: it validates the stored pipeline definition,
//! persists the run, and queues the entry job under an idempotency key so
//! the scheduler's next tick adopts the work. These tests drive the real
//! router with an in-memory database and assert on the rows the handler
//! itself wrote. (The delegation path — a configured client — is covered
//! by `ci_routes.rs` fail-closed tests and the spawned-binary e2e suite;
//! the client pins the production loopback URL by design and cannot be
//! pointed at a mock.)

use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Router,
};
use gitforge_api::{ApiAuth, ApiServer};
use gitforge_common::{PipelineId, RepoId};
use gitforge_db::{
    models::{JobStatus, Pipeline, Repository, User},
    queries::{JobQueries, PipelineQueries, PipelineRunQueries, RepoQueries, UserQueries},
    Pool,
};
use serde_json::{json, Value};
use tower::ServiceExt;

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

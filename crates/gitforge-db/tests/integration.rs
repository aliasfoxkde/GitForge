//! Integration tests for GitForce database
//!
//! These tests use in-memory SQLite databases for testing.

use gitforge_common::PipelineId;
use gitforge_db::models::{
    Event, Job, Pipeline, PipelineRun, Repository, Runner, RunnerType, User,
};
use gitforge_db::publication_outbox::PublicationOutboxQueries;
use gitforge_db::queries::{
    EventQueries, JobQueries, PipelineQueries, PipelineRunQueries, RepoQueries, RunnerQueries,
    UserQueries,
};
use gitforge_db::Pool;

#[tokio::test]
async fn test_database_in_memory_pool() {
    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();

    // Create user
    let user = User::new(
        "testuser".to_string(),
        "test@example.com".to_string(),
        "hash".to_string(),
    );
    UserQueries::create(&pool, &user).await.unwrap();

    // Verify user can be retrieved
    let found = UserQueries::get(&pool, user.id).await.unwrap();
    assert!(found.is_some());
    assert_eq!(found.unwrap().username, "testuser");
}

#[tokio::test]
async fn test_database_repository_crud() {
    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();

    // Create user first
    let user = User::new(
        "owner".to_string(),
        "owner@example.com".to_string(),
        "hash".to_string(),
    );
    UserQueries::create(&pool, &user).await.unwrap();

    // Create repository
    let repo = Repository::new(
        "test-repo".to_string(),
        user.id,
        "/git/test-repo".to_string(),
    );
    RepoQueries::create(&pool, &repo).await.unwrap();

    // List repositories
    let repos = RepoQueries::list(&pool).await.unwrap();
    assert_eq!(repos.len(), 1);

    // Delete repository
    RepoQueries::delete(&pool, repo.id).await.unwrap();
    let repos = RepoQueries::list(&pool).await.unwrap();
    assert!(repos.is_empty());
}

#[tokio::test]
async fn test_database_runner_operations() {
    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();

    // Create runner
    let runner = Runner::new("test-runner".to_string(), RunnerType::Docker, 4);
    RunnerQueries::create(&pool, &runner).await.unwrap();

    // Verify runner
    let found = RunnerQueries::get(&pool, runner.id).await.unwrap();
    assert!(found.is_some());
    assert_eq!(found.unwrap().name, "test-runner");

    // Update status
    RunnerQueries::update_status(&pool, runner.id, "offline")
        .await
        .unwrap();
    let found = RunnerQueries::get(&pool, runner.id).await.unwrap();
    assert_eq!(found.unwrap().status, "offline");
}

#[tokio::test]
async fn test_database_pipeline_with_dependencies() {
    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();

    // Create user and repo
    let user = User::new(
        "owner".to_string(),
        "owner@example.com".to_string(),
        "hash".to_string(),
    );
    UserQueries::create(&pool, &user).await.unwrap();

    let repo = Repository::new(
        "test-repo".to_string(),
        user.id,
        "/git/test-repo".to_string(),
    );
    RepoQueries::create(&pool, &repo).await.unwrap();

    // Create pipeline
    let pipeline = Pipeline {
        id: PipelineId::new(),
        repo_id: repo.id,
        name: "CI Pipeline".to_string(),
        trigger_type: "push".to_string(),
        config: serde_json::json!({}),
        created_at: chrono::Utc::now(),
    };
    PipelineQueries::create(&pool, &pipeline).await.unwrap();

    // Create pipeline run
    let run = PipelineRun::new(
        pipeline.id,
        repo.id,
        "alice".to_string(),
        "abc123".to_string(),
    );
    PipelineRunQueries::create(&pool, &run).await.unwrap();

    // Create job
    let job = Job::new(run.id, "build".to_string());
    JobQueries::create(&pool, &job).await.unwrap();

    // Verify job
    let found = JobQueries::get(&pool, job.id).await.unwrap();
    assert!(found.is_some());
    assert_eq!(found.unwrap().name, "build");

    // List jobs by run
    let jobs = JobQueries::list_by_run(&pool, run.id).await.unwrap();
    assert_eq!(jobs.len(), 1);
}

#[tokio::test]
async fn test_database_event_storage() {
    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();

    // Create event
    let event = Event::new(
        "push.received".to_string(),
        serde_json::json!({"repo": "test", "branch": "main"}),
    );
    EventQueries::create(&pool, &event).await.unwrap();

    // List events by type
    let events = EventQueries::list_by_type(&pool, "push.received", 10)
        .await
        .unwrap();
    assert_eq!(events.len(), 1);

    // List recent events
    let recent = EventQueries::list_recent(&pool, 10).await.unwrap();
    assert_eq!(recent.len(), 1);
}

#[tokio::test]
async fn test_database_multiple_users() {
    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();

    // Create multiple users
    for i in 0..5 {
        let user = User::new(
            format!("user{}", i),
            format!("user{}@example.com", i),
            "hash".to_string(),
        );
        UserQueries::create(&pool, &user).await.unwrap();
    }

    // List all users
    let users = UserQueries::list(&pool).await.unwrap();
    assert_eq!(users.len(), 5);
}

#[tokio::test]
async fn test_database_job_state_transitions() {
    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();

    // Setup: user, repo, pipeline, run, job
    let user = User::new(
        "owner".to_string(),
        "owner@example.com".to_string(),
        "hash".to_string(),
    );
    UserQueries::create(&pool, &user).await.unwrap();

    let repo = Repository::new(
        "test-repo".to_string(),
        user.id,
        "/git/test-repo".to_string(),
    );
    RepoQueries::create(&pool, &repo).await.unwrap();

    let pipeline = Pipeline {
        id: PipelineId::new(),
        repo_id: repo.id,
        name: "Test".to_string(),
        trigger_type: "push".to_string(),
        config: serde_json::json!({}),
        created_at: chrono::Utc::now(),
    };
    PipelineQueries::create(&pool, &pipeline).await.unwrap();

    let run = PipelineRun::new(
        pipeline.id,
        repo.id,
        "alice".to_string(),
        "abc123".to_string(),
    );
    PipelineRunQueries::create(&pool, &run).await.unwrap();

    let job = Job::new(run.id, "build".to_string());
    JobQueries::create(&pool, &job).await.unwrap();

    // Create runner for assignment
    let runner = Runner::new("runner".to_string(), RunnerType::Docker, 2);
    RunnerQueries::create(&pool, &runner).await.unwrap();

    // Update job status to running
    JobQueries::update_status(&pool, job.id, "running")
        .await
        .unwrap();
    let found = JobQueries::get(&pool, job.id).await.unwrap();
    assert_eq!(found.unwrap().status, "running");

    // Assign runner
    JobQueries::assign(&pool, job.id, runner.id).await.unwrap();
    let found = JobQueries::get(&pool, job.id).await.unwrap();
    assert!(found.unwrap().runner_id.is_some());
}

#[tokio::test]
async fn test_database_durable_job_lease_fences_replay() {
    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();
    let user = User::new(
        "lease-owner".to_string(),
        "lease-owner@example.com".to_string(),
        "hash".to_string(),
    );
    UserQueries::create(&pool, &user).await.unwrap();
    let repo = Repository::new(
        "lease-repo".to_string(),
        user.id,
        "/git/lease-repo".to_string(),
    );
    RepoQueries::create(&pool, &repo).await.unwrap();
    let pipeline = Pipeline {
        id: PipelineId::new(),
        repo_id: repo.id,
        name: "lease-ci".to_string(),
        trigger_type: "manual".to_string(),
        config: serde_json::json!({}),
        created_at: chrono::Utc::now(),
    };
    PipelineQueries::create(&pool, &pipeline).await.unwrap();
    let run = PipelineRun::new(
        pipeline.id,
        repo.id,
        "lease-owner".to_string(),
        "lease-commit".to_string(),
    );
    PipelineRunQueries::create(&pool, &run).await.unwrap();
    let job = Job::new(run.id, "lease-job".to_string());
    JobQueries::create(&pool, &job).await.unwrap();
    let runner = Runner::new("lease-runner".to_string(), RunnerType::Docker, 1);
    RunnerQueries::create(&pool, &runner).await.unwrap();

    assert!(
        JobQueries::assign_with_lease(&pool, job.id, runner.id, "lease-a")
            .await
            .unwrap()
    );
    assert!(
        !JobQueries::assign_with_lease(&pool, job.id, runner.id, "lease-b")
            .await
            .unwrap()
    );
    assert!(
        !JobQueries::start_with_lease(&pool, job.id, runner.id, "lease-b")
            .await
            .unwrap()
    );
    assert!(
        JobQueries::start_with_lease(&pool, job.id, runner.id, "lease-a")
            .await
            .unwrap()
    );
    assert_eq!(
        JobQueries::append_log_with_lease(&pool, job.id, runner.id, "lease-b", "stale")
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        JobQueries::append_log_with_lease(&pool, job.id, runner.id, "lease-a", "hello\n")
            .await
            .unwrap(),
        Some(0)
    );
    assert_eq!(
        JobQueries::list_logs(&pool, job.id).await.unwrap()[0].chunk,
        "hello\n"
    );

    // stdout and stderr delivery can call the append endpoint concurrently.
    // Every accepted chunk must receive a unique sequence number rather than
    // surfacing a transient SQLite primary-key collision to the runner.
    let (a, b, c, d, e, f, g, h) = tokio::join!(
        JobQueries::append_log_with_lease(&pool, job.id, runner.id, "lease-a", "a"),
        JobQueries::append_log_with_lease(&pool, job.id, runner.id, "lease-a", "b"),
        JobQueries::append_log_with_lease(&pool, job.id, runner.id, "lease-a", "c"),
        JobQueries::append_log_with_lease(&pool, job.id, runner.id, "lease-a", "d"),
        JobQueries::append_log_with_lease(&pool, job.id, runner.id, "lease-a", "e"),
        JobQueries::append_log_with_lease(&pool, job.id, runner.id, "lease-a", "f"),
        JobQueries::append_log_with_lease(&pool, job.id, runner.id, "lease-a", "g"),
        JobQueries::append_log_with_lease(&pool, job.id, runner.id, "lease-a", "h"),
    );
    for result in [a, b, c, d, e, f, g, h] {
        assert!(result.unwrap().is_some());
    }
    let logs = JobQueries::list_logs(&pool, job.id).await.unwrap();
    assert_eq!(logs.len(), 9);
    assert_eq!(
        logs.iter().map(|entry| entry.sequence).collect::<Vec<_>>(),
        (0..9).collect::<Vec<_>>()
    );
    assert!(!JobQueries::complete_with_lease(
        &pool,
        job.id,
        runner.id,
        "lease-b",
        "succeeded",
        "{\"ok\":true}",
    )
    .await
    .unwrap());
    assert!(JobQueries::complete_with_lease_and_publication(
        &pool,
        job.id,
        runner.id,
        "lease-a",
        "succeeded",
        "{\"ok\":true}",
        "github",
        "check_run",
        "{\"status\":\"success\"}",
    )
    .await
    .unwrap());
    let publication = PublicationOutboxQueries::get(&pool, job.id, "github", "check_run")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(publication.payload, "{\"status\":\"success\"}");
}

#[tokio::test]
async fn test_database_pipeline_run_status_updates() {
    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();

    // Setup
    let user = User::new(
        "owner".to_string(),
        "owner@example.com".to_string(),
        "hash".to_string(),
    );
    UserQueries::create(&pool, &user).await.unwrap();

    let repo = Repository::new(
        "test-repo".to_string(),
        user.id,
        "/git/test-repo".to_string(),
    );
    RepoQueries::create(&pool, &repo).await.unwrap();

    let pipeline = Pipeline {
        id: PipelineId::new(),
        repo_id: repo.id,
        name: "Test".to_string(),
        trigger_type: "push".to_string(),
        config: serde_json::json!({}),
        created_at: chrono::Utc::now(),
    };
    PipelineQueries::create(&pool, &pipeline).await.unwrap();

    let run = PipelineRun::new(
        pipeline.id,
        repo.id,
        "alice".to_string(),
        "abc123".to_string(),
    );
    PipelineRunQueries::create(&pool, &run).await.unwrap();

    // Update status through various states
    for status in &["pending", "running", "succeeded"] {
        PipelineRunQueries::update_status(&pool, run.id, status)
            .await
            .unwrap();
        let found = PipelineRunQueries::get(&pool, run.id).await.unwrap();
        assert_eq!(found.unwrap().status, *status);
    }
}

// ============================================================================
// Review persistence (ADR 20260905 code review contract)
// ============================================================================

use gitforge_db::queries::{
    CreateOrGetReviewRun, FindingInsertOutcome, NewReviewFinding, NewReviewRun, ReviewQueries,
    ReviewRun,
};
use gitforge_review::domain::{PositionStatus, ReviewRunState};

/// Deterministic fixtures: fixed repo/user ids and immutable SHA strings so
/// failures are reproducible.
fn review_fixture() -> (uuid::Uuid, uuid::Uuid, NewReviewRun) {
    let user_id = uuid::Uuid::parse_str("00000000-0000-0000-0000-00000000a001").unwrap();
    let repo_id = uuid::Uuid::parse_str("00000000-0000-0000-0000-00000000b001").unwrap();
    let new_run = NewReviewRun {
        repo_id: Some(repo_id),
        base_sha: "1111111111111111111111111111111111111111".to_string(),
        head_sha: "2222222222222222222222222222222222222222".to_string(),
        idempotency_key: "review-key-001".to_string(),
        attempt: 1,
    };
    (user_id, repo_id, new_run)
}

async fn seeded_pool() -> (Pool, uuid::Uuid) {
    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();
    let (user_id, repo_id, _) = review_fixture();
    let user = User::new(
        "review-owner".to_string(),
        "review-owner@example.com".to_string(),
        "hash".to_string(),
    );
    let mut user = user;
    user.id = gitforge_common::UserId::from(user_id);
    UserQueries::create(&pool, &user).await.unwrap();
    let repo = Repository::new(
        "review-repo".to_string(),
        user_id.into(),
        "/git/review-repo".to_string(),
    );
    let mut repo = repo;
    repo.id = gitforge_common::RepoId::from(repo_id);
    RepoQueries::create(&pool, &repo).await.unwrap();
    (pool, repo_id)
}

fn line_finding(run_id: uuid::Uuid) -> NewReviewFinding {
    NewReviewFinding {
        run_id,
        source: "static-analysis".to_string(),
        file: "src/main.rs".to_string(),
        line: Some(42),
        severity: "high".to_string(),
        category: "no-hardcoded-secret".to_string(),
        title: "hardcoded secret".to_string(),
        message: "hardcoded password detected".to_string(),
        evidence: Some("let password = \"hunter2\";".to_string()),
        confidence: "high".to_string(),
        position_status: PositionStatus::Line,
    }
}

#[tokio::test]
async fn review_run_create_or_get_is_idempotent_per_key_and_head() {
    let (pool, _) = seeded_pool().await;
    let (_, _, new_run) = review_fixture();

    let first = ReviewQueries::create_or_get_run(&pool, &new_run)
        .await
        .unwrap();
    let created_id = match &first {
        CreateOrGetReviewRun::Created(run) => run.id,
        other => panic!("expected Created, got {other:?}"),
    };

    // Retry with the same key and head SHA returns the same run unchanged.
    let second = ReviewQueries::create_or_get_run(&pool, &new_run)
        .await
        .unwrap();
    match second {
        CreateOrGetReviewRun::Existing(run) => {
            assert_eq!(run.id, created_id);
            assert_eq!(run.head_sha, new_run.head_sha);
            assert_eq!(run.status, ReviewRunState::Pending);
        }
        other => panic!("expected Existing, got {other:?}"),
    }
}

#[tokio::test]
async fn review_run_same_key_different_head_is_typed_conflict() {
    let (pool, _) = seeded_pool().await;
    let (_, _, new_run) = review_fixture();
    let created = match ReviewQueries::create_or_get_run(&pool, &new_run)
        .await
        .unwrap()
    {
        CreateOrGetReviewRun::Created(run) => run,
        other => panic!("expected Created, got {other:?}"),
    };

    let mut replay = new_run.clone();
    replay.head_sha = "3333333333333333333333333333333333333333".to_string();
    match ReviewQueries::create_or_get_run(&pool, &replay)
        .await
        .unwrap()
    {
        CreateOrGetReviewRun::HeadConflict {
            existing,
            requested_head_sha,
        } => {
            assert_eq!(existing.id, created.id);
            assert_eq!(existing.head_sha, new_run.head_sha);
            assert_eq!(requested_head_sha, replay.head_sha);
        }
        other => panic!("expected HeadConflict, got {other:?}"),
    }
}

#[tokio::test]
async fn review_run_read_by_id_and_missing_id() {
    let (pool, _) = seeded_pool().await;
    let (_, _, new_run) = review_fixture();
    let created = match ReviewQueries::create_or_get_run(&pool, &new_run)
        .await
        .unwrap()
    {
        CreateOrGetReviewRun::Created(run) => run,
        other => panic!("expected Created, got {other:?}"),
    };

    let found: ReviewRun = ReviewQueries::get_run(&pool, created.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(found, created);
    assert!(ReviewQueries::get_run(&pool, uuid::Uuid::nil())
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn review_run_conditional_transitions_follow_monotonic_lifecycle() {
    let (pool, _) = seeded_pool().await;
    let (_, _, new_run) = review_fixture();
    let created = match ReviewQueries::create_or_get_run(&pool, &new_run)
        .await
        .unwrap()
    {
        CreateOrGetReviewRun::Created(run) => run,
        other => panic!("expected Created, got {other:?}"),
    };

    // pending → running → succeeded (happy path).
    let running = ReviewQueries::transition_run(&pool, created.id, ReviewRunState::Running)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(running.status, ReviewRunState::Running);

    let succeeded = ReviewQueries::transition_run(&pool, created.id, ReviewRunState::Succeeded)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(succeeded.status, ReviewRunState::Succeeded);

    // Terminal states are final: succeeded → running must fail, and the
    // stored state must remain succeeded.
    let error = ReviewQueries::transition_run(&pool, created.id, ReviewRunState::Running)
        .await
        .unwrap_err();
    assert_eq!(error.kind, gitforge_common::ErrorKind::InvalidInput);
    let unchanged = ReviewQueries::get_run(&pool, created.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(unchanged.status, ReviewRunState::Succeeded);
}

#[tokio::test]
async fn review_run_transition_unknown_id_returns_none() {
    let (pool, _) = seeded_pool().await;
    let missing = ReviewQueries::transition_run(
        &pool,
        uuid::Uuid::parse_str("00000000-0000-0000-0000-00000000d001").unwrap(),
        ReviewRunState::Running,
    )
    .await
    .unwrap();
    assert!(missing.is_none());
}

#[tokio::test]
async fn review_finding_insert_is_idempotent_by_fingerprint() {
    let (pool, _) = seeded_pool().await;
    let (_, _, new_run) = review_fixture();
    let created = match ReviewQueries::create_or_get_run(&pool, &new_run)
        .await
        .unwrap()
    {
        CreateOrGetReviewRun::Created(run) => run,
        other => panic!("expected Created, got {other:?}"),
    };

    let finding = line_finding(created.id);
    let first = ReviewQueries::insert_finding(&pool, &finding)
        .await
        .unwrap();
    let inserted = match &first {
        FindingInsertOutcome::Inserted(f) => f.clone(),
        other => panic!("expected Inserted, got {other:?}"),
    };
    let expected_fingerprint = gitforge_review::domain::finding_fingerprint(
        "src/main.rs",
        Some(42),
        "no-hardcoded-secret",
        "hardcoded password detected",
    );
    assert_eq!(inserted.fingerprint, expected_fingerprint);
    assert_eq!(inserted.line, Some(42));

    // Retried insertion of identical content returns the stored row.
    let second = ReviewQueries::insert_finding(&pool, &finding)
        .await
        .unwrap();
    match second {
        FindingInsertOutcome::Duplicate(duplicate) => {
            assert_eq!(duplicate, inserted);
        }
        other => panic!("expected Duplicate, got {other:?}"),
    }
    assert_eq!(
        ReviewQueries::list_findings(&pool, created.id)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn review_finding_line_position_invariant_is_enforced() {
    let (pool, _) = seeded_pool().await;
    let (_, _, new_run) = review_fixture();
    let created = match ReviewQueries::create_or_get_run(&pool, &new_run)
        .await
        .unwrap()
    {
        CreateOrGetReviewRun::Created(run) => run,
        other => panic!("expected Created, got {other:?}"),
    };

    // A `line` position without a line number must be rejected.
    let mut bad = line_finding(created.id);
    bad.line = None;
    assert!(ReviewQueries::insert_finding(&pool, &bad).await.is_err());

    // A file-level finding must not carry a line number.
    let mut file_level = line_finding(created.id);
    file_level.position_status = PositionStatus::File;
    file_level.line = Some(7);
    assert!(ReviewQueries::insert_finding(&pool, &file_level)
        .await
        .is_err());

    // A well-formed file-level finding with line = None is accepted.
    file_level.line = None;
    let inserted = ReviewQueries::insert_finding(&pool, &file_level)
        .await
        .unwrap();
    match inserted {
        FindingInsertOutcome::Inserted(f) => {
            assert_eq!(f.position_status, PositionStatus::File);
            assert_eq!(f.line, None);
        }
        other => panic!("expected Inserted, got {other:?}"),
    }
}

#[tokio::test]
async fn review_findings_cascade_delete_with_run() {
    let (pool, _) = seeded_pool().await;
    let (_, repo_id, new_run) = review_fixture();
    let created = match ReviewQueries::create_or_get_run(&pool, &new_run)
        .await
        .unwrap()
    {
        CreateOrGetReviewRun::Created(run) => run,
        other => panic!("expected Created, got {other:?}"),
    };
    ReviewQueries::insert_finding(&pool, &line_finding(created.id))
        .await
        .unwrap();

    RepoQueries::delete(&pool, gitforge_common::RepoId::from(repo_id))
        .await
        .unwrap();
    // The run's repo_id was SET NULL; the run itself survives.
    let run = ReviewQueries::get_run(&pool, created.id)
        .await
        .unwrap()
        .unwrap();
    assert!(run.repo_id.is_none());
    assert_eq!(
        ReviewQueries::list_findings(&pool, created.id)
            .await
            .unwrap()
            .len(),
        1
    );
}

// ============================================================================
// Review run claim (worker hand-off)
// ============================================================================

/// Insert a row directly with a chosen status and deterministic timestamps
/// so FIFO ordering tests can build known-good queues without depending on
/// wall-clock interleaving.
async fn insert_review_run_with_state(
    pool: &Pool,
    repo_id: uuid::Uuid,
    status: ReviewRunState,
    created_at: &str,
    idempotency_key: &str,
) -> ReviewRun {
    let new_run = NewReviewRun {
        repo_id: Some(repo_id),
        base_sha: format!("base-{idempotency_key}"),
        head_sha: format!("head-{idempotency_key}"),
        idempotency_key: idempotency_key.to_string(),
        attempt: 1,
    };
    let run = match ReviewQueries::create_or_get_run(pool, &new_run)
        .await
        .unwrap()
    {
        CreateOrGetReviewRun::Created(run) => run,
        other => panic!("expected Created, got {other:?}"),
    };
    sqlx::query("UPDATE review_runs SET status = ?, created_at = ?, updated_at = ? WHERE id = ?")
        .bind(status.to_string())
        .bind(created_at)
        .bind(created_at)
        .bind(run.id.to_string())
        .execute(pool.pool())
        .await
        .unwrap();
    ReviewQueries::get_run(pool, run.id)
        .await
        .unwrap()
        .expect("review run should be readable after direct status update")
}

#[tokio::test]
async fn claim_pending_returns_none_when_queue_is_empty() {
    let (pool, _) = seeded_pool().await;
    assert!(ReviewQueries::claim_pending(&pool).await.unwrap().is_none());
}

#[tokio::test]
async fn claim_pending_advances_oldest_pending_to_running_and_bumps_attempt() {
    let (pool, repo_id) = seeded_pool().await;

    // Build a queue where the second run is the oldest by `created_at`,
    // so we can prove FIFO ordering selects it.
    let newer = insert_review_run_with_state(
        &pool,
        repo_id,
        ReviewRunState::Pending,
        "2026-09-05T10:00:00Z",
        "claim-key-newer",
    )
    .await;
    let oldest = insert_review_run_with_state(
        &pool,
        repo_id,
        ReviewRunState::Pending,
        "2026-09-05T09:00:00Z",
        "claim-key-oldest",
    )
    .await;

    let claimed = ReviewQueries::claim_pending(&pool)
        .await
        .unwrap()
        .expect("a pending run should be claimable");
    assert_eq!(claimed.id, oldest.id);
    assert_eq!(claimed.status, ReviewRunState::Running);
    // Attempt semantics: the existing `attempt` counter (the only retry
    // counter on the schema) is incremented by the claim itself.
    assert_eq!(claimed.attempt, 2);

    // The remaining pending run is still claimable; a second claim must
    // advance it to running and bump its own attempt counter.
    let next = ReviewQueries::claim_pending(&pool)
        .await
        .unwrap()
        .expect("second oldest pending run should also be claimable");
    assert_eq!(next.id, newer.id);
    assert_eq!(next.status, ReviewRunState::Running);
    assert_eq!(next.attempt, 2);

    // The queue is now drained from the worker's perspective.
    assert!(ReviewQueries::claim_pending(&pool).await.unwrap().is_none());

    let stored = ReviewQueries::get_run(&pool, oldest.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.status, ReviewRunState::Running);
    assert_eq!(stored.attempt, 2);
}

#[tokio::test]
async fn claim_pending_ignores_running_terminal_and_missing_runs() {
    let (pool, repo_id) = seeded_pool().await;

    let _running = insert_review_run_with_state(
        &pool,
        repo_id,
        ReviewRunState::Running,
        "2026-09-05T08:00:00Z",
        "claim-key-running",
    )
    .await;
    let _succeeded = insert_review_run_with_state(
        &pool,
        repo_id,
        ReviewRunState::Succeeded,
        "2026-09-05T07:00:00Z",
        "claim-key-succeeded",
    )
    .await;
    let _failed = insert_review_run_with_state(
        &pool,
        repo_id,
        ReviewRunState::Failed,
        "2026-09-05T06:00:00Z",
        "claim-key-failed",
    )
    .await;
    let _cancelled = insert_review_run_with_state(
        &pool,
        repo_id,
        ReviewRunState::Cancelled,
        "2026-09-05T05:00:00Z",
        "claim-key-cancelled",
    )
    .await;

    // None of the rows above are eligible; FIFO on pending finds nothing.
    assert!(ReviewQueries::claim_pending(&pool).await.unwrap().is_none());
}

#[tokio::test]
async fn claim_pending_uses_id_as_deterministic_tiebreaker() {
    let (pool, repo_id) = seeded_pool().await;

    // Two pending runs with identical created_at: the lexicographically
    // smaller id wins, mirroring the schema's FIFO tiebreak.
    let a = insert_review_run_with_state(
        &pool,
        repo_id,
        ReviewRunState::Pending,
        "2026-09-05T11:00:00Z",
        "claim-tie-a",
    )
    .await;
    let b = insert_review_run_with_state(
        &pool,
        repo_id,
        ReviewRunState::Pending,
        "2026-09-05T11:00:00Z",
        "claim-tie-b",
    )
    .await;
    let (first_id, second_id) = if a.id < b.id {
        (a.id, b.id)
    } else {
        (b.id, a.id)
    };

    let first = ReviewQueries::claim_pending(&pool).await.unwrap().unwrap();
    assert_eq!(first.id, first_id);

    let second = ReviewQueries::claim_pending(&pool).await.unwrap().unwrap();
    assert_eq!(second.id, second_id);
    assert_eq!(second.status, ReviewRunState::Running);

    // The queue is now drained for pending rows.
    assert!(ReviewQueries::claim_pending(&pool).await.unwrap().is_none());
}

#[tokio::test]
async fn claim_pending_after_terminal_transition_is_a_no_op() {
    let (pool, _) = seeded_pool().await;
    let (_, _, new_run) = review_fixture();
    let created = match ReviewQueries::create_or_get_run(&pool, &new_run)
        .await
        .unwrap()
    {
        CreateOrGetReviewRun::Created(run) => run,
        other => panic!("expected Created, got {other:?}"),
    };

    // Move the run through the lifecycle to a terminal state.
    ReviewQueries::transition_run(&pool, created.id, ReviewRunState::Running)
        .await
        .unwrap();
    ReviewQueries::transition_run(&pool, created.id, ReviewRunState::Succeeded)
        .await
        .unwrap();

    // A terminal run must never become claimable again, even though it is
    // strictly "older" than any other pending run by created_at.
    assert!(ReviewQueries::claim_pending(&pool).await.unwrap().is_none());
    let stored = ReviewQueries::get_run(&pool, created.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.status, ReviewRunState::Succeeded);
}

#[tokio::test]
async fn claim_pending_under_concurrent_callers_partitions_unique_rows() {
    use std::sync::Arc;

    // Concurrent claim tests need a file-backed pool: the in-memory pool uses
    // private cache mode, so distinct connections observe separate
    // databases. The claim code is identical either way; the file pool is
    // what exercises real cross-connection durability.
    let db_path =
        std::env::temp_dir().join(format!("gitforge-claim-conc-{}.db", uuid::Uuid::new_v4()));
    let pool = Pool::new(&db_path.to_string_lossy()).await.unwrap();
    pool.migrate().await.unwrap();

    let (user_id, repo_id, _) = review_fixture();
    let user = User::new(
        "claim-conc-owner".to_string(),
        "claim-conc-owner@example.com".to_string(),
        "hash".to_string(),
    );
    let mut user = user;
    user.id = gitforge_common::UserId::from(user_id);
    UserQueries::create(&pool, &user).await.unwrap();
    let repo = Repository::new(
        "claim-conc-repo".to_string(),
        user_id.into(),
        "/git/claim-conc-repo".to_string(),
    );
    let mut repo = repo;
    repo.id = gitforge_common::RepoId::from(repo_id);
    RepoQueries::create(&pool, &repo).await.unwrap();

    let pool = Arc::new(pool);

    // Insert N pending runs; each concurrent caller must walk away with a
    // distinct id (no double-claim).
    let total = 8usize;
    for i in 0..total {
        let created_at = format!("2026-09-05T12:00:{i:02}Z");
        let key = format!("claim-conc-{i:02}");
        insert_review_run_with_state(&pool, repo_id, ReviewRunState::Pending, &created_at, &key)
            .await;
    }

    let mut handles = Vec::with_capacity(total);
    for _ in 0..total {
        let pool = Arc::clone(&pool);
        handles.push(tokio::spawn(async move {
            ReviewQueries::claim_pending(&pool).await
        }));
    }

    let mut claimed_ids = Vec::with_capacity(total);
    let mut races = 0usize;
    for handle in handles {
        match handle.await.unwrap().unwrap() {
            Some(run) => claimed_ids.push(run.id),
            None => races += 1,
        }
    }

    let unique = {
        let mut sorted = claimed_ids.clone();
        sorted.sort();
        sorted.dedup();
        sorted.len()
    };
    assert_eq!(
        unique,
        claimed_ids.len(),
        "every concurrent claim must yield a distinct review run id"
    );
    assert_eq!(
        claimed_ids.len() + races,
        total,
        "claim counts plus races must cover the full pending queue"
    );

    // After draining, the queue is empty from the worker perspective.
    assert!(ReviewQueries::claim_pending(&pool).await.unwrap().is_none());

    drop(pool);
    let _ = std::fs::remove_file(&db_path);
    let _ = std::fs::remove_file(db_path.with_extension("db-wal"));
    let _ = std::fs::remove_file(db_path.with_extension("db-shm"));
}

#[tokio::test]
async fn claim_pending_repeated_call_after_success_does_not_double_claim() {
    let (pool, repo_id) = seeded_pool().await;
    let _ = insert_review_run_with_state(
        &pool,
        repo_id,
        ReviewRunState::Pending,
        "2026-09-05T13:00:00Z",
        "claim-repeat-1",
    )
    .await;

    let first = ReviewQueries::claim_pending(&pool)
        .await
        .unwrap()
        .expect("first claim must succeed");
    let second = ReviewQueries::claim_pending(&pool).await.unwrap();
    assert!(
        second.is_none(),
        "a second claim must not re-claim the same row"
    );

    // attempt must have been incremented exactly once across both calls.
    let stored = ReviewQueries::get_run(&pool, first.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.status, ReviewRunState::Running);
    assert_eq!(stored.attempt, 2);
}

// ===========================================================================
// Restart recovery tests for P0-GF-RESTART-20260913
//
// These tests reproduce the failure mode from run 001a73d4-238f-447f-9ff7-
// bcf1ee9dbd71: a scheduler restart between claim and completion caused
// late completion messages and live log chunks to become 409 conflicts. The
// fixes preserve the runner's lease across the fence so late events still
// authenticate, and add an idempotent late-completion path that overwrites
// the synthetic fence receipt with the real one.
// ===========================================================================

/// Helper: create a durable lease, advance the job to the `running` status,
/// and return the lease token. Mirrors the real scheduler's assign + start
/// flow so the recovery tests exercise the same row shape.
async fn assign_and_start(
    pool: &Pool,
    job_id: gitforge_common::JobId,
    runner_id: gitforge_common::RunnerId,
    lease_token: &str,
) {
    assert!(
        JobQueries::assign_with_lease(pool, job_id, runner_id, lease_token)
            .await
            .unwrap(),
        "lease assignment must take effect on a fresh queued job"
    );
    assert!(
        JobQueries::start_with_lease(pool, job_id, runner_id, lease_token)
            .await
            .unwrap(),
        "start transition must succeed while the lease is active"
    );
}

/// Reproduce the incident: scheduler restart between `claim` and `complete`.
/// The fence must preserve the lease so the original runner's late
/// completion can still authenticate, and the new `complete_late_with_lease`
/// must overwrite the synthetic fence receipt with the real one.
#[tokio::test]
async fn test_restart_fence_accepts_late_completion_under_original_lease() {
    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();
    let user = User::new(
        "restart-owner".to_string(),
        "restart-owner@example.com".to_string(),
        "hash".to_string(),
    );
    UserQueries::create(&pool, &user).await.unwrap();
    let repo = Repository::new(
        "restart-repo".to_string(),
        user.id,
        "/git/restart-repo".to_string(),
    );
    RepoQueries::create(&pool, &repo).await.unwrap();
    let pipeline = Pipeline {
        id: PipelineId::new(),
        repo_id: repo.id,
        name: "restart-pipeline".to_string(),
        trigger_type: "manual".to_string(),
        config: serde_json::json!({}),
        created_at: chrono::Utc::now(),
    };
    PipelineQueries::create(&pool, &pipeline).await.unwrap();
    let run = PipelineRun::new(
        pipeline.id,
        repo.id,
        "restart-owner".to_string(),
        "restart-commit".to_string(),
    );
    PipelineRunQueries::create(&pool, &run).await.unwrap();
    let job = Job::new(run.id, "restart-job".to_string());
    JobQueries::create(&pool, &job).await.unwrap();
    let runner = Runner::new("restart-runner".to_string(), RunnerType::Docker, 1);
    RunnerQueries::create(&pool, &runner).await.unwrap();

    assign_and_start(&pool, job.id, runner.id, "lease-original").await;

    // 1. Simulate the scheduler restart: the recovery sweep fences running
    //    jobs without nullifying their runner_id or lease_token.
    JobQueries::requeue_inflight(&pool).await.unwrap();
    let fenced = JobQueries::get(&pool, job.id).await.unwrap().unwrap();
    assert_eq!(fenced.status, "failed", "fenced job must be visibly failed");
    assert_eq!(
        fenced.runner_id,
        Some(runner.id),
        "fence must preserve runner_id so late lease matches"
    );
    // The Job model does not expose the durable lease_token column; the
    // lease_matches query below verifies the column is preserved.
    assert!(
        JobQueries::lease_matches(&pool, job.id, runner.id, "lease-original")
            .await
            .unwrap(),
        "fence must preserve lease_token so late lease matches"
    );

    // 2. A stale lease from a different runner must NOT be accepted.
    let other_runner = Runner::new("other-runner".to_string(), RunnerType::Docker, 1);
    RunnerQueries::create(&pool, &other_runner).await.unwrap();
    assert!(
        !JobQueries::complete_late_with_lease(
            &pool,
            job.id,
            other_runner.id,
            "lease-original",
            "succeeded",
            r#"{"success":true}"#,
        )
        .await
        .unwrap(),
        "a foreign runner's lease must not authorize a late completion"
    );

    // 3. The original runner's late completion is accepted and overwrites
    //    the synthetic fence receipt with the real one.
    assert!(
        JobQueries::complete_late_with_lease(
            &pool,
            job.id,
            runner.id,
            "lease-original",
            "succeeded",
            r#"{"success":true,"exit_code":0}"#,
        )
        .await
        .unwrap(),
        "the original runner's late completion must authenticate"
    );
    let finalized = JobQueries::get(&pool, job.id).await.unwrap().unwrap();
    assert_eq!(finalized.status, "succeeded");
    assert_eq!(
        finalized.result_json.as_deref(),
        Some(r#"{"success":true,"exit_code":0}"#)
    );

    // 4. Replays of the same late completion must not duplicate state
    //    and the durable row must remain in the terminal state.
    assert!(
        JobQueries::complete_late_with_lease(
            &pool,
            job.id,
            runner.id,
            "lease-original",
            "succeeded",
            r#"{"success":true,"exit_code":0}"#,
        )
        .await
        .unwrap(),
        "an idempotent replay must still apply under the matching lease"
    );
}

/// Late log chunks arriving after a restart fence must still be accepted.
/// Without this, a streaming log line from a live container can land after
/// the scheduler decided the job was lost, and the runner surfaces a 409
/// for an otherwise valid chunk.
#[tokio::test]
async fn test_restart_fence_accepts_late_log_chunks_under_original_lease() {
    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();
    let user = User::new(
        "restart-log-owner".to_string(),
        "restart-log-owner@example.com".to_string(),
        "hash".to_string(),
    );
    UserQueries::create(&pool, &user).await.unwrap();
    let repo = Repository::new(
        "restart-log-repo".to_string(),
        user.id,
        "/git/restart-log-repo".to_string(),
    );
    RepoQueries::create(&pool, &repo).await.unwrap();
    let pipeline = Pipeline {
        id: PipelineId::new(),
        repo_id: repo.id,
        name: "restart-log-pipeline".to_string(),
        trigger_type: "manual".to_string(),
        config: serde_json::json!({}),
        created_at: chrono::Utc::now(),
    };
    PipelineQueries::create(&pool, &pipeline).await.unwrap();
    let run = PipelineRun::new(
        pipeline.id,
        repo.id,
        "restart-log-owner".to_string(),
        "restart-log-commit".to_string(),
    );
    PipelineRunQueries::create(&pool, &run).await.unwrap();
    let job = Job::new(run.id, "restart-log-job".to_string());
    JobQueries::create(&pool, &job).await.unwrap();
    let runner = Runner::new("restart-log-runner".to_string(), RunnerType::Docker, 1);
    RunnerQueries::create(&pool, &runner).await.unwrap();

    assign_and_start(&pool, job.id, runner.id, "lease-original").await;
    JobQueries::requeue_inflight(&pool).await.unwrap();

    // Foreign lease: must be silently rejected.
    let other_runner = Runner::new("foreign-log-runner".to_string(), RunnerType::Docker, 1);
    RunnerQueries::create(&pool, &other_runner).await.unwrap();
    assert_eq!(
        JobQueries::append_log_with_lease_late(
            &pool,
            job.id,
            other_runner.id,
            "lease-original",
            "must be fenced",
        )
        .await
        .unwrap(),
        None,
        "foreign runner's late log chunk must be silently rejected"
    );

    // Original lease: late chunk is appended with a stable sequence number.
    let sequence = JobQueries::append_log_with_lease_late(
        &pool,
        job.id,
        runner.id,
        "lease-original",
        "late line\n",
    )
    .await
    .unwrap()
    .expect("the original runner's lease must still authorize late chunks");
    let logs = JobQueries::list_logs(&pool, job.id).await.unwrap();
    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0].chunk, "late line\n");
    assert_eq!(logs[0].sequence, sequence);
}

/// The standard `complete_with_lease` path must NOT clobber a fenced row
/// once the lease no longer matches. This guarantees a runner that holds a
/// stale lease (because the scheduler rotated it) cannot rewrite history.
#[tokio::test]
async fn test_complete_with_lease_rejects_after_lease_rotation() {
    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();
    let user = User::new(
        "rotation-owner".to_string(),
        "rotation-owner@example.com".to_string(),
        "hash".to_string(),
    );
    UserQueries::create(&pool, &user).await.unwrap();
    let repo = Repository::new(
        "rotation-repo".to_string(),
        user.id,
        "/git/rotation-repo".to_string(),
    );
    RepoQueries::create(&pool, &repo).await.unwrap();
    let pipeline = Pipeline {
        id: PipelineId::new(),
        repo_id: repo.id,
        name: "rotation-pipeline".to_string(),
        trigger_type: "manual".to_string(),
        config: serde_json::json!({}),
        created_at: chrono::Utc::now(),
    };
    PipelineQueries::create(&pool, &pipeline).await.unwrap();
    let run = PipelineRun::new(
        pipeline.id,
        repo.id,
        "rotation-owner".to_string(),
        "rotation-commit".to_string(),
    );
    PipelineRunQueries::create(&pool, &run).await.unwrap();
    let job = Job::new(run.id, "rotation-job".to_string());
    JobQueries::create(&pool, &job).await.unwrap();
    let runner = Runner::new("rotation-runner".to_string(), RunnerType::Docker, 1);
    RunnerQueries::create(&pool, &runner).await.unwrap();
    assign_and_start(&pool, job.id, runner.id, "lease-a").await;
    JobQueries::requeue_inflight(&pool).await.unwrap();
    // After the fence the row is in a terminal status; simulate a future
    // scheduler tick explicitly clearing the lease before reassignment.
    sqlx::query("UPDATE jobs SET lease_token = 'lease-b' WHERE id = ?")
        .bind(job.id.to_string())
        .execute(pool.pool())
        .await
        .unwrap();
    // The old lease token must NOT be accepted as a late completion.
    assert!(
        !JobQueries::complete_late_with_lease(
            &pool,
            job.id,
            runner.id,
            "lease-a",
            "succeeded",
            r#"{"success":true}"#,
        )
        .await
        .unwrap(),
        "rotated lease must reject late completion under the old token"
    );
}

/// `lease_matches` must return true for a fenced row (preserved lease) but
/// false for an explicitly cleared lease. Used by the artifact upload path
/// to decide whether to accept a late upload without surfacing a 409.
#[tokio::test]
async fn test_lease_matches_after_restart_fence() {
    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();
    let user = User::new(
        "match-owner".to_string(),
        "match-owner@example.com".to_string(),
        "hash".to_string(),
    );
    UserQueries::create(&pool, &user).await.unwrap();
    let repo = Repository::new(
        "match-repo".to_string(),
        user.id,
        "/git/match-repo".to_string(),
    );
    RepoQueries::create(&pool, &repo).await.unwrap();
    let pipeline = Pipeline {
        id: PipelineId::new(),
        repo_id: repo.id,
        name: "match-pipeline".to_string(),
        trigger_type: "manual".to_string(),
        config: serde_json::json!({}),
        created_at: chrono::Utc::now(),
    };
    PipelineQueries::create(&pool, &pipeline).await.unwrap();
    let run = PipelineRun::new(
        pipeline.id,
        repo.id,
        "match-owner".to_string(),
        "match-commit".to_string(),
    );
    PipelineRunQueries::create(&pool, &run).await.unwrap();
    let job = Job::new(run.id, "match-job".to_string());
    JobQueries::create(&pool, &job).await.unwrap();
    let runner = Runner::new("match-runner".to_string(), RunnerType::Docker, 1);
    RunnerQueries::create(&pool, &runner).await.unwrap();
    assign_and_start(&pool, job.id, runner.id, "lease-a").await;
    JobQueries::requeue_inflight(&pool).await.unwrap();
    assert!(
        JobQueries::lease_matches(&pool, job.id, runner.id, "lease-a")
            .await
            .unwrap(),
        "fenced row must still report the lease as matching"
    );
    assert!(
        !JobQueries::lease_matches(&pool, job.id, runner.id, "lease-other")
            .await
            .unwrap(),
        "a foreign lease must not match"
    );
}

//! Integration tests for GitForce database
//!
//! These tests use in-memory SQLite databases for testing.

use gitforge_common::PipelineId;
use gitforge_db::models::{
    Event, Job, Pipeline, PipelineRun, Repository, Runner, RunnerType, User,
};
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
    assert!(JobQueries::complete_with_lease(
        &pool,
        job.id,
        runner.id,
        "lease-a",
        "succeeded",
        "{\"ok\":true}",
    )
    .await
    .unwrap());
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

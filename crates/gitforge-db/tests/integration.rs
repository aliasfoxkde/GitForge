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
async fn test_pipeline_versioning_active_uniqueness() {
    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();

    let user = User::new(
        "versioner".to_string(),
        "versioner@example.com".to_string(),
        "hash".to_string(),
    );
    UserQueries::create(&pool, &user).await.unwrap();

    let repo = Repository::new(
        "versioned-repo".to_string(),
        user.id,
        "/git/versioned-repo".to_string(),
    );
    RepoQueries::create(&pool, &repo).await.unwrap();

    let pipeline = |id: PipelineId| Pipeline {
        id,
        repo_id: repo.id,
        name: "gates".to_string(),
        trigger_type: "push".to_string(),
        config: serde_json::json!({}),
        created_at: chrono::Utc::now(),
    };

    // The partial UNIQUE index admits only one active version per
    // (repo, name): a second insert without retiring the predecessor must
    // be rejected...
    PipelineQueries::create(&pool, &pipeline(PipelineId::new()))
        .await
        .unwrap();
    assert_eq!(
        PipelineQueries::count_active(&pool, repo.id, "gates")
            .await
            .unwrap(),
        1
    );
    assert!(PipelineQueries::create(&pool, &pipeline(PipelineId::new()))
        .await
        .is_err());

    // ...and the push path retires the predecessor before recording the
    // new version, leaving exactly one active row again.
    PipelineQueries::deactivate_active(&pool, repo.id, "gates")
        .await
        .unwrap();
    PipelineQueries::create(&pool, &pipeline(PipelineId::new()))
        .await
        .unwrap();
    assert_eq!(
        PipelineQueries::count_active(&pool, repo.id, "gates")
            .await
            .unwrap(),
        1
    );
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

#[tokio::test]
async fn test_database_ssh_key_registry() {
    use gitforge_common::SshKeyId;
    use gitforge_db::models::SshKey;
    use gitforge_db::queries::SshKeyQueries;

    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();

    let owner = User::new(
        "keyowner".to_string(),
        "keys@example.com".to_string(),
        "hash".to_string(),
    );
    UserQueries::create(&pool, &owner).await.unwrap();
    let other = User::new(
        "otheruser".to_string(),
        "other@example.com".to_string(),
        "hash".to_string(),
    );
    UserQueries::create(&pool, &other).await.unwrap();

    let laptop = SshKey::new(
        owner.id,
        "laptop".to_string(),
        "SHA256:aaaa".to_string(),
        "ssh-ed25519 AAAA1 laptop".to_string(),
    );
    SshKeyQueries::create(&pool, &laptop).await.unwrap();
    let server_key = SshKey::new(
        owner.id,
        "server".to_string(),
        "SHA256:bbbb".to_string(),
        "ssh-ed25519 AAAA2 server".to_string(),
    );
    SshKeyQueries::create(&pool, &server_key).await.unwrap();

    // Fingerprints resolve to their owning account.
    let found = SshKeyQueries::find_by_fingerprint(&pool, "SHA256:aaaa")
        .await
        .unwrap()
        .expect("registered fingerprint resolves");
    assert_eq!(found.user_id, owner.id);
    assert_eq!(found.name, "laptop");
    assert!(SshKeyQueries::find_by_fingerprint(&pool, "SHA256:missing")
        .await
        .unwrap()
        .is_none());

    // Listing is scoped to one account.
    let listed = SshKeyQueries::list_by_user(&pool, owner.id).await.unwrap();
    assert_eq!(listed.len(), 2);
    assert!(SshKeyQueries::list_by_user(&pool, other.id)
        .await
        .unwrap()
        .is_empty());

    // The same public key can never belong to a second account.
    let duplicate = SshKey::new(
        other.id,
        "stolen".to_string(),
        "SHA256:aaaa".to_string(),
        "ssh-ed25519 AAAA1 laptop".to_string(),
    );
    let error = SshKeyQueries::create(&pool, &duplicate)
        .await
        .expect_err("duplicate fingerprint must be rejected");
    assert_eq!(error.kind, gitforge_common::ErrorKind::InvalidInput);

    // Deletion is fenced by ownership.
    assert!(
        !SshKeyQueries::delete_owned(&pool, laptop.id, other.id)
            .await
            .unwrap(),
        "another account must not delete someone else's key"
    );
    assert!(SshKeyQueries::delete_owned(&pool, laptop.id, owner.id)
        .await
        .unwrap());
    assert!(
        SshKeyQueries::get(&pool, SshKeyId::from(uuid::Uuid::new_v4()))
            .await
            .unwrap()
            .is_none()
    );
}

// ===========================================================================
// Restart recovery tests for P0-GF-RESTART-20260913
//
// These tests reproduce the failure mode from the production incident: a
// scheduler restart between claim and completion caused late completion
// messages and live log chunks to become 409 conflicts. The fixes preserve
// the runner's lease across the fence for a bounded grace window so late
// events still authenticate, then expire the lease so an old runner cannot
// rewrite a terminal row indefinitely.
// ===========================================================================

use gitforge_common::{JobId, RunnerId};

/// Helper: assign a queued job, advance it to `running`, and return the
/// lease token. Mirrors the real scheduler's assign + start flow so the
/// recovery tests exercise the same row shape.
async fn assign_and_start(pool: &Pool, job_id: JobId, runner_id: RunnerId, lease_token: &str) {
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

/// Helper: seed a user, repo, pipeline, run, and job for the recovery
/// tests. Returns the job so the caller can wire it into the lease flow.
async fn seed_pipeline_and_job(
    pool: &Pool,
    repo_name: &str,
    pipeline_name: &str,
    job_name: &str,
) -> (RunnerId, JobId) {
    let user = User::new(
        format!("restart-{}-owner", repo_name),
        format!("restart-{}-owner@example.com", repo_name),
        "hash".to_string(),
    );
    UserQueries::create(pool, &user).await.unwrap();
    let repo = Repository::new(
        repo_name.to_string(),
        user.id,
        format!("/git/{}", repo_name),
    );
    RepoQueries::create(pool, &repo).await.unwrap();
    let pipeline = Pipeline {
        id: PipelineId::new(),
        repo_id: repo.id,
        name: pipeline_name.to_string(),
        trigger_type: "manual".to_string(),
        config: serde_json::json!({}),
        created_at: chrono::Utc::now(),
    };
    PipelineQueries::create(pool, &pipeline).await.unwrap();
    let run = PipelineRun::new(
        pipeline.id,
        repo.id,
        user.username.clone(),
        "restart-commit".to_string(),
    );
    PipelineRunQueries::create(pool, &run).await.unwrap();
    let job = Job::new(run.id, job_name.to_string());
    let job_id = job.id;
    JobQueries::create(pool, &job).await.unwrap();
    let runner = Runner::new(
        format!("restart-{}-runner", repo_name),
        RunnerType::Docker,
        1,
    );
    RunnerQueries::create(pool, &runner).await.unwrap();
    (runner.id, job_id)
}

/// Reproduce the incident: a scheduler restart between `claim` and
/// `complete` must preserve the lease for a bounded grace window so the
/// original runner's late completion can still authenticate. The new
/// `complete_late_with_lease` must overwrite the synthetic fence receipt
/// with the real one and clear `lease_expires_at` / `fenced_at`.
#[tokio::test]
async fn test_restart_fence_preserves_lease_for_late_completion() {
    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();
    let (runner_id, job_id) =
        seed_pipeline_and_job(&pool, "late-completion", "late-pipeline", "late-job").await;
    assign_and_start(&pool, job_id, runner_id, "lease-original").await;

    // 1. Simulate the scheduler restart: the recovery sweep fences
    //    running jobs without nullifying their runner_id or lease_token,
    //    but sets a bounded `lease_expires_at`.
    JobQueries::requeue_inflight(&pool).await.unwrap();
    let fenced = JobQueries::get(&pool, job_id).await.unwrap().unwrap();
    assert_eq!(fenced.status, "failed", "fenced job must be visibly failed");
    assert_eq!(
        fenced.runner_id,
        Some(runner_id),
        "fence must preserve runner_id so late lease matches"
    );
    assert!(
        fenced.fenced_at.is_some(),
        "fence must stamp fenced_at for auditability"
    );
    let grace = fenced
        .lease_expires_at
        .expect("fence must stamp lease_expires_at");
    assert!(
        grace > chrono::Utc::now(),
        "grace window must be in the future"
    );

    // 2. A stale lease from a different runner must NOT be accepted.
    let other_runner = Runner::new("other-runner".to_string(), RunnerType::Docker, 1);
    RunnerQueries::create(&pool, &other_runner).await.unwrap();
    assert!(
        !JobQueries::complete_late_with_lease(
            &pool,
            job_id,
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
    //    the synthetic fence receipt with the real one. The grace and
    //    fence columns are cleared so the row is indistinguishable from
    //    one that completed through the active-lease path.
    assert!(
        JobQueries::complete_late_with_lease(
            &pool,
            job_id,
            runner_id,
            "lease-original",
            "succeeded",
            r#"{"success":true,"exit_code":0}"#,
        )
        .await
        .unwrap(),
        "the original runner's late completion must authenticate"
    );
    let finalized = JobQueries::get(&pool, job_id).await.unwrap().unwrap();
    assert_eq!(finalized.status, "succeeded");
    assert_eq!(
        finalized.result_json.as_deref(),
        Some(r#"{"success":true,"exit_code":0}"#)
    );
    assert!(
        finalized.lease_expires_at.is_none(),
        "grace window must clear on successful late completion"
    );
    assert!(
        finalized.fenced_at.is_none(),
        "fenced_at must clear on successful late completion"
    );

    // 4. Replays of the same late completion must not duplicate state
    //    and the durable row must remain in the terminal state.
    let replayed = JobQueries::complete_late_with_lease(
        &pool,
        job_id,
        runner_id,
        "lease-original",
        "succeeded",
        r#"{"success":true,"exit_code":0}"#,
    )
    .await
    .unwrap();
    assert!(
        !replayed,
        "an idempotent replay must be rejected (the row already moved past the fence)"
    );
}

/// The bounded reconciliation window must reject a late completion once
/// the lease has expired. Without this guard an offline runner could
/// rewrite a terminal row days after the restart.
#[tokio::test]
async fn test_late_completion_rejected_after_grace_expiry() {
    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();
    let (runner_id, job_id) =
        seed_pipeline_and_job(&pool, "grace-expiry", "grace-pipeline", "grace-job").await;
    assign_and_start(&pool, job_id, runner_id, "lease-original").await;
    JobQueries::requeue_inflight(&pool).await.unwrap();

    // Force the grace window into the past.
    JobQueries::_set_lease_expires_at_for_tests(
        &pool,
        job_id,
        chrono::Utc::now() - chrono::Duration::seconds(60),
    )
    .await
    .unwrap();

    assert!(
        !JobQueries::complete_late_with_lease(
            &pool,
            job_id,
            runner_id,
            "lease-original",
            "succeeded",
            r#"{"success":true,"exit_code":0}"#,
        )
        .await
        .unwrap(),
        "an expired lease must not authorize a late completion"
    );
    let row = JobQueries::get(&pool, job_id).await.unwrap().unwrap();
    assert_eq!(
        row.status, "failed",
        "the row must stay fenced after an expired-lease rejection"
    );
    assert_eq!(
        row.result_json.as_deref(),
        Some(r#"{"status":"failed","reason":"scheduler_restart_fenced_running_job"}"#)
    );
}

/// A late completion must NOT overwrite a row that was finalized by an
/// operator cancel, the watchdog timeout, or any other transition out of
/// the synthetic fence state. The fence-marker check is the
/// fail-closed guard against rewriting arbitrary terminal history.
#[tokio::test]
async fn test_late_completion_does_not_overwrite_unrelated_terminal_state() {
    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();
    let (runner_id, job_id) =
        seed_pipeline_and_job(&pool, "fence-marker", "fence-pipeline", "fence-job").await;
    assign_and_start(&pool, job_id, runner_id, "lease-original").await;
    JobQueries::requeue_inflight(&pool).await.unwrap();

    // Simulate the watchdog reconciling the fenced row into a timeout
    // before the runner's late completion arrives. The synthetic fence
    // marker is gone, the row is `timed_out`, and the runner's lease is
    // stale relative to that state.
    JobQueries::_override_status_for_tests(
        &pool,
        job_id,
        "timed_out",
        r#"{"status":"timed_out","reason":"job_timeout_reconciled_by_watchdog"}"#,
    )
    .await
    .unwrap();

    assert!(
        !JobQueries::complete_late_with_lease(
            &pool,
            job_id,
            runner_id,
            "lease-original",
            "succeeded",
            r#"{"success":true,"exit_code":0}"#,
        )
        .await
        .unwrap(),
        "a row that moved past the synthetic fence must not be overwritten"
    );
    let row = JobQueries::get(&pool, job_id).await.unwrap().unwrap();
    assert_eq!(
        row.status, "timed_out",
        "the watchdog terminal status must be preserved"
    );
}

/// Late log chunks arriving after a restart fence must still be accepted
/// within the bounded grace window. Without this, a streaming log line
/// from a live container can land after the scheduler decided the job was
/// lost, and the runner surfaces a 409 for an otherwise valid chunk.
#[tokio::test]
async fn test_restart_fence_accepts_late_log_chunks_within_grace() {
    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();
    let (runner_id, job_id) =
        seed_pipeline_and_job(&pool, "late-log", "log-pipeline", "log-job").await;
    assign_and_start(&pool, job_id, runner_id, "lease-original").await;
    JobQueries::requeue_inflight(&pool).await.unwrap();

    // Foreign lease: must be silently rejected.
    let other_runner = Runner::new("foreign-log-runner".to_string(), RunnerType::Docker, 1);
    RunnerQueries::create(&pool, &other_runner).await.unwrap();
    assert_eq!(
        JobQueries::append_log_with_lease_late(
            &pool,
            job_id,
            other_runner.id,
            "lease-original",
            "must be fenced",
        )
        .await
        .unwrap(),
        None,
        "foreign runner's late log chunk must be silently rejected"
    );

    // Original lease: late chunk is appended with a stable sequence
    // number and the chunk text is preserved verbatim (ordering).
    let sequence = JobQueries::append_log_with_lease_late(
        &pool,
        job_id,
        runner_id,
        "lease-original",
        "late line one\n",
    )
    .await
    .unwrap()
    .expect("the original runner's lease must still authorize late chunks");
    JobQueries::append_log_with_lease_late(
        &pool,
        job_id,
        runner_id,
        "lease-original",
        "late line two\n",
    )
    .await
    .unwrap();
    let logs = JobQueries::list_logs(&pool, job_id).await.unwrap();
    assert_eq!(logs.len(), 2);
    assert_eq!(logs[0].chunk, "late line one\n");
    assert_eq!(logs[1].chunk, "late line two\n");
    assert_eq!(logs[0].sequence, sequence);
    assert_eq!(logs[1].sequence, sequence + 1);
}

/// An expired-lease late log chunk must be rejected (size limit applies
/// only to chunks that the lease authorizes). Without the grace check a
/// runaway runner could exhaust the durable log volume on a job that has
/// already been fenced.
#[tokio::test]
async fn test_late_log_chunk_rejected_after_grace_expiry() {
    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();
    let (runner_id, job_id) =
        seed_pipeline_and_job(&pool, "log-expiry", "log-expiry-pipeline", "log-expiry-job").await;
    assign_and_start(&pool, job_id, runner_id, "lease-original").await;
    JobQueries::requeue_inflight(&pool).await.unwrap();
    JobQueries::_set_lease_expires_at_for_tests(
        &pool,
        job_id,
        chrono::Utc::now() - chrono::Duration::seconds(60),
    )
    .await
    .unwrap();
    assert_eq!(
        JobQueries::append_log_with_lease_late(
            &pool,
            job_id,
            runner_id,
            "lease-original",
            "stale line\n",
        )
        .await
        .unwrap(),
        None,
        "an expired lease must not authorize a late log chunk"
    );
    assert_eq!(
        JobQueries::list_logs(&pool, job_id).await.unwrap().len(),
        0,
        "no log chunks must be retained for an expired-lease append"
    );
}

/// An oversized late chunk must surface a structured error so callers
/// can distinguish it from a quiet rejection.
#[tokio::test]
async fn test_late_log_chunk_rejects_oversized_payload() {
    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();
    let (runner_id, job_id) = seed_pipeline_and_job(
        &pool,
        "log-oversize",
        "log-oversize-pipeline",
        "log-oversize-job",
    )
    .await;
    assign_and_start(&pool, job_id, runner_id, "lease-original").await;
    JobQueries::requeue_inflight(&pool).await.unwrap();
    let oversized = "x".repeat(gitforge_db::queries::MAX_JOB_LOG_CHUNK_BYTES + 1);
    let error = JobQueries::append_log_with_lease_late(
        &pool,
        job_id,
        runner_id,
        "lease-original",
        &oversized,
    )
    .await
    .expect_err("oversized late chunk must surface an error");
    assert_eq!(error.kind, gitforge_common::ErrorKind::InvalidInput);
}

/// `lease_matches_within_grace` must return true for a fenced row inside
/// its grace window and false after expiry. The artifact upload gate
/// uses this to decide whether to accept a late upload.
#[tokio::test]
async fn test_lease_matches_within_grace_window() {
    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();
    let (runner_id, job_id) =
        seed_pipeline_and_job(&pool, "lease-match", "lease-pipeline", "lease-job").await;
    assign_and_start(&pool, job_id, runner_id, "lease-a").await;
    JobQueries::requeue_inflight(&pool).await.unwrap();
    assert!(
        JobQueries::lease_matches_within_grace(&pool, job_id, runner_id, "lease-a")
            .await
            .unwrap(),
        "fenced row must report the lease as live inside the grace window"
    );
    assert!(
        !JobQueries::lease_matches_within_grace(&pool, job_id, runner_id, "lease-other")
            .await
            .unwrap(),
        "a foreign lease must not match"
    );
    JobQueries::_set_lease_expires_at_for_tests(
        &pool,
        job_id,
        chrono::Utc::now() - chrono::Duration::seconds(60),
    )
    .await
    .unwrap();
    assert!(
        !JobQueries::lease_matches_within_grace(&pool, job_id, runner_id, "lease-a")
            .await
            .unwrap(),
        "an expired lease must not match"
    );
}

/// `complete_with_lease` (the active-lease path) must clear the grace and
/// fence columns so a successful active-lease completion is
/// indistinguishable from one that came in through the late path.
#[tokio::test]
async fn test_active_lease_completion_clears_grace_columns() {
    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();
    let (runner_id, job_id) =
        seed_pipeline_and_job(&pool, "active-clear", "active-pipeline", "active-job").await;
    assign_and_start(&pool, job_id, runner_id, "lease-active").await;
    // Inject a fence-shaped row so we can confirm the clear.
    JobQueries::_stamp_grace_for_tests(
        &pool,
        job_id,
        chrono::Utc::now(),
        chrono::Utc::now() + chrono::Duration::seconds(60),
    )
    .await
    .unwrap();
    assert!(JobQueries::complete_with_lease(
        &pool,
        job_id,
        runner_id,
        "lease-active",
        "succeeded",
        r#"{"success":true}"#,
    )
    .await
    .unwrap());
    let row = JobQueries::get(&pool, job_id).await.unwrap().unwrap();
    assert!(row.lease_expires_at.is_none());
    assert!(row.fenced_at.is_none());
    // The active path clears `lease_token` but historically preserves
    // `runner_id` for auditability (the runner that ran the job is
    // always known). The grace/fence columns are what the new
    // bounded-reconciliation contract requires to be cleared.
}

/// Two concurrent same-token late completions must not both rewrite the
/// fence marker. The single conditional UPDATE in `complete_late_with_lease`
/// serializes the write at the row level; exactly one call returns `true`
/// and the durable row reflects the first writer's payload. This guards
/// against the read-then-write race that the original implementation
/// exhibited when both reads observed the synthetic fence marker before
/// either UPDATE ran.
#[tokio::test]
async fn test_concurrent_same_token_late_completion_rewrites_atomic() {
    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();
    let (runner_id, job_id) = seed_pipeline_and_job(
        &pool,
        "concurrent-late",
        "concurrent-pipeline",
        "concurrent-job",
    )
    .await;
    assign_and_start(&pool, job_id, runner_id, "lease-original").await;
    JobQueries::requeue_inflight(&pool).await.unwrap();

    let first_payload = r#"{"success":true,"exit_code":0,"writer":"first"}"#;
    let second_payload = r#"{"success":true,"exit_code":0,"writer":"second"}"#;

    // Two concurrent attempts with the same lease token. The race window is
    // tight on a single SQLite connection, so we serialize only the
    // observable writes: one UPDATE must report rows_affected == 1 and the
    // other must report 0.
    let (first, second) = tokio::join!(
        JobQueries::complete_late_with_lease(
            &pool,
            job_id,
            runner_id,
            "lease-original",
            "succeeded",
            first_payload,
        ),
        JobQueries::complete_late_with_lease(
            &pool,
            job_id,
            runner_id,
            "lease-original",
            "succeeded",
            second_payload,
        ),
    );

    let winners: Vec<bool> = [first.unwrap(), second.unwrap()]
        .into_iter()
        .filter(|won| *won)
        .collect();
    assert_eq!(
        winners.len(),
        1,
        "exactly one concurrent late completion must win; got {winners:?}"
    );

    // The losing writer must NOT have rewritten the durable row.
    let row = JobQueries::get(&pool, job_id).await.unwrap().unwrap();
    assert_eq!(row.status, "succeeded");
    assert!(
        row.result_json.as_deref() == Some(first_payload)
            || row.result_json.as_deref() == Some(second_payload),
        "durable result_json must be one of the submitted payloads, never a fused value"
    );
    assert!(
        row.lease_expires_at.is_none(),
        "the winning late completion must clear the grace window"
    );
    assert!(
        row.fenced_at.is_none(),
        "the winning late completion must clear fenced_at"
    );
}

/// Repeated restart sweeps must not extend the bounded grace window
/// indefinitely. The original implementation's `requeue_inflight` always
/// overwrote `lease_expires_at`, which let a misbehaving scheduler restart
/// loop extend an old runner's effective write authority forever. The
/// patched query only writes the fence columns when transitioning from a
/// live `running` row into the fenced state — a sweep against an
/// already-fenced row is a no-op for `lease_expires_at` and `fenced_at`.
#[tokio::test]
async fn test_repeated_requeue_inflight_does_not_extend_grace_window() {
    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();
    let (runner_id, job_id) = seed_pipeline_and_job(
        &pool,
        "fence-immunity",
        "fence-immunity-pipeline",
        "fence-immunity-job",
    )
    .await;
    assign_and_start(&pool, job_id, runner_id, "lease-original").await;

    // First sweep fences the row and stamps the original grace deadline.
    JobQueries::requeue_inflight(&pool).await.unwrap();
    let first = JobQueries::get(&pool, job_id).await.unwrap().unwrap();
    let original_deadline = first
        .lease_expires_at
        .expect("first sweep must stamp lease_expires_at");
    let original_fenced_at = first.fenced_at.expect("first sweep must stamp fenced_at");
    assert_eq!(first.status, "failed");

    // Run the sweep several more times. The fenced row is no longer
    // `running`, so the bounded predicate must skip it: the deadline and
    // fence stamp must be byte-for-byte identical to the first sweep's
    // values. A regression that always overwrites these columns would
    // let the deadline drift forward on every restart tick.
    for _ in 0..5 {
        JobQueries::requeue_inflight(&pool).await.unwrap();
    }
    let after_repeats = JobQueries::get(&pool, job_id).await.unwrap().unwrap();
    assert_eq!(
        after_repeats.lease_expires_at,
        Some(original_deadline),
        "repeated sweeps must not extend lease_expires_at past the original deadline"
    );
    assert_eq!(
        after_repeats.fenced_at,
        Some(original_fenced_at),
        "repeated sweeps must not stamp a new fenced_at"
    );
    assert_eq!(
        after_repeats.status, "failed",
        "the row must remain visibly fenced after repeated sweeps"
    );
}

/// `start_with_lease` must clear `lease_expires_at` and `fenced_at` on
/// the assigned → running transition. Without the clear, a row whose
/// previous attempt was fenced (e.g. by an earlier recovery sweep that
/// later requeued the job for a fresh assignment) would carry a stale
/// grace deadline forward, and the artifact-upload gate would
/// misclassify the row as still inside a (no-longer-relevant) grace
/// window.
#[tokio::test]
async fn test_start_with_lease_clears_grace_columns() {
    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();
    let (runner_id, job_id) = seed_pipeline_and_job(
        &pool,
        "start-clear",
        "start-clear-pipeline",
        "start-clear-job",
    )
    .await;
    // Stamp fence-shaped grace columns on the queued row so we can prove
    // the assigned → running transition clears them. We also seed the
    // runner_id and lease_token via the dedicated test helper so the
    // `start_with_lease` predicate can match without an intermediate
    // assignment round-trip — the contract being verified is "clearing
    // the grace columns", not the assigner.
    JobQueries::_stamp_grace_for_tests(
        &pool,
        job_id,
        chrono::Utc::now(),
        chrono::Utc::now() + chrono::Duration::seconds(60),
    )
    .await
    .unwrap();
    JobQueries::_assign_for_tests(&pool, job_id, runner_id, "lease-clear")
        .await
        .unwrap();
    let pre_start = JobQueries::get(&pool, job_id).await.unwrap().unwrap();
    assert_eq!(pre_start.status, "assigned");
    assert!(
        pre_start.lease_expires_at.is_some(),
        "test fixture must pre-populate lease_expires_at"
    );
    assert!(
        pre_start.fenced_at.is_some(),
        "test fixture must pre-populate fenced_at"
    );
    assert!(
        JobQueries::start_with_lease(&pool, job_id, runner_id, "lease-clear")
            .await
            .unwrap(),
        "start must succeed for the freshly assigned row"
    );
    let post_start = JobQueries::get(&pool, job_id).await.unwrap().unwrap();
    assert_eq!(post_start.status, "running");
    assert!(
        post_start.lease_expires_at.is_none(),
        "start_with_lease must clear lease_expires_at"
    );
    assert!(
        post_start.fenced_at.is_none(),
        "start_with_lease must clear fenced_at"
    );
}

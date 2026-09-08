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

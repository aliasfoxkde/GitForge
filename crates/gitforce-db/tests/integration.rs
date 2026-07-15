//! Integration tests for GitForce database
//!
//! These tests use in-memory SQLite databases for testing.

use gitforce_db::Pool;
use gitforce_db::queries::{RepoQueries, UserQueries, RunnerQueries, PipelineQueries, PipelineRunQueries, JobQueries, EventQueries};
use gitforce_db::models::{Repository, User, Runner, RunnerType, Pipeline, PipelineRun, Job, Event};
use gitforce_common::PipelineId;

#[tokio::test]
async fn test_database_in_memory_pool() {
    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();

    // Create user
    let user = User::new("testuser".to_string(), "test@example.com".to_string(), "hash".to_string());
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
    let user = User::new("owner".to_string(), "owner@example.com".to_string(), "hash".to_string());
    UserQueries::create(&pool, &user).await.unwrap();

    // Create repository
    let repo = Repository::new("test-repo".to_string(), user.id, "/git/test-repo".to_string());
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
    RunnerQueries::update_status(&pool, runner.id, "offline").await.unwrap();
    let found = RunnerQueries::get(&pool, runner.id).await.unwrap();
    assert_eq!(found.unwrap().status, "offline");
}

#[tokio::test]
async fn test_database_pipeline_with_dependencies() {
    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();

    // Create user and repo
    let user = User::new("owner".to_string(), "owner@example.com".to_string(), "hash".to_string());
    UserQueries::create(&pool, &user).await.unwrap();

    let repo = Repository::new("test-repo".to_string(), user.id, "/git/test-repo".to_string());
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
    let run = PipelineRun::new(pipeline.id, repo.id, "alice".to_string(), "abc123".to_string());
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
    let events = EventQueries::list_by_type(&pool, "push.received", 10).await.unwrap();
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
    let user = User::new("owner".to_string(), "owner@example.com".to_string(), "hash".to_string());
    UserQueries::create(&pool, &user).await.unwrap();

    let repo = Repository::new("test-repo".to_string(), user.id, "/git/test-repo".to_string());
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

    let run = PipelineRun::new(pipeline.id, repo.id, "alice".to_string(), "abc123".to_string());
    PipelineRunQueries::create(&pool, &run).await.unwrap();

    let job = Job::new(run.id, "build".to_string());
    JobQueries::create(&pool, &job).await.unwrap();

    // Create runner for assignment
    let runner = Runner::new("runner".to_string(), RunnerType::Docker, 2);
    RunnerQueries::create(&pool, &runner).await.unwrap();

    // Update job status to running
    JobQueries::update_status(&pool, job.id, "running").await.unwrap();
    let found = JobQueries::get(&pool, job.id).await.unwrap();
    assert_eq!(found.unwrap().status, "running");

    // Assign runner
    JobQueries::assign(&pool, job.id, runner.id).await.unwrap();
    let found = JobQueries::get(&pool, job.id).await.unwrap();
    assert!(found.unwrap().runner_id.is_some());
}

#[tokio::test]
async fn test_database_pipeline_run_status_updates() {
    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();

    // Setup
    let user = User::new("owner".to_string(), "owner@example.com".to_string(), "hash".to_string());
    UserQueries::create(&pool, &user).await.unwrap();

    let repo = Repository::new("test-repo".to_string(), user.id, "/git/test-repo".to_string());
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

    let run = PipelineRun::new(pipeline.id, repo.id, "alice".to_string(), "abc123".to_string());
    PipelineRunQueries::create(&pool, &run).await.unwrap();

    // Update status through various states
    for status in &["pending", "running", "succeeded"] {
        PipelineRunQueries::update_status(&pool, run.id, status).await.unwrap();
        let found = PipelineRunQueries::get(&pool, run.id).await.unwrap();
        assert_eq!(found.unwrap().status, *status);
    }
}

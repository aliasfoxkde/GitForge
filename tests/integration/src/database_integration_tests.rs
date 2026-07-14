//! Integration tests for Database Queries
//!
//! These tests verify database query functions with in-memory SQLite:
//! - Repository CRUD operations
//! - Pipeline and PipelineRun queries
//! - Job assignment and status updates
//! - Runner management

use gitforce_common::{JobId, PipelineId, PipelineRunId, RepoId, RunnerId, UserId};
use gitforce_db::models::{Job, Pipeline, PipelineRun, Repository, Runner, RunnerType, User};
use gitforce_db::queries::{JobQueries, PipelineQueries, PipelineRunQueries, RepoQueries, RunnerQueries, UserQueries};
use gitforce_db::Pool;
use crate::integration_test_helpers::*;

/// Test repository creation, retrieval, listing, and deletion
#[tokio::test]
async fn test_repo_crud_operations() {
    let pool = create_test_db_pool().await;

    // Create user first (repo has FK to owner)
    let user = create_test_user("repo_owner");
    UserQueries::create(&pool, &user).await.unwrap();

    // Create repository
    let repo = create_test_repo("test-repo", user.id);
    RepoQueries::create(&pool, &repo).await.unwrap();

    // Get repository
    let found = RepoQueries::get(&pool, repo.id).await.unwrap();
    assert!(found.is_some());
    let found = found.unwrap();
    assert_eq!(found.name, "test-repo");
    assert_eq!(found.owner_id, user.id);

    // List by owner
    let repos = RepoQueries::list_by_owner(&pool, user.id).await.unwrap();
    assert_eq!(repos.len(), 1);

    // List all
    let all_repos = RepoQueries::list(&pool).await.unwrap();
    assert_eq!(all_repos.len(), 1);

    // Delete repository
    RepoQueries::delete(&pool, repo.id).await.unwrap();
    let found = RepoQueries::get(&pool, repo.id).await.unwrap();
    assert!(found.is_none());
}

/// Test repository not found case
#[tokio::test]
async fn test_repo_not_found() {
    let pool = create_test_db_pool().await;

    let non_existent_id = RepoId::new();
    let found = RepoQueries::get(&pool, non_existent_id).await.unwrap();
    assert!(found.is_none());
}

/// Test user creation and retrieval
#[tokio::test]
async fn test_user_crud_operations() {
    let pool = create_test_db_pool().await;

    // Create user
    let user = create_test_user("testuser");
    UserQueries::create(&pool, &user).await.unwrap();

    // Get by ID
    let found = UserQueries::get(&pool, user.id).await.unwrap();
    assert!(found.is_some());
    assert_eq!(found.unwrap().username, "testuser");

    // Get by username
    let found = UserQueries::get_by_username(&pool, "testuser").await.unwrap();
    assert!(found.is_some());
    assert_eq!(found.unwrap().email, "test@example.com");

    // Not found case
    let found = UserQueries::get_by_username(&pool, "nonexistent").await.unwrap();
    assert!(found.is_none());

    // List all
    let all_users = UserQueries::list(&pool).await.unwrap();
    assert_eq!(all_users.len(), 1);
}

/// Test pipeline creation and retrieval
#[tokio::test]
async fn test_pipeline_crud_operations() {
    let pool = create_test_db_pool().await;

    // Create user and repo first
    let user = create_test_user("pipeline_owner");
    UserQueries::create(&pool, &user).await.unwrap();

    let repo = create_test_repo("pipeline-test-repo", user.id);
    RepoQueries::create(&pool, &repo).await.unwrap();

    // Create pipeline
    let pipeline = create_test_pipeline(repo.id);
    PipelineQueries::create(&pool, &pipeline).await.unwrap();

    // Get pipeline
    let found = PipelineQueries::get(&pool, pipeline.id).await.unwrap();
    assert!(found.is_some());
    assert_eq!(found.unwrap().name, "Test Pipeline");

    // List by repo
    let pipelines = PipelineQueries::list_by_repo(&pool, repo.id).await.unwrap();
    assert_eq!(pipelines.len(), 1);

    // List all
    let all_pipelines = PipelineQueries::list(&pool).await.unwrap();
    assert_eq!(all_pipelines.len(), 1);
}

/// Test pipeline run operations
#[tokio::test]
async fn test_pipeline_run_operations() {
    let pool = create_test_db_pool().await;

    // Create user and repo first
    let user = create_test_user("run_owner");
    UserQueries::create(&pool, &user).await.unwrap();

    let repo = create_test_repo("run-test-repo", user.id);
    RepoQueries::create(&pool, &repo).await.unwrap();

    let pipeline = create_test_pipeline(repo.id);
    PipelineQueries::create(&pool, &pipeline).await.unwrap();

    // Create pipeline run
    let run = create_test_pipeline_run(pipeline.id, repo.id);
    PipelineRunQueries::create(&pool, &run).await.unwrap();

    // Get pipeline run
    let found = PipelineRunQueries::get(&pool, run.id).await.unwrap();
    assert!(found.is_some());
    assert_eq!(found.unwrap().commit_hash, "abc123def456");

    // Update status
    PipelineRunQueries::update_status(&pool, run.id, "running").await.unwrap();
    let found = PipelineRunQueries::get(&pool, run.id).await.unwrap();
    assert_eq!(found.unwrap().status, "running");

    // List by pipeline
    let runs = PipelineRunQueries::list_by_pipeline(&pool, pipeline.id).await.unwrap();
    assert_eq!(runs.len(), 1);

    // List all
    let all_runs = PipelineRunQueries::list(&pool).await.unwrap();
    assert_eq!(all_runs.len(), 1);
}

/// Test job operations
#[tokio::test]
async fn test_job_operations() {
    let pool = create_test_db_pool().await;

    // Create user, repo, pipeline, run first
    let user = create_test_user("job_owner");
    UserQueries::create(&pool, &user).await.unwrap();

    let repo = create_test_repo("job-test-repo", user.id);
    RepoQueries::create(&pool, &repo).await.unwrap();

    let pipeline = create_test_pipeline(repo.id);
    PipelineQueries::create(&pool, &pipeline).await.unwrap();

    let run = create_test_pipeline_run(pipeline.id, repo.id);
    PipelineRunQueries::create(&pool, &run).await.unwrap();

    // Create job
    let job = Job::new(run.id, "build".to_string());
    JobQueries::create(&pool, &job).await.unwrap();

    // Get job
    let found = JobQueries::get(&pool, job.id).await.unwrap();
    assert!(found.is_some());
    assert_eq!(found.unwrap().name, "build");

    // Update status
    JobQueries::update_status(&pool, job.id, "running").await.unwrap();
    let found = JobQueries::get(&pool, job.id).await.unwrap();
    assert_eq!(found.unwrap().status, "running");

    // Assign runner
    let runner = create_test_runner("job-runner", RunnerType::Docker);
    RunnerQueries::create(&pool, &runner).await.unwrap();
    JobQueries::assign(&pool, job.id, runner.id).await.unwrap();

    // List by run
    let jobs = JobQueries::list_by_run(&pool, run.id).await.unwrap();
    assert_eq!(jobs.len(), 1);
}

/// Test pending jobs listing
#[tokio::test]
async fn test_pending_jobs() {
    let pool = create_test_db_pool().await;

    // Create user, repo, pipeline, run first
    let user = create_test_user("pending_job_owner");
    UserQueries::create(&pool, &user).await.unwrap();

    let repo = create_test_repo("pending-job-repo", user.id);
    RepoQueries::create(&pool, &repo).await.unwrap();

    let pipeline = create_test_pipeline(repo.id);
    PipelineQueries::create(&pool, &pipeline).await.unwrap();

    let run = create_test_pipeline_run(pipeline.id, repo.id);
    PipelineRunQueries::create(&pool, &run).await.unwrap();

    // Create multiple jobs
    let job1 = Job::new(run.id, "job1".to_string());
    let job2 = Job::new(run.id, "job2".to_string());
    JobQueries::create(&pool, &job1).await.unwrap();
    JobQueries::create(&pool, &job2).await.unwrap();

    // List pending (they're created with 'pending' status)
    let pending = JobQueries::list_pending(&pool).await.unwrap();
    assert!(pending.len() >= 2);
}

/// Test runner operations
#[tokio::test]
async fn test_runner_operations() {
    let pool = create_test_db_pool().await;

    // Create runner
    let runner = create_test_runner("test-runner", RunnerType::Docker);
    RunnerQueries::create(&pool, &runner).await.unwrap();

    // Get runner
    let found = RunnerQueries::get(&pool, runner.id).await.unwrap();
    assert!(found.is_some());
    assert_eq!(found.unwrap().name, "test-runner");

    // List runners
    let runners = RunnerQueries::list(&pool).await.unwrap();
    assert_eq!(runners.len(), 1);

    // Heartbeat
    RunnerQueries::heartbeat(&pool, runner.id).await.unwrap();

    // Update status
    RunnerQueries::update_status(&pool, runner.id, "offline").await.unwrap();
    let found = RunnerQueries::get(&pool, runner.id).await.unwrap();
    assert_eq!(found.unwrap().status, "offline");
}

/// Test runner online listing
#[tokio::test]
async fn test_runner_online_listing() {
    let pool = create_test_db_pool().await;

    // Create multiple runners with different statuses
    let runner1 = create_test_runner("runner1", RunnerType::Docker);
    let runner2 = create_test_runner("runner2", RunnerType::Firecracker);
    RunnerQueries::create(&pool, &runner1).await.unwrap();
    RunnerQueries::create(&pool, &runner2).await.unwrap();

    // List online (they start as online)
    let online = RunnerQueries::list_online(&pool).await.unwrap();
    assert_eq!(online.len(), 2);

    // Set one offline
    RunnerQueries::update_status(&pool, runner1.id, "offline").await.unwrap();

    let online = RunnerQueries::list_online(&pool).await.unwrap();
    assert_eq!(online.len(), 1);
}

/// Test event creation and listing
#[tokio::test]
async fn test_event_operations() {
    use gitforce_db::models::Event;
    use gitforce_db::queries::EventQueries;

    let pool = create_test_db_pool().await;

    // Create event
    let event = Event::new(
        "push.received".to_string(),
        serde_json::json!({"repo": "test", "branch": "main"}),
    );
    EventQueries::create(&pool, &event).await.unwrap();

    // List by type
    let events = EventQueries::list_by_type(&pool, "push.received", 10).await.unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_type, "push.received");

    // List recent
    let recent = EventQueries::list_recent(&pool, 10).await.unwrap();
    assert_eq!(recent.len(), 1);

    // List with limit
    let limited = EventQueries::list_by_type(&pool, "push.received", 1).await.unwrap();
    assert_eq!(limited.len(), 1);
}

/// Test multiple pipelines for same repo
#[tokio::test]
async fn test_multiple_pipelines_per_repo() {
    let pool = create_test_db_pool().await;

    // Create user and repo first
    let user = create_test_user("multi_pipeline_user");
    UserQueries::create(&pool, &user).await.unwrap();

    let repo = create_test_repo("multi-pipeline-repo", user.id);
    RepoQueries::create(&pool, &repo).await.unwrap();

    // Create multiple pipelines
    let pipeline1 = Pipeline {
        id: PipelineId::new(),
        repo_id: repo.id,
        name: "CI Pipeline".to_string(),
        trigger_type: "push".to_string(),
        config: serde_json::json!({}),
        created_at: chrono::Utc::now(),
    };
    let pipeline2 = Pipeline {
        id: PipelineId::new(),
        repo_id: repo.id,
        name: "CD Pipeline".to_string(),
        trigger_type: "tag".to_string(),
        config: serde_json::json!({}),
        created_at: chrono::Utc::now(),
    };

    PipelineQueries::create(&pool, &pipeline1).await.unwrap();
    PipelineQueries::create(&pool, &pipeline2).await.unwrap();

    // List by repo
    let pipelines = PipelineQueries::list_by_repo(&pool, repo.id).await.unwrap();
    assert_eq!(pipelines.len(), 2);
}
//! Integration test helpers shared across all integration tests

use gitforce_common::{JobId, PipelineId, PipelineRunId, RepoId, RunnerId, UserId};
use gitforce_db::models::{Pipeline, PipelineRun, Repository, Runner, RunnerType, User};
use gitforce_db::Pool;
use gitforce_scheduler::{Scheduler, scheduler_routes, create_state};
use gitforce_ci::{PipelineDefinition, PipelineTriggerEvent, TriggerType, JobDefinition, StepDefinition};
use std::collections::HashMap;

/// Create a test user
pub fn create_test_user(username: &str) -> User {
    User::new(
        username.to_string(),
        format!("{}@example.com", username),
        "hash_placeholder".to_string(),
    )
}

/// Create a test repository
pub fn create_test_repo(name: &str, owner_id: UserId) -> Repository {
    Repository::new(
        name.to_string(),
        owner_id,
        format!("/git/repos/{}", name),
    )
}

/// Create a test pipeline
pub fn create_test_pipeline(repo_id: RepoId) -> Pipeline {
    Pipeline {
        id: PipelineId::new(),
        repo_id,
        name: "Test Pipeline".to_string(),
        trigger_type: "push".to_string(),
        config: serde_json::json!({}),
        created_at: chrono::Utc::now(),
    }
}

/// Create a test pipeline run
pub fn create_test_pipeline_run(pipeline_id: PipelineId, repo_id: RepoId) -> PipelineRun {
    PipelineRun::new(
        pipeline_id,
        repo_id,
        "test_user".to_string(),
        "abc123def456".to_string(),
    )
}

/// Create a test runner
pub fn create_test_runner(name: &str, runner_type: RunnerType) -> Runner {
    Runner::new(name.to_string(), runner_type, 2)
}

/// Create a simple pipeline definition for testing
pub fn create_simple_pipeline_definition() -> PipelineDefinition {
    PipelineDefinition {
        name: "test-pipeline".to_string(),
        version: "1.0".to_string(),
        trigger_on: vec![TriggerType::Push],
        environment: HashMap::new(),
        jobs: vec![
            JobDefinition {
                name: "build".to_string(),
                image: "rust:latest".to_string(),
                needs: vec![],
                env: HashMap::new(),
                steps: vec![StepDefinition {
                    name: "build".to_string(),
                    run: "cargo build".to_string(),
                    env: None,
                    working_directory: None,
                    condition: None,
                }],
                timeout: None,
                retry: None,
            },
            JobDefinition {
                name: "test".to_string(),
                image: "rust:latest".to_string(),
                needs: vec!["build".to_string()],
                env: HashMap::new(),
                steps: vec![StepDefinition {
                    name: "test".to_string(),
                    run: "cargo test".to_string(),
                    env: None,
                    working_directory: None,
                    condition: None,
                }],
                timeout: None,
                retry: None,
            },
        ],
    }
}

/// Create a scheduler with test state
pub fn create_test_scheduler() -> Scheduler {
    Scheduler::new()
}

/// Create scheduler routes for testing
pub fn create_scheduler_test_app() -> axum::Router {
    let scheduler = Scheduler::new();
    let state = create_state(scheduler);
    scheduler_routes(state)
}

/// Set up a test database pool
pub async fn create_test_db_pool() -> Pool {
    let pool = Pool::memory().await.expect("failed to create memory pool");
    pool.migrate().await.expect("failed to run migrations");
    pool
}
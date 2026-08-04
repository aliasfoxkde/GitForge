//! Integration tests for the Scheduler
//!
//! These tests verify the scheduler's internal logic.
//! Note: HTTP route tests are limited due to issues with state passing in the router setup.

use gitforce_scheduler::{Scheduler, Priority};
use gitforce_common::{JobId, PipelineRunId, RepoId, RunnerId};
use gitforce_db::models::Runner;
use crate::integration_test_helpers::*;

/// Test scheduler job enqueuing
#[tokio::test]
async fn test_scheduler_enqueue() {
    let scheduler = Scheduler::new();

    let job_id = JobId::new();
    let run_id = PipelineRunId::new();
    let repo_id = RepoId::new();

    scheduler.enqueue(job_id, run_id, repo_id).await;
    assert_eq!(scheduler.queue_len().await, 1);
}

/// Test scheduler job enqueuing with priority
#[tokio::test]
async fn test_scheduler_enqueue_with_priority() {
    let scheduler = Scheduler::new();

    let job_id = JobId::new();
    let run_id = PipelineRunId::new();
    let repo_id = RepoId::new();

    scheduler.enqueue_with_priority(job_id, run_id, repo_id, Priority::High).await;
    assert_eq!(scheduler.queue_len().await, 1);
}

/// Test scheduler job cancellation
#[tokio::test]
async fn test_scheduler_cancel() {
    let scheduler = Scheduler::new();

    let job_id = JobId::new();
    let run_id = PipelineRunId::new();
    let repo_id = RepoId::new();

    scheduler.enqueue(job_id, run_id, repo_id).await;
    assert_eq!(scheduler.queue_len().await, 1);

    scheduler.cancel(job_id).await;
    assert_eq!(scheduler.queue_len().await, 0);
}

/// Test scheduler runner registration
#[tokio::test]
async fn test_scheduler_register_runner() {
    let scheduler = Scheduler::new();

    let runner = Runner::new("test-runner".to_string(), gitforce_db::models::RunnerType::Docker, 2);
    let runner_id = runner.id;

    scheduler.register_runner(runner).await;

    // Verify runner is registered by enqueuing a job and processing
    let job_id = JobId::new();
    let run_id = PipelineRunId::new();
    let repo_id = RepoId::new();

    scheduler.enqueue(job_id, run_id, repo_id).await;
    scheduler.process_queue().await;

    let assigned = scheduler.is_assigned(job_id).await;
    assert_eq!(assigned, Some(runner_id));
}

/// Test scheduler heartbeat
#[tokio::test]
async fn test_scheduler_heartbeat() {
    let scheduler = Scheduler::new();

    let runner = Runner::new("test-runner".to_string(), gitforce_db::models::RunnerType::Docker, 2);
    let runner_id = runner.id;

    scheduler.register_runner(runner).await;
    scheduler.heartbeat(runner_id).await;
    // No panic means success
}

/// Test scheduler runner offline
#[tokio::test]
async fn test_scheduler_runner_offline() {
    let scheduler = Scheduler::new();

    let runner = Runner::new("test-runner".to_string(), gitforce_db::models::RunnerType::Docker, 2);
    let runner_id = runner.id;

    scheduler.register_runner(runner).await;
    scheduler.runner_offline(runner_id).await;

    // Verify runner is offline by enqueuing a job and processing
    let job_id = JobId::new();
    let run_id = PipelineRunId::new();
    let repo_id = RepoId::new();

    scheduler.enqueue(job_id, run_id, repo_id).await;
    scheduler.process_queue().await;

    // Job should not be assigned since runner is offline
    let assigned = scheduler.is_assigned(job_id).await;
    assert!(assigned.is_none());
}

/// Test scheduler cancel nonexistent job
#[tokio::test]
async fn test_scheduler_cancel_nonexistent() {
    let scheduler = Scheduler::new();
    let job_id = JobId::new();

    // Cancel should not panic
    scheduler.cancel(job_id).await;
    assert_eq!(scheduler.queue_len().await, 0);
}

/// Test scheduler multiple runners
#[tokio::test]
async fn test_scheduler_multiple_runners() {
    let scheduler = Scheduler::new();

    let runner1 = Runner::new("runner-1".to_string(), gitforce_db::models::RunnerType::Docker, 4);
    let runner2 = Runner::new("runner-2".to_string(), gitforce_db::models::RunnerType::Firecracker, 2);

    scheduler.register_runner(runner1.clone()).await;
    scheduler.register_runner(runner2.clone()).await;

    // Enqueue jobs
    let job_id1 = JobId::new();
    let job_id2 = JobId::new();
    let run_id = PipelineRunId::new();
    let repo_id = RepoId::new();

    scheduler.enqueue(job_id1, run_id, repo_id).await;
    scheduler.enqueue(job_id2, run_id, repo_id).await;

    assert_eq!(scheduler.queue_len().await, 2);
}
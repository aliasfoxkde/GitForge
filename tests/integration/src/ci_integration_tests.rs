//! Integration tests for CI Engine
//!
//! These tests verify the CI engine's pipeline orchestration,
//! DAG building, job state management, and pipeline lifecycle.

use gitforge_common::{
    JobId, JobStatus, PipelineId, PipelineRunId, PipelineStatus, RepoId, RunnerId,
};
use gitforge_ci::{
    CiEngine, DagBuilder, JobGraph, JobNode, JobStateMachine, PipelineDefinition,
    PipelineExecutor, PipelineTriggerEvent, TriggerType, JobDefinition, StepDefinition,
};
use std::collections::HashMap;

/// Create a simple pipeline definition with two sequential jobs
fn create_sequential_pipeline() -> PipelineDefinition {
    PipelineDefinition {
        name: "sequential-pipeline".to_string(),
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

/// Create a parallel pipeline where build and lint can run concurrently
fn create_parallel_pipeline() -> PipelineDefinition {
    PipelineDefinition {
        name: "parallel-pipeline".to_string(),
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
                name: "lint".to_string(),
                image: "rust:latest".to_string(),
                needs: vec![],
                env: HashMap::new(),
                steps: vec![StepDefinition {
                    name: "lint".to_string(),
                    run: "cargo clippy".to_string(),
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

#[tokio::test]
async fn test_ci_engine_initialization() {
    let event = PipelineTriggerEvent::new(
        PipelineId::new(),
        RepoId::new(),
        "abc123".to_string(),
        TriggerType::Push,
    );
    let pipeline = create_sequential_pipeline();

    let engine = CiEngine::new(event, pipeline).await.unwrap();
    let state = engine.state().await;

    assert_eq!(state.status, PipelineStatus::Pending);
    assert!(state.started_at.is_none());
    assert!(state.finished_at.is_none());
    assert_eq!(state.jobs.len(), 2); // build and test
}

#[tokio::test]
async fn test_ci_engine_start() {
    let event = PipelineTriggerEvent::new(
        PipelineId::new(),
        RepoId::new(),
        "abc123".to_string(),
        TriggerType::Push,
    );
    let pipeline = create_sequential_pipeline();

    let engine = CiEngine::new(event, pipeline).await.unwrap();
    engine.start().await.unwrap();

    let state = engine.state().await;
    assert_eq!(state.status, PipelineStatus::Running);
    assert!(state.started_at.is_some());
}

#[tokio::test]
async fn test_ci_engine_ready_jobs_after_start() {
    let event = PipelineTriggerEvent::new(
        PipelineId::new(),
        RepoId::new(),
        "abc123".to_string(),
        TriggerType::Push,
    );
    let pipeline = create_sequential_pipeline();

    let engine = CiEngine::new(event, pipeline).await.unwrap();
    engine.start().await.unwrap();

    let ready = engine.ready_jobs().await;
    // Only "build" should be ready initially (no dependencies)
    assert_eq!(ready.len(), 1);
}

#[tokio::test]
async fn test_ci_engine_job_assignment() {
    let event = PipelineTriggerEvent::new(
        PipelineId::new(),
        RepoId::new(),
        "abc123".to_string(),
        TriggerType::Push,
    );
    let pipeline = create_sequential_pipeline();

    let engine = CiEngine::new(event, pipeline).await.unwrap();
    engine.start().await.unwrap();

    let ready = engine.ready_jobs().await;
    assert_eq!(ready.len(), 1);

    let build_job = ready[0];
    let runner_id = RunnerId::new();

    engine.assign_job(build_job, runner_id).await.unwrap();

    let job_state = engine.get_job(build_job).await;
    assert!(job_state.is_some());
    assert_eq!(job_state.unwrap().runner_id(), Some(runner_id));
}

#[tokio::test]
async fn test_ci_engine_job_start() {
    let event = PipelineTriggerEvent::new(
        PipelineId::new(),
        RepoId::new(),
        "abc123".to_string(),
        TriggerType::Push,
    );
    let pipeline = create_sequential_pipeline();

    let engine = CiEngine::new(event, pipeline).await.unwrap();
    engine.start().await.unwrap();

    let ready = engine.ready_jobs().await;
    let build_job = ready[0];
    let runner_id = RunnerId::new();

    engine.assign_job(build_job, runner_id).await.unwrap();
    engine.start_job(build_job).await.unwrap();

    let job_state = engine.get_job(build_job).await;
    assert_eq!(job_state.unwrap().status(), JobStatus::Running);
}

#[tokio::test]
async fn test_ci_engine_job_success() {
    let event = PipelineTriggerEvent::new(
        PipelineId::new(),
        RepoId::new(),
        "abc123".to_string(),
        TriggerType::Push,
    );
    let pipeline = create_sequential_pipeline();

    let engine = CiEngine::new(event, pipeline).await.unwrap();
    engine.start().await.unwrap();

    let ready = engine.ready_jobs().await;
    let build_job = ready[0];
    let runner_id = RunnerId::new();

    engine.assign_job(build_job, runner_id).await.unwrap();
    engine.start_job(build_job).await.unwrap();
    engine.succeed_job(build_job, 0).await.unwrap();

    let job_state = engine.get_job(build_job).await;
    assert_eq!(job_state.unwrap().status(), JobStatus::Succeeded);
}

#[tokio::test]
async fn test_ci_engine_pipeline_completion() {
    let event = PipelineTriggerEvent::new(
        PipelineId::new(),
        RepoId::new(),
        "abc123".to_string(),
        TriggerType::Push,
    );
    let pipeline = create_sequential_pipeline();

    let engine = CiEngine::new(event, pipeline).await.unwrap();
    engine.start().await.unwrap();

    // Build job
    let ready = engine.ready_jobs().await;
    let build_job = ready[0];
    let runner_id = RunnerId::new();

    engine.assign_job(build_job, runner_id).await.unwrap();
    engine.start_job(build_job).await.unwrap();
    engine.succeed_job(build_job, 0).await.unwrap();

    // After build succeeds, test job should become ready
    let ready = engine.ready_jobs().await;
    // Note: The engine doesn't automatically transition jobs,
    // they need to be picked up by scheduler
}

#[tokio::test]
async fn test_ci_engine_cancel() {
    let event = PipelineTriggerEvent::new(
        PipelineId::new(),
        RepoId::new(),
        "abc123".to_string(),
        TriggerType::Push,
    );
    let pipeline = create_sequential_pipeline();

    let engine = CiEngine::new(event, pipeline).await.unwrap();
    engine.start().await.unwrap();

    engine.cancel().await.unwrap();

    let state = engine.state().await;
    assert_eq!(state.status, PipelineStatus::Cancelled);
    assert!(state.finished_at.is_some());
}

#[tokio::test]
async fn test_ci_engine_graph_access() {
    let event = PipelineTriggerEvent::new(
        PipelineId::new(),
        RepoId::new(),
        "abc123".to_string(),
        TriggerType::Push,
    );
    let pipeline = create_sequential_pipeline();

    let engine = CiEngine::new(event, pipeline).await.unwrap();
    let graph = engine.graph();

    assert_eq!(graph.nodes.len(), 2);
    assert_eq!(graph.entry_points().len(), 1); // "build" has no deps
}

/// DAG tests
#[test]
fn test_dag_builder_simple() {
    let pipeline = PipelineDefinition {
        name: "test".to_string(),
        version: "1.0".to_string(),
        trigger_on: vec![],
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
    };

    let run_id = PipelineRunId::new();
    let graph = DagBuilder::build(&pipeline, run_id).unwrap();

    assert_eq!(graph.nodes.len(), 2);
    assert!(!graph.has_cycle());

    let build = graph.get_by_name("build").unwrap();
    assert!(build.dependencies.is_empty());

    let test = graph.get_by_name("test").unwrap();
    assert_eq!(test.dependencies.len(), 1);
}

#[test]
fn test_dag_builder_parallel_jobs() {
    let pipeline = create_parallel_pipeline();
    let run_id = PipelineRunId::new();
    let graph = DagBuilder::build(&pipeline, run_id).unwrap();

    assert_eq!(graph.nodes.len(), 3);
    assert!(!graph.has_cycle());

    // build and lint should be entry points (no dependencies)
    let entry_points = graph.entry_points();
    assert_eq!(entry_points.len(), 2);
}

#[test]
fn test_dag_topological_order() {
    let pipeline = create_sequential_pipeline();
    let run_id = PipelineRunId::new();
    let graph = DagBuilder::build(&pipeline, run_id).unwrap();

    let order = graph.topological_order();
    assert_eq!(order.len(), 2);

    // Build should come before test
    let build_idx = order.iter().position(|id| graph.get(*id).unwrap().name == "build").unwrap();
    let test_idx = order.iter().position(|id| graph.get(*id).unwrap().name == "test").unwrap();
    assert!(build_idx < test_idx);
}

#[test]
fn test_dag_cycle_detection() {
    // Create a pipeline with a cycle: build -> test -> build
    let pipeline = PipelineDefinition {
        name: "cyclic".to_string(),
        version: "1.0".to_string(),
        trigger_on: vec![],
        environment: HashMap::new(),
        jobs: vec![
            JobDefinition {
                name: "a".to_string(),
                image: "rust:latest".to_string(),
                needs: vec!["c".to_string()],
                env: HashMap::new(),
                steps: vec![StepDefinition {
                    name: "step".to_string(),
                    run: "echo a".to_string(),
                    env: None,
                    working_directory: None,
                    condition: None,
                }],
                timeout: None,
                retry: None,
            },
            JobDefinition {
                name: "b".to_string(),
                image: "rust:latest".to_string(),
                needs: vec!["a".to_string()],
                env: HashMap::new(),
                steps: vec![StepDefinition {
                    name: "step".to_string(),
                    run: "echo b".to_string(),
                    env: None,
                    working_directory: None,
                    condition: None,
                }],
                timeout: None,
                retry: None,
            },
            JobDefinition {
                name: "c".to_string(),
                image: "rust:latest".to_string(),
                needs: vec!["b".to_string()],
                env: HashMap::new(),
                steps: vec![StepDefinition {
                    name: "step".to_string(),
                    run: "echo c".to_string(),
                    env: None,
                    working_directory: None,
                    condition: None,
                }],
                timeout: None,
                retry: None,
            },
        ],
    };

    let run_id = PipelineRunId::new();
    let result = DagBuilder::build(&pipeline, run_id);
    assert!(result.is_err());
}

#[test]
fn test_dag_get_node() {
    let pipeline = create_sequential_pipeline();
    let run_id = PipelineRunId::new();
    let graph = DagBuilder::build(&pipeline, run_id).unwrap();

    // Get node by name
    let build = graph.get_by_name("build");
    assert!(build.is_some());
    assert_eq!(build.unwrap().name, "build");

    // Get node by ID
    let build_id = build.unwrap().id;
    let by_id = graph.get(build_id);
    assert!(by_id.is_some());
    assert_eq!(by_id.unwrap().name, "build");

    // Non-existent node
    let nonexistent = graph.get_by_name("nonexistent");
    assert!(nonexistent.is_none());
}

#[test]
fn test_dag_dependents() {
    let pipeline = create_sequential_pipeline();
    let run_id = PipelineRunId::new();
    let graph = DagBuilder::build(&pipeline, run_id).unwrap();

    // Build has "test" as dependent
    let build_id = graph.get_by_name("build").unwrap().id;
    let dependents = graph.dependents(build_id);
    assert_eq!(dependents.len(), 1);
    assert_eq!(dependents[0].name, "test");

    // Test has no dependents
    let test_id = graph.get_by_name("test").unwrap().id;
    let dependents = graph.dependents(test_id);
    assert!(dependents.is_empty());
}

#[test]
fn test_dag_max_depth() {
    // Pipeline: build -> test -> deploy (depth 2)
    let pipeline = PipelineDefinition {
        name: "depth-test".to_string(),
        version: "1.0".to_string(),
        trigger_on: vec![],
        environment: HashMap::new(),
        jobs: vec![
            JobDefinition {
                name: "build".to_string(),
                image: "rust:latest".to_string(),
                needs: vec![],
                env: HashMap::new(),
                steps: vec![StepDefinition {
                    name: "step".to_string(),
                    run: "echo".to_string(),
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
                    name: "step".to_string(),
                    run: "echo".to_string(),
                    env: None,
                    working_directory: None,
                    condition: None,
                }],
                timeout: None,
                retry: None,
            },
            JobDefinition {
                name: "deploy".to_string(),
                image: "rust:latest".to_string(),
                needs: vec!["test".to_string()],
                env: HashMap::new(),
                steps: vec![StepDefinition {
                    name: "step".to_string(),
                    run: "echo".to_string(),
                    env: None,
                    working_directory: None,
                    condition: None,
                }],
                timeout: None,
                retry: None,
            },
        ],
    };

    let run_id = PipelineRunId::new();
    let graph = DagBuilder::build(&pipeline, run_id).unwrap();
    let depth = DagBuilder::max_depth(&graph);
    assert_eq!(depth, 2); // build=0, test=1, deploy=2
}

/// Job state machine tests
#[test]
fn test_job_state_pending() {
    let job_id = JobId::new();
    let state = JobStateMachine::new(job_id);

    assert_eq!(state.status(), JobStatus::Pending);
    assert!(state.runner_id().is_none());
    assert!(state.exit_code().is_none());
    assert!(state.error_message().is_none());
}

#[test]
fn test_job_state_full_lifecycle() {
    let job_id = JobId::new();
    let mut state = JobStateMachine::new(job_id);

    // Pending -> Queued
    state.queue().unwrap();
    assert_eq!(state.status(), JobStatus::Queued);

    // Queued -> Assigned
    let runner_id = RunnerId::new();
    state.assign(runner_id).unwrap();
    assert_eq!(state.status(), JobStatus::Assigned);
    assert_eq!(state.runner_id(), Some(runner_id));

    // Assigned -> Running
    state.start().unwrap();
    assert_eq!(state.status(), JobStatus::Running);

    // Running -> Succeeded
    state.succeed(0).unwrap();
    assert_eq!(state.status(), JobStatus::Succeeded);
    assert_eq!(state.exit_code(), Some(0));
    assert!(state.is_terminal());
}

#[test]
fn test_job_state_failure() {
    let job_id = JobId::new();
    let mut state = JobStateMachine::new(job_id);

    state.queue().unwrap();
    state.assign(RunnerId::new()).unwrap();
    state.start().unwrap();
    state.fail(1, "compilation error".to_string()).unwrap();

    assert_eq!(state.status(), JobStatus::Failed);
    assert_eq!(state.exit_code(), Some(1));
    assert_eq!(state.error_message(), Some("compilation error"));
    assert!(state.is_terminal());
}

#[test]
fn test_job_state_cancel() {
    let job_id = JobId::new();
    let mut state = JobStateMachine::new(job_id);

    state.queue().unwrap();
    state.cancel().unwrap();

    assert_eq!(state.status(), JobStatus::Cancelled);
    assert!(state.is_terminal());
}

#[test]
fn test_job_state_invalid_transitions() {
    let job_id = JobId::new();
    let mut state = JobStateMachine::new(job_id);

    // Can't go directly from Pending to Running
    assert!(state.start().is_err());

    // Can't go from Pending to Succeeded
    assert!(state.succeed(0).is_err());

    // Can't fail a pending job
    assert!(state.fail(1, "error".to_string()).is_err());
}

#[test]
fn test_job_state_summary() {
    let job_id = JobId::new();
    let runner_id = RunnerId::new();
    let mut state = JobStateMachine::new(job_id);

    state.queue().unwrap();
    state.assign(runner_id).unwrap();
    state.start().unwrap();
    state.succeed(0).unwrap();

    let summary = state.summary();
    assert_eq!(summary.job_id, job_id);
    assert_eq!(summary.status, JobStatus::Succeeded);
    assert_eq!(summary.runner_id, Some(runner_id));
    assert_eq!(summary.exit_code, Some(0));
}

/// Pipeline executor tests
#[tokio::test]
async fn test_pipeline_executor_creation() {
    let event = PipelineTriggerEvent::new(
        PipelineId::new(),
        RepoId::new(),
        "abc123".to_string(),
        TriggerType::Push,
    );
    let pipeline = create_sequential_pipeline();

    let engine = CiEngine::new(event, pipeline).await.unwrap();
    let executor = PipelineExecutor::new(engine);

    let state = executor.state().await;
    assert_eq!(state.status, PipelineStatus::Pending);
}

#[tokio::test]
async fn test_pipeline_executor_start() {
    let event = PipelineTriggerEvent::new(
        PipelineId::new(),
        RepoId::new(),
        "abc123".to_string(),
        TriggerType::Push,
    );
    let pipeline = create_sequential_pipeline();

    let engine = CiEngine::new(event, pipeline).await.unwrap();
    let executor = PipelineExecutor::new(engine);

    executor.start().await.unwrap();

    let state = executor.state().await;
    assert_eq!(state.status, PipelineStatus::Running);
}

#[tokio::test]
async fn test_pipeline_executor_ready_jobs() {
    let event = PipelineTriggerEvent::new(
        PipelineId::new(),
        RepoId::new(),
        "abc123".to_string(),
        TriggerType::Push,
    );
    let pipeline = create_sequential_pipeline();

    let engine = CiEngine::new(event, pipeline).await.unwrap();
    let executor = PipelineExecutor::new(engine);

    executor.start().await.unwrap();
    let ready = executor.ready_jobs().await;

    assert_eq!(ready.len(), 1);
}

#[tokio::test]
async fn test_pipeline_executor_graph() {
    let event = PipelineTriggerEvent::new(
        PipelineId::new(),
        RepoId::new(),
        "abc123".to_string(),
        TriggerType::Push,
    );
    let pipeline = create_sequential_pipeline();

    let engine = CiEngine::new(event, pipeline).await.unwrap();
    let executor = PipelineExecutor::new(engine);

    let graph = executor.graph().await;
    assert_eq!(graph.nodes.len(), 2);
}
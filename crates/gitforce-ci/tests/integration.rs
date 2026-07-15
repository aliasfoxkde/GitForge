//! Integration tests for GitForce CI
//!
//! These tests require Docker and a running test database.

use gitforce_ci::{CiEngine, PipelineDefinition, PipelineTriggerEvent, TriggerType, DagBuilder};
use gitforce_common::{PipelineId, RepoId, PipelineRunId};
use std::collections::HashMap;

/// Create a test pipeline definition
fn make_pipeline(jobs: Vec<(&str, Vec<&str>)>) -> PipelineDefinition {
    let jobs = jobs.into_iter().map(|(name, needs)| {
        gitforce_ci::JobDefinition {
            name: name.to_string(),
            image: "rust:latest".to_string(),
            needs: needs.into_iter().map(|s| s.to_string()).collect(),
            env: HashMap::new(),
            steps: vec![],
            timeout: None,
            retry: None,
        }
    }).collect();

    PipelineDefinition {
        name: "test-pipeline".to_string(),
        version: "1.0".to_string(),
        trigger_on: vec![TriggerType::Push],
        environment: HashMap::new(),
        jobs,
    }
}

#[tokio::test]
async fn test_ci_engine_with_full_pipeline() {
    let pipeline_id = PipelineId::new();
    let repo_id = RepoId::new();
    let run_id = PipelineRunId::new();

    let trigger = PipelineTriggerEvent::new(
        pipeline_id,
        repo_id,
        "abc123".to_string(),
        TriggerType::Push,
    );

    let pipeline = make_pipeline(vec![
        ("build", vec![]),
        ("test", vec!["build"]),
        ("deploy", vec!["test"]),
    ]);

    let graph = DagBuilder::build(&pipeline, run_id).unwrap();
    assert_eq!(graph.nodes.len(), 3);

    let engine = CiEngine::new(trigger, pipeline).await.unwrap();
    assert_eq!(engine.state().await.status, gitforce_common::PipelineStatus::Pending);
}

#[tokio::test]
async fn test_ci_engine_single_job() {
    let pipeline_id = PipelineId::new();
    let repo_id = RepoId::new();
    let trigger = PipelineTriggerEvent::new(
        pipeline_id,
        repo_id,
        "abc123".to_string(),
        TriggerType::Push,
    );

    let pipeline = make_pipeline(vec![
        ("build", vec![]),
    ]);

    let engine = CiEngine::new(trigger, pipeline).await.unwrap();
    let state = engine.state().await;
    assert_eq!(state.status, gitforce_common::PipelineStatus::Pending);
}

#[tokio::test]
async fn test_ci_engine_no_jobs() {
    let pipeline_id = PipelineId::new();
    let repo_id = RepoId::new();
    let trigger = PipelineTriggerEvent::new(
        pipeline_id,
        repo_id,
        "abc123".to_string(),
        TriggerType::Push,
    );

    let pipeline = make_pipeline(vec![]);

    let engine = CiEngine::new(trigger, pipeline).await.unwrap();
    let ready = engine.ready_jobs().await;
    assert!(ready.is_empty());
}

#[test]
fn test_dag_with_parallel_jobs() {
    let pipeline = make_pipeline(vec![
        ("build", vec![]),
        ("test1", vec!["build"]),
        ("test2", vec!["build"]),
        ("integration", vec!["test1", "test2"]),
    ]);

    let run_id = PipelineRunId::new();
    let graph = DagBuilder::build(&pipeline, run_id).unwrap();

    assert_eq!(graph.nodes.len(), 4);
    assert!(!graph.has_cycle());

    let order = graph.topological_order();
    assert_eq!(order.len(), 4);
}

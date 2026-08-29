//! End-to-End Pipeline Flow Tests
//!
//! These tests verify complete flows across multiple components:
//! - Database -> Scheduler -> CI Engine integration
//! - Pipeline from trigger to completion

use gitforge_ci::{
    CiEngine, DagBuilder, JobGraph, PipelineDefinition, PipelineExecutor,
    PipelineTriggerEvent, TriggerType, JobDefinition, StepDefinition,
};
use gitforge_common::{
    JobId, JobStatus, PipelineId, PipelineRunId, PipelineStatus, RepoId, RunnerId, UserId,
};
use gitforge_db::models::{Job, Pipeline, PipelineRun, Repository, Runner, RunnerType, User};
use gitforge_db::queries::{
    JobQueries, PipelineQueries, PipelineRunQueries, RepoQueries, RunnerQueries, UserQueries,
};
use gitforge_db::Pool;
use gitforge_scheduler::{Scheduler, Priority};
use std::collections::HashMap;

/// Create a complete test database with all entities
async fn setup_complete_pipeline_db(pool: &Pool) -> (User, Repository, Pipeline, PipelineRun, Vec<Job>) {
    // Create user
    let user = User::new(
        "test_user".to_string(),
        "test@example.com".to_string(),
        "hash_placeholder".to_string(),
    );
    UserQueries::create(pool, &user).await.unwrap();

    // Create repository
    let repo = Repository::new(
        "test-repo".to_string(),
        user.id,
        "/git/repos/test-repo".to_string(),
    );
    RepoQueries::create(pool, &repo).await.unwrap();

    // Create pipeline
    let pipeline = Pipeline {
        id: PipelineId::new(),
        repo_id: repo.id,
        name: "CI Pipeline".to_string(),
        trigger_type: "push".to_string(),
        config: serde_json::json!({}),
        created_at: chrono::Utc::now(),
    };
    PipelineQueries::create(pool, &pipeline).await.unwrap();

    // Create pipeline run
    let run = PipelineRun::new(
        pipeline.id,
        repo.id,
        user.username.clone(),
        "abc123def456".to_string(),
    );
    PipelineRunQueries::create(pool, &run).await.unwrap();

    // Create jobs for the pipeline
    let jobs = vec![
        Job::new(run.id, "build".to_string()),
        Job::new(run.id, "test".to_string()),
        Job::new(run.id, "lint".to_string()),
    ];

    for job in &jobs {
        JobQueries::create(pool, job).await.unwrap();
    }

    (user, repo, pipeline, run, jobs)
}

#[tokio::test]
async fn test_full_pipeline_db_flow() {
    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();

    let (user, repo, pipeline, run, jobs) = setup_complete_pipeline_db(&pool).await;

    // Verify all entities were created
    assert_eq!(user.username, "test_user");
    assert_eq!(repo.name, "test-repo");
    assert_eq!(pipeline.name, "CI Pipeline");
    assert_eq!(run.commit_hash, "abc123def456");
    assert_eq!(jobs.len(), 3);

    // Verify jobs can be retrieved
    for job in &jobs {
        let found = JobQueries::get(&pool, job.id).await.unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().name, job.name);
    }
}

#[tokio::test]
async fn test_pipeline_run_status_updates() {
    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();

    let (_, _, _, run, _) = setup_complete_pipeline_db(&pool).await;

    // Initially pending
    let found = PipelineRunQueries::get(&pool, run.id).await.unwrap().unwrap();
    assert_eq!(found.status, "pending");

    // Update to running
    PipelineRunQueries::update_status(&pool, run.id, "running").await.unwrap();
    let found = PipelineRunQueries::get(&pool, run.id).await.unwrap().unwrap();
    assert_eq!(found.status, "running");

    // Update to success
    PipelineRunQueries::update_status(&pool, run.id, "success").await.unwrap();
    let found = PipelineRunQueries::get(&pool, run.id).await.unwrap().unwrap();
    assert_eq!(found.status, "success");
}

#[tokio::test]
async fn test_job_assignment_flow() {
    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();

    let (_, _, _, _, jobs) = setup_complete_pipeline_db(&pool).await;

    // Create a runner
    let runner = Runner::new("test-runner".to_string(), RunnerType::Docker, 2);
    RunnerQueries::create(&pool, &runner).await.unwrap();

    // Assign job to runner
    let job = &jobs[0];
    JobQueries::assign(&pool, job.id, runner.id).await.unwrap();

    // Verify assignment - note: assign only sets runner_id, not status
    let found = JobQueries::get(&pool, job.id).await.unwrap().unwrap();
    assert_eq!(found.runner_id, Some(runner.id));
    // Status remains "pending" since assign doesn't update status
    assert_eq!(found.status, "pending");
}

#[tokio::test]
async fn test_scheduler_with_runner_registration() {
    let scheduler = Scheduler::new();

    // Register multiple runners
    let runner1 = Runner::new("runner-1".to_string(), RunnerType::Docker, 4);
    let runner2 = Runner::new("runner-2".to_string(), RunnerType::Firecracker, 2);

    scheduler.register_runner(runner1.clone()).await;
    scheduler.register_runner(runner2.clone()).await;

    // Enqueue jobs
    let job_id1 = JobId::new();
    let job_id2 = JobId::new();
    let run_id = PipelineRunId::new();
    let repo_id = RepoId::new();

    scheduler.enqueue_with_priority(job_id1, run_id, repo_id, Priority::High).await;
    scheduler.enqueue(job_id2, run_id, repo_id).await;

    assert_eq!(scheduler.queue_len().await, 2);

    // Process queue (should assign to runner with most capacity)
    scheduler.process_queue().await;

    // One job should be assigned
    let assigned = scheduler.is_assigned(job_id1).await;
    assert!(assigned.is_some());
}

#[tokio::test]
async fn test_scheduler_job_cancel() {
    let scheduler = Scheduler::new();

    let job_id = JobId::new();
    let run_id = PipelineRunId::new();
    let repo_id = RepoId::new();

    scheduler.enqueue(job_id, run_id, repo_id).await;
    assert_eq!(scheduler.queue_len().await, 1);

    scheduler.cancel(job_id).await;
    assert_eq!(scheduler.queue_len().await, 0);
}

#[tokio::test]
async fn test_scheduler_runner_offline() {
    let scheduler = Scheduler::new();

    let runner = Runner::new("offline-test".to_string(), RunnerType::Docker, 2);
    scheduler.register_runner(runner.clone()).await;

    let runner_id = runner.id;
    scheduler.runner_offline(runner_id).await;

    // Verify runner is offline by checking process queue
    let job_id = JobId::new();
    scheduler.enqueue(job_id, PipelineRunId::new(), RepoId::new()).await;
    scheduler.process_queue().await;

    // Job should not be assigned since runner is offline
    let assigned = scheduler.is_assigned(job_id).await;
    assert!(assigned.is_none());
}

#[tokio::test]
async fn test_ci_engine_pipeline_parsing() {
    let yaml = r#"
name: test-pipeline
version: "1.0"
trigger_on:
  - push
  - pull_request
environment:
  RUST_BACKTRACE: "1"
jobs:
  - name: build
    image: rust:latest
    steps:
      - name: build
        run: cargo build --release
  - name: test
    needs:
      - build
    image: rust:latest
    steps:
      - name: test
        run: cargo test
"#;

    let pipeline = PipelineDefinition::parse(yaml).unwrap();
    assert_eq!(pipeline.name, "test-pipeline");
    assert_eq!(pipeline.version, "1.0");
    assert_eq!(pipeline.trigger_on.len(), 2);
    assert_eq!(pipeline.jobs.len(), 2);
    assert_eq!(pipeline.jobs[0].name, "build");
    assert_eq!(pipeline.jobs[1].name, "test");
    assert_eq!(pipeline.jobs[1].needs, vec!["build"]);
}

#[tokio::test]
async fn test_ci_engine_pipeline_to_yaml() {
    let pipeline = PipelineDefinition {
        name: "yaml-test".to_string(),
        version: "1.0".to_string(),
        trigger_on: vec![TriggerType::Manual],
        environment: HashMap::new(),
        jobs: vec![JobDefinition {
            name: "deploy".to_string(),
            image: "docker:latest".to_string(),
            needs: vec![],
            env: HashMap::new(),
            steps: vec![StepDefinition {
                name: "deploy".to_string(),
                run: "kubectl apply".to_string(),
                env: None,
                working_directory: None,
                condition: None,
            }],
            timeout: Some("30m".to_string()),
            retry: Some(2),
        }],
    };

    let yaml = pipeline.to_yaml().unwrap();
    assert!(yaml.contains("yaml-test"));
    assert!(yaml.contains("deploy"));
}

#[tokio::test]
async fn test_pipeline_trigger_event_creation() {
    let pipeline_id = PipelineId::new();
    let repo_id = RepoId::new();

    let event = PipelineTriggerEvent::new(
        pipeline_id,
        repo_id,
        "def456".to_string(),
        TriggerType::Tag,
    );

    assert_eq!(event.pipeline_id, pipeline_id);
    assert_eq!(event.repo_id, repo_id);
    assert_eq!(event.commit_hash, "def456");
    assert_eq!(event.trigger_type, TriggerType::Tag);
    assert!(event.ref_name.is_none());
    assert!(event.actor_id.is_none());
}

#[tokio::test]
async fn test_pipeline_trigger_event_with_options() {
    let event = PipelineTriggerEvent::new(
        PipelineId::new(),
        RepoId::new(),
        "abc123".to_string(),
        TriggerType::Push,
    )
    .with_ref("refs/heads/main".to_string())
    .with_actor(UserId::new());

    assert!(event.ref_name.is_some());
    assert_eq!(event.ref_name.unwrap(), "refs/heads/main");
    assert!(event.actor_id.is_some());
}

#[test]
fn test_trigger_type_serialization() {
    assert_eq!(TriggerType::Push.as_str(), "push");
    assert_eq!(TriggerType::Tag.as_str(), "tag");
    assert_eq!(TriggerType::PullRequest.as_str(), "pull_request");
    assert_eq!(TriggerType::Manual.as_str(), "manual");
}

#[test]
fn test_job_definition_has_dependencies() {
    let job_no_deps = JobDefinition {
        name: "build".to_string(),
        image: "rust:latest".to_string(),
        needs: vec![],
        env: HashMap::new(),
        steps: vec![],
        timeout: None,
        retry: None,
    };
    assert!(!job_no_deps.has_dependencies());

    let job_with_deps = JobDefinition {
        name: "test".to_string(),
        image: "rust:latest".to_string(),
        needs: vec!["build".to_string()],
        env: HashMap::new(),
        steps: vec![],
        timeout: None,
        retry: None,
    };
    assert!(job_with_deps.has_dependencies());
}

#[test]
fn test_step_definition_env() {
    let step_without_env = StepDefinition {
        name: "build".to_string(),
        run: "cargo build".to_string(),
        env: None,
        working_directory: None,
        condition: None,
    };
    assert!(step_without_env.get_env().is_empty());

    let mut custom_env = HashMap::new();
    custom_env.insert("CI".to_string(), "true".to_string());

    let step_with_env = StepDefinition {
        name: "build".to_string(),
        run: "cargo build".to_string(),
        env: Some(custom_env),
        working_directory: None,
        condition: None,
    };
    assert_eq!(step_with_env.get_env().get("CI"), Some(&"true".to_string()));
}

/// End-to-end test: Create pipeline, run through CI engine, complete jobs
#[tokio::test]
async fn test_end_to_end_pipeline_execution() {
    // 1. Create pipeline definition
    let pipeline_def = PipelineDefinition {
        name: "e2e-pipeline".to_string(),
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
    };

    // 2. Create trigger event
    let event = PipelineTriggerEvent::new(
        PipelineId::new(),
        RepoId::new(),
        "abc123".to_string(),
        TriggerType::Push,
    );

    // 3. Create CI engine
    let engine = CiEngine::new(event, pipeline_def).await.unwrap();

    // 4. Start pipeline
    engine.start().await.unwrap();
    {
        let state = engine.state().await;
        assert_eq!(state.status, PipelineStatus::Running);
    }

    // 5. Get ready jobs (build should be ready)
    let ready = engine.ready_jobs().await;
    assert_eq!(ready.len(), 1);

    // 6. Simulate build job execution
    let build_job = ready[0];
    let runner_id = RunnerId::new();

    engine.assign_job(build_job, runner_id).await.unwrap();
    engine.start_job(build_job).await.unwrap();
    engine.succeed_job(build_job, 0).await.unwrap();

    // 7. Verify build is done
    let job_state = engine.get_job(build_job).await.unwrap();
    assert_eq!(job_state.status(), JobStatus::Succeeded);

    // 8. Test job should now be ready (but engine doesn't auto-transition)
    // In real system, scheduler would pick it up
}

/// Test concurrent job execution simulation
#[tokio::test]
async fn test_concurrent_jobs_in_pipeline() {
    let pipeline_def = PipelineDefinition {
        name: "concurrent-pipeline".to_string(),
        version: "1.0".to_string(),
        trigger_on: vec![TriggerType::Push],
        environment: HashMap::new(),
        jobs: vec![
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
                name: "format".to_string(),
                image: "rust:latest".to_string(),
                needs: vec![],
                env: HashMap::new(),
                steps: vec![StepDefinition {
                    name: "format".to_string(),
                    run: "cargo fmt".to_string(),
                    env: None,
                    working_directory: None,
                    condition: None,
                }],
                timeout: None,
                retry: None,
            },
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
        ],
    };

    let event = PipelineTriggerEvent::new(
        PipelineId::new(),
        RepoId::new(),
        "abc123".to_string(),
        TriggerType::Push,
    );

    let engine = CiEngine::new(event, pipeline_def).await.unwrap();
    engine.start().await.unwrap();

    // All three jobs should be ready (no dependencies between them)
    let ready = engine.ready_jobs().await;
    assert_eq!(ready.len(), 3);

    // Simulate running all three concurrently
    let runner1 = RunnerId::new();
    let runner2 = RunnerId::new();
    let runner3 = RunnerId::new();

    let lint_job = ready.iter().find(|j| engine.graph().get(**j).unwrap().name == "lint").copied().unwrap();
    let format_job = ready.iter().find(|j| engine.graph().get(**j).unwrap().name == "format").copied().unwrap();
    let build_job = ready.iter().find(|j| engine.graph().get(**j).unwrap().name == "build").copied().unwrap();

    // Assign and start all
    engine.assign_job(lint_job, runner1).await.unwrap();
    engine.assign_job(format_job, runner2).await.unwrap();
    engine.assign_job(build_job, runner3).await.unwrap();

    engine.start_job(lint_job).await.unwrap();
    engine.start_job(format_job).await.unwrap();
    engine.start_job(build_job).await.unwrap();

    // Complete them
    engine.succeed_job(lint_job, 0).await.unwrap();
    engine.succeed_job(format_job, 0).await.unwrap();
    engine.succeed_job(build_job, 0).await.unwrap();

    // All should be succeeded
    assert_eq!(engine.get_job(lint_job).await.unwrap().status(), JobStatus::Succeeded);
    assert_eq!(engine.get_job(format_job).await.unwrap().status(), JobStatus::Succeeded);
    assert_eq!(engine.get_job(build_job).await.unwrap().status(), JobStatus::Succeeded);
}
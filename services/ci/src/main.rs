//! GitForce CI Orchestrator
//!
//! Main entry point for the CI orchestration service.

use futures::StreamExt;
use gitforce_ci::{
    CiEngine, JobDefinition, PipelineDefinition, PipelineTriggerEvent, StepDefinition, TriggerType,
};
use gitforce_events::{
    EventBus, EventEnvelope, EventFilter, EventPayload, EventType, InMemoryEventBus,
};
use gitforce_scheduler::Scheduler;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::signal;
use tokio::time::timeout;

type PipelineCache = HashMap<gitforce_common::RepoId, PipelineDefinition>;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .init();

    tracing::info!("starting GitForce CI Orchestrator");

    // Initialize event bus
    let event_bus: Arc<dyn EventBus> = Arc::new(InMemoryEventBus::new());

    // Initialize scheduler
    let scheduler = Arc::new(Scheduler::new());

    // Pipeline definitions cache
    let pipeline_cache: Arc<std::sync::Mutex<PipelineCache>> =
        Arc::new(std::sync::Mutex::new(HashMap::new()));

    // Clone for event consumer
    let event_bus_clone = event_bus.clone();
    let scheduler_clone = scheduler.clone();
    let pipeline_cache_clone = pipeline_cache.clone();

    // Shared shutdown flag
    let shutdown = create_shutdown_flag();
    let shutdown_flag = shutdown.clone();

    // Spawn graceful shutdown handler
    spawn_shutdown_handler(shutdown_flag);

    // Start event consumer loop
    let shutdown_consumer = shutdown.clone();
    let _consumer_handle = tokio::spawn(async move {
        if let Err(e) = run_event_consumer(
            event_bus_clone,
            scheduler_clone,
            pipeline_cache_clone,
            shutdown_consumer,
        )
        .await
        {
            tracing::error!("event consumer error: {}", e);
        }
    });

    // Start scheduler loop
    let scheduler_clone = scheduler.clone();
    let shutdown_scheduler = shutdown.clone();
    let _scheduler_handle = tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(5));
        loop {
            if shutdown_scheduler.load(Ordering::SeqCst) {
                tracing::info!("scheduler loop shutting down");
                break;
            }
            ticker.tick().await;
            scheduler_clone.process_queue().await;
        }
    });

    tracing::info!("CI Orchestrator initialized successfully");

    // Wait for shutdown signal
    let shutdown_future = create_shutdown_future(shutdown.clone());

    // Wait for either shutdown or tasks to complete
    timeout(Duration::MAX, shutdown_future).await.ok();

    tracing::info!("shutting down CI Orchestrator");

    // Wait for in-flight work to complete (with timeout)
    graceful_shutdown_delay().await;

    tracing::info!("CI Orchestrator stopped");
    Ok(())
}

/// Create a shutdown flag
pub fn create_shutdown_flag() -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(false))
}

/// Spawn the shutdown signal handler (Unix-only)
#[cfg(unix)]
pub fn spawn_shutdown_handler(shutdown_flag: Arc<AtomicBool>) {
    tokio::spawn(async move {
        let mut sigterm = signal::unix::signal(signal::unix::SignalKind::terminate()).unwrap();
        let mut sigint = signal::unix::signal(signal::unix::SignalKind::interrupt()).unwrap();

        tokio::select! {
            _ = sigterm.recv() => {
                tracing::info!("received SIGTERM, initiating graceful shutdown...");
            }
            _ = sigint.recv() => {
                tracing::info!("received SIGINT, initiating graceful shutdown...");
            }
        }
        shutdown_flag.store(true, Ordering::SeqCst);
    });
}

/// Spawn the shutdown signal handler (Windows stub)
#[cfg(windows)]
pub fn spawn_shutdown_handler(_shutdown_flag: Arc<AtomicBool>) {}

/// Create the shutdown future that waits for shutdown signal
pub async fn create_shutdown_future(shutdown: Arc<AtomicBool>) {
    while !shutdown.load(Ordering::SeqCst) {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Perform graceful shutdown delay
pub async fn graceful_shutdown_delay() {
    timeout(Duration::from_secs(5), async {
        tokio::time::sleep(Duration::from_secs(1)).await;
    })
    .await
    .ok();
}

/// Run the event consumer loop
async fn run_event_consumer(
    event_bus: Arc<dyn EventBus>,
    scheduler: Arc<Scheduler>,
    pipeline_cache: Arc<std::sync::Mutex<PipelineCache>>,
    shutdown: Arc<AtomicBool>,
) -> anyhow::Result<()> {
    tracing::info!("starting event consumer loop");

    // Subscribe to push events
    let filter = EventFilter::for_types(vec![EventType::PushReceived]);
    let mut stream = event_bus.subscribe(filter).await?;

    loop {
        if shutdown.load(Ordering::SeqCst) {
            tracing::info!("event consumer loop shutting down");
            break;
        }

        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(100)) => {
                if shutdown.load(Ordering::SeqCst) {
                    break;
                }
            }
            event = stream.next() => {
                match event {
                    Some(event) => {
                        tracing::debug!("received event: {:?}", event.event_type);
                        if let Err(e) = handle_push_event(&event, &scheduler, &pipeline_cache).await {
                            tracing::error!("failed to handle push event: {}", e);
                        }
                    }
                    None => {
                        tracing::info!("event stream closed");
                        break;
                    }
                }
            }
        }
    }

    Ok(())
}

/// Handle a push received event - trigger pipeline if configured
async fn handle_push_event(
    event: &EventEnvelope,
    scheduler: &Arc<Scheduler>,
    pipeline_cache: &Arc<std::sync::Mutex<PipelineCache>>,
) -> anyhow::Result<()> {
    // Only handle PushReceived events
    let EventPayload::PushReceived(payload) = &event.payload else {
        return Ok(());
    };

    let repo_id = payload.repo_id;
    let ref_name = &payload.ref_name;

    tracing::info!(
        "push received for repo {} on ref {} ({} -> {})",
        repo_id,
        ref_name,
        payload.old_hash,
        payload.new_hash
    );

    // Get or create pipeline definition for this repo
    let pipeline = {
        let mut cache = pipeline_cache.lock().unwrap();
        cache
            .entry(repo_id)
            .or_insert_with(|| create_default_pipeline(&repo_id.to_string()))
            .clone()
    };

    // Create trigger event
    let trigger_event = PipelineTriggerEvent::new(
        gitforce_common::PipelineId::new(),
        repo_id,
        payload.new_hash.clone(),
        TriggerType::Push,
    )
    .with_ref(ref_name.clone());

    // Create and start the CI engine
    let engine = CiEngine::new(trigger_event, pipeline).await?;
    engine.start().await?;

    tracing::info!(
        "pipeline triggered for repo {} on ref {}",
        repo_id,
        ref_name
    );

    // Enqueue ready jobs to scheduler
    let ready_jobs = engine.ready_jobs().await;
    tracing::info!("enqueueing {} ready jobs", ready_jobs.len());

    let state = engine.state().await;
    for job_id in ready_jobs {
        if let Some(_job_state) = state.jobs.get(&job_id) {
            scheduler.enqueue(job_id, state.run_id, repo_id).await;
            tracing::debug!("enqueued job {} for pipeline run {}", job_id, state.run_id);
        }
    }

    Ok(())
}

/// Create a default pipeline definition
fn create_default_pipeline(repo_id: &str) -> PipelineDefinition {
    PipelineDefinition {
        name: format!("{}-pipeline", repo_id),
        version: "1.0".to_string(),
        trigger_on: vec![TriggerType::Push],
        environment: HashMap::new(),
        jobs: vec![
            JobDefinition {
                name: "build".to_string(),
                image: "rust:latest".to_string(),
                needs: vec![],
                env: HashMap::new(),
                steps: vec![
                    StepDefinition {
                        name: "setup".to_string(),
                        run: "cargo fetch".to_string(),
                        env: None,
                        working_directory: None,
                        condition: None,
                    },
                    StepDefinition {
                        name: "build".to_string(),
                        run: "cargo build --release".to_string(),
                        env: None,
                        working_directory: None,
                        condition: None,
                    },
                ],
                timeout: Some("30m".to_string()),
                retry: Some(1),
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
                timeout: Some("30m".to_string()),
                retry: Some(1),
            },
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_default_pipeline() {
        let pipeline = create_default_pipeline("test-repo");

        assert_eq!(pipeline.name, "test-repo-pipeline");
        assert_eq!(pipeline.version, "1.0");
        assert_eq!(pipeline.trigger_on, vec![TriggerType::Push]);
        assert!(pipeline.environment.is_empty());
        assert_eq!(pipeline.jobs.len(), 2);
    }

    #[test]
    fn test_create_default_pipeline_has_build_job() {
        let pipeline = create_default_pipeline("my-repo");

        let build_job = pipeline.jobs.iter().find(|j| j.name == "build").unwrap();
        assert_eq!(build_job.image, "rust:latest");
        assert!(build_job.needs.is_empty());
        assert_eq!(build_job.steps.len(), 2);
    }

    #[test]
    fn test_create_default_pipeline_has_test_job() {
        let pipeline = create_default_pipeline("my-repo");

        let test_job = pipeline.jobs.iter().find(|j| j.name == "test").unwrap();
        assert_eq!(test_job.image, "rust:latest");
        assert_eq!(test_job.needs, vec!["build".to_string()]);
        assert!(test_job.retry.is_some());
    }

    #[test]
    fn test_create_default_pipeline_build_steps() {
        let pipeline = create_default_pipeline("my-repo");

        let build_job = pipeline.jobs.iter().find(|j| j.name == "build").unwrap();
        let step_names: Vec<&str> = build_job.steps.iter().map(|s| s.name.as_str()).collect();
        assert!(step_names.contains(&"setup"));
        assert!(step_names.contains(&"build"));
    }

    #[test]
    fn test_create_default_pipeline_test_depends_on_build() {
        let pipeline = create_default_pipeline("my-repo");

        let test_job = pipeline.jobs.iter().find(|j| j.name == "test").unwrap();
        assert!(test_job.needs.contains(&"build".to_string()));
    }

    #[test]
    fn test_create_default_pipeline_timeout() {
        let pipeline = create_default_pipeline("my-repo");

        for job in &pipeline.jobs {
            assert!(job.timeout.is_some());
            assert_eq!(job.timeout.as_ref().unwrap(), "30m");
        }
    }

    #[test]
    fn test_create_default_pipeline_retry() {
        let pipeline = create_default_pipeline("my-repo");

        for job in &pipeline.jobs {
            assert!(job.retry.is_some());
            assert_eq!(job.retry.unwrap(), 1);
        }
    }

    #[test]
    fn test_create_shutdown_flag_initial_state() {
        let flag = create_shutdown_flag();
        assert!(!flag.load(Ordering::SeqCst));
    }

    #[test]
    fn test_create_shutdown_flag_clone() {
        let flag1 = create_shutdown_flag();
        let flag2 = flag1.clone();
        flag1.store(true, Ordering::SeqCst);
        assert!(flag2.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn test_create_shutdown_future() {
        let shutdown = create_shutdown_flag();
        let shutdown_flag = shutdown.clone();

        // Set shutdown after a short delay
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            shutdown_flag.store(true, Ordering::SeqCst);
        });

        create_shutdown_future(shutdown).await;
    }

    #[test]
    fn test_graceful_shutdown_delay_does_not_panic() {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                graceful_shutdown_delay().await;
            });
    }

    #[test]
    fn test_create_shutdown_flag_cloneable() {
        let flag = create_shutdown_flag();
        let _cloned = flag.clone();
        // Verify the flag can be cloned and used
        assert!(!flag.load(Ordering::SeqCst));
    }

    #[test]
    fn test_pipeline_cache_type_alias() {
        // Verify the PipelineCache type works correctly
        let cache: PipelineCache = HashMap::new();
        assert!(cache.is_empty());
    }

    #[tokio::test]
    async fn test_spawn_shutdown_handler_does_not_panic() {
        let flag = create_shutdown_flag();
        // Just verify the function doesn't panic when called
        spawn_shutdown_handler(flag);
    }
}

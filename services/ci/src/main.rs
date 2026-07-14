//! GitForce CI Orchestrator
//!
//! Main entry point for the CI orchestration service.

use futures::StreamExt;
use gitforce_ci::{CiEngine, PipelineDefinition, PipelineTriggerEvent, TriggerType, JobDefinition, StepDefinition};
use gitforce_events::{
    EventBus, EventEnvelope, EventFilter, EventType, EventPayload, InMemoryEventBus,
    PushReceivedPayload,
};
use gitforce_scheduler::Scheduler;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::signal;
use tokio::time::{interval, Duration};

/// Pipeline definitions cache (in production, this would be from database)
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

    // Start event consumer loop
    tokio::spawn(async move {
        if let Err(e) = run_event_consumer(event_bus_clone, scheduler_clone, pipeline_cache_clone).await {
            tracing::error!("event consumer error: {}", e);
        }
    });

    // Start scheduler loop
    let scheduler_clone = scheduler.clone();
    tokio::spawn(async move {
        let mut ticker = interval(Duration::from_secs(5));
        loop {
            ticker.tick().await;
            scheduler_clone.process_queue().await;
        }
    });

    tracing::info!("CI Orchestrator initialized successfully");

    // Wait for shutdown signal
    signal::ctrl_c().await?;

    tracing::info!("shutting down CI Orchestrator");
    Ok(())
}

/// Run the event consumer loop
async fn run_event_consumer(
    event_bus: Arc<dyn EventBus>,
    scheduler: Arc<Scheduler>,
    pipeline_cache: Arc<std::sync::Mutex<PipelineCache>>,
) -> anyhow::Result<()> {
    tracing::info!("starting event consumer loop");

    // Subscribe to push events
    let filter = EventFilter::for_types(vec![EventType::PushReceived]);
    let mut stream = event_bus.subscribe(filter).await?;

    // Pin the stream for async iteration
    tokio::pin!(stream);

    loop {
        match stream.next().await {
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
        cache.entry(repo_id).or_insert_with(|| {
            // Default pipeline for demo purposes - in production, load from DB
            create_default_pipeline(&repo_id.to_string())
        }).clone()
    };

    // Create trigger event
    let trigger_event = PipelineTriggerEvent::new(
        gitforce_common::PipelineId::new(),
        repo_id,
        payload.new_hash.clone(),
        TriggerType::Push,
    ).with_ref(ref_name.clone());

    // Create and start the CI engine
    let engine = CiEngine::new(trigger_event, pipeline).await?;
    engine.start().await?;

    tracing::info!("pipeline triggered for repo {} on ref {}", repo_id, ref_name);

    // Enqueue ready jobs to scheduler
    let ready_jobs = engine.ready_jobs().await;
    tracing::info!("enqueueing {} ready jobs", ready_jobs.len());

    let state = engine.state().await;
    for job_id in ready_jobs {
        if let Some(_job_state) = state.jobs.get(&job_id) {
            // Enqueue job to scheduler
            scheduler
                .enqueue(
                    job_id,
                    state.run_id,
                    repo_id,
                )
                .await;
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
                steps: vec![
                    StepDefinition {
                        name: "test".to_string(),
                        run: "cargo test".to_string(),
                        env: None,
                        working_directory: None,
                        condition: None,
                    },
                ],
                timeout: Some("30m".to_string()),
                retry: Some(1),
            },
        ],
    }
}

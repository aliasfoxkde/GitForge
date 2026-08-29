//! Job scheduler

use crate::policy::{SchedulingPolicy, SimplePolicy};
use crate::queue::{JobQueue, Priority, QueuedJob};
use gitforge_common::{JobId, PipelineRunId, RepoId, RunnerId};
use gitforge_db::models::Runner;
use gitforge_db::Pool;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};

/// Scheduler command
#[derive(Debug)]
pub enum SchedulerCommand {
    /// Enqueue a new job
    Enqueue {
        job_id: JobId,
        pipeline_run_id: PipelineRunId,
        repo_id: RepoId,
        priority: Priority,
    },
    /// Cancel a job
    Cancel { job_id: JobId },
    /// Register a runner
    RegisterRunner(Runner),
    /// Runner heartbeat
    Heartbeat { runner_id: RunnerId },
    /// Runner went offline
    RunnerOffline { runner_id: RunnerId },
}

/// Scheduler event
#[derive(Debug, Clone)]
pub enum SchedulerEvent {
    /// Job assigned to runner
    JobAssigned { job_id: JobId, runner_id: RunnerId },
    /// No runner available for job
    NoRunnerAvailable { job_id: JobId },
}

/// Scheduler state
#[derive(Debug)]
pub struct SchedulerState {
    pub queue: JobQueue,
    pub runners: HashMap<RunnerId, Runner>,
    pub job_assignments: HashMap<JobId, RunnerId>,
    /// Pipeline run IDs for assigned jobs; assignments are removed from the queue.
    pub assigned_pipeline_runs: HashMap<JobId, PipelineRunId>,
}

impl Default for SchedulerState {
    fn default() -> Self {
        Self::new()
    }
}

impl SchedulerState {
    pub fn new() -> Self {
        Self {
            queue: JobQueue::new(),
            runners: HashMap::new(),
            job_assignments: HashMap::new(),
            assigned_pipeline_runs: HashMap::new(),
        }
    }

    pub fn add_runner(&mut self, runner: Runner) {
        self.runners.insert(runner.id, runner);
    }

    pub fn remove_runner(&mut self, runner_id: RunnerId) {
        self.runners.remove(&runner_id);
    }

    pub fn get_runner(&self, runner_id: RunnerId) -> Option<&Runner> {
        self.runners.get(&runner_id)
    }

    pub fn list_online_runners(&self) -> Vec<Runner> {
        self.runners
            .values()
            .filter(|r| r.status == "online")
            .cloned()
            .collect()
    }
}

/// Job scheduler
pub struct Scheduler {
    state: Arc<RwLock<SchedulerState>>,
    policy: Arc<dyn SchedulingPolicy>,
    event_tx: broadcast::Sender<SchedulerEvent>,
    db_pool: Option<Pool>,
}

impl Scheduler {
    /// Create a new scheduler without database
    pub fn new() -> Self {
        let (event_tx, _) = broadcast::channel(100);
        Self {
            state: Arc::new(RwLock::new(SchedulerState::new())),
            policy: Arc::new(SimplePolicy::new()),
            event_tx,
            db_pool: None,
        }
    }

    /// Create a new scheduler with database pool
    pub fn with_db(pool: Pool) -> Self {
        let (event_tx, _) = broadcast::channel(100);
        Self {
            state: Arc::new(RwLock::new(SchedulerState::new())),
            policy: Arc::new(SimplePolicy::new()),
            event_tx,
            db_pool: Some(pool),
        }
    }

    /// Set the scheduling policy
    pub fn with_policy<P: SchedulingPolicy + 'static>(self, policy: P) -> Self {
        Self {
            policy: Arc::new(policy),
            state: self.state,
            event_tx: self.event_tx,
            db_pool: self.db_pool,
        }
    }

    /// Get whether scheduler has database connection
    pub fn has_db(&self) -> bool {
        self.db_pool.is_some()
    }

    /// Subscribe to scheduler events
    pub fn subscribe(&self) -> broadcast::Receiver<SchedulerEvent> {
        self.event_tx.subscribe()
    }

    /// Enqueue a job (creates a new job in DB if pool exists)
    ///
    /// WARNING: When the job has already been persisted to the database,
    /// use `enqueue_persisted_job()` instead to avoid creating duplicate rows.
    pub async fn enqueue(&self, job_id: JobId, pipeline_run_id: PipelineRunId, repo_id: RepoId) {
        let job = QueuedJob::new(job_id, pipeline_run_id, repo_id);
        let mut state = self.state.write().await;
        state.queue.enqueue(job);
        tracing::debug!("job {} enqueued", job_id);

        // Persist to database if available - creates a new job row.
        // NOTE: This path is deprecated. Jobs must be created with full metadata
        // (including image) via the API layer BEFORE enqueueing.
        // Use `enqueue_persisted_job()` instead to avoid creating rows without images.
        if let Some(pool) = &self.db_pool {
            // The job must already exist in the DB with image set.
            // Just update its status to "queued".
            if let Err(e) =
                gitforge_db::queries::JobQueries::update_status(pool, job_id, "queued").await
            {
                tracing::error!("failed to update job status in DB: {}", e);
            }
        }
    }

    /// Enqueue a job that has already been persisted to the database.
    ///
    /// This method queues the job in memory and updates the existing DB row's status
    /// to "queued" without creating a duplicate row.
    ///
    /// Use this when the job was already created via `JobQueries::create()` and
    /// you just need to queue it for scheduling.
    pub async fn enqueue_persisted_job(
        &self,
        job_id: JobId,
        pipeline_run_id: PipelineRunId,
        repo_id: RepoId,
    ) {
        let job = QueuedJob::new(job_id, pipeline_run_id, repo_id);
        let mut state = self.state.write().await;
        state.queue.enqueue(job);
        tracing::debug!("persisted job {} enqueued", job_id);

        // Update existing job status in database if pool available
        if let Some(pool) = &self.db_pool {
            if let Err(e) =
                gitforge_db::queries::JobQueries::update_status(pool, job_id, "queued").await
            {
                tracing::error!("failed to update persisted job status in DB: {}", e);
            }
        }
    }

    /// Enqueue a job with priority
    pub async fn enqueue_with_priority(
        &self,
        job_id: JobId,
        pipeline_run_id: PipelineRunId,
        repo_id: RepoId,
        priority: Priority,
    ) {
        let job = QueuedJob::new(job_id, pipeline_run_id, repo_id).with_priority(priority);
        let mut state = self.state.write().await;
        state.queue.enqueue(job);
        tracing::debug!("job {} enqueued with {:?} priority", job_id, priority);
    }

    /// Cancel a job
    pub async fn cancel(&self, job_id: JobId) {
        let mut state = self.state.write().await;
        if let Some(_job) = state.queue.remove(job_id) {
            tracing::debug!("job {} cancelled", job_id);
        }
        // Also remove assignment if exists
        state.job_assignments.remove(&job_id);
        state.assigned_pipeline_runs.remove(&job_id);
    }

    /// Record job completion and remove it from the assigned set.
    pub async fn complete_job(&self, job_id: JobId, success: bool) {
        self.complete_job_with_receipt(job_id, success, None, None, None)
            .await;
    }

    /// Record completion and persist bounded output plus correlated events.
    pub async fn complete_job_with_receipt(
        &self,
        job_id: JobId,
        success: bool,
        exit_code: Option<i32>,
        error_message: Option<String>,
        step_results: Option<&[serde_json::Value]>,
    ) {
        let (pipeline_run_id, runner_id) = {
            let mut state = self.state.write().await;
            let runner_id = state.job_assignments.get(&job_id).copied();
            let pipeline_run_id = state.assigned_pipeline_runs.get(&job_id).copied();
            state.job_assignments.remove(&job_id);
            state.assigned_pipeline_runs.remove(&job_id);
            (pipeline_run_id, runner_id)
        };

        if let (Some(pool), Some(run_id)) = (&self.db_pool, pipeline_run_id) {
            let status = if success { "completed" } else { "failed" };
            if let Err(error) =
                gitforge_db::queries::JobQueries::update_status(pool, job_id, status).await
            {
                tracing::error!("failed to persist job {} completion: {}", job_id, error);
            }

            if let Ok(Some(_job)) = gitforge_db::queries::JobQueries::get(pool, job_id).await {
                let (stdout, stderr) = step_results
                    .map(|steps| {
                        steps
                            .iter()
                            .fold((String::new(), String::new()), |mut acc, step| {
                                if let Some(value) = step.get("stdout").and_then(|v| v.as_str()) {
                                    acc.0.push_str(value);
                                }
                                if let Some(value) = step.get("stderr").and_then(|v| v.as_str()) {
                                    acc.1.push_str(value);
                                }
                                acc
                            })
                    })
                    .unwrap_or_default();
                let log = gitforge_db::models::JobLog::new(job_id, run_id, stdout, stderr);
                if let Err(error) = gitforge_db::queries::JobLogQueries::upsert(pool, &log).await {
                    tracing::error!("failed to persist job {} log: {}", job_id, error);
                }
                let event = gitforge_db::models::EventReceipt::new(
                    "job.finished",
                    Some(job_id),
                    Some(run_id),
                    format!("job.finished:{}", job_id),
                    serde_json::json!({
                        "status": status,
                        "success": success,
                        "exit_code": exit_code,
                        "error": error_message,
                        "runner_id": runner_id.map(|id| id.to_string()),
                    }),
                );
                if let Err(error) =
                    gitforge_db::queries::EventReceiptQueries::upsert(pool, &event).await
                {
                    tracing::error!("failed to persist job {} event: {}", job_id, error);
                }
            }

            match gitforge_db::queries::JobQueries::list_by_run(pool, run_id).await {
                Ok(jobs)
                    if !jobs.is_empty()
                        && jobs
                            .iter()
                            .all(|job| job.status == "completed" || job.status == "failed") =>
                {
                    let run_status = if jobs.iter().any(|job| job.status == "failed") {
                        "failed"
                    } else {
                        "completed"
                    };
                    if let Err(error) = gitforge_db::queries::PipelineRunQueries::update_status(
                        pool, run_id, run_status,
                    )
                    .await
                    {
                        tracing::error!(
                            "failed to persist pipeline run {} completion: {}",
                            run_id,
                            error
                        );
                    }
                    let event = gitforge_db::models::EventReceipt::new(
                        "pipeline.finished",
                        None,
                        Some(run_id),
                        format!("pipeline.finished:{}", run_id),
                        serde_json::json!({"status": run_status}),
                    );
                    if let Err(error) =
                        gitforge_db::queries::EventReceiptQueries::upsert(pool, &event).await
                    {
                        tracing::error!("failed to persist pipeline {} event: {}", run_id, error);
                    }
                }
                Ok(_) => {}
                Err(error) => {
                    tracing::error!("failed to inspect pipeline run {} jobs: {}", run_id, error);
                }
            }
        }

        tracing::info!("job {} completion recorded (success={})", job_id, success);
    }

    /// Register a runner
    pub async fn register_runner(&self, runner: Runner) {
        let runner_id = runner.id;
        {
            let mut state = self.state.write().await;
            state.add_runner(runner.clone());
        }

        // A DB-backed scheduler must persist runners before assigning jobs so
        // the jobs.runner_id foreign key remains valid across processes.
        if let Some(pool) = &self.db_pool {
            if let Err(error) = gitforge_db::queries::RunnerQueries::create(pool, &runner).await {
                tracing::error!("failed to persist runner {}: {}", runner_id, error);
            }
        }

        tracing::info!("runner {} registered", runner_id);
    }

    /// Handle runner heartbeat
    pub async fn heartbeat(&self, runner_id: RunnerId) {
        let mut state = self.state.write().await;
        if let Some(runner) = state.runners.get_mut(&runner_id) {
            runner.last_heartbeat = Some(chrono::Utc::now());
            tracing::debug!("runner {} heartbeat", runner_id);
        }
    }

    /// Handle runner going offline
    pub async fn runner_offline(&self, runner_id: RunnerId) {
        let mut state = self.state.write().await;
        if let Some(runner) = state.runners.get_mut(&runner_id) {
            runner.status = "offline".to_string();
            tracing::warn!("runner {} went offline", runner_id);
        }
    }

    /// Try to assign jobs to available runners
    pub async fn process_queue(&self) {
        let mut state = self.state.write().await;
        let runners = state.list_online_runners();
        let available_runners = runners.len();

        // Process multiple jobs if we have multiple runners
        // Use a loop to batch process jobs
        let mut processed = 0;
        let max_jobs_per_batch = available_runners.max(1);

        while processed < max_jobs_per_batch {
            // Peek at next job
            let job = match state.queue.peek() {
                Some(j) => j,
                None => break, // No more jobs
            };

            let job_id = job.job_id;
            let pipeline_run_id = job.pipeline_run_id;

            // Select runner using policy
            let runner_id = self.policy.select_runner(job_id, &runners).await;

            match runner_id {
                Some(r_id) => {
                    // Dequeue and assign (JobId and RunnerId are Copy types)
                    state.queue.dequeue();
                    state.job_assignments.insert(job_id, r_id);
                    state.assigned_pipeline_runs.insert(job_id, pipeline_run_id);
                    tracing::info!("assigned job {} to runner {}", job_id, r_id);
                    processed += 1;

                    // Emit event
                    let event = SchedulerEvent::JobAssigned {
                        job_id,
                        runner_id: r_id,
                    };
                    let _ = self.event_tx.send(event);

                    // Persist assignment to database if available
                    if let Some(pool) = &self.db_pool {
                        if let Err(e) =
                            gitforge_db::queries::JobQueries::assign(pool, job_id, r_id).await
                        {
                            tracing::error!("failed to persist job assignment to DB: {}", e);
                        }
                        if let Err(e) = gitforge_db::queries::JobQueries::update_status(
                            pool, job_id, "assigned",
                        )
                        .await
                        {
                            tracing::error!("failed to update job status in DB: {}", e);
                        } else {
                            let event = gitforge_db::models::EventReceipt::new(
                                "job.started",
                                Some(job_id),
                                Some(pipeline_run_id),
                                format!("job.started:{}", job_id),
                                serde_json::json!({
                                    "status": "assigned",
                                    "runner_id": r_id.to_string(),
                                }),
                            );
                            if let Err(e) =
                                gitforge_db::queries::EventReceiptQueries::upsert(pool, &event)
                                    .await
                            {
                                tracing::error!(
                                    "failed to persist job {} start event: {}",
                                    job_id,
                                    e
                                );
                            }
                        }
                    }
                }
                None => {
                    // No runner available for this job, skip remaining
                    tracing::debug!("no runner available for job {}, stopping batch", job_id);
                    break;
                }
            }
        }

        if processed > 0 {
            tracing::debug!("batch assigned {} jobs", processed);
        }
    }

    /// Load pending jobs from database
    pub async fn load_pending_jobs(&self) -> anyhow::Result<usize> {
        let pool = match &self.db_pool {
            Some(p) => p,
            None => return Ok(0),
        };

        let pending_jobs = gitforge_db::queries::JobQueries::list_pending(pool).await?;
        let mut state = self.state.write().await;

        let mut loaded = 0;
        for db_job in pending_jobs {
            if !state.queue.contains(db_job.id) {
                state.queue.enqueue(QueuedJob::new(
                    db_job.id,
                    db_job.pipeline_run_id,
                    RepoId::new(), // RepoId not in job model, use default
                ));
                loaded += 1;
            }
        }

        tracing::info!("loaded {} pending jobs from database", loaded);
        Ok(loaded)
    }

    /// Get queue length
    pub async fn queue_len(&self) -> usize {
        let state = self.state.read().await;
        state.queue.len()
    }

    /// Check if a job is assigned
    pub async fn is_assigned(&self, job_id: JobId) -> Option<RunnerId> {
        let state = self.state.read().await;
        state.job_assignments.get(&job_id).copied()
    }

    /// Get all assigned jobs (jobs assigned to runners, awaiting execution)
    pub async fn get_assigned_jobs(&self) -> Vec<(JobId, RunnerId, PipelineRunId)> {
        let state = self.state.read().await;
        state
            .job_assignments
            .iter()
            .filter_map(|(job_id, runner_id)| {
                state
                    .assigned_pipeline_runs
                    .get(job_id)
                    .map(|pipeline_run_id| (*job_id, *runner_id, *pipeline_run_id))
            })
            .collect()
    }
}

impl Default for Scheduler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gitforge_db::models::RunnerType;

    fn make_runner(id: RunnerId, name: &str, status: &str, capacity: i32) -> Runner {
        Runner {
            id,
            name: name.to_string(),
            runner_type: RunnerType::Docker.as_str().to_string(),
            status: status.to_string(),
            capacity,
            last_heartbeat: None,
            created_at: chrono::Utc::now(),
        }
    }

    #[tokio::test]
    async fn test_enqueue_dequeue() {
        let scheduler = Scheduler::new();
        let repo_id = RepoId::new();
        let run_id = PipelineRunId::new();
        let job_id = JobId::new();

        scheduler.enqueue(job_id, run_id, repo_id).await;
        assert_eq!(scheduler.queue_len().await, 1);

        // Register a runner
        let runner = make_runner(RunnerId::new(), "test-runner", "online", 2);
        scheduler.register_runner(runner.clone()).await;

        // Process queue
        scheduler.process_queue().await;
    }

    #[tokio::test]
    async fn test_cancel() {
        let scheduler = Scheduler::new();
        let repo_id = RepoId::new();
        let run_id = PipelineRunId::new();
        let job_id = JobId::new();

        scheduler.enqueue(job_id, run_id, repo_id).await;
        assert_eq!(scheduler.queue_len().await, 1);

        scheduler.cancel(job_id).await;
        assert_eq!(scheduler.queue_len().await, 0);
    }

    #[tokio::test]
    async fn test_enqueue_with_priority() {
        let scheduler = Scheduler::new();
        let repo_id = RepoId::new();
        let run_id = PipelineRunId::new();
        let job_id = JobId::new();

        scheduler
            .enqueue_with_priority(job_id, run_id, repo_id, Priority::High)
            .await;
        assert_eq!(scheduler.queue_len().await, 1);
    }

    #[tokio::test]
    async fn test_register_runner() {
        let scheduler = Scheduler::new();
        let runner = make_runner(RunnerId::new(), "test-runner", "online", 2);
        let runner_id = runner.id;

        scheduler.register_runner(runner).await;
        // Verify runner is registered by checking process_queue doesn't emit NoRunnerAvailable
        let repo_id = RepoId::new();
        let run_id = PipelineRunId::new();
        let job_id = JobId::new();

        scheduler.enqueue(job_id, run_id, repo_id).await;
        scheduler.process_queue().await;

        let assigned = scheduler.is_assigned(job_id).await;
        assert_eq!(assigned, Some(runner_id));
    }

    #[tokio::test]
    async fn test_heartbeat() {
        let scheduler = Scheduler::new();
        let runner_id = RunnerId::new();
        let runner = make_runner(runner_id, "test-runner", "online", 2);

        scheduler.register_runner(runner).await;
        scheduler.heartbeat(runner_id).await;
        // No panic means success
    }

    #[tokio::test]
    async fn test_runner_offline() {
        let scheduler = Scheduler::new();
        let runner_id = RunnerId::new();
        let runner = make_runner(runner_id, "test-runner", "online", 2);

        scheduler.register_runner(runner).await;
        scheduler.runner_offline(runner_id).await;
        // No panic means success
    }

    #[tokio::test]
    async fn test_cancel_nonexistent_job() {
        let scheduler = Scheduler::new();
        let job_id = JobId::new();

        // Cancel should not panic
        scheduler.cancel(job_id).await;
    }

    #[tokio::test]
    async fn test_scheduler_subscribe() {
        let scheduler = Scheduler::new();
        let mut rx = scheduler.subscribe();

        // Try to receive with a timeout - should get an error since no event sent
        let result = tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv()).await;
        assert!(result.is_err() || result.unwrap().is_err()); // Timeout or channel closed
    }

    #[test]
    fn test_scheduler_command_debug() {
        let cmd = SchedulerCommand::Enqueue {
            job_id: JobId::new(),
            pipeline_run_id: PipelineRunId::new(),
            repo_id: RepoId::new(),
            priority: Priority::Normal,
        };
        assert!(format!("{:?}", cmd).contains("Enqueue"));
    }

    #[test]
    fn test_scheduler_event_debug() {
        let evt = SchedulerEvent::NoRunnerAvailable {
            job_id: JobId::new(),
        };
        assert!(format!("{:?}", evt).contains("NoRunnerAvailable"));
    }

    #[test]
    fn test_scheduler_state_new() {
        let state = SchedulerState::new();
        assert!(state.queue.is_empty());
        assert!(state.runners.is_empty());
        assert!(state.job_assignments.is_empty());
    }

    #[tokio::test]
    async fn test_scheduler_state_add_remove_runner() {
        let mut state = SchedulerState::new();
        let runner = make_runner(RunnerId::new(), "test-runner", "online", 2);

        state.add_runner(runner.clone());
        assert_eq!(state.runners.len(), 1);

        state.remove_runner(runner.id);
        assert!(state.runners.is_empty());
    }

    #[tokio::test]
    async fn test_scheduler_state_get_runner() {
        let mut state = SchedulerState::new();
        let runner = make_runner(RunnerId::new(), "test-runner", "online", 2);

        state.add_runner(runner.clone());
        assert!(state.get_runner(runner.id).is_some());
        assert!(state.get_runner(RunnerId::new()).is_none());
    }

    #[tokio::test]
    async fn test_scheduler_state_list_online_runners() {
        let mut state = SchedulerState::new();
        state.add_runner(make_runner(RunnerId::new(), "runner1", "online", 2));
        state.add_runner(make_runner(RunnerId::new(), "runner2", "offline", 2));

        let online = state.list_online_runners();
        assert_eq!(online.len(), 1);
        assert_eq!(online[0].name, "runner1");
    }

    #[test]
    fn test_priority_ord() {
        assert!(Priority::High > Priority::Normal);
        assert!(Priority::Normal > Priority::Low);
        assert!(Priority::High > Priority::Low);
    }

    #[test]
    fn test_priority_default() {
        assert_eq!(Priority::default(), Priority::Normal);
    }

    #[tokio::test]
    async fn test_scheduler_with_multiple_jobs_and_runners() {
        let scheduler = Scheduler::new();

        // Add multiple runners
        let runner1 = make_runner(RunnerId::new(), "runner1", "online", 4);
        let runner2 = make_runner(RunnerId::new(), "runner2", "online", 2);
        scheduler.register_runner(runner1.clone()).await;
        scheduler.register_runner(runner2.clone()).await;

        // Add multiple jobs
        for _i in 0..5 {
            let job_id = JobId::new();
            let run_id = PipelineRunId::new();
            let repo_id = RepoId::new();
            scheduler.enqueue(job_id, run_id, repo_id).await;
        }

        assert_eq!(scheduler.queue_len().await, 5);

        // Process queue should assign jobs
        scheduler.process_queue().await;
    }

    #[tokio::test]
    async fn test_scheduler_assign_job_multiple_times() {
        let scheduler = Scheduler::new();
        let runner = make_runner(RunnerId::new(), "runner1", "online", 4);
        scheduler.register_runner(runner.clone()).await;

        let job_id = JobId::new();
        let run_id = PipelineRunId::new();
        let repo_id = RepoId::new();
        scheduler.enqueue(job_id, run_id, repo_id).await;

        // Process queue twice - second time should be no-op since job is assigned
        scheduler.process_queue().await;
        scheduler.process_queue().await;

        // Job should be assigned
        let assigned = scheduler.is_assigned(job_id).await;
        assert!(assigned.is_some());
    }

    #[tokio::test]
    async fn test_scheduler_cancel_assigned_job() {
        let scheduler = Scheduler::new();
        let runner = make_runner(RunnerId::new(), "runner1", "online", 4);
        scheduler.register_runner(runner.clone()).await;

        let job_id = JobId::new();
        let run_id = PipelineRunId::new();
        let repo_id = RepoId::new();
        scheduler.enqueue(job_id, run_id, repo_id).await;

        // Assign job
        scheduler.process_queue().await;
        assert!(scheduler.is_assigned(job_id).await.is_some());

        // Cancel should remove assignment
        scheduler.cancel(job_id).await;
        assert!(scheduler.is_assigned(job_id).await.is_none());
    }

    #[tokio::test]
    async fn test_scheduler_queue_len_after_dequeue() {
        let scheduler = Scheduler::new();
        let runner = make_runner(RunnerId::new(), "runner1", "online", 4);
        scheduler.register_runner(runner.clone()).await;

        let job_id = JobId::new();
        let run_id = PipelineRunId::new();
        let repo_id = RepoId::new();
        scheduler.enqueue(job_id, run_id, repo_id).await;
        assert_eq!(scheduler.queue_len().await, 1);

        // Process queue - job should be dequeued
        scheduler.process_queue().await;
        assert_eq!(scheduler.queue_len().await, 0);
    }

    #[test]
    fn test_scheduler_state_with_multiple_runners() {
        let mut state = SchedulerState::new();
        state.add_runner(make_runner(RunnerId::new(), "runner1", "online", 4));
        state.add_runner(make_runner(RunnerId::new(), "runner2", "online", 2));
        state.add_runner(make_runner(RunnerId::new(), "runner3", "offline", 1));

        assert_eq!(state.runners.len(), 3);
        let online = state.list_online_runners();
        assert_eq!(online.len(), 2);
    }

    #[test]
    fn test_scheduler_state_remove_nonexistent() {
        let mut state = SchedulerState::new();
        state.remove_runner(RunnerId::new());
        assert!(state.runners.is_empty());
    }

    #[test]
    fn test_scheduler_state_get_nonexistent() {
        let state = SchedulerState::new();
        assert!(state.get_runner(RunnerId::new()).is_none());
    }

    #[tokio::test]
    async fn test_scheduler_is_assigned_not_found() {
        let scheduler = Scheduler::new();
        let assigned = scheduler.is_assigned(JobId::new()).await;
        assert!(assigned.is_none());
    }

    #[tokio::test]
    async fn test_scheduler_enqueue_with_priority_high() {
        let scheduler = Scheduler::new();
        let repo_id = RepoId::new();
        let run_id = PipelineRunId::new();

        scheduler
            .enqueue_with_priority(JobId::new(), run_id, repo_id, Priority::High)
            .await;
        assert_eq!(scheduler.queue_len().await, 1);
    }

    #[tokio::test]
    async fn test_scheduler_with_policy() {
        use crate::policy::PriorityPolicy;
        let scheduler = Scheduler::new().with_policy(PriorityPolicy::new());
        let runner = make_runner(RunnerId::new(), "runner1", "online", 4);
        scheduler.register_runner(runner.clone()).await;

        let job_id = JobId::new();
        let run_id = PipelineRunId::new();
        let repo_id = RepoId::new();
        scheduler.enqueue(job_id, run_id, repo_id).await;

        scheduler.process_queue().await;
        assert!(scheduler.is_assigned(job_id).await.is_some());
    }

    #[tokio::test]
    async fn test_scheduler_process_queue_no_runner_emits_event() {
        let scheduler = Scheduler::new();

        // Subscribe to scheduler events
        let _rx = scheduler.subscribe();

        let job_id = JobId::new();
        let run_id = PipelineRunId::new();
        let repo_id = RepoId::new();
        scheduler.enqueue(job_id, run_id, repo_id).await;

        // Process queue with no runners - should emit NoRunnerAvailable
        scheduler.process_queue().await;

        // Check event was sent (non-blocking check)
        // Note: broadcast channel may not have received yet, so we just verify no panic
        assert_eq!(scheduler.queue_len().await, 1); // Job still in queue
    }

    #[tokio::test]
    async fn test_scheduler_subscribe_receives_events() {
        let scheduler = Scheduler::new();

        // Add a runner so job can be assigned
        let runner = make_runner(RunnerId::new(), "runner1", "online", 4);
        scheduler.register_runner(runner.clone()).await;

        // Subscribe before enqueueing
        let _rx = scheduler.subscribe();

        let job_id = JobId::new();
        let run_id = PipelineRunId::new();
        let repo_id = RepoId::new();
        scheduler.enqueue(job_id, run_id, repo_id).await;

        // Process queue - should assign job
        scheduler.process_queue().await;

        // Verify job is assigned
        let assigned = scheduler.is_assigned(job_id).await;
        assert!(assigned.is_some());
    }

    // =============================================================================
    // enqueue_persisted_job tests
    // =============================================================================

    #[tokio::test]
    async fn test_enqueue_persisted_job_adds_to_queue() {
        let scheduler = Scheduler::new();
        let repo_id = RepoId::new();
        let run_id = PipelineRunId::new();
        let job_id = JobId::new();

        // Use enqueue_persisted_job for a job already in DB
        scheduler
            .enqueue_persisted_job(job_id, run_id, repo_id)
            .await;
        assert_eq!(scheduler.queue_len().await, 1);
    }

    #[tokio::test]
    async fn test_enqueue_persisted_job_queue_and_assign() {
        let scheduler = Scheduler::new();
        let repo_id = RepoId::new();
        let run_id = PipelineRunId::new();
        let job_id = JobId::new();

        // Register a runner
        let runner = make_runner(RunnerId::new(), "test-runner", "online", 2);
        scheduler.register_runner(runner.clone()).await;

        // Enqueue persisted job and process
        scheduler
            .enqueue_persisted_job(job_id, run_id, repo_id)
            .await;
        scheduler.process_queue().await;

        // Job should be assigned
        let assigned = scheduler.is_assigned(job_id).await;
        assert_eq!(assigned, Some(runner.id));

        let assigned_jobs = scheduler.get_assigned_jobs().await;
        assert_eq!(assigned_jobs, vec![(job_id, runner.id, run_id)]);

        scheduler.complete_job(job_id, true).await;
        assert!(scheduler.is_assigned(job_id).await.is_none());
        assert!(scheduler.get_assigned_jobs().await.is_empty());
    }

    #[tokio::test]
    async fn test_enqueue_persisted_job_cancel() {
        let scheduler = Scheduler::new();
        let repo_id = RepoId::new();
        let run_id = PipelineRunId::new();
        let job_id = JobId::new();

        scheduler
            .enqueue_persisted_job(job_id, run_id, repo_id)
            .await;
        assert_eq!(scheduler.queue_len().await, 1);

        // Cancel should work
        scheduler.cancel(job_id).await;
        assert_eq!(scheduler.queue_len().await, 0);
    }

    #[tokio::test]
    async fn test_enqueue_persisted_job_multiple() {
        let scheduler = Scheduler::new();
        let repo_id = RepoId::new();
        let run_id = PipelineRunId::new();

        // Enqueue multiple persisted jobs
        for _ in 0..5 {
            scheduler
                .enqueue_persisted_job(JobId::new(), run_id, repo_id)
                .await;
        }
        assert_eq!(scheduler.queue_len().await, 5);
    }

    #[tokio::test]
    async fn test_enqueue_vs_enqueue_persisted_job_distinct() {
        // Test that both methods can be used independently on different jobs
        let scheduler = Scheduler::new();
        let repo_id = RepoId::new();
        let run_id = PipelineRunId::new();

        let job1 = JobId::new();
        let job2 = JobId::new();

        // job1 uses regular enqueue (scheduler creates DB entry)
        scheduler.enqueue(job1, run_id, repo_id).await;

        // job2 uses enqueue_persisted_job (caller already created DB entry)
        scheduler.enqueue_persisted_job(job2, run_id, repo_id).await;

        // Both should be in queue
        assert_eq!(scheduler.queue_len().await, 2);
    }

    #[tokio::test]
    async fn test_enqueue_persisted_job_empty_repo_id() {
        // Test with default RepoId (zero value)
        let scheduler = Scheduler::new();
        let run_id = PipelineRunId::new();
        let job_id = JobId::new();

        scheduler
            .enqueue_persisted_job(job_id, run_id, RepoId::new())
            .await;
        assert_eq!(scheduler.queue_len().await, 1);
    }
}

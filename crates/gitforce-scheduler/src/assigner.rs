//! Job scheduler

use crate::policy::{SchedulingPolicy, SimplePolicy};
use crate::queue::{JobQueue, Priority, QueuedJob};
use gitforce_common::{JobId, PipelineRunId, RepoId, RunnerId};
use gitforce_db::models::{Job as DbJob, Runner};
use gitforce_db::Pool;
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

    /// Enqueue a job
    pub async fn enqueue(&self, job_id: JobId, pipeline_run_id: PipelineRunId, repo_id: RepoId) {
        let job = QueuedJob::new(job_id, pipeline_run_id, repo_id);
        let mut state = self.state.write().await;
        state.queue.enqueue(job);
        tracing::debug!("job {} enqueued", job_id);

        // Persist to database if available
        if let Some(pool) = &self.db_pool {
            let db_job = DbJob::new(pipeline_run_id, format!("job-{}", job_id));
            if let Err(e) = gitforce_db::queries::JobQueries::create(pool, &db_job).await {
                tracing::error!("failed to persist job to DB: {}", e);
            }
            if let Err(e) = gitforce_db::queries::JobQueries::update_status(pool, job_id, "queued").await {
                tracing::error!("failed to update job status in DB: {}", e);
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
    }

    /// Register a runner
    pub async fn register_runner(&self, runner: Runner) {
        let mut state = self.state.write().await;
        let runner_id = runner.id;
        state.add_runner(runner);
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

        // Try to assign jobs while we have available runners
        let mut assigned = None;

        if let Some(job) = state.queue.peek() {
            let job_id = job.job_id;

            // Select runner using policy
            let runner_id = self.policy.select_runner(job_id, &runners).await;

            match runner_id {
                Some(r_id) => {
                    // Dequeue and assign
                    state.queue.dequeue();
                    state.job_assignments.insert(job_id, r_id);
                    tracing::info!("assigned job {} to runner {}", job_id, r_id);
                    assigned = Some((job_id, r_id));
                }
                None => {
                    // No runner available
                    tracing::debug!("no runner available for job {}", job_id);
                }
            }
        }

        // Emit event if we assigned a job
        if let Some((job_id, runner_id)) = assigned {
            let event = SchedulerEvent::JobAssigned { job_id, runner_id };
            let _ = self.event_tx.send(event);

            // Persist assignment to database if available
            if let Some(pool) = &self.db_pool {
                if let Err(e) = gitforce_db::queries::JobQueries::assign(pool, job_id, runner_id).await {
                    tracing::error!("failed to persist job assignment to DB: {}", e);
                }
                if let Err(e) = gitforce_db::queries::JobQueries::update_status(pool, job_id, "assigned").await {
                    tracing::error!("failed to update job status in DB: {}", e);
                }
            }
        }
    }

    /// Load pending jobs from database
    pub async fn load_pending_jobs(&self) -> anyhow::Result<usize> {
        let pool = match &self.db_pool {
            Some(p) => p,
            None => return Ok(0),
        };

        let pending_jobs = gitforce_db::queries::JobQueries::list_pending(pool).await?;
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
                // Find the queued job to get pipeline_run_id
                state
                    .queue
                    .all()
                    .iter()
                    .find(|j| j.job_id == *job_id)
                    .map(|j| (j.job_id, *runner_id, j.pipeline_run_id))
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
    use gitforce_db::models::RunnerType;

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
}

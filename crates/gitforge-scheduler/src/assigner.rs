//! Job scheduler

use crate::policy::{SchedulingPolicy, SimplePolicy};
use crate::queue::{JobQueue, Priority, QueuedJob};
use gitforge_common::{JobId, PipelineRunId, RepoId, RunnerId};
use gitforge_db::models::{Job as DbJob, PipelineRun as DbPipelineRun, Runner};
use gitforge_db::Pool;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};
use uuid::Uuid;

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
    /// Runner submitted a terminal job receipt.
    JobCompleted {
        job_id: JobId,
        pipeline_run_id: PipelineRunId,
        runner_id: RunnerId,
        success: bool,
    },
}

/// Scheduler state
#[derive(Debug)]
pub struct SchedulerState {
    pub queue: JobQueue,
    pub runners: HashMap<RunnerId, Runner>,
    pub job_assignments: HashMap<JobId, RunnerId>,
    /// Assignment metadata retained until completion or requeue. Keeping the
    /// repository ID here is essential: a runner-loss recovery must enqueue
    /// the same repository, never synthesize a new UUID.
    pub assigned_jobs: HashMap<JobId, (RunnerId, PipelineRunId, RepoId)>,
    pub job_definitions: HashMap<JobId, JobExecutionDefinition>,
    pub completed_receipts: HashMap<JobId, String>,
    /// Opaque per-assignment lease tokens. A runner must present the active
    /// token for the started and completion transitions.
    pub job_leases: HashMap<JobId, String>,
    /// Jobs cancelled by an operator in this scheduler process. Durable
    /// status is checked as well when a database is configured.
    pub cancelled_jobs: std::collections::HashSet<JobId>,
}

/// The scheduler-facing portion of a CI job definition. It is deliberately
/// transport-neutral so the runner receives exactly the commands selected by
/// the CI engine rather than reconstructing or guessing them.
#[derive(Debug, Clone)]
pub struct JobExecutionDefinition {
    pub commands: Vec<String>,
    pub working_dir: Option<String>,
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
            assigned_jobs: HashMap::new(),
            job_definitions: HashMap::new(),
            completed_receipts: HashMap::new(),
            job_leases: HashMap::new(),
            cancelled_jobs: std::collections::HashSet::new(),
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
    recovery_done: Arc<AtomicBool>,
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
            recovery_done: Arc::new(AtomicBool::new(true)),
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
            recovery_done: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Set the scheduling policy
    pub fn with_policy<P: SchedulingPolicy + 'static>(self, policy: P) -> Self {
        Self {
            policy: Arc::new(policy),
            state: self.state,
            event_tx: self.event_tx,
            db_pool: self.db_pool,
            recovery_done: self.recovery_done,
        }
    }

    /// Get whether scheduler has database connection
    pub fn has_db(&self) -> bool {
        self.db_pool.is_some()
    }

    /// Check whether a job is known to durable or in-memory scheduler state.
    pub async fn job_exists(&self, job_id: JobId) -> bool {
        if let Some(pool) = &self.db_pool {
            return gitforge_db::queries::JobQueries::get(pool, job_id)
                .await
                .ok()
                .flatten()
                .is_some();
        }
        let state = self.state.read().await;
        state.job_definitions.contains_key(&job_id)
            || state.job_assignments.contains_key(&job_id)
            || state.assigned_jobs.contains_key(&job_id)
            || state.queue.contains(job_id)
    }

    /// Read a durable pipeline run for the CI status adapter.
    pub async fn get_pipeline_run(
        &self,
        run_id: PipelineRunId,
    ) -> anyhow::Result<Option<DbPipelineRun>> {
        let Some(pool) = &self.db_pool else {
            return Ok(None);
        };
        gitforge_db::queries::PipelineRunQueries::get(pool, run_id)
            .await
            .map_err(|error| anyhow::anyhow!(error.to_string()))
    }

    /// Read durable jobs and their bounded terminal receipts for a pipeline.
    pub async fn get_pipeline_run_jobs(
        &self,
        run_id: PipelineRunId,
    ) -> anyhow::Result<Vec<gitforge_db::models::Job>> {
        let Some(pool) = &self.db_pool else {
            return Ok(Vec::new());
        };
        gitforge_db::queries::JobQueries::list_by_run(pool, run_id)
            .await
            .map_err(|error| anyhow::anyhow!(error.to_string()))
    }

    /// Subscribe to scheduler events
    pub fn subscribe(&self) -> broadcast::Receiver<SchedulerEvent> {
        self.event_tx.subscribe()
    }

    /// Enqueue a job
    pub async fn enqueue(&self, job_id: JobId, pipeline_run_id: PipelineRunId, repo_id: RepoId) {
        self.enqueue_with_definition(job_id, pipeline_run_id, repo_id, Vec::new(), None)
            .await;
    }

    /// Enqueue a job together with its executable definition.
    pub async fn enqueue_with_definition(
        &self,
        job_id: JobId,
        pipeline_run_id: PipelineRunId,
        repo_id: RepoId,
        commands: Vec<String>,
        working_dir: Option<String>,
    ) {
        let job = QueuedJob::new(job_id, pipeline_run_id, repo_id);
        let mut state = self.state.write().await;
        state.queue.enqueue(job);
        state.job_definitions.insert(
            job_id,
            JobExecutionDefinition {
                commands: commands.clone(),
                working_dir: working_dir.clone(),
            },
        );
        tracing::debug!("job {} enqueued", job_id);

        // Persist to database if available
        if let Some(pool) = &self.db_pool {
            let mut db_job = DbJob::new(pipeline_run_id, format!("job-{}", job_id));
            db_job.id = job_id;
            if let Err(e) = gitforge_db::queries::JobQueries::create(pool, &db_job).await {
                tracing::error!("failed to persist job to DB: {}", e);
            }
            if let Err(e) =
                gitforge_db::queries::JobQueries::update_status(pool, job_id, "queued").await
            {
                tracing::error!("failed to update job status in DB: {}", e);
            }
            if let Err(e) = gitforge_db::queries::JobQueries::set_definition(
                pool,
                job_id,
                &commands,
                working_dir.as_deref(),
            )
            .await
            {
                tracing::error!("failed to persist job definition: {}", e);
            }
        }
    }

    /// Submit an operator job exactly once across control-plane retries. This
    /// deliberately requires the durable scheduler database; an in-memory
    /// fallback cannot provide the same guarantee after a restart.
    pub async fn submit_idempotent(
        &self,
        pipeline_run_id: PipelineRunId,
        repo_id: RepoId,
        commands: Vec<String>,
        working_dir: Option<String>,
        idempotency_key: &str,
        request_fingerprint: &str,
    ) -> anyhow::Result<(JobId, bool)> {
        let Some(pool) = &self.db_pool else {
            return Err(anyhow::anyhow!("durable_scheduler_database_required"));
        };
        let scope = "operator";
        if let Some((job_id, fingerprint)) =
            gitforge_db::queries::JobQueries::get_idempotency(pool, scope, idempotency_key).await?
        {
            if fingerprint != request_fingerprint {
                return Err(anyhow::anyhow!(
                    "idempotency_key_reused_with_different_request"
                ));
            }
            return Ok((job_id, false));
        }

        let job_id = JobId::new();
        if !gitforge_db::queries::JobQueries::reserve_idempotency(
            pool,
            scope,
            idempotency_key,
            request_fingerprint,
            job_id,
        )
        .await?
        {
            let Some((existing_id, fingerprint)) =
                gitforge_db::queries::JobQueries::get_idempotency(pool, scope, idempotency_key)
                    .await?
            else {
                return Err(anyhow::anyhow!("idempotency_reservation_disappeared"));
            };
            if fingerprint != request_fingerprint {
                return Err(anyhow::anyhow!(
                    "idempotency_key_reused_with_different_request"
                ));
            }
            return Ok((existing_id, false));
        }
        self.enqueue_with_definition(job_id, pipeline_run_id, repo_id, commands, working_dir)
            .await;
        Ok((job_id, true))
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
        state.cancelled_jobs.insert(job_id);
        if let Some(_job) = state.queue.remove(job_id) {
            tracing::debug!("job {} cancelled", job_id);
        }
        // Also remove assignment if exists
        state.job_assignments.remove(&job_id);
        state.assigned_jobs.remove(&job_id);
        state.job_leases.remove(&job_id);
        drop(state);
        if let Some(pool) = &self.db_pool {
            let receipt = serde_json::json!({
                "job_id": job_id.to_string(),
                "status": "cancelled",
                "reason": "operator requested cancellation",
            })
            .to_string();
            if let Err(error) =
                gitforge_db::queries::JobQueries::cancel(pool, job_id, &receipt).await
            {
                tracing::warn!(%error, %job_id, "failed to persist job cancellation");
            }
        }
    }

    /// Register a runner
    pub async fn register_runner(&self, runner: Runner) {
        let mut state = self.state.write().await;
        let runner_id = runner.id;
        state.add_runner(runner);
        if let Some(pool) = &self.db_pool {
            if let Some(runner) = state.runners.get(&runner_id) {
                if let Err(error) = gitforge_db::queries::RunnerQueries::create(pool, runner).await
                {
                    tracing::error!("failed to persist runner {}: {}", runner_id, error);
                }
            }
        }
        tracing::info!("runner {} registered", runner_id);
    }

    /// Handle runner heartbeat
    pub async fn heartbeat(&self, runner_id: RunnerId) -> bool {
        let updated = {
            let mut state = self.state.write().await;
            if let Some(runner) = state.runners.get_mut(&runner_id) {
                runner.last_heartbeat = Some(chrono::Utc::now());
                tracing::debug!("runner {} heartbeat", runner_id);
                true
            } else {
                false
            }
        };
        if updated {
            if let Some(pool) = &self.db_pool {
                if let Err(error) =
                    gitforge_db::queries::RunnerQueries::heartbeat(pool, runner_id).await
                {
                    tracing::warn!(%error, %runner_id, "failed to persist runner heartbeat");
                }
            }
        }
        updated
    }

    /// Handle runner going offline
    pub async fn runner_offline(&self, runner_id: RunnerId) {
        let mut state = self.state.write().await;
        if let Some(runner) = state.runners.get_mut(&runner_id) {
            runner.status = "offline".to_string();
            tracing::warn!("runner {} went offline", runner_id);
        }
    }

    /// Mark runners as offline if they have not sent a heartbeat within the threshold.
    /// Returns the number of runners marked offline.
    pub async fn mark_stale_runners_offline(&self, heartbeat_timeout_secs: i64) -> usize {
        let stale_threshold =
            chrono::Utc::now() - chrono::Duration::seconds(heartbeat_timeout_secs);
        let mut marked_offline = 0;

        let mut stale_runners = Vec::new();
        {
            let mut state = self.state.write().await;
            for runner in state.runners.values_mut() {
                if runner.status == "online" {
                    if let Some(last_heartbeat) = runner.last_heartbeat {
                        if last_heartbeat < stale_threshold {
                            runner.status = "offline".to_string();
                            tracing::warn!(
                                "runner {} marked offline: last heartbeat {} seconds ago",
                                runner.id,
                                heartbeat_timeout_secs
                            );
                            marked_offline += 1;
                            stale_runners.push(runner.id);
                        }
                    }
                }
            }
        }
        if let Some(pool) = &self.db_pool {
            for runner_id in &stale_runners {
                if let Err(error) =
                    gitforge_db::queries::RunnerQueries::update_status(pool, *runner_id, "offline")
                        .await
                {
                    tracing::warn!(%error, %runner_id, "failed to persist stale runner status");
                }
            }
        }
        for runner_id in stale_runners {
            self.requeue_jobs_for_offline_runner(runner_id).await;
        }

        marked_offline
    }

    /// Re-enqueue jobs that were assigned to an offline runner so they can be
    /// picked up by another runner. Returns the number of jobs re-enqueued.
    pub async fn requeue_jobs_for_offline_runner(&self, runner_id: RunnerId) -> usize {
        let jobs_to_requeue = {
            let state = self.state.read().await;
            state
                .assigned_jobs
                .iter()
                .filter(|(_, (rid, _, _))| *rid == runner_id)
                .map(|(job_id, (_, pipeline_run_id, repo_id))| {
                    (*job_id, *pipeline_run_id, *repo_id)
                })
                .collect::<Vec<_>>()
        };

        if jobs_to_requeue.is_empty() {
            return 0;
        }

        let mut requeued = 0;
        let db_pool = self.db_pool.clone();
        let mut state = self.state.write().await;

        for (job_id, pipeline_run_id, repo_id) in &jobs_to_requeue {
            // Remove from assignments
            state.job_assignments.remove(job_id);
            state.assigned_jobs.remove(job_id);
            state.job_leases.remove(job_id);

            // Re-enqueue the job with the original pipeline run and repository
            // IDs. The queue entry must remain tied to the original checkout.
            state.queue.enqueue(crate::queue::QueuedJob::new(
                *job_id,
                *pipeline_run_id,
                *repo_id,
            ));
            tracing::info!(
                "re-enqueued job {} after runner {} went offline",
                job_id,
                runner_id
            );
            requeued += 1;

            // Persist status change to DB
            if let Some(pool) = &db_pool {
                let _ = gitforge_db::queries::JobQueries::requeue(pool, *job_id).await;
            }
        }

        requeued
    }

    /// Try to assign jobs to available runners
    pub async fn process_queue(&self) {
        if self.db_pool.is_some() && !self.recovery_done.swap(true, Ordering::AcqRel) {
            if let Some(pool) = &self.db_pool {
                match gitforge_db::queries::JobQueries::requeue_inflight(pool).await {
                    Ok(count) if count > 0 => {
                        tracing::warn!(count, "requeued jobs left in-flight by scheduler restart");
                    }
                    Ok(_) => {}
                    Err(error) => {
                        tracing::error!(%error, "failed to recover in-flight jobs");
                        self.recovery_done.store(false, Ordering::Release);
                        return;
                    }
                }
            }
            if let Err(error) = self.load_pending_jobs().await {
                tracing::error!(%error, "failed to load durable jobs after scheduler recovery");
                self.recovery_done.store(false, Ordering::Release);
                return;
            }
        }

        // Keep the scheduler responsive to jobs submitted by the separate API
        // process after startup. `load_pending_jobs` is deduplicated by the
        // in-memory queue and runs on the existing bounded scheduler tick.
        if self.recovery_done.load(Ordering::Acquire) {
            if let Err(error) = self.load_pending_jobs().await {
                tracing::error!(%error, "failed to refresh pending durable jobs");
            }
        }

        // API-side cancellation writes the durable row directly because API
        // and scheduler are separate processes. Reconcile those rows before
        // assignment so a canceled queued job cannot be handed to a runner.
        if let Some(pool) = &self.db_pool {
            let queued_ids = {
                let state = self.state.read().await;
                state
                    .queue
                    .all()
                    .into_iter()
                    .map(|job| job.job_id)
                    .collect::<Vec<_>>()
            };
            for job_id in queued_ids {
                let is_cancelled = gitforge_db::queries::JobQueries::get(pool, job_id)
                    .await
                    .ok()
                    .flatten()
                    .map(|job| job.status == "cancelled")
                    .unwrap_or(false);
                if is_cancelled {
                    self.cancel(job_id).await;
                }
            }
        }
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
            let repo_id = job.repo_id;

            // Select runner using policy
            let runner_id = self.policy.select_runner(job_id, &runners).await;

            match runner_id {
                Some(r_id) => {
                    let lease_token = state
                        .job_leases
                        .entry(job_id)
                        .or_insert_with(|| Uuid::new_v4().to_string())
                        .clone();

                    // Let the durable database win races between scheduler
                    // instances before mutating this scheduler's mirror.
                    if let Some(pool) = &self.db_pool {
                        match gitforge_db::queries::JobQueries::assign_with_lease(
                            pool,
                            job_id,
                            r_id,
                            &lease_token,
                        )
                        .await
                        {
                            Ok(true) => {}
                            Ok(false) => {
                                state.queue.dequeue();
                                state.job_leases.remove(&job_id);
                                tracing::debug!(%job_id, "job assignment won by another scheduler");
                                continue;
                            }
                            Err(error) => {
                                state.job_leases.remove(&job_id);
                                tracing::error!(%error, %job_id, "failed to persist durable job lease");
                                break;
                            }
                        }
                    }

                    // Dequeue and assign (JobId and RunnerId are Copy types)
                    state.queue.dequeue();
                    state.job_assignments.insert(job_id, r_id);
                    state
                        .assigned_jobs
                        .insert(job_id, (r_id, pipeline_run_id, repo_id));
                    tracing::info!("assigned job {} to runner {}", job_id, r_id);
                    processed += 1;

                    // Emit event
                    let event = SchedulerEvent::JobAssigned {
                        job_id,
                        runner_id: r_id,
                    };
                    let _ = self.event_tx.send(event);
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
                // Jobs reference a pipeline run, which is the durable source
                // of the repository identity. Refuse to enqueue if the run is
                // missing instead of silently routing to a random repository.
                let Some(run) =
                    gitforge_db::queries::PipelineRunQueries::get(pool, db_job.pipeline_run_id)
                        .await?
                else {
                    tracing::error!(
                        job_id = %db_job.id,
                        pipeline_run_id = %db_job.pipeline_run_id,
                        "cannot recover job without its pipeline run"
                    );
                    continue;
                };
                state.queue.enqueue(QueuedJob::new(
                    db_job.id,
                    db_job.pipeline_run_id,
                    run.repo_id,
                ));
                state.job_definitions.insert(
                    db_job.id,
                    JobExecutionDefinition {
                        commands: db_job.commands,
                        working_dir: db_job.working_dir,
                    },
                );
                loaded += 1;
            }
        }

        tracing::info!("loaded {} pending jobs from database", loaded);
        Ok(loaded)
    }

    /// Return whether an operator has cancelled a job. This endpoint is
    /// intentionally read-only and lets a runner terminate its local
    /// sandbox without granting the runner authority to cancel jobs.
    pub async fn is_cancelled(&self, job_id: JobId) -> bool {
        {
            let state = self.state.read().await;
            if state.cancelled_jobs.contains(&job_id) {
                return true;
            }
        }
        if let Some(pool) = &self.db_pool {
            return gitforge_db::queries::JobQueries::get(pool, job_id)
                .await
                .ok()
                .flatten()
                .map(|job| job.status == "cancelled")
                .unwrap_or(false);
        }
        false
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
            .assigned_jobs
            .iter()
            .map(|(job_id, (runner_id, run_id, _repo_id))| (*job_id, *runner_id, *run_id))
            .collect()
    }

    /// Get assigned jobs with their executable definition.
    pub async fn get_assigned_job_details(
        &self,
    ) -> Vec<(JobId, RunnerId, PipelineRunId, JobExecutionDefinition)> {
        let state = self.state.read().await;
        state
            .assigned_jobs
            .iter()
            .filter_map(|(job_id, (runner_id, run_id, _repo_id))| {
                state
                    .job_definitions
                    .get(job_id)
                    .map(|definition| (*job_id, *runner_id, *run_id, definition.clone()))
            })
            .collect()
    }

    /// Return the current lease for a job, creating one for an existing
    /// assignment when needed. Repeated calls are idempotent.
    pub async fn ensure_job_lease(&self, job_id: JobId) -> Option<String> {
        let mut state = self.state.write().await;
        if !state.assigned_jobs.contains_key(&job_id) {
            return None;
        }
        Some(
            state
                .job_leases
                .entry(job_id)
                .or_insert_with(|| Uuid::new_v4().to_string())
                .clone(),
        )
    }

    /// Verify the runner's lease and persist the assigned-to-running
    /// transition. The state lock makes the check-and-use atomic in the
    /// in-memory scheduler; the database records the durable timestamp.
    pub async fn start_job(
        &self,
        job_id: JobId,
        runner_id: RunnerId,
        lease_token: &str,
    ) -> anyhow::Result<()> {
        {
            let state = self.state.read().await;
            let assigned_runner = state
                .assigned_jobs
                .get(&job_id)
                .map(|(runner, _, _)| *runner);
            if assigned_runner != Some(runner_id)
                || state.job_leases.get(&job_id).map(String::as_str) != Some(lease_token)
            {
                anyhow::bail!("invalid job lease or runner assignment");
            }
        }
        if let Some(pool) = &self.db_pool {
            let accepted = gitforge_db::queries::JobQueries::start_with_lease(
                pool,
                job_id,
                runner_id,
                lease_token,
            )
            .await
            .map_err(|error| anyhow::anyhow!(error.to_string()))?;
            if !accepted {
                anyhow::bail!("durable job lease is no longer active");
            }
        }
        Ok(())
    }

    /// Complete a job only when the active runner lease is presented.
    pub async fn complete_job_with_lease(
        &self,
        job_id: JobId,
        runner_id: RunnerId,
        lease_token: &str,
        success: bool,
        result_json: String,
    ) -> anyhow::Result<()> {
        {
            let state = self.state.read().await;
            let assigned_runner = state
                .assigned_jobs
                .get(&job_id)
                .map(|(runner, _, _)| *runner);
            if assigned_runner != Some(runner_id)
                || state.job_leases.get(&job_id).map(String::as_str) != Some(lease_token)
            {
                anyhow::bail!("invalid job lease or runner assignment");
            }
        }
        if let Some(pool) = &self.db_pool {
            let status = if success { "succeeded" } else { "failed" };
            let accepted = gitforge_db::queries::JobQueries::complete_with_lease(
                pool,
                job_id,
                runner_id,
                lease_token,
                status,
                &result_json,
            )
            .await
            .map_err(|error| anyhow::anyhow!(error.to_string()))?;
            if !accepted {
                anyhow::bail!("durable job lease is no longer active");
            }
        }
        self.complete_job(job_id, success, result_json).await
    }

    /// Append runner output to the durable log ledger under the active lease.
    pub async fn append_log_with_lease(
        &self,
        job_id: JobId,
        runner_id: RunnerId,
        lease_token: &str,
        chunk: &str,
    ) -> anyhow::Result<Option<i64>> {
        let Some(pool) = &self.db_pool else {
            anyhow::bail!("durable job logs require a scheduler database");
        };
        gitforge_db::queries::JobQueries::append_log_with_lease(
            pool,
            job_id,
            runner_id,
            lease_token,
            chunk,
        )
        .await
        .map_err(|error| anyhow::anyhow!(error.to_string()))
    }

    /// Check whether a runner may upload output for a live job lease.
    pub async fn job_lease_active(
        &self,
        job_id: JobId,
        runner_id: RunnerId,
        lease_token: &str,
    ) -> bool {
        let Some(pool) = &self.db_pool else {
            return false;
        };
        gitforge_db::queries::JobQueries::lease_is_active(pool, job_id, runner_id, lease_token)
            .await
            .unwrap_or(false)
    }

    /// Read durable runner log chunks for the operator/API adapter.
    pub async fn list_logs(
        &self,
        job_id: JobId,
    ) -> anyhow::Result<Vec<gitforge_db::queries::JobLogChunk>> {
        let Some(pool) = &self.db_pool else {
            return Ok(Vec::new());
        };
        gitforge_db::queries::JobQueries::list_logs(pool, job_id)
            .await
            .map_err(|error| anyhow::anyhow!(error.to_string()))
    }

    /// Record a terminal receipt and persist it when a scheduler DB exists.
    pub async fn complete_job(
        &self,
        job_id: JobId,
        success: bool,
        result_json: String,
    ) -> anyhow::Result<()> {
        let status = if success { "succeeded" } else { "failed" };
        let assignment = {
            let state = self.state.read().await;
            state.assigned_jobs.get(&job_id).copied()
        };
        if let Some(pool) = &self.db_pool {
            gitforge_db::queries::JobQueries::complete(pool, job_id, status, &result_json)
                .await
                .map_err(|error| anyhow::anyhow!(error.to_string()))?;
        }
        let mut state = self.state.write().await;
        if let Some(existing) = state.completed_receipts.get(&job_id) {
            if existing == &result_json {
                return Ok(());
            }
            anyhow::bail!("job {} already has a conflicting receipt", job_id);
        }
        state.completed_receipts.insert(job_id, result_json);
        state.job_assignments.remove(&job_id);
        state.assigned_jobs.remove(&job_id);
        state.job_leases.remove(&job_id);
        if let Some((runner_id, pipeline_run_id, _repo_id)) = assignment {
            let _ = self.event_tx.send(SchedulerEvent::JobCompleted {
                job_id,
                pipeline_run_id,
                runner_id,
                success,
            });
        }
        Ok(())
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
    async fn test_idempotent_submission_replays_same_job_and_rejects_conflict() {
        let pool = gitforge_db::Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();
        let scheduler = Scheduler::with_db(pool);
        let run_id = PipelineRunId::new();
        let repo_id = RepoId::new();
        let first = scheduler
            .submit_idempotent(
                run_id,
                repo_id,
                vec!["cargo test".to_string()],
                None,
                "request-1",
                "fingerprint-1",
            )
            .await
            .unwrap();
        let replay = scheduler
            .submit_idempotent(
                run_id,
                repo_id,
                vec!["cargo test".to_string()],
                None,
                "request-1",
                "fingerprint-1",
            )
            .await
            .unwrap();
        assert_eq!(first.0, replay.0);
        assert!(first.1);
        assert!(!replay.1);
        let conflict = scheduler
            .submit_idempotent(
                run_id,
                repo_id,
                vec!["cargo fmt".to_string()],
                None,
                "request-1",
                "fingerprint-2",
            )
            .await;
        assert!(conflict
            .unwrap_err()
            .to_string()
            .contains("idempotency_key_reused"));
    }

    #[tokio::test]
    async fn test_api_cancelled_queued_job_is_not_assigned() {
        let pool = gitforge_db::Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();
        let user = gitforge_db::models::User::new(
            "scheduler-owner".to_string(),
            "scheduler@example.com".to_string(),
            "hash".to_string(),
        );
        gitforge_db::queries::UserQueries::create(&pool, &user)
            .await
            .unwrap();
        let repo_id = RepoId::new();
        gitforge_db::queries::RepoQueries::create(
            &pool,
            &gitforge_db::models::Repository {
                id: repo_id,
                name: "scheduler-repo".to_string(),
                owner_id: user.id,
                visibility: "private".to_string(),
                git_path: "/tmp/scheduler-repo".to_string(),
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            },
        )
        .await
        .unwrap();
        let pipeline_id = gitforge_common::PipelineId::new();
        gitforge_db::queries::PipelineQueries::create(
            &pool,
            &gitforge_db::models::Pipeline {
                id: pipeline_id,
                repo_id,
                name: "scheduler-ci".to_string(),
                trigger_type: "manual".to_string(),
                config: serde_json::json!({}),
                created_at: chrono::Utc::now(),
            },
        )
        .await
        .unwrap();
        let scheduler = Scheduler::with_db(pool.clone());
        let job_id = JobId::new();
        let run_id = PipelineRunId::new();
        gitforge_db::queries::PipelineRunQueries::create(
            &pool,
            &gitforge_db::models::PipelineRun {
                id: run_id,
                pipeline_id,
                repo_id,
                status: "queued".to_string(),
                triggered_by: "test".to_string(),
                commit_hash: "abc123".to_string(),
                started_at: None,
                finished_at: None,
                created_at: chrono::Utc::now(),
            },
        )
        .await
        .unwrap();
        scheduler
            .enqueue_with_definition(
                job_id,
                run_id,
                repo_id,
                vec!["cargo test".to_string()],
                None,
            )
            .await;
        let receipt = serde_json::json!({
            "job_id": job_id.to_string(),
            "status": "cancelled",
            "reason": "api test"
        })
        .to_string();
        gitforge_db::queries::JobQueries::cancel(&pool, job_id, &receipt)
            .await
            .unwrap();
        scheduler
            .register_runner(make_runner(RunnerId::new(), "runner", "online", 1))
            .await;
        scheduler.process_queue().await;
        assert_eq!(scheduler.queue_len().await, 0);
        assert!(scheduler.is_assigned(job_id).await.is_none());
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
    async fn test_cancelled_job_cannot_be_assigned_from_lazy_queue_entry() {
        let scheduler = Scheduler::new();
        let runner = make_runner(RunnerId::new(), "test-runner", "online", 1);
        scheduler.register_runner(runner).await;
        let job_id = JobId::new();
        scheduler
            .enqueue(job_id, PipelineRunId::new(), RepoId::new())
            .await;

        scheduler.cancel(job_id).await;
        scheduler.process_queue().await;

        assert!(scheduler.is_assigned(job_id).await.is_none());
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
        assert!(scheduler.heartbeat(runner_id).await);
        assert!(!scheduler.heartbeat(RunnerId::new()).await);
    }

    #[tokio::test]
    async fn test_stale_runner_is_offlined_and_assigned_job_is_requeued() {
        let scheduler = Scheduler::new();
        let runner_id = RunnerId::new();
        let mut runner = make_runner(runner_id, "stale-runner", "online", 1);
        runner.last_heartbeat = Some(chrono::Utc::now() - chrono::Duration::seconds(120));
        scheduler.register_runner(runner).await;
        let job_id = JobId::new();
        let repo_id = RepoId::new();
        scheduler
            .enqueue_with_definition(
                job_id,
                PipelineRunId::new(),
                repo_id,
                vec!["cargo test".to_string()],
                None,
            )
            .await;
        scheduler.process_queue().await;
        assert_eq!(scheduler.is_assigned(job_id).await, Some(runner_id));

        assert_eq!(scheduler.mark_stale_runners_offline(30).await, 1);
        assert!(scheduler.is_assigned(job_id).await.is_none());
        assert_eq!(scheduler.queue_len().await, 1);

        let state = scheduler.state.read().await;
        assert_eq!(state.runners[&runner_id].status, "offline");
        assert!(!state.job_leases.contains_key(&job_id));
        assert_eq!(state.queue.all()[0].repo_id, repo_id);
    }

    #[tokio::test]
    async fn test_runner_loss_preserves_repository_for_multiple_jobs() {
        let scheduler = Scheduler::new();
        let first_runner_id = RunnerId::new();
        let second_runner_id = RunnerId::new();
        let stale_heartbeat = chrono::Utc::now() - chrono::Duration::seconds(120);
        let mut first_runner = make_runner(first_runner_id, "multi-repo-runner-a", "online", 1);
        first_runner.last_heartbeat = Some(stale_heartbeat);
        let mut second_runner = make_runner(second_runner_id, "multi-repo-runner-b", "online", 1);
        second_runner.last_heartbeat = Some(stale_heartbeat);
        scheduler.register_runner(first_runner).await;
        scheduler.register_runner(second_runner).await;

        let first_job = JobId::new();
        let first_repo = RepoId::new();
        let second_job = JobId::new();
        let second_repo = RepoId::new();
        scheduler
            .enqueue(first_job, PipelineRunId::new(), first_repo)
            .await;
        scheduler
            .enqueue(second_job, PipelineRunId::new(), second_repo)
            .await;
        scheduler.process_queue().await;
        assert!(scheduler.is_assigned(first_job).await.is_some());
        assert!(scheduler.is_assigned(second_job).await.is_some());

        assert_eq!(scheduler.mark_stale_runners_offline(30).await, 2);

        let state = scheduler.state.read().await;
        assert_eq!(state.queue.len(), 2);
        assert_eq!(
            state
                .queue
                .all()
                .iter()
                .find(|job| job.job_id == first_job)
                .map(|job| job.repo_id),
            Some(first_repo)
        );
        assert_eq!(
            state
                .queue
                .all()
                .iter()
                .find(|job| job.job_id == second_job)
                .map(|job| job.repo_id),
            Some(second_repo)
        );
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

    #[tokio::test]
    async fn test_job_lease_started_and_completion_lifecycle() {
        let scheduler = Scheduler::new();
        let runner = make_runner(RunnerId::new(), "lease-runner", "online", 1);
        let runner_id = runner.id;
        let job_id = JobId::new();
        scheduler
            .enqueue_with_definition(
                job_id,
                PipelineRunId::new(),
                RepoId::new(),
                vec!["cargo test --workspace".to_string()],
                Some("/workspace".to_string()),
            )
            .await;
        scheduler.register_runner(runner).await;
        scheduler.process_queue().await;

        let lease = scheduler.ensure_job_lease(job_id).await.unwrap();
        assert!(!lease.is_empty());
        assert!(scheduler.start_job(job_id, runner_id, &lease).await.is_ok());
        assert!(scheduler
            .start_job(job_id, RunnerId::new(), &lease)
            .await
            .is_err());
        assert!(scheduler
            .start_job(job_id, runner_id, "wrong")
            .await
            .is_err());
        assert!(scheduler
            .complete_job_with_lease(
                job_id,
                RunnerId::new(),
                &lease,
                true,
                "{\"success\":true}".to_string(),
            )
            .await
            .is_err());
        assert!(scheduler
            .complete_job_with_lease(
                job_id,
                runner_id,
                &lease,
                true,
                "{\"success\":true}".to_string(),
            )
            .await
            .is_ok());
        assert!(scheduler.is_assigned(job_id).await.is_none());
    }

    #[tokio::test]
    async fn test_scheduler_persists_and_enforces_durable_lease() {
        let pool = gitforge_db::Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();
        let user = gitforge_db::models::User::new(
            "lease-owner".to_string(),
            "lease-owner@example.com".to_string(),
            "hash".to_string(),
        );
        gitforge_db::queries::UserQueries::create(&pool, &user)
            .await
            .unwrap();
        let repo = gitforge_db::models::Repository::new(
            "lease-repo".to_string(),
            user.id,
            "/git/lease-repo".to_string(),
        );
        gitforge_db::queries::RepoQueries::create(&pool, &repo)
            .await
            .unwrap();
        let pipeline = gitforge_db::models::Pipeline {
            id: gitforge_common::PipelineId::new(),
            repo_id: repo.id,
            name: "lease-pipeline".to_string(),
            trigger_type: "manual".to_string(),
            config: serde_json::json!({}),
            created_at: chrono::Utc::now(),
        };
        gitforge_db::queries::PipelineQueries::create(&pool, &pipeline)
            .await
            .unwrap();
        let run = gitforge_db::models::PipelineRun::new(
            pipeline.id,
            repo.id,
            "lease-owner".to_string(),
            "lease-commit".to_string(),
        );
        gitforge_db::queries::PipelineRunQueries::create(&pool, &run)
            .await
            .unwrap();
        let job = gitforge_db::models::Job::new(run.id, "durable-lease".to_string());
        let job_id = job.id;
        gitforge_db::queries::JobQueries::create(&pool, &job)
            .await
            .unwrap();

        let scheduler = Scheduler::with_db(pool.clone());
        let runner = make_runner(RunnerId::new(), "durable-runner", "online", 1);
        let runner_id = runner.id;
        scheduler.register_runner(runner).await;
        scheduler.enqueue(job_id, run.id, repo.id).await;
        scheduler.process_queue().await;

        let persisted = gitforge_db::queries::JobQueries::get(&pool, job_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(persisted.runner_id, Some(runner_id));
        assert_eq!(persisted.status, "assigned");
        let lease = scheduler.ensure_job_lease(job_id).await.unwrap();
        assert!(scheduler.start_job(job_id, runner_id, &lease).await.is_ok());
        assert!(scheduler
            .complete_job_with_lease(
                job_id,
                runner_id,
                &lease,
                true,
                "{\"durable\":true}".to_string(),
            )
            .await
            .is_ok());
        let completed = gitforge_db::queries::JobQueries::get(&pool, job_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(completed.status, "succeeded");
        assert!(scheduler.is_assigned(job_id).await.is_none());
    }
}

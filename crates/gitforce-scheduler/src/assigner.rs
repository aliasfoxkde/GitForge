//! Job scheduler

use crate::policy::{SchedulingPolicy, SimplePolicy};
use crate::queue::{JobQueue, Priority, QueuedJob};
use futures::StreamExt;
use gitforce_common::{JobId, PipelineRunId, RepoId, Result, RunnerId};
use gitforce_db::models::Runner;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, RwLock};

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
    JobAssigned {
        job_id: JobId,
        runner_id: RunnerId,
    },
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
}

impl Scheduler {
    pub fn new() -> Self {
        let (event_tx, _) = broadcast::channel(100);
        Self {
            state: Arc::new(RwLock::new(SchedulerState::new())),
            policy: Arc::new(SimplePolicy::new()),
            event_tx,
        }
    }

    /// Set the scheduling policy
    pub fn with_policy<P: SchedulingPolicy + 'static>(self, policy: P) -> Self {
        Self {
            policy: Arc::new(policy),
            state: self.state,
            event_tx: self.event_tx,
        }
    }

    /// Subscribe to scheduler events
    pub fn subscribe(&self) -> broadcast::Receiver<SchedulerEvent> {
        self.event_tx.subscribe()
    }

    /// Enqueue a job
    pub async fn enqueue(
        &self,
        job_id: JobId,
        pipeline_run_id: PipelineRunId,
        repo_id: RepoId,
    ) {
        let job = QueuedJob::new(job_id, pipeline_run_id, repo_id);
        let mut state = self.state.write().await;
        state.queue.enqueue(job);
        tracing::debug!("job {} enqueued", job_id);
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

        while let Some(job) = state.queue.peek() {
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
                    break; // Process one at a time for simplicity
                }
                None => {
                    // No runner available
                    tracing::debug!("no runner available for job {}", job_id);
                    break;
                }
            }
        }

        // Emit event if we assigned a job
        if let Some((job_id, runner_id)) = assigned {
            let event = SchedulerEvent::JobAssigned { job_id, runner_id };
            let _ = self.event_tx.send(event);
        }
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
}

impl Default for Scheduler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_enqueue_dequeue() {
        let scheduler = Scheduler::new();
        let repo_id = RepoId::new();
        let run_id = PipelineRunId::new();
        let job_id = JobId::new();

        scheduler.enqueue(job_id, run_id, repo_id).await;
        assert_eq!(scheduler.queue_len().await, 1);

        // Register a runner
        let runner = Runner::new("test-runner".to_string(), gitforce_db::models::RunnerType::Docker, 2);
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
}

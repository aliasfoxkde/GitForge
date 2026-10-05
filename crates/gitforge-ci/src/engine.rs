//! CI Engine - orchestrates pipeline execution

use crate::dag::{DagBuilder, JobGraph};
use crate::pipeline::{PipelineDefinition, PipelineTriggerEvent};
use crate::state::JobStateMachine;
use gitforge_common::{JobId, JobStatus, PipelineRunId, PipelineStatus, RepoId, Result};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// CI Engine state
#[derive(Debug, Clone)]
pub struct CiEngineState {
    pub run_id: PipelineRunId,
    pub pipeline_id: gitforge_common::PipelineId,
    pub repo_id: RepoId,
    pub status: PipelineStatus,
    pub jobs: HashMap<JobId, JobStateMachine>,
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    pub finished_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Set by [`CiEngine::cancel`] when the run was cancelled while jobs a
    /// runner owns (`Assigned`/`Running`) were still live. Those jobs are
    /// left to finish, and the run's verdict is `Cancelled` whenever the
    /// last of them settles — regardless of whether it succeeds or fails —
    /// because the operator's cancellation outranks the in-flight outcome.
    pub cancel_requested: bool,
}

impl CiEngineState {
    pub fn new(
        run_id: PipelineRunId,
        pipeline_id: gitforge_common::PipelineId,
        repo_id: RepoId,
    ) -> Self {
        Self {
            run_id,
            pipeline_id,
            repo_id,
            status: PipelineStatus::Pending,
            jobs: HashMap::new(),
            started_at: None,
            finished_at: None,
            cancel_requested: false,
        }
    }

    /// Grade the run once its last job turned terminal.
    ///
    /// Recorded cancellation intent (a run-level cancel request) wins: a
    /// cancelled run stays `Cancelled` even when its remaining runner-owned
    /// job succeeds or fails, because the job's own row keeps its true
    /// outcome while the run's verdict answers "did the pipeline as
    /// requested complete".
    ///
    /// Otherwise the verdict follows the durable graders' precedence (the
    /// scheduler's `finalize_pipeline_if_terminal` and the service's
    /// orphan-run reconciliation use the same order, so a restart
    /// re-derives the identical verdict): a failed, timed-out, or
    /// infrastructure-failed job fails the run — its ancestors' outcome is
    /// the reason the pipeline did not complete, even when a sibling was
    /// cancelled along the way — otherwise any cancelled job cancels the
    /// run (a per-job operator cancellation with every other job green is a
    /// cancelled run, not a failed one), and a run with nothing but
    /// successes succeeded.
    fn settle_if_all_finished(&mut self) {
        if !self.all_jobs_finished() {
            return;
        }
        self.status = if self.cancel_requested {
            PipelineStatus::Cancelled
        } else if self.jobs.values().any(|job| {
            matches!(
                job.status(),
                JobStatus::Failed | JobStatus::TimedOut | JobStatus::InfrastructureFailure
            )
        }) {
            PipelineStatus::Failed
        } else if self
            .jobs
            .values()
            .any(|job| job.status() == JobStatus::Cancelled)
        {
            PipelineStatus::Cancelled
        } else {
            PipelineStatus::Succeeded
        };
        self.finished_at = Some(chrono::Utc::now());
    }

    /// Check if all jobs are finished
    pub fn all_jobs_finished(&self) -> bool {
        self.jobs
            .values()
            .all(super::state::JobStateMachine::is_terminal)
    }

    /// Check if all jobs succeeded
    pub fn all_jobs_succeeded(&self) -> bool {
        self.jobs
            .values()
            .all(|j| j.status() == JobStatus::Succeeded)
    }

    /// Get failed jobs
    pub fn failed_jobs(&self) -> Vec<JobId> {
        self.jobs
            .iter()
            .filter(|(_, j)| j.status() == JobStatus::Failed)
            .map(|(id, _)| *id)
            .collect()
    }

    /// Get pending jobs (not yet started)
    pub fn pending_jobs(&self) -> Vec<JobId> {
        self.jobs
            .iter()
            .filter(|(_, j)| {
                matches!(
                    j.status(),
                    JobStatus::Pending | JobStatus::Queued | JobStatus::Assigned
                )
            })
            .map(|(id, _)| *id)
            .collect()
    }
}

/// The action the orchestrator should apply to an engine job whose
/// scheduler row already reached a terminal state without a completion
/// event arriving — the runner that would have reported the outcome is
/// gone, so nothing else will ever drive the DAG forward.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FenceAction {
    /// The scheduler fenced the job as failed (typically a lost runner).
    Fail,
    /// The scheduler recorded the job as timed out.
    Timeout,
    /// The scheduler cancelled the job.
    Cancel,
    /// The scheduler recorded the job as succeeded while the engine still
    /// considers it running — the engine missed the completion event (a
    /// dropped broadcast delivery). The durable row is only written by a
    /// lease-verified completion, so converging on it advances the DAG
    /// exactly as the lost event would have.
    Succeed,
}

/// Compare the engine's unfinished jobs against the scheduler's terminal job
/// statuses and return the (job, action) pairs required to converge.
///
/// Every non-terminal mirror state is considered — `Running`, but also
/// `Pending`/`Queued`/`Assigned`. A queued or assigned mirror job whose
/// durable row is already terminal has no runner and no completion event
/// coming: the canonical case is an operator cancelling a queued job, where
/// the durable row turns `cancelled` while the engine mirror still shows
/// `Queued`. Restricting the sweep to `Running` mirrors left such runs
/// non-terminal forever, holding the engine and the run workspace open.
/// A job the engine already finished is still excluded: it must not be
/// re-judged from a stale scheduler row.
///
/// Every terminal durable status maps to an action, `succeeded` included:
/// an unmapped direction is a run the watchdog can never settle (a durable
/// `succeeded` under a `Running` mirror previously fell through `_ => None`,
/// leaving the run wedged non-terminal forever — run bccaa1be). Non-terminal
/// durable statuses map to nothing: a queued/assigned row under a
/// queued/assigned mirror is the ordinary pre-dispatch state, and dispatch
/// progress reaches the mirror through the completion event, not this sweep.
pub fn fence_actions(
    state: &CiEngineState,
    db_status: &HashMap<JobId, String>,
) -> Vec<(JobId, FenceAction)> {
    state
        .jobs
        .iter()
        .filter(|(_, job_state)| !job_state.status().is_terminal())
        .filter_map(
            |(job_id, _)| match db_status.get(job_id).map(String::as_str) {
                Some("failed") => Some((*job_id, FenceAction::Fail)),
                // The backend failed this job (R6.3). The engine mirror only
                // speaks success/failure, so converge as Fail — the durable
                // row keeps the infrastructure classification, and leaving
                // it unmapped here would wedge the run non-terminal forever.
                Some("infrastructure_failure") => Some((*job_id, FenceAction::Fail)),
                Some("timed_out" | "timeout" | "timed-out") => {
                    Some((*job_id, FenceAction::Timeout))
                }
                Some("cancelled") => Some((*job_id, FenceAction::Cancel)),
                Some("succeeded") => Some((*job_id, FenceAction::Succeed)),
                _ => None,
            },
        )
        .collect()
}

/// CI Engine
pub struct CiEngine {
    state: Arc<RwLock<CiEngineState>>,
    graph: JobGraph,
}

impl CiEngine {
    /// Create a new CI engine from a trigger event and pipeline definition
    pub async fn new(event: PipelineTriggerEvent, pipeline: PipelineDefinition) -> Result<Self> {
        Self::new_with_run_id(event, pipeline, PipelineRunId::new()).await
    }

    /// Create a new CI engine with an explicit run id.
    ///
    /// The normal trigger path generates a fresh id; the restart-recovery
    /// path passes the durable run's id so completion reports and scheduler
    /// rows keep routing to the rebuilt engine.
    pub async fn new_with_run_id(
        event: PipelineTriggerEvent,
        pipeline: PipelineDefinition,
        run_id: PipelineRunId,
    ) -> Result<Self> {
        // Build DAG from pipeline
        let graph = DagBuilder::build(&pipeline, run_id)?;

        // Create job state machines
        let mut jobs = HashMap::new();
        for node in &graph.nodes {
            jobs.insert(node.id, JobStateMachine::new(node.id));
        }

        let mut state = CiEngineState::new(run_id, event.pipeline_id, event.repo_id);
        state.jobs = jobs;

        Ok(Self {
            state: Arc::new(RwLock::new(state)),
            graph,
        })
    }

    /// Rebuild an engine for a run whose control-plane process died.
    ///
    /// The fresh engine adopts the durable run id and grafts the job rows
    /// that survived the restart onto the DAG: graph node ids are replaced
    /// by the durable ids (matched by job name) and each state machine is
    /// restored to the row's status, so completion events, fencing, and
    /// chain advancement keep working against the rows that already exist.
    ///
    /// `rows` carries one entry per durable job: its id, the job name from
    /// the pipeline definition, and the durable status. Names absent from
    /// `rows` (a pre-durable-planning run that died before enqueueing its
    /// tail) keep fresh ids and start at `Pending`, exactly as a live
    /// engine would hold them.
    pub async fn rebuild(
        run_id: PipelineRunId,
        pipeline_id: gitforge_common::PipelineId,
        repo_id: RepoId,
        pipeline: PipelineDefinition,
        rows: &[(
            JobId,
            String,
            JobStatus,
            Option<chrono::DateTime<chrono::Utc>>,
        )],
    ) -> Result<Self> {
        let mut graph = DagBuilder::build(&pipeline, run_id)?;

        let mut jobs: HashMap<JobId, JobStateMachine> = HashMap::new();
        let by_name: HashMap<&str, (JobId, JobStatus, Option<chrono::DateTime<chrono::Utc>>)> =
            rows.iter()
                .map(|(id, name, status, started)| (name.as_str(), (*id, *status, *started)))
                .collect();

        // Grafting replaces node ids, so every dependency edge that points at
        // a grafted node must be remapped too — readiness checks resolve
        // dependencies through the state map, and a stale pre-graft id there
        // would leave a restored stage permanently unready (observed as a
        // rebuilt engine that never re-enqueued its queued stage).
        let mut remap: HashMap<JobId, JobId> = HashMap::new();
        for node in &graph.nodes {
            if let Some(entry) = by_name.get(node.name.as_str()) {
                remap.insert(node.id, entry.0);
            }
        }

        for node in &mut graph.nodes {
            if let Some(&(durable_id, status, started_at)) = by_name.get(node.name.as_str()) {
                node.id = durable_id;
                let mut machine = JobStateMachine::new(durable_id);
                machine.restore(status, started_at);
                jobs.insert(durable_id, machine);
            } else {
                jobs.insert(node.id, JobStateMachine::new(node.id));
            }
            for dep in &mut node.dependencies {
                if let Some(durable) = remap.get(dep) {
                    *dep = *durable;
                }
            }
        }

        let mut state = CiEngineState::new(run_id, pipeline_id, repo_id);
        // A run that already has rows was started by a previous process; the
        // engine mirror marks it running (the durable run row keeps the true
        // start time).
        state.status = PipelineStatus::Running;
        state.started_at = Some(chrono::Utc::now());
        state.jobs = jobs;

        Ok(Self {
            state: Arc::new(RwLock::new(state)),
            graph,
        })
    }

    /// Every planned (job id, job name) pair in the DAG.
    ///
    /// The trigger path persists one durable `pending` row per pair at
    /// planning time so the full job set survives a control-plane restart
    /// (the F21 failure class: a lost engine used to take its unenqueued
    /// stages with it).
    pub fn planned_jobs(&self) -> Vec<(JobId, String)> {
        self.graph
            .nodes
            .iter()
            .map(|node| (node.id, node.name.clone()))
            .collect()
    }

    /// Get current engine state
    pub async fn state(&self) -> CiEngineState {
        self.state.read().await.clone()
    }

    /// Return the immutable definition that produced a job ID. The scheduler
    /// uses this to carry exact pipeline steps to the runner.
    pub fn job_definition(&self, job_id: JobId) -> Option<crate::pipeline::JobDefinition> {
        self.graph.get(job_id).map(|node| node.definition.clone())
    }

    /// Start the pipeline run
    pub async fn start(&self) -> Result<()> {
        let mut state = self.state.write().await;
        state.status = PipelineStatus::Running;
        state.started_at = Some(chrono::Utc::now());

        // Queue all entry point jobs
        for node in self.graph.entry_points() {
            if let Some(job_state) = state.jobs.get_mut(&node.id) {
                job_state.queue()?;
            }
        }

        tracing::info!(
            "pipeline {} started with {} jobs",
            state.run_id,
            state.jobs.len()
        );

        Ok(())
    }

    /// Get jobs ready to be scheduled (dependencies satisfied, not yet queued)
    pub async fn ready_jobs(&self) -> Vec<JobId> {
        let state = self.state.read().await;
        let mut ready = Vec::new();

        for node in &self.graph.nodes {
            let job_state = match state.jobs.get(&node.id) {
                Some(s) => s,
                None => continue,
            };

            // Skip if not in queued state
            if job_state.status() != JobStatus::Queued {
                continue;
            }

            // Check if all dependencies are satisfied
            let deps_satisfied = node.dependencies.iter().all(|dep_id| {
                state
                    .jobs
                    .get(dep_id)
                    .is_some_and(|s| s.status() == JobStatus::Succeeded)
            });

            if deps_satisfied {
                ready.push(node.id);
            }
        }

        ready
    }

    /// Queue dependency-gated jobs after a predecessor succeeds and return
    /// the jobs that became schedulable. Entry-point jobs are queued by
    /// `start`; downstream jobs are released through this method.
    pub async fn queue_ready_jobs(&self) -> Result<Vec<JobId>> {
        let mut state = self.state.write().await;
        let mut queued = Vec::new();
        for node in &self.graph.nodes {
            let Some(job_state) = state.jobs.get(&node.id) else {
                continue;
            };
            if job_state.status() != JobStatus::Pending {
                continue;
            }
            let deps_satisfied = node.dependencies.iter().all(|dep_id| {
                state
                    .jobs
                    .get(dep_id)
                    .is_some_and(|dependency| dependency.status() == JobStatus::Succeeded)
            });
            if deps_satisfied {
                if let Some(job_state) = state.jobs.get_mut(&node.id) {
                    job_state.queue()?;
                    queued.push(node.id);
                }
            }
        }
        Ok(queued)
    }

    /// Assign a job to a runner
    pub async fn assign_job(
        &self,
        job_id: JobId,
        runner_id: gitforge_common::RunnerId,
    ) -> Result<()> {
        let mut state = self.state.write().await;
        if let Some(job_state) = state.jobs.get_mut(&job_id) {
            job_state.assign(runner_id)?;
        }
        Ok(())
    }

    /// Mark a job as started
    pub async fn start_job(&self, job_id: JobId) -> Result<()> {
        let mut state = self.state.write().await;
        if let Some(job_state) = state.jobs.get_mut(&job_id) {
            job_state.start()?;
        }
        Ok(())
    }

    /// Mark a job as succeeded
    pub async fn succeed_job(&self, job_id: JobId, exit_code: i32) -> Result<()> {
        let mut state = self.state.write().await;
        if let Some(job_state) = state.jobs.get_mut(&job_id) {
            job_state.succeed(exit_code)?;

            // Check if pipeline is complete
            state.settle_if_all_finished();
        }
        Ok(())
    }

    /// Mark a job as failed
    pub async fn fail_job(&self, job_id: JobId, exit_code: i32, error: String) -> Result<()> {
        let mut state = self.state.write().await;
        if let Some(job_state) = state.jobs.get_mut(&job_id) {
            job_state.fail(exit_code, error)?;
        }
        // Everything downstream of the failure can never run; cancelling it
        // keeps the run able to reach a terminal state (see
        // `cancel_descendants`).
        self.cancel_descendants(&mut state, job_id);
        // A failed job dooms the pipeline, but the terminal status waits
        // for every job to finish: siblings may still be executing in
        // the run workspace, and finalizing here would fence off their
        // completions ("unknown pipeline run") and delete the checkout
        // out from under their containers.
        state.settle_if_all_finished();
        Ok(())
    }

    /// Mark a job as timed out
    pub async fn timeout_job(&self, job_id: JobId) -> Result<()> {
        let mut state = self.state.write().await;
        if let Some(job_state) = state.jobs.get_mut(&job_id) {
            job_state.timeout()?;
        }
        self.cancel_descendants(&mut state, job_id);
        // Same reasoning as `fail_job`: wait for all jobs to finish
        // before declaring the pipeline failed.
        state.settle_if_all_finished();
        Ok(())
    }

    /// Cancel every job that transitively depends on `failed`. They can
    /// never be scheduled (`ready_jobs` requires succeeded dependencies),
    /// and leaving them `Pending` would keep the pipeline non-terminal
    /// forever — never failed, never finalized, workspace never freed.
    /// Unrelated branches of the DAG are untouched and still run.
    fn cancel_descendants(&self, state: &mut CiEngineState, failed: JobId) {
        let mut doomed: Vec<JobId> = Vec::new();
        let mut frontier = vec![failed];
        while let Some(id) = frontier.pop() {
            for node in &self.graph.nodes {
                if node.dependencies.contains(&id) && !doomed.contains(&node.id) {
                    doomed.push(node.id);
                    frontier.push(node.id);
                }
            }
        }
        for id in doomed {
            if let Some(job_state) = state.jobs.get_mut(&id) {
                if !job_state.is_terminal() {
                    job_state.cancel().ok();
                }
            }
        }
    }

    /// Cancel a specific job.
    ///
    /// Everything transitively downstream of the cancelled job can never be
    /// dispatched (`ready_jobs` requires succeeded dependencies), so it is
    /// cancelled in the mirror too — the same rule `fail_job` applies — and
    /// the run settles once its last job is terminal. Without the cascade
    /// and the settle, a per-job cancellation of a stage with dependents (or
    /// of the last unfinished job in the run) left the mirror non-terminal:
    /// the run never graded, finalization never ran, and the engine plus
    /// workspace leaked until a restart reconciled them.
    pub async fn cancel_job(&self, job_id: JobId) -> Result<()> {
        let mut state = self.state.write().await;
        if let Some(job_state) = state.jobs.get_mut(&job_id) {
            if !job_state.is_terminal() {
                job_state.cancel()?;
            }
        }
        // A cancelled ancestor dooms its dependents exactly like a failed
        // one: they can never run under any verdict.
        self.cancel_descendants(&mut state, job_id);
        state.settle_if_all_finished();
        Ok(())
    }

    /// Cancel the pipeline run.
    ///
    /// Records cancellation intent and immediately cancels every mirror job
    /// no runner has taken over (`Pending`/`Queued`). Jobs in runner-owned
    /// mirror states (`Assigned`/`Running`) are deliberately left untouched:
    /// there is no run-level cancellation request/ack protocol that reaches
    /// a runner (the only runner-facing cancellation signal is the durable
    /// per-job row the runner's cancellation watch polls), so their work is
    /// preserved and their own completion — or the scheduler's fence and
    /// timeout reaping — is what settles them. The run is marked `Cancelled`
    /// only once every job is terminal, the same rule the fail-fast paths
    /// follow, so an in-flight job keeps its completion event, its lease,
    /// and its workspace. The service-side finalizer additionally refuses to
    /// commit the verdict while durable `assigned`/`running` rows are still
    /// live — the mirror cannot see dispatch, only the durable rows can
    /// arbitrate — keeping the engine and workspace for the completion
    /// consumer, fence sweep, or timeout watchdog to reconcile.
    ///
    /// Known limitation: cancellation intent is held in the engine mirror
    /// only until finalization commits. If the control plane restarts before
    /// that, the rebuilt engine has no durable trace of the cancel and the
    /// run resumes; the cancellation must be re-issued (the durable per-job
    /// cancel path, which runners do honor, is the durable alternative).
    ///
    /// A run that already reached a terminal verdict is left untouched: a
    /// late cancellation never re-grades a settled run (F24).
    pub async fn cancel(&self) -> Result<()> {
        let mut state = self.state.write().await;
        if matches!(
            state.status,
            PipelineStatus::Succeeded | PipelineStatus::Failed | PipelineStatus::Cancelled
        ) {
            return Ok(());
        }
        state.cancel_requested = true;
        for job_state in state.jobs.values_mut() {
            if matches!(job_state.status(), JobStatus::Pending | JobStatus::Queued) {
                job_state.cancel().ok();
            }
        }
        state.settle_if_all_finished();

        Ok(())
    }

    /// Get job info
    pub async fn get_job(&self, job_id: JobId) -> Option<JobStateMachine> {
        let state = self.state.read().await;
        state.jobs.get(&job_id).cloned()
    }

    /// Get the job graph
    pub fn graph(&self) -> &JobGraph {
        &self.graph
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::{JobDefinition, StepDefinition, TriggerType};
    use gitforge_common::PipelineId;
    use std::collections::HashMap;

    fn make_pipeline() -> PipelineDefinition {
        PipelineDefinition {
            name: "test".to_string(),
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

    /// Two independent entry jobs, so a test can have one fail while the
    /// other is still in flight.
    fn make_parallel_pipeline() -> PipelineDefinition {
        let entry_job = |name: &str| JobDefinition {
            name: name.to_string(),
            image: "rust:latest".to_string(),
            needs: vec![],
            env: HashMap::new(),
            steps: vec![StepDefinition {
                name: "run".to_string(),
                run: "true".to_string(),
                env: None,
                working_directory: None,
                condition: None,
            }],
            timeout: None,
            retry: None,
        };
        PipelineDefinition {
            name: "test-parallel".to_string(),
            version: "1.0".to_string(),
            trigger_on: vec![TriggerType::Push],
            environment: HashMap::new(),
            jobs: vec![entry_job("a"), entry_job("b")],
        }
    }

    #[tokio::test]
    async fn test_engine_lifecycle() {
        let event = PipelineTriggerEvent::new(
            PipelineId::new(),
            RepoId::new(),
            "abc123".to_string(),
            TriggerType::Push,
        );

        let engine = CiEngine::new(event, make_pipeline()).await.unwrap();

        // Initially pending
        let state = engine.state().await;
        assert_eq!(state.status, PipelineStatus::Pending);

        // Start
        engine.start().await.unwrap();
        let state = engine.state().await;
        assert_eq!(state.status, PipelineStatus::Running);

        // Build job should be queued (entry point with no dependencies)
        let ready = engine.ready_jobs().await;
        assert_eq!(ready.len(), 1);

        // Simulate build completing - job must be assigned first
        let build_job = ready[0];
        let runner_id = gitforge_common::RunnerId::new();
        engine.assign_job(build_job, runner_id).await.unwrap();
        engine.start_job(build_job).await.unwrap();
        engine.succeed_job(build_job, 0).await.unwrap();

        // After build succeeds, test job is now ready (still needs to be picked up by scheduler)
        // Note: In real system, scheduler would dequeue it. For test, we verify pipeline completion.
        let state = engine.state().await;
        assert!(
            !state.failed_jobs().is_empty()
                || state.status == PipelineStatus::Succeeded
                || state
                    .jobs
                    .values()
                    .any(|j| j.status() == JobStatus::Succeeded)
        );
    }

    #[tokio::test]
    async fn test_engine_fail_job() {
        let event = PipelineTriggerEvent::new(
            PipelineId::new(),
            RepoId::new(),
            "abc123".to_string(),
            TriggerType::Push,
        );

        let engine = CiEngine::new(event, make_parallel_pipeline())
            .await
            .unwrap();
        engine.start().await.unwrap();

        let ready = engine.ready_jobs().await;
        assert_eq!(ready.len(), 2);
        let runner_id = gitforge_common::RunnerId::new();
        for job in &ready {
            engine.assign_job(*job, runner_id).await.unwrap();
            engine.start_job(*job).await.unwrap();
        }

        // The first job fails while its sibling is still running: the run is
        // doomed but must stay non-terminal so the sibling's completion is
        // still accepted and the workspace keeps existing for it.
        engine
            .fail_job(ready[0], 1, "step failed".to_string())
            .await
            .unwrap();

        let state = engine.state().await;
        assert_eq!(state.status, PipelineStatus::Running);
        assert!(state.finished_at.is_none());

        // The last in-flight job finishing settles the pipeline as failed.
        engine
            .fail_job(ready[1], 0, "cancelled after sibling failure".to_string())
            .await
            .unwrap();

        let state = engine.state().await;
        assert_eq!(state.status, PipelineStatus::Failed);
        assert!(state.finished_at.is_some());
    }

    #[tokio::test]
    async fn test_engine_fail_cancels_descendants() {
        let event = PipelineTriggerEvent::new(
            PipelineId::new(),
            RepoId::new(),
            "abc123".to_string(),
            TriggerType::Push,
        );

        // build -> test: failing build must cancel test, otherwise the run
        // can never reach a terminal state and its workspace leaks.
        let mut chain = make_pipeline();
        chain.jobs.push(JobDefinition {
            name: "deploy".to_string(),
            image: "rust:latest".to_string(),
            needs: vec!["test".to_string()],
            env: HashMap::new(),
            steps: vec![StepDefinition {
                name: "deploy".to_string(),
                run: "echo deploy".to_string(),
                env: None,
                working_directory: None,
                condition: None,
            }],
            timeout: None,
            retry: None,
        });

        let engine = CiEngine::new(event, chain).await.unwrap();
        engine.start().await.unwrap();

        let ready = engine.ready_jobs().await;
        assert_eq!(ready.len(), 1);
        let runner_id = gitforge_common::RunnerId::new();
        engine.assign_job(ready[0], runner_id).await.unwrap();
        engine.start_job(ready[0]).await.unwrap();
        engine
            .fail_job(ready[0], 1, "build exploded".to_string())
            .await
            .unwrap();

        let state = engine.state().await;
        assert_eq!(state.status, PipelineStatus::Failed);
        assert!(state.finished_at.is_some());
        assert_eq!(state.jobs.len(), 3);
        for job in state.jobs.values() {
            assert!(
                job.is_terminal(),
                "job {:?} left non-terminal",
                job.status()
            );
        }
    }

    #[tokio::test]
    async fn test_engine_cancel() {
        let event = PipelineTriggerEvent::new(
            PipelineId::new(),
            RepoId::new(),
            "abc123".to_string(),
            TriggerType::Push,
        );

        let engine = CiEngine::new(event, make_pipeline()).await.unwrap();
        engine.start().await.unwrap();

        engine.cancel().await.unwrap();

        let state = engine.state().await;
        assert_eq!(state.status, PipelineStatus::Cancelled);
        assert!(state.finished_at.is_some());
    }

    // Cancelling a run whose job a runner already owns must NOT cancel that
    // mirror job: no run-level cancellation request/ack protocol reaches a
    // runner, so the work belongs to the runner lifecycle until it reports.
    // The not-yet-dispatched tail is cancelled immediately, and the run only
    // becomes terminal once the owned job settles — with the cancellation
    // winning the verdict even when the job succeeds.
    #[tokio::test]
    async fn test_engine_cancel_spares_runner_owned_job_and_settles_on_completion() {
        let event = PipelineTriggerEvent::new(
            PipelineId::new(),
            RepoId::new(),
            "abc123".to_string(),
            TriggerType::Push,
        );
        let engine = CiEngine::new(event, make_pipeline()).await.unwrap();
        engine.start().await.unwrap();

        let ready = engine.ready_jobs().await;
        assert_eq!(ready.len(), 1);
        let runner_id = gitforge_common::RunnerId::new();
        engine.assign_job(ready[0], runner_id).await.unwrap();
        engine.start_job(ready[0]).await.unwrap();

        engine.cancel().await.unwrap();

        let state = engine.state().await;
        assert!(
            state.cancel_requested,
            "cancellation intent must be recorded"
        );
        assert_eq!(
            state.status,
            PipelineStatus::Running,
            "a run with live runner-owned work stays non-terminal"
        );
        assert!(
            state.finished_at.is_none(),
            "no finish time while a runner owns work"
        );
        assert_eq!(
            state.jobs[&ready[0]].status(),
            JobStatus::Running,
            "a runner-owned job keeps its lifecycle"
        );
        let test_id = *state
            .jobs
            .keys()
            .find(|id| **id != ready[0])
            .expect("downstream stage");
        assert_eq!(
            state.jobs[&test_id].status(),
            JobStatus::Cancelled,
            "the not-yet-dispatched tail is cancelled immediately"
        );
        assert!(
            engine.ready_jobs().await.is_empty(),
            "a cancelled stage is never dispatched"
        );

        // The runner finishes after the cancel: the completion is still a
        // valid transition, and the last job settling is what grades the
        // run — as `Cancelled`, the operator's intent, not `Succeeded`.
        engine.succeed_job(ready[0], 0).await.unwrap();
        let state = engine.state().await;
        assert_eq!(state.status, PipelineStatus::Cancelled);
        assert!(state.finished_at.is_some());
        assert_eq!(
            state.jobs[&ready[0]].status(),
            JobStatus::Succeeded,
            "the job row keeps its true outcome"
        );
    }

    // The same preservation applies when the runner-owned job fails after
    // the cancel: the verdict is still `Cancelled`, not `Failed`.
    #[tokio::test]
    async fn test_engine_cancel_verdict_survives_late_failure() {
        let event = PipelineTriggerEvent::new(
            PipelineId::new(),
            RepoId::new(),
            "abc123".to_string(),
            TriggerType::Push,
        );
        let engine = CiEngine::new(event, make_parallel_pipeline())
            .await
            .unwrap();
        engine.start().await.unwrap();

        let ready = engine.ready_jobs().await;
        let runner_id = gitforge_common::RunnerId::new();
        for job in &ready {
            engine.assign_job(*job, runner_id).await.unwrap();
            engine.start_job(*job).await.unwrap();
        }

        engine.cancel().await.unwrap();
        assert_eq!(engine.state().await.status, PipelineStatus::Running);

        engine
            .fail_job(ready[0], 1, "failed after cancel".to_string())
            .await
            .unwrap();
        assert_eq!(
            engine.state().await.status,
            PipelineStatus::Running,
            "one runner-owned job is still live"
        );
        engine.succeed_job(ready[1], 0).await.unwrap();

        let state = engine.state().await;
        assert_eq!(
            state.status,
            PipelineStatus::Cancelled,
            "cancellation outranks the in-flight outcomes"
        );
        assert!(state.finished_at.is_some());
    }

    // A cancel that arrives after the run already reached a verdict is a
    // no-op: terminal verdicts are never rewritten (F24), not even by an
    // operator.
    #[tokio::test]
    async fn test_engine_cancel_after_terminal_verdict_is_a_no_op() {
        let event = PipelineTriggerEvent::new(
            PipelineId::new(),
            RepoId::new(),
            "abc123".to_string(),
            TriggerType::Push,
        );
        let settled = CiEngine::new(event, make_parallel_pipeline())
            .await
            .unwrap();
        settled.start().await.unwrap();
        let ready = settled.ready_jobs().await;
        let runner_id = gitforge_common::RunnerId::new();
        for job in &ready {
            settled.assign_job(*job, runner_id).await.unwrap();
            settled.start_job(*job).await.unwrap();
            settled.succeed_job(*job, 0).await.unwrap();
        }
        assert_eq!(settled.state().await.status, PipelineStatus::Succeeded);

        settled.cancel().await.unwrap();
        let state = settled.state().await;
        assert_eq!(
            state.status,
            PipelineStatus::Succeeded,
            "a late cancel never re-grades a settled run"
        );
        assert!(!state.cancel_requested);
    }

    #[tokio::test]
    async fn test_engine_get_job() {
        let event = PipelineTriggerEvent::new(
            PipelineId::new(),
            RepoId::new(),
            "abc123".to_string(),
            TriggerType::Push,
        );

        let engine = CiEngine::new(event, make_pipeline()).await.unwrap();
        engine.start().await.unwrap();

        let ready = engine.ready_jobs().await;
        let build_job = ready[0];

        let job = engine.get_job(build_job).await;
        assert!(job.is_some());

        let non_existent = gitforge_common::JobId::new();
        let not_found = engine.get_job(non_existent).await;
        assert!(not_found.is_none());
    }

    #[tokio::test]
    async fn test_engine_state_all_jobs_finished() {
        let event = PipelineTriggerEvent::new(
            PipelineId::new(),
            RepoId::new(),
            "abc123".to_string(),
            TriggerType::Push,
        );

        let engine = CiEngine::new(event, make_pipeline()).await.unwrap();
        let state = engine.state().await;

        // No jobs finished yet
        assert!(!state.all_jobs_finished());
    }

    #[tokio::test]
    async fn test_engine_state_pending_jobs() {
        let event = PipelineTriggerEvent::new(
            PipelineId::new(),
            RepoId::new(),
            "abc123".to_string(),
            TriggerType::Push,
        );

        let engine = CiEngine::new(event, make_pipeline()).await.unwrap();
        engine.start().await.unwrap();

        let state = engine.state().await;
        let pending = state.pending_jobs();
        assert!(!pending.is_empty());
    }

    #[tokio::test]
    async fn test_engine_ready_jobs_before_start() {
        let event = PipelineTriggerEvent::new(
            PipelineId::new(),
            RepoId::new(),
            "abc123".to_string(),
            TriggerType::Push,
        );

        let engine = CiEngine::new(event, make_pipeline()).await.unwrap();

        // Before start, no jobs should be ready
        let ready = engine.ready_jobs().await;
        assert!(ready.is_empty());
    }

    #[tokio::test]
    async fn test_engine_state_failed_jobs() {
        let event = PipelineTriggerEvent::new(
            PipelineId::new(),
            RepoId::new(),
            "abc123".to_string(),
            TriggerType::Push,
        );

        let engine = CiEngine::new(event, make_pipeline()).await.unwrap();
        engine.start().await.unwrap();

        let ready = engine.ready_jobs().await;
        let build_job = ready[0];
        let runner_id = gitforge_common::RunnerId::new();

        engine.assign_job(build_job, runner_id).await.unwrap();
        engine.start_job(build_job).await.unwrap();
        engine
            .fail_job(build_job, 1, "test failure".to_string())
            .await
            .unwrap();

        let state = engine.state().await;
        assert!(!state.failed_jobs().is_empty());
    }

    /// A scheduler row that went terminal behind the engine's back (a
    /// runner was fenced while its job was running) must map to exactly
    /// one action, and only for jobs the engine still believes are
    /// running.
    #[tokio::test]
    async fn test_fence_actions_maps_terminal_scheduler_rows() {
        let event = PipelineTriggerEvent::new(
            PipelineId::new(),
            RepoId::new(),
            "abc123".to_string(),
            TriggerType::Push,
        );
        let engine = CiEngine::new(event, make_parallel_pipeline())
            .await
            .unwrap();
        engine.start().await.unwrap();

        let ready = engine.ready_jobs().await;
        assert_eq!(ready.len(), 2);
        let runner_id = gitforge_common::RunnerId::new();
        for job in &ready {
            engine.assign_job(*job, runner_id).await.unwrap();
            engine.start_job(*job).await.unwrap();
        }

        let state = engine.state().await;
        let (a, b) = (ready[0], ready[1]);
        let db_status: HashMap<JobId, String> =
            [(a, "failed".to_string()), (b, "running".to_string())]
                .into_iter()
                .collect();
        assert_eq!(
            fence_actions(&state, &db_status),
            vec![(a, FenceAction::Fail)]
        );

        // Applying the action converges the DAG exactly like a runner-
        // reported failure: the fenced job fails, the run waits for the
        // sibling.
        engine
            .fail_job(a, 137, "runner lost; job fenced".to_string())
            .await
            .unwrap();
        let state = engine.state().await;
        assert_eq!(state.status, PipelineStatus::Running);

        // The sibling finishing settles the run as failed, not hung.
        engine.succeed_job(b, 0).await.unwrap();
        let state = engine.state().await;
        assert_eq!(state.status, PipelineStatus::Failed);
        assert!(state.finished_at.is_some());
    }

    /// Timeout and cancelled scheduler rows map to their own actions;
    /// non-terminal rows and engine-finished jobs map to none.
    #[tokio::test]
    async fn test_fence_actions_action_and_noise_mapping() {
        let event = PipelineTriggerEvent::new(
            PipelineId::new(),
            RepoId::new(),
            "abc123".to_string(),
            TriggerType::Push,
        );
        let engine = CiEngine::new(event, make_parallel_pipeline())
            .await
            .unwrap();
        engine.start().await.unwrap();

        let ready = engine.ready_jobs().await;
        let (a, b, c) = (ready[0], ready[1], JobId::new());
        let runner_id = gitforge_common::RunnerId::new();
        for job in &ready {
            engine.assign_job(*job, runner_id).await.unwrap();
            engine.start_job(*job).await.unwrap();
        }

        let state = engine.state().await;
        let db_status: HashMap<JobId, String> = [
            (a, "timed_out".to_string()),
            (b, "cancelled".to_string()),
            (c, "failed".to_string()),
        ]
        .into_iter()
        .collect();
        let actions = fence_actions(&state, &db_status);
        assert!(actions.contains(&(a, FenceAction::Timeout)));
        assert!(actions.contains(&(b, FenceAction::Cancel)));
        assert_eq!(actions.len(), 2, "unknown job rows must be ignored");

        // A durable `infrastructure_failure` row (R6.3) must converge too,
        // as Fail: the engine mirror speaks success/failure only, and an
        // unmapped terminal status wedges the run exactly like bccaa1be.
        assert_eq!(
            fence_actions(
                &state,
                &[(a, "infrastructure_failure".to_string())]
                    .into_iter()
                    .collect()
            ),
            vec![(a, FenceAction::Fail)]
        );

        // A durable `succeeded` row under a Running mirror maps to Succeed:
        // the engine missed the completion event and the durable row is the
        // authority. Every terminal durable status must converge — an
        // unmapped direction is a run the watchdog can never settle
        // (the run bccaa1be wedge fell through `_ => None` here).
        assert_eq!(
            fence_actions(
                &state,
                &[(b, "succeeded".to_string())].into_iter().collect()
            ),
            vec![(b, FenceAction::Succeed)]
        );

        // A job the engine already finished is never re-judged from a stale
        // scheduler row — only unfinished mirror states converge.
        engine.succeed_job(a, 0).await.unwrap();
        let state = engine.state().await;
        let db_status: HashMap<JobId, String> = [(a, "failed".to_string())].into_iter().collect();
        assert!(fence_actions(&state, &db_status).is_empty());
    }

    /// An operator cancelled a job that was still queued: the durable row is
    /// terminal while the engine mirror shows `Queued` and no completion
    /// event will ever arrive (there is no runner). The fence sweep must
    /// converge that mirror too — restricting it to `Running` mirrors left
    /// such runs non-terminal forever, holding the engine and workspace.
    #[tokio::test]
    async fn test_fence_actions_converges_queued_mirror_with_cancelled_row() {
        let event = PipelineTriggerEvent::new(
            PipelineId::new(),
            RepoId::new(),
            "abc123".to_string(),
            TriggerType::Push,
        );
        let engine = CiEngine::new(event, make_pipeline()).await.unwrap();
        engine.start().await.unwrap();
        let planned: HashMap<String, JobId> = engine
            .planned_jobs()
            .into_iter()
            .map(|(id, name)| (name, id))
            .collect();

        let state = engine.state().await;
        assert_eq!(
            state.jobs[&planned["build"]].status(),
            JobStatus::Queued,
            "the mirror is queued while the durable row was cancelled beneath it"
        );
        let db_status: HashMap<JobId, String> = [(planned["build"], "cancelled".to_string())]
            .into_iter()
            .collect();
        assert_eq!(
            fence_actions(&state, &db_status),
            vec![(planned["build"], FenceAction::Cancel)]
        );

        // Applying the action cancels the queued job AND its dependents and
        // settles the run — the per-job cancel has no failures, so the run
        // grades `Cancelled`, not `Failed`.
        engine.cancel_job(planned["build"]).await.unwrap();
        let state = engine.state().await;
        assert_eq!(state.jobs[&planned["build"]].status(), JobStatus::Cancelled);
        assert_eq!(
            state.jobs[&planned["test"]].status(),
            JobStatus::Cancelled,
            "the dependent of a cancelled stage can never be dispatched"
        );
        assert_eq!(state.status, PipelineStatus::Cancelled);
        assert!(state.finished_at.is_some());
    }

    /// Per-job cancellation verdicts follow the durable graders' precedence:
    /// a cancelled job with every other job green grades the run
    /// `Cancelled` (not a synthetic `Failed`), while a genuinely failed
    /// sibling still fails the run, and a run-level cancel request outranks
    /// everything it covers.
    #[tokio::test]
    async fn test_cancel_settlement_precedence_matches_durable_graders() {
        let event_for = |name: &str| {
            PipelineTriggerEvent::new(
                PipelineId::new(),
                RepoId::new(),
                format!("{name}-sha"),
                TriggerType::Push,
            )
        };

        // Operator cancels one entry job while its sibling succeeds.
        let engine = CiEngine::new(event_for("cancel"), make_parallel_pipeline())
            .await
            .unwrap();
        engine.start().await.unwrap();
        let ready = engine.ready_jobs().await;
        let (a, b) = (ready[0], ready[1]);
        engine.succeed_job(a, 0).await.unwrap();
        engine.cancel_job(b).await.unwrap();
        let state = engine.state().await;
        assert_eq!(state.status, PipelineStatus::Cancelled);
        assert!(!state.cancel_requested, "this was a per-job cancel");

        // A real failure with a cancelled sibling still fails the run.
        let engine = CiEngine::new(event_for("failed"), make_parallel_pipeline())
            .await
            .unwrap();
        engine.start().await.unwrap();
        let ready = engine.ready_jobs().await;
        let (a, b) = (ready[0], ready[1]);
        engine
            .fail_job(a, 1, "genuinely broken".to_string())
            .await
            .unwrap();
        engine.cancel_job(b).await.unwrap();
        let state = engine.state().await;
        assert_eq!(state.status, PipelineStatus::Failed);

        // A run-level cancel request keeps winning over in-flight outcomes:
        // both jobs are runner-owned when the cancel lands, so neither
        // mirror is touched — the recorded intent alone decides the verdict
        // once the last of them settles.
        let engine = CiEngine::new(event_for("run-cancel"), make_parallel_pipeline())
            .await
            .unwrap();
        engine.start().await.unwrap();
        let ready = engine.ready_jobs().await;
        let (a, b) = (ready[0], ready[1]);
        let runner = gitforge_common::RunnerId::new();
        engine.assign_job(a, runner).await.unwrap();
        engine.start_job(a).await.unwrap();
        engine.assign_job(b, runner).await.unwrap();
        engine.start_job(b).await.unwrap();
        engine
            .fail_job(a, 1, "lost sibling".to_string())
            .await
            .unwrap();
        engine.cancel().await.unwrap();
        engine.succeed_job(b, 0).await.unwrap();
        let state = engine.state().await;
        assert_eq!(state.status, PipelineStatus::Cancelled);
        assert!(state.cancel_requested);
    }

    // Restart recovery grafts durable rows onto a rebuilt engine: graph node
    // ids are replaced by the durable ids (matched by job name), statuses
    // are restored verbatim — including ones no live transition from
    // `Pending` could reach — and the DAG still advances from the grafted
    // state. A run that died with its head done and its second stage queued
    // must come back exactly there, not from scratch.
    #[tokio::test]
    async fn test_rebuild_grafts_durable_rows_by_name() {
        let pipeline = make_pipeline();
        let event = PipelineTriggerEvent::new(
            PipelineId::new(),
            RepoId::new(),
            "abc123".to_string(),
            TriggerType::Push,
        );

        // The live engine's planned ids stand in for the durable ids a
        // previous process persisted at trigger time.
        let live = CiEngine::new_with_run_id(event, pipeline.clone(), PipelineRunId::new())
            .await
            .unwrap();
        let planned: HashMap<String, JobId> = live
            .planned_jobs()
            .into_iter()
            .map(|(id, name)| (name, id))
            .collect();
        let build_id = planned["build"];
        let test_id = planned["test"];
        let started = chrono::Utc::now() - chrono::Duration::seconds(45);

        let rows = vec![
            (build_id, "build".to_string(), JobStatus::Succeeded, None),
            (
                test_id,
                "test".to_string(),
                JobStatus::Queued,
                Some(started),
            ),
        ];
        let rebuilt = CiEngine::rebuild(
            PipelineRunId::new(),
            PipelineId::new(),
            RepoId::new(),
            pipeline,
            &rows,
        )
        .await
        .unwrap();

        let state = rebuilt.state().await;
        assert_eq!(state.status, PipelineStatus::Running);
        assert_eq!(state.jobs[&build_id].status(), JobStatus::Succeeded);
        assert_eq!(state.jobs[&test_id].status(), JobStatus::Queued);
        assert_eq!(state.jobs[&test_id].started_at(), Some(started));

        // The chain resumes exactly where it stopped: the queued stage whose
        // dependency succeeded is ready, nothing else is.
        assert_eq!(rebuilt.ready_jobs().await, vec![test_id]);

        // The grafted machine keeps living a normal lifecycle.
        let runner_id = gitforge_common::RunnerId::new();
        rebuilt.assign_job(test_id, runner_id).await.unwrap();
        rebuilt.start_job(test_id).await.unwrap();
        rebuilt.succeed_job(test_id, 0).await.unwrap();
        assert!(rebuilt.state().await.jobs[&test_id].is_terminal());
    }

    // A name with no durable row (a pre-durable-planning run that died
    // before enqueueing its tail) keeps its fresh id and starts at Pending;
    // it must be releasable once its grafted dependencies are terminal.
    #[tokio::test]
    async fn test_rebuild_keeps_fresh_ids_for_unplanned_stages() {
        let pipeline = make_pipeline();
        let build_id = JobId::new();
        let rows = vec![(build_id, "build".to_string(), JobStatus::Succeeded, None)];

        let rebuilt = CiEngine::rebuild(
            PipelineRunId::new(),
            PipelineId::new(),
            RepoId::new(),
            pipeline,
            &rows,
        )
        .await
        .unwrap();

        let state = rebuilt.state().await;
        assert_eq!(state.jobs[&build_id].status(), JobStatus::Succeeded);
        let test_id = *state
            .jobs
            .iter()
            .find(|(id, _)| **id != build_id)
            .expect("unplanned stage keeps an id")
            .0;
        assert_eq!(state.jobs[&test_id].status(), JobStatus::Pending);

        // The unplanned tail is released through the normal path once its
        // dependency is terminal.
        let queued = rebuilt.queue_ready_jobs().await.unwrap();
        assert_eq!(queued, vec![test_id]);
        assert_eq!(rebuilt.ready_jobs().await, vec![test_id]);
    }
}

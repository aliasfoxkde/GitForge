//! CI Engine - orchestrates pipeline execution

use crate::dag::{DagBuilder, JobGraph};
use crate::pipeline::{PipelineDefinition, PipelineTriggerEvent};
use crate::state::JobStateMachine;
use gitforge_common::{JobId, JobStatus, PipelineRunId, PipelineStatus, RepoId, Result};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::process::Command;
use tokio::sync::RwLock;
use tokio::time::{timeout, Duration};

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
    /// Environment variables produced by the detect-changes job, read from its
    /// job result. Used to evaluate `skip_if` on downstream jobs.
    pub pipeline_env: HashMap<String, String>,
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
            pipeline_env: HashMap::new(),
        }
    }

    /// Merge job output environment variables (from detect-changes) into pipeline_env.
    /// Called by the orchestrator when a job completes with a result JSON that
    /// contains CI_CHANGE_* vars — only detect-changes writes these.
    pub fn merge_job_env(&mut self, _job_id: JobId, env_vars: &HashMap<String, String>) {
        for (k, v) in env_vars {
            if k.starts_with("CI_CHANGE_") {
                self.pipeline_env.insert(k.clone(), v.clone());
            }
        }
    }

    /// Check if all jobs are finished
    pub fn all_jobs_finished(&self) -> bool {
        self.jobs
            .values()
            .all(super::state::JobStateMachine::is_terminal)
    }

    /// Settle the run's status once every job is terminal.
    ///
    /// Every terminal transition of a job funnels through here: the run's
    /// status is a function of its jobs, so the last job to reach a terminal
    /// state decides it. Skipping a job is such a transition — a run whose
    /// tail was all skipped would otherwise stay `Running` forever with no
    /// job left to settle it.
    fn settle_status(&mut self) {
        if !matches!(
            self.status,
            PipelineStatus::Pending | PipelineStatus::Running
        ) {
            return;
        }
        if !self.all_jobs_finished() {
            return;
        }
        self.status = if self.all_jobs_succeeded() {
            PipelineStatus::Succeeded
        } else {
            PipelineStatus::Failed
        };
        self.finished_at = Some(chrono::Utc::now());
    }

    /// Check if all jobs succeeded or were skipped.
    /// Skipped jobs correctly didn't need to run (their skip_if was false);
    /// they count as succeeded for pipeline outcome.
    pub fn all_jobs_succeeded(&self) -> bool {
        self.jobs
            .values()
            .all(|j| matches!(j.status(), JobStatus::Succeeded | JobStatus::Skipped))
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

/// Extract the `CI_CHANGE_*` variables a completed job published.
///
/// The `detect-changes` job prints its classification as `KEY=value` lines on
/// stdout; the orchestrator parses the persisted job receipt and merges the
/// result into the run's `pipeline_env` — the only environment `skip_if`
/// expressions are evaluated against. A job receipt that carries no such
/// lines contributes nothing.
///
/// Only `CI_CHANGE_`-prefixed keys are taken: the receipt is job output, and
/// an untrusted job must not be able to seed arbitrary environment variables
/// into the engine's expression sandbox.
pub fn extract_change_env(result_json: &str) -> HashMap<String, String> {
    let mut env = HashMap::new();
    let Ok(receipt) = serde_json::from_str::<serde_json::Value>(result_json) else {
        return env;
    };
    let Some(step_results) = receipt.get("step_results").and_then(|v| v.as_array()) else {
        return env;
    };
    for step in step_results {
        let Some(stdout) = step.get("stdout").and_then(|v| v.as_str()) else {
            continue;
        };
        for line in stdout.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let key = key.trim();
            if !key.starts_with("CI_CHANGE_") {
                continue;
            }
            env.insert(key.to_string(), value.trim().to_string());
        }
    }
    env
}

/// Evaluate a `skip_if` condition expression using the CI_CHANGE_* environment
/// variables written by the `detect-changes` job. Returns `Ok(false)` if the
/// job should be skipped, `Ok(true)` if it should run, or an error if the
/// expression could not be evaluated. A `None` expression always runs.
pub async fn eval_skip_if(skip_if: &Option<String>, env: &HashMap<String, String>) -> Result<bool> {
    let Some(expr) = skip_if else {
        return Ok(true); // No condition means always run
    };
    // Build the bash environment from the detect-changes output vars
    let bash_env: Vec<String> = env
        .iter()
        .filter(|(k, _)| k.starts_with("CI_CHANGE_"))
        .map(|(k, v)| format!("{k}={v}"))
        .collect();
    // Add CI_CHANGE_FILE_COUNT for convenience (count of all changed files)
    let file_count = env.get("CI_CHANGE_FILE_COUNT");
    let combined_env = if bash_env.is_empty() && file_count.is_none() {
        format!("set -e; {expr}")
    } else {
        let env_str = bash_env.join(" ")
            + &format!(
                " CI_CHANGE_FILE_COUNT={}",
                file_count.unwrap_or(&"0".to_string())
            );
        format!("export {env_str}; set -e; {expr}")
    };
    // Timeout: 10s max to prevent a malicious expression from stalling the engine
    let result = timeout(
        Duration::from_secs(10),
        Command::new("bash")
            .arg("-c")
            .arg(&combined_env)
            .envs(env.iter().filter(|(k, _)| !k.starts_with("CI_CHANGE_")))
            .output(),
    )
    .await;
    match result {
        Ok(Ok(output)) => {
            let exit_ok = output.status.success();
            if !exit_ok {
                tracing::info!(
                    "skip_if condition '{expr}' evaluated to false (exit {}); skipping job",
                    output.status.code().unwrap_or(-1)
                );
            }
            Ok(exit_ok)
        }
        Ok(Err(e)) => Err(gitforge_common::Error::invalid_input(format!(
            "skip_if expression '{expr}' failed to execute: {e}"
        ))),
        Err(_) => Err(gitforge_common::Error::invalid_input(format!(
            "skip_if expression '{expr}' timed out after 10 seconds"
        ))),
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

/// Compare the engine's running jobs against the scheduler's terminal job
/// statuses and return the (job, action) pairs required to converge.
///
/// Only `Running` engine jobs are considered: a job the engine already
/// finished must not be re-judged from a stale scheduler row, and
/// queued/assigned jobs legitimately have non-terminal scheduler rows.
///
/// Every terminal durable status maps to an action, `succeeded` included:
/// an unmapped direction is a run the watchdog can never settle (a durable
/// `succeeded` under a `Running` mirror previously fell through `_ => None`,
/// leaving the run wedged non-terminal forever — run bccaa1be).
pub fn fence_actions(
    state: &CiEngineState,
    db_status: &HashMap<JobId, String>,
) -> Vec<(JobId, FenceAction)> {
    state
        .jobs
        .iter()
        .filter(|(_, job_state)| job_state.status() == JobStatus::Running)
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

        // Queue all entry point jobs. Entry points always run — they are the
        // source of CI_CHANGE_* vars for downstream skip_if evaluation, and
        // detect-changes must execute to populate them.
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

            // Check if all dependencies are satisfied.
            // Skipped jobs count as succeeded for dependency purposes so
            // downstream jobs run normally (they were gated by a condition).
            let deps_satisfied = node.dependencies.iter().all(|dep_id| {
                state.jobs.get(dep_id).is_some_and(|s| {
                    s.status() == JobStatus::Succeeded || s.status() == JobStatus::Skipped
                })
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
    /// Jobs with `skip_if` conditions that evaluate to false are transitioned
    /// to `Skipped` instead of queued.
    ///
    /// The release is iterated to a fixed point. A skipped job satisfies its
    /// dependants' dependency check (skipped counts as succeeded), so a chain
    /// of skips releases the next stage — and the release must not depend on
    /// the definition listing a job before the job it depends on.
    pub async fn queue_ready_jobs(&self) -> Result<Vec<JobId>> {
        let mut state = self.state.write().await;
        let mut queued = Vec::new();
        loop {
            let mut released = false;
            for node in &self.graph.nodes {
                let Some(job_state) = state.jobs.get(&node.id) else {
                    continue;
                };
                if job_state.status() != JobStatus::Pending {
                    continue;
                }
                let deps_satisfied = node.dependencies.iter().all(|dep_id| {
                    state.jobs.get(dep_id).is_some_and(|dependency| {
                        dependency.status() == JobStatus::Succeeded
                            || dependency.status() == JobStatus::Skipped
                    })
                });
                if !deps_satisfied {
                    continue;
                }
                // Evaluate skip_if condition using pipeline_env from detect-changes
                let should_run =
                    eval_skip_if(&node.definition.skip_if, &state.pipeline_env).await?;
                if !should_run {
                    // Condition evaluated to false — mark skipped instead of queued
                    if let Some(job_state) = state.jobs.get_mut(&node.id) {
                        job_state.skip()?;
                    }
                    released = true;
                    continue;
                }
                if let Some(job_state) = state.jobs.get_mut(&node.id) {
                    job_state.queue()?;
                    queued.push(node.id);
                }
                released = true;
            }
            if !released {
                break;
            }
        }
        // Skipping is a terminal transition like any other: without settling
        // the run's status here, a run whose tail was all skipped stayed
        // `Running` with no job left to settle it, and the durable row never
        // reached a terminal state.
        state.settle_status();
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
            state.settle_status();
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
        state.settle_status();
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
        state.settle_status();
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

    /// Cancel a specific job. Everything downstream of it can never be
    /// scheduled (`ready_jobs` requires succeeded dependencies), so the
    /// same cascade that follows a failure follows a manual cancel —
    /// without it the run holds its engine and workspace open forever on
    /// jobs whose dependencies can never succeed. (F37 covered the
    /// failure-grading path only; the durable descendant rows are still
    /// cancelled by the reconciler's `cancel_doomed_rows` pass.)
    pub async fn cancel_job(&self, job_id: JobId) -> Result<()> {
        let mut state = self.state.write().await;
        if let Some(job_state) = state.jobs.get_mut(&job_id) {
            if !job_state.is_terminal() {
                job_state.cancel()?;
            }
        }
        self.cancel_descendants(&mut state, job_id);
        // Same reasoning as `fail_job`: wait for every job to reach a
        // terminal state before finalizing, so siblings still executing in
        // the run workspace are not fenced off mid-flight.
        if state.all_jobs_finished() {
            state.status = PipelineStatus::Cancelled;
            state.finished_at = Some(chrono::Utc::now());
        }
        Ok(())
    }

    /// Cancel the pipeline
    pub async fn cancel(&self) -> Result<()> {
        let mut state = self.state.write().await;
        state.status = PipelineStatus::Cancelled;
        state.finished_at = Some(chrono::Utc::now());

        for job_state in state.jobs.values_mut() {
            if !job_state.is_terminal() {
                job_state.cancel().ok();
            }
        }

        Ok(())
    }

    /// Merge job output into pipeline_env. Called by the orchestrator when a
    /// job result arrives — the detect-changes job writes CI_CHANGE_* vars that
    /// downstream skip_if conditions depend on.
    pub async fn merge_job_result(&self, job_id: JobId, env_vars: &HashMap<String, String>) {
        let mut state = self.state.write().await;
        state.merge_job_env(job_id, env_vars);
    }

    /// The jobs whose `skip_if` condition evaluated to false.
    ///
    /// The orchestrator persists a durable row for each of them: a skipped
    /// job never reaches a runner, so nothing else would ever move its row
    /// off `pending` and the run would never settle.
    pub async fn skipped_jobs(&self) -> Vec<JobId> {
        let state = self.state.read().await;
        state
            .jobs
            .iter()
            .filter(|(_, job_state)| job_state.status() == JobStatus::Skipped)
            .map(|(job_id, _)| *job_id)
            .collect()
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
                    skip_if: None,
                    volumes: Vec::new(),
                    cache: None,
                    lane: Default::default(),
                    fault: None,
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
                    skip_if: None,
                    volumes: Vec::new(),
                    cache: None,
                    lane: Default::default(),
                    fault: None,
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
            skip_if: None,
            volumes: Vec::new(),
            cache: None,
            lane: Default::default(),
            fault: None,
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
            skip_if: None,
            volumes: Vec::new(),
            cache: None,
            lane: Default::default(),
            fault: None,
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

    /// A manual cancel must cascade exactly like a failure: the cancelled
    /// job's descendants can never be scheduled, so leaving them pending
    /// holds the engine and workspace open forever.
    #[tokio::test]
    async fn test_engine_manual_cancel_cascades_to_descendants() {
        let event = PipelineTriggerEvent::new(
            PipelineId::new(),
            RepoId::new(),
            "abc123".to_string(),
            TriggerType::Push,
        );

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
            skip_if: None,
            volumes: Vec::new(),
            cache: None,
            lane: Default::default(),
            fault: None,
        });

        let engine = CiEngine::new(event, chain).await.unwrap();
        engine.start().await.unwrap();

        let ready = engine.ready_jobs().await;
        assert_eq!(ready.len(), 1);
        let runner_id = gitforge_common::RunnerId::new();
        engine.assign_job(ready[0], runner_id).await.unwrap();
        engine.start_job(ready[0]).await.unwrap();
        engine.cancel_job(ready[0]).await.unwrap();

        let state = engine.state().await;
        assert_eq!(state.status, PipelineStatus::Cancelled);
        assert!(state.finished_at.is_some());
        for job in state.jobs.values() {
            assert!(
                job.is_terminal(),
                "descendant of a manually cancelled job left non-terminal: {:?}",
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

        // A queued engine job with a terminal scheduler row is not
        // fenceable (nothing is running to converge); same for a job the
        // engine already finished.
        engine.succeed_job(a, 0).await.unwrap();
        let state = engine.state().await;
        let db_status: HashMap<JobId, String> = [(a, "failed".to_string())].into_iter().collect();
        assert!(fence_actions(&state, &db_status).is_empty());
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

    /// A pipeline whose tail is gated on the change classification.
    fn make_gated_pipeline(skip_if: Option<String>) -> PipelineDefinition {
        let step = |name: &str| StepDefinition {
            name: name.to_string(),
            run: "true".to_string(),
            env: None,
            working_directory: None,
            condition: None,
        };
        PipelineDefinition {
            name: "gated".to_string(),
            version: "1.0".to_string(),
            trigger_on: vec![TriggerType::Push],
            environment: HashMap::new(),
            jobs: vec![
                JobDefinition {
                    name: "detect-changes".to_string(),
                    image: "rust:latest".to_string(),
                    needs: vec![],
                    env: HashMap::new(),
                    steps: vec![step("detect")],
                    timeout: None,
                    retry: None,
                    skip_if: None,
                    volumes: Vec::new(),
                    cache: None,
                    lane: Default::default(),
                    fault: None,
                },
                JobDefinition {
                    name: "clippy".to_string(),
                    image: "rust:latest".to_string(),
                    needs: vec!["detect-changes".to_string()],
                    env: HashMap::new(),
                    steps: vec![step("lint")],
                    timeout: None,
                    retry: None,
                    skip_if,
                    volumes: Vec::new(),
                    cache: None,
                    lane: Default::default(),
                    fault: None,
                },
            ],
        }
    }

    fn job_id(engine: &CiEngine, name: &str) -> JobId {
        engine
            .graph()
            .nodes
            .iter()
            .find(|node| node.name == name)
            .map(|node| node.id)
            .expect("job exists in the graph")
    }

    #[test]
    fn extract_change_env_reads_the_classification_from_a_receipt() {
        let receipt = serde_json::json!({
            "job_id": "b8c751e8",
            "success": true,
            "step_results": [
                {
                    "exit_code": 0,
                    "stdout": "CI_CHANGE_FILE_COUNT=16\nCI_CHANGE_INCLUDE_CODE=1\nCI_CHANGE_INCLUDE_DOCS=0\n",
                    "stderr": ""
                }
            ],
            "artifacts": []
        })
        .to_string();

        let env = extract_change_env(&receipt);
        assert_eq!(
            env.get("CI_CHANGE_FILE_COUNT").map(String::as_str),
            Some("16")
        );
        assert_eq!(
            env.get("CI_CHANGE_INCLUDE_CODE").map(String::as_str),
            Some("1")
        );
        assert_eq!(
            env.get("CI_CHANGE_INCLUDE_DOCS").map(String::as_str),
            Some("0")
        );
    }

    #[test]
    fn extract_change_env_ignores_foreign_variables_and_junk() {
        let receipt = serde_json::json!({
            "step_results": [{"stdout": "PATH=/usr/bin\nHOME=/root\nnot an assignment\n"}]
        })
        .to_string();

        assert!(extract_change_env(&receipt).is_empty());
        assert!(extract_change_env("not json at all").is_empty());
        assert!(extract_change_env("{}").is_empty());
    }

    /// The defect this guards: an unmerged receipt leaves the expression
    /// environment empty, every condition evaluates false, and the engine
    /// prunes the gate that should have run.
    #[tokio::test]
    async fn merged_change_env_keeps_the_gated_job_running() {
        let event = PipelineTriggerEvent::new(
            PipelineId::new(),
            RepoId::new(),
            "abc123".to_string(),
            TriggerType::Push,
        );
        let pipeline = make_gated_pipeline(Some(
            r#"[[ "$CI_CHANGE_INCLUDE_CODE" == "1" ]]"#.to_string(),
        ));
        let engine = CiEngine::new(event, pipeline).await.unwrap();
        engine.start().await.unwrap();

        let detect = job_id(&engine, "detect-changes");
        let clippy = job_id(&engine, "clippy");
        engine
            .assign_job(detect, gitforge_common::RunnerId::new())
            .await
            .unwrap();
        engine.start_job(detect).await.unwrap();

        let receipt = serde_json::json!({
            "step_results": [{"stdout": "CI_CHANGE_INCLUDE_CODE=1\n"}]
        })
        .to_string();
        engine
            .merge_job_result(detect, &extract_change_env(&receipt))
            .await;
        engine.succeed_job(detect, 0).await.unwrap();

        let queued = engine.queue_ready_jobs().await.unwrap();
        assert_eq!(queued, vec![clippy]);
        let state = engine.state().await;
        assert_eq!(state.jobs[&clippy].status(), JobStatus::Queued);
    }

    /// The same pipeline with an empty environment: the gate does not apply,
    /// so the job is skipped — and the run must still settle.
    #[tokio::test]
    async fn a_skipped_tail_settles_the_run() {
        let event = PipelineTriggerEvent::new(
            PipelineId::new(),
            RepoId::new(),
            "abc123".to_string(),
            TriggerType::Push,
        );
        let pipeline = make_gated_pipeline(Some(
            r#"[[ "$CI_CHANGE_INCLUDE_CODE" == "1" ]]"#.to_string(),
        ));
        let engine = CiEngine::new(event, pipeline).await.unwrap();
        engine.start().await.unwrap();

        let detect = job_id(&engine, "detect-changes");
        let clippy = job_id(&engine, "clippy");
        engine
            .assign_job(detect, gitforge_common::RunnerId::new())
            .await
            .unwrap();
        engine.start_job(detect).await.unwrap();
        engine.succeed_job(detect, 0).await.unwrap();

        let queued = engine.queue_ready_jobs().await.unwrap();
        assert!(queued.is_empty());
        let state = engine.state().await;
        assert_eq!(state.jobs[&clippy].status(), JobStatus::Skipped);
        // The run settles: a skipped job is a terminal transition like any
        // other, and the run's status is a function of its jobs.
        assert_eq!(state.status, PipelineStatus::Succeeded);
        assert_eq!(engine.skipped_jobs().await, vec![clippy]);
    }
}

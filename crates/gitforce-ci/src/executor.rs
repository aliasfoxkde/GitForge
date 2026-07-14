//! Pipeline executor - coordinates with scheduler and runners

use crate::dag::JobGraph;
use crate::engine::{CiEngine, CiEngineState};
use gitforce_common::{JobId, Result, RunnerId};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Pipeline executor coordinates CI engine with external systems
pub struct PipelineExecutor {
    engine: Arc<RwLock<CiEngine>>,
}

impl PipelineExecutor {
    /// Create a new pipeline executor
    pub fn new(engine: CiEngine) -> Self {
        Self {
            engine: Arc::new(RwLock::new(engine)),
        }
    }

    /// Start the pipeline
    pub async fn start(&self) -> Result<()> {
        let engine = self.engine.read().await;
        engine.start().await
    }

    /// Get the engine state
    pub async fn state(&self) -> CiEngineState {
        let engine = self.engine.read().await;
        engine.state().await
    }

    /// Get ready jobs for scheduling
    pub async fn ready_jobs(&self) -> Vec<JobId> {
        let engine = self.engine.read().await;
        engine.ready_jobs().await
    }

    /// Get the job graph
    pub async fn graph(&self) -> JobGraph {
        let engine = self.engine.read().await;
        engine.graph().clone()
    }

    /// Assign a job to a runner
    pub async fn assign_job(&self, job_id: JobId, runner_id: RunnerId) -> Result<()> {
        let engine = self.engine.read().await;
        engine.assign_job(job_id, runner_id).await
    }

    /// Start a job
    pub async fn start_job(&self, job_id: JobId) -> Result<()> {
        let engine = self.engine.read().await;
        engine.start_job(job_id).await
    }

    /// Complete a job successfully
    pub async fn succeed_job(&self, job_id: JobId, exit_code: i32) -> Result<()> {
        let engine = self.engine.read().await;
        engine.succeed_job(job_id, exit_code).await
    }

    /// Fail a job
    pub async fn fail_job(&self, job_id: JobId, exit_code: i32, error: String) -> Result<()> {
        let engine = self.engine.read().await;
        engine.fail_job(job_id, exit_code, error).await
    }

    /// Cancel the pipeline
    pub async fn cancel(&self) -> Result<()> {
        let engine = self.engine.read().await;
        engine.cancel().await
    }
}

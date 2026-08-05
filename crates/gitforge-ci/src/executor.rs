//! Pipeline executor - coordinates with scheduler and runners

use crate::dag::JobGraph;
use crate::engine::{CiEngine, CiEngineState};
use gitforge_common::{JobId, Result, RunnerId};
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::{PipelineDefinition, PipelineTriggerEvent, TriggerType};
    use gitforge_common::{PipelineId, RepoId};
    use std::collections::HashMap;

    async fn create_test_engine() -> CiEngine {
        let pipeline_id = PipelineId::new();
        let repo_id = RepoId::new();
        let trigger = PipelineTriggerEvent::new(
            pipeline_id,
            repo_id,
            "abc123".to_string(),
            TriggerType::Push,
        );
        let pipeline = PipelineDefinition {
            name: "test".to_string(),
            version: "1.0".to_string(),
            trigger_on: vec![TriggerType::Push],
            environment: HashMap::new(),
            jobs: vec![],
        };
        CiEngine::new(trigger, pipeline).await.unwrap()
    }

    #[tokio::test]
    async fn test_pipeline_executor_new() {
        let engine = create_test_engine().await;
        let executor = PipelineExecutor::new(engine);
        let state = executor.state().await;
        assert_eq!(state.status, gitforge_common::PipelineStatus::Pending);
    }

    #[tokio::test]
    async fn test_pipeline_executor_ready_jobs() {
        let engine = create_test_engine().await;
        let executor = PipelineExecutor::new(engine);
        let jobs = executor.ready_jobs().await;
        assert!(jobs.is_empty());
    }

    #[tokio::test]
    async fn test_pipeline_executor_graph() {
        let engine = create_test_engine().await;
        let executor = PipelineExecutor::new(engine);
        let graph = executor.graph().await;
        assert_eq!(graph.nodes.len(), 0);
    }

    #[tokio::test]
    async fn test_pipeline_executor_start() {
        let engine = create_test_engine().await;
        let executor = PipelineExecutor::new(engine);
        let result = executor.start().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_pipeline_executor_cancel() {
        let engine = create_test_engine().await;
        let executor = PipelineExecutor::new(engine);
        let result = executor.cancel().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_pipeline_executor_assign_job() {
        let engine = create_test_engine().await;
        let executor = PipelineExecutor::new(engine);
        let job_id = JobId::new();
        let runner_id = RunnerId::new();
        // Assigning non-existent job should be no-op (returns Ok)
        let result = executor.assign_job(job_id, runner_id).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_pipeline_executor_start_job() {
        let engine = create_test_engine().await;
        let executor = PipelineExecutor::new(engine);
        let job_id = JobId::new();
        let result = executor.start_job(job_id).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_pipeline_executor_succeed_job() {
        let engine = create_test_engine().await;
        let executor = PipelineExecutor::new(engine);
        let job_id = JobId::new();
        let result = executor.succeed_job(job_id, 0).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_pipeline_executor_fail_job() {
        let engine = create_test_engine().await;
        let executor = PipelineExecutor::new(engine);
        let job_id = JobId::new();
        let result = executor.fail_job(job_id, 1, "test error".to_string()).await;
        assert!(result.is_ok());
    }
}

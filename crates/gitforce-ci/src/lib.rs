//! GitForce CI Orchestrator
//!
//! Pipeline parsing, DAG building, and job execution orchestration.

pub mod dag;
pub mod engine;
pub mod executor;
pub mod pipeline;
pub mod state;

pub use dag::{DagBuilder, JobGraph, JobNode};
pub use engine::CiEngine;
pub use executor::PipelineExecutor;
pub use pipeline::{PipelineDefinition, PipelineTriggerEvent, TriggerType, JobDefinition, StepDefinition};
pub use state::JobStateMachine;

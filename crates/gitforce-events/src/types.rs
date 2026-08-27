//! Event type aliases for convenience
//!
//! Re-exports all event types from the event module.

pub use crate::event::{
    ArtifactCreatedPayload, EventEnvelope, EventPayload, EventType, JobFinishedPayload,
    JobQueuedPayload, JobStartedPayload, MirrorSyncCompletedPayload, MirrorSyncRequestedPayload,
    PipelineFinishedPayload, PipelineStartedPayload, PipelineTriggeredPayload, PushReceivedPayload,
    RefUpdatedPayload, RepoCreatedPayload, RepoDeletedPayload, RunnerHeartbeatPayload,
    RunnerOfflinePayload, RunnerRegisteredPayload,
};

//! Event type aliases for convenience
//!
//! Re-exports all event types from the event module.

pub use crate::event::{
    EventEnvelope, EventPayload, EventType,
    RepoCreatedPayload, RepoDeletedPayload,
    PushReceivedPayload, RefUpdatedPayload,
    PipelineTriggeredPayload, PipelineStartedPayload, PipelineFinishedPayload,
    JobQueuedPayload, JobStartedPayload, JobFinishedPayload,
    ArtifactCreatedPayload,
    RunnerRegisteredPayload, RunnerHeartbeatPayload, RunnerOfflinePayload,
    MirrorSyncRequestedPayload, MirrorSyncCompletedPayload,
};

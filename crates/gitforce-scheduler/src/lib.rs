//! GitForce Scheduler
//!
//! Job queue management, runner assignment, and scheduling policies.

pub mod assigner;
pub mod auth;
pub mod policy;
pub mod queue;
pub mod server;

pub use assigner::Scheduler;
pub use auth::{with_auth, SchedulerAuthState};
pub use policy::{SchedulingPolicy, SimplePolicy};
pub use queue::{JobQueue, Priority};
pub use server::{
    create_state, create_state_with_pool, scheduler_routes, start_claim_reaper,
    SchedulerServerState,
};

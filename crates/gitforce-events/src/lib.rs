//! GitForce Event System
//!
//! Event bus and event type definitions for GitForge.

pub mod bus;
pub mod event;
pub mod serializer;
pub mod types;
pub mod webhook;

pub use bus::{EventBus, EventFilter, EventStream, InMemoryEventBus};
pub use event::EventEnvelope;
pub use serializer::EventSerializer;
pub use types::*;
pub use webhook::{WebhookError, WebhookEvent, WebhookManager, WebhookPayload, WebhookSender};

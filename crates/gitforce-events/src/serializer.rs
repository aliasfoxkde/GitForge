//! Event serialization

use super::event::EventEnvelope;
use serde::{Deserialize, Serialize};

/// JSON serializer for events
pub struct EventSerializer;

impl EventSerializer {
    /// Serialize an event to JSON bytes
    pub fn serialize(event: &EventEnvelope) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(event)
    }

    /// Serialize an event to a JSON string
    pub fn serialize_to_string(event: &EventEnvelope) -> Result<String, serde_json::Error> {
        serde_json::to_string(event)
    }

    /// Deserialize an event from JSON bytes
    pub fn deserialize(bytes: &[u8]) -> Result<EventEnvelope, serde_json::Error> {
        serde_json::from_slice(bytes)
    }

    /// Deserialize an event from a JSON string
    pub fn deserialize_from_str(s: &str) -> Result<EventEnvelope, serde_json::Error> {
        serde_json::from_str(s)
    }
}

/// JSON representation for external systems
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonEvent {
    pub event_id: String,
    pub event_type: String,
    pub event_version: u8,
    pub timestamp: i64,
    pub repo_id: Option<String>,
    pub actor_id: Option<String>,
    pub correlation_id: Option<String>,
    pub payload: serde_json::Value,
}

impl JsonEvent {
    /// Convert from EventEnvelope
    pub fn from_envelope(event: &EventEnvelope) -> Self {
        Self {
            event_id: event.event_id.to_string(),
            event_type: event.event_type.to_string(),
            event_version: event.event_version,
            timestamp: event.timestamp,
            repo_id: event.repo_id.map(|id| id.to_string()),
            actor_id: event.actor_id.map(|id| id.to_string()),
            correlation_id: event.correlation_id.map(|id| id.to_string()),
            payload: serde_json::to_value(&event.payload).unwrap_or_default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{PushReceivedPayload, EventType, EventPayload};

    #[test]
    fn test_serialize_roundtrip() {
        let event = EventEnvelope::new(
            EventType::PushReceived,
            EventPayload::PushReceived(PushReceivedPayload {
                repo_id: gitforce_common::RepoId::new(),
                ref_name: "refs/heads/main".to_string(),
                old_hash: "abc123".to_string(),
                new_hash: "def456".to_string(),
                pusher_id: None,
            }),
            None,
            None,
        );

        let json = EventSerializer::serialize_to_string(&event).unwrap();
        let parsed: EventEnvelope = EventSerializer::deserialize_from_str(&json).unwrap();

        assert_eq!(event.event_id, parsed.event_id);
        assert_eq!(event.event_type, parsed.event_type);
    }
}

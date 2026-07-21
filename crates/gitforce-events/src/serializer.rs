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
    use crate::event::{PushReceivedPayload, EventType, EventPayload, PipelineStartedPayload};

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

    #[test]
    fn test_serialize_to_bytes() {
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

        let bytes = EventSerializer::serialize(&event).unwrap();
        assert!(!bytes.is_empty());

        let parsed: EventEnvelope = EventSerializer::deserialize(&bytes).unwrap();
        assert_eq!(event.event_id, parsed.event_id);
    }

    #[test]
    fn test_json_event_from_envelope() {
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

        let json_event = JsonEvent::from_envelope(&event);
        assert_eq!(json_event.event_type, "push.received");
        assert_eq!(json_event.event_version, 1);
    }

    #[test]
    fn test_json_event_from_envelope_with_actor() {
        let actor_id = gitforce_common::UserId::new();
        let repo_id = gitforce_common::RepoId::new();
        let event = EventEnvelope::new(
            EventType::PushReceived,
            EventPayload::PushReceived(PushReceivedPayload {
                repo_id,
                ref_name: "refs/heads/main".to_string(),
                old_hash: "abc123".to_string(),
                new_hash: "def456".to_string(),
                pusher_id: Some(actor_id),
            }),
            Some(repo_id),
            Some(actor_id),
        );

        let json_event = JsonEvent::from_envelope(&event);
        assert!(json_event.actor_id.is_some());
        assert!(json_event.repo_id.is_some());
        assert!(json_event.correlation_id.is_none());
    }

    #[test]
    fn test_json_event_from_envelope_with_correlation() {
        let correlation_id = uuid::Uuid::new_v4();
        let event = EventEnvelope::new(
            EventType::PipelineStarted,
            EventPayload::PipelineStarted(PipelineStartedPayload {
                pipeline_run_id: gitforce_common::PipelineRunId::new(),
                started_at: chrono::Utc::now().timestamp(),
            }),
            None,
            None,
        ).with_correlation(correlation_id);

        let json_event = JsonEvent::from_envelope(&event);
        assert!(json_event.correlation_id.is_some());
    }

    #[test]
    fn test_json_event_serialization_roundtrip() {
        let event = EventEnvelope::new(
            EventType::PushReceived,
            EventPayload::PushReceived(PushReceivedPayload {
                repo_id: gitforce_common::RepoId::new(),
                ref_name: "refs/heads/develop".to_string(),
                old_hash: "old123".to_string(),
                new_hash: "new456".to_string(),
                pusher_id: None,
            }),
            None,
            None,
        );

        let json_event = JsonEvent::from_envelope(&event);
        let json_str = serde_json::to_string(&json_event).unwrap();
        assert!(json_str.contains("push_received"));
        assert!(json_str.contains("event_version"));
    }

    #[test]
    fn test_json_event_debug_trait() {
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

        let json_event = JsonEvent::from_envelope(&event);
        let debug_str = format!("{:?}", json_event);
        assert!(debug_str.contains("event_id"));
        assert!(debug_str.contains("event_type"));
    }

    #[test]
    fn test_serializer_deserialize_invalid_json() {
        let invalid_json = b"not valid json";
        let result: Result<EventEnvelope, _> = EventSerializer::deserialize(invalid_json);
        assert!(result.is_err());
    }

    #[test]
    fn test_serializer_deserialize_from_str_invalid() {
        let result: Result<EventEnvelope, _> = EventSerializer::deserialize_from_str("invalid json");
        assert!(result.is_err());
    }

    #[test]
    fn test_json_event_all_event_types() {
        let event_types = vec![
            EventType::PushReceived,
            EventType::RepoCreated,
            EventType::RepoDeleted,
            EventType::PipelineTriggered,
            EventType::PipelineStarted,
            EventType::PipelineFinished,
            EventType::JobQueued,
            EventType::JobStarted,
            EventType::JobFinished,
        ];

        for event_type in event_types {
            let event = EventEnvelope::new(
                event_type.clone(),
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
            let json_event = JsonEvent::from_envelope(&event);
            assert!(!json_event.event_type.is_empty());
        }
    }
}

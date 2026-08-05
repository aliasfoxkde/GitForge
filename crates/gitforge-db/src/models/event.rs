//! Event model

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Event entity (append-only log)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub id: Uuid,
    pub event_type: String,
    pub payload: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

impl Event {
    /// Create a new event
    pub fn new(event_type: String, payload: serde_json::Value) -> Self {
        Self {
            id: Uuid::new_v4(),
            event_type,
            payload,
            created_at: Utc::now(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_creation() {
        let event = Event::new(
            "push".to_string(),
            serde_json::json!({"ref": "refs/heads/main"}),
        );
        assert_eq!(event.event_type, "push");
    }

    #[test]
    fn test_event_id_unique() {
        let event1 = Event::new("push".to_string(), serde_json::json!({}));
        let event2 = Event::new("push".to_string(), serde_json::json!({}));
        assert_ne!(event1.id, event2.id);
    }

    #[test]
    fn test_event_payload() {
        let payload = serde_json::json!({"key": "value", "number": 42});
        let event = Event::new("test".to_string(), payload.clone());
        assert_eq!(event.payload, payload);
    }
}

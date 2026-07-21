//! Webhook system for GitForge
//!
//! Supports sending webhook notifications to external services.

use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Webhook event types
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum WebhookEvent {
    /// Repository events
    RepoCreated,
    RepoDeleted,
    RepoPushed,

    /// Pipeline events
    PipelineStarted,
    PipelineCompleted,
    PipelineFailed,

    /// Job events
    JobStarted,
    JobCompleted,
    JobFailed,

    /// Runner events
    RunnerRegistered,
    RunnerOffline,
}

/// Webhook payload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookPayload {
    pub event: WebhookEvent,
    pub timestamp: String,
    pub data: serde_json::Value,
}

impl WebhookPayload {
    /// Create a new webhook payload
    pub fn new(event: WebhookEvent, data: impl Serialize) -> Self {
        Self {
            event,
            timestamp: chrono::Utc::now().to_rfc3339(),
            data: serde_json::to_value(data).unwrap_or_default(),
        }
    }
}

/// Webhook configuration
#[derive(Debug, Clone)]
pub struct WebhookConfig {
    pub url: String,
    pub secret: Option<String>,
    pub enabled: bool,
    pub events: Vec<WebhookEvent>,
}

impl Default for WebhookConfig {
    fn default() -> Self {
        Self {
            url: String::new(),
            secret: None,
            enabled: false,
            events: vec![
                WebhookEvent::PipelineCompleted,
                WebhookEvent::PipelineFailed,
            ],
        }
    }
}

/// Webhook sender trait
#[async_trait]
pub trait WebhookSender: Send + Sync {
    /// Send a webhook payload
    async fn send(&self, payload: &WebhookPayload) -> Result<(), WebhookError>;
}

/// HTTP webhook sender
pub struct HttpWebhookSender {
    client: Client,
    url: String,
    secret: Option<String>,
}

impl HttpWebhookSender {
    /// Create a new HTTP webhook sender
    pub fn new(url: &str, secret: Option<&str>) -> Self {
        Self {
            client: Client::new(),
            url: url.to_string(),
            secret: secret.map(String::from),
        }
    }

    /// Create from webhook config
    pub fn from_config(config: &WebhookConfig) -> Option<Self> {
        if config.enabled {
            Some(Self::new(&config.url, config.secret.as_deref()))
        } else {
            None
        }
    }

    /// Generate signature for payload
    fn generate_signature(&self, payload: &[u8]) -> Option<String> {
        use sha2::{Sha256, Digest};

        let secret = self.secret.as_ref()?;
        let mut hasher = Sha256::new();
        hasher.update(secret.as_bytes());
        hasher.update(payload);
        let result = hasher.finalize();
        Some(format!("sha256={}", hex::encode(result)))
    }
}

#[async_trait]
impl WebhookSender for HttpWebhookSender {
    async fn send(&self, payload: &WebhookPayload) -> Result<(), WebhookError> {
        let json = serde_json::to_vec(payload)
            .map_err(|e| WebhookError::Serialization(e.to_string()))?;

        let mut request = self.client.post(&self.url);

        // Add signature header if secret is configured
        if let Some(sig) = self.generate_signature(&json) {
            request = request.header("X-GitForge-Signature", sig);
        }

        request
            .header("Content-Type", "application/json")
            .header("X-GitForge-Event", serde_json::to_string(&payload.event).unwrap_or_default())
            .body(json)
            .send()
            .await
            .map_err(|e| WebhookError::Network(e.to_string()))?;

        Ok(())
    }
}

/// Webhook manager for handling multiple webhooks
pub struct WebhookManager {
    senders: Arc<RwLock<Vec<Box<dyn WebhookSender>>>>,
    event_filters: HashMap<WebhookEvent, bool>,
}

impl WebhookManager {
    /// Create a new webhook manager
    pub fn new() -> Self {
        Self {
            senders: Arc::new(RwLock::new(Vec::new())),
            event_filters: HashMap::new(),
        }
    }

    /// Add a webhook sender
    pub async fn add_webhook(&self, sender: Box<dyn WebhookSender>) {
        let mut senders = self.senders.write().await;
        senders.push(sender);
    }

    /// Configure which events to send
    pub fn set_event_filter(&mut self, event: WebhookEvent, enabled: bool) {
        self.event_filters.insert(event, enabled);
    }

    /// Check if an event should be sent
    fn should_send(&self, event: &WebhookEvent) -> bool {
        self.event_filters.get(event).copied().unwrap_or(true)
    }

    /// Send a webhook event to all registered webhooks
    pub async fn send(&self, payload: &WebhookPayload) {
        if !self.should_send(&payload.event) {
            tracing::debug!("webhook event {:?} filtered out", payload.event);
            return;
        }

        let senders = self.senders.read().await;
        for sender in senders.iter() {
            if let Err(e) = sender.send(payload).await {
                tracing::error!("failed to send webhook: {:?}", e);
            }
        }
    }

    /// Send a simple event with data
    pub async fn send_event<T: Serialize>(&self, event: WebhookEvent, data: &T) {
        let payload = WebhookPayload::new(event, data);
        self.send(&payload).await;
    }
}

impl Default for WebhookManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Webhook error types
#[derive(Debug, thiserror::Error)]
pub enum WebhookError {
    #[error("network error: {0}")]
    Network(String),

    #[error("serialization error: {0}")]
    Serialization(String),

    #[error("configuration error: {0}")]
    Config(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_webhook_payload_creation() {
        let payload = WebhookPayload::new(
            WebhookEvent::PipelineCompleted,
            serde_json::json!({"pipeline_id": "123"}),
        );

        assert_eq!(payload.event, WebhookEvent::PipelineCompleted);
        assert!(!payload.timestamp.is_empty());
        assert_eq!(payload.data["pipeline_id"], "123");
    }

    #[test]
    fn test_webhook_event_serialization() {
        let event = WebhookEvent::PipelineCompleted;
        let serialized = serde_json::to_string(&event).unwrap();
        assert!(serialized.contains("pipeline_completed"));

        let deserialized: WebhookEvent = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized, event);
    }

    #[tokio::test]
    async fn test_webhook_manager_filters() {
        let mut manager = WebhookManager::new();
        manager.set_event_filter(WebhookEvent::PipelineCompleted, false);

        assert!(!manager.should_send(&WebhookEvent::PipelineCompleted));
        assert!(manager.should_send(&WebhookEvent::PipelineFailed));
    }

    #[test]
    fn test_http_webhook_sender_signature() {
        let sender = HttpWebhookSender::new("http://example.com/webhook", Some("secret"));
        let payload = b"test payload";
        let sig = sender.generate_signature(payload);
        assert!(sig.is_some());
        assert!(sig.unwrap().starts_with("sha256="));
    }

    #[test]
    fn test_webhook_config_default() {
        let config = WebhookConfig::default();
        assert!(!config.enabled);
        assert!(config.url.is_empty());
        assert!(config.secret.is_none());
    }
}

//! Event bus implementation

use super::event::{EventEnvelope, EventType};
use async_trait::async_trait;
use futures::Stream;
use gitforce_common::Result;
use std::pin::Pin;
use tokio::sync::broadcast;

/// Event filter for subscriptions
#[derive(Debug, Clone, Default)]
pub struct EventFilter {
    /// Filter by event types
    pub event_types: Option<Vec<EventType>>,
    /// Filter by repository ID
    pub repo_id: Option<gitforce_common::RepoId>,
}

impl EventFilter {
    /// Create a filter that matches all events
    pub fn all() -> Self {
        Self::default()
    }

    /// Create a filter for specific event types
    pub fn for_types(event_types: Vec<EventType>) -> Self {
        Self {
            event_types: Some(event_types),
            repo_id: None,
        }
    }

    /// Create a filter for a specific repository
    pub fn for_repo(repo_id: gitforce_common::RepoId) -> Self {
        Self {
            event_types: None,
            repo_id: Some(repo_id),
        }
    }

    /// Check if an event matches this filter
    pub fn matches(&self, event: &EventEnvelope) -> bool {
        // Check event type
        if let Some(ref types) = self.event_types {
            if !types.contains(&event.event_type) {
                return false;
            }
        }

        // Check repo_id
        if let Some(ref repo_id) = self.repo_id {
            if event.repo_id != Some(*repo_id) {
                return false;
            }
        }

        true
    }
}

/// Event bus trait
#[async_trait]
pub trait EventBus: Send + Sync {
    /// Publish an event to the bus
    async fn publish(&self, event: EventEnvelope) -> Result<()>;

    /// Subscribe to events matching a filter
    async fn subscribe(&self, filter: EventFilter) -> Result<Box<dyn EventStream>>;
}

/// Event stream trait
#[async_trait]
pub trait EventStream: Send + Sync + Stream<Item = EventEnvelope> + Unpin {
    /// Get the filter for this stream
    fn filter(&self) -> &EventFilter;
}

/// In-memory event bus implementation using broadcast channels
pub struct InMemoryEventBus {
    sender: broadcast::Sender<EventEnvelope>,
}

impl InMemoryEventBus {
    /// Create a new in-memory event bus
    pub fn new() -> Self {
        let (sender, _) = broadcast::channel(1024);
        Self { sender }
    }

    /// Get the number of active subscribers
    pub async fn subscriber_count(&self) -> usize {
        0
    }
}

impl Default for InMemoryEventBus {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl EventBus for InMemoryEventBus {
    async fn publish(&self, event: EventEnvelope) -> Result<()> {
        self.sender
            .send(event)
            .map_err(|e| gitforce_common::Error::event_system(format!("publish failed: {}", e)))?;

        Ok(())
    }

    async fn subscribe(&self, filter: EventFilter) -> Result<Box<dyn EventStream>> {
        let rx = self.sender.subscribe();

        Ok(Box::new(InMemoryEventStream {
            rx,
            filter,
        }))
    }
}

/// In-memory event stream
struct InMemoryEventStream {
    rx: broadcast::Receiver<EventEnvelope>,
    filter: EventFilter,
}

impl Stream for InMemoryEventStream {
    type Item = EventEnvelope;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        use std::task::Poll;

        loop {
            match self.rx.try_recv() {
                Ok(event) => {
                    if self.filter.matches(&event) {
                        return Poll::Ready(Some(event));
                    }
                    // Continue looking for matching event
                }
                Err(broadcast::error::TryRecvError::Empty) => {
                    // No message available, register waker and return pending
                    let waker = cx.waker().clone();
                    std::thread::spawn(move || {
                        std::thread::sleep(std::time::Duration::from_millis(10));
                        waker.wake();
                    });
                    return Poll::Pending;
                }
                Err(broadcast::error::TryRecvError::Closed) => return Poll::Ready(None),
                Err(broadcast::error::TryRecvError::Lagged(_)) => {
                    // Subscriber fell behind, continue with next event
                }
            }
        }
    }
}

#[async_trait]
impl EventStream for InMemoryEventStream {
    fn filter(&self) -> &EventFilter {
        &self.filter
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{PushReceivedPayload, EventType, EventPayload};

    #[tokio::test]
    async fn test_publish_subscribe() {
        let bus = InMemoryEventBus::new();

        // Subscribe to push events
        let stream = bus
            .subscribe(EventFilter::for_types(vec![EventType::PushReceived]))
            .await
            .unwrap();

        // Pin the stream - tokio::pin! handles the boxing correctly
        tokio::pin!(stream);

        // Publish an event
        let event = EventEnvelope::new(
            EventType::PushReceived,
            EventPayload::PushReceived(PushReceivedPayload {
                repo_id: gitforce_common::RepoId::new(),
                ref_name: "refs/heads/main".to_string(),
                old_hash: "abc".to_string(),
                new_hash: "def".to_string(),
                pusher_id: None,
            }),
            None,
            None,
        );

        bus.publish(event.clone()).await.unwrap();

        // Receive should get the event
        let received = tokio::time::timeout(std::time::Duration::from_secs(1), futures::StreamExt::next(&mut stream))
            .await
            .unwrap();

        assert!(received.is_some());
    }
}

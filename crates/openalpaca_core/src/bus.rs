use crate::events::SystemEvent;
use tokio::sync::broadcast;

/// The central nervous system of OpenAlpaca.
/// A typed Event Bus using tokio::broadcast.
#[derive(Clone)]
pub struct EventBus {
    sender: broadcast::Sender<SystemEvent>,
}

impl EventBus {
    /// Create a new EventBus with a default capacity.
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Self { sender }
    }

    /// Subscribe to the event stream.
    pub fn subscribe(&self) -> broadcast::Receiver<SystemEvent> {
        self.sender.subscribe()
    }

    /// Publish an event to all subscribers.
    /// Returns the number of subscribers triggered.
    pub fn publish(&self, event: SystemEvent) -> usize {
        // We ignore the error if there are no subscribers,
        // but we might want to log it if it's critical.
        // For now, implicit drop is fine.
        match self.sender.send(event) {
            Ok(count) => count,
            Err(_) => 0, // No active receivers
        }
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new(1024)
    }
}

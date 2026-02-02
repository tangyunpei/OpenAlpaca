//! Server events for WebSocket streaming
//!
//! All events include instance_id for client validation.

use chrono::{DateTime, Utc};
use serde::Serialize;
use tokio::sync::broadcast;

/// Server events pushed to clients via WebSocket
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerEvent {
    Heartbeat {
        ts: DateTime<Utc>,
        instance_id: String,
    },
    Log {
        level: String,
        message: String,
        ts: DateTime<Utc>,
        instance_id: String,
    },
    CommandReceived {
        request_id: String,
        command: String,
        ts: DateTime<Utc>,
        instance_id: String,
    },
}

/// Event broadcaster for pushing events to all connected clients
#[derive(Clone)]
pub struct EventBroadcaster {
    tx: broadcast::Sender<ServerEvent>,
    instance_id: String,
}

impl EventBroadcaster {
    /// Create a new event broadcaster with given capacity
    pub fn new(capacity: usize, instance_id: String) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self { tx, instance_id }
    }

    /// Subscribe to events
    pub fn subscribe(&self) -> broadcast::Receiver<ServerEvent> {
        self.tx.subscribe()
    }

    /// Broadcast a heartbeat event
    pub fn heartbeat(&self) {
        let _ = self.tx.send(ServerEvent::Heartbeat {
            ts: Utc::now(),
            instance_id: self.instance_id.clone(),
        });
    }

    /// Broadcast a log event
    pub fn log(&self, level: &str, message: &str) {
        let _ = self.tx.send(ServerEvent::Log {
            level: level.to_string(),
            message: message.to_string(),
            ts: Utc::now(),
            instance_id: self.instance_id.clone(),
        });
    }

    /// Broadcast a command received event
    pub fn command_received(&self, request_id: &str, command: &str) {
        let _ = self.tx.send(ServerEvent::CommandReceived {
            request_id: request_id.to_string(),
            command: command.to_string(),
            ts: Utc::now(),
            instance_id: self.instance_id.clone(),
        });
    }
}

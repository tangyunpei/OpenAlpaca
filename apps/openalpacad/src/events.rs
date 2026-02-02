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

use openalpaca_storage::{Database, repository::EventLogRepository};

/// Event broadcaster for pushing events to all connected clients and persisting to DB
#[derive(Clone)]
pub struct EventBroadcaster {
    tx: broadcast::Sender<ServerEvent>,
    instance_id: String,
    db: Option<Database>,
}

impl EventBroadcaster {
    /// Create a new event broadcaster with given capacity
    pub fn new(capacity: usize, instance_id: String, db: Option<Database>) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self {
            tx,
            instance_id,
            db,
        }
    }

    /// Subscribe to events
    pub fn subscribe(&self) -> broadcast::Receiver<ServerEvent> {
        self.tx.subscribe()
    }

    /// Broadcast a heartbeat event (not persisted)
    pub fn heartbeat(&self) {
        let _ = self.tx.send(ServerEvent::Heartbeat {
            ts: Utc::now(),
            instance_id: self.instance_id.clone(),
        });
    }

    /// Broadcast a log event and persist it
    pub fn log(&self, level: &str, message: &str) {
        let event = ServerEvent::Log {
            level: level.to_string(),
            message: message.to_string(),
            ts: Utc::now(),
            instance_id: self.instance_id.clone(),
        };

        self.persist(&event);
        let _ = self.tx.send(event);
    }

    /// Broadcast a command received event and persist it
    pub fn command_received(&self, request_id: &str, command: &str) {
        let event = ServerEvent::CommandReceived {
            request_id: request_id.to_string(),
            command: command.to_string(),
            ts: Utc::now(),
            instance_id: self.instance_id.clone(),
        };

        self.persist(&event);
        let _ = self.tx.send(event);
    }

    /// Persist important events to the database
    fn persist(&self, event: &ServerEvent) {
        if let Some(db) = &self.db {
            let repo = EventLogRepository::new(db);
            let _ = match event {
                ServerEvent::Heartbeat { .. } => Ok(0), // Skip heartbeats
                ServerEvent::Log { level, message, .. } => {
                    let detail = serde_json::json!({
                        "level": level,
                        "message": message
                    });
                    repo.log("log", None, Some(&detail), None)
                }
                ServerEvent::CommandReceived {
                    request_id,
                    command,
                    ..
                } => {
                    let detail = serde_json::json!({
                        "request_id": request_id,
                        "command": command
                    });
                    repo.log("command_received", None, Some(&detail), None)
                }
            };
            // Error handling strategy: log errors but don't crash or block broadcast
            // For now we just ignore errors as per architecture plan
        }
    }
}

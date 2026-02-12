//! Server events for WebSocket streaming
//!
//! All events include instance_id for client validation.

use chrono::Utc;
use tokio::sync::broadcast;

use openalpaca_api::events::ServerEvent;
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

    /// Broadcast a Wake event and persist it
    pub fn wake(&self, wake_event: openalpaca_api::events::WakeEvent) {
        let event = ServerEvent::Wake {
            wake: wake_event,
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
                // Wake events are persisted by the same mechanism
                ServerEvent::Wake { wake, .. } => {
                    let detail = serde_json::json!({
                        "wake_event": wake
                    });
                    repo.log("wake", None, Some(&detail), None)
                }
                // Log connector status changes
                ServerEvent::ConnectorStatus { id, status, .. } => {
                    let detail = serde_json::json!({
                        "connector_id": id,
                        "status": status
                    });
                    repo.log("agent_status", None, Some(&detail), None)
                }
                // Log agent status changes
                ServerEvent::AgentStatus {
                    agent_id,
                    name,
                    status,
                    current_task_id,
                    ..
                } => {
                    let detail = serde_json::json!({
                        "agent_id": agent_id,
                        "name": name,
                        "status": status,
                        "current_task_id": current_task_id
                    });
                    repo.log("agent_status_change", None, Some(&detail), None)
                }
                // Log task status changes
                ServerEvent::TaskStatus {
                    task_id,
                    status,
                    progress_current,
                    progress_total,
                    result_summary,
                    ..
                } => {
                    let detail = serde_json::json!({
                        "task_id": task_id,
                        "status": status,
                        "progress_current": progress_current,
                        "progress_total": progress_total,
                        "result_summary": result_summary
                    });
                    repo.log("task_status", None, Some(&detail), None)
                }
                // Log key status changes
                ServerEvent::KeyStatusChanged {
                    provider,
                    key_id,
                    status,
                    ..
                } => {
                    let detail = serde_json::json!({
                        "provider": provider,
                        "key_id": key_id,
                        "status": status
                    });
                    repo.log("key_status_changed", None, Some(&detail), None)
                }
                // Log chat stream events
                ServerEvent::ChatStreamStarted {
                    stream_id,
                    lane_key,
                    ..
                } => {
                    let detail = serde_json::json!({
                        "stream_id": stream_id,
                        "lane_key": lane_key
                    });
                    repo.log("chat_stream_started", None, Some(&detail), None)
                }
                ServerEvent::ChatStreamEnded {
                    stream_id,
                    lane_key,
                    status,
                    ..
                } => {
                    let detail = serde_json::json!({
                        "stream_id": stream_id,
                        "lane_key": lane_key,
                        "status": status
                    });
                    repo.log("chat_stream_ended", None, Some(&detail), None)
                }
                ServerEvent::AgentConfigChanged {
                    agent_id,
                    action,
                    config_version,
                    ..
                } => {
                    let detail = serde_json::json!({
                        "agent_id": agent_id,
                        "action": action,
                        "config_version": config_version
                    });
                    repo.log("agent_config_changed", None, Some(&detail), None)
                }
                ServerEvent::OrchestratorConfigChanged { model, .. } => {
                    let detail = serde_json::json!({
                        "model": model
                    });
                    repo.log("orchestrator_config_changed", None, Some(&detail), None)
                }
                // Log SOUL.md personality updates with actor attribution
                ServerEvent::SoulUpdated {
                    actor,
                    mode,
                    content_sha256,
                    backup_path,
                    ..
                } => {
                    let detail = serde_json::json!({
                        "actor": actor,
                        "mode": mode,
                        "content_sha256": content_sha256,
                        "backup_path": backup_path
                    });
                    repo.log("soul_updated", None, Some(&detail), None)
                }
            };
            // Error handling strategy: log errors but don't crash or block broadcast
            // For now we just ignore errors as per architecture plan
        }
    }

    /// Broadcast a task status event and persist it
    pub fn task_status(
        &self,
        task_id: &str,
        title: &str,
        status: &str,
        progress_current: Option<i32>,
        progress_total: Option<i32>,
        result_summary: Option<String>,
    ) {
        let event = ServerEvent::TaskStatus {
            task_id: task_id.to_string(),
            title: title.to_string(),
            status: status.to_string(),
            progress_current,
            progress_total,
            result_summary,
            ts: Utc::now(),
            instance_id: self.instance_id.clone(),
        };

        self.persist(&event);
        let _ = self.tx.send(event);
    }

    /// Broadcast an agent status event and persist it
    pub fn agent_status(
        &self,
        agent_id: &str,
        name: &str,
        status: &str,
        current_task_id: Option<String>,
    ) {
        let event = ServerEvent::AgentStatus {
            agent_id: agent_id.to_string(),
            name: name.to_string(),
            status: status.to_string(),
            current_task_id,
            ts: Utc::now(),
            instance_id: self.instance_id.clone(),
        };

        self.persist(&event);
        let _ = self.tx.send(event);
    }

    /// Broadcast a key status change event and persist it
    pub fn key_status_changed(&self, provider: &str, key_id: &str, status: &str) {
        let event = ServerEvent::KeyStatusChanged {
            provider: provider.to_string(),
            key_id: key_id.to_string(),
            status: status.to_string(),
            ts: Utc::now(),
            instance_id: self.instance_id.clone(),
        };

        self.persist(&event);
        let _ = self.tx.send(event);
    }

    /// Broadcast a connector status change and persist it
    pub fn connector_status(&self, id: &str, status: &str) {
        let event = ServerEvent::ConnectorStatus {
            id: id.to_string(),
            status: status.to_string(),
            ts: Utc::now(),
            instance_id: self.instance_id.clone(),
        };

        self.persist(&event);
        let _ = self.tx.send(event);
    }

    /// Broadcast a chat stream started event and persist it
    pub fn chat_stream_started(&self, stream_id: &str, lane_key: &str) {
        let event = ServerEvent::ChatStreamStarted {
            stream_id: stream_id.to_string(),
            lane_key: lane_key.to_string(),
            ts: Utc::now(),
            instance_id: self.instance_id.clone(),
        };

        self.persist(&event);
        let _ = self.tx.send(event);
    }

    /// Broadcast an agent config changed event and persist it
    pub fn agent_config_changed(&self, agent_id: &str, action: &str, config_version: u64) {
        let event = ServerEvent::AgentConfigChanged {
            agent_id: agent_id.to_string(),
            action: action.to_string(),
            config_version,
            ts: Utc::now(),
            instance_id: self.instance_id.clone(),
        };

        self.persist(&event);
        let _ = self.tx.send(event);
    }

    /// Broadcast an orchestrator config changed event and persist it
    pub fn orchestrator_config_changed(&self, model: &str) {
        let event = ServerEvent::OrchestratorConfigChanged {
            model: model.to_string(),
            ts: Utc::now(),
            instance_id: self.instance_id.clone(),
        };

        self.persist(&event);
        let _ = self.tx.send(event);
    }

    /// Broadcast a chat stream ended event and persist it
    pub fn chat_stream_ended(&self, stream_id: &str, lane_key: &str, status: &str) {
        let event = ServerEvent::ChatStreamEnded {
            stream_id: stream_id.to_string(),
            lane_key: lane_key.to_string(),
            status: status.to_string(),
            ts: Utc::now(),
            instance_id: self.instance_id.clone(),
        };

        self.persist(&event);
        let _ = self.tx.send(event);
    }

    /// Broadcast a SOUL.md update event and persist it
    pub fn soul_updated(
        &self,
        actor: &str,
        mode: &str,
        content_sha256: &str,
        backup_path: Option<String>,
    ) {
        let event = ServerEvent::SoulUpdated {
            actor: actor.to_string(),
            mode: mode.to_string(),
            content_sha256: content_sha256.to_string(),
            backup_path,
            ts: Utc::now(),
            instance_id: self.instance_id.clone(),
        };

        self.persist(&event);
        let _ = self.tx.send(event);
    }
}

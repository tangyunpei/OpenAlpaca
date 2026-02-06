//! ChatStreamManager — SSE stream lifecycle management
//!
//! Manages broadcast channels for chat streaming. Each active chat request
//! gets a unique stream_id with a broadcast channel for SSE delivery.

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};
use tokio::sync::broadcast;
use uuid::Uuid;

/// Events sent over an SSE chat stream.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum ChatStreamEvent {
    Thinking,
    Delta { content: String },
    Done {
        content: String,
        model: String,
        tokens_in: u64,
        tokens_out: u64,
        duration_ms: u64,
    },
    Error { message: String },
}

struct StreamEntry {
    tx: broadcast::Sender<ChatStreamEvent>,
    created_at: Instant,
    #[allow(dead_code)]
    lane_key: String,
}

/// Manages active SSE chat streams.
pub struct ChatStreamManager {
    streams: DashMap<String, StreamEntry>,
}

impl ChatStreamManager {
    pub fn new() -> Self {
        Self {
            streams: DashMap::new(),
        }
    }

    /// Create a new stream, returning (stream_id, receiver).
    pub fn create_stream(&self, lane_key: &str) -> (String, broadcast::Receiver<ChatStreamEvent>) {
        let stream_id = Uuid::new_v4().to_string();
        let (tx, rx) = broadcast::channel(32);
        self.streams.insert(
            stream_id.clone(),
            StreamEntry {
                tx,
                created_at: Instant::now(),
                lane_key: lane_key.to_string(),
            },
        );
        (stream_id, rx)
    }

    /// Get a receiver for an existing stream (for SSE endpoint).
    pub fn get_receiver(&self, stream_id: &str) -> Option<broadcast::Receiver<ChatStreamEvent>> {
        self.streams.get(stream_id).map(|entry| entry.tx.subscribe())
    }

    /// Send an event to a stream.
    pub fn send(&self, stream_id: &str, event: ChatStreamEvent) -> anyhow::Result<()> {
        let entry = self
            .streams
            .get(stream_id)
            .ok_or_else(|| anyhow::anyhow!("Stream not found: {stream_id}"))?;
        let _ = entry.tx.send(event);
        Ok(())
    }

    /// Remove a stream.
    pub fn remove(&self, stream_id: &str) {
        self.streams.remove(stream_id);
    }

    /// Remove streams older than `max_age`.
    pub fn cleanup_stale(&self, max_age: Duration) {
        let now = Instant::now();
        self.streams
            .retain(|_, entry| now.duration_since(entry.created_at) < max_age);
    }
}

impl Default for ChatStreamManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_and_get_receiver() {
        let mgr = ChatStreamManager::new();
        let (stream_id, _rx) = mgr.create_stream("user:gui");

        assert!(mgr.get_receiver(&stream_id).is_some());
        assert!(mgr.get_receiver("nonexistent").is_none());
    }

    #[test]
    fn test_send_and_receive() {
        let mgr = ChatStreamManager::new();
        let (stream_id, mut rx) = mgr.create_stream("user:gui");

        mgr.send(&stream_id, ChatStreamEvent::Thinking).unwrap();

        let event = rx.try_recv().unwrap();
        assert!(matches!(event, ChatStreamEvent::Thinking));
    }

    #[test]
    fn test_remove() {
        let mgr = ChatStreamManager::new();
        let (stream_id, _rx) = mgr.create_stream("user:gui");

        mgr.remove(&stream_id);
        assert!(mgr.get_receiver(&stream_id).is_none());
    }

    #[test]
    fn test_cleanup_stale() {
        let mgr = ChatStreamManager::new();
        let (stream_id, _rx) = mgr.create_stream("user:gui");

        // With zero-duration max_age, everything is stale
        mgr.cleanup_stale(Duration::ZERO);
        assert!(mgr.get_receiver(&stream_id).is_none());
    }

    #[test]
    fn test_cleanup_keeps_fresh() {
        let mgr = ChatStreamManager::new();
        let (stream_id, _rx) = mgr.create_stream("user:gui");

        // With large max_age, nothing is stale
        mgr.cleanup_stale(Duration::from_secs(3600));
        assert!(mgr.get_receiver(&stream_id).is_some());
    }
}

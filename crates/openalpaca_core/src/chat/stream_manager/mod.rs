//! ChatStreamManager — SSE stream lifecycle management
//!
//! Manages broadcast channels for chat streaming. Each active chat request
//! gets a unique stream_id with a broadcast channel for SSE delivery.

use crate::gateway::{DelegationInfo, SkippedAttachment};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::broadcast;
use uuid::Uuid;

/// Events sent over an SSE chat stream.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum ChatStreamEvent {
    /// The turn has started and no text has been produced yet — the
    /// placeholder a client shows as "thinking…". Still sent for a model that
    /// emits no reasoning of its own.
    Thinking,
    /// S2: the model's own reasoning, as it is produced.
    ///
    /// Shown live and thrown away: it is never persisted, never part of
    /// `Done.content`, and never replayed by the history route. A client
    /// renders it beside the thinking indicator and drops it when the answer
    /// starts.
    Reasoning {
        text: String,
    },
    Delta {
        content: String,
    },
    Done {
        content: String,
        model: String,
        tokens_in: u64,
        tokens_out: u64,
        duration_ms: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        attachments_used: Option<Vec<String>>,
        /// U3 — the turn's attachments that never reached the model, each with
        /// a reason. Omitted when there are none, like `attachments_used`.
        #[serde(skip_serializing_if = "Option::is_none")]
        attachments_skipped: Option<Vec<SkippedAttachment>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        delegation: Option<DelegationInfo>,
    },
    Error {
        message: String,
    },
    ConfirmationRequested {
        request_id: String,
        tool_name: String,
        tool_arguments: serde_json::Value,
    },
}

/// A cloneable handle for sending streaming events into a chat stream.
///
/// Created by `ChatStreamManager::create_stream()` and used by background tasks
/// to emit Thinking/Delta/Done/Error events without needing a reference to the manager.
/// Each send refreshes `last_active` so that `cleanup_stale()` won't GC active streams.
#[derive(Clone)]
pub struct StreamSink {
    stream_id: String,
    tx: broadcast::Sender<ChatStreamEvent>,
    last_active: Arc<Mutex<Instant>>,
}

impl StreamSink {
    /// Send an event and refresh the stream's last_active timestamp.
    fn send_event(&self, event: ChatStreamEvent) {
        let _ = self.tx.send(event);
        if let Ok(mut la) = self.last_active.lock() {
            *la = Instant::now();
        }
    }

    /// Send a Thinking event (call after client has subscribed).
    pub fn send_thinking(&self) {
        self.send_event(ChatStreamEvent::Thinking);
    }

    /// Send one piece of the model's reasoning (S2). Live only — nothing
    /// downstream keeps it.
    pub fn send_reasoning(&self, text: &str) {
        self.send_event(ChatStreamEvent::Reasoning {
            text: text.to_string(),
        });
    }

    /// Send a delta chunk of the response.
    pub fn send_delta(&self, content: &str) {
        self.send_event(ChatStreamEvent::Delta {
            content: content.to_string(),
        });
    }

    /// Send the final Done event with full content and metadata.
    pub fn send_done(
        &self,
        content: &str,
        model: &str,
        tokens_in: u64,
        tokens_out: u64,
        duration_ms: u64,
        delegation: Option<DelegationInfo>,
    ) {
        self.send_done_with_attachments(
            content,
            model,
            tokens_in,
            tokens_out,
            duration_ms,
            Vec::new(),
            Vec::new(),
            delegation,
        );
    }

    /// Send the final Done event with attachment info.
    ///
    /// U3: `attachments_skipped` travels beside `attachments_used` and is
    /// omitted from the frame the same way — a turn that withheld nothing is
    /// byte-identical to what it always was. A turn whose *every* attachment
    /// was withheld has an empty `used` and a non-empty `skipped`, which is
    /// why the two are one call rather than two.
    #[allow(clippy::too_many_arguments)]
    pub fn send_done_with_attachments(
        &self,
        content: &str,
        model: &str,
        tokens_in: u64,
        tokens_out: u64,
        duration_ms: u64,
        attachments_used: Vec<String>,
        attachments_skipped: Vec<SkippedAttachment>,
        delegation: Option<DelegationInfo>,
    ) {
        self.send_event(ChatStreamEvent::Done {
            content: content.to_string(),
            model: model.to_string(),
            tokens_in,
            tokens_out,
            duration_ms,
            attachments_used: (!attachments_used.is_empty()).then_some(attachments_used),
            attachments_skipped: (!attachments_skipped.is_empty()).then_some(attachments_skipped),
            delegation,
        });
    }

    /// Send an error event.
    pub fn send_error(&self, message: &str) {
        self.send_event(ChatStreamEvent::Error {
            message: message.to_string(),
        });
    }

    /// Get the stream ID this sink writes to.
    pub fn stream_id(&self) -> &str {
        &self.stream_id
    }
}

/// S1: an SSE chat stream is where a turn's live text goes. The provider's
/// deltas reach this sink from inside the agentic loop, one `Delta` frame
/// each, while the turn is still running.
impl crate::chat::turn_sink::TurnSink for StreamSink {
    fn text_delta(&self, text: &str) {
        self.send_delta(text);
    }

    fn reasoning_delta(&self, text: &str) {
        self.send_reasoning(text);
    }
}

struct StreamEntry {
    tx: broadcast::Sender<ChatStreamEvent>,
    #[allow(dead_code)]
    created_at: Instant,
    /// Updated on every send (via StreamSink or ChatStreamManager::send());
    /// used by cleanup_stale() to avoid GC'ing active streams.
    /// Shared with StreamSink via Arc so sink sends also refresh it.
    last_active: Arc<Mutex<Instant>>,
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

    /// Create a new stream, returning (stream_id, receiver, sink).
    pub fn create_stream(
        &self,
        lane_key: &str,
    ) -> (String, broadcast::Receiver<ChatStreamEvent>, StreamSink) {
        let stream_id = Uuid::new_v4().to_string();
        let now = Instant::now();
        // Sized for the model's own token deltas (S1), not for the handful
        // of word chunks the simulated streaming used to send: a local model
        // at ~60 tokens/s fills 128 slots in two seconds, and a subscriber
        // that falls that far behind loses the deltas it skipped. `Done`
        // still arrives — it is the last event in the buffer, and its content
        // is authoritative — so the cost of a lag is a flicker, but there is
        // no reason to invite one.
        let (tx, rx) = broadcast::channel(1024);
        let last_active = Arc::new(Mutex::new(now));
        let sink = StreamSink {
            stream_id: stream_id.clone(),
            tx: tx.clone(),
            last_active: last_active.clone(),
        };
        self.streams.insert(
            stream_id.clone(),
            StreamEntry {
                tx,
                created_at: now,
                last_active,
                lane_key: lane_key.to_string(),
            },
        );
        (stream_id, rx, sink)
    }

    /// Get a receiver for an existing stream (for SSE endpoint).
    pub fn get_receiver(&self, stream_id: &str) -> Option<broadcast::Receiver<ChatStreamEvent>> {
        self.streams
            .get(stream_id)
            .map(|entry| entry.tx.subscribe())
    }

    /// Send an event to a stream. Also refreshes `last_active` to prevent stale cleanup.
    pub fn send(&self, stream_id: &str, event: ChatStreamEvent) -> anyhow::Result<()> {
        let entry = self
            .streams
            .get(stream_id)
            .ok_or_else(|| anyhow::anyhow!("Stream not found: {stream_id}"))?;
        let _ = entry.tx.send(event);
        // Refresh last_active so cleanup_stale() won't GC active streams
        if let Ok(mut la) = entry.last_active.lock() {
            *la = Instant::now();
        }
        Ok(())
    }

    /// Remove a stream.
    pub fn remove(&self, stream_id: &str) {
        self.streams.remove(stream_id);
    }

    /// Remove streams inactive for longer than `max_age`.
    ///
    /// Uses `last_active` (not `created_at`) so that streams with ongoing
    /// delta delivery are not prematurely garbage-collected.
    pub fn cleanup_stale(&self, max_age: Duration) {
        let now = Instant::now();
        self.streams.retain(|_, entry| {
            let last = entry.last_active.lock().map(|la| *la).unwrap_or(now);
            now.duration_since(last) < max_age
        });
    }
}

impl Default for ChatStreamManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;

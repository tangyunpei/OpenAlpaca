//! ChatStreamManager — SSE stream lifecycle management
//!
//! Manages broadcast channels for chat streaming. Each active chat request
//! gets a unique stream_id with a broadcast channel for SSE delivery.

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
    Thinking,
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
        #[serde(skip_serializing_if = "Option::is_none")]
        citations: Option<Vec<Citation>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        artifacts: Option<Vec<Artifact>>,
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

/// A citation reference linking a response passage to a source document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Citation {
    /// ID of the source file asset.
    pub source_file_id: String,
    /// Page number within the document (PDF only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page: Option<u32>,
    /// Timestamp offset in milliseconds (audio only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp_ms: Option<u64>,
    /// Short excerpt from the source that supports the claim.
    pub excerpt: String,
}

/// An artifact produced during the response (e.g., generated file).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    /// ID of the produced file asset.
    pub file_id: String,
    /// Human-readable label for the artifact.
    pub label: String,
    /// MIME type of the artifact.
    pub mime_type: String,
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
    ) {
        self.send_event(ChatStreamEvent::Done {
            content: content.to_string(),
            model: model.to_string(),
            tokens_in,
            tokens_out,
            duration_ms,
            attachments_used: None,
            citations: None,
            artifacts: None,
        });
    }

    /// Send the final Done event with attachment info.
    pub fn send_done_with_attachments(
        &self,
        content: &str,
        model: &str,
        tokens_in: u64,
        tokens_out: u64,
        duration_ms: u64,
        attachments_used: Vec<String>,
    ) {
        let att = if attachments_used.is_empty() {
            None
        } else {
            Some(attachments_used)
        };
        self.send_event(ChatStreamEvent::Done {
            content: content.to_string(),
            model: model.to_string(),
            tokens_in,
            tokens_out,
            duration_ms,
            attachments_used: att,
            citations: None,
            artifacts: None,
        });
    }

    /// Send the final Done event with citation and artifact info.
    #[allow(clippy::too_many_arguments)]
    pub fn send_done_with_citations(
        &self,
        content: &str,
        model: &str,
        tokens_in: u64,
        tokens_out: u64,
        duration_ms: u64,
        attachments_used: Vec<String>,
        citations: Vec<Citation>,
        artifacts: Vec<Artifact>,
    ) {
        let att = if attachments_used.is_empty() {
            None
        } else {
            Some(attachments_used)
        };
        let cit = if citations.is_empty() {
            None
        } else {
            Some(citations)
        };
        let art = if artifacts.is_empty() {
            None
        } else {
            Some(artifacts)
        };
        self.send_event(ChatStreamEvent::Done {
            content: content.to_string(),
            model: model.to_string(),
            tokens_in,
            tokens_out,
            duration_ms,
            attachments_used: att,
            citations: cit,
            artifacts: art,
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

/// Split text into chunks of approximately `words_per_chunk` words,
/// preserving exact byte content (whitespace, newlines, indentation).
///
/// Each returned `&str` is a direct slice of the input — no characters are
/// added, removed, or reordered. Concatenating all chunks reproduces the
/// original text exactly.
///
/// The algorithm counts whitespace→non-whitespace transitions (word starts).
/// After every `words_per_chunk` words, it splits at the preceding whitespace
/// boundary. The final chunk contains any remaining text.
pub fn chunk_by_words(text: &str, words_per_chunk: usize) -> Vec<&str> {
    if text.is_empty() || words_per_chunk == 0 {
        return if text.is_empty() { vec![] } else { vec![text] };
    }

    let mut chunks = Vec::new();
    let mut chunk_start = 0;
    let mut word_count = 0;
    let mut in_word = false;
    #[allow(unused_assignments)]
    let mut last_word_boundary = 0; // byte offset of the start of the current word

    for (i, ch) in text.char_indices() {
        let is_ws = ch.is_whitespace();
        if !is_ws && !in_word {
            // Entering a new word
            word_count += 1;
            last_word_boundary = i;
            in_word = true;

            if word_count > words_per_chunk && last_word_boundary > chunk_start {
                // Cut before this new word
                chunks.push(&text[chunk_start..last_word_boundary]);
                chunk_start = last_word_boundary;
                word_count = 1;
            }
        } else if is_ws {
            in_word = false;
        }
    }

    // Remainder
    if chunk_start < text.len() {
        chunks.push(&text[chunk_start..]);
    }

    chunks
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
        let (tx, rx) = broadcast::channel(128);
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

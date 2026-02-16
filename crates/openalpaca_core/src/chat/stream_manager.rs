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
        self.streams.get(stream_id).map(|entry| entry.tx.subscribe())
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
mod tests {
    use super::*;

    #[test]
    fn test_create_and_get_receiver() {
        let mgr = ChatStreamManager::new();
        let (stream_id, _rx, _sink) = mgr.create_stream("user:gui");

        assert!(mgr.get_receiver(&stream_id).is_some());
        assert!(mgr.get_receiver("nonexistent").is_none());
    }

    #[test]
    fn test_send_and_receive() {
        let mgr = ChatStreamManager::new();
        let (stream_id, mut rx, _sink) = mgr.create_stream("user:gui");

        mgr.send(&stream_id, ChatStreamEvent::Thinking).unwrap();

        let event = rx.try_recv().unwrap();
        assert!(matches!(event, ChatStreamEvent::Thinking));
    }

    #[test]
    fn test_sink_send_delta() {
        let mgr = ChatStreamManager::new();
        let (_stream_id, mut rx, sink) = mgr.create_stream("user:gui");

        sink.send_delta("hello world");

        let event = rx.try_recv().unwrap();
        match event {
            ChatStreamEvent::Delta { content } => assert_eq!(content, "hello world"),
            _ => panic!("Expected Delta event"),
        }
    }

    #[test]
    fn test_sink_send_done() {
        let mgr = ChatStreamManager::new();
        let (_stream_id, mut rx, sink) = mgr.create_stream("user:gui");

        sink.send_done("full response", "gpt-4", 100, 200, 500);

        let event = rx.try_recv().unwrap();
        match event {
            ChatStreamEvent::Done {
                content,
                model,
                tokens_in,
                tokens_out,
                duration_ms,
            } => {
                assert_eq!(content, "full response");
                assert_eq!(model, "gpt-4");
                assert_eq!(tokens_in, 100);
                assert_eq!(tokens_out, 200);
                assert_eq!(duration_ms, 500);
            }
            _ => panic!("Expected Done event"),
        }
    }

    #[test]
    fn test_remove() {
        let mgr = ChatStreamManager::new();
        let (stream_id, _rx, _sink) = mgr.create_stream("user:gui");

        mgr.remove(&stream_id);
        assert!(mgr.get_receiver(&stream_id).is_none());
    }

    #[test]
    fn test_cleanup_stale() {
        let mgr = ChatStreamManager::new();
        let (stream_id, _rx, _sink) = mgr.create_stream("user:gui");

        // With zero-duration max_age, everything is stale
        mgr.cleanup_stale(Duration::ZERO);
        assert!(mgr.get_receiver(&stream_id).is_none());
    }

    #[test]
    fn test_cleanup_keeps_fresh() {
        let mgr = ChatStreamManager::new();
        let (stream_id, _rx, _sink) = mgr.create_stream("user:gui");

        // With large max_age, nothing is stale
        mgr.cleanup_stale(Duration::from_secs(3600));
        assert!(mgr.get_receiver(&stream_id).is_some());
    }

    #[test]
    fn test_send_refreshes_last_active() {
        let mgr = ChatStreamManager::new();
        let (stream_id, _rx, _sink) = mgr.create_stream("user:gui");

        // Send an event to refresh last_active
        mgr.send(&stream_id, ChatStreamEvent::Thinking).unwrap();

        // Even with very short max_age based on created_at, the stream should survive
        // because last_active was refreshed
        mgr.cleanup_stale(Duration::from_secs(3600));
        assert!(mgr.get_receiver(&stream_id).is_some());
    }

    #[test]
    fn test_sink_refreshes_last_active() {
        let mgr = ChatStreamManager::new();
        let (stream_id, _rx, sink) = mgr.create_stream("user:gui");

        // Send via sink (not manager) — should still refresh last_active
        sink.send_delta("hello");

        // Stream should survive cleanup because sink sends refresh last_active
        mgr.cleanup_stale(Duration::from_secs(3600));
        assert!(mgr.get_receiver(&stream_id).is_some());
    }

    // ── chunk_by_words tests ──────────────────────────────────────────

    #[test]
    fn test_chunk_by_words_empty() {
        assert!(chunk_by_words("", 3).is_empty());
    }

    #[test]
    fn test_chunk_by_words_single_word() {
        let chunks = chunk_by_words("hello", 3);
        assert_eq!(chunks, vec!["hello"]);
    }

    #[test]
    fn test_chunk_by_words_exact_boundary() {
        let chunks = chunk_by_words("one two three four five six", 3);
        assert_eq!(chunks, vec!["one two three ", "four five six"]);
    }

    #[test]
    fn test_chunk_by_words_preserves_newlines() {
        let text = "hello\nworld\nfoo bar";
        let chunks = chunk_by_words(text, 2);
        // "hello\nworld\n" then "foo bar"
        assert_eq!(chunks.len(), 2);
        // Concatenation must reproduce original
        let reassembled: String = chunks.iter().copied().collect();
        assert_eq!(reassembled, text);
    }

    #[test]
    fn test_chunk_by_words_preserves_multiple_spaces() {
        let text = "hello   world   foo";
        let chunks = chunk_by_words(text, 2);
        let reassembled: String = chunks.iter().copied().collect();
        assert_eq!(reassembled, text);
    }

    #[test]
    fn test_chunk_by_words_preserves_leading_whitespace() {
        let text = "  hello world foo bar";
        let chunks = chunk_by_words(text, 2);
        let reassembled: String = chunks.iter().copied().collect();
        assert_eq!(reassembled, text);
    }

    #[test]
    fn test_chunk_by_words_preserves_trailing_whitespace() {
        let text = "hello world  ";
        let chunks = chunk_by_words(text, 2);
        let reassembled: String = chunks.iter().copied().collect();
        assert_eq!(reassembled, text);
    }

    #[test]
    fn test_chunk_by_words_fewer_words_than_chunk_size() {
        let chunks = chunk_by_words("hi there", 5);
        assert_eq!(chunks, vec!["hi there"]);
    }

    #[test]
    fn test_chunk_by_words_code_block_with_indentation() {
        let text = "```\n  fn main() {\n    println!(\"hello\");\n  }\n```";
        let chunks = chunk_by_words(text, 3);
        let reassembled: String = chunks.iter().copied().collect();
        assert_eq!(reassembled, text);
    }
}

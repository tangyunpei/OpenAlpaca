use serde::{Deserialize, Serialize};

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    /// Capacity for the EventBus broadcast channel (system-wide event distribution).
    pub event_bus_capacity: usize,
    /// Capacity for the WebSocket event broadcaster channel.
    pub event_broadcaster_capacity: usize,
    /// Capacity for the wake event channel.
    pub wake_channel_capacity: usize,
    /// Heartbeat interval in seconds.
    pub heartbeat_interval_secs: u64,
    /// SSE keep-alive interval in seconds.
    pub sse_keep_alive_secs: u64,
    pub chat_streams: ChatStreamsConfig,
    pub embedding_indexer: EmbeddingIndexerConfig,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            event_bus_capacity: 1024,
            event_broadcaster_capacity: 256,
            wake_channel_capacity: 256,
            heartbeat_interval_secs: 5,
            sse_keep_alive_secs: 15,
            chat_streams: ChatStreamsConfig::default(),
            embedding_indexer: EmbeddingIndexerConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ChatStreamsConfig {
    /// Interval in seconds to run the stale stream cleanup.
    pub cleanup_interval_secs: u64,
    /// Seconds after which a stream is considered stale.
    pub stale_timeout_secs: u64,
    /// Delay in milliseconds between streaming word chunks. 0 = no delay (all deltas at once).
    pub stream_chunk_delay_ms: u64,
    /// Number of words per delta chunk sent to SSE clients.
    pub stream_chunk_words: usize,
}

impl Default for ChatStreamsConfig {
    fn default() -> Self {
        Self {
            cleanup_interval_secs: 60,
            stale_timeout_secs: 30,
            stream_chunk_delay_ms: 30,
            stream_chunk_words: 3,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct EmbeddingIndexerConfig {
    /// Interval in seconds between embedding indexer runs.
    pub poll_interval_secs: u64,
    /// Number of missing embeddings to process per batch.
    pub batch_size: usize,
}

impl Default for EmbeddingIndexerConfig {
    fn default() -> Self {
        Self {
            poll_interval_secs: 30,
            batch_size: 50,
        }
    }
}

/// Configuration for execution telemetry persistence.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TelemetryConfig {
    /// Whether to store a preview of the user query in the telemetry log.
    #[serde(default = "default_true")]
    pub store_query_preview: bool,
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self {
            store_query_preview: true,
        }
    }
}

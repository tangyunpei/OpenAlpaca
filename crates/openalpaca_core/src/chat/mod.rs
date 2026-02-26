//! Chat module — SSE stream management and chat service

pub mod service;
pub mod stream_manager;

pub use service::ChatService;
pub use stream_manager::{
    Artifact, ChatStreamEvent, ChatStreamManager, Citation, StreamSink, chunk_by_words,
};

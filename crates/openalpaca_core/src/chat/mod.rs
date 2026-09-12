//! Chat module — SSE stream management and chat service

pub mod service;
pub mod stream_manager;

pub use service::{ChatService, preflight_attachments};
pub use stream_manager::{
    ChatStreamEvent, ChatStreamManager, StreamSink, chunk_by_words,
};

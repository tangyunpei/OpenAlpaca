//! Chat module — SSE stream management and chat service

pub mod service;
pub mod stream_manager;
pub mod turn_sink;

pub use service::{ChatService, preflight_attachments};
pub use stream_manager::{ChatStreamEvent, ChatStreamManager, StreamSink};
pub use turn_sink::{TurnSink, TurnSinkHandle};

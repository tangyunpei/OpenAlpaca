//! OpenAlpaca wrapper around the `rmcp` Model Context Protocol SDK.
//!
//! Phase 1 (foundation) provides:
//! - [`McpClient`] — connect to MCP servers, list/call tools, auto-reconnect.
//! - [`Transport`] trait with stdio + streamable-HTTP implementations.
//! - [`McpError`] with retry/cancellation helpers.
//!
//! Phases 2–5 build on top: registry wiring (P2), built-in tool annotations (P3),
//! server mode (P4), resources & prompts (P5).

pub mod error;
pub mod transport;

mod client;
mod lifecycle;
mod server;

// TODO(P1): these re-exports become live as each symbol is filled in by later tasks.
// - `client::{McpClient, McpClientConfig}` — implemented later in P1.
// - `server::McpServer` — implemented later in P1.
// - `transport::StreamableHttpTransport` — implemented in Task 5.
// pub use client::{McpClient, McpClientConfig};
pub use error::{ErrorCategory, McpError};
// pub use server::McpServer;
pub use transport::{HttpAuth, StdioTransport, Transport, TransportKind};

// Re-exports from rmcp — data types pass through unchanged.
pub use rmcp::model::{
    CallToolResult, Implementation, Prompt, PromptMessage, ProtocolVersion, Resource,
    ResourceContents, Tool, ToolAnnotations,
};

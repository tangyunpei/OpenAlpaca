//! OpenAlpaca wrapper around the `rmcp` Model Context Protocol SDK.
//!
//! **Client-only.** OpenAlpaca connects out to external MCP servers (Claude
//! Desktop, third-party tool servers, etc.); it does not expose its own tools
//! over MCP. Server mode (originally sketched as P4) is an explicit non-goal.
//!
//! Provides:
//! - [`McpClient`] — connect to MCP servers, list/call tools, auto-reconnect.
//! - [`Transport`] trait with stdio + streamable-HTTP implementations.
//! - [`McpError`] with retry/cancellation helpers.

pub mod error;
pub mod transport;

mod client;
mod lifecycle;

pub use client::{McpClient, McpClientConfig, NotifyingHandler, ServerChange};
pub use error::{ErrorCategory, McpError};
pub use lifecycle::ConnectionSnapshot;
pub use transport::{HttpAuth, StdioTransport, Transport, TransportKind};

// Re-exports from rmcp — data types pass through unchanged.
pub use rmcp::model::{
    CallToolResult, Content, Implementation, Prompt, PromptMessage, ProtocolVersion, RawContent,
    Resource, ResourceContents, Tool, ToolAnnotations,
};

//! Integration between `openalpaca_mcp` (the SDK wrapper) and the tool registry.
//!
//! - [`config::McpConfig`] — parse `config/mcp.toml`.
//! - [`bridge`] — convert `rmcp::Tool` to `RegisteredTool`; serialize `CallToolResult`.
//! - [`client_set::McpClientSet`] — daemon-lifetime holder for connected MCP clients.

pub mod bridge;
pub mod client_set;
pub mod config;

// Re-exports are added incrementally as each submodule lands (Tasks 2, 3, 5).
pub use config::{HttpAuthConfig, LoadError, McpConfig, McpDefaults, McpServerConfig};

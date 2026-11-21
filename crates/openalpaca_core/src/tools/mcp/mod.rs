//! Integration between `openalpaca_mcp` (the SDK wrapper) and the tool registry.
//!
//! - [`config::McpConfig`] — parse `config/mcp.toml`.
//! - [`bridge`] — convert `rmcp::Tool` to `RegisteredTool`; serialize `CallToolResult`.

pub mod bridge;
pub mod classify;
pub mod config;
pub mod fingerprint;

#[cfg(test)]
mod by_product_tests;

// Re-exports are added incrementally as each submodule lands (Tasks 2, 3, 5).
pub use bridge::{SerializeOptions, rmcp_tool_to_registered, serialize_call_result};
pub use classify::{classify_bringup_failure, classify_call_failure};
pub use config::{
    HttpAuthConfig, LoadError, McpConfig, McpDefaults, McpProtocol, McpServerConfig,
    is_valid_server_name,
};
pub use fingerprint::config_fingerprint;

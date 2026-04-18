// crates/openalpaca_mcp/src/server.rs

//! MCP server mode. **Phase 4 feature** — P1 ships only the type so P2–P3 have
//! a stable import path.

/// Placeholder for Phase 4 (server mode).
///
/// Constructing this in P1 panics. The type exists so downstream crates
/// can depend on a stable import path and so the P4 implementation can
/// fill it in without breaking changes.
pub struct McpServer {
    _private: (),
}

#[allow(clippy::new_without_default)]
impl McpServer {
    /// Panics in P1. Implemented in Phase 4 of the MCP roadmap.
    pub fn new() -> Self {
        unimplemented!("McpServer is implemented in Phase 4 of the MCP roadmap")
    }
}

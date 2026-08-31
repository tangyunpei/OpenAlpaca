// crates/openalpaca_core/src/tools/mcp/client_set.rs

#[derive(Debug, Clone)]
pub struct McpServerSummary {
    pub server_name: String,
    pub transport_kind: &'static str,
    pub status: McpServerStatus,
    pub discovered_tools: Vec<String>,
}

#[derive(Debug, Clone)]
pub enum McpServerStatus {
    Connected {
        server_version: String,
        protocol_version: String,
    },
    Failed { reason: String },
    Disabled,
}

// crates/openalpaca_core/src/tools/mcp/client_set.rs

use std::sync::Arc;

/// Daemon-lifetime container for connected MCP clients. Holds Arc handles so
/// clients outlive the tool registry's references and introspection summaries
/// for diagnostics.
#[derive(Default)]
pub struct McpClientSet {
    clients: Vec<Arc<openalpaca_mcp::McpClient>>,
    summaries: Vec<McpServerSummary>,
}

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

impl McpClientSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push_client(&mut self, client: Arc<openalpaca_mcp::McpClient>) {
        self.clients.push(client);
    }

    pub fn add_summary(&mut self, summary: McpServerSummary) {
        self.summaries.push(summary);
    }

    /// Add a disabled-by-config server summary without a client.
    pub fn add_disabled(&mut self, server_name: String, transport_kind: &'static str) {
        self.summaries.push(McpServerSummary {
            server_name,
            transport_kind,
            status: McpServerStatus::Disabled,
            discovered_tools: Vec::new(),
        });
    }

    pub fn summaries(&self) -> &[McpServerSummary] {
        &self.summaries
    }

    pub fn client_count(&self) -> usize {
        self.clients.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_set_has_zero_clients() {
        let s = McpClientSet::new();
        assert_eq!(s.client_count(), 0);
        assert!(s.summaries().is_empty());
    }

    #[test]
    fn add_and_summary_roundtrip() {
        let mut s = McpClientSet::new();
        s.add_summary(McpServerSummary {
            server_name: "fs".into(),
            transport_kind: "stdio",
            status: McpServerStatus::Connected {
                server_version: "1.0".into(),
                protocol_version: "2024-11".into(),
            },
            discovered_tools: vec!["fs__read_file".into()],
        });
        assert_eq!(s.summaries().len(), 1);
        assert_eq!(s.summaries()[0].server_name, "fs");
        assert_eq!(s.summaries()[0].discovered_tools, vec!["fs__read_file"]);
    }

    #[test]
    fn summaries_preserve_insertion_order() {
        let mut s = McpClientSet::new();
        for name in ["a", "b", "c"] {
            s.add_disabled(name.into(), "stdio");
        }
        let names: Vec<_> = s.summaries().iter().map(|s| s.server_name.clone()).collect();
        assert_eq!(names, vec!["a", "b", "c"]);
    }

    #[test]
    fn add_disabled_uses_disabled_status() {
        let mut s = McpClientSet::new();
        s.add_disabled("x".into(), "stdio");
        assert!(matches!(s.summaries()[0].status, McpServerStatus::Disabled));
    }
}

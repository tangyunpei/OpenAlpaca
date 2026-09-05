//! Conversion between `openalpaca_mcp` (rmcp-wrapped) types and the tool registry.
//!
//! - [`rmcp_tool_to_registered`] — wrap a discovered `rmcp::Tool` as a
//!   [`RegisteredTool`] with a [`ToolBackend::Mcp`] backend.
//! - [`serialize_call_result`] — flatten a `CallToolResult` into the
//!   `Result<String, String>` shape the rest of the tool-call path consumes.

use std::sync::Arc;

use openalpaca_mcp::{CallToolResult, McpClient, RawContent, Tool};

use crate::tools::registry::{RegisteredTool, ToolBackend};

/// Convert an rmcp `Tool` (discovered via `McpClient::list_tools`) into a
/// [`RegisteredTool`] with a [`ToolBackend::Mcp`] backend.
///
/// The registered definition uses the namespaced name
/// `<server_name>__<remote_name>` so MCP-provided tools can't collide with
/// built-ins or with tools from other servers.
///
/// `generation` is the load this handle belongs to — the number E0 handed the
/// supervisor (extension design §3.0 Fact 3). This is the **single** production
/// stamping site for MCP. Until the MCP supervisor lands, its one production
/// caller passes `0`, which is inert: with no ledger record the gate fails open
/// and never compares it.
pub fn rmcp_tool_to_registered(
    server_name: &str,
    server_version: &str,
    tool: Tool,
    client: Arc<McpClient>,
    generation: u64,
) -> RegisteredTool {
    let remote_name = tool.name.to_string();
    let namespaced_name = format!("{server_name}__{remote_name}");

    let description = tool
        .description
        .as_ref()
        .map(|d| d.to_string())
        .unwrap_or_else(|| format!("Tool '{remote_name}' from MCP server '{server_name}'"));

    let parameters = serde_json::to_value(&tool.input_schema)
        .unwrap_or_else(|_| serde_json::json!({ "type": "object", "properties": {} }));

    let definition = openalpaca_llm::ToolDefinition {
        name: namespaced_name.clone(),
        description,
        parameters,
        strict: None,
        input_examples: None,
    };

    let backend = ToolBackend::Mcp {
        client,
        remote_name: remote_name.clone(),
        server_name: server_name.to_string(),
        generation,
    };

    RegisteredTool {
        definition,
        backend,
        // Capability-per-name (same pattern as the workspace builtins): an agent
        // template or skill that lists the namespaced tool name (e.g.
        // "fs__read_file") in its `capabilities` resolves this tool via the
        // registry's capability index.
        provides_capabilities: vec![namespaced_name],
        exempt_from_timeout: false,
        annotations: tool.annotations,
        version: server_version.to_string(),
        author: format!("mcp:{}", server_name),
        created_at: chrono::Utc::now(),
    }
}

/// Convert an rmcp [`CallToolResult`] into the `Result<String, String>` shape
/// the tool registry hands back to the agentic loop.
///
/// Concatenates text content blocks with newline separators. Non-text blocks
/// are replaced by a bracketed placeholder so the LLM can see they existed —
/// P2 doesn't yet surface images/resources/audio back to the model. When
/// `is_error == Some(true)`, returns `Err(body)` so the tool-call path treats
/// it as a tool error.
pub fn serialize_call_result(result: CallToolResult) -> Result<String, String> {
    let mut buf = String::new();
    for content in &result.content {
        if !buf.is_empty() {
            buf.push('\n');
        }
        match &content.raw {
            RawContent::Text(t) => buf.push_str(&t.text),
            RawContent::Image(_) => {
                buf.push_str(
                    "[image content omitted — P2 does not yet surface non-text MCP content]",
                );
            }
            other => {
                buf.push_str(&format!("[non-text content: {}]", content_kind_name(other)));
            }
        }
    }

    if result.is_error.unwrap_or(false) {
        return Err(buf);
    }
    Ok(buf)
}

fn content_kind_name(c: &RawContent) -> &'static str {
    match c {
        RawContent::Text(_) => "text",
        RawContent::Image(_) => "image",
        RawContent::Audio(_) => "audio",
        RawContent::Resource(_) => "resource",
        RawContent::ResourceLink(_) => "resource_link",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openalpaca_mcp::Content;

    fn text_content(s: &str) -> Content {
        Content::text(s.to_string())
    }

    #[test]
    fn namespace_tool_name_format() {
        // Verify the prefix pattern our bridge uses.
        let server = "fs";
        let remote = "read_file";
        assert_eq!(format!("{server}__{remote}"), "fs__read_file");
    }

    #[test]
    fn serialize_text_content_concatenates() {
        let result = CallToolResult::success(vec![text_content("hello"), text_content("world")]);
        let out = serialize_call_result(result).unwrap();
        assert_eq!(out, "hello\nworld");
    }

    #[test]
    fn serialize_is_error_true_returns_err() {
        let result = CallToolResult::error(vec![text_content("boom")]);
        let err = serialize_call_result(result).unwrap_err();
        assert_eq!(err, "boom");
    }

    #[test]
    fn serialize_empty_content_returns_ok_empty() {
        let result = CallToolResult::success(vec![]);
        let out = serialize_call_result(result).unwrap();
        assert_eq!(out, "");
    }

    #[test]
    fn serialize_is_error_false_treated_as_ok() {
        let result = CallToolResult::success(vec![text_content("ok")]);
        let out = serialize_call_result(result).unwrap();
        assert_eq!(out, "ok");
    }

    #[test]
    fn registered_mcp_tool_provides_capability_per_name() {
        let client = Arc::new(McpClient::disconnected_for_tests("srv"));
        let tool = Tool::new("echo", "Echo tool", serde_json::Map::new());
        let registered = rmcp_tool_to_registered("srv", "1.0", tool, client, 0);
        assert_eq!(registered.definition.name, "srv__echo");
        assert_eq!(registered.provides_capabilities, vec!["srv__echo".to_string()]);
    }

    #[test]
    fn template_capability_listing_mcp_tool_name_resolves_tool_def() {
        // An agent template that lists the namespaced MCP tool name in its
        // `capabilities` resolves the tool definition through the same
        // capability-index path builtins use (resolve_agent_tools).
        use crate::agent::{AgentConstraints, AgentLlmConfig, AgentPreset, AgentStatus, Capability, SubAgent};
        use crate::tools::{ToolRegistry, resolve_agent_tools};

        let client = Arc::new(McpClient::disconnected_for_tests("srv"));
        let tool = Tool::new("echo", "Echo tool", serde_json::Map::new());
        let registered = rmcp_tool_to_registered("srv", "1.0", tool, client, 0);

        let registry = Arc::new(ToolRegistry::default());
        registry.register(registered).expect("register MCP tool");

        let agent = SubAgent {
            id: "test_agent".into(),
            template_id: "test_agent".into(),
            name: "Test Agent".into(),
            description: None,
            icon: None,
            status: AgentStatus::Idle,
            current_task: None,
            capabilities: vec![Capability {
                name: "srv__echo".into(),
                category: "mcp".into(),
                proficiency: 1.0,
            }],
            preset: AgentPreset::default(),
            constraints: AgentConstraints::default(),
            llm_config: AgentLlmConfig::default(),
        };

        let defs = resolve_agent_tools(&agent, &registry);
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].name, "srv__echo");
    }

    #[test]
    fn mcp_author_includes_server_name_format() {
        let server_name = "filesystem";
        let author = format!("mcp:{server_name}");
        assert_eq!(author, "mcp:filesystem");
    }

    #[test]
    fn mcp_version_defaults_to_unknown_when_server_info_missing() {
        // services/mcp.rs logic: unwrap_or("unknown")
        let server_info: Option<openalpaca_mcp::Implementation> = None;
        let version = server_info
            .as_ref()
            .map(|i| i.version.as_str())
            .unwrap_or("unknown");
        assert_eq!(version, "unknown");
    }
}

//! Conversion between `openalpaca_mcp` (rmcp-wrapped) types and the tool registry.
//!
//! - [`rmcp_tool_to_registered`] — wrap a discovered `rmcp::Tool` as a
//!   [`RegisteredTool`] with a [`ToolBackend::Mcp`] backend.
//! - [`mcp_tool_name`] / [`map_tool_list`] — the registry name of a remote
//!   tool, in the providers' alphabet, and one server's list mapped with its
//!   collisions resolved (N-MCP-4).
//! - [`serialize_call_result`] — flatten a `CallToolResult` into the
//!   `Result<String, String>` shape the rest of the tool-call path consumes.

use std::sync::Arc;

use openalpaca_mcp::{CallToolResult, ContentBlock, McpClient, Tool};

use crate::tools::registry::{RegisteredTool, ToolBackend};

/// The longest tool name every provider accepts: OpenAI's documented limit
/// for a function name ("a-z, A-Z, 0-9, or contain underscores and dashes,
/// with a maximum length of 64"; `openai-openapi` `FunctionObject.name`).
/// Anthropic's is `^[a-zA-Z0-9_-]{1,128}$` (Messages API, tool `name`); Ollama
/// documents none. The alphabet is the same for both.
pub const PROVIDER_TOOL_NAME_MAX: usize = 64;

/// The registry name of remote tool `remote_name` on server `server_name`:
/// `<server_name>__<remote_name>` with every character outside
/// `[A-Za-z0-9_-]` replaced by `_`, capped at [`PROVIDER_TOOL_NAME_MAX`]
/// (N-MCP-4). A name already in the alphabet and within the cap is returned
/// unchanged, so a template or skill naming it keeps matching.
///
/// Past the cap the remote half keeps its first characters and ends in `_`
/// plus the first 8 hex digits of the SHA-256 of the **raw** remote name, so
/// two long names sharing a prefix stay two names. The call itself always
/// goes to the server under the raw name (the backend's `remote_name`).
///
/// Server names are `^[a-zA-Z][a-zA-Z0-9_-]{0,30}$` (`McpConfig::parse`), so
/// at least 22 characters of the remote half survive the cap.
pub fn mcp_tool_name(server_name: &str, remote_name: &str) -> String {
    let mapped: String = remote_name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let full = format!("{server_name}__{mapped}");
    if full.len() <= PROVIDER_TOOL_NAME_MAX {
        return full;
    }
    let digest = openalpaca_storage::hash::sha256_hex(remote_name.as_bytes());
    let suffix = &digest[..8];
    // `mapped` is ASCII by construction, so any byte index is a char boundary.
    let keep = PROVIDER_TOOL_NAME_MAX
        .saturating_sub(server_name.len() + 2 + 1 + suffix.len())
        .min(mapped.len());
    format!("{server_name}__{}_{suffix}", &mapped[..keep])
}

/// One server's tool list under its registry names, in the server's order.
pub struct MappedTools {
    /// `(registry name, tool)` — the first tool to claim each name.
    pub tools: Vec<(String, Tool)>,
    /// Every later tool whose name mapped onto one already claimed.
    pub collisions: Vec<NameCollision>,
}

/// Two remote names of one server that map to the same registry name.
pub struct NameCollision {
    pub registry_name: String,
    /// The raw remote name that keeps it (listed first).
    pub kept: String,
    /// The raw remote name that is not registered.
    pub dropped: String,
}

/// Map one server's list with [`mcp_tool_name`], **before** anything is
/// registered: of two remote names that map to the same registry name the
/// first listed is kept and the other is left out with a warning naming both
/// originals (N-MCP-4). Without this the second would replace the first in
/// the registry — the case-13 rule skips only another extension's incumbent
/// (`McpSupervisor::is_live_incumbent`).
pub fn map_tool_list(server_name: &str, tools: Vec<Tool>) -> MappedTools {
    let mut claimed: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut mapped = MappedTools {
        tools: Vec::new(),
        collisions: Vec::new(),
    };
    for tool in tools {
        let raw = tool.name.to_string();
        let name = mcp_tool_name(server_name, &raw);
        if let Some(kept) = claimed.get(&name) {
            tracing::warn!(
                server = %server_name,
                tool = %name,
                kept = %kept,
                dropped = %raw,
                "two MCP tools map to one name; keeping the first listed"
            );
            mapped.collisions.push(NameCollision {
                registry_name: name,
                kept: kept.clone(),
                dropped: raw,
            });
            continue;
        }
        claimed.insert(name.clone(), raw);
        mapped.tools.push((name, tool));
    }
    mapped
}

/// Convert an rmcp `Tool` (discovered via `McpClient::list_tools`) into a
/// [`RegisteredTool`] with a [`ToolBackend::Mcp`] backend.
///
/// The registered definition uses the namespaced name
/// `<server_name>__<remote_name>`, mapped into the providers' alphabet by
/// [`mcp_tool_name`], so MCP-provided tools can't collide with built-ins or
/// with tools from other servers; the backend keeps the raw remote name for
/// the call.
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
    let namespaced_name = mcp_tool_name(server_name, &remote_name);

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
        match content {
            ContentBlock::Text(t) => buf.push_str(&t.text),
            ContentBlock::Image(_) => {
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

fn content_kind_name(c: &ContentBlock) -> &'static str {
    match c {
        ContentBlock::Text(_) => "text",
        ContentBlock::Image(_) => "image",
        ContentBlock::Audio(_) => "audio",
        ContentBlock::Resource(_) => "resource",
        ContentBlock::ResourceLink(_) => "resource_link",
        // `ContentBlock` is `#[non_exhaustive]` since rmcp 3; a block kind a
        // later rmcp adds still reaches the model as a placeholder.
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openalpaca_mcp::ContentBlock;

    fn text_content(s: &str) -> ContentBlock {
        ContentBlock::text(s.to_string())
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

        let defs = resolve_agent_tools(&agent, &registry, None);
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].name, "srv__echo");
    }

    fn backend_remote_name(registered: &RegisteredTool) -> &str {
        match &registered.backend {
            ToolBackend::Mcp { remote_name, .. } => remote_name,
            _ => panic!("an MCP tool has an MCP backend"),
        }
    }

    /// N-MCP-4: the registry name is in the providers' alphabet; the call goes
    /// to the server under the name it gave.
    #[test]
    fn a_remote_name_outside_the_alphabet_registers_under_the_mapped_name_and_calls_with_the_raw_one()
     {
        let client = Arc::new(McpClient::disconnected_for_tests("srv"));
        let tool = Tool::new("read.file", "Read a file", serde_json::Map::new());
        let registered = rmcp_tool_to_registered("srv", "1.0", tool, client, 0);
        assert_eq!(registered.definition.name, "srv__read_file");
        assert_eq!(
            registered.provides_capabilities,
            vec!["srv__read_file".to_string()]
        );
        assert_eq!(backend_remote_name(&registered), "read.file");
    }

    /// N-MCP-4: a name the providers already accept is left exactly as it was,
    /// so a template naming it keeps matching.
    #[test]
    fn a_name_already_in_the_alphabet_is_unchanged() {
        let client = Arc::new(McpClient::disconnected_for_tests("srv"));
        let tool = Tool::new("Read_file-2", "Read a file", serde_json::Map::new());
        let registered = rmcp_tool_to_registered("srv", "1.0", tool, client, 0);
        assert_eq!(registered.definition.name, "srv__Read_file-2");
        assert_eq!(backend_remote_name(&registered), "Read_file-2");
    }

    /// N-MCP-4: the combined name fits the strictest provider's 64-character
    /// limit, and two long names that share a prefix stay distinct.
    #[test]
    fn a_combined_name_past_64_characters_is_capped_and_stays_distinct() {
        let long_a = format!("{}_alpha", "x".repeat(80));
        let long_b = format!("{}_beta", "x".repeat(80));
        let names: Vec<String> = [long_a.as_str(), long_b.as_str()]
            .iter()
            .map(|remote| {
                let client = Arc::new(McpClient::disconnected_for_tests("filesystem"));
                let tool = Tool::new(remote.to_string(), "long", serde_json::Map::new());
                let registered = rmcp_tool_to_registered("filesystem", "1.0", tool, client, 0);
                assert_eq!(
                    backend_remote_name(&registered),
                    *remote,
                    "the call keeps the raw name"
                );
                registered.definition.name
            })
            .collect();
        for name in &names {
            assert!(name.len() <= 64, "{name} is {} characters", name.len());
            assert!(name.starts_with("filesystem__xxx"), "{name}");
            assert!(
                name.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
                "{name}"
            );
        }
        assert_ne!(
            names[0], names[1],
            "two long names must not collapse into one"
        );
    }

    /// N-MCP-4: `read.file` beside `read_file` — the first keeps the name,
    /// the second is left out, and the warning names both originals.
    #[tracing_test::traced_test]
    #[test]
    fn read_file_beside_read_dot_file_keeps_the_first_and_warns_naming_both() {
        let tools = vec![
            Tool::new("read.file", "first", serde_json::Map::new()),
            Tool::new("read_file", "second", serde_json::Map::new()),
            Tool::new("list", "unrelated", serde_json::Map::new()),
        ];
        let mapped = map_tool_list("srv", tools);
        let names: Vec<(&str, &str)> = mapped
            .tools
            .iter()
            .map(|(name, tool)| (name.as_str(), tool.name.as_ref()))
            .collect();
        assert_eq!(
            names,
            vec![("srv__read_file", "read.file"), ("srv__list", "list")]
        );
        assert_eq!(mapped.collisions.len(), 1);
        assert_eq!(mapped.collisions[0].kept, "read.file");
        assert_eq!(mapped.collisions[0].dropped, "read_file");
        assert!(logs_contain("two MCP tools map to one name"));
        assert!(logs_contain("kept=read.file"));
        assert!(logs_contain("dropped=read_file"));
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

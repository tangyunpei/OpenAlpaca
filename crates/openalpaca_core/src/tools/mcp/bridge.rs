//! Conversion between `openalpaca_mcp` (rmcp-wrapped) types and the tool registry.
//!
//! - [`rmcp_tool_to_registered`] — wrap a discovered `rmcp::Tool` as a
//!   [`RegisteredTool`] with a [`ToolBackend::Mcp`] backend.
//! - [`mcp_tool_name`] / [`map_tool_list`] — the registry name of a remote
//!   tool, in the providers' alphabet, and one server's list mapped with its
//!   collisions resolved (N-MCP-4).
//! - [`serialize_call_result`] — flatten a `CallToolResult` into the
//!   `Result<String, String>` shape the rest of the tool-call path consumes,
//!   under one [`SerializeOptions`]; an image, audio clip or embedded blob is
//!   kept as a by-product and named by a line (M85).

use std::sync::Arc;

use openalpaca_mcp::{CallToolResult, ContentBlock, McpClient, Resource, ResourceContents, Tool};
use serde_json::Value;

use crate::tools::builtins::{ByProductSink, ToolBlob, save_tool_by_product};
use crate::tools::registry::{RegisteredTool, ToolBackend, ToolContext};

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

    // M14: the declared output schema is carried on the backend and served
    // by `GET /v1/tools`; it never enters the provider's `ToolDefinition`
    // above (`strict_mode_compatible` stays the rule there).
    let backend = ToolBackend::Mcp {
        client,
        remote_name: remote_name.clone(),
        server_name: server_name.to_string(),
        generation,
        output_schema: tool.output_schema.clone(),
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

/// How [`serialize_call_result`] reads one result: the one place an entry
/// adds what the serializer must know (M85 the by-product sink and the call's
/// context, M82 the output schema), so the signature keeps one parameter.
/// `SerializeOptions::default()` has no sink and no context: an image, audio
/// clip or embedded blob is then named by a "nothing was saved" line.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct SerializeOptions<'a> {
    /// M85: where a block of bytes is kept, and the call it came from.
    pub(crate) by_products: ByProducts<'a>,
}

impl<'a> SerializeOptions<'a> {
    /// The options of one dispatched call: its registry name, the registry's
    /// sink and the call's context, either of which may be absent.
    pub(crate) fn for_call(
        tool: &'a str,
        sink: Option<&'a ByProductSink>,
        ctx: Option<&'a ToolContext>,
    ) -> Self {
        Self {
            by_products: ByProducts { tool, sink, ctx },
        }
    }
}

/// What M85's by-products need from the call (see [`SerializeOptions`]).
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct ByProducts<'a> {
    /// The tool's registry name (`<server>__<tool>`): it leads each
    /// by-product's title and the store's refusals name it.
    pub(crate) tool: &'a str,
    pub(crate) sink: Option<&'a ByProductSink>,
    pub(crate) ctx: Option<&'a ToolContext>,
}

/// Convert an rmcp [`CallToolResult`] into the `Result<String, String>` shape
/// the tool registry hands back to the agentic loop.
///
/// Concatenates content blocks with newline separators: a text block, and an
/// embedded text resource, as its text; a resource link as
/// `[resource link: <label> — <uri>]` (the label is its `name`, else its
/// `title`; the URI alone when both are empty); an image, an audio clip and an
/// embedded blob kept as a by-product through `save_tool_by_product` (M85,
/// DU-R6 (b)) and named by its line — the artifact's id, absolute path, MIME
/// type and size, or why nothing was saved; any other block as a bracketed
/// placeholder so the LLM can see it existed. Each kept block takes a title
/// of its own (`<tool> <kind> <call tag> <n>`), so two blocks of one call, or
/// of two calls, never become versions of one artifact. Then, when
/// `structured_content` holds a value other than JSON `null` and no text block
/// already parses to that same value (an rmcp-built server mirrors it as
/// compact text), the value is appended pretty-printed (M14). When `is_error == Some(true)`, returns `Err(body)` so the tool-call
/// path treats it as a tool error.
pub fn serialize_call_result(
    result: CallToolResult,
    options: &SerializeOptions<'_>,
) -> Result<String, String> {
    let by_products = options.by_products;
    // One tag per call, so the titles of two calls to one tool differ.
    let call_tag = uuid::Uuid::new_v4().simple().to_string()[..8].to_string();
    let keep = |n: usize, what: &str, label: &str, base64: &str, mime: Option<&str>| {
        let title = by_product_title(by_products.tool, label, &call_tag, n);
        save_tool_by_product(
            by_products.sink,
            by_products.ctx,
            ToolBlob {
                tool: by_products.tool,
                what,
                title: &title,
                base64,
                mime_type: mime,
            },
        )
    };

    let mut buf = String::new();
    for (index, content) in result.content.iter().enumerate() {
        if !buf.is_empty() {
            buf.push('\n');
        }
        let n = index + 1;
        match content {
            ContentBlock::Text(t) => buf.push_str(&t.text),
            ContentBlock::Image(image) => buf.push_str(&keep(
                n,
                "image",
                "image",
                &image.data,
                Some(&image.mime_type),
            )),
            ContentBlock::Audio(audio) => buf.push_str(&keep(
                n,
                "audio",
                "audio",
                &audio.data,
                Some(&audio.mime_type),
            )),
            ContentBlock::Resource(embedded) => match &embedded.resource {
                ResourceContents::TextResourceContents { text, .. } => buf.push_str(text),
                ResourceContents::BlobResourceContents {
                    blob, mime_type, ..
                } => buf.push_str(&keep(
                    n,
                    "embedded resource",
                    "resource",
                    blob,
                    mime_type.as_deref(),
                )),
                // `ResourceContents` is `#[non_exhaustive]`: a kind a later
                // rmcp adds still reaches the model as a placeholder.
                _ => buf.push_str("[non-text content: resource]"),
            },
            ContentBlock::ResourceLink(link) => buf.push_str(&resource_link_line(link)),
            other => {
                buf.push_str(&format!("[non-text content: {}]", content_kind_name(other)));
            }
        }
    }

    // `"structuredContent": null` arrives as `Some(Null)`; it is read as
    // absent, so N-TOOL-6's line still answers for a result with nothing else.
    if let Some(value) = result.structured_content.as_ref().filter(|v| !v.is_null())
        && !mirrored(&result.content, value)
    {
        if !buf.is_empty() {
            buf.push('\n');
        }
        // A `Value` always serializes; the compact form is the fallback.
        buf.push_str(&serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string()));
    }

    if result.is_error.unwrap_or(false) {
        return Err(buf);
    }
    Ok(buf)
}

/// Whether a text block of `content` is a copy of the structured `value`: its
/// exact compact form (what rmcp's `CallToolResult::structured` writes,
/// compared as text so a float's re-parse cannot make it differ) or any text
/// that parses to the same value.
fn mirrored(content: &[ContentBlock], value: &Value) -> bool {
    let compact = value.to_string();
    content.iter().any(|block| match block {
        ContentBlock::Text(t) => {
            t.text == compact
                || serde_json::from_str::<Value>(&t.text).is_ok_and(|parsed| parsed == *value)
        }
        _ => false,
    })
}

/// `[resource link: <label> — <uri>]`, the label the link's `name`, else its
/// `title`; `[resource link: <uri>]` when both are empty or absent. `name` is
/// a required string in MCP, so "absent" can only be empty.
fn resource_link_line(link: &Resource) -> String {
    let label = Some(link.name.as_str())
        .filter(|n| !n.is_empty())
        .or_else(|| link.title.as_deref().filter(|t| !t.is_empty()));
    match label {
        Some(label) => format!("[resource link: {label} — {}]", link.uri),
        None => format!("[resource link: {}]", link.uri),
    }
}

/// `<tool> <label> <call tag> <n>`, the tool's name cut so the whole title
/// stays within the store's slug (`FILE_NAME_SLUG_BYTES`) and the tag and
/// the block's number, which make it distinct, are never cut away.
fn by_product_title(tool: &str, label: &str, call_tag: &str, n: usize) -> String {
    let suffix = format!(" {label} {call_tag} {n}");
    let room = openalpaca_storage::store::FILE_NAME_SLUG_BYTES.saturating_sub(suffix.len());
    let tool = &tool[..tool.floor_char_boundary(room)];
    format!("{tool}{suffix}")
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
        let out = serialize_call_result(result, &SerializeOptions::default()).unwrap();
        assert_eq!(out, "hello\nworld");
    }

    #[test]
    fn serialize_is_error_true_returns_err() {
        let result = CallToolResult::error(vec![text_content("boom")]);
        let err = serialize_call_result(result, &SerializeOptions::default()).unwrap_err();
        assert_eq!(err, "boom");
    }

    #[test]
    fn serialize_empty_content_returns_ok_empty() {
        let result = CallToolResult::success(vec![]);
        let out = serialize_call_result(result, &SerializeOptions::default()).unwrap();
        assert_eq!(out, "");
    }

    #[test]
    fn serialize_is_error_false_treated_as_ok() {
        let result = CallToolResult::success(vec![text_content("ok")]);
        let out = serialize_call_result(result, &SerializeOptions::default()).unwrap();
        assert_eq!(out, "ok");
    }

    fn serialize(result: CallToolResult) -> Result<String, String> {
        serialize_call_result(result, &SerializeOptions::default())
    }

    /// A successful result carrying only `structuredContent` (`content`
    /// defaults to `[]`).
    fn structured_only(value: Value) -> CallToolResult {
        let mut result = CallToolResult::default();
        result.structured_content = Some(value);
        result
    }

    /// M14: the data a tool returned only as `structuredContent` reaches the
    /// model pretty-printed, so N-TOOL-6's "completed with no output" line no
    /// longer stands in for it.
    #[test]
    fn structured_content_alone_reaches_the_model_as_pretty_json() {
        let value = serde_json::json!({"issue": {"number": 7, "open": true}});
        let out = serialize(structured_only(value.clone())).unwrap();
        assert_eq!(out, serde_json::to_string_pretty(&value).unwrap());
        assert!(
            crate::session_log::no_output_line("srv__get", &out).is_none(),
            "a structured-only result is not 'completed with no output': {out}"
        );
    }

    /// M14: an rmcp-built server mirrors the value as compact text into
    /// `content`; the model reads it once, not twice.
    #[test]
    fn a_mirrored_structured_value_is_printed_once() {
        let value = serde_json::json!({"count": 3, "items": ["a", "b"], "ratio": 0.1});
        let out = serialize(CallToolResult::structured(value.clone())).unwrap();
        assert_eq!(out, value.to_string());
    }

    /// M14: a text block that parses to the same value, in any layout,
    /// counts as the mirror.
    #[test]
    fn a_pretty_text_block_of_the_same_value_counts_as_the_mirror() {
        let value = serde_json::json!({"b": 2, "a": 1});
        let mut result = structured_only(value.clone());
        result.content = vec![text_content(&serde_json::to_string_pretty(&value).unwrap())];
        let out = serialize(result).unwrap();
        assert_eq!(out, serde_json::to_string_pretty(&value).unwrap());
    }

    /// M14: a summary in `content` and different data in
    /// `structuredContent` — the model reads both, the summary first.
    #[test]
    fn a_summary_and_a_different_structured_value_are_both_kept() {
        let value = serde_json::json!({"created": 2});
        let mut result = structured_only(value.clone());
        result.content = vec![text_content("Created two issues.")];
        let out = serialize(result).unwrap();
        assert_eq!(
            out,
            format!(
                "Created two issues.\n{}",
                serde_json::to_string_pretty(&value).unwrap()
            )
        );
    }

    /// M14: `structured_error` mirrors too; the error reads the text once and
    /// the `is_error` mapping is unchanged.
    #[test]
    fn a_structured_error_is_an_err_with_its_text_once() {
        let value = serde_json::json!({"error": "not found"});
        let err = serialize(CallToolResult::structured_error(value.clone())).unwrap_err();
        assert_eq!(err, value.to_string());
    }

    /// M14: `"structuredContent": null` arrives as `Some(Null)` and is read
    /// as absent — nothing is appended, so N-TOOL-6's line still answers.
    #[test]
    fn a_null_structured_value_is_read_as_absent() {
        let out = serialize(structured_only(Value::Null)).unwrap();
        assert_eq!(out, "");
        assert_eq!(
            crate::session_log::no_output_line("srv__get", &out).as_deref(),
            Some("(srv__get completed with no output)")
        );

        let mut result = structured_only(Value::Null);
        result.content = vec![text_content("done")];
        assert_eq!(serialize(result).unwrap(), "done");
    }

    /// M14: a resource link prints its name and URI.
    #[test]
    fn a_resource_link_prints_its_name_and_uri() {
        let link = Resource::new("file:///reports/q3.csv", "q3.csv").with_title("Q3 report");
        let result = CallToolResult::success(vec![
            text_content("Here it is:"),
            ContentBlock::resource_link(link),
        ]);
        assert_eq!(
            serialize(result).unwrap(),
            "Here it is:\n[resource link: q3.csv — file:///reports/q3.csv]"
        );
    }

    /// M14: with an empty name the label is the title.
    #[test]
    fn a_resource_link_with_only_a_title_is_labelled_by_it() {
        let link = Resource::new("file:///reports/q3.csv", "").with_title("Q3 report");
        let result = CallToolResult::success(vec![ContentBlock::resource_link(link)]);
        assert_eq!(
            serialize(result).unwrap(),
            "[resource link: Q3 report — file:///reports/q3.csv]"
        );
    }

    /// M14: with neither a name nor a title (absent or empty), the URI alone.
    #[test]
    fn a_resource_link_with_neither_prints_the_uri_alone() {
        for link in [
            Resource::new("file:///reports/q3.csv", ""),
            Resource::new("file:///reports/q3.csv", "").with_title(""),
        ] {
            let result = CallToolResult::success(vec![ContentBlock::resource_link(link)]);
            assert_eq!(
                serialize(result).unwrap(),
                "[resource link: file:///reports/q3.csv]"
            );
        }
    }

    /// M14: the declared output schema is carried on the backend.
    /// Regression guard beside it: the schema never reaches the provider's
    /// tool definition (green on the parent, which carried it nowhere).
    #[test]
    fn the_output_schema_is_carried_on_the_backend_and_not_in_the_definition() {
        let schema: serde_json::Map<String, Value> = serde_json::from_value(serde_json::json!({
            "type": "object",
            "properties": {"number": {"type": "integer"}},
            "required": ["number"]
        }))
        .unwrap();
        let client = Arc::new(McpClient::disconnected_for_tests("srv"));
        let tool = Tool::new("get", "Get an issue", serde_json::Map::new())
            .with_raw_output_schema(Arc::new(schema.clone()));
        let registered = rmcp_tool_to_registered("srv", "1.0", tool, client, 0);
        match &registered.backend {
            ToolBackend::Mcp { output_schema, .. } => {
                assert_eq!(output_schema.as_deref(), Some(&schema));
            }
            _ => panic!("an MCP tool has an MCP backend"),
        }
        let definition = serde_json::to_string(&registered.definition).unwrap();
        assert!(
            !definition.contains("\"required\":[\"number\"]"),
            "the output schema leaked into the provider definition: {definition}"
        );
        assert_eq!(registered.definition.parameters, serde_json::json!({}));
        assert_eq!(registered.definition.strict, None);

        let client = Arc::new(McpClient::disconnected_for_tests("srv"));
        let tool = Tool::new("ping", "Ping", serde_json::Map::new());
        let registered = rmcp_tool_to_registered("srv", "1.0", tool, client, 0);
        match &registered.backend {
            ToolBackend::Mcp { output_schema, .. } => assert!(output_schema.is_none()),
            _ => panic!("an MCP tool has an MCP backend"),
        }
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

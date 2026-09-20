//! `GET /v1/tools` — GAP-18's shape, sort and `origin` (design §8).
//!
//! The property under test is the one S1 turns on: **there is no per-tool
//! enable state anywhere.** A builtin row carries no enable field at all, and
//! an extension tool's `origin` is a *read* of the ledger, not a switch.

use super::*;

use openalpaca_core::tools::extensions::{ExtensionId, ExtensionState};
use openalpaca_core::tools::registry::RegisteredTool;
use openalpaca_llm::types::ToolDefinition;

fn definition(name: &str) -> ToolDefinition {
    ToolDefinition {
        name: name.to_string(),
        description: format!("the {name} tool"),
        parameters: serde_json::json!({"type": "object", "properties": {}}),
        ..Default::default()
    }
}

fn mcp_tool(server: &str, name: &str) -> RegisteredTool {
    RegisteredTool {
        definition: definition(name),
        backend: ToolBackend::Mcp {
            client: Arc::new(openalpaca_mcp::McpClient::disconnected_for_tests(server)),
            remote_name: "echo".to_string(),
            server_name: server.to_string(),
            generation: 1,
        },
        provides_capabilities: vec![name.to_string()],
        exempt_from_timeout: false,
        annotations: Some(openalpaca_mcp::ToolAnnotations {
            destructive_hint: Some(true),
            ..Default::default()
        }),
        version: "1.4.0".to_string(),
        author: format!("mcp:{server}"),
        created_at: chrono::Utc::now(),
    }
}

struct Noop;

#[async_trait::async_trait]
impl openalpaca_core::tools::registry::BuiltInTool for Noop {
    async fn execute(&self, _arguments: &serde_json::Value) -> Result<String, String> {
        Ok(String::new())
    }
}

fn builtin_tool(name: &str) -> RegisteredTool {
    RegisteredTool {
        definition: definition(name),
        backend: ToolBackend::BuiltIn(Arc::new(Noop)),
        provides_capabilities: vec![name.to_string()],
        exempt_from_timeout: false,
        annotations: None,
        version: "1.0.0".to_string(),
        author: "built-in".to_string(),
        created_at: chrono::Utc::now(),
    }
}

fn config_tool(name: &str) -> RegisteredTool {
    RegisteredTool {
        definition: definition(name),
        backend: ToolBackend::Http {
            method: "GET".to_string(),
            url: "https://example.invalid/".to_string(),
            headers: Default::default(),
            timeout_secs: 5,
        },
        provides_capabilities: vec![],
        exempt_from_timeout: false,
        annotations: None,
        version: "0.1.0".to_string(),
        author: "config".to_string(),
        created_at: chrono::Utc::now(),
    }
}

struct PluginExec(&'static str);

#[async_trait::async_trait]
impl openalpaca_api::plugin_traits::PluginToolExecutor for PluginExec {
    async fn execute(&self, _tool: &str, _arguments: &serde_json::Value) -> Result<String, String> {
        Ok(String::new())
    }
    fn plugin_id(&self) -> &str {
        self.0
    }
}

fn plugin_tool(plugin: &'static str, name: &str) -> RegisteredTool {
    RegisteredTool {
        definition: definition(name),
        backend: ToolBackend::Plugin(Arc::new(PluginExec(plugin))),
        provides_capabilities: vec![name.to_string()],
        exempt_from_timeout: false,
        annotations: None,
        version: "2.0.0".to_string(),
        author: format!("plugin:{plugin}"),
        created_at: chrono::Utc::now(),
    }
}

fn find<'a>(rows: &'a [serde_json::Value], name: &str) -> &'a serde_json::Value {
    rows.iter()
        .find(|r| r["name"] == name)
        .unwrap_or_else(|| panic!("'{name}' is not in the catalog"))
}

/// A builtin is never on the ENABLE axis, so its `origin` is `null` and its row
/// has **no** enable field of any kind — not `enabled`, and not the `denied`
/// boolean this shape supersedes.
#[test]
fn a_builtin_row_carries_no_enable_field() {
    let registry = ToolRegistry::new().expect("registry");
    registry
        .register(builtin_tool("remember"))
        .expect("register a builtin");

    let rows = tools_json(&registry, registry.extensions(), &HashMap::new());
    let builtin = find(&rows, "remember");
    assert_eq!(builtin["source"], "builtin");
    assert_eq!(builtin["origin"], serde_json::Value::Null);
    for absent in ["enabled", "denied", "provider", "state"] {
        assert!(
            builtin.get(absent).is_none(),
            "a builtin row must not carry '{absent}': {builtin}"
        );
    }
    // Every §8 field is present, though.
    for field in [
        "name",
        "description",
        "source",
        "origin",
        "provides_capabilities",
        "requires_confirmation",
        "invocations_today",
        "version",
        "author",
    ] {
        assert!(
            builtin.get(field).is_some(),
            "the row is missing '{field}': {builtin}"
        );
    }
}

/// `origin.enabled` / `origin.state` are a **read** of the ledger, taken at
/// render time. Availability is derived; nothing here is a switch.
#[test]
fn an_mcp_rows_origin_tracks_the_ledger() {
    let registry = ToolRegistry::new().expect("registry");
    let ext = ExtensionId::mcp("github");
    registry.extensions().upsert(&ext, true, ExtensionState::Enabled);
    registry
        .register(mcp_tool("github", "github__create_issue"))
        .expect("register the mcp tool");

    let rows = tools_json(&registry, registry.extensions(), &HashMap::new());
    let row = find(&rows, "github__create_issue");
    assert_eq!(row["source"], "mcp");
    assert_eq!(
        row["origin"],
        serde_json::json!({"kind": "mcp", "id": "github", "enabled": true, "state": "enabled"})
    );
    assert_eq!(row["author"], "mcp:github");
    assert_eq!(row["version"], "1.4.0");
    assert_eq!(
        row["requires_confirmation"], true,
        "requires_confirmation is the destructive hint"
    );
    assert_eq!(
        row["provides_capabilities"],
        serde_json::json!(["github__create_issue"])
    );

    // The ledger moves; the row follows, with no re-registration.
    registry
        .extensions()
        .upsert(&ext, false, ExtensionState::Disabled);
    let rows = tools_json(&registry, registry.extensions(), &HashMap::new());
    let row = find(&rows, "github__create_issue");
    assert_eq!(row["origin"]["enabled"], false);
    assert_eq!(row["origin"]["state"], "disabled");
}

/// §6.2a: an extension with no ledger entry means *"no supervisor owns this
/// yet"*, not *"disabled"*, and the catalog says so the same way the gate does.
#[test]
fn an_unrecorded_extension_tool_reads_as_enabled() {
    let registry = ToolRegistry::new().expect("registry");
    registry
        .register(mcp_tool("unrecorded", "unrecorded__echo"))
        .expect("register");

    let rows = tools_json(&registry, registry.extensions(), &HashMap::new());
    let row = find(&rows, "unrecorded__echo");
    assert_eq!(row["origin"]["enabled"], true);
    assert_eq!(row["origin"]["state"], "enabled");
}

/// A `config/tools/*.toml` tool is `source: "config"` and, like a builtin, has
/// no origin: only MCP servers and plugins are extensions.
#[test]
fn a_config_tool_has_no_origin() {
    let registry = ToolRegistry::new().expect("registry");
    registry.register(config_tool("weather")).expect("register");

    let rows = tools_json(&registry, registry.extensions(), &HashMap::new());
    let row = find(&rows, "weather");
    assert_eq!(row["source"], "config");
    assert_eq!(row["origin"], serde_json::Value::Null);
    assert_eq!(
        row["requires_confirmation"], false,
        "no annotations means no confirmation"
    );
}

/// `DashMap` iteration jitters, so the array is sorted by name and two reads of
/// an unchanged registry are byte-identical.
#[test]
fn the_catalog_is_sorted_by_name() {
    let registry = ToolRegistry::new().expect("registry");
    for name in ["zzz__last", "aaa__first", "mmm__middle"] {
        registry.register(mcp_tool("srv", name)).expect("register");
    }

    let rows = tools_json(&registry, registry.extensions(), &HashMap::new());
    let names: Vec<&str> = rows.iter().map(|r| r["name"].as_str().unwrap()).collect();
    let mut sorted = names.clone();
    sorted.sort_unstable();
    assert_eq!(names, sorted, "the catalog must be sorted");
    assert_eq!(
        rows,
        tools_json(&registry, registry.extensions(), &HashMap::new()),
        "two reads of an unchanged registry must agree"
    );
}

/// `invocations_today` is the count for **that name**, and `0` — not `null` —
/// for a tool nobody has called today.
#[test]
fn invocations_today_is_per_name_and_defaults_to_zero() {
    let registry = ToolRegistry::new().expect("registry");
    registry
        .register(mcp_tool("srv", "srv__called"))
        .expect("register");
    registry
        .register(mcp_tool("srv", "srv__uncalled"))
        .expect("register");

    let counts = HashMap::from([("srv__called".to_string(), 12i64)]);
    let rows = tools_json(&registry, registry.extensions(), &counts);
    assert_eq!(find(&rows, "srv__called")["invocations_today"], 12);
    assert_eq!(find(&rows, "srv__uncalled")["invocations_today"], 0);
}

/// The fourth `source` value, and the second `origin.kind`. A plugin tool's
/// origin is read through `RegisteredTool::extension_id()`, which derives the
/// plugin's **directory** name from the `plugin:<id>` author prefix
/// (`registry/mod.rs:220`) — the same key the ledger and the supervisor use.
#[test]
fn a_plugin_row_carries_its_plugins_origin() {
    let registry = ToolRegistry::new().expect("registry");
    let ext = ExtensionId::plugin("notion");
    registry
        .extensions()
        .upsert(&ext, false, ExtensionState::Disabled);
    registry
        .register(plugin_tool("notion", "notion::create_page"))
        .expect("register the plugin tool");

    let rows = tools_json(&registry, registry.extensions(), &HashMap::new());
    let row = find(&rows, "notion::create_page");
    assert_eq!(row["source"], "plugin");
    assert_eq!(
        row["origin"],
        serde_json::json!({
            "kind": "plugin", "id": "notion", "enabled": false, "state": "disabled"
        }),
        "a plugin row reads the ledger the same way an MCP row does"
    );
    assert_eq!(row["author"], "plugin:notion");
    assert_eq!(row["version"], "2.0.0");
}

/// **The §8 shape, exactly** — nine keys per row and no tenth, on every one of
/// the four sources. The two fields this contract supersedes (`denied`, the
/// ADR-029 boolean) and folds away (`provider`, into `origin.id`) are covered
/// by the same assertion: any key not in §8 fails it.
#[test]
fn every_row_carries_the_nine_section_8_keys_and_no_others() {
    let registry = ToolRegistry::new().expect("registry");
    registry.register(builtin_tool("remember")).expect("builtin");
    registry.register(config_tool("weather")).expect("config");
    registry
        .register(mcp_tool("github", "github__create_issue"))
        .expect("mcp");
    registry
        .register(plugin_tool("notion", "notion::create_page"))
        .expect("plugin");

    let expected = [
        "name",
        "description",
        "source",
        "origin",
        "provides_capabilities",
        "requires_confirmation",
        "invocations_today",
        "version",
        "author",
    ];
    let rows = tools_json(&registry, registry.extensions(), &HashMap::new());
    assert_eq!(rows.len(), 4);
    for row in &rows {
        let object = row.as_object().expect("each row is an object");
        let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
        keys.sort_unstable();
        let mut want = expected;
        want.sort_unstable();
        assert_eq!(keys, want, "row is not the §8 shape: {row}");
        // Types, field for field.
        assert!(row["name"].is_string());
        assert!(row["description"].is_string());
        assert!(row["provides_capabilities"].is_array());
        assert!(row["requires_confirmation"].is_boolean());
        assert!(row["invocations_today"].is_i64());
        assert!(row["version"].is_string());
        assert!(row["author"].is_string());
        assert!(
            ["builtin", "mcp", "plugin", "config"].contains(&row["source"].as_str().unwrap()),
            "unknown source: {row}"
        );
        // `origin` is null exactly for the two off-axis sources.
        let off_axis = matches!(row["source"].as_str(), Some("builtin") | Some("config"));
        assert_eq!(
            row["origin"].is_null(),
            off_axis,
            "the origin-null rule does not hold for {row}"
        );
    }
}

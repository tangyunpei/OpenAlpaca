//! E2E: spin up the reference filesystem MCP server, exercise the full pipeline
//! from McpClient::connect → list_tools → bridge → ToolRegistry::execute_with_context.
//!
//! Prerequisites: npx + Node.js in PATH.
//! Run with: `cargo test -p openalpaca_core -- --ignored`

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use openalpaca_core::tools::{
    mcp::bridge,
    registry::{ToolContext, ToolRegistry},
};
use openalpaca_mcp::{Implementation, McpClient, McpClientConfig, TransportKind};

fn mk_config(tmp_canonical: &std::path::Path) -> McpClientConfig {
    McpClientConfig {
        server_name: "fs".into(),
        transport: TransportKind::Stdio {
            command: "npx".into(),
            args: vec![
                "-y".into(),
                "@modelcontextprotocol/server-filesystem".into(),
                tmp_canonical.to_string_lossy().into_owned(),
            ],
            env: HashMap::new(),
            cwd: None,
        },
        client_info: Implementation {
            name: "openalpaca-mcp-p2-test".into(),
            version: "0.1.0".into(),
            ..Default::default()
        },
        request_timeout: Duration::from_secs(30),
        max_reconnect_attempts: 1,
        reconnect_backoff_ms: 100,
    }
}

#[tokio::test]
#[ignore = "requires npx and Node.js in PATH"]
async fn mcp_tool_flows_through_registry_execute_with_context() {
    let tmp = tempfile::tempdir().unwrap();
    // macOS canonicalizes /var → /private/var; the filesystem MCP server
    // enforces allowed-roots after canonicalization.
    let tmp_canonical = std::fs::canonicalize(tmp.path()).unwrap();
    std::fs::write(tmp_canonical.join("hello.txt"), "world").unwrap();

    // Connect client.
    let client = McpClient::connect(mk_config(&tmp_canonical)).await.expect("connect");
    let client = Arc::new(client);

    // Discover tools.
    let tools = client.list_tools(None).await.expect("list_tools");
    assert!(!tools.is_empty(), "filesystem server should expose tools");

    // Build a fresh tool registry and register all discovered tools.
    let registry = ToolRegistry::new().expect("new registry");
    for tool in tools {
        let reg = bridge::rmcp_tool_to_registered("fs", "test-v1", tool, Arc::clone(&client));
        registry.register(reg).expect("register");
    }

    // Verify namespacing — server exposes either read_file or read_text_file depending on version.
    let names = registry.registered_tool_names();
    let read_tool = names
        .iter()
        .find(|n| n.as_str() == "fs__read_file" || n.as_str() == "fs__read_text_file")
        .cloned()
        .unwrap_or_else(|| panic!(
            "expected fs__read_file or fs__read_text_file; got: {names:?}"
        ));

    // Execute via the registry (exercises ToolBackend::Mcp dispatch).
    let args = serde_json::json!({
        "path": tmp_canonical.join("hello.txt").to_string_lossy(),
    });
    let result = registry
        .execute_with_context(&read_tool, &args, &ToolContext::default())
        .await
        .expect("execute_with_context");

    assert!(
        result.contains("world"),
        "tool result should contain file content: {result:?}"
    );

    // Cleanup — drop registry first to release its Arc, then disconnect client.
    drop(registry);
    if let Ok(client) = Arc::try_unwrap(client) {
        let _ = client.disconnect().await;
    }
}

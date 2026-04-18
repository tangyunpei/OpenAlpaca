// crates/openalpaca_mcp/tests/filesystem_e2e.rs

//! End-to-end test: spawn the reference filesystem MCP server via npx,
//! list its tools, call read_file, verify the result. Also exercises
//! cancellation.
//!
//! Prerequisites: Node.js and `npx` available in PATH.
//! Run with: `cargo test -p openalpaca_mcp -- --ignored`

use std::collections::HashMap;
use std::time::Duration;

use openalpaca_mcp::{
    Implementation, McpClient, McpClientConfig, McpError, TransportKind,
};

fn mk_config(tmp: &std::path::Path) -> McpClientConfig {
    McpClientConfig {
        server_name: "filesystem-test".into(),
        transport: TransportKind::Stdio {
            command: "npx".into(),
            args: vec![
                "-y".into(),
                "@modelcontextprotocol/server-filesystem".into(),
                tmp.to_string_lossy().into_owned(),
            ],
            env: HashMap::new(),
            cwd: None,
        },
        client_info: Implementation {
            name: "openalpaca-mcp-test".into(),
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
async fn filesystem_server_list_and_call_read_file() {
    let tmp = tempfile::tempdir().expect("tempdir");
    // Canonicalize the tempdir path. On macOS /var/folders/... is a symlink to
    // /private/var/folders/... and the filesystem server refuses paths that
    // don't match its (canonicalized) allowed roots.
    let tmp_path = std::fs::canonicalize(tmp.path()).expect("canonicalize tempdir");
    std::fs::write(tmp_path.join("hello.txt"), "world").unwrap();

    let client = McpClient::connect(mk_config(&tmp_path)).await.expect("connect");

    // 1. list tools
    let tools = client.list_tools(None).await.expect("list_tools");
    assert!(
        tools.iter().any(|t| t.name.as_ref() == "read_file"),
        "filesystem server should expose read_file; got: {:?}",
        tools.iter().map(|t| t.name.to_string()).collect::<Vec<_>>()
    );

    // 2. call read_file
    let result = client
        .call_tool(
            "read_file",
            serde_json::json!({ "path": tmp_path.join("hello.txt").to_string_lossy() }),
            None,
        )
        .await
        .expect("call_tool");

    // rmcp's CallToolResult has a content field; extract the text body.
    let text = result
        .content
        .iter()
        .find_map(|c| match c.raw {
            rmcp::model::RawContent::Text(ref t) => Some(t.text.clone()),
            _ => None,
        })
        .expect("read_file should return text content");
    assert!(text.contains("world"), "expected file contents in result, got: {text}");

    // 3. verify we got server identity from handshake
    let info = client.server_info().expect("server_info");
    assert!(
        !info.name.is_empty() && !info.version.is_empty(),
        "server info should be populated: {info:?}"
    );

    client.disconnect().await.expect("disconnect");
}

#[tokio::test]
#[ignore = "requires npx and Node.js in PATH"]
async fn filesystem_call_tool_cancellation_returns_cancelled() {
    use tokio_util::sync::CancellationToken;

    let tmp = tempfile::tempdir().unwrap();
    let tmp_path = std::fs::canonicalize(tmp.path()).expect("canonicalize tempdir");
    let client = McpClient::connect(mk_config(&tmp_path)).await.expect("connect");

    let token = CancellationToken::new();
    token.cancel(); // pre-cancelled

    let err = client
        .call_tool(
            "read_file",
            serde_json::json!({ "path": "/tmp/anything" }),
            Some(&token),
        )
        .await
        .expect_err("expected error");

    assert!(
        matches!(err, McpError::Cancelled),
        "expected Cancelled, got {err:?}"
    );

    client.disconnect().await.expect("disconnect");
}

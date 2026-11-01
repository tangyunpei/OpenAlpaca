//! [`ToolRegistry::precheck`]: dispatch's refusals, asked as a question
//! before a confirmation card (N-TOOL-2, audit Z1-04).
//!
//! Each test compares the precheck with what dispatch itself answers for the
//! same call, so "the same refusals, only earlier" is checked, not assumed.

use super::*;
use crate::tools::extensions::{ExtensionId, ExtensionState, Transition, WithdrawalCause};
use std::sync::atomic::{AtomicUsize, Ordering};

/// A builtin that counts the times it ran.
#[derive(Default)]
struct Counted {
    runs: AtomicUsize,
}

#[async_trait]
impl BuiltInTool for Counted {
    async fn execute(&self, _arguments: &serde_json::Value) -> Result<String, String> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        Ok("ran".to_string())
    }
}

fn registered(name: &str, parameters: serde_json::Value, backend: ToolBackend) -> RegisteredTool {
    RegisteredTool {
        definition: ToolDefinition {
            name: name.to_string(),
            description: name.to_string(),
            parameters,
            strict: None,
            input_examples: None,
        },
        backend,
        provides_capabilities: vec![],
        exempt_from_timeout: false,
        annotations: None,
        version: "test".into(),
        author: "test".into(),
        created_at: chrono::Utc::now(),
    }
}

fn note_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {"path": {"type": "string"}},
        "required": ["path"]
    })
}

fn ctx() -> ToolContext {
    ToolContext {
        task_id: Some("task-1".into()),
        ..Default::default()
    }
}

/// An enabled MCP server `github` serving `github__create_issue`, whose
/// handle carries the load's generation.
fn with_github(registry: &ToolRegistry, parameters: serde_json::Value) -> ExtensionId {
    let ext = ExtensionId::mcp("github");
    let ledger = registry.extensions();
    let generation = match ledger.begin(&ext, ExtensionState::Enabling, None) {
        Transition::Took(g) => g,
        other => panic!("E0 refused: {other:?}"),
    };
    ledger.restore(&ext);
    ledger.record_tools(&ext, ["github__create_issue".to_string()]);
    assert!(ledger.commit(&ext, ExtensionState::Enabled));
    registry
        .register(registered(
            "github__create_issue",
            parameters,
            ToolBackend::Mcp {
                client: Arc::new(openalpaca_mcp::McpClient::disconnected_for_tests("github")),
                remote_name: "create_issue".to_string(),
                server_name: "github".to_string(),
                generation,
            },
        ))
        .unwrap();
    ext
}

fn disable(registry: &ToolRegistry, ext: &ExtensionId) {
    let ledger = registry.extensions();
    ledger.begin(
        ext,
        ExtensionState::Disabling,
        Some(WithdrawalCause::Disable),
    );
    assert!(ledger.commit(ext, ExtensionState::Disabled));
}

#[tokio::test]
async fn an_argument_the_schema_refuses_is_dispatchs_own_refusal() {
    let tool = Arc::new(Counted::default());
    let registry = ToolRegistry::default();
    registry
        .register(registered(
            "write_note",
            note_schema(),
            ToolBackend::BuiltIn(tool.clone()),
        ))
        .unwrap();
    let arguments = serde_json::json!({});

    let early = registry
        .precheck("write_note", &arguments, Some(&ctx()))
        .expect_err("the precheck refuses a missing required parameter");
    let late = registry
        .execute_with_context("write_note", &arguments, &ctx())
        .await
        .expect_err("and so does dispatch");

    assert_eq!(early, late);
    assert_eq!(
        early,
        "Tool 'write_note': missing required parameter 'path'"
    );
    assert_eq!(tool.runs.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_disabled_extensions_tool_is_dispatchs_own_refusal_and_holds_nothing() {
    let registry = ToolRegistry::default();
    let ext = with_github(&registry, serde_json::json!({"type": "object"}));
    disable(&registry, &ext);
    let arguments = serde_json::json!({"title": "x"});

    let early = registry
        .precheck("github__create_issue", &arguments, Some(&ctx()))
        .expect_err("the precheck withholds a disabled extension's tool");
    let late = registry
        .execute_with_context("github__create_issue", &arguments, &ctx())
        .await
        .expect_err("and so does dispatch");

    assert_eq!(early, late);
    assert!(
        crate::tools::extensions::is_withheld_refusal(&early),
        "{early}"
    );
    assert_eq!(registry.extensions().in_flight(&ext), 0);
}

/// The gate comes before the schema, at dispatch and so in the precheck: a
/// disabled extension's tool called with bad arguments reads as withheld.
#[tokio::test]
async fn the_gate_is_asked_before_the_schema() {
    let registry = ToolRegistry::default();
    let ext = with_github(&registry, note_schema());
    disable(&registry, &ext);

    let early = registry
        .precheck("github__create_issue", &serde_json::json!({}), Some(&ctx()))
        .expect_err("refused");

    assert!(
        crate::tools::extensions::is_withheld_refusal(&early),
        "{early}"
    );
    assert!(!early.contains("missing required parameter"), "{early}");
}

/// The miss arm: the live registry no longer holds the name, the ledger still
/// attributes it — the precheck gives dispatch's attributed refusal, and an
/// unknown name dispatch's "not found".
#[tokio::test]
async fn a_name_the_registry_lost_is_refused_as_dispatch_refuses_it() {
    let registry = ToolRegistry::default();
    let ext = with_github(&registry, serde_json::json!({"type": "object"}));
    disable(&registry, &ext);
    assert!(registry.remove("github__create_issue"));
    let arguments = serde_json::json!({});

    for name in ["github__create_issue", "never_registered"] {
        let early = registry
            .precheck(name, &arguments, Some(&ctx()))
            .expect_err("refused");
        let late = registry
            .execute_with_context(name, &arguments, &ctx())
            .await
            .expect_err("refused");
        assert_eq!(early, late, "{name}");
    }
}

/// An enabled extension's valid call passes, and the precheck neither runs
/// the tool nor leaves a call counted against the drain.
#[tokio::test]
async fn a_valid_call_passes_and_nothing_runs() {
    let tool = Arc::new(Counted::default());
    let registry = ToolRegistry::default();
    registry
        .register(registered(
            "write_note",
            note_schema(),
            ToolBackend::BuiltIn(tool.clone()),
        ))
        .unwrap();
    let ext = with_github(&registry, serde_json::json!({"type": "object"}));

    assert_eq!(
        registry.precheck(
            "write_note",
            &serde_json::json!({"path": "a.md"}),
            Some(&ctx())
        ),
        Ok(())
    );
    assert_eq!(
        registry.precheck("github__create_issue", &serde_json::json!({}), Some(&ctx())),
        Ok(())
    );
    assert_eq!(
        tool.runs.load(Ordering::SeqCst),
        0,
        "a precheck never runs the tool"
    );
    assert_eq!(registry.extensions().in_flight(&ext), 0);
}

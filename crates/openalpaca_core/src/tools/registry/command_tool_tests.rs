//! `ToolRegistry::is_command_tool` — which tools run a command (N-TOOL-4).
//!
//! Two consumers read the one predicate: the sandbox's timeout arm, which
//! hints a command it cut at its time limit, and — from N-TOOL-7 — the
//! loop's spill, which previews a command's output head **and** tail. Both arms of it
//! are pinned here — the sanitizer's shell set by name, and every
//! `Command`-backend tool, whatever its name — so dropping either arm fails
//! a test rather than quietly taking a TOML command tool's hint and stub
//! with it.

use super::*;

struct NoopTool;

#[async_trait::async_trait]
impl BuiltInTool for NoopTool {
    async fn execute(&self, _arguments: &serde_json::Value) -> Result<String, String> {
        Ok(String::new())
    }
}

fn register(registry: &ToolRegistry, name: &str, backend: ToolBackend) {
    registry
        .register(RegisteredTool {
            definition: ToolDefinition {
                name: name.to_string(),
                description: name.to_string(),
                parameters: serde_json::json!({"type": "object"}),
                strict: None,
                input_examples: None,
            },
            backend,
            provides_capabilities: vec![],
            exempt_from_timeout: false,
            annotations: None,
            version: "test-0.0.0".into(),
            author: "test".into(),
            created_at: chrono::Utc::now(),
        })
        .unwrap();
}

#[test]
fn a_command_tool_is_the_shell_by_name_or_any_command_backend() {
    let registry = ToolRegistry::default();
    register(
        &registry,
        "lint_project",
        ToolBackend::Command {
            command: "true".to_string(),
            args_template: None,
            timeout_secs: 5,
        },
    );
    register(
        &registry,
        "file_read",
        ToolBackend::BuiltIn(Arc::new(NoopTool)),
    );

    assert!(
        registry.is_command_tool("shell_execute"),
        "the sanitizer's shell set, by name, registered or not"
    );
    assert!(
        registry.is_command_tool("lint_project"),
        "a Command-backend tool, whatever its name"
    );
    assert!(
        !registry.is_command_tool("file_read"),
        "a builtin outside the shell set runs no command"
    );
    assert!(
        !registry.is_command_tool("no_such_tool"),
        "a name nothing registered is not a command"
    );
}

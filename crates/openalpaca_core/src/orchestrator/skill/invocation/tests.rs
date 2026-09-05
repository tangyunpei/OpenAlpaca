//! A0 (bug A): a plugin skill whose providing extension is absent or disabled
//! must lose reach, never gain it.

use super::*;
use crate::middleware::skill::SkillFrontmatter;
use crate::security::capabilities::CapabilityManager;
use crate::tools::ToolRegistry;
use crate::tools::registry::{BuiltInTool, RegisteredTool, ToolBackend};

struct NoopTool;

#[async_trait::async_trait]
impl BuiltInTool for NoopTool {
    async fn execute(&self, _arguments: &serde_json::Value) -> Result<String, String> {
        Ok(String::new())
    }
}

fn register(registry: &ToolRegistry, name: &str, provides: Vec<String>) {
    registry
        .register(RegisteredTool {
            definition: openalpaca_llm::ToolDefinition {
                name: name.to_string(),
                description: name.to_string(),
                parameters: serde_json::json!({"type": "object"}),
                strict: None,
                input_examples: None,
            },
            backend: ToolBackend::BuiltIn(Arc::new(NoopTool)),
            provides_capabilities: provides,
            exempt_from_timeout: false,
            annotations: None,
            version: "test-0.0.0".into(),
            author: "test".into(),
            created_at: Utc::now(),
        })
        .unwrap();
}

/// A registry holding one unrelated builtin plus, when `with_extension`, the
/// extension tool the skill under test declares as its requirement.
fn registry(with_extension: bool) -> ToolRegistry {
    let registry = ToolRegistry::default();
    register(&registry, "shell_execute", vec![]);
    if with_extension {
        register(&registry, "acme__search", vec!["acme__search".to_string()]);
    }
    registry
}

/// Would the sandbox refuse `tool` for a plugin skill carrying `allowed`?
fn denies(allowed: &Allowlist, tool: &str) -> bool {
    CapabilityManager::check_agent_capability("plugin:acme", tool, allowed, &[]).is_err()
}

#[test]
fn plugin_skill_total_loss_cannot_call_unrelated_builtin() {
    let fm = SkillFrontmatter {
        name: "acme-search".to_string(),
        requires_capabilities: vec!["acme__search".to_string()],
        ..Default::default()
    };

    // Extension installed: the skill is scoped to the tool it asked for.
    let allowed = plugin_skill_allowlist("acme-search", &fm, &registry(true)).unwrap();
    assert_eq!(allowed, Allowlist::Only(vec!["acme__search".to_string()]));
    assert!(!denies(&allowed, "acme__search"));
    assert!(denies(&allowed, "shell_execute"));

    // Extension absent or disabled: the requirement resolves to nothing. The
    // skill is refused up front instead of running with a widened reach.
    let err = plugin_skill_allowlist("acme-search", &fm, &registry(false))
        .expect_err("a skill whose required capabilities resolve to nothing must be refused");
    assert!(err.contains("acme-search"), "refusal must name the skill: {err}");
    assert!(
        err.contains("acme__search"),
        "refusal must name the unresolved capabilities: {err}"
    );

    // And the empty resolution it would otherwise have carried admits nothing.
    assert!(
        denies(&Allowlist::Only(vec![]), "shell_execute"),
        "a plugin skill that lost its extension must not reach an unrelated builtin"
    );
}

#[test]
fn plugin_skill_with_no_lists_cannot_call_any_tool() {
    let fm = SkillFrontmatter {
        name: "bare".to_string(),
        ..Default::default()
    };

    // No `requires_capabilities` and no `tools.allow`: nothing to refuse up
    // front, but nothing granted either.
    let allowed = plugin_skill_allowlist("bare", &fm, &registry(true)).unwrap();
    assert_eq!(allowed, Allowlist::Only(vec![]));
    for tool in ["shell_execute", "acme__search", "workspace_read"] {
        assert!(
            denies(&allowed, tool),
            "a plugin skill declaring no capabilities and no allow list must reach no tool, \
             but '{tool}' was permitted"
        );
    }
}

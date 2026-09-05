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
    let (allowed, _) =
        plugin_skill_allowlist("acme-search", &fm, &registry(true), "scope").unwrap();
    assert_eq!(allowed, Allowlist::Only(vec!["acme__search".to_string()]));
    assert!(!denies(&allowed, "acme__search"));
    assert!(denies(&allowed, "shell_execute"));

    // Extension absent or disabled: the requirement resolves to nothing. The
    // skill is refused up front instead of running with a widened reach.
    let err = plugin_skill_allowlist("acme-search", &fm, &registry(false), "scope")
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
    let (allowed, _) = plugin_skill_allowlist("bare", &fm, &registry(true), "scope").unwrap();
    assert_eq!(allowed, Allowlist::Only(vec![]));
    for tool in ["shell_execute", "acme__search", "workspace_read"] {
        assert!(
            denies(&allowed, tool),
            "a plugin skill declaring no capabilities and no allow list must reach no tool, \
             but '{tool}' was permitted"
        );
    }
}

// ---------------------------------------------------------------------------
// C5 — the same property, END TO END through `invoke_plugin_skill`
// ---------------------------------------------------------------------------

use crate::bus::EventBus;
use crate::context::SharedContext;
use crate::daemon_config::DaemonConfig;
use crate::lane::LaneManager;
use crate::memory::scope_context::MemoryScopeContext;
use crate::middleware::skill::SkillDocument;
use crate::orchestrator::{Orchestrator, skill_catalog, skill_router};
use crate::runner::LoopConfig;
use crate::security::gate::SecurityGate;
use crate::security::sandbox::SandboxManager;
use crate::tools::extensions::{ExtensionId, ExtensionState, WithdrawalCause};
use arc_swap::ArcSwap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

/// A plugin that does exactly one thing when invoked: reach for a tool through
/// the sandboxed callback and report what came back.
struct StubPluginSkill {
    tool: String,
    invoked: Arc<AtomicBool>,
    outcome: Arc<Mutex<Option<Result<String, String>>>>,
}

#[async_trait::async_trait]
impl openalpaca_api::plugin_traits::PluginSkillExecutor for StubPluginSkill {
    async fn invoke(
        &self,
        _query: &str,
        _context: &serde_json::Value,
        tool_executor: &dyn openalpaca_api::plugin_traits::ToolCallbackExecutor,
    ) -> Result<String, String> {
        self.invoked.store(true, Ordering::SeqCst);
        let outcome = tool_executor
            .execute_tool(&self.tool, &serde_json::json!({}))
            .await;
        *self.outcome.lock().unwrap() = Some(outcome);
        Ok("stub finished".to_string())
    }
    fn plugin_id(&self) -> &str {
        "acme"
    }
    fn skill_id(&self) -> &str {
        "acme-search"
    }
}

fn orchestrator_for(registry: Arc<crate::tools::ToolRegistry>) -> Orchestrator {
    let bus = EventBus::default();
    let sandbox = Arc::new(SandboxManager::with_defaults(registry.clone(), bus.clone()));
    Orchestrator::new(
        Arc::new(SharedContext::new()),
        Arc::new(LaneManager::new()),
        bus,
        crate::middleware::prompt::SystemPersona::default(),
        None,
        LoopConfig::default(),
        Arc::new(SecurityGate::new(sandbox)),
        registry,
        None,
        None,
        Arc::new(skill_catalog::SkillCatalog::new()),
        Arc::new(skill_router::SkillRouter::new(0.65, 0.45)),
        Arc::new(ArcSwap::from_pointee(DaemonConfig::default())),
    )
}

fn skill_doc(fm: SkillFrontmatter) -> SkillDocument {
    SkillDocument {
        frontmatter: fm,
        body: "stub".to_string(),
        sections: Default::default(),
    }
}

async fn run_plugin_skill(
    orch: &Orchestrator,
    fm: SkillFrontmatter,
    stub: Arc<StubPluginSkill>,
) -> Result<String, String> {
    let doc = skill_doc(fm);
    orch.invoke_plugin_skill(
        Uuid::new_v4(),
        "cli",
        "acme-search",
        "acme",
        stub,
        "go",
        "test:cli",
        None,
        &MemoryScopeContext::new(None),
        None,
        &doc,
    )
    .await
    .map(|r| r.content)
}

/// **The escalation, closed end to end.** A plugin skill whose every capability
/// is withheld is refused before the plugin is invoked at all — and the empty
/// allow list it would otherwise have carried denies `shell_execute` at the
/// callee, so neither half alone is load-bearing (design §6.2 #11).
#[tokio::test]
async fn plugin_skill_total_loss_cannot_call_unrelated_builtin_end_to_end() {
    // The escalation's real shape: the plugin providing the skill is fine; the
    // **MCP server** whose capability the skill declared has been disabled.
    // Before A0 the resolution went to `allowed_capabilities: vec![]`, which
    // `check_agent_capability` read as ALLOW EVERYTHING — so disabling an
    // extension *widened* this skill's reach to every tool in the registry.
    let registry = Arc::new(registry(true));
    register(
        &registry,
        "github__create_issue",
        vec!["github_issues".to_string()],
    );
    let github = ExtensionId::mcp("github");
    let ledger = registry.extensions();
    ledger.upsert(&github, true, ExtensionState::Enabled);
    ledger.record_tools(&github, ["github__create_issue"]);
    ledger.begin(
        &github,
        ExtensionState::Disabling,
        Some(WithdrawalCause::Disable),
    );
    ledger.withdraw(&github, ["github_issues".to_string()]);
    registry.remove("github__create_issue");
    ledger.commit(&github, ExtensionState::Disabled);

    let orch = orchestrator_for(registry);
    let invoked = Arc::new(AtomicBool::new(false));
    let outcome = Arc::new(Mutex::new(None));
    let stub = Arc::new(StubPluginSkill {
        tool: "shell_execute".to_string(),
        invoked: invoked.clone(),
        outcome: outcome.clone(),
    });

    let fm = SkillFrontmatter {
        name: "acme-search".to_string(),
        requires_capabilities: vec!["github_issues".to_string()],
        ..Default::default()
    };
    let err = run_plugin_skill(&orch, fm, stub)
        .await
        .expect_err("a plugin skill whose every capability is withheld must be refused");

    assert!(err.contains("acme-search"), "names the skill: {err}");
    assert!(err.contains("github_issues"), "names the capability: {err}");
    assert!(err.contains("github"), "names the extension: {err}");
    assert!(
        err.contains("Settings → Extensions"),
        "names the remedy: {err}"
    );
    assert!(
        !invoked.load(Ordering::SeqCst),
        "the plugin must never be invoked — the refusal is up front"
    );
    assert!(
        outcome.lock().unwrap().is_none(),
        "and no tool call was ever proxied"
    );
}

/// The callee half, end to end: a plugin skill that *does* run with an empty
/// allow list cannot reach an unrelated builtin through `SandboxToolCallback`.
/// This is the path `allowed_capabilities` feeds and A0 could only read.
#[tokio::test]
async fn plugin_skill_with_no_lists_cannot_call_any_tool_end_to_end() {
    let registry = Arc::new(registry(true));
    let orch = orchestrator_for(registry);
    let invoked = Arc::new(AtomicBool::new(false));
    let outcome = Arc::new(Mutex::new(None));
    let stub = Arc::new(StubPluginSkill {
        tool: "shell_execute".to_string(),
        invoked: invoked.clone(),
        outcome: outcome.clone(),
    });

    let fm = SkillFrontmatter {
        name: "bare".to_string(),
        ..Default::default()
    };
    let content = run_plugin_skill(&orch, fm, stub)
        .await
        .expect("a skill declaring nothing still runs — it just cannot call anything");
    assert_eq!(content, "stub finished");

    assert!(invoked.load(Ordering::SeqCst), "the plugin did run");
    let refusal = outcome
        .lock()
        .unwrap()
        .clone()
        .expect("the stub called a tool")
        .expect_err("an empty allow list must deny shell_execute at the callback");
    assert!(
        refusal.to_lowercase().contains("shell_execute"),
        "the refusal names the tool it denied: {refusal}"
    );
}

/// The `Allowlist::Only` lowercase contract, at a construction site
/// (X-23, A0's deferred finding). A mixed-case MCP or plugin tool name in
/// `tools.allow` used to deny the very tool it was declared to admit.
#[test]
fn a_mixed_case_allowed_name_is_lowercased_at_construction() {
    let mut fm = SkillFrontmatter {
        name: "mixed".to_string(),
        ..Default::default()
    };
    fm.tools.allow = vec!["Acme__Search".to_string(), "Notion::Query".to_string()];

    let (allowed, _) = plugin_skill_allowlist("mixed", &fm, &registry(true), "scope").unwrap();
    assert_eq!(
        allowed,
        Allowlist::Only(vec!["acme__search".to_string(), "notion::query".to_string()]),
        "the allow list is normalized at construction, not left verbatim"
    );
    for tool in ["Acme__Search", "acme__search", "NOTION::query"] {
        assert!(
            !denies(&allowed, tool),
            "'{tool}' must be admitted by the list that declared it"
        );
    }
    assert!(denies(&allowed, "shell_execute"));
}

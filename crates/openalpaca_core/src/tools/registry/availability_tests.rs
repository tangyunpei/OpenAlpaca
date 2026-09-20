//! C5 — **the one predicate**, unit level (design §10 case 3, §6.2 #10/#12).
//!
//! `is_satisfiable` is "no requested capability is wholly withheld"; on the
//! legacy `tools.allow` arm it is "not *every* allowed name is withdrawn". The
//! same value gates invocation, router candidacy, `<available_skills>`, the
//! cron skip and `/slash`, so these tests pin the rule once.

use super::*;
use crate::middleware::skill::SkillFrontmatter;
use crate::tools::extensions::{ExtensionId, ExtensionState, WithdrawalCause};
use crate::tools::registry::{BuiltInTool, RegisteredTool, ToolBackend};
use openalpaca_llm::ToolDefinition;
use std::sync::Arc;

struct NoopTool;

#[async_trait::async_trait]
impl BuiltInTool for NoopTool {
    async fn execute(&self, _arguments: &serde_json::Value) -> Result<String, String> {
        Ok(String::new())
    }
}

fn register(registry: &ToolRegistry, name: &str, provides: &[&str]) {
    registry
        .register(RegisteredTool {
            definition: ToolDefinition {
                name: name.to_string(),
                description: name.to_string(),
                parameters: serde_json::json!({"type": "object"}),
                strict: None,
                input_examples: None,
            },
            backend: ToolBackend::BuiltIn(Arc::new(NoopTool)),
            provides_capabilities: provides.iter().map(|s| s.to_string()).collect(),
            exempt_from_timeout: false,
            annotations: None,
            version: "test-0.0.0".into(),
            author: "test".into(),
            created_at: chrono::Utc::now(),
        })
        .unwrap();
}

/// T0–T5 as a supervisor runs them: the capabilities are tombstoned, the tools
/// leave the registry, the names stay attributed.
fn disable(registry: &ToolRegistry, ext: &ExtensionId, tools: &[&str], caps: &[&str]) {
    let ledger = registry.extensions();
    ledger.begin(
        ext,
        ExtensionState::Disabling,
        Some(WithdrawalCause::Disable),
    );
    ledger.withdraw(ext, caps.iter().map(|c| c.to_string()));
    for tool in tools {
        registry.remove(tool);
    }
    ledger.commit(ext, ExtensionState::Disabled);
}

fn enable(registry: &ToolRegistry, ext: &ExtensionId, tools: &[&str]) {
    let ledger = registry.extensions();
    ledger.upsert(ext, true, ExtensionState::Enabled);
    ledger.record_tools(ext, tools.iter().map(|t| t.to_string()));
}

fn caps_fm(name: &str, caps: &[&str]) -> SkillFrontmatter {
    SkillFrontmatter {
        name: name.to_string(),
        requires_capabilities: caps.iter().map(|c| c.to_string()).collect(),
        ..Default::default()
    }
}

fn allow_fm(name: &str, allow: &[&str]) -> SkillFrontmatter {
    let mut fm = SkillFrontmatter {
        name: name.to_string(),
        ..Default::default()
    };
    fm.tools.allow = allow.iter().map(|a| a.to_string()).collect();
    fm
}

/// The design's headline case: `requires_capabilities: [withheld, live]`, the
/// first wholly withheld and the second served. **Refused**, not degraded —
/// even though the resolution is non-empty.
#[test]
fn one_wholly_withheld_capability_makes_a_multi_capability_skill_unsatisfiable() {
    let registry = ToolRegistry::default();
    let github = ExtensionId::mcp("github");
    register(&registry, "github__create_issue", &["issues"]);
    register(&registry, "web_search", &["search"]);
    enable(&registry, &github, &["github__create_issue"]);

    let fm = caps_fm("triage", &["issues", "search"]);
    assert!(
        registry.skill_requirements(&fm).is_satisfiable(),
        "both capabilities are served while github is enabled"
    );

    disable(&registry, &github, &["github__create_issue"], &["issues"]);

    let requirements = registry.skill_requirements(&fm);
    assert!(
        !requirements.is_satisfiable(),
        "one wholly withheld capability is enough — the other still resolving does not save it"
    );
    // ...and the resolution really is non-empty, which is the point.
    assert!(
        !registry
            .resolve_capabilities(&fm.requires_capabilities, &[])
            .defs
            .is_empty(),
        "'search' still resolves, so this is not the empty-resolution case"
    );

    let refusal = requirements.refusal("triage");
    assert!(refusal.contains("triage"), "names the skill: {refusal}");
    assert!(refusal.contains("issues"), "names the capability: {refusal}");
    assert!(refusal.contains("github"), "names the extension: {refusal}");
    assert!(
        refusal.contains("Settings → Extensions"),
        "names the remedy: {refusal}"
    );
    assert!(
        !refusal.contains("search"),
        "the served capability is not part of the refusal: {refusal}"
    );
}

/// The same skill with `issues` only **partially** withheld — a second provider
/// still serves it — runs, and carries the prefix.
#[test]
fn a_partially_withheld_capability_runs_with_the_chat_prefix() {
    let registry = ToolRegistry::default();
    let github = ExtensionId::mcp("github");
    register(&registry, "github__create_issue", &["issues"]);
    register(&registry, "local_issues", &["issues"]);
    register(&registry, "web_search", &["search"]);
    enable(&registry, &github, &["github__create_issue"]);
    disable(&registry, &github, &["github__create_issue"], &["issues"]);

    let fm = caps_fm("triage", &["issues", "search"]);
    let requirements = registry.skill_requirements(&fm);
    assert!(
        requirements.is_satisfiable(),
        "a surviving provider keeps every requirement resolvable — partial never gates"
    );
    let prefix = requirements
        .chat_prefix()
        .expect("partial loss carries the chat-visible warning");
    assert!(prefix.contains("issues"), "names the capability: {prefix}");
    assert!(prefix.contains("github"), "names the extension: {prefix}");
}

/// Legacy arm: **every** allowed name withdrawn is total loss.
#[test]
fn the_legacy_arm_gates_only_when_every_allowed_name_is_withdrawn() {
    let registry = ToolRegistry::default();
    let github = ExtensionId::mcp("github");
    register(&registry, "github__create_issue", &["issues"]);
    register(&registry, "shell_execute", &[]);
    enable(&registry, &github, &["github__create_issue"]);
    disable(&registry, &github, &["github__create_issue"], &["issues"]);

    let only_withdrawn = allow_fm("filer", &["github__create_issue"]);
    let requirements = registry.skill_requirements(&only_withdrawn);
    assert!(!requirements.is_satisfiable());
    let refusal = requirements.refusal("filer");
    assert!(refusal.contains("filer"));
    assert!(refusal.contains("github__create_issue"));
    assert!(refusal.contains("github"));

    // One live builtin in the list and the skill runs — with the prefix.
    let mixed = allow_fm("mixed", &["github__create_issue", "shell_execute"]);
    let requirements = registry.skill_requirements(&mixed);
    assert!(
        requirements.is_satisfiable(),
        "a live name keeps a legacy-allow skill runnable"
    );
    let prefix = requirements
        .chat_prefix()
        .expect("the withdrawn half is still announced in chat");
    assert!(prefix.contains("github__create_issue"), "{prefix}");
}

/// Upgrade safety (§7.2, §10 case 3): a capability nothing ever provided is
/// `unknown`, never `withheld`, so no existing install changes behaviour.
#[test]
fn an_unknown_capability_and_an_unowned_name_stay_satisfiable() {
    let registry = ToolRegistry::default();
    register(&registry, "shell_execute", &[]);

    assert!(
        registry
            .skill_requirements(&caps_fm("typo", &["telepathy"]))
            .is_satisfiable()
    );
    assert!(
        registry
            .skill_requirements(&allow_fm("typo", &["nonexistent_tool"]))
            .is_satisfiable()
    );
    assert!(
        registry
            .skill_requirements(&SkillFrontmatter::default())
            .is_satisfiable(),
        "a skill declaring nothing declares nothing to lose"
    );
}

/// One predicate: the capability arm agrees with `resolve_capabilities`'
/// `withheld` classification, which is what the surface sites announce from.
#[test]
fn the_predicate_agrees_with_resolve_capabilities() {
    let registry = ToolRegistry::default();
    let github = ExtensionId::mcp("github");
    register(&registry, "github__create_issue", &["issues"]);
    register(&registry, "local_issues", &["issues"]);
    register(&registry, "web_search", &["search"]);
    enable(&registry, &github, &["github__create_issue"]);

    for caps in [
        vec!["issues".to_string()],
        vec!["issues".to_string(), "search".to_string()],
        vec!["telepathy".to_string()],
    ] {
        let fm = SkillFrontmatter {
            requires_capabilities: caps.clone(),
            ..Default::default()
        };
        let resolution = registry.resolve_capabilities(&caps, &[]);
        assert_eq!(
            registry.skill_requirements(&fm).is_satisfiable(),
            resolution.withheld.is_empty(),
            "predicate disagreed with resolve_capabilities for {caps:?} (enabled)"
        );
    }

    // Withdraw one of two providers, then both.
    disable(&registry, &github, &["github__create_issue"], &["issues"]);
    let both = vec!["issues".to_string(), "search".to_string()];
    let fm = SkillFrontmatter {
        requires_capabilities: both.clone(),
        ..Default::default()
    };
    assert_eq!(
        registry.skill_requirements(&fm).is_satisfiable(),
        registry.resolve_capabilities(&both, &[]).withheld.is_empty()
    );
    registry.remove("local_issues");
    assert_eq!(
        registry.skill_requirements(&fm).is_satisfiable(),
        registry.resolve_capabilities(&both, &[]).withheld.is_empty()
    );
    assert!(!registry.skill_requirements(&fm).is_satisfiable());
}

/// A name the server itself withdrew is attributed to an owner that is **still
/// enabled** — never rendered as the `Disabled` row (§7.2, §3.7).
#[test]
fn a_server_withdrawn_name_is_refused_as_still_enabled() {
    let registry = ToolRegistry::default();
    let github = ExtensionId::mcp("github");
    register(&registry, "github__create_issue", &["issues"]);
    enable(&registry, &github, &["github__create_issue"]);

    // §3.7 step 5: the tool leaves the registry, the name stays attributed and
    // flagged, and the record stays `Enabled`.
    let ledger = registry.extensions();
    ledger.withdraw(&github, ["issues".to_string()]);
    registry.remove("github__create_issue");
    ledger.flag_server_withdrawn(&github, "github__create_issue");

    let refusal = registry
        .skill_requirements(&allow_fm("filer", &["github__create_issue"]))
        .refusal("filer");
    assert!(
        refusal.contains("still enabled"),
        "the owner is named as still enabled: {refusal}"
    );
    assert!(
        !refusal.contains("disabled by the owner"),
        "never the Disabled row: {refusal}"
    );
}

/// The tombstone answer for a withdrawn plugin contribution (§10 case 5(a)).
#[test]
fn a_withdrawn_plugin_contribution_is_attributed_to_its_plugin() {
    let registry = ToolRegistry::default();
    let notion = ExtensionId::plugin("notion");
    registry
        .extensions()
        .upsert(&notion, false, ExtensionState::Disabled);

    let answer = registry.withdrawn_contribution_refusal("Skill", "triage", "notion");
    assert!(answer.contains("triage"), "{answer}");
    assert!(
        answer.contains("provided by plugin 'notion'"),
        "names the plugin: {answer}"
    );
    assert!(answer.contains("disabled"), "names the state: {answer}");

    // No record at all — the plugin was uninstalled, not disabled.
    let answer = registry.withdrawn_contribution_refusal("Agent template", "reader", "gone");
    assert!(answer.contains("no longer loaded"), "{answer}");
}

//! T1 step 3 (§7.3) and the §7.2 classification, against a real `ToolRegistry`,
//! a real `AgentRegistry` and a real `SkillCatalog`.

use std::sync::Arc;

use openalpaca_llm::ToolDefinition;
use tempfile::TempDir;

use super::*;
use crate::agent::AgentRegistry;
use crate::agent::template::{AgentSource, AgentTemplate, AgentTemplateFrontmatter};
use crate::bus::EventBus;
use crate::events::SystemEvent;
use crate::middleware::skill::SkillScope;
use crate::orchestrator::skill::catalog::SkillCatalog;
use crate::tools::ToolRegistry;
use crate::tools::registry::{BuiltInTool, RegisteredTool, ToolBackend};

// ── Fixtures ─────────────────────────────────────────────────────────────

struct Ok200;

#[async_trait::async_trait]
impl BuiltInTool for Ok200 {
    async fn execute(&self, _arguments: &serde_json::Value) -> Result<String, String> {
        Ok("ok".into())
    }
}

fn tool(name: &str, caps: &[&str], author: &str) -> RegisteredTool {
    RegisteredTool {
        definition: ToolDefinition {
            name: name.to_string(),
            description: format!("{name} tool"),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
            strict: None,
            input_examples: None,
        },
        backend: ToolBackend::BuiltIn(Arc::new(Ok200)),
        provides_capabilities: caps.iter().map(|c| c.to_string()).collect(),
        exempt_from_timeout: false,
        annotations: None,
        version: "test-0.0.0".into(),
        author: author.into(),
        created_at: chrono::Utc::now(),
    }
}

fn mcp_tool(name: &str, caps: &[&str], server: &str) -> RegisteredTool {
    let mut t = tool(name, caps, &format!("mcp:{server}"));
    t.backend = ToolBackend::Mcp {
        client: Arc::new(openalpaca_mcp::McpClient::disconnected_for_tests(server)),
        remote_name: "echo".into(),
        server_name: server.into(),
        generation: 1,
    };
    t
}

fn template(id: &str, caps: &[&str]) -> AgentTemplate {
    AgentTemplate {
        frontmatter: AgentTemplateFrontmatter {
            id: id.into(),
            name: id.into(),
            description: String::new(),
            icon: None,
            singleton: false,
            capabilities: caps.iter().map(|c| c.to_string()).collect(),
            denied_capabilities: vec![],
            temperature: 0.5,
            verbosity: "normal".into(),
            model: None,
            fallback_models: vec![],
            max_tool_calls: None,
            timeout_seconds: None,
            max_cost_per_task: None,
            max_rounds: None,
            require_confirmation_for: vec![],
        },
        body: String::new(),
        sections: std::collections::HashMap::new(),
        source: AgentSource::default(),
    }
}

/// A skill catalog built from real `SKILL.md` files, so the scan reads the same
/// frontmatter the router and the invocation path do.
fn catalog(dir: &TempDir, skills: &[(&str, &str)]) -> Arc<SkillCatalog> {
    for (name, body) in skills {
        let d = dir.path().join(name);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("SKILL.md"), body).unwrap();
    }
    let catalog = SkillCatalog::new();
    catalog.scan_directory(dir.path(), SkillScope::Project);
    Arc::new(catalog)
}

const CAP_SKILL: &str = "---
id: issue-triage
name: Issue Triage
description: Triage inbound issues
invoke:
  mode: auto
requires_capabilities:
  - issues
---
Body.
";

const CRON_SKILL: &str = "---
id: nightly-digest
name: Nightly Digest
description: Post a nightly digest
invoke:
  mode: scheduled
  cron: \"0 3 * * *\"
requires_capabilities:
  - issues
---
Body.
";

const LEGACY_SKILL: &str = "---
id: legacy-filer
name: Legacy Filer
description: Files issues by tool name
invoke:
  mode: manual
tools:
  allow:
    - github__create_issue
---
Body.
";

const LEGACY_MIXED_SKILL: &str = "---
id: legacy-mixed
name: Legacy Mixed
description: One extension tool and one builtin
invoke:
  mode: manual
tools:
  allow:
    - github__create_issue
    - file_read
---
Body.
";

/// Registry + ledger with `github`'s two tools registered and recorded, plus a
/// second, never-disabled provider of `search`.
fn registry_with_github(bus: EventBus) -> (Arc<ToolRegistry>, ExtensionId) {
    let registry = Arc::new(ToolRegistry::with_event_bus(bus).unwrap());
    let ext = ExtensionId::mcp("github");

    registry
        .register(mcp_tool("github__create_issue", &["issues"], "github"))
        .unwrap();
    registry
        .register(mcp_tool("github__search", &["search"], "github"))
        .unwrap();
    // A surviving provider of `search` — this is what makes it *partial*.
    registry
        .register(tool("local_search", &["search"], "builtin"))
        .unwrap();
    registry.register(tool("file_read", &[], "builtin")).unwrap();

    let ledger = registry.extensions();
    ledger.upsert(&ext, true, ExtensionState::Enabled);
    ledger.record_tools(&ext, ["github__create_issue", "github__search"]);
    (registry, ext)
}

/// T1 steps 1–2 as a supervisor runs them, returning the withdrawn set.
fn t1(registry: &ToolRegistry, ext: &ExtensionId) -> WithdrawnSet {
    let ledger = registry.extensions();
    let mut withdrawn = WithdrawnSet::default();
    for name in ledger.tool_names(ext) {
        if let Some(t) = registry.get(&name) {
            ledger.withdraw(ext, t.provides_capabilities.clone());
            withdrawn.add_capabilities(t.provides_capabilities.clone());
        }
        if registry.remove(&name) {
            withdrawn.add_tool(name);
        }
    }
    withdrawn
}

fn withdrawn_events(
    rx: &mut tokio::sync::broadcast::Receiver<SystemEvent>,
) -> Vec<(ExtensionId, ExtensionState, WithdrawalCause, ScanOutcome, String)> {
    let mut out = Vec::new();
    while let Ok(event) = rx.try_recv() {
        if let SystemEvent::ExtensionCapabilityWithdrawn {
            extension,
            state,
            cause,
            affected_templates,
            affected_skills,
            affected_cron_skills,
            notice_lane,
            ..
        } = event
        {
            out.push((
                extension,
                state,
                cause,
                ScanOutcome {
                    affected_templates,
                    affected_skills,
                    affected_cron_skills,
                },
                notice_lane,
            ));
        }
    }
    out
}

// ── §7.2 classification ──────────────────────────────────────────────────

#[test]
fn withheld_partial_and_unattributed_are_three_different_answers() {
    let bus = EventBus::new(256);
    let mut rx = bus.subscribe();
    let (registry, ext) = registry_with_github(bus);

    registry.extensions().begin(
        &ext,
        ExtensionState::Disabling,
        Some(WithdrawalCause::Disable),
    );
    t1(&registry, &ext);
    registry.extensions().commit(&ext, ExtensionState::Disabled);

    let resolution = registry.resolve_capabilities(
        &[
            "issues".to_string(),
            "search".to_string(),
            "telepathy".to_string(),
        ],
        &[],
    );

    // Attributed, total: `issues` had exactly one provider and it is blocked.
    assert_eq!(resolution.withheld.len(), 1);
    assert_eq!(resolution.withheld[0].capability, "issues");
    assert_eq!(resolution.withheld[0].providers.len(), 1);
    assert_eq!(resolution.withheld[0].providers[0].extension, ext);
    assert!(!resolution.withheld[0].providers[0].server_withdrawn);

    // Attributed, partial: `search` still resolves through `local_search`.
    assert_eq!(resolution.partially_withheld.len(), 1);
    assert_eq!(resolution.partially_withheld[0].capability, "search");
    assert_eq!(resolution.partially_withheld[0].providers[0].extension, ext);
    assert!(
        resolution.defs.iter().any(|d| d.name == "local_search"),
        "partial withdrawal never gates — the resolution proceeds with B's tools"
    );

    // Unattributed: nothing ever provided it, so it stays a `debug!`.
    assert_eq!(resolution.unknown, vec!["telepathy".to_string()]);

    // Both attributed classes announce; the unknown one does not.
    let before = registry.extensions().warned_count();
    assert_eq!(before, 0);
    registry.announce_withheld(&resolution, None, Some("scope-1"));
    assert_eq!(
        registry.extensions().warned_count(),
        1,
        "one extension, one moment, one scope — both capabilities collapse into it"
    );

    let events: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok())
        .filter(|e| matches!(e, SystemEvent::ExtensionCapabilityWithheld { .. }))
        .collect();
    assert_eq!(events.len(), 1);
    let SystemEvent::ExtensionCapabilityWithheld {
        subject, moment, ..
    } = &events[0]
    else {
        unreachable!()
    };
    assert_eq!(*moment, Moment::SurfaceAssembly);
    assert_eq!(subject, "issues");
}

#[test]
fn a_server_withdrawn_name_is_attributed_to_a_still_enabled_owner() {
    // For MCP the capability **is** the namespaced tool name (§7.1's
    // *"Enabled, server-withdrawn"* row, §7.2), which is what makes the
    // `server_withdrawn` lookup in `blocked_providers` land.
    let bus = EventBus::new(64);
    let registry = Arc::new(ToolRegistry::with_event_bus(bus).unwrap());
    let ext = ExtensionId::mcp("github");
    registry
        .register(mcp_tool(
            "github__create_issue",
            &["github__create_issue"],
            "github",
        ))
        .unwrap();
    let ledger = registry.extensions();
    ledger.upsert(&ext, true, ExtensionState::Enabled);
    ledger.record_tools(&ext, ["github__create_issue"]);

    // §3.7 step 5: the server dropped the tool; the owner stays `Enabled`.
    ledger.withdraw(&ext, ["github__create_issue".to_string()]);
    registry.remove("github__create_issue");
    ledger.flag_server_withdrawn(&ext, "github__create_issue");

    let resolution = registry.resolve_capabilities(&["github__create_issue".to_string()], &[]);
    assert_eq!(resolution.withheld.len(), 1);
    let provider = &resolution.withheld[0].providers[0];
    assert_eq!(provider.extension, ext);
    assert!(
        provider.server_withdrawn,
        "the owner is still enabled — the attribution must say so, not 'disabled'"
    );
}

// ── §7.3 the dependent scan ──────────────────────────────────────────────

#[tracing_test::traced_test]
#[test]
fn a_disable_announces_once_and_names_every_dependent() {
    let bus = EventBus::new(256);
    let mut rx = bus.subscribe();
    let (registry, ext) = registry_with_github(bus);

    let agents = AgentRegistry::new();
    agents.register_template(template("issue_agent", &["issues"]));
    agents.register_template(template("search_agent", &["search"]));
    agents.register_template(template("unrelated_agent", &["file_write"]));

    let dir = tempfile::tempdir().unwrap();
    let skills = catalog(
        &dir,
        &[
            ("issue-triage", CAP_SKILL),
            ("nightly-digest", CRON_SKILL),
            ("legacy-filer", LEGACY_SKILL),
            ("legacy-mixed", LEGACY_MIXED_SKILL),
        ],
    );

    registry.extensions().begin(
        &ext,
        ExtensionState::Disabling,
        Some(WithdrawalCause::Disable),
    );
    let withdrawn = t1(&registry, &ext);

    let outcome = DependentScan {
        registry: &registry,
        agents: Some(&agents),
        skills: Some(&skills),
        notice_lane: "owner:gui",
    }
    .run(
        &ext,
        &ExtensionState::Disabling,
        WithdrawalCause::Disable,
        &withdrawn,
        false,
    );

    assert_eq!(
        outcome.affected_templates,
        vec!["issue_agent".to_string()],
        "`search` still has a provider, so `search_agent` did not stop resolving"
    );
    assert_eq!(
        outcome.affected_skills,
        vec![
            "issue-triage".to_string(),
            "legacy-filer".to_string(),
            "nightly-digest".to_string(),
        ],
        "the legacy `tools.allow` skill is named too; `legacy-mixed` keeps a builtin"
    );
    assert_eq!(outcome.affected_cron_skills, vec!["nightly-digest"]);

    let events = withdrawn_events(&mut rx);
    assert_eq!(events.len(), 1, "one transition, one announcement");
    assert_eq!(events[0].0, ext);
    assert_eq!(events[0].2, WithdrawalCause::Disable);
    assert_eq!(events[0].3, outcome);
    assert_eq!(events[0].4, "owner:gui");

    logs_assert(|lines: &[&str]| {
        let warns = lines
            .iter()
            .filter(|l| l.contains("WARN") && l.contains("no longer resolve"))
            .count();
        match warns {
            1 => Ok(()),
            n => Err(format!("expected exactly one scan WARN, saw {n}")),
        }
    });
    assert!(
        logs_contain("mcp:github: disabled —"),
        "the wording is keyed on the cause, not on the transient state"
    );
}

#[tracing_test::traced_test]
#[test]
fn a_deny_scan_is_worded_denied_not_disabled() {
    let bus = EventBus::new(256);
    let mut rx = bus.subscribe();
    let (registry, ext) = registry_with_github(bus);

    let agents = AgentRegistry::new();
    agents.register_template(template("issue_agent", &["issues"]));
    let dir = tempfile::tempdir().unwrap();
    let skills = catalog(&dir, &[("issue-triage", CAP_SKILL)]);

    let withdrawn = t1(&registry, &ext);
    DependentScan {
        registry: &registry,
        agents: Some(&agents),
        skills: Some(&skills),
        notice_lane: "owner:gui",
    }
    .run(
        &ext,
        &ExtensionState::Disabling,
        WithdrawalCause::Deny,
        &withdrawn,
        false,
    );

    let events = withdrawn_events(&mut rx);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].2, WithdrawalCause::Deny);

    logs_assert(|lines: &[&str]| {
        let scan: Vec<&&str> = lines
            .iter()
            .filter(|l| l.contains("no longer resolve"))
            .collect();
        if scan.len() != 1 {
            return Err(format!("expected one scan line, saw {}", scan.len()));
        }
        // The wording sits between the extension id and the em dash; the
        // enclosing span carries the test's own name, so match the phrase.
        if !scan[0].contains("mcp:github: denied —") {
            return Err(format!("scan line is not worded 'denied': {}", scan[0]));
        }
        if scan[0].contains("mcp:github: disabled —") {
            return Err(format!("scan line says 'disabled': {}", scan[0]));
        }
        Ok(())
    });
}

#[test]
fn a_crash_scan_carries_the_crash_detail_not_the_word_disabled() {
    let bus = EventBus::new(64);
    let (registry, ext) = registry_with_github(bus);
    let agents = AgentRegistry::new();
    agents.register_template(template("issue_agent", &["issues"]));
    let dir = tempfile::tempdir().unwrap();
    let skills = catalog(&dir, &[("issue-triage", CAP_SKILL)]);

    let state = ExtensionState::Failed {
        reason: FailureReason::Crashed,
        detail: "reconnect exhausted after 3 attempts".into(),
        since: chrono::Utc::now(),
    };
    let withdrawn = t1(&registry, &ext);
    let outcome = DependentScan {
        registry: &registry,
        agents: Some(&agents),
        skills: Some(&skills),
        notice_lane: "owner:gui",
    }
    .run(&ext, &state, WithdrawalCause::Crash, &withdrawn, false);

    assert_eq!(outcome.affected_templates, vec!["issue_agent"]);
    assert_eq!(
        WithdrawalCause::Crash.wording(&ext, "reconnect exhausted after 3 attempts"),
        "stopped running (crashed: reconnect exhausted after 3 attempts)"
    );
}

#[test]
fn an_empty_withdrawn_set_announces_nothing() {
    let bus = EventBus::new(64);
    let mut rx = bus.subscribe();
    let (registry, ext) = registry_with_github(bus);
    let agents = AgentRegistry::new();
    agents.register_template(template("issue_agent", &["issues"]));

    // First pass withdraws; a second, idempotent pass finds nothing left.
    let first = t1(&registry, &ext);
    let second = t1(&registry, &ext);
    assert!(!first.is_empty());
    assert!(second.is_empty());

    let scan = DependentScan {
        registry: &registry,
        agents: Some(&agents),
        skills: None,
        notice_lane: "owner:gui",
    };
    scan.run(
        &ext,
        &ExtensionState::Disabling,
        WithdrawalCause::Disable,
        &second,
        false,
    );
    assert!(
        withdrawn_events(&mut rx).is_empty(),
        "one transition, one announcement — a second pass says nothing"
    );
}

#[test]
fn a_reload_that_ends_enabled_carries_no_cron_skills() {
    let bus = EventBus::new(64);
    let mut rx = bus.subscribe();
    let (registry, ext) = registry_with_github(bus);
    let dir = tempfile::tempdir().unwrap();
    let skills = catalog(&dir, &[("nightly-digest", CRON_SKILL)]);

    let withdrawn = t1(&registry, &ext);
    let outcome = DependentScan {
        registry: &registry,
        agents: None,
        skills: Some(&skills),
        notice_lane: "owner:gui",
    }
    .run(
        &ext,
        &ExtensionState::Disabling,
        WithdrawalCause::Reload,
        &withdrawn,
        true,
    );

    assert_eq!(
        outcome.affected_cron_skills,
        vec!["nightly-digest"],
        "the scan still found it"
    );
    let events = withdrawn_events(&mut rx);
    assert_eq!(events.len(), 1);
    assert!(
        events[0].3.affected_cron_skills.is_empty(),
        "a reload that ended `Enabled` fires no cron notice, and the dispatcher's \
         rule is only 'post when affected_cron_skills is non-empty'"
    );
    assert_eq!(events[0].3.affected_skills, vec!["nightly-digest"]);
}

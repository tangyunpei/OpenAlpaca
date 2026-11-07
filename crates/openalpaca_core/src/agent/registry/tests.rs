use super::*;
use crate::agent::subagent::{AgentConstraints, AgentLlmConfig, AgentPreset, Capability};
use crate::agent::template::{AgentSource, AgentTemplate, AgentTemplateFrontmatter, parse_agent_markdown};

fn make_agent(id: &str, skills: Vec<&str>) -> SubAgent {
    SubAgent {
        id: id.to_string(),
        template_id: id.to_string(),
        name: format!("Agent {}", id),
        description: None,
        icon: None,
        status: AgentStatus::Idle,
        current_task: None,
        capabilities: skills
            .into_iter()
            .map(|s| Capability {
                name: s.to_string(),
                category: "test".to_string(),
                proficiency: 1.0,
            })
            .collect(),
        preset: AgentPreset::default(),
        constraints: AgentConstraints::default(),
        llm_config: AgentLlmConfig::default(),
    }
}

fn make_template(id: &str, skills: Vec<&str>, singleton: bool) -> AgentTemplate {
    AgentTemplate {
        frontmatter: AgentTemplateFrontmatter {
            id: id.to_string(),
            name: format!("Template {}", id),
            description: format!("{} template", id),
            icon: None,
            singleton,
            capabilities: skills.into_iter().map(|s| s.to_string()).collect(),
            denied_capabilities: vec![],
            temperature: 0.5,
            verbosity: "normal".to_string(),
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

// ── Legacy (backward compat) tests ─────────────────────────────

#[test]
fn test_register_and_get() {
    let reg = AgentRegistry::new();
    assert!(reg.register(make_agent("a1", vec!["search"])));
    assert!(!reg.register(make_agent("a1", vec!["search"]))); // duplicate
    assert_eq!(reg.count(), 1);

    let agent = reg.get("a1").unwrap();
    assert_eq!(agent.name, "Agent a1");
}

#[test]
fn test_get_nonexistent() {
    let reg = AgentRegistry::new();
    assert!(reg.get("nope").is_none());
}

#[test]
fn test_remove() {
    let reg = AgentRegistry::new();
    reg.register(make_agent("a1", vec![]));
    assert!(reg.remove("a1"));
    assert!(!reg.remove("a1"));
    assert_eq!(reg.count(), 0);
}

#[test]
fn test_update_status() {
    let reg = AgentRegistry::new();
    reg.register(make_agent("a1", vec![]));

    assert!(reg.update_status(
        "a1",
        AgentStatus::Busy {
            task_id: "t1".into()
        }
    ));
    let agent = reg.get("a1").unwrap();
    assert_eq!(agent.status.as_str(), "busy");
    assert_eq!(agent.current_task.as_deref(), Some("t1"));

    assert!(reg.update_status("a1", AgentStatus::Idle));
    let agent = reg.get("a1").unwrap();
    assert!(agent.status.is_available());
    assert!(agent.current_task.is_none());

    assert!(!reg.update_status("nope", AgentStatus::Idle));
}

#[test]
fn test_list_instances() {
    let reg = AgentRegistry::new();
    reg.register(make_agent("a1", vec![]));
    reg.register(make_agent("a2", vec![]));

    let all = reg.list_instances();
    assert_eq!(all.len(), 2);
}

#[test]
fn test_list_idle() {
    let reg = AgentRegistry::new();
    reg.register(make_agent("a1", vec![]));
    reg.register(make_agent("a2", vec![]));

    reg.update_status(
        "a1",
        AgentStatus::Busy {
            task_id: "t1".into(),
        },
    );

    let idle = reg.list_idle();
    assert_eq!(idle.len(), 1);
    assert_eq!(idle[0].id, "a2");
}

#[test]
fn test_find_by_capability() {
    let reg = AgentRegistry::new();
    reg.register(make_agent("a1", vec!["search", "summarize"]));
    reg.register(make_agent("a2", vec!["write"]));
    reg.register(make_agent("a3", vec!["search"]));

    let searchers = reg.find_by_capability("search");
    assert_eq!(searchers.len(), 2);

    let writers = reg.find_by_capability("write");
    assert_eq!(writers.len(), 1);

    let none = reg.find_by_capability("nonexistent");
    assert!(none.is_empty());
}

#[test]
fn test_get_with_version() {
    let reg = AgentRegistry::new();
    reg.register(make_agent("a1", vec!["search"]));

    let (agent, version) = reg.get_with_version("a1").unwrap();
    assert_eq!(agent.id, "a1");
    assert_eq!(version, 0);

    assert!(reg.get_with_version("nope").is_none());
}

#[test]
fn test_update_config_success() {
    let reg = AgentRegistry::new();
    reg.register(make_agent("a1", vec!["search"]));

    let mut updated = make_agent("a1", vec!["search", "summarize"]);
    updated.name = "Updated Agent".to_string();

    let new_version = reg.update_config("a1", updated, 0).unwrap();
    assert_eq!(new_version, 1);

    let (agent, version) = reg.get_with_version("a1").unwrap();
    assert_eq!(agent.name, "Updated Agent");
    assert_eq!(agent.capabilities.len(), 2);
    assert_eq!(version, 1);
}

#[test]
fn test_update_config_conflict() {
    let reg = AgentRegistry::new();
    reg.register(make_agent("a1", vec!["search"]));

    // First update succeeds
    let updated = make_agent("a1", vec!["search", "summarize"]);
    reg.update_config("a1", updated, 0).unwrap();

    // Second update with stale version fails
    let updated2 = make_agent("a1", vec!["write"]);
    let err = reg.update_config("a1", updated2, 0).unwrap_err();
    assert_eq!(err, "CONFIG_CONFLICT");
}

#[test]
fn test_update_config_not_found() {
    let reg = AgentRegistry::new();
    let agent = make_agent("a1", vec![]);
    let err = reg.update_config("a1", agent, 0).unwrap_err();
    assert_eq!(err, "AGENT_NOT_FOUND");
}

#[test]
fn test_update_config_sequential_versions() {
    let reg = AgentRegistry::new();
    reg.register(make_agent("a1", vec![]));

    let v1 = reg
        .update_config("a1", make_agent("a1", vec!["s1"]), 0)
        .unwrap();
    assert_eq!(v1, 1);

    let v2 = reg
        .update_config("a1", make_agent("a1", vec!["s2"]), 1)
        .unwrap();
    assert_eq!(v2, 2);

    let v3 = reg
        .update_config("a1", make_agent("a1", vec!["s3"]), 2)
        .unwrap();
    assert_eq!(v3, 3);
}

#[test]
fn test_try_claim_success() {
    let reg = AgentRegistry::new();
    reg.register(make_agent("a1", vec!["search"]));

    let agent = reg.try_claim("a1", "task-1".to_string()).unwrap();
    assert_eq!(agent.id, "a1");
    assert_eq!(agent.status.as_str(), "busy");
    assert_eq!(agent.current_task.as_deref(), Some("task-1"));

    // Registry state is also Busy
    let fetched = reg.get("a1").unwrap();
    assert_eq!(fetched.status.as_str(), "busy");
    assert_eq!(fetched.current_task.as_deref(), Some("task-1"));
}

#[test]
fn test_try_claim_already_busy() {
    let reg = AgentRegistry::new();
    reg.register(make_agent("a1", vec!["search"]));

    // First claim succeeds
    reg.try_claim("a1", "task-1".to_string()).unwrap();

    // Second claim fails
    let err = reg.try_claim("a1", "task-2".to_string()).unwrap_err();
    assert!(err.contains("not available"));

    // Original task_id preserved
    let fetched = reg.get("a1").unwrap();
    assert_eq!(fetched.current_task.as_deref(), Some("task-1"));
}

#[test]
fn test_try_claim_not_found() {
    let reg = AgentRegistry::new();
    let err = reg
        .try_claim("nonexistent", "task-1".to_string())
        .unwrap_err();
    assert!(err.contains("not found"));
}

#[test]
fn test_try_claim_concurrent() {
    use std::sync::Arc;
    use std::thread;

    let reg = Arc::new(AgentRegistry::new());
    reg.register(make_agent("a1", vec!["search"]));

    let handles: Vec<_> = (0..10)
        .map(|i| {
            let reg = reg.clone();
            thread::spawn(move || reg.try_claim("a1", format!("task-{}", i)))
        })
        .collect();

    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    let wins = results.iter().filter(|r| r.is_ok()).count();
    let losses = results.iter().filter(|r| r.is_err()).count();

    assert_eq!(wins, 1, "Exactly one thread should win the claim");
    assert_eq!(losses, 9);
}

// ── Template + Instance tests ──────────────────────────────────

#[test]
fn test_register_template() {
    let reg = AgentRegistry::new();
    let t = make_template("code_agent", vec!["file_read", "file_write"], false);
    assert!(reg.register_template(t.clone()));
    assert!(!reg.register_template(t)); // duplicate
    assert_eq!(reg.template_count(), 1);

    let fetched = reg.get_template("code_agent").unwrap();
    assert_eq!(fetched.frontmatter.id, "code_agent");
}

#[test]
fn test_list_templates() {
    let reg = AgentRegistry::new();
    reg.register_template(make_template("a", vec![], false));
    reg.register_template(make_template("b", vec![], false));
    assert_eq!(reg.list_templates().len(), 2);
}

#[test]
fn test_find_templates_by_capability() {
    let reg = AgentRegistry::new();
    reg.register_template(make_template("a", vec!["search"], false));
    reg.register_template(make_template("b", vec!["write"], false));
    reg.register_template(make_template("c", vec!["search", "write"], false));

    let searchers = reg.find_templates_by_capability("search");
    assert_eq!(searchers.len(), 2);

    let writers = reg.find_templates_by_capability("write");
    assert_eq!(writers.len(), 2);

    let none = reg.find_templates_by_capability("nonexistent");
    assert!(none.is_empty());
}

#[test]
fn test_spawn_instance_non_singleton() {
    let reg = AgentRegistry::new();
    reg.register_template(make_template("code_agent", vec!["file_read"], false));

    let inst1 = reg.spawn_instance("code_agent", "task-1".into()).unwrap();
    assert!(inst1.id.starts_with("code_agent::"));
    assert_eq!(inst1.template_id, "code_agent");
    assert_eq!(inst1.status.as_str(), "busy");
    assert_eq!(inst1.current_task.as_deref(), Some("task-1"));

    // Can spawn multiple instances from the same template
    let inst2 = reg.spawn_instance("code_agent", "task-2".into()).unwrap();
    assert!(inst2.id.starts_with("code_agent::"));
    assert_ne!(inst1.id, inst2.id);
    assert_eq!(reg.count(), 2);
}

#[test]
fn test_spawn_instance_singleton() {
    let reg = AgentRegistry::new();
    reg.register_template(make_template("lead_agent", vec!["orchestrate"], true));

    // First spawn creates the singleton instance
    let inst1 = reg.spawn_instance("lead_agent", "task-1".into()).unwrap();
    assert_eq!(inst1.id, "lead_agent"); // stable ID
    assert_eq!(inst1.status.as_str(), "busy");

    // Second spawn fails (singleton is busy)
    let err = reg
        .spawn_instance("lead_agent", "task-2".into())
        .unwrap_err();
    assert!(err.contains("busy"));

    // Release singleton
    reg.destroy_instance("lead_agent");
    let fetched = reg.get_instance("lead_agent").unwrap();
    assert!(fetched.status.is_available());

    // Re-claim succeeds
    let inst2 = reg.spawn_instance("lead_agent", "task-3".into()).unwrap();
    assert_eq!(inst2.id, "lead_agent");
    assert_eq!(inst2.current_task.as_deref(), Some("task-3"));
}

#[test]
fn test_spawn_instance_template_not_found() {
    let reg = AgentRegistry::new();
    let err = reg.spawn_instance("nope", "task-1".into()).unwrap_err();
    assert!(err.contains("not found"));
}

#[test]
fn test_destroy_instance_non_singleton() {
    let reg = AgentRegistry::new();
    reg.register_template(make_template("code_agent", vec![], false));

    let inst = reg.spawn_instance("code_agent", "task-1".into()).unwrap();
    assert_eq!(reg.count(), 1);

    // Destroying non-singleton removes it entirely
    assert_eq!(reg.destroy_instance(&inst.id), DestroyOutcome::Removed);
    assert_eq!(reg.count(), 0);
    assert!(reg.get_instance(&inst.id).is_none());
}

#[test]
fn test_destroy_instance_singleton_resets_to_idle() {
    let reg = AgentRegistry::new();
    reg.register_template(make_template("lead", vec![], true));

    let inst = reg.spawn_instance("lead", "task-1".into()).unwrap();
    assert_eq!(inst.id, "lead");

    // Destroying singleton resets to Idle instead of removing
    reg.destroy_instance("lead");
    let fetched = reg.get_instance("lead").unwrap();
    assert!(fetched.status.is_available());
    assert!(fetched.current_task.is_none());
    assert_eq!(reg.count(), 1); // still in registry
}

#[test]
fn test_count_instances_of() {
    let reg = AgentRegistry::new();
    reg.register_template(make_template("code_agent", vec![], false));
    reg.register_template(make_template("research_agent", vec![], false));

    reg.spawn_instance("code_agent", "t1".into()).unwrap();
    reg.spawn_instance("code_agent", "t2".into()).unwrap();
    reg.spawn_instance("research_agent", "t3".into()).unwrap();

    assert_eq!(reg.count_instances_of("code_agent"), 2);
    assert_eq!(reg.count_instances_of("research_agent"), 1);
    assert_eq!(reg.count_instances_of("nonexistent"), 0);
}

#[test]
fn test_spawn_instance_concurrent() {
    use std::sync::Arc;
    use std::thread;

    let reg = Arc::new(AgentRegistry::new());
    reg.register_template(make_template("lead", vec![], true));

    let handles: Vec<_> = (0..10)
        .map(|i| {
            let reg = reg.clone();
            thread::spawn(move || reg.spawn_instance("lead", format!("task-{}", i)))
        })
        .collect();

    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    let wins = results.iter().filter(|r| r.is_ok()).count();
    let losses = results.iter().filter(|r| r.is_err()).count();

    assert_eq!(wins, 1, "Exactly one thread should win the singleton claim");
    assert_eq!(losses, 9);
}

#[test]
fn test_spawn_non_singleton_concurrent() {
    use std::sync::Arc;
    use std::thread;

    let reg = Arc::new(AgentRegistry::new());
    reg.register_template(make_template("worker", vec![], false));

    let handles: Vec<_> = (0..10)
        .map(|i| {
            let reg = reg.clone();
            thread::spawn(move || reg.spawn_instance("worker", format!("task-{}", i)))
        })
        .collect();

    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    let wins = results.iter().filter(|r| r.is_ok()).count();

    // All 10 should succeed for non-singleton
    assert_eq!(wins, 10, "All threads should spawn non-singleton instances");
    assert_eq!(reg.count(), 10);
}

#[test]
fn test_template_with_markdown() {
    let md = r#"---
id: "test_agent"
name: "Test Agent"
description: "A test agent"
singleton: false
capabilities:
  - "file_read"
temperature: 0.3
---

## Persona

You are a test agent.
"#;
    let template = parse_agent_markdown(md).unwrap();
    let reg = AgentRegistry::new();
    reg.register_template(template);

    let inst = reg.spawn_instance("test_agent", "task-1".into()).unwrap();
    assert!(inst.id.starts_with("test_agent::"));
    assert_eq!(inst.preset.persona, "You are a test agent.");
    assert_eq!(inst.preset.temperature, 0.3);
    // 1 explicit skill + 2 injected workspace tools + the ambient memory
    // search every internal template is granted (design 09 §5)
    assert_eq!(inst.capabilities.len(), 4);
    assert_eq!(inst.capabilities[0].name, "file_read");
    assert_eq!(inst.capabilities[1].name, "workspace_read");
    assert_eq!(inst.capabilities[2].name, "workspace_write");
    assert_eq!(inst.capabilities[3].name, "memory_read");
}

// ── GAP-24: an uninstall expires the withdrawal tombstones ───────────────

/// C5 leaves a tombstone behind every withdrawn plugin template so
/// `spawn_subagent` can say *who* took it away. Nothing expired those, because
/// no uninstall path existed: after the plugin's directory is gone the
/// tombstone would name a plugin that is not there any more, for good.
///
/// GAP-24's uninstall clears them by plugin, which is what lets a re-install
/// under the same directory name start clean.
#[test]
fn clearing_a_plugins_tombstones_leaves_every_other_plugins_alone() {
    let reg = AgentRegistry::new();
    // A tombstone records a withdrawal that happened (N-PLUG-6), so each
    // template is the plugin's own before it is withdrawn.
    for (id, plugin) in [
        ("reader", "notion"),
        ("writer", "notion"),
        ("summarizer", "linear"),
    ] {
        assert!(reg.register_template(plugin_template(id, plugin)));
    }
    reg.remove_plugin_template("reader", "notion");
    reg.remove_plugin_template("writer", "notion");
    reg.remove_plugin_template("summarizer", "linear");

    assert_eq!(reg.template_tombstone("reader").as_deref(), Some("notion"));

    let cleared = reg.clear_plugin_tombstones("notion");

    assert_eq!(cleared, 2, "both of notion's tombstones are expired");
    assert!(reg.template_tombstone("reader").is_none());
    assert!(reg.template_tombstone("writer").is_none());
    assert_eq!(
        reg.template_tombstone("summarizer").as_deref(),
        Some("linear"),
        "another plugin's tombstone is untouched"
    );
    assert_eq!(reg.clear_plugin_tombstones("notion"), 0, "idempotent");
}

// ── N-PLUG-6: a plugin's unload removes only what it owns ────────────────

/// A plugin agent's executor that is never driven: these tests only register
/// and remove templates.
struct IdleExecutor;

#[async_trait::async_trait]
impl openalpaca_api::plugin_traits::PluginAgentExecutor for IdleExecutor {
    async fn spawn(
        &self,
        _instance_id: &str,
        _task_id: &str,
        _instructions: &str,
        _context: &serde_json::Value,
    ) -> Result<bool, String> {
        Ok(true)
    }

    async fn step(
        &self,
        _instance_id: &str,
        _tool_results: Option<&serde_json::Value>,
    ) -> Result<(String, String, Vec<serde_json::Value>), String> {
        Ok(("complete".to_string(), String::new(), vec![]))
    }

    async fn stop(&self, _instance_id: &str) -> Result<(), String> {
        Ok(())
    }

    fn plugin_id(&self) -> &str {
        "plugin"
    }

    fn agent_id(&self) -> &str {
        "agent"
    }
}

/// A template a plugin's load registered.
fn plugin_template(id: &str, plugin: &str) -> AgentTemplate {
    AgentTemplate {
        source: AgentSource::Plugin {
            plugin_id: plugin.to_string(),
            executor: std::sync::Arc::new(IdleExecutor),
        },
        ..make_template(id, vec![], false)
    }
}

/// A shipped template on the id a plugin names is not the plugin's to
/// remove: it stays, and nothing is attributed to the plugin.
#[test]
fn a_plugin_removal_leaves_a_shipped_template() {
    let reg = AgentRegistry::new();
    assert!(reg.register_template(make_template("writing_agent", vec!["file_read"], false)));
    assert!(
        !reg.register_template(plugin_template("writing_agent", "notion")),
        "a duplicate id is refused"
    );

    assert!(!reg.remove_plugin_template("writing_agent", "notion"));

    let shipped = reg
        .get_template("writing_agent")
        .expect("the shipped template stays");
    assert!(matches!(shipped.source, AgentSource::Internal));
    assert!(reg.template_tombstone("writing_agent").is_none());
}

/// Another plugin's template is not this plugin's to remove either.
#[test]
fn a_plugin_removal_leaves_another_plugins_template() {
    let reg = AgentRegistry::new();
    assert!(reg.register_template(plugin_template("reader", "linear")));

    assert!(!reg.remove_plugin_template("reader", "notion"));

    assert!(reg.get_template("reader").is_some());
    assert!(reg.template_tombstone("reader").is_none());
}

/// The plugin's own template goes, and its tombstone names the plugin.
#[test]
fn a_plugin_removal_takes_its_own_template_and_tombstones_it() {
    let reg = AgentRegistry::new();
    assert!(reg.register_template(plugin_template("reader", "notion")));

    assert!(reg.remove_plugin_template("reader", "notion"));

    assert!(reg.get_template("reader").is_none());
    assert_eq!(reg.template_tombstone("reader").as_deref(), Some("notion"));
}

// ── Subagent design C1: the layers, resolved on read ──────────────────────

/// A file path as the file layer records it.
fn config_path(name: &str) -> std::path::PathBuf {
    std::path::PathBuf::from("/config/agents").join(name)
}

/// The file layer is replaced whole (subagent design §2.4 L-1), so an id no
/// file holds any more leaves the catalog: what lets a rescan see a file
/// deleted, renamed or given a new id. Only the file layer is replaced.
#[test]
fn replace_file_layer_drops_an_id_no_file_holds() {
    let reg = AgentRegistry::new();
    assert!(reg.register_template(make_template("old_agent", vec![], false)));
    assert!(reg.register_template(plugin_template("reader", "notion")));

    let mut layer = FileLayer::new();
    assert!(layer.insert(
        make_template("new_agent", vec![], false),
        Some(config_path("new_agent.md"))
    ));
    reg.replace_file_layer(layer);

    assert!(reg.get_template("old_agent").is_none(), "no file holds it");
    assert!(reg.spawn_instance("old_agent", "t1".into()).is_err());
    assert!(reg.get_template("new_agent").is_some());
    assert!(
        reg.get_template("reader").is_some(),
        "a plugin's template is in the plugin layer, which is not replaced"
    );
    assert_eq!(reg.template_count(), 2);
}

/// `register_template` keeps its signature and places a template by its
/// `AgentSource`: a plugin's in the plugin layer, any other in the file
/// layer, with no path (subagent design §2.4 L-1).
#[test]
fn register_template_places_a_plugin_template_in_the_plugin_layer() {
    let reg = AgentRegistry::new();
    assert!(reg.register_template(plugin_template("reader", "notion")));
    assert!(reg.register_template(make_template("writer", vec![], false)));

    let layer_of = |id: &str| reg.provenance(id).map(|p| p.layer);
    assert_eq!(layer_of("reader"), Some(TemplateLayer::Plugin));
    assert_eq!(layer_of("writer"), Some(TemplateLayer::File));

    reg.replace_file_layer(FileLayer::new());
    assert_eq!(layer_of("writer"), None, "the file layer went whole");
    assert_eq!(layer_of("reader"), Some(TemplateLayer::Plugin));

    // A plugin load's rollback removes the template it registered.
    assert!(reg.remove_template("reader"));
    assert_eq!(layer_of("reader"), None);
}

/// `provenance` says which layer a resolved template comes from and the file
/// it was read from; it names nothing for an id no layer holds.
#[test]
fn provenance_names_the_layer_and_path() {
    let reg = AgentRegistry::new();
    let mut layer = FileLayer::new();
    assert!(layer.insert(
        make_template("code_agent", vec!["file_read"], false),
        Some(config_path("code.md"))
    ));
    reg.replace_file_layer(layer);
    assert!(reg.register_template(make_template("own_agent", vec![], false)));
    assert!(reg.register_template(plugin_template("reader", "notion")));

    let code = reg.provenance("code_agent").expect("a file template");
    assert_eq!(code.layer, TemplateLayer::File);
    assert_eq!(code.path, Some(config_path("code.md")));
    assert_eq!(code.state, TemplateState::Ready);
    assert!(code.not_used.is_empty());
    assert!(code.warnings.is_empty());

    let own = reg.provenance("own_agent").expect("registered, no file");
    assert_eq!((own.layer, own.path), (TemplateLayer::File, None));

    let reader = reg.provenance("reader").expect("a plugin template");
    assert_eq!((reader.layer, reader.path), (TemplateLayer::Plugin, None));

    assert!(reg.provenance("missing").is_none());
}

/// The file layer resolves over the bundled one, and the bundled template is
/// back as soon as no file holds its id; nothing removes it, and no plugin
/// template takes its id (subagent design §2.3, §2.4 L-1).
#[test]
fn the_file_layer_resolves_over_the_bundled_layer_which_nothing_removes() {
    let reg = AgentRegistry::new();
    let mut bundled = make_template("general_agent", vec!["file_read"], false);
    bundled.frontmatter.description = "bundled".to_string();
    reg.set_bundled_layer(vec![bundled]);
    assert_eq!(
        reg.provenance("general_agent").map(|p| p.layer),
        Some(TemplateLayer::Bundled)
    );

    let mut edited = make_template("general_agent", vec!["file_read"], false);
    edited.frontmatter.description = "edited".to_string();
    let mut layer = FileLayer::new();
    assert!(layer.insert(edited, Some(config_path("general_agent.md"))));
    reg.replace_file_layer(layer);
    let resolved = reg.get_template("general_agent").expect("resolved");
    assert_eq!(resolved.frontmatter.description, "edited");
    assert_eq!(reg.template_count(), 1, "one id, two layers");

    reg.replace_file_layer(FileLayer::new());
    let resolved = reg.get_template("general_agent").expect("resolved");
    assert_eq!(resolved.frontmatter.description, "bundled");

    assert!(!reg.remove_template("general_agent"), "never bundled");
    assert!(!reg.register_template(plugin_template("general_agent", "notion")));
    assert!(!reg.remove_plugin_template("general_agent", "notion"));
    assert_eq!(
        reg.provenance("general_agent").map(|p| p.layer),
        Some(TemplateLayer::Bundled)
    );
}

/// A file layer that brings back an id a plugin's withdrawal tombstoned
/// clears that tombstone, as `register_template` does for a template that
/// came back.
#[test]
fn a_file_layer_bringing_back_a_withdrawn_id_clears_its_tombstone() {
    let reg = AgentRegistry::new();
    assert!(reg.register_template(plugin_template("reader", "notion")));
    assert!(reg.remove_plugin_template("reader", "notion"));
    assert_eq!(reg.template_tombstone("reader").as_deref(), Some("notion"));

    let mut layer = FileLayer::new();
    assert!(layer.insert(
        make_template("reader", vec![], false),
        Some(config_path("reader.md"))
    ));
    assert!(
        !layer.insert(
            make_template("reader", vec!["web_fetch"], false),
            Some(config_path("reader2.md"))
        ),
        "the first file of an id holds it"
    );
    reg.replace_file_layer(layer);

    assert!(reg.template_tombstone("reader").is_none());
    assert_eq!(
        reg.provenance("reader").and_then(|p| p.path),
        Some(config_path("reader.md"))
    );
}

// ── C2: one order ───────────────────────────────────────────────

/// L-1: the lists have one order. Twenty ids registered out of order come
/// back sorted, whatever the map's hasher made of them, so the lead's
/// `<agents>` block and both spawn descriptions read the same from start to
/// start.
#[test]
fn list_templates_is_sorted_by_id() {
    let reg = AgentRegistry::new();
    let mut ids: Vec<String> = (0..20)
        .map(|i| format!("template_{:02}", (i * 7) % 20))
        .collect();
    for id in &ids {
        assert!(reg.register_template(make_template(id, vec!["web_search"], false)));
    }
    ids.sort();

    let listed: Vec<String> = reg
        .list_templates()
        .into_iter()
        .map(|t| t.frontmatter.id)
        .collect();
    assert_eq!(listed, ids);
    let found: Vec<String> = reg
        .find_templates_by_capability("web_search")
        .into_iter()
        .map(|t| t.frontmatter.id)
        .collect();
    assert_eq!(found, ids, "one layer: by id");
}

/// G-1: `find_templates_by_capability` tries the person's files, then the
/// bundled layer, then plugins, each by id — the order the lead is chosen in.
#[test]
fn find_templates_by_capability_orders_file_then_bundled_then_plugin() {
    let reg = AgentRegistry::new();
    reg.set_bundled_layer(vec![
        make_template("lead_b", vec!["orchestration"], true),
        make_template("lead_a", vec!["orchestration"], true),
    ]);
    let mut layer = FileLayer::new();
    layer.insert(
        make_template("my_lead_z", vec!["orchestration"], true),
        Some(config_path("my_lead_z.md")),
    );
    layer.insert(
        make_template("my_lead_y", vec!["orchestration"], true),
        Some(config_path("my_lead_y.md")),
    );
    reg.replace_file_layer(layer);
    assert!(reg.register_template(AgentTemplate {
        frontmatter: make_template("aa_plugin_lead", vec!["orchestration"], true).frontmatter,
        ..plugin_template("aa_plugin_lead", "notion")
    }));

    let ids: Vec<String> = reg
        .find_templates_by_capability("orchestration")
        .into_iter()
        .map(|t| t.frontmatter.id)
        .collect();
    assert_eq!(
        ids,
        [
            "my_lead_y",
            "my_lead_z",
            "lead_a",
            "lead_b",
            "aa_plugin_lead"
        ]
    );
    let all: Vec<String> = reg
        .list_templates_by_layer()
        .into_iter()
        .map(|t| t.frontmatter.id)
        .collect();
    assert_eq!(all, ids);
}

/// L-4: an id the file layer withholds is out of every list, refused at a
/// spawn with the file's reason, and still held: a plugin cannot take it.
#[test]
fn a_withheld_id_is_refused_and_still_held() {
    let reg = AgentRegistry::new();
    let mut layer = FileLayer::new();
    assert!(layer.withhold(
        "reader".to_string(),
        config_path("reader.md"),
        "does not parse (Missing YAML frontmatter)".to_string(),
    ));
    reg.replace_file_layer(layer);

    assert!(reg.list_templates().is_empty());
    assert!(reg.get_template("reader").is_none());
    assert!(!reg.register_template(plugin_template("reader", "notion")));
    assert_eq!(
        reg.spawn_instance("reader", "t1".to_string()).unwrap_err(),
        format!(
            "template 'reader' is withheld: {} does not parse (Missing YAML frontmatter); \
             fix it, or remove it",
            config_path("reader.md").display()
        )
    );

    // With a bundled template under it, the refusal says what removing the
    // file brings back.
    reg.set_bundled_layer(vec![make_template("reader", vec![], false)]);
    assert!(
        reg.spawn_instance("reader", "t2".to_string())
            .unwrap_err()
            .ends_with("fix it, or remove it to use the bundled template")
    );
}

/// The plugin manager's rollback is `remove_template`'s one caller from C2:
/// with a file over the id it registered, the rollback takes the plugin's
/// template, not the person's file (C1's hand-off).
#[test]
fn remove_template_takes_a_plugins_template_before_a_files() {
    let reg = AgentRegistry::new();
    assert!(reg.register_template(plugin_template("reader", "notion")));
    let mut layer = FileLayer::new();
    layer.insert(
        make_template("reader", vec!["file_read"], false),
        Some(config_path("reader.md")),
    );
    reg.replace_file_layer(layer);

    assert!(reg.remove_template("reader"));

    let provenance = reg.provenance("reader").expect("the file still holds it");
    assert_eq!(provenance.layer, TemplateLayer::File);
    reg.replace_file_layer(FileLayer::new());
    assert!(
        reg.get_template("reader").is_none(),
        "nothing left under it"
    );
}

/// A plugin's template withdrawn while a file of the person's holds its id
/// leaves no tombstone: the id still resolves, and a spawn of it that fails
/// for another reason must not name the plugin (C1's hand-off).
#[test]
fn a_plugin_withdrawal_under_a_file_template_leaves_no_tombstone() {
    let reg = AgentRegistry::new();
    assert!(reg.register_template(plugin_template("reader", "notion")));
    let mut layer = FileLayer::new();
    layer.insert(
        make_template("reader", vec!["file_read"], false),
        Some(config_path("reader.md")),
    );
    reg.replace_file_layer(layer);

    assert!(reg.remove_plugin_template("reader", "notion"));

    assert!(reg.template_tombstone("reader").is_none());
    assert_eq!(
        reg.provenance("reader").map(|p| p.layer),
        Some(TemplateLayer::File)
    );
}

/// L-1, P-2 (§7 O-4 (a)): a plugin's template naming a bundled id is
/// refused, and the refusal is listed on the holder's row — the plugin and
/// the reason — until the plugin is withdrawn.
#[test]
fn a_refused_plugin_template_is_listed_plugin_refused() {
    let registry = AgentRegistry::new();
    registry.set_bundled_layer(crate::bundled::bundled_templates());

    assert!(!registry.register_template(plugin_template("general_agent", "acme")));
    assert!(!registry.register_template(plugin_template("general_agent", "acme")));

    let provenance = registry.provenance("general_agent").unwrap();
    assert_eq!(provenance.layer, TemplateLayer::Bundled);
    assert_eq!(
        provenance.not_used,
        [NotUsed {
            layer: TemplateLayer::Plugin,
            path: None,
            reason: NotUsedReason::PluginRefused,
            plugin: Some("acme".to_string()),
        }],
        "listed once, however often the plugin asks"
    );
    assert!(matches!(
        registry.get_template("general_agent").unwrap().source,
        AgentSource::Internal
    ));

    registry.forget_plugin_refusals("acme");
    assert!(
        registry
            .provenance("general_agent")
            .unwrap()
            .not_used
            .is_empty()
    );
}

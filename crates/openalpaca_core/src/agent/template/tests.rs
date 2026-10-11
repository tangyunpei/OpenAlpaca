use super::*;

const VALID_AGENT: &str = r#"---
id: "code_agent"
name: "Code Agent"
description: "Software development agent for coding tasks"
icon: "code"
singleton: false
capabilities:
  - "file_read"
  - "file_write"
  - "shell_execute"
denied_capabilities:
  - "web_search"
  - "web_fetch"
temperature: 0.3
verbosity: "detailed"
model: "claude-sonnet-4-5-20250929"
fallback_models:
  - "claude-opus-4-6"
max_tool_calls: 50
timeout_seconds: 600
max_cost_per_task: 3.0
require_confirmation_for:
  - "shell_execute"
---

## Persona

You are an expert software development agent. Your job is to implement
code changes, fix bugs, refactor code, and verify your work by running tests.

## Guidelines

1. Read existing code first to understand context and conventions.
2. Make minimal, focused changes.
3. Follow existing code style.
4. Run tests after changes to verify correctness.
"#;

const MINIMAL_AGENT: &str = r#"---
id: "minimal"
name: "Minimal Agent"
description: "A minimal agent definition"
---
"#;

const SINGLETON_AGENT: &str = r#"---
id: "lead_agent"
name: "Lead Agent"
description: "Orchestrates complex tasks"
icon: "brain"
singleton: true
capabilities:
  - "orchestration"
temperature: 0.3
verbosity: "detailed"
model: "claude-sonnet-4-5-20250929"
---

## Persona

You are a strategic orchestration agent. Break complex tasks into
sub-objectives, delegate to specialized agents, and synthesize results.
"#;

#[test]
fn test_parse_frontmatter_full() {
    let fm = parse_agent_frontmatter(VALID_AGENT).expect("should parse");
    assert_eq!(fm.id, "code_agent");
    assert_eq!(fm.name, "Code Agent");
    assert_eq!(
        fm.description,
        "Software development agent for coding tasks"
    );
    assert_eq!(fm.icon, Some("code".to_string()));
    assert!(!fm.singleton);
    assert_eq!(fm.capabilities, vec!["file_read", "file_write", "shell_execute"]);
    assert_eq!(fm.denied_capabilities, vec!["web_search", "web_fetch"]);
    assert_eq!(fm.temperature, 0.3);
    assert_eq!(fm.verbosity, "detailed");
    assert_eq!(fm.model, Some("claude-sonnet-4-5-20250929".to_string()));
    assert_eq!(fm.fallback_models, vec!["claude-opus-4-6"]);
    assert_eq!(fm.max_tool_calls, Some(50));
    assert_eq!(fm.timeout_seconds, Some(600));
    assert_eq!(fm.max_cost_per_task, Some(3.0));
    assert_eq!(fm.require_confirmation_for, vec!["shell_execute"]);
}

#[test]
fn test_parse_full_document() {
    let doc = parse_agent_markdown(VALID_AGENT).expect("should parse");
    assert_eq!(doc.frontmatter.id, "code_agent");
    assert!(!doc.body.is_empty());
    assert!(doc.body.contains("expert software development agent"));
    assert!(doc.sections.contains_key("Persona"));
    assert!(doc.sections.contains_key("Guidelines"));
    assert!(doc.sections["Persona"].contains("expert software development agent"));
    assert!(doc.sections["Guidelines"].contains("Read existing code first"));
}

#[test]
fn test_parse_minimal() {
    let doc = parse_agent_markdown(MINIMAL_AGENT).expect("should parse");
    assert_eq!(doc.frontmatter.id, "minimal");
    assert_eq!(doc.frontmatter.name, "Minimal Agent");
    assert_eq!(doc.frontmatter.description, "A minimal agent definition");
    assert_eq!(doc.frontmatter.icon, None);
    assert!(!doc.frontmatter.singleton);
    assert!(doc.frontmatter.capabilities.is_empty());
    assert!(doc.frontmatter.denied_capabilities.is_empty());
    assert_eq!(doc.frontmatter.temperature, 0.5); // default
    assert_eq!(doc.frontmatter.verbosity, "normal"); // default
    assert_eq!(doc.frontmatter.model, None);
    assert!(doc.frontmatter.fallback_models.is_empty());
    assert_eq!(doc.frontmatter.max_tool_calls, None);
    assert!(doc.body.is_empty());
    assert!(doc.sections.is_empty());
}

#[test]
fn test_parse_singleton() {
    let fm = parse_agent_frontmatter(SINGLETON_AGENT).expect("should parse");
    assert_eq!(fm.id, "lead_agent");
    assert!(fm.singleton);
    assert_eq!(fm.capabilities, vec!["orchestration"]);
}

#[test]
fn test_missing_id() {
    let input = "---\nname: \"test\"\ndescription: \"test\"\n---\n";
    let err = parse_agent_frontmatter(input).expect_err("should fail");
    assert_eq!(err, AgentParseError::MissingField("id"));
}

#[test]
fn test_missing_name() {
    let input = "---\nid: \"test\"\ndescription: \"test\"\n---\n";
    let err = parse_agent_frontmatter(input).expect_err("should fail");
    assert_eq!(err, AgentParseError::MissingField("name"));
}

#[test]
fn test_missing_description() {
    let input = "---\nid: \"test\"\nname: \"test\"\n---\n";
    let err = parse_agent_frontmatter(input).expect_err("should fail");
    assert_eq!(err, AgentParseError::MissingField("description"));
}

#[test]
fn test_missing_frontmatter() {
    let input = "# No frontmatter\nJust text.";
    let err = parse_agent_frontmatter(input).expect_err("should fail");
    assert_eq!(err, AgentParseError::MissingFrontmatter);
}

#[test]
fn test_unterminated_frontmatter() {
    let input = "---\nid: \"test\"\nname: \"test\"\n";
    let err = parse_agent_frontmatter(input).expect_err("should fail");
    assert_eq!(err, AgentParseError::UnterminatedFrontmatter);
}

#[test]
fn test_unquoted_values() {
    let input = r#"---
id: my_agent
name: My Agent
description: A test agent without quotes
temperature: 0.7
verbosity: concise
---
"#;
    let fm = parse_agent_frontmatter(input).expect("unquoted values should parse");
    assert_eq!(fm.id, "my_agent");
    assert_eq!(fm.name, "My Agent");
    assert_eq!(fm.description, "A test agent without quotes");
    assert_eq!(fm.temperature, 0.7);
    assert_eq!(fm.verbosity, "concise");
}

#[test]
fn test_unknown_fields_tolerated() {
    let input = r#"---
id: "test"
name: "Test"
description: "Test agent"
unknown_field: "hello"
another_unknown:
  - "item1"
---
"#;
    let fm = parse_agent_frontmatter(input).expect("unknown fields should be tolerated");
    assert_eq!(fm.id, "test");
}

#[test]
fn test_render_roundtrip() {
    let doc = parse_agent_markdown(VALID_AGENT).expect("should parse");
    let rendered = render_agent_markdown(&doc);
    let reparsed = parse_agent_markdown(&rendered).expect("rendered should re-parse");

    assert_eq!(doc.frontmatter, reparsed.frontmatter);
    assert_eq!(
        doc.sections.keys().count(),
        reparsed.sections.keys().count()
    );
    for (key, value) in &doc.sections {
        let reparsed_value = reparsed.sections.get(key).expect("section should exist");
        assert_eq!(
            value.trim(),
            reparsed_value.trim(),
            "Section '{}' content should match",
            key
        );
    }
}

#[test]
fn test_render_minimal_roundtrip() {
    let doc = parse_agent_markdown(MINIMAL_AGENT).expect("should parse");
    let rendered = render_agent_markdown(&doc);
    let reparsed = parse_agent_markdown(&rendered).expect("rendered should re-parse");
    assert_eq!(doc.frontmatter, reparsed.frontmatter);
}

#[test]
fn test_extract_persona() {
    let doc = parse_agent_markdown(VALID_AGENT).expect("should parse");
    let persona = extract_persona(&doc);
    assert!(persona.contains("expert software development agent"));
}

#[test]
fn test_extract_persona_default() {
    let doc = parse_agent_markdown(MINIMAL_AGENT).expect("should parse");
    let persona = extract_persona(&doc);
    assert_eq!(persona, "You are a helpful assistant.");
}

#[test]
fn test_singleton_extract_persona() {
    let doc = parse_agent_markdown(SINGLETON_AGENT).expect("should parse");
    let persona = extract_persona(&doc);
    assert!(persona.contains("strategic orchestration agent"));
}

#[test]
fn test_to_subagent_orchestration_allows_coordination_tools() {
    use crate::security::capabilities::{Allowlist, CapabilityManager};

    let doc = parse_agent_markdown(SINGLETON_AGENT).expect("should parse");
    let agent = doc.to_subagent("lead-01", "task-1");
    let allowed = Allowlist::from_agent_constraints(&agent.constraints);
    for tool in [
        "spawn_subagent",
        "spawn_subagents_batch",
        "check_subagent_status",
        "wait_for_subagents",
        "post_update",
        "queue_followup",
    ] {
        assert!(
            CapabilityManager::check_agent_capability(
                &agent.id,
                tool,
                &allowed,
                &agent.constraints.denied_capabilities
            )
            .is_ok(),
            "coordination tool '{}' should pass the capability allowlist",
            tool
        );
    }
}

#[test]
fn test_to_subagent_non_orchestration_denies_coordination_tools() {
    use crate::security::capabilities::{Allowlist, CapabilityManager};

    let doc = parse_agent_markdown(VALID_AGENT).expect("should parse");
    let agent = doc.to_subagent("code-01", "task-1");
    assert!(
        CapabilityManager::check_agent_capability(
            &agent.id,
            "spawn_subagent",
            &Allowlist::from_agent_constraints(&agent.constraints),
            &agent.constraints.denied_capabilities
        )
        .is_err(),
        "non-orchestration agents must not get coordination tools"
    );
}

/// R84: the spill stub tells the model to page a large tool result with
/// `read_result`, so every **shipped** template grants the capability — an agent
/// told to page with a tool the gate refuses is worse than the head-only cut the
/// spill replaced. The ambient append above (`workspace_read`/`workspace_write`)
/// is deliberately *not* where this lives: owner decision T15 is untouched, and
/// a user's own template stays the user's to write.
#[test]
fn every_shipped_template_grants_read_result() {
    use crate::security::capabilities::{Allowlist, CapabilityManager};

    let agents = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/agents");
    let mut seen = 0;
    for entry in std::fs::read_dir(&agents).expect("the shipped templates are in the checkout") {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let body = std::fs::read_to_string(&path).unwrap();
        let doc = parse_agent_markdown(&body)
            .unwrap_or_else(|e| panic!("{} does not parse: {e}", path.display()));
        assert!(
            doc.frontmatter.capabilities.iter().any(|c| c == "read_result"),
            "{} must grant read_result",
            path.display()
        );

        // And the grant is what the gate reads: resolved allowlist, not the raw
        // frontmatter list.
        let agent = doc.to_subagent("a1", "task-1");
        assert!(
            CapabilityManager::check_agent_capability(
                &agent.id,
                "read_result",
                &Allowlist::from_agent_constraints(&agent.constraints),
                &agent.constraints.denied_capabilities,
            )
            .is_ok(),
            "{} resolves read_result into its allowlist",
            path.display()
        );
        seen += 1;
    }
    assert!(seen >= 9, "every shipped template was checked, not {seen}");
}

/// Storage design §15 and R42. `writing_agent` gains `file_read` in the
/// shipped template — it is asked to change documents, so it reads them
/// first. And `file_list` comes with `file_read`, `artifact_read` with
/// `read_result`: every shipped template that names `file_read` is handed
/// both file readers and every one (all nine name `read_result`) is handed
/// `artifact_read`, with no other template edit. Walked as the subagent spawn
/// walks a template: `to_subagent`, then `resolve_agent_tools` over the
/// builtins.
#[test]
fn shipped_templates_are_handed_the_stage_2_readers_by_the_capabilities_they_name() {
    use crate::tools::ToolRegistry;
    use crate::tools::builtins::builtin_tools;

    let registry = ToolRegistry::default();
    for tool in builtin_tools(None, None, None) {
        registry.register(tool).unwrap();
    }
    let registry = std::sync::Arc::new(registry);

    let agents = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/agents");
    let mut seen = 0;
    for entry in std::fs::read_dir(&agents).expect("the shipped templates are in the checkout") {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let body = std::fs::read_to_string(&path).unwrap();
        let doc = parse_agent_markdown(&body)
            .unwrap_or_else(|e| panic!("{} does not parse: {e}", path.display()));
        let agent = doc.to_subagent("a1", "task-1");
        let surface: Vec<String> = crate::tools::resolve_agent_tools(&agent, &registry, None)
            .into_iter()
            .map(|d| d.name)
            .collect();
        let has = |name: &str| surface.iter().any(|n| n == name);
        let names_file_read = doc.frontmatter.capabilities.iter().any(|c| c == "file_read");

        assert_eq!(has("file_read"), names_file_read, "{}: {surface:?}", path.display());
        assert_eq!(has("file_list"), names_file_read, "{}: {surface:?}", path.display());
        assert!(has("artifact_read"), "{}: {surface:?}", path.display());
        if doc.frontmatter.id == "writing_agent" {
            assert!(names_file_read, "writing_agent reads files (§15)");
        }
        seen += 1;
    }
    assert!(seen >= 9, "every shipped template was checked, not {seen}");
}

/// Owner decision T34 (`tasks/api-fix-plan.md` §0; storage design §12 R30,
/// Stage 0 fix F1). The shipped `system_agent` template used to end its
/// frontmatter with `require_confirmation_for: ["file_delete"]`. An explicit,
/// non-empty list is returned as written by `effective_confirmation_set`
/// (`security/sandbox/mod.rs`; since R-T35 with `file_edit` added beside a
/// `file_write` it names) instead of the set derived from the tools'
/// `destructive_hint` annotations, and no tool is named `file_delete`, so
/// that agent's `shell_execute` and `file_write` never prompted — live, the
/// lead delegated to it a shell write the person had just denied another
/// agent. The template is read from the checkout and walked exactly as the
/// subagent spawn walks it: `to_subagent`, then `SandboxPolicy::from_constraints`,
/// then the effective set over a registry holding the builtins.
///
/// Only `shell_execute`, `file_write` and `file_edit` (ST-22) are pinned,
/// plus one read that must not prompt: ST-15 took `artifact_write` and
/// `workspace_write` out of the derived set
/// (`annotations_for_builtin_by_product_writers_never_prompt`), so the whole
/// set is not asserted.
#[test]
fn the_shipped_system_agent_template_prompts_for_its_shell_and_file_writes() {
    use crate::security::sandbox::{SandboxPolicy, effective_confirmation_set};
    use crate::tools::ToolRegistry;
    use crate::tools::builtins::builtin_tools;

    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../config/agents/system_agent.md");
    let body = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
    let doc = parse_agent_markdown(&body)
        .unwrap_or_else(|e| panic!("{} does not parse: {e}", path.display()));
    assert_eq!(doc.frontmatter.id, "system_agent");

    // The registry the spawn checks against: the builtins, with the
    // annotations `builtin_tools` registers them with.
    let registry = ToolRegistry::default();
    for tool in builtin_tools(None, None, None) {
        registry.register(tool).unwrap();
    }

    let agent = doc.to_subagent("system_agent::t34", "task-1");
    let policy = SandboxPolicy::from_constraints(&agent.id, &agent.constraints);
    let set = effective_confirmation_set(&policy, &registry);

    for name in ["shell_execute", "file_write", "file_edit"] {
        assert!(
            set.contains(&name.to_string()),
            "{name} must prompt for system_agent; the effective set is {set:?}"
        );
    }
    assert!(
        !set.contains(&"file_read".to_string()),
        "a read never prompts; the effective set is {set:?}"
    );
}

/// R48 (storage design §13): `file_edit` comes with the `file_write`
/// capability. Every shipped template that grants `file_write` is handed
/// `file_edit` beside it and its sandbox admits the call; every one that
/// denies `file_write` — and the lead's, which grants neither — is handed
/// neither, and its sandbox refuses `file_edit` by name. Each template is
/// walked exactly as the subagent spawn walks it: `to_subagent`,
/// `resolve_agent_tools`, `SandboxPolicy::from_constraints`,
/// `admit_tool_surface`.
#[test]
fn file_edit_comes_with_the_file_write_capability_in_every_shipped_template() {
    use crate::security::capabilities::CapabilityManager;
    use crate::security::sandbox::SandboxPolicy;
    use crate::tools::ToolRegistry;
    use crate::tools::builtins::builtin_tools;

    let registry = std::sync::Arc::new(ToolRegistry::default());
    for tool in builtin_tools(None, None, None) {
        registry.register(tool).unwrap();
    }
    let agents = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/agents");
    for (id, writes) in [
        ("code_agent", true),
        ("general_agent", true),
        ("system_agent", true),
        ("writing_agent", true),
        ("explore_agent", false),
        ("planning_agent", false),
        ("research_agent", false),
        ("review_agent", false),
        ("lead_agent", false),
    ] {
        let path = agents.join(format!("{id}.md"));
        let body = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
        let doc = parse_agent_markdown(&body)
            .unwrap_or_else(|e| panic!("{} does not parse: {e}", path.display()));
        let agent = doc.to_subagent(&format!("{id}::r48"), "task-1");

        let surface = crate::tools::resolve_agent_tools(&agent, &registry, None);
        let names: Vec<&str> = surface.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(
            names.contains(&"file_write"),
            writes,
            "{id}: file_write on {names:?}"
        );
        assert_eq!(
            names.contains(&"file_edit"),
            writes,
            "{id}: file_edit on {names:?}"
        );

        let mut policy = SandboxPolicy::from_constraints(&agent.id, &agent.constraints);
        policy.admit_tool_surface(&surface);
        assert_eq!(
            CapabilityManager::check_agent_capability(
                &agent.id,
                "file_edit",
                &policy.allowed_capabilities,
                &policy.denied_capabilities,
            )
            .is_ok(),
            writes,
            "{id}: the sandbox admits file_edit"
        );
    }
}

// ── Design 09 §5 (LQ3): memory search for every built-in agent ──────────

/// Whether the gate admits `memory_read` for `agent`, from the resolved
/// allowlist — what a subagent's call is checked against, not the raw
/// frontmatter.
fn memory_read_admitted(agent: &crate::agent::subagent::SubAgent) -> bool {
    use crate::security::capabilities::{Allowlist, CapabilityManager};

    CapabilityManager::check_agent_capability(
        &agent.id,
        "memory_read",
        &Allowlist::from_agent_constraints(&agent.constraints),
        &agent.constraints.denied_capabilities,
    )
    .is_ok()
}

fn memory_read_entries(agent: &crate::agent::subagent::SubAgent) -> (usize, usize) {
    (
        agent
            .capabilities
            .iter()
            .filter(|c| c.name == "memory_read")
            .count(),
        agent
            .constraints
            .allowed_capabilities
            .iter()
            .filter(|c| *c == "memory_read")
            .count(),
    )
}

/// An installed template written before memory search reached every agent.
const PLAIN_AGENT: &str = r#"---
id: "notes_agent"
name: "Notes Agent"
description: "A template that does not name memory_read"
capabilities:
  - "file_read"
---
"#;

/// A template that names `memory_read` itself.
const NAMING_AGENT: &str = r#"---
id: "recall_agent"
name: "Recall Agent"
description: "A template that names memory_read"
capabilities:
  - "file_read"
  - "memory_read"
---
"#;

/// A plugin agent's executor that is never driven: these tests only build
/// the subagent.
struct IdlePluginExecutor;

#[async_trait::async_trait]
impl openalpaca_api::plugin_traits::PluginAgentExecutor for IdlePluginExecutor {
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
        "acme"
    }

    fn agent_id(&self) -> &str {
        "notes_agent"
    }
}

fn from_plugin(mut template: AgentTemplate) -> AgentTemplate {
    template.source = AgentSource::Plugin {
        plugin_id: "acme".to_string(),
        executor: Arc::new(IdlePluginExecutor),
    };
    template
}

/// Design 09 §5: every shipped template names `memory_read` — the three
/// that did not (writing, research, system) among them — and the gate reads
/// it from the resolved allowlist, so a fresh install's agents can all
/// search memory.
#[test]
fn all_nine_templates_name_memory_read() {
    let agents = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/agents");
    let mut seen = 0;
    for entry in std::fs::read_dir(&agents).expect("the shipped templates are in the checkout") {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let body = std::fs::read_to_string(&path).unwrap();
        let doc = parse_agent_markdown(&body)
            .unwrap_or_else(|e| panic!("{} does not parse: {e}", path.display()));
        assert!(
            doc.frontmatter
                .capabilities
                .iter()
                .any(|c| c == "memory_read"),
            "{} must name memory_read",
            path.display()
        );
        let agent = doc.to_subagent("a1", "task-1");
        assert_eq!(
            memory_read_entries(&agent),
            (1, 1),
            "{}: listed once, never appended a second time",
            path.display()
        );
        assert!(memory_read_admitted(&agent), "{}", path.display());
        seen += 1;
    }
    assert!(seen >= 9, "every shipped template was checked, not {seen}");
}

/// Design 09 §5 (LQ3): the owner's installed `config/agents/` is never
/// re-seeded, so an internal template that does not name `memory_read` is
/// granted it on the way to a subagent — in its capabilities, which tool
/// resolution reads, and in its allowlist, which the gate reads — as
/// `workspace_read`/`workspace_write` are.
#[test]
fn an_internal_template_is_granted_memory_read_ambiently() {
    let doc = parse_agent_markdown(PLAIN_AGENT).unwrap();
    assert!(
        !doc.frontmatter
            .capabilities
            .iter()
            .any(|c| c == "memory_read")
    );
    let agent = doc.to_subagent("notes-1", "task-1");
    assert_eq!(memory_read_entries(&agent), (1, 1));
    assert!(memory_read_admitted(&agent));
    // The template itself is not rewritten: the grant lives on the subagent.
    assert_eq!(doc.frontmatter.capabilities, vec!["file_read"]);

    let named = parse_agent_markdown(NAMING_AGENT)
        .unwrap()
        .to_subagent("recall-1", "task-1");
    assert_eq!(memory_read_entries(&named), (1, 1), "no second copy");
}

/// Design 09 §5: `denied_capabilities` still wins — a template that denies
/// `memory_read`, in any case, is granted nothing, and the gate refuses it.
#[test]
fn an_ambient_grant_respects_denied_capabilities() {
    for denied in ["memory_read", "Memory_Read"] {
        let md = format!(
            "---\nid: \"quiet_agent\"\nname: \"Quiet Agent\"\ndescription: \"Denies memory\"\n\
             capabilities:\n  - \"file_read\"\ndenied_capabilities:\n  - \"{denied}\"\n---\n"
        );
        let agent = parse_agent_markdown(&md)
            .unwrap()
            .to_subagent("quiet-1", "task-1");
        assert!(
            !agent
                .capabilities
                .iter()
                .any(|c| c.name.eq_ignore_ascii_case("memory_read")),
            "denied {denied:?}: nothing is appended to the capabilities"
        );
        assert!(
            !agent
                .constraints
                .allowed_capabilities
                .iter()
                .any(|c| c == "memory_read"),
            "denied {denied:?}: nothing is appended to the allowlist"
        );
        assert!(!memory_read_admitted(&agent), "denied {denied:?}");
    }
}

/// Design 09 §5 (LQ3): a plugin-contributed template gets `memory_read`
/// only by naming it. Its subagent's tool results go back to the plugin's
/// own process, and the memory read tools never prompt, so an ambient grant
/// would hand every note the owner has to third-party code by default.
#[test]
fn a_plugin_template_gets_no_ambient_memory_read() {
    let plain = from_plugin(parse_agent_markdown(PLAIN_AGENT).unwrap());
    let agent = plain.to_subagent("acme-1", "task-1");
    assert_eq!(memory_read_entries(&agent), (0, 0));
    assert!(!memory_read_admitted(&agent));
    // The workspace tools stay ambient for a plugin template, as before.
    for ws in ["workspace_read", "workspace_write"] {
        assert!(agent.capabilities.iter().any(|c| c.name == ws), "{ws}");
    }

    // A plugin template that names it keeps it, exactly as today.
    let named = from_plugin(parse_agent_markdown(NAMING_AGENT).unwrap());
    let agent = named.to_subagent("acme-2", "task-1");
    assert_eq!(memory_read_entries(&agent), (1, 1));
    assert!(memory_read_admitted(&agent));
}

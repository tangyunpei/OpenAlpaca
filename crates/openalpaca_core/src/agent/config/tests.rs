use super::*;

fn sample_toml() -> &'static str {
    r#"
[agent]
id = "test_agent"
name = "Test Agent"
description = "A test agent"
icon = "star"

[skills]
assigned = ["web_search", "summarize"]
denied = ["shell_execute"]

[preset]
persona = "You are a test assistant."
temperature = 0.3
verbosity = "detailed"

[constraints]
max_tool_calls = 20
timeout_seconds = 300
max_cost_per_task = 0.50
require_confirmation_for = ["file_delete"]
"#
}

#[test]
fn test_parse_toml() {
    let config: AgentConfigFile = toml::from_str(sample_toml()).unwrap();
    assert_eq!(config.agent.id, "test_agent");
    assert_eq!(config.skills.assigned.len(), 2);
    assert_eq!(config.preset.temperature, Some(0.3));
    assert_eq!(
        config.constraints.as_ref().unwrap().max_tool_calls,
        Some(20)
    );
}

#[test]
fn test_into_subagent() {
    let config: AgentConfigFile = toml::from_str(sample_toml()).unwrap();
    let agent = config.into_subagent();
    assert_eq!(agent.id, "test_agent");
    assert_eq!(agent.name, "Test Agent");
    assert_eq!(agent.skills.len(), 2);
    assert_eq!(agent.preset.temperature, 0.3);
    assert_eq!(agent.constraints.max_tool_calls, Some(20));
    assert!(agent.status.is_available());
}

#[test]
fn test_require_confirmation_for_roundtrip() {
    let config: AgentConfigFile = toml::from_str(sample_toml()).unwrap();
    let agent = config.into_subagent();
    assert_eq!(
        agent.constraints.require_confirmation_for,
        vec!["file_delete"]
    );
}

#[test]
fn test_into_storage_config() {
    let config: AgentConfigFile = toml::from_str(sample_toml()).unwrap();
    let sc = config.into_storage_config();
    assert_eq!(sc.id, "test_agent");
    assert_eq!(sc.status, "idle");
    assert!(sc.constraints_json.is_some());
}

#[test]
fn test_skills_denied_merged_into_denied_capabilities() {
    let config: AgentConfigFile = toml::from_str(sample_toml()).unwrap();
    let agent = config.into_subagent();
    // skills.denied = ["shell_execute"] should be merged
    assert!(
        agent
            .constraints
            .denied_capabilities
            .contains(&"shell_execute".to_string())
    );
}

#[test]
fn test_toml_with_capabilities() {
    let toml_str = r#"
[agent]
id = "cap_agent"
name = "Cap Agent"
description = "Agent with capabilities"

[skills]
assigned = ["web_search"]
denied = ["shell_execute"]

[preset]
persona = "test"

[constraints]
max_tool_calls = 5
allowed_capabilities = ["web_search", "summarize"]
denied_capabilities = ["file_write"]
"#;
    let config: AgentConfigFile = toml::from_str(toml_str).unwrap();
    let agent = config.into_subagent();
    assert_eq!(
        agent.constraints.allowed_capabilities,
        vec!["web_search", "summarize"]
    );
    // "file_write" from denied_capabilities + "shell_execute" from skills.denied
    assert!(
        agent
            .constraints
            .denied_capabilities
            .contains(&"file_write".to_string())
    );
    assert!(
        agent
            .constraints
            .denied_capabilities
            .contains(&"shell_execute".to_string())
    );
}

#[test]
fn test_toml_without_constraints_but_with_denied_skills() {
    let toml_str = r#"
[agent]
id = "no_constraints"
name = "No Constraints"
description = "Agent without constraints section"

[skills]
assigned = ["web_search"]
denied = ["shell_execute"]

[preset]
persona = "test"
"#;
    let config: AgentConfigFile = toml::from_str(toml_str).unwrap();
    let agent = config.into_subagent();
    // skills.denied should still be captured even without [constraints]
    assert!(
        agent
            .constraints
            .denied_capabilities
            .contains(&"shell_execute".to_string())
    );
}

// ── Template bridge tests ──────────────────────────────────────

fn sample_template() -> AgentTemplate {
    use crate::agent::template::parse_agent_markdown;
    parse_agent_markdown(
        r#"---
id: "bridge_agent"
name: "Bridge Agent"
description: "Agent for bridge testing"
icon: "bridge"
skills:
  - "web_search"
  - "summarize"
denied_skills:
  - "shell_execute"
temperature: 0.3
verbosity: "detailed"
model: "claude-sonnet-4-5-20250929"
fallback_models:
  - "claude-haiku-4-5-20251001"
max_tool_calls: 20
timeout_seconds: 300
max_cost_per_task: 0.50
require_confirmation_for:
  - "file_delete"
---

## Persona

You are a bridge testing assistant.
"#,
    )
    .unwrap()
}

#[test]
fn test_from_template() {
    let template = sample_template();
    let config = AgentConfigFile::from_template(&template);

    assert_eq!(config.agent.id, "bridge_agent");
    assert_eq!(config.agent.name, "Bridge Agent");
    assert_eq!(config.agent.icon, Some("bridge".to_string()));
    assert_eq!(config.skills.assigned, vec!["web_search", "summarize"]);
    assert_eq!(
        config.skills.denied,
        Some(vec!["shell_execute".to_string()])
    );
    assert_eq!(config.preset.temperature, Some(0.3));
    assert_eq!(config.preset.verbosity, Some("detailed".to_string()));
    assert!(config.preset.persona.contains("bridge testing assistant"));
    assert_eq!(
        config.constraints.as_ref().unwrap().max_tool_calls,
        Some(20)
    );
    assert_eq!(
        config.llm.as_ref().unwrap().model,
        Some("claude-sonnet-4-5-20250929".to_string())
    );
    assert_eq!(
        config.llm.as_ref().unwrap().fallback_models,
        Some(vec!["claude-haiku-4-5-20251001".to_string()])
    );
}

#[test]
fn test_from_template_roundtrip_to_subagent() {
    let template = sample_template();
    let config = AgentConfigFile::from_template(&template);
    let agent = config.into_subagent();

    assert_eq!(agent.id, "bridge_agent");
    assert_eq!(agent.template_id, "bridge_agent");
    assert_eq!(agent.skills.len(), 2);
    assert_eq!(agent.preset.temperature, 0.3);
    assert!(
        agent
            .constraints
            .denied_capabilities
            .contains(&"shell_execute".to_string())
    );
    assert!(agent.status.is_available());
}

#[test]
fn test_into_template() {
    let config: AgentConfigFile = toml::from_str(sample_toml()).unwrap();
    let template = config.into_template();

    assert_eq!(template.frontmatter.id, "test_agent");
    assert_eq!(template.frontmatter.name, "Test Agent");
    assert_eq!(template.frontmatter.skills, vec!["web_search", "summarize"]);
    assert_eq!(template.frontmatter.denied_skills, vec!["shell_execute"]);
    assert_eq!(template.frontmatter.temperature, 0.3);
    assert_eq!(template.frontmatter.max_tool_calls, Some(20));
    assert!(
        template
            .sections
            .get("Persona")
            .unwrap()
            .contains("test assistant")
    );
    assert!(!template.frontmatter.singleton);
}

#[test]
fn test_into_template_roundtrip() {
    // TOML → AgentConfigFile → AgentTemplate → AgentConfigFile → SubAgent
    let config: AgentConfigFile = toml::from_str(sample_toml()).unwrap();
    let template = config.into_template();
    let config2 = AgentConfigFile::from_template(&template);
    let agent = config2.into_subagent();

    assert_eq!(agent.id, "test_agent");
    assert_eq!(agent.skills.len(), 2);
    assert_eq!(agent.preset.temperature, 0.3);
    assert!(
        agent
            .constraints
            .denied_capabilities
            .contains(&"shell_execute".to_string())
    );
}

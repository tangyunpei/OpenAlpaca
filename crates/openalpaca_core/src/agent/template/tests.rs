use super::*;

const VALID_AGENT: &str = r#"---
id: "code_agent"
name: "Code Agent"
description: "Software development agent for coding tasks"
icon: "code"
singleton: false
skills:
  - "file_read"
  - "file_write"
  - "shell_execute"
denied_skills:
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
skills:
  - "lead_orchestration"
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
    assert_eq!(fm.skills, vec!["file_read", "file_write", "shell_execute"]);
    assert_eq!(fm.denied_skills, vec!["web_search", "web_fetch"]);
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
    assert!(doc.frontmatter.skills.is_empty());
    assert!(doc.frontmatter.denied_skills.is_empty());
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
    assert_eq!(fm.skills, vec!["lead_orchestration"]);
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

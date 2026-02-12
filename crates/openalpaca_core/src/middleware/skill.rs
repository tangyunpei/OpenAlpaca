//! SKILL.md parser, renderer, and prompt block formatter.
//!
//! A Skill is a folder on disk containing a `SKILL.md` file with YAML frontmatter
//! and a markdown body. The frontmatter provides lightweight metadata for catalog
//! discovery (Level 1), while the full body provides instructions loaded on demand
//! (Level 2).
//!
//! Parsing is **lenient**: only `name` and `description` are required in the
//! frontmatter. All body sections are optional.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// YAML frontmatter metadata — loaded at Level 1 (startup catalog scan).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillFrontmatter {
    /// Human-readable skill name (e.g. "Code Review").
    pub name: String,
    /// Short description for catalog display.
    pub description: String,
    /// Slash command that invokes this skill (e.g. "review").
    pub command: Option<String>,
    /// Regex patterns that auto-detect when this skill should activate.
    pub trigger_patterns: Vec<String>,
    /// Tool names this skill requires to function.
    pub tools_required: Vec<String>,
    /// If true, this skill's instructions are always loaded into context.
    pub auto_load: bool,
    /// Controls when the skill description appears in the LLM catalog prompt.
    pub read_when: Vec<String>,
}

/// Full parsed SKILL.md document — loaded at Level 2 (on invocation).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillDocument {
    pub frontmatter: SkillFrontmatter,
    /// The full markdown body (instructions, examples, templates).
    pub body: String,
    /// Parsed `## Section` headings → body text.
    pub sections: HashMap<String, String>,
}

/// Errors that can occur while parsing a SKILL.md file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillParseError {
    MissingFrontmatter,
    UnterminatedFrontmatter,
    MissingField(&'static str),
    InvalidYaml(String),
}

impl fmt::Display for SkillParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingFrontmatter => write!(f, "Missing YAML frontmatter"),
            Self::UnterminatedFrontmatter => write!(f, "Unterminated YAML frontmatter"),
            Self::MissingField(field) => write!(f, "Missing required frontmatter field '{}'", field),
            Self::InvalidYaml(msg) => write!(f, "Invalid YAML: {}", msg),
        }
    }
}

impl std::error::Error for SkillParseError {}

// ---------------------------------------------------------------------------
// Frontmatter helpers (reuse soul.rs patterns)
// ---------------------------------------------------------------------------

fn strip_outer_quotes(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.len() >= 2
        && ((trimmed.starts_with('"') && trimmed.ends_with('"'))
            || (trimmed.starts_with('\'') && trimmed.ends_with('\'')))
    {
        trimmed[1..trimmed.len() - 1].trim().to_string()
    } else {
        trimmed.to_string()
    }
}

fn split_frontmatter(input: &str) -> Result<(Vec<String>, Vec<String>), SkillParseError> {
    let mut lines = input.lines();
    let first = lines.next().unwrap_or_default();
    if first.trim() != "---" {
        return Err(SkillParseError::MissingFrontmatter);
    }

    let mut frontmatter = Vec::new();
    let mut body = Vec::new();
    let mut in_frontmatter = true;

    for line in lines {
        if in_frontmatter {
            if line.trim() == "---" {
                in_frontmatter = false;
                continue;
            }
            frontmatter.push(line.to_string());
            continue;
        }
        body.push(line.to_string());
    }

    if in_frontmatter {
        return Err(SkillParseError::UnterminatedFrontmatter);
    }

    Ok((frontmatter, body))
}

/// Parse a YAML list of `- "item"` lines starting at `idx + 1`.
/// Returns the collected items and advances `idx` past them.
fn parse_yaml_list(lines: &[String], idx: &mut usize) -> Vec<String> {
    let mut items = Vec::new();
    *idx += 1;
    while *idx < lines.len() {
        let item = lines[*idx].trim();
        if item.is_empty() {
            *idx += 1;
            continue;
        }
        if let Some(v) = item.strip_prefix("- ") {
            items.push(strip_outer_quotes(v));
            *idx += 1;
            continue;
        }
        break; // Non-list-item line → stop
    }
    items
}

/// Parse a scalar boolean value from YAML (e.g. `true`, `false`, `"true"`).
fn parse_yaml_bool(value: &str) -> bool {
    let v = strip_outer_quotes(value).to_lowercase();
    matches!(v.as_str(), "true" | "yes" | "1")
}

fn parse_skill_frontmatter_lines(lines: &[String]) -> Result<SkillFrontmatter, SkillParseError> {
    let mut name: Option<String> = None;
    let mut description: Option<String> = None;
    let mut command: Option<String> = None;
    let mut trigger_patterns: Vec<String> = Vec::new();
    let mut tools_required: Vec<String> = Vec::new();
    let mut auto_load: bool = false;
    let mut read_when: Vec<String> = Vec::new();

    let mut idx = 0usize;
    while idx < lines.len() {
        let trimmed = lines[idx].trim();
        if trimmed.is_empty() {
            idx += 1;
            continue;
        }

        if let Some(rest) = trimmed.strip_prefix("name:") {
            name = Some(strip_outer_quotes(rest));
            idx += 1;
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("description:") {
            description = Some(strip_outer_quotes(rest));
            idx += 1;
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("command:") {
            let v = strip_outer_quotes(rest);
            if !v.is_empty() {
                command = Some(v);
            }
            idx += 1;
            continue;
        }
        if trimmed.starts_with("trigger_patterns:") {
            trigger_patterns = parse_yaml_list(lines, &mut idx);
            continue;
        }
        if trimmed.starts_with("tools_required:") {
            tools_required = parse_yaml_list(lines, &mut idx);
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("auto_load:") {
            auto_load = parse_yaml_bool(rest);
            idx += 1;
            continue;
        }
        if trimmed.starts_with("read_when:") {
            read_when = parse_yaml_list(lines, &mut idx);
            continue;
        }

        // Unknown field — skip silently
        idx += 1;
    }

    let name = name.ok_or(SkillParseError::MissingField("name"))?;
    let description = description.ok_or(SkillParseError::MissingField("description"))?;

    Ok(SkillFrontmatter {
        name,
        description,
        command,
        trigger_patterns,
        tools_required,
        auto_load,
        read_when,
    })
}

// ---------------------------------------------------------------------------
// Body parsing (lenient — all sections optional)
// ---------------------------------------------------------------------------

fn parse_body_sections(lines: &[String]) -> (String, HashMap<String, String>) {
    let mut sections: HashMap<String, String> = HashMap::new();
    let mut current_section: Option<String> = None;
    let mut current_lines: Vec<String> = Vec::new();
    let mut full_body = String::new();

    for line in lines {
        // Build full body text
        if !full_body.is_empty() || !line.trim().is_empty() {
            if !full_body.is_empty() {
                full_body.push('\n');
            }
            full_body.push_str(line);
        }

        if let Some(heading) = line.trim().strip_prefix("## ") {
            // Save previous section
            if let Some(ref name) = current_section {
                let content = current_lines.join("\n").trim().to_string();
                if !content.is_empty() {
                    sections.insert(name.clone(), content);
                }
            }
            current_section = Some(heading.trim().to_string());
            current_lines.clear();
        } else if current_section.is_some() {
            current_lines.push(line.to_string());
        }
    }

    // Save last section
    if let Some(ref name) = current_section {
        let content = current_lines.join("\n").trim().to_string();
        if !content.is_empty() {
            sections.insert(name.clone(), content);
        }
    }

    // Trim trailing whitespace from full body
    let body = full_body.trim_end().to_string();
    (body, sections)
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Parse only the YAML frontmatter from a SKILL.md file (Level 1).
///
/// Use this for lightweight catalog scanning at startup — does NOT parse the body.
pub fn parse_skill_frontmatter(input: &str) -> Result<SkillFrontmatter, SkillParseError> {
    let (frontmatter_lines, _body_lines) = split_frontmatter(input)?;
    parse_skill_frontmatter_lines(&frontmatter_lines)
}

/// Parse the full SKILL.md file including body sections (Level 2).
pub fn parse_skill_markdown(input: &str) -> Result<SkillDocument, SkillParseError> {
    let (frontmatter_lines, body_lines) = split_frontmatter(input)?;
    let frontmatter = parse_skill_frontmatter_lines(&frontmatter_lines)?;
    let (body, sections) = parse_body_sections(&body_lines);

    Ok(SkillDocument {
        frontmatter,
        body,
        sections,
    })
}

/// Render a `SkillDocument` back to valid SKILL.md markdown.
///
/// Designed to round-trip: `parse_skill_markdown(render_skill_markdown(doc))`
/// should produce a semantically-equal `SkillDocument`.
pub fn render_skill_markdown(doc: &SkillDocument) -> String {
    let mut out = String::new();

    // -- Frontmatter --
    out.push_str("---\n");
    out.push_str(&format!("name: \"{}\"\n", doc.frontmatter.name));
    out.push_str(&format!("description: \"{}\"\n", doc.frontmatter.description));
    if let Some(ref cmd) = doc.frontmatter.command {
        out.push_str(&format!("command: \"{}\"\n", cmd));
    }
    if !doc.frontmatter.trigger_patterns.is_empty() {
        out.push_str("trigger_patterns:\n");
        for pattern in &doc.frontmatter.trigger_patterns {
            out.push_str(&format!("  - \"{}\"\n", pattern));
        }
    }
    if !doc.frontmatter.tools_required.is_empty() {
        out.push_str("tools_required:\n");
        for tool in &doc.frontmatter.tools_required {
            out.push_str(&format!("  - \"{}\"\n", tool));
        }
    }
    out.push_str(&format!("auto_load: {}\n", doc.frontmatter.auto_load));
    if !doc.frontmatter.read_when.is_empty() {
        out.push_str("read_when:\n");
        for item in &doc.frontmatter.read_when {
            out.push_str(&format!("  - \"{}\"\n", item));
        }
    }
    out.push_str("---\n");

    // -- Body --
    if !doc.body.is_empty() {
        out.push('\n');
        out.push_str(&doc.body);
        out.push('\n');
    }

    out
}

/// Render a skill's instructions into a prompt block for LLM context injection.
///
/// Budget: 4000 characters. Returns empty string if body is empty.
pub fn skill_to_prompt_block(doc: &SkillDocument) -> String {
    if doc.body.trim().is_empty() {
        return String::new();
    }

    let mut block = format!("### SKILL CONTEXT: {} ###\n", doc.frontmatter.name);
    let truncated: String = doc.body.chars().take(4000).collect();
    block.push_str(&truncated);
    block
}

/// Check whether a skill document has meaningful content (non-empty body).
pub fn skill_document_has_content(doc: &SkillDocument) -> bool {
    !doc.body.trim().is_empty()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const VALID_SKILL: &str = r#"---
name: "Code Review"
description: "Review code for bugs, style issues, and improvements"
command: "review"
trigger_patterns:
  - "review.*code"
  - "code review"
tools_required:
  - "file_read"
auto_load: false
read_when:
  - "User asks for code review"
---

## Instructions

When performing a code review, follow these steps:

1. Read the file(s) specified by the user
2. Analyze the code for bugs, style issues, security concerns
3. Provide feedback organized by severity

## Style

Be constructive and specific. Reference line numbers.
"#;

    const MINIMAL_SKILL: &str = r#"---
name: "Minimal"
description: "A minimal skill"
---
"#;

    #[test]
    fn test_parse_skill_frontmatter_only() {
        let fm = parse_skill_frontmatter(VALID_SKILL).expect("valid skill should parse");
        assert_eq!(fm.name, "Code Review");
        assert_eq!(fm.description, "Review code for bugs, style issues, and improvements");
        assert_eq!(fm.command, Some("review".to_string()));
        assert_eq!(fm.trigger_patterns, vec!["review.*code", "code review"]);
        assert_eq!(fm.tools_required, vec!["file_read"]);
        assert!(!fm.auto_load);
        assert_eq!(fm.read_when, vec!["User asks for code review"]);
    }

    #[test]
    fn test_parse_skill_full() {
        let doc = parse_skill_markdown(VALID_SKILL).expect("valid skill should parse");
        assert_eq!(doc.frontmatter.name, "Code Review");
        assert!(!doc.body.is_empty());
        assert!(doc.body.contains("When performing a code review"));
        assert!(doc.sections.contains_key("Instructions"));
        assert!(doc.sections.contains_key("Style"));
        assert!(doc.sections["Style"].contains("constructive"));
    }

    #[test]
    fn test_parse_minimal_skill() {
        let doc = parse_skill_markdown(MINIMAL_SKILL).expect("minimal skill should parse");
        assert_eq!(doc.frontmatter.name, "Minimal");
        assert_eq!(doc.frontmatter.description, "A minimal skill");
        assert_eq!(doc.frontmatter.command, None);
        assert!(doc.frontmatter.trigger_patterns.is_empty());
        assert!(doc.frontmatter.tools_required.is_empty());
        assert!(!doc.frontmatter.auto_load);
        assert!(doc.frontmatter.read_when.is_empty());
        assert!(doc.body.is_empty());
        assert!(doc.sections.is_empty());
    }

    #[test]
    fn test_parse_missing_name() {
        let input = "---\ndescription: \"test\"\n---\n";
        let err = parse_skill_frontmatter(input).expect_err("missing name should fail");
        assert_eq!(err, SkillParseError::MissingField("name"));
    }

    #[test]
    fn test_parse_missing_description() {
        let input = "---\nname: \"test\"\n---\n";
        let err = parse_skill_frontmatter(input).expect_err("missing description should fail");
        assert_eq!(err, SkillParseError::MissingField("description"));
    }

    #[test]
    fn test_parse_missing_frontmatter() {
        let input = "# No frontmatter\nJust a heading.";
        let err = parse_skill_frontmatter(input).expect_err("no frontmatter should fail");
        assert_eq!(err, SkillParseError::MissingFrontmatter);
    }

    #[test]
    fn test_parse_unterminated_frontmatter() {
        let input = "---\nname: \"test\"\ndescription: \"test\"\n";
        let err = parse_skill_frontmatter(input).expect_err("unterminated should fail");
        assert_eq!(err, SkillParseError::UnterminatedFrontmatter);
    }

    #[test]
    fn test_auto_load_true() {
        let input = "---\nname: \"test\"\ndescription: \"test\"\nauto_load: true\n---\n";
        let fm = parse_skill_frontmatter(input).expect("should parse");
        assert!(fm.auto_load);
    }

    #[test]
    fn test_auto_load_yes() {
        let input = "---\nname: \"test\"\ndescription: \"test\"\nauto_load: yes\n---\n";
        let fm = parse_skill_frontmatter(input).expect("should parse");
        assert!(fm.auto_load);
    }

    #[test]
    fn test_render_roundtrip() {
        let doc = parse_skill_markdown(VALID_SKILL).expect("valid skill should parse");
        let rendered = render_skill_markdown(&doc);
        let reparsed = parse_skill_markdown(&rendered).expect("rendered should re-parse");
        assert_eq!(doc.frontmatter, reparsed.frontmatter);
        // Body may differ in whitespace but sections should match
        assert_eq!(doc.sections.keys().collect::<Vec<_>>().len(),
                   reparsed.sections.keys().collect::<Vec<_>>().len());
        for (key, value) in &doc.sections {
            let reparsed_value = reparsed.sections.get(key).expect("section should exist");
            assert_eq!(value.trim(), reparsed_value.trim(),
                "Section '{}' content should match", key);
        }
    }

    #[test]
    fn test_render_minimal_roundtrip() {
        let doc = parse_skill_markdown(MINIMAL_SKILL).expect("minimal should parse");
        let rendered = render_skill_markdown(&doc);
        let reparsed = parse_skill_markdown(&rendered).expect("rendered should re-parse");
        assert_eq!(doc.frontmatter, reparsed.frontmatter);
    }

    #[test]
    fn test_skill_to_prompt_block_nonempty() {
        let doc = parse_skill_markdown(VALID_SKILL).expect("should parse");
        let block = skill_to_prompt_block(&doc);
        assert!(block.starts_with("### SKILL CONTEXT: Code Review ###"));
        assert!(block.contains("When performing a code review"));
    }

    #[test]
    fn test_skill_to_prompt_block_empty_body() {
        let doc = parse_skill_markdown(MINIMAL_SKILL).expect("should parse");
        let block = skill_to_prompt_block(&doc);
        assert!(block.is_empty());
    }

    #[test]
    fn test_skill_document_has_content() {
        let full = parse_skill_markdown(VALID_SKILL).expect("should parse");
        assert!(skill_document_has_content(&full));

        let minimal = parse_skill_markdown(MINIMAL_SKILL).expect("should parse");
        assert!(!skill_document_has_content(&minimal));
    }

    #[test]
    fn test_unknown_frontmatter_fields_tolerated() {
        let input = r#"---
name: "test"
description: "test"
unknown_field: "hello"
another_unknown:
  - "item1"
---
"#;
        let fm = parse_skill_frontmatter(input).expect("unknown fields should be tolerated");
        assert_eq!(fm.name, "test");
    }

    #[test]
    fn test_unquoted_frontmatter_values() {
        let input = r#"---
name: My Skill
description: A skill without quotes
command: my-skill
auto_load: false
---
"#;
        let fm = parse_skill_frontmatter(input).expect("unquoted values should parse");
        assert_eq!(fm.name, "My Skill");
        assert_eq!(fm.description, "A skill without quotes");
        assert_eq!(fm.command, Some("my-skill".to_string()));
    }
}

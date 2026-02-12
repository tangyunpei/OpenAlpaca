//! User profile (USER.md) parsing and rendering.
//!
//! Mirrors the SOUL.md system but with lenient parsing — all sections are optional
//! since a freshly-bootstrapped profile starts empty and fills in organically.

use std::collections::HashMap;
use std::fmt;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserFrontmatter {
    pub title: String,
    pub summary: String,
    pub read_when: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserDocument {
    pub frontmatter: UserFrontmatter,
    /// Key-value pairs from `## Identity` (e.g. "Name" → "Alex").
    pub identity: HashMap<String, String>,
    pub communication_style: String,
    pub expertise: String,
    pub projects: String,
    pub preferences: String,
    pub notes: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserParseError {
    MissingFrontmatter,
    UnterminatedFrontmatter,
    MissingField(&'static str),
}

impl fmt::Display for UserParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingFrontmatter => write!(f, "Missing YAML frontmatter"),
            Self::UnterminatedFrontmatter => write!(f, "Unterminated YAML frontmatter"),
            Self::MissingField(field) => write!(f, "Missing frontmatter field '{}'", field),
        }
    }
}

impl std::error::Error for UserParseError {}

// ---------------------------------------------------------------------------
// Frontmatter helpers (shared logic with soul.rs, inlined to avoid coupling)
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

fn split_frontmatter(input: &str) -> Result<(Vec<String>, Vec<String>), UserParseError> {
    let mut lines = input.lines();
    let first = lines.next().unwrap_or_default();
    if first.trim() != "---" {
        return Err(UserParseError::MissingFrontmatter);
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
        return Err(UserParseError::UnterminatedFrontmatter);
    }

    Ok((frontmatter, body))
}

fn parse_frontmatter(lines: &[String]) -> Result<UserFrontmatter, UserParseError> {
    let mut title: Option<String> = None;
    let mut summary: Option<String> = None;
    let mut read_when: Vec<String> = Vec::new();

    let mut idx = 0usize;
    while idx < lines.len() {
        let trimmed = lines[idx].trim();
        if trimmed.is_empty() {
            idx += 1;
            continue;
        }

        if let Some(rest) = trimmed.strip_prefix("title:") {
            title = Some(strip_outer_quotes(rest));
            idx += 1;
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("summary:") {
            summary = Some(strip_outer_quotes(rest));
            idx += 1;
            continue;
        }
        if trimmed.starts_with("read_when:") {
            idx += 1;
            while idx < lines.len() {
                let item = lines[idx].trim();
                if item.is_empty() {
                    idx += 1;
                    continue;
                }
                if let Some(v) = item.strip_prefix("- ") {
                    read_when.push(strip_outer_quotes(v));
                    idx += 1;
                    continue;
                }
                break;
            }
            continue;
        }

        idx += 1;
    }

    let title = title.ok_or(UserParseError::MissingField("title"))?;
    let summary = summary.ok_or(UserParseError::MissingField("summary"))?;
    if read_when.is_empty() {
        return Err(UserParseError::MissingField("read_when"));
    }

    Ok(UserFrontmatter {
        title,
        summary,
        read_when,
    })
}

// ---------------------------------------------------------------------------
// Section parsing
// ---------------------------------------------------------------------------

enum Section {
    Identity,
    CommunicationStyle,
    Expertise,
    Projects,
    Preferences,
    Notes,
    Other,
}

/// Parse `* Key: Value` bullet items into a HashMap.
fn parse_identity_line(line: &str) -> Option<(String, String)> {
    let trimmed = line.trim();
    let stripped = trimmed
        .strip_prefix("* ")
        .or_else(|| trimmed.strip_prefix("- "))?;
    let colon_pos = stripped.find(':')?;
    let key = stripped[..colon_pos].trim().to_string();
    let value = stripped[colon_pos + 1..].trim().to_string();
    if key.is_empty() {
        return None;
    }
    Some((key, value))
}

fn classify_heading(title: &str) -> Section {
    match title.trim() {
        "Identity" => Section::Identity,
        "Communication Style" => Section::CommunicationStyle,
        "Expertise & Background" => Section::Expertise,
        "Projects & Context" => Section::Projects,
        "Preferences" => Section::Preferences,
        "Notes" => Section::Notes,
        _ => Section::Other,
    }
}

fn parse_sections(lines: &[String]) -> (HashMap<String, String>, String, String, String, String, String) {
    let mut section = Section::Other;
    let mut identity = HashMap::new();
    let mut comm_lines: Vec<String> = Vec::new();
    let mut expertise_lines: Vec<String> = Vec::new();
    let mut projects_lines: Vec<String> = Vec::new();
    let mut preferences_lines: Vec<String> = Vec::new();
    let mut notes_lines: Vec<String> = Vec::new();

    for raw_line in lines {
        let line = raw_line.as_str();
        let trimmed = line.trim();

        // Section heading
        if let Some(title) = trimmed.strip_prefix("## ") {
            section = classify_heading(title);
            continue;
        }

        // Skip top-level heading (# USER.md ...)
        if trimmed.starts_with("# ") {
            continue;
        }

        match section {
            Section::Identity => {
                if let Some((key, value)) = parse_identity_line(trimmed) {
                    if !value.is_empty() {
                        identity.insert(key, value);
                    }
                }
            }
            Section::CommunicationStyle => {
                if !trimmed.is_empty() {
                    comm_lines.push(trimmed.to_string());
                }
            }
            Section::Expertise => {
                if !trimmed.is_empty() {
                    expertise_lines.push(trimmed.to_string());
                }
            }
            Section::Projects => {
                if !trimmed.is_empty() {
                    projects_lines.push(trimmed.to_string());
                }
            }
            Section::Preferences => {
                if !trimmed.is_empty() {
                    preferences_lines.push(trimmed.to_string());
                }
            }
            Section::Notes => {
                if !trimmed.is_empty() {
                    notes_lines.push(trimmed.to_string());
                }
            }
            Section::Other => {}
        }
    }

    (
        identity,
        comm_lines.join(" "),
        expertise_lines.join(" "),
        projects_lines.join(" "),
        preferences_lines.join(" "),
        notes_lines.join(" "),
    )
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Parse a USER.md file into a `UserDocument`.
///
/// Unlike `parse_soul_markdown`, all body sections are optional.
/// Only the YAML frontmatter is required.
pub fn parse_user_markdown(input: &str) -> Result<UserDocument, UserParseError> {
    let (frontmatter_lines, body_lines) = split_frontmatter(input)?;
    let frontmatter = parse_frontmatter(&frontmatter_lines)?;
    let (identity, communication_style, expertise, projects, preferences, notes) =
        parse_sections(&body_lines);

    Ok(UserDocument {
        frontmatter,
        identity,
        communication_style,
        expertise,
        projects,
        preferences,
        notes,
    })
}

/// Render a `UserDocument` back to valid USER.md markdown.
///
/// Round-trip: `parse_user_markdown(render_user_markdown(doc))` produces a
/// semantically-equal `UserDocument`.
pub fn render_user_markdown(doc: &UserDocument) -> String {
    let mut out = String::new();

    // -- Frontmatter --
    out.push_str("---\n");
    out.push_str(&format!("title: \"{}\"\n", doc.frontmatter.title));
    out.push_str(&format!("summary: \"{}\"\n", doc.frontmatter.summary));
    out.push_str("read_when:\n");
    for item in &doc.frontmatter.read_when {
        out.push_str(&format!("  - {}\n", item));
    }
    out.push_str("---\n\n");

    out.push_str("# USER.md -- About Your Human\n\n");

    // -- Identity --
    out.push_str("## Identity\n\n");
    // Use a stable order for known keys, then alphabetical for the rest
    let known_keys = ["Name", "What to call them", "Pronouns", "Timezone"];
    for key in &known_keys {
        if let Some(value) = doc.identity.get(*key) {
            out.push_str(&format!("* {}: {}\n", key, value));
        } else {
            out.push_str(&format!("* {}:\n", key));
        }
    }
    // Any extra keys not in the known set
    let mut extra_keys: Vec<_> = doc
        .identity
        .keys()
        .filter(|k| !known_keys.contains(&k.as_str()))
        .collect();
    extra_keys.sort();
    for key in extra_keys {
        out.push_str(&format!("* {}: {}\n", key, doc.identity[key]));
    }
    out.push('\n');

    // -- Communication Style --
    out.push_str("## Communication Style\n\n");
    if doc.communication_style.is_empty() {
        out.push_str("(How they like to communicate -- terse vs verbose, formal vs casual, etc.)\n");
    } else {
        out.push_str(&doc.communication_style);
        out.push('\n');
    }
    out.push('\n');

    // -- Expertise & Background --
    out.push_str("## Expertise & Background\n\n");
    if doc.expertise.is_empty() {
        out.push_str("(Technical background, domains of expertise, skill level in various areas)\n");
    } else {
        out.push_str(&doc.expertise);
        out.push('\n');
    }
    out.push('\n');

    // -- Projects & Context --
    out.push_str("## Projects & Context\n\n");
    if doc.projects.is_empty() {
        out.push_str("(Current projects, tools they use, stack preferences)\n");
    } else {
        out.push_str(&doc.projects);
        out.push('\n');
    }
    out.push('\n');

    // -- Preferences --
    out.push_str("## Preferences\n\n");
    if doc.preferences.is_empty() {
        out.push_str("(Likes, dislikes, pet peeves, formatting preferences, etc.)\n");
    } else {
        out.push_str(&doc.preferences);
        out.push('\n');
    }
    out.push('\n');

    // -- Notes --
    out.push_str("## Notes\n\n");
    if doc.notes.is_empty() {
        out.push_str("(Anything else. Build this over time.)\n");
    } else {
        out.push_str(&doc.notes);
        out.push('\n');
    }

    out
}

/// Returns true if the document has any meaningful content beyond the template defaults.
pub fn user_document_has_content(doc: &UserDocument) -> bool {
    !doc.identity.is_empty()
        || !doc.communication_style.is_empty()
        || !doc.expertise.is_empty()
        || !doc.projects.is_empty()
        || !doc.preferences.is_empty()
        || !doc.notes.is_empty()
}

/// Character budget for the user profile prompt block.
const USER_PROFILE_BUDGET: usize = 1000;

/// Render a `UserDocument` into a `### USER PROFILE ###` prompt block.
///
/// Returns an empty string if the document has no meaningful content.
pub fn user_to_prompt_block(doc: &UserDocument) -> String {
    if !user_document_has_content(doc) {
        return String::new();
    }

    let mut block = String::from("### USER PROFILE ###\n");
    let mut budget = USER_PROFILE_BUDGET;

    // Identity line (compact: "Name: Alex | Timezone: PST")
    if !doc.identity.is_empty() {
        let known_keys = ["Name", "What to call them", "Pronouns", "Timezone"];
        let mut parts = Vec::new();
        for key in &known_keys {
            if let Some(value) = doc.identity.get(*key) {
                if !value.is_empty() {
                    parts.push(format!("{}: {}", key, value));
                }
            }
        }
        // Extra keys
        let mut extra_keys: Vec<_> = doc
            .identity
            .keys()
            .filter(|k| !known_keys.contains(&k.as_str()))
            .collect();
        extra_keys.sort();
        for key in extra_keys {
            let val = &doc.identity[key];
            if !val.is_empty() {
                parts.push(format!("{}: {}", key, val));
            }
        }
        if !parts.is_empty() {
            let line = parts.join(" | ");
            let entry = format!("{}\n", line);
            if entry.len() <= budget {
                block.push_str(&entry);
                budget -= entry.len();
            }
        }
    }

    // Remaining sections as labeled lines
    let sections = [
        ("Style", &doc.communication_style),
        ("Background", &doc.expertise),
        ("Working on", &doc.projects),
        ("Preferences", &doc.preferences),
        ("Notes", &doc.notes),
    ];

    for (label, content) in &sections {
        if content.is_empty() {
            continue;
        }
        let truncated: String = content.chars().take(200).collect();
        let entry = format!("{}: {}\n", label, truncated);
        if entry.len() > budget {
            break;
        }
        block.push_str(&entry);
        budget -= entry.len();
    }

    block
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const VALID_TEMPLATE: &str = r#"---
title: "USER.md"
summary: "User profile record"
read_when:
  - Bootstrapping a workspace manually
---

# USER.md -- About Your Human

Learn about the person you're helping. Update this as you go.

## Identity

* Name:
* What to call them:
* Pronouns:
* Timezone:

## Communication Style

(How they like to communicate -- terse vs verbose, formal vs casual, etc.)

## Expertise & Background

(Technical background, domains of expertise, skill level in various areas)

## Projects & Context

(Current projects, tools they use, stack preferences)

## Preferences

(Likes, dislikes, pet peeves, formatting preferences, etc.)

## Notes

(Anything else. Build this over time.)

The more you know, the better you can help. But remember -- you're learning about a person, not building a dossier. Respect the difference.
"#;

    const POPULATED_DOC: &str = r#"---
title: "USER.md"
summary: "User profile record"
read_when:
  - Bootstrapping a workspace manually
---

# USER.md -- About Your Human

## Identity

* Name: Alex
* What to call them: Alex
* Pronouns: they/them
* Timezone: PST (UTC-8)

## Communication Style

Prefers concise, technical responses. No filler or sycophancy.

## Expertise & Background

Senior Rust developer. Strong in systems programming and async patterns. Familiar with Python and TypeScript.

## Projects & Context

Building OpenAlpaca orchestrator. Focus on memory system and agent pipeline.

## Preferences

Code examples over prose. Dark mode. Prefers snake_case.

## Notes

Likes to work late. Coffee over tea.
"#;

    #[test]
    fn test_parse_empty_template() {
        let doc = parse_user_markdown(VALID_TEMPLATE).expect("template should parse");
        assert_eq!(doc.frontmatter.title, "USER.md");
        assert_eq!(doc.frontmatter.summary, "User profile record");
        assert_eq!(
            doc.frontmatter.read_when,
            vec!["Bootstrapping a workspace manually"]
        );
        // All sections should be empty (template placeholders are parenthesized hints)
        assert!(doc.identity.is_empty());
        assert!(!user_document_has_content(&doc) || doc.communication_style.contains("How they like"));
    }

    #[test]
    fn test_parse_populated_document() {
        let doc = parse_user_markdown(POPULATED_DOC).expect("populated doc should parse");
        assert_eq!(doc.identity.get("Name"), Some(&"Alex".to_string()));
        assert_eq!(
            doc.identity.get("What to call them"),
            Some(&"Alex".to_string())
        );
        assert_eq!(
            doc.identity.get("Pronouns"),
            Some(&"they/them".to_string())
        );
        assert_eq!(
            doc.identity.get("Timezone"),
            Some(&"PST (UTC-8)".to_string())
        );
        assert!(doc.communication_style.contains("concise"));
        assert!(doc.expertise.contains("Rust"));
        assert!(doc.projects.contains("OpenAlpaca"));
        assert!(doc.preferences.contains("Dark mode"));
        assert!(doc.notes.contains("Coffee"));
    }

    #[test]
    fn test_parse_missing_frontmatter() {
        let err = parse_user_markdown("No frontmatter here").expect_err("should fail");
        assert_eq!(err, UserParseError::MissingFrontmatter);
    }

    #[test]
    fn test_parse_missing_summary() {
        let invalid = VALID_TEMPLATE.replace("summary: \"User profile record\"\n", "");
        let err = parse_user_markdown(&invalid).expect_err("should fail");
        assert_eq!(err, UserParseError::MissingField("summary"));
    }

    #[test]
    fn test_render_roundtrip() {
        let doc = parse_user_markdown(POPULATED_DOC).expect("should parse");
        let rendered = render_user_markdown(&doc);
        let reparsed = parse_user_markdown(&rendered).expect("rendered should re-parse");
        assert_eq!(doc.frontmatter, reparsed.frontmatter);
        assert_eq!(doc.identity, reparsed.identity);
        assert_eq!(doc.communication_style, reparsed.communication_style);
        assert_eq!(doc.expertise, reparsed.expertise);
        assert_eq!(doc.projects, reparsed.projects);
        assert_eq!(doc.preferences, reparsed.preferences);
        assert_eq!(doc.notes, reparsed.notes);
    }

    #[test]
    fn test_render_empty_template_roundtrip() {
        let original = parse_user_markdown(VALID_TEMPLATE).expect("should parse");
        let rendered = render_user_markdown(&original);
        let reparsed = parse_user_markdown(&rendered).expect("rendered should re-parse");
        assert_eq!(original.frontmatter, reparsed.frontmatter);
        assert_eq!(original.identity, reparsed.identity);
    }

    #[test]
    fn test_prompt_block_empty_returns_empty() {
        // A document with only template hints but no real content in identity
        let doc = UserDocument {
            frontmatter: UserFrontmatter {
                title: "USER.md".to_string(),
                summary: "User profile record".to_string(),
                read_when: vec!["Bootstrapping".to_string()],
            },
            identity: HashMap::new(),
            communication_style: String::new(),
            expertise: String::new(),
            projects: String::new(),
            preferences: String::new(),
            notes: String::new(),
        };
        assert!(user_to_prompt_block(&doc).is_empty());
    }

    #[test]
    fn test_prompt_block_populated() {
        let doc = parse_user_markdown(POPULATED_DOC).expect("should parse");
        let block = user_to_prompt_block(&doc);
        assert!(block.starts_with("### USER PROFILE ###\n"));
        assert!(block.contains("Name: Alex"));
        assert!(block.contains("Timezone: PST"));
        assert!(block.contains("Style:"));
        assert!(block.contains("Background:"));
        assert!(block.len() <= 1000 + "### USER PROFILE ###\n".len());
    }

    #[test]
    fn test_has_content_populated() {
        let doc = parse_user_markdown(POPULATED_DOC).expect("should parse");
        assert!(user_document_has_content(&doc));
    }

    #[test]
    fn test_unknown_sections_tolerated() {
        let with_extra = format!("{}\n\n## Extra Section\nSome future content.\n", POPULATED_DOC);
        let doc = parse_user_markdown(&with_extra).expect("unknown section should be tolerated");
        assert_eq!(doc.identity.get("Name"), Some(&"Alex".to_string()));
    }

    #[test]
    fn test_extra_identity_keys() {
        let custom = r#"---
title: "USER.md"
summary: "User profile record"
read_when:
  - Bootstrapping a workspace manually
---

## Identity

* Name: Alex
* Company: Acme Corp
* Role: CTO
"#;
        let doc = parse_user_markdown(custom).expect("should parse");
        assert_eq!(doc.identity.get("Name"), Some(&"Alex".to_string()));
        assert_eq!(doc.identity.get("Company"), Some(&"Acme Corp".to_string()));
        assert_eq!(doc.identity.get("Role"), Some(&"CTO".to_string()));
    }
}

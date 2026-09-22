//! Agent identity (IDENTITY.md) parsing and rendering.
//!
//! Mirrors the USER.md system with lenient parsing — all body fields are optional
//! since a freshly-bootstrapped identity starts with placeholder values that the
//! agent fills in during its first conversation.

use std::fmt;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityFrontmatter {
    pub summary: String,
    pub read_when: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityDocument {
    pub frontmatter: IdentityFrontmatter,
    /// The orchestrator's chosen name.
    pub name: String,
    /// What kind of entity it is (AI, robot, familiar, etc.).
    pub creature: String,
    /// How it comes across (sharp, warm, chaotic, calm, etc.).
    pub vibe: String,
    /// Signature emoji.
    pub emoji: String,
    /// Avatar path (workspace-relative), URL, or data URI.
    pub avatar: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentityParseError {
    MissingFrontmatter,
    UnterminatedFrontmatter,
    MissingField(&'static str),
}

impl fmt::Display for IdentityParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingFrontmatter => write!(f, "Missing YAML frontmatter"),
            Self::UnterminatedFrontmatter => write!(f, "Unterminated YAML frontmatter"),
            Self::MissingField(field) => write!(f, "Missing frontmatter field '{}'", field),
        }
    }
}

impl std::error::Error for IdentityParseError {}

// ---------------------------------------------------------------------------
// Document-specific frontmatter requirements
// ---------------------------------------------------------------------------

fn parse_frontmatter(lines: &[String]) -> Result<IdentityFrontmatter, IdentityParseError> {
    let crate::utils::markdown::PersonaFrontmatter {
        summary, read_when, ..
    } = crate::utils::markdown::scan_persona_frontmatter(lines);

    let summary = summary.ok_or(IdentityParseError::MissingField("summary"))?;
    if read_when.is_empty() {
        return Err(IdentityParseError::MissingField("read_when"));
    }

    Ok(IdentityFrontmatter { summary, read_when })
}

// ---------------------------------------------------------------------------
// Body parsing
// ---------------------------------------------------------------------------

/// Parse a `- **Key:** Value` or `- Key: Value` or `* Key: Value` bullet item.
///
/// Handles markdown bold (`**Key:**`) by stripping the `**` markers.
fn parse_identity_field(line: &str) -> Option<(String, String)> {
    let trimmed = line.trim();

    // Strip bullet prefix: "- " or "* "
    let stripped = trimmed
        .strip_prefix("- ")
        .or_else(|| trimmed.strip_prefix("* "))?;

    // Strip optional markdown bold around key: "**Name:**" → "Name:"
    let cleaned = if stripped.starts_with("**") {
        stripped.replace("**", "")
    } else {
        stripped.to_string()
    };

    let colon_pos = cleaned.find(':')?;
    let key = cleaned[..colon_pos].trim().to_string();
    let value = cleaned[colon_pos + 1..].trim().to_string();

    if key.is_empty() {
        return None;
    }

    // Filter out template placeholders like "_(pick something you like)_"
    if value.starts_with("_(") && (value.ends_with(')') || value.ends_with(")_")) {
        return Some((key, String::new()));
    }

    Some((key, value))
}

fn parse_body_fields(lines: &[String]) -> (String, String, String, String, String) {
    let mut name = String::new();
    let mut creature = String::new();
    let mut vibe = String::new();
    let mut emoji = String::new();
    let mut avatar = String::new();

    for raw_line in lines {
        let line = raw_line.as_str();
        let trimmed = line.trim();

        // Skip headings
        if trimmed.starts_with('#') {
            continue;
        }

        // Skip horizontal rules
        if trimmed == "---" {
            continue;
        }

        // Skip italic hints like "_Fill this in..._"
        if trimmed.starts_with('_') && trimmed.ends_with('_') && !trimmed.contains(':') {
            continue;
        }

        if let Some((key, value)) = parse_identity_field(trimmed) {
            match key.to_lowercase().as_str() {
                "name" => name = value,
                "creature" => creature = value,
                "vibe" => vibe = value,
                "emoji" => emoji = value,
                "avatar" => avatar = value,
                _ => {} // Unknown fields tolerated
            }
        }
    }

    (name, creature, vibe, emoji, avatar)
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Parse an IDENTITY.md file into an `IdentityDocument`.
///
/// All body fields are optional (empty string if not present).
/// Only the YAML frontmatter is required.
pub fn parse_identity_markdown(input: &str) -> Result<IdentityDocument, IdentityParseError> {
    let (frontmatter_lines, body_lines) = crate::utils::markdown::split_frontmatter(input)
        .map_err(|error| match error {
            crate::utils::markdown::FrontmatterError::MissingFrontmatter => {
                IdentityParseError::MissingFrontmatter
            }
            crate::utils::markdown::FrontmatterError::UnterminatedFrontmatter => {
                IdentityParseError::UnterminatedFrontmatter
            }
        })?;
    let frontmatter = parse_frontmatter(&frontmatter_lines)?;
    let (name, creature, vibe, emoji, avatar) = parse_body_fields(&body_lines);

    Ok(IdentityDocument {
        frontmatter,
        name,
        creature,
        vibe,
        emoji,
        avatar,
    })
}

/// Render an `IdentityDocument` back to valid IDENTITY.md markdown.
///
/// Round-trip: `parse_identity_markdown(render_identity_markdown(doc))`
/// produces a semantically-equal `IdentityDocument`.
pub fn render_identity_markdown(doc: &IdentityDocument) -> String {
    let mut out = String::new();

    // -- Frontmatter --
    out.push_str("---\n");
    out.push_str(&format!("summary: \"{}\"\n", doc.frontmatter.summary));
    out.push_str("read_when:\n");
    for item in &doc.frontmatter.read_when {
        out.push_str(&format!("  - {}\n", item));
    }
    out.push_str("---\n\n");

    out.push_str("# IDENTITY.md - Who Am I?\n\n");

    // Render fields as plain bullet items (no markdown bold for clean round-trip)
    let fields = [
        ("Name", &doc.name),
        ("Creature", &doc.creature),
        ("Vibe", &doc.vibe),
        ("Emoji", &doc.emoji),
        ("Avatar", &doc.avatar),
    ];

    for (key, value) in &fields {
        if value.is_empty() {
            out.push_str(&format!("- {}:\n", key));
        } else {
            out.push_str(&format!("- {}: {}\n", key, value));
        }
    }

    out
}

/// Returns true if the document has any meaningful content beyond the template defaults.
pub fn identity_document_has_content(doc: &IdentityDocument) -> bool {
    !doc.name.is_empty()
        || !doc.creature.is_empty()
        || !doc.vibe.is_empty()
        || !doc.emoji.is_empty()
        || !doc.avatar.is_empty()
}

/// Strip markdown heading markers that could be used for prompt injection.
fn sanitize_prompt_field(value: &str) -> String {
    value
        .replace("###", "")
        .replace("## ", "")
        .replace("# ", "")
        .lines()
        .next()
        .unwrap_or("")
        .to_string()
}

/// Default character budget for the identity prompt block.
const IDENTITY_PROMPT_BUDGET: usize = 300;

/// Render an `IdentityDocument` into a `### AGENT IDENTITY ###` prompt block.
///
/// Returns an empty string if the document has no meaningful content.
/// Format: `Name: Koda | Creature: familiar | Vibe: sharp | Emoji: 🦙`
///
/// The `budget` parameter controls the maximum character length of the identity
/// line. Pass `None` to use the compiled default (300 chars).
pub fn identity_to_prompt_block(doc: &IdentityDocument, budget: Option<usize>) -> String {
    let budget = budget.unwrap_or(IDENTITY_PROMPT_BUDGET);

    if !identity_document_has_content(doc) {
        return String::new();
    }

    let mut block = String::from("### AGENT IDENTITY ###\n");

    let fields = [
        ("Name", &doc.name),
        ("Creature", &doc.creature),
        ("Vibe", &doc.vibe),
        ("Emoji", &doc.emoji),
        ("Avatar", &doc.avatar),
    ];

    let parts: Vec<String> = fields
        .iter()
        .filter(|(_, v)| !v.is_empty())
        .map(|(k, v)| format!("{}: {}", k, sanitize_prompt_field(v)))
        .collect();

    if parts.is_empty() {
        return String::new();
    }

    let line = parts.join(" | ");
    let truncated: String = line.chars().take(budget).collect();
    block.push_str(&truncated);
    block.push('\n');

    block
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;

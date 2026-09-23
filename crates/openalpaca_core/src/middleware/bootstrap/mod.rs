//! First-run onboarding (BOOTSTRAP.md) parsing and prompt injection.
//!
//! Unlike IDENTITY.md / USER.md / SOUL.md, BOOTSTRAP.md is **temporary**: it
//! exists only after a fresh install and is deleted once the agent has populated
//! both its identity and the user profile.  The body is free-form markdown that
//! gets injected verbatim into the system prompt as onboarding instructions.

use super::persona_frontmatter;
use std::fmt;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootstrapFrontmatter {
    pub summary: String,
    pub read_when: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootstrapDocument {
    pub frontmatter: BootstrapFrontmatter,
    /// Raw markdown body — injected as-is into the system prompt.
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BootstrapParseError {
    MissingFrontmatter,
    UnterminatedFrontmatter,
    MissingField(&'static str),
}

impl fmt::Display for BootstrapParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingFrontmatter => write!(f, "Missing YAML frontmatter"),
            Self::UnterminatedFrontmatter => write!(f, "Unterminated YAML frontmatter"),
            Self::MissingField(field) => write!(f, "Missing frontmatter field '{}'", field),
        }
    }
}

impl std::error::Error for BootstrapParseError {}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Parse a BOOTSTRAP.md file into a `BootstrapDocument`.
///
/// The body is preserved verbatim (free-form markdown).
/// Only the YAML frontmatter (`summary` and `read_when`) is required.
pub fn parse_bootstrap_markdown(input: &str) -> Result<BootstrapDocument, BootstrapParseError> {
    let (fm, body_lines) = persona_frontmatter::parse(
        input,
        BootstrapParseError::MissingFrontmatter,
        BootstrapParseError::UnterminatedFrontmatter,
    )?;
    let summary = fm
        .summary
        .ok_or(BootstrapParseError::MissingField("summary"))?;
    if fm.read_when.is_empty() {
        return Err(BootstrapParseError::MissingField("read_when"));
    }
    let frontmatter = BootstrapFrontmatter {
        summary,
        read_when: fm.read_when,
    };

    // Join body lines preserving original formatting.
    // Trim leading/trailing blank lines but keep interior formatting intact.
    let body = body_lines.join("\n");
    let body = body.trim().to_string();

    Ok(BootstrapDocument { frontmatter, body })
}

/// Returns true if the document has any meaningful body content.
pub fn bootstrap_document_has_content(doc: &BootstrapDocument) -> bool {
    !doc.body.trim().is_empty()
}

/// Render a `BootstrapDocument` into a `### BOOTSTRAP ###` prompt block.
///
/// Returns an empty string if the body is blank.
pub fn bootstrap_to_prompt_block(doc: &BootstrapDocument) -> String {
    if !bootstrap_document_has_content(doc) {
        return String::new();
    }

    let mut block = String::from("### BOOTSTRAP ###\n");
    block.push_str(&doc.body);
    block.push('\n');
    block
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;

//! First-run onboarding (BOOTSTRAP.md) parsing and prompt injection.
//!
//! Unlike IDENTITY.md / USER.md / SOUL.md, BOOTSTRAP.md is **temporary**: it
//! exists only after a fresh install and is deleted once the agent has populated
//! both its identity and the user profile.  The body is free-form markdown that
//! gets injected verbatim into the system prompt as onboarding instructions.

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
// Frontmatter helpers (inlined from identity.rs / soul.rs to avoid coupling)
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

fn split_frontmatter(input: &str) -> Result<(Vec<String>, Vec<String>), BootstrapParseError> {
    let mut lines = input.lines();
    let first = lines.next().unwrap_or_default();
    if first.trim() != "---" {
        return Err(BootstrapParseError::MissingFrontmatter);
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
        return Err(BootstrapParseError::UnterminatedFrontmatter);
    }

    Ok((frontmatter, body))
}

fn parse_frontmatter(lines: &[String]) -> Result<BootstrapFrontmatter, BootstrapParseError> {
    let mut summary: Option<String> = None;
    let mut read_when: Vec<String> = Vec::new();

    let mut idx = 0usize;
    while idx < lines.len() {
        let trimmed = lines[idx].trim();
        if trimmed.is_empty() {
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

    let summary = summary.ok_or(BootstrapParseError::MissingField("summary"))?;
    if read_when.is_empty() {
        return Err(BootstrapParseError::MissingField("read_when"));
    }

    Ok(BootstrapFrontmatter { summary, read_when })
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Parse a BOOTSTRAP.md file into a `BootstrapDocument`.
///
/// The body is preserved verbatim (free-form markdown).
/// Only the YAML frontmatter (`summary` and `read_when`) is required.
pub fn parse_bootstrap_markdown(input: &str) -> Result<BootstrapDocument, BootstrapParseError> {
    let (frontmatter_lines, body_lines) = split_frontmatter(input)?;
    let frontmatter = parse_frontmatter(&frontmatter_lines)?;

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

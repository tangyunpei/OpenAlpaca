//! SKILL.md parser, renderer, and prompt block formatter.
//!
//! A Skill is a folder on disk containing a `SKILL.md` file with YAML frontmatter
//! and a markdown body. The frontmatter provides lightweight metadata for catalog
//! discovery (Level 1), while the full body provides instructions loaded on demand
//! (Level 2).
//!
//! Parsing is **lenient**: only `name` and `description` are required in the
//! frontmatter. All body sections are optional.

mod parser;
pub mod renderer;
pub mod types;

pub use renderer::{render_skill_markdown, skill_document_has_content, skill_to_prompt_block};
pub use types::*;

use std::fmt;

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
            Self::MissingField(field) => {
                write!(f, "Missing required frontmatter field '{}'", field)
            }
            Self::InvalidYaml(msg) => write!(f, "Invalid YAML: {}", msg),
        }
    }
}

impl std::error::Error for SkillParseError {}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Parse only the YAML frontmatter from a SKILL.md file (Level 1).
///
/// Use this for lightweight catalog scanning at startup — does NOT parse the body.
pub fn parse_skill_frontmatter(input: &str) -> Result<SkillFrontmatter, SkillParseError> {
    let (yaml_str, _body_lines) = parser::extract_frontmatter_str(input)?;
    let mut fm: SkillFrontmatter =
        serde_yaml::from_str(yaml_str).map_err(|e| SkillParseError::InvalidYaml(e.to_string()))?;
    fm.apply_legacy_compat();
    fm.validate()?;
    Ok(fm)
}

/// Parse the full SKILL.md file including body sections (Level 2).
pub fn parse_skill_markdown(input: &str) -> Result<SkillDocument, SkillParseError> {
    let (yaml_str, body_lines) = parser::extract_frontmatter_str(input)?;
    let mut frontmatter: SkillFrontmatter =
        serde_yaml::from_str(yaml_str).map_err(|e| SkillParseError::InvalidYaml(e.to_string()))?;
    frontmatter.apply_legacy_compat();
    frontmatter.validate()?;
    let (body, sections) = parser::parse_body_sections(&body_lines);

    Ok(SkillDocument {
        frontmatter,
        body,
        sections,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;

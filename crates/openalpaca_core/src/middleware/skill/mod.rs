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
    // What the catalog's scan meets beside a parse (its diagnostics,
    // N-SKILL-2): the first two skip the skill, the last five load it anyway.
    ReadFailed(String),
    TooLarge {
        bytes: u64,
        limit: u64,
    },
    IdMismatch {
        declared: String,
        directory: String,
    },
    DeprecatedField {
        field: &'static str,
        note: &'static str,
    },
    SlashConflict {
        command: String,
        other: String,
    },
    UnknownKey(String),
    NoTools,
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
            Self::ReadFailed(why) => write!(f, "Could not read SKILL.md: {why}"),
            Self::TooLarge { bytes, limit } => {
                write!(f, "SKILL.md is {bytes} bytes, over the {limit}-byte limit")
            }
            Self::IdMismatch {
                declared,
                directory,
            } => write!(
                f,
                "Frontmatter id '{declared}' does not match directory name '{directory}'"
            ),
            Self::DeprecatedField { field, note } => {
                write!(f, "'{field}' is deprecated and has no effect. {note}")
            }
            Self::SlashConflict { command, other } => write!(
                f,
                "Slash command '/{command}' is also claimed by skill '{other}'; this skill now answers it"
            ),
            Self::UnknownKey(key) => write!(f, "Unknown frontmatter key '{key}' is ignored"),
            Self::NoTools => write!(
                f,
                "The skill declares no tools: 'requires_capabilities', 'tools.allow', 'scripts' and 'depends_on' are all empty, so it runs with none"
            ),
        }
    }
}

impl std::error::Error for SkillParseError {}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// How SKILL.md frontmatter is read: serde-saphyr, set as close to how the
/// serde_yaml 0.9 it replaced read a file as its options go. `yes`/`no`/`on`/
/// `off`/`y`/`n` stay strings (a `parameters` schema goes to the model
/// verbatim), a `<<` key is an ordinary key rather than a merge, a repeated key
/// keeps its last value, a run of comments is read the way serde_yaml read it
/// (serde-saphyr's default refuses 33 or more comment lines between a key and
/// its block value), a `!!binary` value or a `.inf` in a string field stays the
/// text it was written as, and an error is one line with no excerpt of the
/// file. Where it differs: in a `serde_json::Value` position (`tools.defaults`,
/// a `parameters` schema, a context source) a non-finite float becomes its
/// canonical spelling as a string (`.inf`, `-.inf`, `.nan`; `1e999` reads
/// `.inf`), where serde_yaml gave null (and kept `1e999` as written).
fn yaml_options() -> serde_saphyr::Options {
    serde_saphyr::options! {
        strict_booleans: true,
        merge_keys: serde_saphyr::MergeKeyPolicy::AsOrdinary,
        duplicate_keys: serde_saphyr::DuplicateKeyPolicy::LastWins,
        non_finite_float_policy: serde_saphyr::NonFiniteFloatPolicy::AsString,
        ignore_binary_tag_for_string: true,
        with_snippet: false,
        emit_comments: false,
    }
}

fn parse_yaml(yaml_str: &str) -> Result<SkillFrontmatter, SkillParseError> {
    serde_saphyr::from_str_with_options(yaml_str, yaml_options())
        .map_err(|e| SkillParseError::InvalidYaml(e.to_string()))
}

/// Parse only the YAML frontmatter from a SKILL.md file (Level 1).
///
/// Use this for lightweight catalog scanning at startup — does NOT parse the body.
pub fn parse_skill_frontmatter(input: &str) -> Result<SkillFrontmatter, SkillParseError> {
    let (yaml_str, _body_lines) = parser::extract_frontmatter_str(input)?;
    let fm = parse_yaml(yaml_str)?;
    fm.validate()?;
    Ok(fm)
}

/// Parse the full SKILL.md file including body sections (Level 2).
pub fn parse_skill_markdown(input: &str) -> Result<SkillDocument, SkillParseError> {
    let (yaml_str, body_lines) = parser::extract_frontmatter_str(input)?;
    let frontmatter = parse_yaml(yaml_str)?;
    frontmatter.validate()?;
    let (body, sections) = super::body_sections::parse_body_sections(&body_lines);

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

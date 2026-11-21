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

/// Errors that can occur while parsing a SKILL.md file.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SkillParseError {
    #[error("Missing YAML frontmatter")]
    MissingFrontmatter,
    #[error("Unterminated YAML frontmatter")]
    UnterminatedFrontmatter,
    #[error("Missing required frontmatter field '{0}'")]
    MissingField(&'static str),
    #[error("Invalid YAML: {0}")]
    InvalidYaml(String),
    // What the catalog's scan meets beside a parse (its diagnostics,
    // N-SKILL-2): the first two skip the skill, the last six load it anyway.
    #[error("Could not read SKILL.md: {0}")]
    ReadFailed(String),
    #[error("SKILL.md is {bytes} bytes, over the {limit}-byte limit")]
    TooLarge { bytes: u64, limit: u64 },
    #[error("Frontmatter id '{declared}' does not match directory name '{directory}'")]
    IdMismatch { declared: String, directory: String },
    #[error("'{field}' is deprecated and has no effect. {note}")]
    DeprecatedField {
        field: &'static str,
        note: &'static str,
    },
    #[error(
        "Slash command '/{command}' is also claimed by skill '{other}'; this skill now answers it"
    )]
    SlashConflict { command: String, other: String },
    #[error("Unknown frontmatter key '{0}' is ignored")]
    UnknownKey(String),
    #[error(
        "The skill declares no tools: 'requires_capabilities', 'tools.allow', 'scripts' and 'depends_on' are all empty, so it runs with none"
    )]
    NoTools,
    /// A file of a folder that overrides a bundled skill is the version an
    /// earlier build shipped at its path, which the bundle has since changed:
    /// the override runs the old file (subagent design §2.8 W-2).
    #[error(
        "{file} is the version an earlier build shipped; the built-in {skill}'s differs, and this override runs the old file"
    )]
    StaleShippedFile {
        /// The file, as on disk.
        file: String,
        /// The bundled skill's key.
        skill: String,
    },
}

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

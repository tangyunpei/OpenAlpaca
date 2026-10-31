//! The catalog's scan diagnostics (N-SKILL-2, audit Z3-03 and Z3-10).
//!
//! A SKILL.md that cannot be read or parsed, that claims another skill's
//! slash command, or that carries a key this build ignores used to be skipped
//! or loaded with a log line only. The catalog now keeps what its last scan
//! found as [`SkillDiagnostic`]s — `{code, severity, path, skill_id?,
//! message}` — which `GET /v1/skills/diagnostics`, `openalpaca skills list|info`
//! and Settings → Tools show. The message is always [`SkillParseError`]'s
//! `Display` string, so a diagnostic reads the same wherever it is shown.
//!
//! The scan reads a SKILL.md only when it is a regular file — a link the owner
//! made to one is still followed — and never more than
//! [`MAX_SKILL_MD_BYTES`]: a FIFO, a device or a directory named `SKILL.md`
//! is reported `read_failed` instead of blocking the boot scan, and an
//! oversized file `too_large` instead of being read whole. An invocation's
//! read of the whole file (`SkillCatalog::load_full`) takes the same guards
//! and fails with the same words.

use crate::middleware::skill::SkillParseError;
use serde::Serialize;
use std::io::Read;
use std::path::Path;

/// The most bytes a SKILL.md may hold. Its instructions reach the prompt cut
/// to `context.budget_tokens * 4` characters (16,000 by default), so a file
/// sixteen times that is far past anything a skill needs.
pub const MAX_SKILL_MD_BYTES: u64 = 256 * 1024;

/// What a diagnostic is about. Serialized in snake case — the codes of record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillDiagnosticCode {
    /// The file could not be read, or is not a regular file.
    ReadFailed,
    /// No YAML frontmatter, or an unterminated one.
    MissingFrontmatter,
    /// The frontmatter is not YAML this build can read.
    InvalidYaml,
    /// `name` or `description` is missing or empty.
    MissingField,
    /// The frontmatter's `id` differs from the directory name, which is the id.
    IdMismatch,
    /// A field that is parsed but has no effect.
    DeprecatedField,
    /// The skill's slash command was another skill's; this one now answers it.
    SlashConflict,
    /// A top-level frontmatter key this build does not know, so ignores.
    UnknownKey,
    /// The file is larger than [`MAX_SKILL_MD_BYTES`].
    TooLarge,
    /// The skill declares no tools at all — no `requires_capabilities`, no
    /// `tools.allow`, no `scripts`, no `depends_on` — so it runs with none
    /// (audit Z3-04).
    NoTools,
}

impl SkillDiagnosticCode {
    /// The code of an error the scan met.
    pub fn of(error: &SkillParseError) -> Self {
        match error {
            SkillParseError::MissingFrontmatter | SkillParseError::UnterminatedFrontmatter => {
                Self::MissingFrontmatter
            }
            SkillParseError::MissingField(_) => Self::MissingField,
            SkillParseError::InvalidYaml(_) => Self::InvalidYaml,
            SkillParseError::ReadFailed(_) => Self::ReadFailed,
            SkillParseError::TooLarge { .. } => Self::TooLarge,
            SkillParseError::IdMismatch { .. } => Self::IdMismatch,
            SkillParseError::DeprecatedField { .. } => Self::DeprecatedField,
            SkillParseError::SlashConflict { .. } => Self::SlashConflict,
            SkillParseError::UnknownKey(_) => Self::UnknownKey,
            SkillParseError::NoTools => Self::NoTools,
        }
    }

    /// The code as it is written on the wire and in the CLI.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReadFailed => "read_failed",
            Self::MissingFrontmatter => "missing_frontmatter",
            Self::InvalidYaml => "invalid_yaml",
            Self::MissingField => "missing_field",
            Self::IdMismatch => "id_mismatch",
            Self::DeprecatedField => "deprecated_field",
            Self::SlashConflict => "slash_conflict",
            Self::UnknownKey => "unknown_key",
            Self::TooLarge => "too_large",
            Self::NoTools => "no_tools",
        }
    }

    /// An error is a file that was not loaded (at boot its skill is missing;
    /// after a hot edit the skill stays listed but does not run until the file
    /// is fixed — an invocation fails with the same error); a warning a
    /// problem in a file that was read anyway.
    pub fn severity(self) -> SkillDiagnosticSeverity {
        match self {
            Self::ReadFailed
            | Self::MissingFrontmatter
            | Self::InvalidYaml
            | Self::MissingField
            | Self::TooLarge => SkillDiagnosticSeverity::Error,
            Self::IdMismatch
            | Self::DeprecatedField
            | Self::SlashConflict
            | Self::UnknownKey
            | Self::NoTools => SkillDiagnosticSeverity::Warning,
        }
    }
}

/// Whether the file was not loaded (`error`) or read in spite of the problem
/// (`warning`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillDiagnosticSeverity {
    Error,
    Warning,
}

/// One thing the last scan found in one SKILL.md.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SkillDiagnostic {
    pub code: SkillDiagnosticCode,
    pub severity: SkillDiagnosticSeverity,
    /// The SKILL.md (or skills directory) the diagnostic is about.
    pub path: String,
    /// The skill's id — its directory name, lowercased — when there is one.
    pub skill_id: Option<String>,
    /// [`SkillParseError`]'s `Display` string.
    pub message: String,
}

impl SkillDiagnostic {
    /// The diagnostic for `error`, met in `path`, for skill `skill_id`.
    pub fn new(error: &SkillParseError, path: &Path, skill_id: Option<&str>) -> Self {
        let code = SkillDiagnosticCode::of(error);
        Self {
            code,
            severity: code.severity(),
            path: path.display().to_string(),
            skill_id: skill_id.map(str::to_string),
            message: error.to_string(),
        }
    }
}

/// Read a SKILL.md the way the scan and an invocation may: only a regular
/// file (a link to one is followed), only up to [`MAX_SKILL_MD_BYTES`], and
/// never by an open that could wait on a FIFO's writer.
pub(super) fn read_skill_md(path: &Path) -> Result<String, SkillParseError> {
    let read_failed = |e: std::io::Error| SkillParseError::ReadFailed(e.to_string());
    let not_regular = || SkillParseError::ReadFailed("not a regular file".to_string());
    let too_large = |bytes: u64| SkillParseError::TooLarge {
        bytes,
        limit: MAX_SKILL_MD_BYTES,
    };

    // `metadata` follows a link, so an owner's link to a file is read; the
    // check comes before any open, so a FIFO is never opened at all.
    let meta = std::fs::metadata(path).map_err(read_failed)?;
    if !meta.is_file() {
        return Err(not_regular());
    }
    if meta.len() > MAX_SKILL_MD_BYTES {
        return Err(too_large(meta.len()));
    }

    let file = open_without_waiting(path).map_err(read_failed)?;
    // The handle is checked again: the path may have been swapped between
    // the check and the open.
    let opened = file.metadata().map_err(read_failed)?;
    if !opened.is_file() {
        return Err(not_regular());
    }
    let mut bytes = Vec::new();
    file.take(MAX_SKILL_MD_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(read_failed)?;
    if bytes.len() as u64 > MAX_SKILL_MD_BYTES {
        return Err(too_large(opened.len().max(bytes.len() as u64)));
    }
    String::from_utf8(bytes)
        .map_err(|_| SkillParseError::ReadFailed("stream did not contain valid UTF-8".to_string()))
}

/// Open for reading. On unix the open does not wait: a FIFO swapped in after
/// the metadata check opens at once (and is then refused) instead of blocking
/// until a writer appears. `O_NONBLOCK` changes nothing for a regular file.
fn open_without_waiting(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    options.open(path)
}

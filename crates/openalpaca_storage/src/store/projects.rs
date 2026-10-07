//! A project's folder in the home store (storage design rev 0.8, Stage 1:
//! ST-1, ST-3…ST-6, ST-13; the lead's rulings R1…R13).
//!
//! `<home>/projects/<name>/` holds one project's content — `artifacts/`,
//! `uploads/`, `memory/` — beside its `project.json`, which records the
//! project's permanent id and its canonical root path. `projects/` is the
//! user's, like everything at the home root but `state/` (R1), and a folder
//! under it holds `project.json` and content directories, nothing else (R3).
//!
//! There is no registry object (§3): these are functions over the files under
//! the **current** home root, read on demand, so the daemon, the CLI and the
//! app's shell agree and a test's `OPENALPACA_HOME_STORE` holds. The file
//! decides whose folder it is, never the name (R6), and a name is never
//! parsed back into a path (R8).

use std::fmt;
use std::path::Path;
use std::str::FromStr;

use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;
use uuid::Uuid;

/// `<home>/projects` — one folder per project (ST-1). Not a
/// [`super::ContentKind`]: it exists only at the home root, and
/// `content_dir(scope, kind)` stays `root/<kind>`.
pub const PROJECTS_DIR: &str = "projects";

/// The file in a project's folder that says whose folder it is (R9).
pub const PROJECT_FILE: &str = "project.json";

/// The one `version` this build reads and writes; any other reads as
/// unreadable (R11).
pub const PROJECT_FILE_VERSION: u32 = 1;

/// R7: a plain name longer than this is cut at a character boundary and
/// always hashed.
pub const NAME_MAX_BYTES: usize = 200;

/// R6: `<plain>-<8 hex of sha256(the canonical path)>`.
pub const NAME_HASH_HEX: usize = 8;

// ============================================================================
// Identity (ST-13)
// ============================================================================

/// A project's permanent id (ST-13): a v4 UUID, written and read only in its
/// canonical hyphenated lowercase form — `NoteId::parse`'s rule
/// (`memory_files/note.rs`), copied rather than shared, so neither id can
/// drift into the other's meaning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ProjectId(Uuid);

impl ProjectId {
    /// A fresh v4 id.
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// Strict: only the canonical hyphenated lowercase spelling is an id.
    pub fn parse(s: &str) -> Result<Self, String> {
        let uuid = Uuid::parse_str(s).map_err(|e| format!("`{s}` is not a UUID: {e}"))?;
        if uuid.hyphenated().to_string() != s {
            return Err(format!(
                "`{s}` is not the canonical hyphenated lowercase form of a UUID"
            ));
        }
        Ok(Self(uuid))
    }

    pub fn as_uuid(&self) -> &Uuid {
        &self.0
    }
}

impl Default for ProjectId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for ProjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0.hyphenated())
    }
}

impl FromStr for ProjectId {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl serde::Serialize for ProjectId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> serde::Deserialize<'de> for ProjectId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        ProjectId::parse(&s).map_err(serde::de::Error::custom)
    }
}

// ============================================================================
// Folder names (R4…R8)
// ============================================================================

/// The folder name of a project root (R4, R5, R7). Pure; the caller passes
/// the **canonical** root (R4), and nothing ever parses a name back into a
/// path (R8).
///
/// The path is normalized to NFC; a character that is a Unicode letter,
/// digit or combining mark is kept — marks because a Thai or Devanagari word
/// is not whole without them — and every other character becomes one `-`,
/// runs not collapsed (R5): `/Users/me/dev/my_app` is `-Users-me-dev-my-app`.
/// A name longer than [`NAME_MAX_BYTES`] is cut at the last character
/// boundary at or below it and always given [`hashed_folder_name`]'s suffix
/// (R7).
pub fn folder_name(root: &Path) -> String {
    name_parts(root).0
}

/// The name a project takes when its plain name belongs to another folder
/// (R6): `<folder_name>-<the first NAME_HASH_HEX hex of sha256(root)>`, the
/// hash over the canonical path as the volume spells it, not its NFC form. A
/// name R7 already cut carries the suffix and is returned as it is.
pub fn hashed_folder_name(root: &Path) -> String {
    match name_parts(root) {
        (name, true) => name,
        (name, false) => format!("{name}-{}", path_hash(root)),
    }
}

/// The folder name, and whether R7 already gave it the hash suffix.
fn name_parts(root: &Path) -> (String, bool) {
    let written = root.to_string_lossy();
    let mut name: String = written
        .nfc()
        .map(|c| {
            if c.is_alphanumeric() || is_combining_mark(c) {
                c
            } else {
                '-'
            }
        })
        .collect();
    if name.len() <= NAME_MAX_BYTES {
        return (name, false);
    }
    let end = name.floor_char_boundary(NAME_MAX_BYTES);
    name.truncate(end);
    name.push('-');
    name.push_str(&path_hash(root));
    (name, true)
}

/// The first [`NAME_HASH_HEX`] hex characters of SHA-256 over the root as
/// written — truncated, never cut with a byte range.
fn path_hash(root: &Path) -> String {
    let mut hash = crate::memory_files::sha256_hex(root.to_string_lossy().as_bytes());
    hash.truncate(NAME_HASH_HEX);
    hash
}

#[cfg(test)]
mod tests;

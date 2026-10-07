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
use std::str::FromStr;

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

#[cfg(test)]
mod tests;

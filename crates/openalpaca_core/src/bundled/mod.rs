//! The bundled layer: the agent templates OpenAlpaca ships, carried in the
//! binary (subagent design §2.1 B-1, `tasks/subagent-brief-design.md`), and
//! the shipped-versions list that tells an earlier build's unedited copy
//! from a person's edit (§2.2).
//!
//! The templates are registered from these bytes at every start; nothing is
//! written into `config/agents/`. A file there overrides the bundled template
//! of its `id` — unless it is byte-identical to a version OpenAlpaca shipped
//! (an earlier build seeded it, or a person copied the repository's
//! `config/`), in which case it is set aside at load: not read, not touched,
//! listed as not used, and the bundled template runs (V-1).

use crate::agent::template::{AgentTemplate, parse_agent_markdown};
use openalpaca_storage::hash::sha256_hex;
use std::sync::LazyLock;

/// One embedded file: its path relative to `config/`, `/`-separated, and
/// its bytes as committed.
pub struct BundledFile {
    pub path: &'static str,
    pub contents: &'static str,
}

macro_rules! bundled {
    ($path:literal) => {
        BundledFile {
            path: $path,
            contents: include_str!(concat!("../../../../config/", $path)),
        }
    };
}

/// The nine shipped agent templates. The repository's `config/agents/` is
/// their source (B-2): editing one there changes the bundle at the next
/// build, and needs its new version appended to `shipped_versions.txt` (V-3).
pub const BUNDLED_AGENTS: &[BundledFile] = &[
    bundled!("agents/code_agent.md"),
    bundled!("agents/explore_agent.md"),
    bundled!("agents/general_agent.md"),
    bundled!("agents/lead_agent.md"),
    bundled!("agents/planning_agent.md"),
    bundled!("agents/research_agent.md"),
    bundled!("agents/review_agent.md"),
    bundled!("agents/system_agent.md"),
    bundled!("agents/writing_agent.md"),
];

/// Every version OpenAlpaca shipped of each bundled path, one line each:
/// `<sha256 hex> <path relative to config/>`, and a third field on the
/// results of the Installation Manual's own edits (V-2a). Append-only (V-3).
const SHIPPED_VERSIONS: &str = include_str!("shipped_versions.txt");

/// Which shipped version an unedited copy is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShippedVersion {
    /// The version this build carries.
    Current,
    /// A version an earlier build shipped.
    Earlier,
}

/// An edit the Installation Manual told people to make to a shipped copy
/// (V-2a): its result is still an unedited copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManualEdit {
    /// Step 7: `system_agent.md`'s two `require_confirmation_for:
    /// ["file_delete"]` lines deleted.
    Step7,
    /// Step 8: `writing_agent.md` given `- "file_read"` under
    /// `capabilities:`.
    Step8,
}

impl ManualEdit {
    /// The word the listings use: `manual_step_7`, `manual_step_8`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Step7 => "manual_step_7",
            Self::Step8 => "manual_step_8",
        }
    }
}

/// What a file byte-identical to a shipped version is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShippedCopy {
    pub version: ShippedVersion,
    pub edit: Option<ManualEdit>,
}

/// One line of the list.
pub(crate) struct ShippedLine {
    pub(crate) hash: &'static str,
    pub(crate) path: &'static str,
    pub(crate) edit: Option<ManualEdit>,
}

/// The list's lines, in file order. A line that does not read as one is
/// skipped (`every_bundled_file_is_a_shipped_version` and the floor test
/// read the list too, so a malformed line fails the gate).
pub(crate) fn shipped_lines() -> impl Iterator<Item = ShippedLine> {
    SHIPPED_VERSIONS.lines().filter_map(|line| {
        let mut fields = line.split(' ');
        let hash = fields.next()?;
        let path = fields.next()?;
        let edit = match fields.next() {
            None => None,
            Some("manual-step-7") => Some(ManualEdit::Step7),
            Some("manual-step-8") => Some(ManualEdit::Step8),
            Some(_) => return None,
        };
        (hash.len() == 64 && fields.next().is_none()).then_some(ShippedLine { hash, path, edit })
    })
}

/// The `sha256_hex` of each bundled template, the current versions.
static CURRENT_AGENT_HASHES: LazyLock<Vec<String>> = LazyLock::new(|| {
    BUNDLED_AGENTS
        .iter()
        .map(|file| sha256_hex(file.contents.as_bytes()))
        .collect()
});

/// The bundled templates, parsed. A bundled file that does not parse is left
/// out with an `ERROR` — `bundled::tests` refuses such a build.
pub fn bundled_templates() -> Vec<AgentTemplate> {
    BUNDLED_AGENTS
        .iter()
        .filter_map(|file| match parse_agent_markdown(file.contents) {
            Ok(template) => Some(template),
            Err(e) => {
                tracing::error!("The bundled template {} does not parse: {e}", file.path);
                None
            }
        })
        .collect()
}

/// Whether `bytes`, a top-level `.md` read from `<config>/agents/`, is a copy
/// of a template version OpenAlpaca shipped — matched by content alone,
/// whatever the file is called (V-4): a template's identity is the `id:`
/// inside it. None when it is not, any byte differing: the rule errs only
/// towards "the person's file". Whether the copy is set aside is the
/// caller's (V-6: only one that would hide a bundled template).
pub fn classify_agent_file(bytes: &[u8]) -> Option<ShippedCopy> {
    let hash = sha256_hex(bytes);
    if CURRENT_AGENT_HASHES.contains(&hash) {
        return Some(ShippedCopy {
            version: ShippedVersion::Current,
            edit: None,
        });
    }
    shipped_lines()
        .find(|line| line.hash == hash && line.path.starts_with("agents/"))
        .map(|line| ShippedCopy {
            version: ShippedVersion::Earlier,
            edit: line.edit,
        })
}

#[cfg(test)]
pub(crate) mod test_support;

#[cfg(test)]
mod tests;

//! The bundled layer: the agent templates and skills OpenAlpaca ships,
//! carried in the binary (subagent design §2.1 B-1,
//! `tasks/subagent-brief-design.md`), and the shipped-versions list that
//! tells an earlier build's unedited copy from a person's edit (§2.2).
//!
//! The templates are registered from these bytes at every start; nothing is
//! written into `config/agents/`. A file there overrides the bundled template
//! of its `id` — unless it is byte-identical to a version OpenAlpaca shipped
//! (an earlier build seeded it, or a person copied the repository's
//! `config/`), in which case it is set aside at load: not read, not touched,
//! listed as not used, and the bundled template runs (V-1).
//!
//! A skill is read from files at every invocation — its scripts run from
//! its folder — so the skills are written, whole, into the machine's
//! `state/cache/bundled-skills/` at each start ([`write_skill_pack`], §2.6
//! K-2) and run from there; nothing is written into `config/skills/`. A
//! folder there named like a bundled skill overrides it, unless every file
//! in it is a version OpenAlpaca shipped at its path
//! ([`classify_skill_dir`], V-5).

use crate::agent::template::{AgentTemplate, parse_agent_markdown};
use crate::middleware::skill::{SkillScope, parse_skill_frontmatter};
use crate::orchestrator::skill_catalog::{SkillEntry, SkillSource};
use openalpaca_storage::hash::sha256_hex;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

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

/// The four shipped skills, each a folder under `skills/`. Their source is
/// the repository's `config/skills/` (B-2), as for the templates.
/// `create-skill` carries helper scripts, which the pack writes executable.
pub const BUNDLED_SKILLS: &[BundledFile] = &[
    bundled!("skills/code-review/SKILL.md"),
    bundled!("skills/commit-message/SKILL.md"),
    bundled!("skills/create-skill/SKILL.md"),
    bundled!("skills/create-skill/scripts/check_exists.sh"),
    bundled!("skills/create-skill/scripts/list_skills.sh"),
    bundled!("skills/create-skill/scripts/scaffold.sh"),
    bundled!("skills/create-skill/scripts/write_skill_md.sh"),
    bundled!("skills/explain-code/SKILL.md"),
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

/// The bundled skills' keys — each one's folder name under `skills/` — in
/// path order.
pub fn bundled_skill_keys() -> impl Iterator<Item = &'static str> {
    BUNDLED_SKILLS
        .iter()
        .filter_map(|file| file.path.strip_prefix("skills/")?.strip_suffix("/SKILL.md"))
}

/// Whether `key` (a folder name, lowercased) is a bundled skill's.
pub fn is_bundled_skill(key: &str) -> bool {
    bundled_skill_keys().any(|bundled| bundled == key)
}

/// The bundled skills as catalog entries, read from the binary's bytes and
/// pointing into the pack at `pack_root`, where they run from
/// ([`write_skill_pack`]). Scope [`SkillScope::Bundled`]. A bundled
/// `SKILL.md` that does not parse is left out with an `ERROR` —
/// `bundled::tests` refuses such a build.
pub fn bundled_skill_entries(pack_root: &Path) -> Vec<(String, SkillEntry)> {
    BUNDLED_SKILLS
        .iter()
        .filter_map(|file| {
            let key = file
                .path
                .strip_prefix("skills/")?
                .strip_suffix("/SKILL.md")?;
            match parse_skill_frontmatter(file.contents) {
                Ok(frontmatter) => {
                    let skill_dir = pack_root.join(key);
                    Some((
                        key.to_string(),
                        SkillEntry {
                            frontmatter,
                            skill_md_path: Some(skill_dir.join("SKILL.md")),
                            skill_dir: Some(skill_dir),
                            scope: SkillScope::Bundled,
                            source: SkillSource::FileBased,
                        },
                    ))
                }
                Err(e) => {
                    tracing::error!("The bundled skill {} does not parse: {e}", file.path);
                    None
                }
            }
        })
        .collect()
}

/// One writer of the pack at a time: the start and a skill's ensure-on-use
/// (K-3) may meet.
static PACK_LOCK: Mutex<()> = Mutex::new(());

/// Write the bundled skills, whole, into `root` (subagent design §2.6 K-2):
/// into `<root>.new/` first — files `0444`, `.sh` files `0555`, directories
/// `0755` on unix, saying "not for editing" — then the old `root` removed and
/// the new one renamed into place. A `<root>.new/` a crash left is removed
/// first. One daemon holds a store (`openalpacad.lock`) and no skill runs
/// before the catalog is built, so no reader is on an older pack; a hand
/// edit inside it never outlives a restart.
pub fn write_skill_pack(root: &Path) -> std::io::Result<()> {
    let _one_writer = PACK_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let name = root.file_name().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("{} names no folder", root.display()),
        )
    })?;
    let mut new_name = name.to_os_string();
    new_name.push(".new");
    let staging = root.with_file_name(new_name);
    if std::fs::symlink_metadata(&staging).is_ok() {
        remove_entry(&staging)?;
    }
    if let Some(parent) = staging.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::create_dir(&staging)?;
    set_mode(&staging, 0o755)?;
    for file in BUNDLED_SKILLS {
        let Some(relative) = file.path.strip_prefix("skills/") else {
            continue;
        };
        let mut dir = staging.clone();
        for part in Path::new(relative)
            .parent()
            .into_iter()
            .flat_map(Path::components)
        {
            dir.push(part);
            if !dir.is_dir() {
                std::fs::create_dir(&dir)?;
                set_mode(&dir, 0o755)?;
            }
        }
        let target = staging.join(relative);
        std::fs::write(&target, file.contents)?;
        let script = target.extension().and_then(|e| e.to_str()) == Some("sh");
        set_mode(&target, if script { 0o555 } else { 0o444 })?;
    }
    if std::fs::symlink_metadata(root).is_ok() {
        remove_entry(root)?;
    }
    std::fs::rename(&staging, root)
}

/// Remove a file, a link (never what it leads to) or a folder and all in it.
fn remove_entry(path: &Path) -> std::io::Result<()> {
    if std::fs::symlink_metadata(path)?.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
}

/// Elsewhere no mode is set, and scripts do not run, as before (DU-33,
/// macOS first).
#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> std::io::Result<()> {
    Ok(())
}

/// What a folder under `<config>/skills/` named like a bundled skill is
/// (subagent design §2.2 V-5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillFolder {
    /// Every regular file in it is byte-identical to a version shipped at its
    /// path, so it is set aside and the bundled skill runs. `Current` when
    /// each is this build's version, else `Earlier`. A file missing is
    /// allowed: an unedited copy of a skill the bundle later gave a file is
    /// still one.
    UneditedCopy(ShippedVersion),
    /// The person's — a file edited, or added where no build shipped one —
    /// so it overrides the bundled skill. `stale` names, relative to the
    /// folder and `/`-separated, each file still an earlier shipped version
    /// of its path that the bundle has since changed: the override runs the
    /// old file (W-2).
    Yours { stale: Vec<String> },
}

/// Classify `<config>/skills/<key>/` against the shipped versions (V-5),
/// or None when `key` is no bundled skill's (V-6: only a folder that would
/// hide a bundled skill is set aside). Every regular file under the folder
/// counts, recursively, by its bytes as on disk — modes ignored, a
/// `.DS_Store` ignored (Finder writes one into a folder merely opened); a
/// link, or anything that is neither a file nor a folder, makes the folder
/// the person's, as does a file that cannot be read. The rule errs only
/// towards "yours". The caller has seen the folder's `SKILL.md`.
pub fn classify_skill_dir(key: &str, dir: &Path) -> Option<SkillFolder> {
    classify_skill_dir_against(BUNDLED_SKILLS, key, dir)
}

/// The folders of `skills_dir` the skill catalog sets aside as unedited
/// copies (V-5) — each a child folder holding a `SKILL.md`, its key its
/// name lowercased, as the catalog reads it — in file-name order, with the
/// version each is: what `openalpaca skills tidy` lists and moves (subagent
/// design §2.12 D-2). Reads only; a directory that does not exist holds
/// none.
pub fn set_aside_skill_folders(
    skills_dir: &Path,
) -> std::io::Result<Vec<(PathBuf, ShippedVersion)>> {
    let entries = match std::fs::read_dir(skills_dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut folders: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir() && path.join("SKILL.md").exists())
        .collect();
    folders.sort_by(|a, b| a.file_name().cmp(&b.file_name()));
    Ok(folders
        .into_iter()
        .filter_map(|folder| {
            let key = folder.file_name()?.to_str()?.to_lowercase();
            match classify_skill_dir(&key, &folder)? {
                SkillFolder::UneditedCopy(version) => Some((folder, version)),
                SkillFolder::Yours { .. } => None,
            }
        })
        .collect())
}

/// [`classify_skill_dir`] against `bundle`, the current versions — the
/// binary's, or a test's.
pub(crate) fn classify_skill_dir_against(
    bundle: &[BundledFile],
    key: &str,
    dir: &Path,
) -> Option<SkillFolder> {
    if !is_bundled_skill(key) {
        return None;
    }
    let mut files = Vec::new();
    if !regular_files(dir, dir, &mut files) {
        return Some(SkillFolder::Yours { stale: Vec::new() });
    }
    files.sort();
    let mut edited = false;
    let mut earlier = false;
    let mut stale = Vec::new();
    for (relative, path) in files {
        let shipped_path = format!("skills/{key}/{relative}");
        let Ok(bytes) = std::fs::read(&path) else {
            edited = true;
            continue;
        };
        let hash = sha256_hex(&bytes);
        let current = bundle
            .iter()
            .find(|file| file.path == shipped_path)
            .map(|file| sha256_hex(file.contents.as_bytes()));
        if current.as_deref() == Some(hash.as_str()) {
            continue;
        }
        if shipped_lines().any(|line| line.hash == hash && line.path == shipped_path) {
            earlier = true;
            if current.is_some() {
                stale.push(relative);
            }
        } else {
            edited = true;
        }
    }
    Some(if edited {
        SkillFolder::Yours { stale }
    } else if earlier {
        SkillFolder::UneditedCopy(ShippedVersion::Earlier)
    } else {
        SkillFolder::UneditedCopy(ShippedVersion::Current)
    })
}

/// Every regular file under `dir`, with its path relative to `root`
/// (`/`-separated), `.DS_Store` left out. False at the first entry that is a
/// link or neither a file nor a folder, or a folder that cannot be read.
fn regular_files(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) -> bool {
    let Ok(read_dir) = std::fs::read_dir(dir) else {
        return false;
    };
    for entry in read_dir {
        let Ok(entry) = entry else {
            return false;
        };
        let path = entry.path();
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            return false;
        };
        if meta.is_dir() {
            if !regular_files(root, &path, out) {
                return false;
            }
        } else if meta.is_file() {
            if entry.file_name() == ".DS_Store" {
                continue;
            }
            let Ok(relative) = path.strip_prefix(root) else {
                return false;
            };
            let relative: Vec<String> = relative
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect();
            out.push((relative.join("/"), path));
        } else {
            return false;
        }
    }
    true
}

#[cfg(test)]
pub(crate) mod test_support;

#[cfg(test)]
mod tests;

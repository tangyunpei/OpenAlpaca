//! A project's folder in the home store (storage design rev 0.8, Stage 1:
//! ST-1, ST-3…ST-6, ST-13; the lead's rulings R1…R13).
//!
//! `<home>/projects/<name>/` holds one project's content — `artifacts/`,
//! `uploads/`, `memory/` — beside its `project.json`, which records the
//! project's permanent id and its root path, canonical when it was recorded
//! (a project whose root no longer canonicalizes to itself is missing, ST-23). `projects/` is the
//! user's, like everything at the home root but `state/` (R1), and a folder
//! under it holds `project.json` and content directories, nothing else (R3).
//!
//! There is no registry object (§3): these are functions over the files under
//! the **current** home root, read on demand, so the daemon, the CLI and the
//! app's shell agree and a test's `OPENALPACA_HOME_STORE` holds. The file
//! decides whose folder it is, never the name (R6), and a name is never
//! parsed back into a path (R8).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::str::FromStr;
use std::sync::{Mutex, MutexGuard};

use anyhow::{Context, Result, bail};
use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;
use uuid::Uuid;

use crate::memory_files::fsx;

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

// ============================================================================
// Roots (R4, R13)
// ============================================================================

/// The canonical form of a project root that may not exist: the deepest
/// existing ancestor canonicalized — links resolved, the volume's own
/// spelling — and the missing tail appended as written
/// (`fsx::canonical_path_lenient`). A relative path is refused before
/// anything is read: `Path::exists` and `canonicalize` would resolve it
/// against the process's working directory, which would make the daemon's
/// CWD a project (R22).
pub fn canonical_root(path: &Path) -> Result<PathBuf> {
    if !path.is_absolute() {
        bail!(
            "a project root must be an absolute path, got {}",
            path.display()
        );
    }
    fsx::canonical_path_lenient(path)
}

/// R13, which replaces `resolves_to_the_home_store` in S2: whether `root` is
/// the home store, lies inside it, or contains it — never a project, so
/// `$HOME` stays "no project" even with a `~/.git`. Both sides canonical, so a
/// link to the home store answers for the store it reaches. The store is
/// also judged as it is named — its parent resolved, its last component not
/// followed — so when the store itself is a link (`~/.openalpaca` leading to
/// another volume), the folder holding that link (`$HOME`) still contains
/// it. `false` when there is no usable home root or `root` is relative: a
/// question, never a reason to fail.
pub fn overlaps_home_store(root: &Path) -> bool {
    let Ok(home) = super::home_root() else {
        return false;
    };
    overlaps_home_store_in(&home, root)
}

/// [`overlaps_home_store`] against the home store `home`, as [`register`]
/// resolved it once.
fn overlaps_home_store_in(home: &Path, root: &Path) -> bool {
    let Ok(root) = canonical_root(root) else {
        return false;
    };
    let overlaps = |store: &Path| root.starts_with(store) || store.starts_with(&root);
    if canonical_root(home).is_ok_and(|reached| overlaps(&reached)) {
        return true;
    }
    match (home.parent(), home.file_name()) {
        (Some(parent), Some(name)) => {
            canonical_root(parent).is_ok_and(|parent| overlaps(&parent.join(name)))
        }
        _ => false,
    }
}

// ============================================================================
// The file and the entry (R9)
// ============================================================================

/// `projects/<name>/project.json` exactly as R9 writes it: the authority for
/// the project's identity (there is no project table, R14). Not
/// `deny_unknown_fields`: "no readable project.json" (R11) means unparseable
/// or another `version`, never "a newer build added a key".
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProjectFile {
    /// [`PROJECT_FILE_VERSION`]; any other reads as unreadable (R11).
    pub version: u32,
    pub id: ProjectId,
    /// The root — the string the rows hold (R4, R14): canonical when it was
    /// recorded; once it no longer canonicalizes to itself the project is
    /// missing (ST-23).
    pub root: String,
    /// RFC 3339, UTC, whole seconds: `2026-10-07T09:30:00Z`.
    pub created_at: String,
    /// Set only while a re-point is in flight (§6, R24).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_root: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_name: Option<String>,
}

/// One project as its folder records it — what `lookup`, `list` and `by_id`
/// return.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub id: ProjectId,
    /// [`ProjectFile::root`].
    pub root: PathBuf,
    /// The folder's name under `projects/`.
    pub name: String,
    /// The folder, canonical: what `store_root` answers for the project (S3)
    /// and the prefix every stored `storage_path` under it starts with.
    pub dir: PathBuf,
    pub created_at: String,
    /// The root no longer leads to its own folder, read when this entry was
    /// read: it is not a directory, or it no longer canonicalizes to itself
    /// ([`root_is_missing`]; ST-7, ST-23).
    pub missing: bool,
    /// `Some` only mid re-point (§6).
    pub previous_root: Option<PathBuf>,
    pub previous_name: Option<String>,
}

impl Project {
    /// The root as the five path columns hold it.
    pub fn root_string(&self) -> String {
        self.root.to_string_lossy().into_owned()
    }

    /// The file this entry was read from, for a rewrite (§6 steps 1 and 4).
    pub fn file(&self) -> ProjectFile {
        ProjectFile {
            version: PROJECT_FILE_VERSION,
            id: self.id,
            root: self.root_string(),
            created_at: self.created_at.clone(),
            previous_root: self
                .previous_root
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned()),
            previous_name: self.previous_name.clone(),
        }
    }
}

// ============================================================================
// Refusals (R12, R13, ST-20)
// ============================================================================

/// Why a project operation was refused — returned **inside** `anyhow::Error`
/// (the `ArtifactError` convention, `artifacts/mod.rs`), recovered with
/// `downcast_ref`. `Display` is one sentence a person can act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectError {
    /// R12: a root with no folder under `projects/`. `store_root` returns it (S3).
    NotRegistered { root: String },
    /// R13: the path is the home store, inside it, or contains it.
    IsHome { path: String },
    /// The path does not exist or is not a directory (register, choose,
    /// re-point target).
    NotADirectory { path: String },
    /// ST-20: the re-point target is already a project, or one of the five
    /// path columns names it.
    Exists { path: String },
    /// ST-20 against a re-point that has not finished: the target is that
    /// project's new root or its old one. The same code as [`Self::Exists`];
    /// its sentence names the start of the daemon that finishes the re-point,
    /// since deleting that project's data is refused until then (residual X7).
    RepointUnfinished { path: String },
    /// A re-point onto the path the project already has.
    SamePath { path: String },
}

impl ProjectError {
    /// The wire code the daemon answers with (S3, S5a).
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotRegistered { .. } => "WORKSPACE_NOT_FOUND",
            Self::IsHome { .. } => "WORKSPACE_IS_HOME",
            Self::NotADirectory { .. } => "WORKSPACE_NOT_A_DIRECTORY",
            Self::Exists { .. } | Self::RepointUnfinished { .. } => "WORKSPACE_EXISTS",
            Self::SamePath { .. } => "INVALID_PATH",
        }
    }
}

impl fmt::Display for ProjectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotRegistered { root } => write!(
                f,
                "{root} is not a project OpenAlpaca knows: no folder under projects/ records it"
            ),
            Self::IsHome { path } => write!(
                f,
                "{path} is, contains or lies inside the OpenAlpaca home store, so it cannot be a \
                 project. Name the project directory itself"
            ),
            Self::NotADirectory { path } => write!(
                f,
                "{path} is not a directory. Create it first, or name the folder the project is in now"
            ),
            Self::Exists { path } => write!(
                f,
                "{path} is already a project of its own, or OpenAlpaca's records already name it. \
                 Re-pointing onto it would merge two projects: delete that project's data first, \
                 or re-point onto another path"
            ),
            Self::RepointUnfinished { path } => write!(
                f,
                "{path} is either path of another project's re-point that has not finished; the \
                 next start of the daemon finishes that re-point. Re-pointing onto it would merge \
                 two projects: re-point onto another path"
            ),
            Self::SamePath { path } => write!(f, "{path} is already where the project is"),
        }
    }
}

impl std::error::Error for ProjectError {}

/// Whether `e`, anywhere in its chain, is [`ProjectError::NotRegistered`] —
/// how a reader learns after the fact that a root is unregistered (R12).
/// Chain-aware, so a `.context(..)` a caller adds never hides it.
pub fn is_not_registered(e: &anyhow::Error) -> bool {
    e.chain().any(|cause| {
        matches!(
            cause.downcast_ref::<ProjectError>(),
            Some(ProjectError::NotRegistered { .. })
        )
    })
}

// ============================================================================
// Reading (§3, R9, R11)
// ============================================================================

/// `home_root()/projects`. A path query: creates nothing.
pub fn projects_dir() -> Result<PathBuf> {
    Ok(projects_dir_in(&super::home_root()?))
}

/// `home/projects`, for the home store `home` as [`register`] resolved it
/// once.
fn projects_dir_in(home: &Path) -> PathBuf {
    home.join(PROJECTS_DIR)
}

/// §3 *lookup*: the project registered at `root`, or `None` — `root` as
/// written first (ST-23), then as named — its parent resolved, its last
/// component not followed (R-T29) — then its canonical form. A recorded root
/// that no longer canonicalizes to itself — its folder renamed by case only
/// on a case-insensitive volume, or moved with a link left at the old path —
/// still names its own project, missing, never the project its canonical
/// form now leads to, also when a link in an ancestor spells it otherwise;
/// any other path that a link or another case leads to a registered root
/// finds that root. Each form is two small reads, no scan and no lock —
/// `projects/<folder_name>/project.json` and, when that is absent,
/// unreadable or records another root,
/// `projects/<hashed_folder_name>/project.json` — and the named and
/// canonical forms' two are read only when they differ from the root as
/// written. Creates
/// nothing. A folder with no readable file is passed over with one WARN per
/// folder per process (R11). An error is a relative `root`, no home root, or
/// a folder that cannot be resolved.
pub fn lookup(root: &Path) -> Result<Option<Project>> {
    lookup_in(&super::home_root()?, root)
}

/// [`lookup`] under the home store `home`, as [`register`] resolved it once.
fn lookup_in(home: &Path, root: &Path) -> Result<Option<Project>> {
    if !root.is_absolute() {
        bail!(
            "a project root must be an absolute path, got {}",
            root.display()
        );
    }
    // Lexically tidied — a trailing `/` or a `.` would change the folder
    // name — and nothing resolved.
    let written: PathBuf = root.components().collect();
    if let Some(project) = recorded_in(home, &written)? {
        return Ok(Some(project));
    }
    let canonical = canonical_root(&written)?;
    if canonical == written {
        return Ok(None);
    }
    recorded_in(home, &canonical)
}

/// The project whose `project.json` records exactly `root`, compared as
/// written and then [`as_named`] — its parent resolved, its last component
/// not followed (the lead's ruling R-T29): a missing project's root spelled
/// through a link in an ancestor (macOS's `/var/…` for a root recorded under
/// `/private/var/…`) still names it, while a link left at the root itself is
/// never followed. Each form is [`recorded_exactly_in`]'s two reads.
fn recorded_in(home: &Path, root: &Path) -> Result<Option<Project>> {
    if let Some(project) = recorded_exactly_in(home, root)? {
        return Ok(Some(project));
    }
    match as_named(root) {
        Some(named) if named != root => recorded_exactly_in(home, &named),
        _ => Ok(None),
    }
}

/// `path` as named (the lead's ruling R-T29): its parent canonicalized
/// ([`canonical_root`]) and its last component kept as written, not
/// followed — the shape [`overlaps_home_store_in`] judges the store by. A
/// recorded root is canonical when it is recorded (R4, R14), so it is its
/// own name, and so is any spelling of it through a link in an ancestor.
/// Letter case and Unicode normalization are not reconciled here (design
/// §18). `None` for a path with no parent or no last component, or one
/// whose parent cannot be resolved.
fn as_named(path: &Path) -> Option<PathBuf> {
    let (parent, name) = (path.parent()?, path.file_name()?);
    canonical_root(parent).ok().map(|parent| parent.join(name))
}

/// The project whose `project.json` records exactly `root` — compared as
/// written, nothing resolved: `projects/<folder_name>/project.json` and, when
/// that is absent, unreadable or records another root,
/// `projects/<hashed_folder_name>/project.json`.
fn recorded_exactly_in(home: &Path, root: &Path) -> Result<Option<Project>> {
    let want = root.to_string_lossy();
    let projects = projects_dir_in(home);
    let plain = folder_name(root);
    let hashed = hashed_folder_name(root);
    let names = if hashed == plain {
        vec![plain]
    } else {
        vec![plain, hashed]
    };
    for name in names {
        if let Some(project) = read_project_dir(&projects.join(name))?
            && project.root_string() == want.as_ref()
        {
            return Ok(Some(project));
        }
    }
    Ok(None)
}

/// The project a folder under `projects/` records, or `None`: no folder
/// there, or one OpenAlpaca does not open (R11) — a link, a file, no
/// `project.json`, a `project.json` that is a link, has another hard link, is
/// not one this build parses, has another `version`, or records a root that is
/// not absolute. Each of those is one WARN per folder per process, and
/// nothing is deleted. A folder that goes while it is read — a re-point
/// renaming it, a delete removing it, on another thread that holds
/// [`lock`] while this reader does not — is no folder there, without a word:
/// R11's report is for a folder that stays without its file.
pub(crate) fn read_project_dir(dir: &Path) -> Result<Option<Project>> {
    match fs::symlink_metadata(dir) {
        Ok(meta) if meta.file_type().is_symlink() => {
            return Ok(unreadable(dir, "it is a symbolic link"));
        }
        Ok(meta) if !meta.is_dir() => return Ok(unreadable(dir, "it is not a directory")),
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Ok(unreadable(dir, &format!("it cannot be read ({e})"))),
    }
    #[cfg(test)]
    during_read(ReadStep::FolderStatted);
    let bytes = match fsx::read_if_exists(&dir.join(PROJECT_FILE)) {
        Ok(Some(bytes)) => bytes,
        Ok(None) | Err(_) if vanished(dir) => return Ok(None),
        Ok(None) => return Ok(unreadable(dir, "it has no project.json")),
        Err(e) => {
            return Ok(unreadable(
                dir,
                &format!("its project.json cannot be read ({e:#})"),
            ));
        }
    };
    let file: ProjectFile = match serde_json::from_slice(&bytes) {
        Ok(file) => file,
        Err(e) => {
            return Ok(unreadable(
                dir,
                &format!("its project.json is not one this build reads ({e})"),
            ));
        }
    };
    if file.version != PROJECT_FILE_VERSION {
        return Ok(unreadable(
            dir,
            &format!(
                "its project.json has version {}, and this build reads version {PROJECT_FILE_VERSION}",
                file.version
            ),
        ));
    }
    if !Path::new(&file.root).is_absolute() {
        return Ok(unreadable(
            dir,
            &format!(
                "its project.json records a root that is not an absolute path ({:?})",
                file.root
            ),
        ));
    }
    let name = dir
        .file_name()
        .with_context(|| format!("{} names no folder", dir.display()))?
        .to_string_lossy()
        .into_owned();
    #[cfg(test)]
    during_read(ReadStep::FileRead);
    let dir = match dir.canonicalize() {
        Ok(dir) => dir,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(e).with_context(|| format!("Failed to resolve {}", dir.display()));
        }
    };
    let root = PathBuf::from(&file.root);
    Ok(Some(Project {
        id: file.id,
        missing: root_is_missing(&root),
        root,
        name,
        dir,
        created_at: file.created_at,
        previous_root: file.previous_root.map(PathBuf::from),
        previous_name: file.previous_name,
    }))
}

/// Whether `dir` is gone now — what tells a folder that went while it was
/// read from one that stays without its file.
fn vanished(dir: &Path) -> bool {
    matches!(fs::symlink_metadata(dir), Err(e) if e.kind() == std::io::ErrorKind::NotFound)
}

/// ST-7 and ST-23: whether a project's recorded `root` no longer leads to its
/// own folder — what [`Project::missing`] says, decided here and nowhere
/// else. Missing when the root is not a directory (its folder is gone, or
/// something else took its path), or when it no longer canonicalizes to
/// itself: a root is canonical when it is recorded (R4, R14 — `register` and
/// the re-point write only a canonical root), so a canonical form that
/// differs means the folder moved under it — renamed by case only on a
/// case-insensitive volume, where the old spelling still opens the renamed
/// folder, or moved with a link left at the old path. A root that cannot be
/// canonicalized at all is missing. Reads no file: a `stat` and a `realpath`.
pub fn root_is_missing(root: &Path) -> bool {
    let leads_to_itself =
        root.is_dir() && fs::canonicalize(root).is_ok_and(|canonical| canonical.as_path() == root);
    !leads_to_itself
}

/// Where in [`read_project_dir`] a test's step runs: the places a re-point's
/// rename on another thread can land, which no call boundary lets a test
/// reach.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReadStep {
    /// The folder's own stat answered; its `project.json` is read next.
    FolderStatted,
    /// `project.json` read and parsed; the folder's canonical path is next.
    FileRead,
}

/// A step and where [`read_project_dir`] runs it.
#[cfg(test)]
type ReadHook = (ReadStep, Box<dyn FnOnce()>);

#[cfg(test)]
thread_local! {
    /// Test-only: a step [`read_project_dir`] runs once on this thread, at
    /// the [`ReadStep`] it names.
    static DURING_READ: std::cell::RefCell<Option<ReadHook>> =
        const { std::cell::RefCell::new(None) };
}

/// Runs this thread's [`DURING_READ`] step when it is set for `at`.
#[cfg(test)]
fn during_read(at: ReadStep) {
    let step = DURING_READ.with(|hook| {
        let mut hook = hook.borrow_mut();
        match hook.take() {
            Some((set_for, step)) if set_for == at => Some(step),
            other => {
                *hook = other;
                None
            }
        }
    });
    if let Some(step) = step {
        step();
    }
}

/// R11's report for `dir`, the first time this process meets it; always
/// `None`, so a reader can return it.
fn unreadable(dir: &Path, why: &str) -> Option<Project> {
    if first_warning(format!("unreadable {}", dir.display())) {
        tracing::warn!(
            "{} is not opened as a project: {why}. OpenAlpaca leaves the folder as it is and never deletes it",
            dir.display()
        );
    }
    None
}

/// What this process has already reported (R11: "reported once at WARN").
static WARNED: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

/// `true` the first time this process sees `key`.
fn first_warning(key: String) -> bool {
    WARNED.lock().unwrap_or_else(|p| p.into_inner()).insert(key)
}

// ============================================================================
// Listing (§3, R11)
// ============================================================================

/// §3 *list*: every project whose folder under `projects/` this build reads,
/// sorted by folder name; entries mid re-point included (R24 reads them).
/// Dot-prefixed names — a folder `register` is still building — links and
/// files are passed over without a word. A missing `projects/` is an empty
/// list, and nothing is created. Two folders carrying one id — a copied
/// folder — are both left out with one WARN (R11): neither can be told from
/// the other, so neither is opened, and neither is deleted.
pub fn list() -> Result<Vec<Project>> {
    list_in(&super::home_root()?)
}

/// [`list`] under the home store `home`, as [`register`] resolved it once.
fn list_in(home: &Path) -> Result<Vec<Project>> {
    let projects = projects_dir_in(home);
    let entries = match fs::read_dir(&projects) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => {
            return Err(e).with_context(|| format!("Failed to read {}", projects.display()));
        }
    };
    let mut found = Vec::new();
    for entry in entries {
        // An entry that went while the listing read it — a re-point's
        // rename, a delete — is no project now; any other failure is one.
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                return Err(e).with_context(|| format!("Failed to read {}", projects.display()));
            }
        };
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        // `DirEntry::file_type` does not follow a link, so a link is passed
        // over here, like a file.
        if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        if let Some(project) = read_project_dir(&entry.path())? {
            found.push(project);
        }
    }

    let mut dirs_by_id: BTreeMap<ProjectId, Vec<String>> = BTreeMap::new();
    for project in &found {
        dirs_by_id
            .entry(project.id)
            .or_default()
            .push(project.dir.display().to_string());
    }
    let mut shared = BTreeSet::new();
    for (id, mut dirs) in dirs_by_id {
        if dirs.len() < 2 {
            continue;
        }
        dirs.sort();
        if first_warning(format!("shared id {id}")) {
            tracing::warn!(
                "{} carry the same project id {id}, as a copied folder does; none of them is \
                 opened as a project, and none is deleted",
                dirs.join(" and ")
            );
        }
        shared.insert(id);
    }
    found.retain(|project| !shared.contains(&project.id));
    found.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(found)
}

/// [`list`] narrowed to one id: `None` when no folder carries it, or when two
/// do (R11).
pub fn by_id(id: &ProjectId) -> Result<Option<Project>> {
    Ok(list()?.into_iter().find(|project| project.id == *id))
}

/// [`list`] narrowed to the project whose `project.json` records `root`
/// **as written** — compared component by component, nothing resolved — or
/// `None`. What a caller holding a root the list printed asks first: a
/// recorded root that no longer canonicalizes to itself (its folder renamed
/// by case only on a case-insensitive volume, or moved with a link left at
/// the old path) still names its own project, never the one its canonical
/// form leads to — as [`lookup`] and [`resolve`] also answer since ST-23.
/// It reads the listing, so a `projects/` that cannot be listed is an error,
/// which the workspace routes answer `500` (residual X1). An entry mid
/// re-point is never the answer: its root is where it is going.
pub fn by_recorded_root(root: &Path) -> Result<Option<Project>> {
    Ok(list()?
        .into_iter()
        .find(|project| project.previous_root.is_none() && project.root == root))
}

/// [`list`] narrowed to a project whose re-point has not finished (§6, R24)
/// and that records `root` as where it is going or where it was. Between a
/// re-point's failed rename (or last write) and the next start, which
/// finishes it, [`lookup`] finds such a project under neither path: its
/// folder keeps the old name and records the new root. A second project at
/// either path would split it (ST-20) — the re-point's rename onto a held
/// name failing at every start, or its rows step merging the other project's
/// rows into it. Compared component by component, nothing resolved; the
/// caller passes the canonical form both fields hold.
pub fn unfinished_repoint_at(root: &Path) -> Result<Option<Project>> {
    Ok(list()?.into_iter().find(|project| {
        project.previous_root.is_some()
            && (project.root == root || project.previous_root.as_deref() == Some(root))
    }))
}

// ============================================================================
// The test-build guard (§3, last bullet)
// ============================================================================

/// Whether this build refuses to register under an unguarded home store:
/// storage's own tests, and the test builds of the crates that enable
/// `test-home-guard` through their `[dev-dependencies]`. Never a release or
/// `cargo run` build.
#[doc(hidden)]
pub fn test_home_guard_armed() -> bool {
    cfg!(any(test, feature = "test-home-guard"))
}

/// Why a test build may not write under the home store it would use, or
/// `None`: the store must have been set through `OPENALPACA_HOME_STORE`, and
/// not into the developer's own store. `set` is the variable;
/// `default_store` is `<user home>/.openalpaca`, `home_root`'s fallback. Pure:
/// it compares the paths it is given, and [`refuse_unguarded_test_home`] asks
/// it twice, of both as written and of both resolved.
#[cfg(any(test, feature = "test-home-guard"))]
fn unguarded_home(
    set: Option<&std::ffi::OsStr>,
    default_store: Option<&Path>,
) -> Option<&'static str> {
    match set {
        None => Some("unset"),
        Some(value) if default_store.is_some_and(|real| Path::new(value).starts_with(real)) => {
            Some("pointing into the real home store")
        }
        Some(_) => None,
    }
}

/// The home store one [`register`] call works under, resolved **once**: the
/// variable and the user's home are read one time, the test-build guard
/// judges that reading, and every read and write the call makes is under the
/// store this returns. A later `home_root()` would read the environment again,
/// and under parallel tests another test's guard may have dropped or moved
/// the variable in between — so a check that passed would no longer describe
/// the store written to (the S1 review's I1).
fn register_home() -> Result<PathBuf> {
    let set = std::env::var_os(super::HOME_STORE_ENV);
    let user_home = directories::BaseDirs::new().map(|b| b.home_dir().to_path_buf());
    refuse_unguarded_test_home(set.as_deref(), user_home.as_deref());
    super::resolve_home_root(set.map(PathBuf::from), user_home)
}

/// The owner's rule that no test writes into a developer's real store:
/// panics, before anything is read or written, in a test build whose home
/// store was not set through `OPENALPACA_HOME_STORE` or was set into the real
/// one — as written, or where it leads: a link, a `..` or another case that
/// reaches `<user home>/.openalpaca` is the real store too. Only the
/// variable's side is resolved through the store; the real store's side is
/// the user's home resolved, with `.openalpaca` appended, so a test whose
/// variable names a temp directory never reads the developer's store, not
/// even its metadata. A path that cannot be resolved is judged as written.
#[cfg(any(test, feature = "test-home-guard"))]
fn refuse_unguarded_test_home(set: Option<&std::ffi::OsStr>, user_home: Option<&Path>) {
    let real = user_home.map(|home| home.join(super::STORE_DIR_NAME));
    let why = unguarded_home(set, real.as_deref()).or_else(|| {
        let set = set.map(|value| resolved_or_as_written(Path::new(value)));
        let real = user_home.map(|home| resolved_or_as_written(home).join(super::STORE_DIR_NAME));
        unguarded_home(set.as_deref().map(Path::as_os_str), real.as_deref())
    });
    if let Some(why) = why {
        panic!(
            "store::projects::register in a test build with {} {why}: take the crate's \
             home-store guard (HomeStoreGuard; EnvSandbox in the CLI) first, so no test writes \
             into the developer's ~/.openalpaca. A daemon binary `cargo test` built carries this \
             check too: rebuild it with `cargo build -p openalpacad` before running it by hand",
            super::HOME_STORE_ENV,
        );
    }
}

/// [`canonical_root`] of `path`, or `path` as written when it has none.
#[cfg(any(test, feature = "test-home-guard"))]
fn resolved_or_as_written(path: &Path) -> PathBuf {
    canonical_root(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Release and `cargo run` builds: no check.
#[cfg(not(any(test, feature = "test-home-guard")))]
fn refuse_unguarded_test_home(_set: Option<&std::ffi::OsStr>, _user_home: Option<&Path>) {}

#[cfg(test)]
thread_local! {
    /// Test-only: a step [`register`] runs once on this thread, right after
    /// it has read the environment, checked it and resolved the home store —
    /// where, under parallel tests, another test's guard can drop or move
    /// `OPENALPACA_HOME_STORE`, which no call boundary lets a test reach.
    static AFTER_HOME_RESOLVED: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
}

// ============================================================================
// Registering (§3, R6, R10, R13)
// ============================================================================

/// The one process-wide lock over the folders under `projects/`.
static PROJECTS_LOCK: Mutex<()> = Mutex::new(());

/// Holding it is the right to create, rename or remove a folder under
/// `projects/`: `register`, the re-point (S3) and the delete's folder
/// removal (S5a) take it. **Lock order: this lock, then the database mutex —
/// never the reverse.** The re-point holds it across one database
/// transaction, so nothing may take it, or call `register`, inside a
/// `with_connection` closure. Not `Send`: it is held on one thread.
pub struct ProjectsLock {
    _held: MutexGuard<'static, ()>,
}

/// Takes [`ProjectsLock`]. A poisoned lock is recovered: the folders on disk
/// are the state, not anything the mutex holds.
pub fn lock() -> ProjectsLock {
    ProjectsLock {
        _held: PROJECTS_LOCK.lock().unwrap_or_else(|p| p.into_inner()),
    }
}

/// §3 *register*: the project at `root`, its folder created when there is
/// none — create-if-absent under [`lock`]. In order: the home store,
/// resolved once and judged by the test-build guard — every step after reads
/// and writes under that one store ([`register_home`]); [`canonical_root`];
/// an entry [`lookup`] finds — `root` as written first, then its canonical
/// form (ST-23) — is returned as it is, a missing project's included (ST-7),
/// so a stored root that now leads to another folder is never registered
/// again at that folder; a root that overlaps the home store is
/// [`ProjectError::IsHome`] (R13), one that is not a directory
/// [`ProjectError::NotADirectory`], and one a re-point is moving a project
/// onto whose rename or last write failed is refused until the next start
/// finishes that re-point (R24) — a second project there would take the
/// folder name its moved rows address; then the folder is built as
/// `projects/.<name>.<pid>-<n>.tmp/` holding `project.json` (0700 / 0600,
/// fsynced) and renamed to `projects/<name>`, so a crash never leaves a
/// folder without its file; a build that fails is removed. The lock makes it
/// create-if-absent within the process; the rename refuses a non-empty
/// target, though POSIX lets it replace an empty directory another process
/// made in between — and only the daemon registers, only where it accepts
/// work for a root (§3), so no other process does.
pub fn register(root: &Path) -> Result<Project> {
    let home = register_home()?;
    #[cfg(test)]
    if let Some(step) = AFTER_HOME_RESOLVED.with(|hook| hook.borrow_mut().take()) {
        step();
    }
    let written = root;
    let root = canonical_root(written)?;
    let held = lock();
    if let Some(existing) = lookup_in(&home, written)? {
        return Ok(existing);
    }
    let path = root.to_string_lossy().into_owned();
    if overlaps_home_store_in(&home, &root) {
        return Err(ProjectError::IsHome { path }.into());
    }
    if !root.is_dir() {
        return Err(ProjectError::NotADirectory { path }.into());
    }
    // A re-point onto this root whose rename or last write failed (§6 steps
    // 3–4) leaves the project's folder under its old name, recording this
    // root with the marker: `lookup` finds it under neither path until the
    // next start finishes it (R24). A second project registered here
    // meanwhile would take the folder name the moved rows already address,
    // and the re-point's rename would fail at every start after.
    if let Some(moving) = list_in(&home)?
        .into_iter()
        .find(|p| p.previous_root.is_some() && p.root_string() == path)
    {
        bail!(
            "{path} is where a re-point is moving the project in {}, and the re-point has not \
             finished; the next start of the daemon finishes it, and until then nothing is \
             registered here",
            moving.dir.display()
        );
    }
    let name = name_for_in(&home, &root, None, &held)?;
    let projects = projects_dir_in(&home);
    fsx::create_private_dir(&projects)?;
    let target = projects.join(&name);
    let building = fsx::temp_path_for(&target)?;
    let file = ProjectFile {
        version: PROJECT_FILE_VERSION,
        id: ProjectId::new(),
        root: path,
        created_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        previous_root: None,
        previous_name: None,
    };
    if let Err(e) = build_project_folder(&building, &target, &file) {
        let _ = fsx::remove_dir_all_if_exists(&building);
        return Err(e);
    }
    read_project_dir(&target)?
        .with_context(|| format!("{} was created but cannot be read back", target.display()))
}

/// The folder name `root` (canonical) takes (R6): the plain name unless
/// `projects/<plain>` exists and is not this project's — its file records
/// another root and, when `this` is `Some`, carries another id; or it holds
/// no readable file — then the hashed name under the same test; neither free
/// is an error naming both. `this` is `Some` only on a re-point
/// (`project_life`), so a project whose new plain name is its own folder's
/// keeps it. `register` calls `name_for_in` with the home store it resolved
/// once.
pub(crate) fn name_for(
    root: &Path,
    this: Option<&ProjectId>,
    held: &ProjectsLock,
) -> Result<String> {
    name_for_in(&super::home_root()?, root, this, held)
}

/// [`name_for`] under the home store `home`, as [`register`] resolved it once.
fn name_for_in(
    home: &Path,
    root: &Path,
    this: Option<&ProjectId>,
    _held: &ProjectsLock,
) -> Result<String> {
    let projects = projects_dir_in(home);
    let want = root.to_string_lossy();
    let usable = |name: &str| -> Result<bool> {
        let dir = projects.join(name);
        if let Err(e) = fs::symlink_metadata(&dir)
            && e.kind() == std::io::ErrorKind::NotFound
        {
            return Ok(true);
        }
        Ok(read_project_dir(&dir)?
            .is_some_and(|p| p.root_string() == want.as_ref() || this == Some(&p.id)))
    };
    let plain = folder_name(root);
    if usable(&plain)? {
        return Ok(plain);
    }
    let hashed = hashed_folder_name(root);
    if hashed != plain && usable(&hashed)? {
        return Ok(hashed);
    }
    let names = if hashed == plain {
        plain
    } else {
        format!("{plain} and {hashed}")
    };
    bail!(
        "no folder name is free for the project {}: {names} under {} belong to other projects \
         or hold no readable {PROJECT_FILE}",
        root.display(),
        projects.display()
    )
}

/// Writes `dir/project.json` (R9): pretty JSON in field order and one final
/// newline, mode 0600, through `fsx`'s atomic write (no-follow, temp file,
/// fsync, rename, directory fsync).
pub(crate) fn write_project_file(dir: &Path, file: &ProjectFile) -> Result<()> {
    let mut bytes = serde_json::to_vec_pretty(file).context("Failed to serialize project.json")?;
    bytes.push(b'\n');
    fsx::write_atomic_private(&dir.join(PROJECT_FILE), &bytes)
}

/// `building` (0700) with `file` in it, renamed onto `target`, `projects/`
/// fsynced after.
fn build_project_folder(building: &Path, target: &Path, file: &ProjectFile) -> Result<()> {
    fsx::create_private_dir(building)?;
    write_project_file(building, file)?;
    fs::rename(building, target).with_context(|| {
        format!(
            "Failed to move {} into place as {}",
            building.display(),
            target.display()
        )
    })?;
    if let Some(projects) = target.parent() {
        fsx::fsync_dir(projects)?;
    }
    Ok(())
}

// ── Resolution (storage design §3 *Resolve*; Stage 1 S2) ──────────────────

/// What a client path resolves to (§3). `project: None` is a root with no
/// folder under `projects/` yet — a git repository nobody has used (R12).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedProject {
    /// The root: the registered project's recorded `root` — canonical when
    /// it was recorded (R14), and as written for a missing project that no
    /// longer leads to itself (ST-23) — or the canonical folder that holds
    /// `.git`.
    pub root: PathBuf,
    /// The project's entry, when the root is registered.
    pub project: Option<Project>,
}

impl ResolvedProject {
    /// Whether the root has a folder under `projects/` — R12: one that does
    /// not has no store.
    pub fn is_registered(&self) -> bool {
        self.project.is_some()
    }

    /// `root.to_string_lossy().into_owned()` — the value the path columns hold.
    pub fn root_string(&self) -> String {
        self.root.to_string_lossy().into_owned()
    }

    fn registered(project: Project) -> Self {
        Self {
            root: project.root.clone(),
            project: Some(project),
        }
    }
}

/// What makes a folder a project root without anyone choosing it (ST-6): a
/// `.git` directory, or a `.git` file — a worktree is its own project.
const GIT_MARKER: &str = ".git";

/// §3 *Resolve* — pure: no cache, creates nothing. `Ok(None)` is no project.
///
/// A relative path is no project (R22: it would be read against the daemon's
/// working directory). A path that is not its own canonical form — it passes
/// through a link or another case, or its folder is gone — is first walked
/// as written ([`resolve_as_written`], ST-23): a root a project records,
/// matched exactly as written or as named (R-T29), is that project when it
/// is missing — it no longer leads to its own folder — unless a folder
/// nearer on the written path is a project by any reading; a project that
/// still leads to itself is resolved by the walk below, as before. Then the
/// path is canonicalised and
/// walked up; at each folder, in this order: a folder that is, lies inside
/// or contains the home store ends the walk with no project (R13 — every
/// folder above it contains the home store too); a registered project's
/// root is that project; a folder holding `.git` is an unregistered root.
/// The nearest match wins. A path that cannot be canonicalised — its
/// folder is gone — is walked as written ([`canonical_root`]) against
/// registered roots only, so a missing project still resolves to itself
/// (ST-7). `.openalpaca` is no marker (ST-9).
pub fn resolve(path: &Path) -> anyhow::Result<Option<ResolvedProject>> {
    if !path.is_absolute() {
        return Ok(None);
    }
    let canonical = path.canonicalize();
    if canonical.as_deref().ok() != Some(path)
        && let Some(found) = resolve_as_written(path)?
    {
        return Ok(Some(found));
    }
    let Ok(canonical) = canonical else {
        return resolve_missing(path);
    };
    for folder in canonical.ancestors() {
        if overlaps_home_store(folder) {
            return Ok(None);
        }
        if let Some(project) = lookup(folder)? {
            return Ok(Some(ResolvedProject::registered(project)));
        }
        if folder.join(GIT_MARKER).exists() {
            return Ok(Some(ResolvedProject {
                root: folder.to_path_buf(),
                project: None,
            }));
        }
    }
    Ok(None)
}

/// ST-23's first pass of [`resolve`], for a path that is not its own
/// canonical form: its folders as written, nearest first — lexically,
/// nothing resolved but what [`overlaps_home_store`], [`lookup`] and a stat
/// of `.git` read. A folder a project records exactly as written — or as
/// named, its parent resolved and its last component not followed, so a
/// spelling through a link in an ancestor counts (the lead's ruling R-T29)
/// — is that project when it is missing ([`root_is_missing`], read into
/// [`Project::missing`]): a recorded root that no longer leads to its own
/// folder names its own project, never the one its canonical form now leads
/// to. The pass ends with no answer, and the canonical walk decides, at the
/// first folder that is anything else: one that is, lies inside or contains
/// the home store (R13; no recorded root can lie at or above it), a recorded
/// root that still leads to itself (resolved as before Stage 2, a path
/// through a link inside it by its canonical form — the lead's ruling
/// R-T17), one that is a registered project only by its canonical form (a
/// root a link or another case reaches), or one holding `.git`. A path with
/// a `..` component has no such pass: its folders as written are not the
/// ones it lies in.
fn resolve_as_written(path: &Path) -> anyhow::Result<Option<ResolvedProject>> {
    if path.components().any(|part| part == Component::ParentDir) {
        return Ok(None);
    }
    let written: PathBuf = path.components().collect();
    for folder in written.ancestors() {
        if overlaps_home_store(folder) {
            return Ok(None);
        }
        if let Some(project) = lookup(folder)? {
            let names_it = project.root.as_path() == folder
                || as_named(folder).as_deref() == Some(project.root.as_path());
            if names_it && project.missing {
                return Ok(Some(ResolvedProject::registered(project)));
            }
            return Ok(None);
        }
        if folder.join(GIT_MARKER).exists() {
            return Ok(None);
        }
    }
    Ok(None)
}

/// [`resolve`] for a path that cannot be canonicalised: its ancestors as
/// written, deepest first, against registered roots only.
fn resolve_missing(path: &Path) -> anyhow::Result<Option<ResolvedProject>> {
    let written = canonical_root(path)?;
    for folder in written.ancestors() {
        if let Some(project) = lookup(folder)? {
            return Ok(Some(ResolvedProject::registered(project)));
        }
    }
    Ok(None)
}

/// [`resolve`], then [`register`] when the root it found has no folder yet —
/// the one call every registration site makes (§3: only the daemon, only
/// where it accepts work for a root). `Ok(None)`: no project, nothing
/// recorded. A registered project, missing or not, is returned as it is.
pub fn resolve_and_register(path: &Path) -> anyhow::Result<Option<Project>> {
    match resolve(path)? {
        None => Ok(None),
        Some(ResolvedProject {
            project: Some(project),
            ..
        }) => Ok(Some(project)),
        Some(ResolvedProject {
            root,
            project: None,
        }) => register(&root).map(Some),
    }
}

/// R10 — choosing a folder (the app's `Use`, `openalpaca chat --project`):
/// resolved like any path, so inside a git repository it is the repository
/// and inside a chosen folder it is that folder. Only when nothing is found
/// does the folder itself become a project, through [`register`]. Its own
/// checks refuse a relative path before anything is read ([`canonical_root`],
/// R22), then the home store, the folders inside it and the folders around
/// it ([`ProjectError::IsHome`]), and anything that is not a directory
/// ([`ProjectError::NotADirectory`]). `POST /v1/workspaces` (S5a) is its
/// first caller.
pub fn choose(path: &Path) -> anyhow::Result<Project> {
    match resolve_and_register(path)? {
        Some(project) => Ok(project),
        None => register(path),
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod resolve_tests;

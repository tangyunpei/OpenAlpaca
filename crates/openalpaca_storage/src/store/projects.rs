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

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
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
/// link to the home store answers for the store it reaches. `false` when
/// there is no usable home root or `root` is relative: a question, never a
/// reason to fail.
pub fn overlaps_home_store(root: &Path) -> bool {
    let Ok(root) = canonical_root(root) else {
        return false;
    };
    let Ok(home) = super::home_root().and_then(|home| canonical_root(&home)) else {
        return false;
    };
    root.starts_with(&home) || home.starts_with(&root)
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
    /// The canonical root — the string the rows hold (R4, R14).
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
    /// The root is not a directory, read when this entry was read (ST-7).
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
            Self::Exists { .. } => "WORKSPACE_EXISTS",
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
    Ok(super::home_root()?.join(PROJECTS_DIR))
}

/// §3 *lookup*: the project registered at `root`, or `None`. Two small
/// reads, no scan and no lock: `projects/<folder_name>/project.json` and,
/// when that is absent, unreadable or records another root,
/// `projects/<hashed_folder_name>/project.json`. Creates nothing. A folder
/// with no readable file is passed over with one WARN per folder per process
/// (R11). An error is a relative `root`, no home root, or a folder that
/// cannot be resolved.
pub fn lookup(root: &Path) -> Result<Option<Project>> {
    let root = canonical_root(root)?;
    let want = root.to_string_lossy();
    let projects = projects_dir()?;
    let plain = folder_name(&root);
    let hashed = hashed_folder_name(&root);
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
/// nothing is deleted.
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
    let bytes = match fsx::read_if_exists(&dir.join(PROJECT_FILE)) {
        Ok(Some(bytes)) => bytes,
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
    let dir = dir
        .canonicalize()
        .with_context(|| format!("Failed to resolve {}", dir.display()))?;
    let root = PathBuf::from(&file.root);
    Ok(Some(Project {
        id: file.id,
        missing: !root.is_dir(),
        root,
        name,
        dir,
        created_at: file.created_at,
        previous_root: file.previous_root.map(PathBuf::from),
        previous_name: file.previous_name,
    }))
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
    let projects = projects_dir()?;
    let entries = match fs::read_dir(&projects) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => {
            return Err(e).with_context(|| format!("Failed to read {}", projects.display()));
        }
    };
    let mut found = Vec::new();
    for entry in entries {
        let entry = entry.with_context(|| format!("Failed to read {}", projects.display()))?;
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
/// `default_store` is `<user home>/.openalpaca`, `home_root`'s fallback.
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

/// The owner's rule that no test writes into a developer's real store:
/// panics, before anything is read or written, in a test build whose home
/// store was not set through `OPENALPACA_HOME_STORE` or was set into the real
/// one.
#[cfg(any(test, feature = "test-home-guard"))]
fn refuse_unguarded_test_home() {
    let set = std::env::var_os(super::HOME_STORE_ENV);
    let default_store =
        directories::BaseDirs::new().map(|b| b.home_dir().join(super::STORE_DIR_NAME));
    if let Some(why) = unguarded_home(set.as_deref(), default_store.as_deref()) {
        panic!(
            "store::projects::register in a test build with {} {why}: take the crate's \
             home-store guard (HomeStoreGuard; EnvSandbox in the CLI) first, so no test writes \
             into the developer's ~/.openalpaca. A daemon binary `cargo test` built carries this \
             check too: rebuild it with `cargo build -p openalpacad` before running it by hand",
            super::HOME_STORE_ENV,
        );
    }
}

/// Release and `cargo run` builds: no check.
#[cfg(not(any(test, feature = "test-home-guard")))]
fn refuse_unguarded_test_home() {}

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
/// none — create-if-absent under [`lock`]. In order: the test-build guard;
/// [`canonical_root`]; an entry [`lookup`] finds is returned as it is, a
/// missing project's included (ST-7); a root that overlaps the home store is
/// [`ProjectError::IsHome`] (R13), one that is not a directory
/// [`ProjectError::NotADirectory`]; then the folder is built as
/// `projects/.<name>.<pid>-<n>.tmp/` holding `project.json` (0700 / 0600,
/// fsynced) and renamed to `projects/<name>`, so a crash never leaves a
/// folder without its file; a build that fails is removed. The lock makes it
/// create-if-absent within the process; the rename refuses a non-empty
/// target, though POSIX lets it replace an empty directory another process
/// made in between — and only the daemon registers, only where it accepts
/// work for a root (§3), so no other process does.
pub fn register(root: &Path) -> Result<Project> {
    refuse_unguarded_test_home();
    let root = canonical_root(root)?;
    let held = lock();
    if let Some(existing) = lookup(&root)? {
        return Ok(existing);
    }
    let path = root.to_string_lossy().into_owned();
    if overlaps_home_store(&root) {
        return Err(ProjectError::IsHome { path }.into());
    }
    if !root.is_dir() {
        return Err(ProjectError::NotADirectory { path }.into());
    }
    let name = name_for(&root, None, &held)?;
    let projects = projects_dir()?;
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
/// is an error naming both. `this` is `Some` only on a re-point (S3), so a
/// project whose new plain name is its own folder's keeps it.
pub(crate) fn name_for(
    root: &Path,
    this: Option<&ProjectId>,
    _held: &ProjectsLock,
) -> Result<String> {
    let projects = projects_dir()?;
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

#[cfg(test)]
mod tests;

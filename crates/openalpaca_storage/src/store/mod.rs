//! The single source of truth for every OpenAlpaca path.
//!
//! There is one root — `~/.openalpaca` (`home_root()`, overridable with
//! `OPENALPACA_HOME_STORE`). App state and content with no project live there,
//! and each project's content lives in its own folder under it,
//! `projects/<name>/` (owner decision ST-1; [`projects`] names and finds the
//! folders). No store writes inside a project's own folder: what is written
//! there is written by an agent's file tool or shell command, each asking
//! first (`openalpaca_core`'s `tools/working_folder.rs` names the folder a
//! turn works in). Both stores have the same shape for content, so
//! `content_dir(scope, kind)` is `root/<kind>` in both cases.
//!
//! ```text
//! ~/.openalpaca/
//!   README.md          seeded once; explains every entry
//!   .layout            line 1: layout version; line 2: install_id=<uuid-v4>
//!   state/             MACHINE STATE — opaque, never user-edited, never committed
//!     openalpaca.db (+ -wal, -shm), discovery.json, openalpacad.lock, .master_key,
//!     repl_history     the CLI chat's line history
//!     backups/         rotated copies of hand-edited config files
//!     logs/            daemon.log, gui.log
//!   config/            USER-EDITED runtime config (GUI/CLI-managed daemons)
//!   plugins/           user-dropped plugin dirs + .permissions.toml
//!   projects/<name>/   one folder per project: project.json, artifacts/, uploads/, memory/
//!   scratch/<id>/      the working folder of a conversation with no project
//!   artifacts/ uploads/ sessions/ …   content store, home scope
//! ```
//!
//! The organising rule: **`state/` is the machine's; everything else at the root
//! is the human's.** A new content kind exists when, and only when, it is added
//! to [`ContentKind`] — no crate ever joins a literal directory name onto a store
//! root.

mod artifact;
pub mod projects;
mod reveal;

pub use artifact::{
    FILE_NAME_SLUG_BYTES, artifact_extension, artifact_file_name, confine_to_root,
    kind_for_file_name, leading_sequence, loose_dir, opens_in_default_app, run_dir, slugify,
    upload_dir, upload_file_name, version_file_path,
};
pub use reveal::{RevealRefusal, revealable_path};

use anyhow::{Context, Result, bail};
use directories::BaseDirs;
use std::fs;
use std::path::{Path, PathBuf};

/// Environment override for the home root (D4). Absolute paths only.
pub const HOME_STORE_ENV: &str = "OPENALPACA_HOME_STORE";

/// The home store's directory name under the user's home: `~/.openalpaca`.
/// No store is made inside a project (storage design ST-1): a project's
/// content lives in its folder under `projects/` here, and an `.openalpaca/`
/// an earlier build left in a project is never read (ST-9).
pub const STORE_DIR_NAME: &str = ".openalpaca";

/// Layout version written to line 1 of `.layout`.
pub const LAYOUT_VERSION: u32 = 1;

/// The database file name under `state/`, so [`database_path`] and
/// [`open_home_database`] cannot drift.
const DB_FILE: &str = "openalpaca.db";

const LAYOUT_FILE: &str = ".layout";
const README_FILE: &str = "README.md";
const INSTALL_ID_KEY: &str = "install_id";

// ============================================================================
// Roots
// ============================================================================

/// The home root: `$OPENALPACA_HOME_STORE` if set, else `<home>/.openalpaca`.
///
/// Read on every call — never cached — so a test (or a wrapper script) can point
/// the whole process at another root by setting the variable. Relative values are
/// rejected: a relative store root would silently re-introduce CWD dependence.
pub fn home_root() -> Result<PathBuf> {
    let home = BaseDirs::new().map(|b| b.home_dir().to_path_buf());
    resolve_home_root(std::env::var_os(HOME_STORE_ENV).map(PathBuf::from), home)
}

/// Pure resolution behind [`home_root`], so the rules are testable without
/// mutating the process environment.
fn resolve_home_root(override_value: Option<PathBuf>, home: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(path) = override_value {
        if !path.is_absolute() {
            bail!(
                "{HOME_STORE_ENV} must be an absolute path, got '{}'",
                path.display()
            );
        }
        return Ok(path);
    }
    let home = home.context("Failed to determine the home directory")?;
    Ok(home.join(STORE_DIR_NAME))
}

/// `home_root()/state` — machine state. Created (0700 on Unix) if missing.
pub fn state_dir() -> Result<PathBuf> {
    let dir = home_root()?.join("state");
    create_private_dir(&dir)?;
    Ok(dir)
}

/// `home_root()/state` without creating it — the file accessors below are pure
/// path queries, so merely *reading* discovery never materialises a store.
fn state_dir_path() -> Result<PathBuf> {
    Ok(home_root()?.join("state"))
}

/// `state/openalpaca.db`
pub fn database_path() -> Result<PathBuf> {
    Ok(state_dir_path()?.join(DB_FILE))
}

/// Opens this install's database, creating `state/` (0700 on unix) if it is not
/// there — the ordering every process outside the daemon's boot preamble needs.
///
/// The daemon makes `state/` itself, long before it opens anything
/// (`discovery::acquire_single_instance_lock` calls [`state_dir`]). Every other
/// process — `openalpaca config`, the connector examples — has no such step, so
/// this is where the private directory is guaranteed. Going through
/// [`database_path`] instead would let SQLite create `state/` at the process
/// umask, leaving `.master_key` and `discovery.json` world-readable.
pub fn open_home_database() -> Result<crate::database::Database> {
    let path = state_dir()?.join(DB_FILE);
    crate::database::Database::open(&path)
        .with_context(|| format!("Failed to open {}", path.display()))
}

/// `state/discovery.json`
pub fn discovery_path() -> Result<PathBuf> {
    Ok(state_dir_path()?.join("discovery.json"))
}

/// `state/openalpacad.lock`
pub fn lock_path() -> Result<PathBuf> {
    Ok(state_dir_path()?.join("openalpacad.lock"))
}

/// Directory holding `.master_key` — the state dir. Passed to `KeyEncryptor::ensure_at`.
pub fn master_key_dir() -> Result<PathBuf> {
    state_dir()
}

/// `state/logs` — created if missing.
pub fn logs_dir() -> Result<PathBuf> {
    let dir = state_dir()?.join(LOGS_DIR);
    fs::create_dir_all(&dir)
        .with_context(|| format!("Failed to create logs directory: {}", dir.display()))?;
    Ok(dir)
}

/// `state/logs/daemon.log` — the log a launched daemon writes to.
///
/// One name, three consumers: both launchers — `openalpaca daemon start` and
/// the GUI sidecar — point the child's stdout and stderr at it (rotating
/// first, with [`rotate_daemon_log`], so it stays bounded), and
/// `GET /v1/status` reports it when it exists.
/// Non-creating, like [`database_path`] — naming a file is not a reason to
/// make its directory, and the status route's question is whether the file is
/// *there*.
pub fn daemon_log_path() -> Result<PathBuf> {
    Ok(state_dir_path()?.join(LOGS_DIR).join(DAEMON_LOG_FILE))
}

const LOGS_DIR: &str = "logs";
const DAEMON_LOG_FILE: &str = "daemon.log";

/// Set on the environment of a daemon whose launcher pointed its stdout and
/// stderr at `daemon_log_path()` and rotated the file first — marks *this*
/// daemon instance as that file's owner. `openalpaca daemon start` and the GUI
/// sidecar both set it; a bare `cargo run` does not. `GET /v1/status` gates
/// `log_path` on this in addition to the file existing, so a daemon nobody
/// pointed at the file never reports a path to some *other* daemon's leftover
/// `daemon.log` just because one happens to be sitting there (T44 fix round 1,
/// Important #3). The meaning is ownership, not which launcher: the sidecar
/// used to send its daemon's output to `/dev/null`, and was left out for that
/// reason alone (T30).
pub const MANAGED_LOG_ENV: &str = "OPENALPACA_MANAGED_LOG";

/// `daemon.log` is rotated once it is past this size — 16 MB.
pub const DAEMON_LOG_MAX_BYTES: u64 = 16 * 1024 * 1024;

/// Rotated generations kept: `daemon.log.1` … `daemon.log.3`, so the log costs
/// at most four files however long a daemon runs.
pub const DAEMON_LOG_KEEP: usize = 3;

/// The most a tail read takes from the end of a log — one seek and one short
/// read, however large the file has grown.
pub const LOG_TAIL_READ_BYTES: u64 = 64 * 1024;

/// Rotate `daemon.log` once it is past [`DAEMON_LOG_MAX_BYTES`], keeping
/// [`DAEMON_LOG_KEEP`] generations.
///
/// The file is a launched daemon's stdout and stderr, and nothing else bounds
/// it: a long-lived daemon that logs at `info` would fill a disk given months.
/// The caps are deliberately dumb — a size check at launch, no timer, no
/// compression, no dependency. Shared, because there are two launchers and
/// only one of them used to do this: the GUI sidecar discarded its daemon's
/// output entirely, which is how a fatal boot error came to reach nobody.
///
/// Callers treat a failure as a warning, never as a reason not to start: a
/// daemon that will not start because its log could not be renamed is the
/// worse bug.
pub fn rotate_daemon_log() -> Result<()> {
    let path = daemon_log_path()?;
    rotate_log(&path, DAEMON_LOG_MAX_BYTES, DAEMON_LOG_KEEP)
        .with_context(|| format!("Failed to rotate {}", path.display()))
}

/// Shift a log's generations down one when it is past `max_bytes`.
///
/// `daemon.log` → `.1` → `.2` → … → `.{keep}`, and whatever was at `.{keep}`
/// is gone. A log that does not exist, or that is still under the cap, is left
/// alone — the first start of a fresh install rotates nothing.
fn rotate_log(path: &Path, max_bytes: u64, keep: usize) -> std::io::Result<()> {
    match fs::metadata(path) {
        Ok(meta) if meta.len() > max_bytes => {}
        // Absent, or small enough: nothing to do. An unreadable log is not a
        // reason to refuse to start, so it is treated the same way.
        _ => return Ok(()),
    }

    // Oldest first, so no rename can overwrite a generation that has not moved
    // yet. `keep` is the last one kept, which makes `.{keep}` the one dropped.
    let _ = fs::remove_file(log_generation(path, keep));
    for n in (1..keep).rev() {
        let from = log_generation(path, n);
        if from.exists() {
            fs::rename(&from, log_generation(path, n + 1))?;
        }
    }
    fs::rename(path, log_generation(path, 1))
}

/// `daemon.log` + `.n` — appended, never substituted, so the base name's own
/// extension survives.
fn log_generation(path: &Path, n: usize) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(format!(".{n}"));
    PathBuf::from(name)
}

/// The last `lines` lines a log gained at or after byte `from`, newest last.
///
/// Reads at most [`LOG_TAIL_READ_BYTES`] from the end of the file. `from` is
/// where the run being asked about began writing — the file's length just
/// before that daemon was spawned, or `0` for the whole file — so a daemon
/// that died before writing a byte reads as an empty tail, never as the
/// *previous* run's last words presented as this one's. When the window
/// starts inside that range, its first line is a fragment of one it did not
/// reach and is dropped: nothing is shown cut mid-line. A missing file is an
/// empty tail. Everything else is [`log_tail`]'s.
pub fn read_log_tail(path: &Path, from: u64, lines: usize) -> std::io::Result<String> {
    use std::io::{Read, Seek, SeekFrom};

    let mut file = match fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(String::new()),
        Err(e) => return Err(e),
    };
    let len = file.metadata()?.len();
    // A file shorter than `from` was rotated or truncated under us: the run's
    // output, if any, is all of what is there now.
    let from = if from > len { 0 } else { from };
    let start = from.max(len.saturating_sub(LOG_TAIL_READ_BYTES));
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.take(LOG_TAIL_READ_BYTES).read_to_end(&mut bytes)?;

    let text = String::from_utf8_lossy(&bytes);
    let text = if start > from {
        text.split_once('\n').map_or("", |(_, rest)| rest)
    } else {
        &text
    };
    Ok(log_tail(text, lines))
}

/// The last `lines` lines of `text`, newest last, as a person should read them.
///
/// Verbatim otherwise: no line is reworded, prefixed or cut, and a blank line
/// *inside* the window stays — a multi-paragraph message must still read as
/// the paragraphs it was written in. Three things are removed, none of them
/// text: trailing blank lines (a log ends in a newline,
/// and a daemon that died mid-write can leave more), the blank lines the
/// window would otherwise open with, and ANSI colour escapes — the daemon's
/// `tracing` output carries them even into a file, and a panel that renders
/// the tail would print them as `[2m` noise.
pub fn log_tail(text: &str, lines: usize) -> String {
    let cleaned: Vec<String> = text.lines().map(strip_ansi).collect();
    let end = cleaned
        .iter()
        .rposition(|line| !line.trim().is_empty())
        .map_or(0, |last| last + 1);
    let window = &cleaned[end.saturating_sub(lines)..end];
    let first = window
        .iter()
        .position(|line| !line.trim().is_empty())
        .unwrap_or(window.len());
    window[first..].join("\n")
}

/// `line` without its ANSI CSI escape sequences (`ESC [` … final byte), which
/// is every colour and style code `tracing`'s formatter writes.
fn strip_ansi(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            // Parameter and intermediate bytes, then one final byte in
            // `@`..=`~`. A sequence the line ends inside is dropped whole.
            for next in chars.by_ref() {
                if ('@'..='~').contains(&next) {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// `state/cache` — derived data the machine can rebuild; today the memory
/// search index, `memory-index-v1.sqlite`. A pure path query: the index opens
/// it, and nothing else creates it. Deleting the directory costs one rebuild
/// of the index from the notes on disk and nothing else.
pub fn cache_dir() -> Result<PathBuf> {
    Ok(state_dir()?.join(CACHE_DIR))
}

const CACHE_DIR: &str = "cache";

/// `state/backups` — created if missing. The atomic config writer's rotation target.
pub fn backups_dir() -> Result<PathBuf> {
    let dir = state_dir()?.join("backups");
    fs::create_dir_all(&dir)
        .with_context(|| format!("Failed to create backups directory: {}", dir.display()))?;
    Ok(dir)
}

/// `home_root()/plugins` — user-dropped plugin directories. Created if missing.
pub fn plugins_dir() -> Result<PathBuf> {
    let dir = home_root()?.join("plugins");
    fs::create_dir_all(&dir)
        .with_context(|| format!("Failed to create plugins directory: {}", dir.display()))?;
    Ok(dir)
}

/// `home_root()/config` — the runtime config dir GUI/CLI-managed daemons are
/// started with (`OPENALPACA_CONFIG_DIR`).
///
/// A pure path query: the CLI calls this only to *test* whether a runtime
/// `llm.toml`/`daemon.toml` exists before falling back to the repo's `./config`,
/// and asking must not materialise a store. Use [`ensure_runtime_config_dir`]
/// where something is about to be written.
///
/// The *semantics* of `OPENALPACA_CONFIG_DIR` are untouched by the root move:
/// a dev run from the repo still resolves `./config` through the exe/CWD walk-up.
pub fn runtime_config_dir() -> Result<PathBuf> {
    Ok(home_root()?.join("config"))
}

/// [`runtime_config_dir`], created if missing.
///
/// The GUI and the CLI pass this to a daemon they spawn, and
/// `resolve_config_base_dir` ignores an `OPENALPACA_CONFIG_DIR` that does not
/// exist — so the directory has to be there before the spawn, not after.
pub fn ensure_runtime_config_dir() -> Result<PathBuf> {
    let dir = runtime_config_dir()?;
    fs::create_dir_all(&dir)
        .with_context(|| format!("Failed to create config directory: {}", dir.display()))?;
    Ok(dir)
}

// ============================================================================
// Content stores (both scopes share one shape)
// ============================================================================

/// Which store a piece of content belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreScope {
    /// A project, named by its root (absolute) — what the resolver returns,
    /// canonical when recorded and as written once the project is missing
    /// (ST-23), never the store directory. Its content lives in its folder
    /// under `<home>/projects/` (ST-1), found by [`projects::lookup`].
    Project(PathBuf),
    /// `~/.openalpaca` — the no-project fallback.
    Home,
}

/// Every content collection a store holds — the home store, and each
/// project's folder under `projects/`. Adding a kind here is the *only* way a
/// new content directory comes into existence (§1.3 rule 2). `projects/`
/// itself is not a kind: it holds stores, and only the home root has it
/// ([`projects::PROJECTS_DIR`]). Nor are `config/skills/` (configuration) or
/// `state/cache/` (machine state).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentKind {
    Artifacts,
    Uploads,
    Sessions,
    Memory,
    /// The working folder of a conversation with no project (storage design
    /// §10, ST-10): `scratch/<session-id>/`, made private on first use by a
    /// file tool or a command (`openalpaca_core::tools::working_folder`) and
    /// removed with the conversation. Home store only: a project's working
    /// folder is the project's own.
    Scratch,
}

impl ContentKind {
    /// The directory name for this kind, identical in both scopes.
    pub fn dir_name(self) -> &'static str {
        match self {
            ContentKind::Artifacts => "artifacts",
            ContentKind::Uploads => "uploads",
            ContentKind::Sessions => "sessions",
            ContentKind::Memory => "memory",
            ContentKind::Scratch => "scratch",
        }
    }
}

/// The root directory of a store. Creates nothing.
///
/// Home: [`home_root`]. A project: its folder under `projects/` (ST-1) as
/// [`projects::lookup`] finds it — `<home>/projects/<name>`, canonical. A
/// root with no folder there is [`projects::ProjectError::NotRegistered`]
/// inside the error (R12): a reader asks [`projects::is_not_registered`] and
/// treats the project as empty, and nothing is created for it here.
pub fn store_root(scope: &StoreScope) -> Result<PathBuf> {
    match scope {
        StoreScope::Home => home_root(),
        StoreScope::Project(root) => {
            if !root.is_absolute() {
                bail!(
                    "project store root must be an absolute path, got {}",
                    root.display()
                );
            }
            match projects::lookup(root)? {
                Some(project) => Ok(project.dir),
                None => Err(anyhow::Error::new(projects::ProjectError::NotRegistered {
                    root: root.to_string_lossy().into_owned(),
                })),
            }
        }
    }
}

/// Creates the home root and seeds its metadata: `README.md` and `.layout`
/// (line 2 `install_id=<uuid-v4>`). Idempotent — each file is written only
/// when absent, so user edits stick.
///
/// A project's folder holds `project.json` and content only (R3) and exists
/// from its registration, so for a project scope this is [`store_root`] and
/// nothing else.
pub fn ensure_store(scope: &StoreScope) -> Result<PathBuf> {
    let root = store_root(scope)?;
    if matches!(scope, StoreScope::Project(_)) {
        return Ok(root);
    }
    fs::create_dir_all(&root)
        .with_context(|| format!("Failed to create store root: {}", root.display()))?;
    let readme = root.join(README_FILE);
    if !readme.exists() {
        write_new(&readme, HOME_README)?;
    }
    ensure_layout(&root)?;
    Ok(root)
}

/// `store_root(scope)/<kind>` — created on use (reserved names stay absent
/// until something needs them). A project's folder exists from its
/// registration, so nothing is seeded first; a root with no folder is
/// [`store_root`]'s typed error and nothing is created (R12). For a project
/// the kind is created below its folder and never the folder itself
/// ([`create_below`]): a folder the delete removed (or a re-point renamed)
/// since the lookup is the same typed error, not a folder re-created with
/// no `project.json`.
pub fn content_dir(scope: &StoreScope, kind: ContentKind) -> Result<PathBuf> {
    let root = store_root(scope)?;
    match scope {
        StoreScope::Home => {
            let dir = root.join(kind.dir_name());
            fs::create_dir_all(&dir).with_context(|| {
                format!("Failed to create content directory: {}", dir.display())
            })?;
            Ok(dir)
        }
        StoreScope::Project(project_root) => create_below(&root, Path::new(kind.dir_name()))
            .map_err(|e| {
                project_folder_gone(e, project_root).unwrap_or_else(|e| {
                    anyhow::Error::new(e).context(format!(
                        "Failed to create content directory: {}",
                        root.join(kind.dir_name()).display()
                    ))
                })
            }),
    }
}

/// `base/rel`, created one level at a time and never `base` itself: each
/// missing level with `fs::create_dir`, an existing one taken only when it is
/// a real directory (`symlink_metadata`: no link, no file —
/// `ErrorKind::NotADirectory` otherwise). A `base` that is gone fails with
/// `ErrorKind::NotFound` and creates nothing — the guard for a project's
/// folder under `projects/`, which `create_dir_all` would re-create with no
/// `project.json` for a writer that resolved the project before the delete
/// removed it (R-F3). `rel` must be plain names (`ErrorKind::InvalidInput`
/// otherwise); an empty `rel` answers `base` when it is a directory.
pub(crate) fn create_below(base: &Path, rel: &Path) -> std::io::Result<PathBuf> {
    use std::io::{Error, ErrorKind};
    let real_directory = |dir: &Path| -> std::io::Result<()> {
        if fs::symlink_metadata(dir)?.file_type().is_dir() {
            Ok(())
        } else {
            Err(Error::new(
                ErrorKind::NotADirectory,
                format!(
                    "{} is not a real directory (a link or a file)",
                    dir.display()
                ),
            ))
        }
    };
    let names = rel
        .components()
        .map(|component| match component {
            std::path::Component::Normal(name) => Ok(name),
            _ => Err(Error::new(
                ErrorKind::InvalidInput,
                format!("{} is not a relative path of plain names", rel.display()),
            )),
        })
        .collect::<std::io::Result<Vec<_>>>()?;
    real_directory(base)?;
    let mut dir = base.to_path_buf();
    for name in names {
        dir.push(name);
        match fs::create_dir(&dir) {
            Ok(()) => {}
            Err(e) if e.kind() == ErrorKind::AlreadyExists => real_directory(&dir)?,
            Err(e) => return Err(e),
        }
    }
    Ok(dir)
}

/// [`create_below`]'s `NotFound` under a project's folder as the
/// [`projects::ProjectError::NotRegistered`] every reader of a project scope
/// already handles (R12): the folder went since the lookup. Any other error
/// is handed back.
pub(crate) fn project_folder_gone(
    e: std::io::Error,
    project_root: &Path,
) -> std::result::Result<anyhow::Error, std::io::Error> {
    if e.kind() == std::io::ErrorKind::NotFound {
        Ok(anyhow::Error::new(projects::ProjectError::NotRegistered {
            root: project_root.to_string_lossy().into_owned(),
        }))
    } else {
        Err(e)
    }
}

/// The path string a content row records as its `project_root`; `None` is
/// the home store, which is the address baseline (§4.8).
///
/// The one place a [`StoreScope`] becomes a stored address, so
/// `COALESCE(project_root, '')` means the same thing to every writer. R15:
/// it is the turn's project, never the directory the bytes reached — under
/// ST-1 that is the project's folder under `projects/` for every project.
/// It is the root of the project [`projects::lookup`] finds — the project
/// whose folder [`store_root`] names, so a row and its bytes name one
/// project — and a root no project records is taken by its canonical form.
/// A recorded root is canonical when it is recorded (R14), so for a project
/// whose root still leads to itself this is that form; a missing project's
/// root that no longer canonicalizes to itself (ST-23) is answered as
/// written, never as the folder it now reaches (the lead's ruling R-T19).
/// An error is [`projects::lookup`]'s: a relative root, no home root, or a
/// folder that cannot be resolved.
pub fn project_root_of(scope: &StoreScope) -> Result<Option<String>> {
    match scope {
        StoreScope::Home => Ok(None),
        StoreScope::Project(root) => {
            let address = match projects::lookup(root)? {
                Some(project) => project.root,
                None => root.canonicalize().unwrap_or_else(|_| root.clone()),
            };
            Ok(Some(address.to_string_lossy().to_string()))
        }
    }
}

/// `path` relative to `root`, with `/` separators — the `rel_path` column.
pub fn relative_to(root: &Path, path: &Path) -> Result<String> {
    let rel = path
        .strip_prefix(root)
        .with_context(|| format!("{} is not under {}", path.display(), root.display()))?;
    Ok(rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/"))
}

/// The layout version recorded in `<root>/.layout`, or `None` when the root
/// carries no marker (not a store yet).
pub fn layout_version(root: &Path) -> Result<Option<u32>> {
    let path = root.join(LAYOUT_FILE);
    let text = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(e).with_context(|| format!("Failed to read {}", path.display()));
        }
    };
    let first = text.lines().next().unwrap_or("").trim();
    let version = first
        .parse::<u32>()
        .with_context(|| format!("Malformed layout version in {}: {first:?}", path.display()))?;
    Ok(Some(version))
}

/// The install id recorded on line 2 of the home root's `.layout`, if present.
pub fn install_id(root: &Path) -> Result<Option<String>> {
    Ok(layout_text(root)?.as_deref().and_then(read_install_id))
}

/// `<root>/.layout`'s contents, or `None` when the root carries no marker.
fn layout_text(root: &Path) -> Result<Option<String>> {
    let path = root.join(LAYOUT_FILE);
    match fs::read_to_string(&path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("Failed to read {}", path.display())),
    }
}

// ============================================================================
// Session paths (§5)
// ============================================================================

/// `home_root()/sessions` — every session log lives under the home root, never
/// in a project directory (transcripts must not be git-committable).
///
/// The layout *beneath* this directory (per-session directories, the event log's
/// name, tool-result spill files) is the session pillar's to define; those
/// accessors land with it.
pub fn sessions_dir() -> Result<PathBuf> {
    content_dir(&StoreScope::Home, ContentKind::Sessions)
}

// ============================================================================
// Internals
// ============================================================================

fn create_private_dir(dir: &Path) -> Result<()> {
    fs::create_dir_all(dir)
        .with_context(|| format!("Failed to create directory: {}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // Best effort: don't fail if permissions can't be set.
        let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
    }
    Ok(())
}

fn write_new(path: &Path, contents: &str) -> Result<()> {
    fs::write(path, contents).with_context(|| format!("Failed to write {}", path.display()))
}

/// Write through a sibling temp file and `rename`, so a crash can never leave a
/// half-written file behind.
fn write_atomic(path: &Path, contents: &str) -> Result<()> {
    let mut tmp_name = path
        .file_name()
        .with_context(|| format!("Not a file path: {}", path.display()))?
        .to_os_string();
    tmp_name.push(".tmp");
    let tmp = path.with_file_name(tmp_name);

    let mut file =
        fs::File::create(&tmp).with_context(|| format!("Failed to create {}", tmp.display()))?;
    use std::io::Write;
    file.write_all(contents.as_bytes())
        .and_then(|()| file.sync_all())
        .with_context(|| format!("Failed to write {}", tmp.display()))?;
    drop(file);

    fs::rename(&tmp, path).with_context(|| format!("Failed to move {} into place", tmp.display()))
}

/// Writes the home root's `.layout` when absent — line 1 the layout version,
/// line 2 `install_id=<uuid>` — and repairs an unreadable version line.
/// Nothing is ever added to a marker that exists: it keeps whatever keys it
/// has, and any other line already there is carried through untouched.
fn ensure_layout(root: &Path) -> Result<()> {
    let path = root.join(LAYOUT_FILE);
    let existing = match fs::read_to_string(&path) {
        Ok(t) => Some(t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e).with_context(|| format!("Failed to read {}", path.display())),
    };

    let Some(text) = existing else {
        let fresh = format!(
            "{LAYOUT_VERSION}\n{INSTALL_ID_KEY}={}\n",
            uuid::Uuid::new_v4()
        );
        return write_atomic(&path, &fresh);
    };

    // Line 1 is the layout version. Something that is not an integer is not a
    // version — repair it rather than preserve it and keep failing every read.
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    if lines
        .first()
        .is_some_and(|first| first.trim().parse::<u32>().is_ok())
    {
        return Ok(());
    }
    tracing::warn!(
        "Repairing the layout marker in {}: line 1 was not a version",
        path.display()
    );
    match lines.first_mut() {
        Some(first) => *first = LAYOUT_VERSION.to_string(),
        None => lines.push(LAYOUT_VERSION.to_string()),
    }
    let mut out = lines.join("\n");
    out.push('\n');
    write_atomic(&path, &out)
}

fn read_install_id(layout: &str) -> Option<String> {
    read_layout_value(layout, INSTALL_ID_KEY)
}

/// One `key=value` line of a `.layout` marker, from line 2 onwards (line 1 is
/// the version).
fn read_layout_value(layout: &str, key: &str) -> Option<String> {
    layout
        .lines()
        .skip(1)
        .find_map(|line| line.trim().strip_prefix(&format!("{key}=")))
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

const HOME_README: &str = r#"# OpenAlpaca — home store

Created and maintained by OpenAlpaca. The rule for this directory: **`state/` is
the machine's; everything else here is yours.**

A factory reset (`openalpaca config reset --factory`) deletes the database
inside `state/` and nothing else there — the master key, the memory control
snapshots, the search index, the logs and the backups stay. Deleting a content
directory loses those files only.

| Entry | Holds | Retention class |
|---|---|---|
| `state/` | database (+ WAL/SHM), `discovery.json`, `openalpacad.lock`, `.master_key`, `repl_history` (the line history of `openalpaca chat`) | never swept — a factory reset deletes the database here and leaves the rest |
| `state/memory-control/` | one control snapshot per memory partition: the revision the service last wrote, each note's origin, the tombstones of forgotten notes | never swept — the notes' authority and forget history; a backup of `memory/` includes it |
| `state/backups/` | rotated copies of hand-edited config (`<name>.bak.<ts>`, `<name>.unparseable-<ts>`) | regenerable — swept freely; never user-edited |
| `state/logs/` | `daemon.log`, `gui.log` | regenerable — swept freely |
| `state/cache/` | derived data the machine can rebuild — `memory-index-v1.sqlite`, the memory search index | regenerable — deleting it costs one rebuild from the notes on the next start |
| `config/` | your runtime config: `llm.toml`, `daemon.toml`, `mcp.toml`, `agents/`, `skills/`, `orchestrator/`, `tools/` | yours — never swept |
| `plugins/` | plugin directories you dropped in, `.permissions.toml`, `.config/<name>.toml`, `.data/<name>/` | yours — never swept |
| `projects/` | one folder per project you work in, named after the project's path: `project.json` (the project's id and the path of its own folder) and that project's `artifacts/` (its by-products: drafts, pictures, things handed between agents), `uploads/` and `memory/`, laid out like the entries below. OpenAlpaca's stores write nothing inside the project's own folder: the files an agent writes or edits there are real files of the project, each write asking you first. OpenAlpaca keeps no versions of them: before an agent replaces or edits a file, the content it had is copied into that conversation's log (`sessions/<id>/snapshots/`, size-capped like the rest of the log), and any longer history is the project's own — git's, where the project is a repository | yours — never swept; a factory reset leaves it; `openalpaca store purge <project>` deletes one project's folder with the rest of its data, and never the project's own files |
| `artifacts/` | files produced by tasks that had no project; under `kept/`, a deleted project's files that another conversation still shows | never garbage-collected |
| `uploads/` | files you uploaded that carried no project signal; under `kept/`, a deleted project's uploads that another conversation still shows | swept: an upload attached to no message is deleted once past the grace period |
| `sessions/` | session event logs — all sessions live here, never in a project | size-capped; no age sweep yet (owner decision T12) |
| `memory/` | your memory notes outside any project, per owner: `owners/<owner-key>/notes/*.md`, with `.history/` (the last five service-written revisions) and `MEMORY.md` (a rebuildable index) beside them | yours — never swept; a factory reset leaves them, orphaned until the new owner adopts them |
| `scratch/` | one working folder per conversation with no project, `scratch/<conversation id>/`: where its agents read and list files and run shell commands, made the first time a tool works there (a file they save with `file_write` is a by-product under `artifacts/`) | removed with its conversation — deleting it, or deleting the data of the project it was later bound to; clearing or archiving it keeps the folder, and nothing else sweeps it |

Directories OpenAlpaca did not create are never touched and never swept.

`.layout` records this store's layout version and its install id. Do not edit it.
"#;

#[cfg(test)]
pub(crate) mod tests;

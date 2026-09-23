//! The single source of truth for every OpenAlpaca path.
//!
//! There is one root for app state and no-project content — `~/.openalpaca`
//! (`home_root()`, overridable with `OPENALPACA_HOME_STORE`) — and one store per
//! project — `<project>/.openalpaca`. Both roots have exactly the same shape for
//! content, so `content_dir(scope, kind)` is `root/<kind>` in both cases.
//!
//! ```text
//! ~/.openalpaca/
//!   README.md          seeded once; explains every entry (refreshed only while
//!                      it is an unedited earlier seeding)
//!   .layout            line 1: layout version; line 2 (home root only): install_id=<uuid-v4>
//!   state/             MACHINE STATE — opaque, never user-edited, never committed
//!     openalpaca.db (+ -wal, -shm), discovery.json, openalpacad.lock, .master_key
//!     backups/         rotated copies of hand-edited config files
//!     logs/            daemon.log, gui.log
//!   config/            USER-EDITED runtime config (GUI/CLI-managed daemons)
//!   plugins/           user-dropped plugin dirs + .permissions.toml
//!   artifacts/ uploads/ sessions/ …   content store, home scope
//! ```
//!
//! The organising rule: **`state/` is the machine's; everything else at the root
//! is the human's.** A new content kind exists when, and only when, it is added
//! to [`ContentKind`] — no crate ever joins a literal directory name onto a store
//! root.

mod artifact;
pub mod legacy_root;
pub mod project_move;

pub use artifact::{
    artifact_extension, artifact_file_name, confine_to_root, leading_sequence, loose_dir, run_dir,
    slugify, upload_dir, upload_file_name, version_file_path,
};

use anyhow::{Context, Result, bail};
use directories::BaseDirs;
use std::fs;
use std::path::{Path, PathBuf};

/// Environment override for the home root (D4). Absolute paths only.
pub const HOME_STORE_ENV: &str = "OPENALPACA_HOME_STORE";

/// The store directory name — `~/.openalpaca` and `<project>/.openalpaca`.
pub const STORE_DIR_NAME: &str = ".openalpaca";

/// Layout version written to line 1 of `.layout`.
pub const LAYOUT_VERSION: u32 = 1;

/// The database file name under `state/`, so [`database_path`] and
/// [`open_home_database`] cannot drift.
const DB_FILE: &str = "openalpaca.db";

const LAYOUT_FILE: &str = ".layout";
const README_FILE: &str = "README.md";
const GITIGNORE_FILE: &str = ".gitignore";
/// Nested under `artifacts/**/`, never at a store root today — reserved for
/// store metadata anyway (§1.3 rule 1: dot-prefixed names are reserved
/// forever), so a future top-level `.versions` is recognised rather than
/// reported as a name this store did not create.
const VERSIONS_DIR: &str = ".versions";
const INSTALL_ID_KEY: &str = "install_id";
const PROJECT_ROOT_KEY: &str = "project_root";

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
///
/// It also runs [`legacy_root::check_legacy_root_result`] first: opening the
/// database **creates** it, so a process that opens before checking is exactly
/// the process that strands an older install's data.
pub fn open_home_database() -> Result<crate::database::Database> {
    legacy_root::check_legacy_root_result()?;
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
/// *inside* the window stays — a multi-paragraph refusal (the legacy-root one
/// is) must still read as the paragraphs it was written in. Three things are
/// removed, none of them text: trailing blank lines (a log ends in a newline,
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

/// `state/cache/fastembed` — the local embedding model's own cache, created if
/// missing (L11).
///
/// fastembed defaults its cache to `./.fastembed_cache`, **relative to the
/// process's working directory**: the daemon's ~1 GB model download landed
/// wherever it happened to be started from, outside the store, and was
/// downloaded again the next time that differed. Regenerable — deleting it
/// costs one download and nothing else — so it belongs in `state/`, with the
/// rest of what the machine can rebuild.
pub fn embedding_cache_dir() -> Result<PathBuf> {
    let dir = state_dir()?.join(CACHE_DIR).join("fastembed");
    fs::create_dir_all(&dir).with_context(|| {
        format!(
            "Failed to create the embedding cache directory: {}",
            dir.display()
        )
    })?;
    Ok(dir)
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
    /// `<project>/.openalpaca` — the project root itself (absolute).
    Project(PathBuf),
    /// `~/.openalpaca` — the no-project fallback.
    Home,
}

/// Every content collection in the system. Adding a kind here is the *only* way
/// a new top-level store directory comes into existence (§1.3 rule 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentKind {
    Artifacts,
    Uploads,
    Sessions,
    Memory,
    Skills,
    Scratch,
    Cache,
}

impl ContentKind {
    /// Every kind there is. The list a caller walks when it has to reason about
    /// the whole store rather than one collection — `store purge`, which must
    /// tell a name the store created from one it did not (§1.3 rule 3).
    pub const ALL: [ContentKind; 7] = [
        ContentKind::Artifacts,
        ContentKind::Uploads,
        ContentKind::Sessions,
        ContentKind::Memory,
        ContentKind::Skills,
        ContentKind::Scratch,
        ContentKind::Cache,
    ];

    /// The directory name for this kind, identical in both scopes.
    pub fn dir_name(self) -> &'static str {
        match self {
            ContentKind::Artifacts => "artifacts",
            ContentKind::Uploads => "uploads",
            ContentKind::Sessions => "sessions",
            ContentKind::Memory => "memory",
            ContentKind::Skills => "skills",
            ContentKind::Scratch => "scratch",
            ContentKind::Cache => "cache",
        }
    }
}

/// The root directory of a store. Does not create anything.
pub fn store_root(scope: &StoreScope) -> Result<PathBuf> {
    match scope {
        StoreScope::Home => home_root(),
        StoreScope::Project(project) => {
            if !project.is_absolute() {
                bail!(
                    "project store root must be an absolute path, got {}",
                    project.display()
                );
            }
            Ok(project.join(STORE_DIR_NAME))
        }
    }
}

/// Creates the store root and seeds its metadata: `README.md`, `.layout`
/// (and, for a project store, `.gitignore`). Idempotent — each file is written
/// only when absent, so user edits stick. The one exception is a home
/// `README.md` byte-identical to a text an earlier build seeded
/// ([`SUPERSEDED_HOME_READMES`]): nobody edited it, it is ours, and it is
/// brought up to date.
///
/// On the home root, `.layout` line 2 carries `install_id=<uuid-v4>`, written
/// once and never rewritten.
///
/// Which metadata is seeded follows the **resolved root**, not the scope
/// variant: `Project($HOME)`'s store root *is* `~/.openalpaca`, and so is a
/// project whose `.openalpaca` is a symlink to it. Seeding those from the
/// project branch put a `.gitignore` in the home root and a README describing a
/// project store — the same fold `resolves_to_the_home_store` applies upstream.
pub fn ensure_store(scope: &StoreScope) -> Result<PathBuf> {
    let root = store_root(scope)?;
    fs::create_dir_all(&root)
        .with_context(|| format!("Failed to create store root: {}", root.display()))?;

    let is_home = is_the_home_root(&root);

    let readme = root.join(README_FILE);
    if !readme.exists() {
        write_new(&readme, readme_text(is_home))?;
    } else if is_home && is_superseded_home_readme(&readme) {
        // Best-effort: the README is documentation, and `ensure_store` runs
        // before the daemon's singleton lock (and in the GUI and the CLI), so
        // two launchers can refresh it at once. The loser's rename finds its
        // shared `README.md.tmp` already moved into place by the winner; that
        // must never stop a boot.
        match write_atomic(&readme, HOME_README) {
            Ok(()) => tracing::info!(
                "Updated {}: it was an earlier build's text, unedited",
                readme.display()
            ),
            Err(e) => tracing::warn!(
                "Could not refresh {} (an earlier build's text): {e:#}",
                readme.display()
            ),
        }
    }

    if !is_home {
        let gitignore = root.join(GITIGNORE_FILE);
        if !gitignore.exists() {
            write_new(&gitignore, GITIGNORE_TEXT)?;
        }
    }

    ensure_layout(&root, is_home)?;
    Ok(root)
}

/// Whether a store root *is* the home root — canonicalized, so a symlinked
/// `<project>/.openalpaca` answers for the store it reaches, the same rule
/// [`project_root_at`] addresses rows by. Paths that cannot be canonicalized
/// (nothing there yet) are compared as written; with no home directory to
/// compare against the answer is `false`, which leaves the root alone.
fn is_the_home_root(root: &Path) -> bool {
    let Ok(home) = home_root() else {
        return false;
    };
    let resolve = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    resolve(root) == resolve(&home)
}

/// Top-level names inside a store root that this store did not create, sorted.
///
/// §1.3 rule 3: unknown directories found in a store root are left untouched and
/// never swept — the store never deletes what it did not create. `store purge`
/// is the first caller that has to *say* so, entry by entry, and this is the
/// list it names. Known names, in **either** scope:
/// - the [`ContentKind`] directories;
/// - the store's own metadata — `.layout`, `README.md`, `.gitignore`, and
///   `.versions` (§1.3 rule 1 reserves every dot-prefixed name forever, even
///   one nothing writes at a store root today);
/// - `config` — reserved in the **project** README too (`memory/`, `skills/`,
///   `config/` — "not created until used"), even though it holds something
///   different there than the home root's real `config/`. Without this, a
///   project that actually has a `<project>/.openalpaca/config/` was reported
///   twice in a purge plan: once by the reserved-names keep line, once again
///   here.
///
/// Known **only when `is_home` is true**: `state` and `plugins`, the two
/// names the **home** root alone owns beside its content dirs. Those are
/// home-root names, not store metadata or a project reservation: a project
/// root has no `state/` of its own, so a user directory that happens to be
/// called `state/` (or `plugins/`) inside `<project>/.openalpaca/` is exactly
/// the kind of name this list exists to report, not a known one to wave
/// through.
///
/// A root that does not exist, or cannot be read, has nothing to report: the
/// answer is empty rather than an error, because "what else is in there" is a
/// remark on a plan and never the reason to refuse one.
pub fn unknown_entries(store_root: &Path, is_home: bool) -> Vec<String> {
    let mut known: Vec<&str> = ContentKind::ALL
        .iter()
        .map(|kind| kind.dir_name())
        .chain([LAYOUT_FILE, README_FILE, GITIGNORE_FILE, VERSIONS_DIR, "config"])
        .collect();
    if is_home {
        known.extend(["state", "plugins"]);
    }
    let Ok(entries) = fs::read_dir(store_root) else {
        return Vec::new();
    };
    let mut out: Vec<String> = entries
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| !known.contains(&name.as_str()))
        .collect();
    out.sort();
    out
}

/// `store_root(scope)/<kind>` — created on use (reserved names stay absent until
/// something actually needs them).
///
/// A project store is seeded first: `uploads/` must never exist before the
/// `.gitignore` that excludes it, or the first upload lands in the user's git
/// index. The home root has no `.gitignore` and nothing to race, so it is left
/// to the explicit `ensure_store` at boot.
pub fn content_dir(scope: &StoreScope, kind: ContentKind) -> Result<PathBuf> {
    let root = match scope {
        StoreScope::Project(_) => ensure_store(scope)?,
        StoreScope::Home => store_root(scope)?,
    };
    let dir = root.join(kind.dir_name());
    fs::create_dir_all(&dir)
        .with_context(|| format!("Failed to create content directory: {}", dir.display()))?;
    Ok(dir)
}

/// The canonical-path string a content row records as its `project_root`;
/// `None` is the home store, which is the address baseline (§4.8).
///
/// The one place a [`StoreScope`] becomes a stored address, so
/// `COALESCE(project_root, '')` means the same thing to every writer.
pub fn project_root_of(scope: &StoreScope) -> Result<Option<String>> {
    match scope {
        StoreScope::Home => Ok(None),
        StoreScope::Project(root) => {
            let canonical = root.canonicalize().unwrap_or_else(|_| root.clone());
            Ok(Some(canonical.to_string_lossy().to_string()))
        }
    }
}

/// [`project_root_of`] for bytes that have already been placed: the address
/// component for a store whose root is `store_root` — *canonicalized*, i.e. the
/// directory the bytes actually landed under rather than the path the caller
/// named.
///
/// `None` when that is the home root (the address baseline); otherwise the
/// directory holding the store. Deriving the address from the resolved root is
/// what keeps placement identity and address identity the same thing: a
/// `<project>/.openalpaca` symlinked at another store resolves to *that* store's
/// address, so it cannot open a second sequence space over a directory that
/// already has one. [`confine_to_root`] cannot see that case — the symlink is
/// *at* the root it canonicalizes, not under it.
pub fn project_root_at(store_root: &Path) -> Result<Option<String>> {
    let home = home_root()?;
    let home = home.canonicalize().unwrap_or(home);
    if store_root == home {
        return Ok(None);
    }
    let project = store_root
        .parent()
        .with_context(|| format!("Store root has no parent: {}", store_root.display()))?;
    Ok(Some(project.to_string_lossy().to_string()))
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

/// The project root a **project store** recorded for itself when it was first
/// seeded — §4.8's "Project moved" made answerable.
///
/// `root` is the store directory (`<project>/.openalpaca`). The value is the
/// canonical path of the project the store belonged to *then*, so a store whose
/// directory has since been moved reports the old path and a re-base has
/// something exact to offer. `None` means no store, no marker, or a store
/// seeded before this key existed — all of which read as "nothing to say", not
/// as "not moved by this much".
pub fn recorded_project_root(root: &Path) -> Result<Option<String>> {
    Ok(layout_text(root)?
        .as_deref()
        .and_then(|text| read_layout_value(text, PROJECT_ROOT_KEY)))
}

/// Rewrite a project store's recorded root — the one thing that may, because a
/// re-base is precisely the statement that the store now lives somewhere else.
///
/// Writes the whole marker atomically, preserving every other line.
pub fn set_recorded_project_root(root: &Path, project_root: &str) -> Result<()> {
    let path = root.join(LAYOUT_FILE);
    let text = layout_text(root)?.unwrap_or_else(|| format!("{LAYOUT_VERSION}\n"));
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let line = format!("{PROJECT_ROOT_KEY}={project_root}");
    match lines
        .iter_mut()
        .skip(1)
        .find(|l| l.trim().starts_with(&format!("{PROJECT_ROOT_KEY}=")))
    {
        Some(existing) => *existing = line,
        None => lines.push(line),
    }
    let mut out = lines.join("\n");
    out.push('\n');
    write_atomic(&path, &out)
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

/// Writes `.layout` when absent; repairs an unreadable version line; on the home
/// root, appends `install_id=<uuid>` exactly once if an existing marker predates
/// it, and on a project root `project_root=<canonical path>` on the same terms.
/// Any other line already there is carried through untouched.
///
/// **`project_root=` is written once and never rewritten**, which is what makes
/// it the record of where this store *was* (§4.8's "Project moved"): `ensure_store`
/// runs on every `content_dir` call, so a line that healed itself to the current
/// path would erase the very difference a moved project is recognised by. The
/// one thing that rewrites it is a re-base, through
/// [`set_recorded_project_root`].
fn ensure_layout(root: &Path, is_home: bool) -> Result<()> {
    let path = root.join(LAYOUT_FILE);
    let existing = match fs::read_to_string(&path) {
        Ok(t) => Some(t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e).with_context(|| format!("Failed to read {}", path.display())),
    };

    // The address this store would record for itself, resolved the way every
    // stored `project_root` is (canonical, home root folded to `None`).
    let recorded = match is_home {
        true => None,
        false => {
            let canonical = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
            project_root_at(&canonical)?
        }
    };

    let Some(text) = existing else {
        let mut fresh = format!("{LAYOUT_VERSION}\n");
        if is_home {
            fresh.push_str(&format!("{INSTALL_ID_KEY}={}\n", uuid::Uuid::new_v4()));
        }
        if let Some(project_root) = &recorded {
            fresh.push_str(&format!("{PROJECT_ROOT_KEY}={project_root}\n"));
        }
        return write_atomic(&path, &fresh);
    };

    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let mut changed = false;

    // Line 1 is the layout version. Something that is not an integer is not a
    // version — repair it rather than preserve it and keep failing every read.
    if lines
        .first()
        .is_none_or(|first| first.trim().parse::<u32>().is_err())
    {
        tracing::warn!(
            "Repairing the layout marker in {}: line 1 was not a version",
            path.display()
        );
        match lines.first_mut() {
            Some(first) => *first = LAYOUT_VERSION.to_string(),
            None => lines.push(LAYOUT_VERSION.to_string()),
        }
        changed = true;
    }

    // Written once, never rewritten: an existing id is left alone.
    if is_home && read_install_id(&text).is_none() {
        lines.push(format!("{INSTALL_ID_KEY}={}", uuid::Uuid::new_v4()));
        changed = true;
    }

    // Likewise the recorded root. A store that predates this key adopts its
    // *current* path — nothing recorded where it used to be, so the honest
    // answer for a project moved before this shipped is "not moved", not a
    // guess.
    if let Some(project_root) = &recorded
        && read_layout_value(&text, PROJECT_ROOT_KEY).is_none()
    {
        lines.push(format!("{PROJECT_ROOT_KEY}={project_root}"));
        changed = true;
    }

    if !changed {
        return Ok(());
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

const GITIGNORE_TEXT: &str = "\
/.layout
/uploads/
/sessions/
/scratch/
/cache/
.versions/
";

const HOME_README: &str = r#"# OpenAlpaca — home store

Created and maintained by OpenAlpaca. The rule for this directory: **`state/` is
the machine's; everything else here is yours.**

A factory reset (`openalpaca config reset --factory`) deletes the database
inside `state/` and nothing else there — the master key, the embedding cache,
the logs and the backups stay. Deleting a content directory loses those files
only.

| Entry | Holds | Retention class |
|---|---|---|
| `state/` | database (+ WAL/SHM), `discovery.json`, `openalpacad.lock`, `.master_key` | never swept — a factory reset deletes the database here and leaves the rest |
| `state/backups/` | rotated copies of hand-edited config (`<name>.bak.<ts>`, `<name>.unparseable-<ts>`) | regenerable — swept freely; never user-edited |
| `state/logs/` | `daemon.log`, `gui.log` | regenerable — swept freely |
| `state/cache/` | derived data the machine can rebuild — `fastembed/` holds the local embedding model (~1 GB) | regenerable — deleting it costs a re-download |
| `config/` | your runtime config: `llm.toml`, `daemon.toml`, `mcp.toml`, `agents/`, `skills/`, `orchestrator/`, `tools/` | yours — never swept |
| `plugins/` | plugin directories you dropped in, `.permissions.toml`, `.config/<name>.toml`, `.data/<name>/` | yours — never swept |
| `artifacts/` | files produced by tasks that had no project | never garbage-collected |
| `uploads/` | files you uploaded that carried no project signal | swept: an upload attached to no message is deleted once past the grace period |
| `sessions/` | session event logs — all sessions live here, never in a project | size-capped; no age sweep yet (owner decision T12) |
| `scratch/`, `cache/` | reserved; agent working space and derived data | swept freely |
| `memory/`, `skills/` | reserved; not created until used | yours — never swept |

Directories OpenAlpaca did not create are never touched and never swept.

`.layout` records this store's layout version and its install id. Do not edit it.
"#;

const PROJECT_README: &str = r#"# OpenAlpaca — project store

Created and maintained by OpenAlpaca for this project. Machine state (database,
keys, logs, sessions) never lives here — it stays in the home store,
`~/.openalpaca`.

Deleting a directory below loses those files only.

| Entry | Holds | Retention class |
|---|---|---|
| `artifacts/` | files produced by tasks run in this project | never garbage-collected; heads are committable |
| `artifacts/**/.versions/` | previous versions of a produced file | OpenAlpaca's private history; git-ignored |
| `uploads/` | copies of files you uploaded here | swept: an upload attached to no message is deleted once past the grace period; git-ignored |
| `sessions/` | reserved, deliberately unused — session logs live in the home store | — |
| `memory/`, `skills/`, `config/` | reserved; not created until used | yours — never swept, and deliberately *not* git-ignored |
| `scratch/`, `cache/` | reserved; agent working space and derived data | swept freely; git-ignored |

Directories OpenAlpaca did not create are never touched and never swept.

`.gitignore` is store-owned and committable, so the rules travel with the repo;
it is written only when absent, so your edits stick. `.layout` records this
store's layout version — do not edit it.
"#;

/// Every home README an earlier build seeded, byte for byte, oldest first —
/// recovered from the history of [`HOME_README`]. Each one says that deleting
/// `state/` is a factory reset, which taken literally destroys `.master_key`
/// and the ~1 GB embedding model; `openalpaca config reset --factory` deletes
/// the database and nothing else there. A seeded README is written only when
/// absent, so without this every existing store would keep that sentence.
///
/// When [`HOME_README`] changes, its previous text is appended here, so a
/// store seeded by the build before keeps being refreshed.
const SUPERSEDED_HOME_READMES: [&str; 5] = [
    include_str!("superseded_readmes/home-1.txt"),
    include_str!("superseded_readmes/home-2.txt"),
    include_str!("superseded_readmes/home-3.txt"),
    include_str!("superseded_readmes/home-4.txt"),
    include_str!("superseded_readmes/home-5.txt"),
];

/// Whether `path` holds exactly one of [`SUPERSEDED_HOME_READMES`]. Anything
/// else — an edit, an unreadable file, a directory — is the user's and is
/// left alone. The length is checked first, so a large file is never read.
fn is_superseded_home_readme(path: &Path) -> bool {
    let Ok(meta) = fs::metadata(path) else {
        return false;
    };
    if !meta.is_file()
        || !SUPERSEDED_HOME_READMES
            .iter()
            .any(|old| old.len() as u64 == meta.len())
    {
        return false;
    }
    fs::read(path).is_ok_and(|bytes| {
        SUPERSEDED_HOME_READMES
            .iter()
            .any(|old| old.as_bytes() == bytes.as_slice())
    })
}

fn readme_text(is_home: bool) -> &'static str {
    if is_home { HOME_README } else { PROJECT_README }
}

#[cfg(test)]
pub(crate) mod tests;

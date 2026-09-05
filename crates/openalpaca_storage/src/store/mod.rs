//! The single source of truth for every OpenAlpaca path.
//!
//! There is one root for app state and no-project content — `~/.openalpaca`
//! (`home_root()`, overridable with `OPENALPACA_HOME_STORE`) — and one store per
//! project — `<project>/.openalpaca`. Both roots have exactly the same shape for
//! content, so `content_dir(scope, kind)` is `root/<kind>` in both cases.
//!
//! ```text
//! ~/.openalpaca/
//!   README.md          seeded once; explains every entry
//!   .layout            line 1: layout version; line 2 (home root only): install_id=<uuid-v4>
//!   state/             MACHINE STATE — opaque, never user-edited, never committed
//!     openalpaca.db (+ -wal, -shm), discovery.json, openalpacad.lock, .master_key
//!     assets/          interim home for content-addressed uploads (until the D2 re-home)
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

pub mod migrate;

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

const LAYOUT_FILE: &str = ".layout";
const README_FILE: &str = "README.md";
const GITIGNORE_FILE: &str = ".gitignore";
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
    state_dir_in(&home_root()?)
}

/// [`state_dir`] under an explicit root — for the mover, which works on the two
/// roots it was given rather than on the ambient one.
pub(crate) fn state_dir_in(root: &Path) -> Result<PathBuf> {
    let dir = root.join("state");
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
    Ok(state_dir_path()?.join("openalpaca.db"))
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
    let dir = state_dir()?.join("logs");
    fs::create_dir_all(&dir)
        .with_context(|| format!("Failed to create logs directory: {}", dir.display()))?;
    Ok(dir)
}

/// `state/backups` — created if missing. The atomic config writer's rotation target.
pub fn backups_dir() -> Result<PathBuf> {
    let dir = state_dir()?.join("backups");
    fs::create_dir_all(&dir)
        .with_context(|| format!("Failed to create backups directory: {}", dir.display()))?;
    Ok(dir)
}

/// `state/assets` — the interim home for content-addressed upload blobs.
///
/// Dies with the D2 re-home into `uploads/` (Phase 8); nothing new should be
/// designed against it.
pub fn interim_assets_dir() -> Result<PathBuf> {
    Ok(state_dir_path()?.join("assets"))
}

/// Sharded path for an upload blob under [`interim_assets_dir`], e.g.
/// `state/assets/ab/cd/abcd…`.
///
/// Interim, like the directory it sits in: under D2 new uploads become
/// human-named (`uploads/<date>/NN-<name>.<ext>`) and dedup keys off the
/// `file_assets.sha256` column, at which point this function and
/// [`interim_assets_dir`] are deleted together (Phase 8). It exists here only so
/// the two upload writers keep producing the layout that
/// [`migrate::rebase_asset_paths`] rebases.
pub fn interim_asset_storage_path(sha256: &str) -> Result<PathBuf> {
    if sha256.len() < 4 {
        bail!("SHA-256 hash too short: {sha256}");
    }
    Ok(interim_assets_dir()?
        .join(&sha256[0..2])
        .join(&sha256[2..4])
        .join(sha256))
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
/// only when absent, so user edits stick.
///
/// On the home root, `.layout` line 2 carries `install_id=<uuid-v4>`, written
/// once and never rewritten.
pub fn ensure_store(scope: &StoreScope) -> Result<PathBuf> {
    let root = store_root(scope)?;
    fs::create_dir_all(&root)
        .with_context(|| format!("Failed to create store root: {}", root.display()))?;

    let is_home = matches!(scope, StoreScope::Home);

    let readme = root.join(README_FILE);
    if !readme.exists() {
        write_new(&readme, readme_text(is_home))?;
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
    let path = root.join(LAYOUT_FILE);
    let text = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(e).with_context(|| format!("Failed to read {}", path.display()));
        }
    };
    Ok(read_install_id(&text))
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
/// it. Any other line already there is carried through untouched — a project
/// root's `project_id=` is not this function's to drop.
fn ensure_layout(root: &Path, is_home: bool) -> Result<()> {
    let path = root.join(LAYOUT_FILE);
    let existing = match fs::read_to_string(&path) {
        Ok(t) => Some(t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e).with_context(|| format!("Failed to read {}", path.display())),
    };

    let Some(text) = existing else {
        let mut fresh = format!("{LAYOUT_VERSION}\n");
        if is_home {
            fresh.push_str(&format!("{INSTALL_ID_KEY}={}\n", uuid::Uuid::new_v4()));
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

    if !changed {
        return Ok(());
    }
    let mut out = lines.join("\n");
    out.push('\n');
    write_atomic(&path, &out)
}

fn read_install_id(layout: &str) -> Option<String> {
    layout
        .lines()
        .skip(1)
        .find_map(|line| line.trim().strip_prefix(&format!("{INSTALL_ID_KEY}=")))
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

Deleting `state/` is a factory reset. Deleting a content directory loses those
files only.

| Entry | Holds | Retention class |
|---|---|---|
| `state/` | database (+ WAL/SHM), `discovery.json`, `openalpacad.lock`, `.master_key` | never swept — deleting it is a factory reset |
| `state/assets/` | interim home for uploaded file bytes | never garbage-collected |
| `state/backups/` | rotated copies of hand-edited config (`<name>.bak.<ts>`, `<name>.unparseable-<ts>`) | regenerable — swept freely; never user-edited |
| `state/logs/` | `daemon.log`, `gui.log` | regenerable — swept freely |
| `config/` | your runtime config: `llm.toml`, `daemon.toml`, `mcp.toml`, `agents/`, `skills/`, `orchestrator/`, `tools/` | yours — never swept |
| `plugins/` | plugin directories you dropped in, `.permissions.toml`, `.config/<name>.toml`, `.data/<name>/` | yours — never swept |
| `artifacts/` | files produced by tasks that had no project | never garbage-collected |
| `uploads/` | files you uploaded that carried no project signal | never garbage-collected |
| `sessions/` | session event logs — all sessions live here, never in a project | size-capped, optional age sweep |
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
| `uploads/` | copies of files you uploaded here | never garbage-collected; git-ignored |
| `sessions/` | reserved, deliberately unused — session logs live in the home store | — |
| `memory/`, `skills/`, `config/` | reserved; not created until used | yours — never swept, and deliberately *not* git-ignored |
| `scratch/`, `cache/` | reserved; agent working space and derived data | swept freely; git-ignored |

Directories OpenAlpaca did not create are never touched and never swept.

`.gitignore` is store-owned and committable, so the rules travel with the repo;
it is written only when absent, so your edits stick. `.layout` records this
store's layout version — do not edit it.
"#;

fn readme_text(is_home: bool) -> &'static str {
    if is_home { HOME_README } else { PROJECT_README }
}

#[cfg(test)]
mod tests;

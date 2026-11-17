//! The content stores' paths, the home store's and each project's.
//!
//! There is one root — `~/.openalpaca` (`openalpaca_app_support::paths::home_root`,
//! overridable with `OPENALPACA_HOME_STORE`). App state and content with no
//! project live there,
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
//!     logs/            daemon.log, gui.log (the app's), gui-dev.log (a dev launch's)
//!     cache/           derived data: the memory search index; open/<file id>/, Open's copies
//!     editor/          the CLI's editor buffers (`memory add|edit`, `persona edit user`)
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
//!
//! The root's own paths — `state/` and what is in it, the logs, `config/`,
//! `plugins/`, the README and `.layout` — are `openalpaca_app_support::paths`
//! (Q05); this module keeps the content stores, the projects and Open's
//! copies, and imports those.

mod artifact;
mod open_copies;
pub mod projects;
mod reveal;

pub use artifact::{
    FILE_NAME_SLUG_BYTES, artifact_extension, artifact_file_name, confine_to_root,
    kind_for_file_name, leading_sequence, loose_dir, opens_in_default_app, run_dir, slugify,
    upload_dir, upload_file_name, version_file_path,
};
pub use open_copies::{open_copies_dir, remove_open_copy, stage_open_copy, sweep_open_copies};
pub use reveal::{RevealRefusal, revealable_path};

use anyhow::{Context, Result, bail};
use openalpaca_app_support::paths::{database_path, ensure_home_store, home_root, state_dir};
use std::fs;
use std::path::{Path, PathBuf};

/// Opens this install's database, creating `state/` (0700 on unix) if it is not
/// there — the ordering every process outside the daemon's boot preamble needs.
///
/// The daemon makes `state/` itself, long before it opens anything
/// (`discovery::acquire_single_instance_lock` calls `state_dir`). Every other
/// process — `openalpaca config`, the connector examples — has no such step, so
/// this is where the private directory is guaranteed. Going through
/// `database_path` alone would let SQLite create `state/` at the process
/// umask, leaving `.master_key` and `discovery.json` world-readable.
pub fn open_home_database() -> Result<crate::database::Database> {
    state_dir()?;
    let path = database_path()?;
    crate::database::Database::open(&path)
        .with_context(|| format!("Failed to open {}", path.display()))
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

/// The store's root, made: the home root with its `README.md` and `.layout`
/// seeded ([`ensure_home_store`], idempotent — each file is written only when
/// absent, so user edits stick).
///
/// A project's folder holds `project.json` and content only (R3) and exists
/// from its registration, so for a project scope this is [`store_root`] and
/// nothing else.
pub fn ensure_store(scope: &StoreScope) -> Result<PathBuf> {
    match scope {
        StoreScope::Home => ensure_home_store(),
        StoreScope::Project(_) => store_root(scope),
    }
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
/// removed it (R-F3), and for a project's own root, which `file_write` makes
/// its missing folders under (the lead's ruling R-T30: a root moved away
/// mid-write is never re-made). `rel` must be plain names
/// (`ErrorKind::InvalidInput` otherwise); an empty `rel` answers `base` when
/// it is a directory.
pub fn create_below(base: &Path, rel: &Path) -> std::io::Result<PathBuf> {
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

#[cfg(test)]
pub(crate) mod tests;

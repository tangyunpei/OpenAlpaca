//! The on-disk half of GAP-24 — install, update and uninstall of a plugin
//! *directory*.
//!
//! The extension design owns the T/E sequences these verbs call and the
//! identity rule (the plugin key is the **directory name**, §2.2); this module
//! owns the ordering on disk, and nothing else. It holds no supervisor state,
//! spawns nothing and never touches `.permissions.toml` — [`PluginManager`]
//! composes it with those.
//!
//! Three rules run through it.
//!
//! 1. **Nothing is ever half-copied into place.** A copy lands in
//!    `<root>/.staging/<name>.<pid>.<stamp>/` first and becomes `plugins/<name>`
//!    with a single `rename`. A [`Staged`] that is dropped without
//!    [`Staged::commit`] — an error, an early return, a panic — sweeps itself,
//!    so a crash can leave a staging copy behind but never a partial plugin the
//!    next boot scan would treat as real.
//! 2. **A user-dropped directory is never deleted** (§1.3 rule 3). Replacing or
//!    uninstalling one *moves* it to `<root>/.trash/<name>-<stamp>/`.
//! 3. **The copy never links out of the store.** A symlink whose target escapes
//!    the source tree is refused outright; one that stays inside is copied as
//!    its target. The child runs with `current_dir(plugin_dir)`
//!    (`process_pool.rs`), so a link out of the tree would be a path the owner
//!    never reviewed.
//! 4. **The copy trusts nothing about the source tree's shape.** Only regular
//!    files and directories are copied — a FIFO would block `fs::copy`'s open
//!    forever — and the walk is bounded by a visited-directory set (a symlink
//!    cycle is refused) plus entry and byte budgets. It runs on a blocking
//!    thread, never on a runtime worker.
//!
//! `.staging`, `.trash` and `.data` are invisible to the scan by construction:
//! [`PluginManager::plugin_directories`] takes the root's immediate children
//! that hold a `plugin.toml`, and these hold their copies one level deeper.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use serde::Serialize;

use crate::manifest::PluginManifest;

/// Where a copy waits until it can be renamed into place.
pub const STAGING_DIR: &str = ".staging";
/// Where a replaced or uninstalled directory goes. Never `rm -rf`.
pub const TRASH_DIR: &str = ".trash";
/// A plugin's own data, which survives an update and — by default — an
/// uninstall (`keep_data`).
pub const DATA_DIR: &str = ".data";

/// How deep the copy will walk before it decides the tree is pathological.
const MAX_DEPTH: usize = 32;

// ============================================================================
// The approval preview
// ============================================================================

/// What a `plugin.toml` declares, read **before** anything is copied.
///
/// This is the approval preview the install route returns beside the resulting
/// `unapproved`/`never_seen` row: an install grants nothing, so the owner needs
/// to see what approving would grant while deciding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ManifestSummary {
    /// The directory name the plugin will take — its extension id (§2.2).
    pub name: String,
    pub version: String,
    pub description: String,
    pub entry: String,
    /// `capabilities.provides` — what an `approve` records consent for.
    pub capabilities: Vec<String>,
    /// `capabilities.virtual.provides`.
    pub virtual_capabilities: Vec<String>,
    /// The `[types]` table as declared, in the row's own vocabulary.
    pub types: BTreeMap<String, bool>,
    pub required_config_keys: Vec<String>,
    /// Keys the manifest marks `sensitive`: their values never land in
    /// `plugins/.config/<name>.toml` (X-29).
    pub sensitive_config_keys: Vec<String>,
}

impl ManifestSummary {
    pub fn of(name: &str, manifest: &PluginManifest) -> Self {
        let mut required: Vec<String> = manifest
            .config
            .iter()
            .filter(|(_, field)| field.required)
            .map(|(key, _)| key.clone())
            .collect();
        required.sort();
        let mut sensitive: Vec<String> = manifest
            .config
            .iter()
            .filter(|(_, field)| field.sensitive)
            .map(|(key, _)| key.clone())
            .collect();
        sensitive.sort();

        Self {
            name: name.to_string(),
            version: manifest.plugin.version.clone(),
            description: manifest.plugin.description.clone(),
            entry: manifest.plugin.entry.clone(),
            capabilities: manifest.capabilities.provides.clone(),
            virtual_capabilities: manifest.capabilities.virtual_.provides.clone(),
            types: BTreeMap::from([
                ("tool".to_string(), manifest.types.tools),
                ("connector".to_string(), manifest.types.connector),
                ("provider".to_string(), manifest.types.provider),
                ("skill".to_string(), manifest.types.skill),
                ("agent".to_string(), manifest.types.agent),
            ]),
            required_config_keys: required,
            sensitive_config_keys: sensitive,
        }
    }
}

// ============================================================================
// Errors
// ============================================================================

/// Why an install, update or uninstall did not happen.
///
/// Every variant leaves the plugins root exactly as it was. [`Self::code`] is
/// the word the route puts in the flat `{"error": "<word>"}` envelope §8 fixes.
#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    /// Not an absolute path, a reserved name, or a path inside the plugins
    /// root — a caller mistake, `400`.
    #[error("{0}")]
    InvalidPath(String),
    /// The source is not there, or is not a directory — `404`.
    #[error("{0}")]
    SourceNotFound(String),
    /// No `plugin.toml`, one that does not parse, or one whose `plugin.name`
    /// is not its directory name — `422`. The directory is never copied.
    #[error("{0}")]
    InvalidManifest(String),
    /// `plugins/<name>` already exists — `409`.
    #[error("{0}")]
    AlreadyInstalled(String),
    /// A transition is in flight for this extension — `409`.
    #[error("{0}")]
    Busy(String),
    /// A symlink in the source tree resolves outside it — `400`.
    #[error("{0}")]
    EscapingSymlink(String),
    /// The copy, the rename or the trash-move failed — `500`.
    #[error("{0}")]
    Io(String),
    /// A supervisor-level refusal (unknown id, orphaned row, unreadable
    /// store): the code and the status are the extension family's own.
    #[error("{0}")]
    Extension(#[from] openalpaca_core::tools::extensions::ExtensionError),
}

impl InstallError {
    /// The error word. For [`Self::Extension`] it is the extension family's
    /// own `Display`, so `store_unreadable` and `orphaned` read the same here
    /// as on every other verb.
    pub fn code(&self) -> String {
        match self {
            Self::InvalidPath(_) => "invalid_path".to_string(),
            Self::SourceNotFound(_) => "source_not_found".to_string(),
            Self::InvalidManifest(_) => "invalid_manifest".to_string(),
            Self::AlreadyInstalled(_) => "already_installed".to_string(),
            Self::Busy(_) => "busy".to_string(),
            Self::EscapingSymlink(_) => "escaping_symlink".to_string(),
            Self::Io(_) => "copy_failed".to_string(),
            Self::Extension(e) => e.to_string(),
        }
    }
}

fn io(context: &str, path: &Path, e: std::io::Error) -> InstallError {
    InstallError::Io(format!("{context} {}: {e}", path.display()))
}

// ============================================================================
// Inspection
// ============================================================================

/// Validate a source directory and summarise its manifest — **before** any
/// byte is copied.
///
/// Returns the directory name, which is the extension id it will install as.
pub fn inspect_source(
    source: &Path,
    plugins_root: &Path,
) -> Result<(String, ManifestSummary), InstallError> {
    if !source.is_absolute() {
        return Err(InstallError::InvalidPath(format!(
            "the source path must be absolute, got '{}'",
            source.display()
        )));
    }
    if source.components().any(|c| c == Component::ParentDir) {
        return Err(InstallError::InvalidPath(format!(
            "the source path must not contain '..', got '{}'",
            source.display()
        )));
    }

    let name = source
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| {
            InstallError::InvalidPath(format!(
                "the source path has no directory name: '{}'",
                source.display()
            ))
        })?
        .to_string();
    if name.starts_with('.') {
        return Err(InstallError::InvalidPath(format!(
            "'{name}' is reserved: the plugins root's own directories start with a dot"
        )));
    }

    if !source.is_dir() {
        return Err(InstallError::SourceNotFound(format!(
            "no plugin directory at '{}'",
            source.display()
        )));
    }

    // "Load in place from an arbitrary path" stays declined: the install is a
    // copy into the store, so a source already inside the store is a caller
    // mistake rather than a no-op to absorb.
    let resolved = source.canonicalize().map_err(|e| io("cannot resolve", source, e))?;
    if let Ok(root) = plugins_root.canonicalize()
        && resolved.starts_with(&root)
    {
        return Err(InstallError::InvalidPath(format!(
            "'{}' is already inside the plugins root; install copies a directory in, \
             it never loads one in place",
            source.display()
        )));
    }

    let manifest = PluginManifest::from_dir(&resolved)
        .map_err(|e| InstallError::InvalidManifest(e.to_string()))?;
    if manifest.plugin.name != name {
        return Err(InstallError::InvalidManifest(format!(
            "the manifest calls this plugin '{}' but its directory is '{name}'; \
             the directory name is the plugin's identity",
            manifest.plugin.name
        )));
    }

    Ok((name.clone(), ManifestSummary::of(&name, &manifest)))
}

/// The same manifest read, for a directory **already** in the store — the
/// update path, where the target name is the installed id rather than the
/// source's own directory name.
pub fn inspect_update_source(
    source: &Path,
    plugins_root: &Path,
    id: &str,
) -> Result<ManifestSummary, InstallError> {
    let summary = match inspect_source(source, plugins_root) {
        Ok((_, summary)) => summary,
        // The source may legitimately sit in a build directory whose name is
        // not the plugin's; only the *manifest* name has to match the id.
        Err(InstallError::InvalidManifest(_)) | Err(InstallError::InvalidPath(_))
            if source.is_absolute() && source.is_dir() =>
        {
            let resolved = source
                .canonicalize()
                .map_err(|e| io("cannot resolve", source, e))?;
            if let Ok(root) = plugins_root.canonicalize()
                && resolved.starts_with(&root)
            {
                return Err(InstallError::InvalidPath(format!(
                    "'{}' is already inside the plugins root",
                    source.display()
                )));
            }
            let manifest = PluginManifest::from_dir(&resolved)
                .map_err(|e| InstallError::InvalidManifest(e.to_string()))?;
            ManifestSummary::of(&manifest.plugin.name.clone(), &manifest)
        }
        Err(e) => return Err(e),
    };

    if summary.name != id {
        return Err(InstallError::InvalidManifest(format!(
            "the replacement calls itself '{}' but the installed plugin is '{id}'; \
             an update never renames a plugin",
            summary.name
        )));
    }
    Ok(summary)
}

// ============================================================================
// Staging
// ============================================================================

/// A copy of a source tree sitting in `<root>/.staging/`, waiting to be renamed
/// into place.
///
/// **Dropping it without [`Self::commit`] removes it.** That is the whole
/// crash story: the only window in which a partial tree exists is inside
/// `.staging/`, which no scan reads.
#[derive(Debug)]
pub struct Staged {
    path: PathBuf,
    committed: bool,
}

impl Staged {
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Rename the staged tree to `dest`. `dest` must not exist — a replace
    /// [`trash`]es the incumbent first, so the rename is always onto a free
    /// name and always atomic.
    pub fn commit(mut self, dest: &Path) -> Result<(), InstallError> {
        if dest.exists() {
            return Err(InstallError::AlreadyInstalled(format!(
                "'{}' already exists",
                dest.display()
            )));
        }
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| io("cannot create", parent, e))?;
        }
        std::fs::rename(&self.path, dest).map_err(|e| io("cannot move into place", dest, e))?;
        self.committed = true;
        Ok(())
    }
}

impl Drop for Staged {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        if let Err(e) = std::fs::remove_dir_all(&self.path)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(
                path = %self.path.display(),
                error = %e,
                "could not sweep an abandoned staging copy"
            );
        }
    }
}

/// Copy `source` into `<plugins_root>/.staging/<name>.<pid>.<stamp>/`, **on a
/// blocking thread**.
///
/// The staging directory is a sibling of the destination, so the commit is a
/// rename inside one filesystem rather than a second copy.
///
/// The copy is `std::fs` throughout and a plugin tree is arbitrarily large, so
/// running it inline would hold a tokio worker for its whole duration — the
/// route's verb runs in a `tokio::spawn`ed task (`routes/extensions.rs`
/// `detached`), i.e. on a worker, and a handful of concurrent installs would
/// wedge the runtime. [`spawn_blocking`](tokio::task::spawn_blocking) is where
/// synchronous I/O belongs.
pub async fn stage(source: &Path, plugins_root: &Path, name: &str) -> Result<Staged, InstallError> {
    let source = source.to_path_buf();
    let plugins_root = plugins_root.to_path_buf();
    let name = name.to_string();
    match tokio::task::spawn_blocking(move || stage_blocking(&source, &plugins_root, &name)).await {
        Ok(result) => result,
        Err(join) => Err(InstallError::Io(format!("the copy did not complete: {join}"))),
    }
}

/// [`stage`]'s body, synchronous — including the [`Staged`] sweep of a failed
/// copy, which therefore also runs off the runtime.
fn stage_blocking(source: &Path, plugins_root: &Path, name: &str) -> Result<Staged, InstallError> {
    let staging_root = plugins_root.join(STAGING_DIR);
    std::fs::create_dir_all(&staging_root).map_err(|e| io("cannot create", &staging_root, e))?;

    let path = staging_root.join(format!("{name}.{}.{}", std::process::id(), stamp()));
    let staged = Staged {
        path,
        committed: false,
    };

    let root = source
        .canonicalize()
        .map_err(|e| io("cannot resolve", source, e))?;
    // From here on every early return drops `staged`, which sweeps the partial
    // copy — including the escaping-symlink refusal.
    copy_tree(&root, &root, &staged.path, 0)?;
    Ok(staged)
}

/// Recursive copy, dereferencing symlinks that stay inside `root` and refusing
/// the ones that do not.
fn copy_tree(root: &Path, from: &Path, to: &Path, depth: usize) -> Result<(), InstallError> {
    if depth > MAX_DEPTH {
        return Err(InstallError::InvalidPath(format!(
            "the source tree is deeper than {MAX_DEPTH} directories at '{}'",
            from.display()
        )));
    }
    std::fs::create_dir_all(to).map_err(|e| io("cannot create", to, e))?;

    let entries = std::fs::read_dir(from).map_err(|e| io("cannot read", from, e))?;
    for entry in entries {
        let entry = entry.map_err(|e| io("cannot read", from, e))?;
        let source_path = entry.path();
        let target = to.join(entry.file_name());

        let kind = source_path
            .symlink_metadata()
            .map_err(|e| io("cannot stat", &source_path, e))?
            .file_type();
        if kind.is_symlink() {
            // Dereferenced, never re-created: the copy must not hold a path the
            // owner did not review, and the child runs with its cwd here.
            let resolved = source_path.canonicalize().map_err(|e| {
                InstallError::EscapingSymlink(format!(
                    "the symlink '{}' does not resolve: {e}",
                    source_path.display()
                ))
            })?;
            if !resolved.starts_with(root) {
                return Err(InstallError::EscapingSymlink(format!(
                    "the symlink '{}' points outside the plugin directory, at '{}'",
                    source_path.display(),
                    resolved.display()
                )));
            }
            // The *target's* type is what will be copied, so it is the one that
            // has to be a directory or a regular file.
            let resolved_kind = resolved
                .symlink_metadata()
                .map_err(|e| io("cannot stat", &resolved, e))?
                .file_type();
            if resolved_kind.is_dir() {
                copy_tree(root, &resolved, &target, depth + 1)?;
            } else if resolved_kind.is_file() {
                copy_file(&resolved, &target)?;
            } else {
                return Err(irregular(&source_path, resolved_kind));
            }
            continue;
        }

        if kind.is_dir() {
            copy_tree(root, &source_path, &target, depth + 1)?;
        } else if kind.is_file() {
            copy_file(&source_path, &target)?;
        } else {
            return Err(irregular(&source_path, kind));
        }
    }
    Ok(())
}

/// An entry that is neither a directory nor a regular file, refused by name.
///
/// **`fs::copy` opens the source for reading**, and opening a FIFO with no
/// writer blocks forever — on the blocking thread the copy runs on, with no
/// timeout anywhere on the path. A device node or a unix socket is no more
/// copyable. `mkfifo` inside an unpacked third-party plugin directory is all it
/// takes, and absorbing a directory the owner did not write is this module's
/// whole job, so the type is checked rather than assumed.
fn irregular(path: &Path, kind: std::fs::FileType) -> InstallError {
    InstallError::InvalidPath(format!(
        "'{}' is {}, which a plugin directory may not contain: \
         only regular files and directories are copied",
        path.display(),
        describe(kind)
    ))
}

fn describe(kind: std::fs::FileType) -> &'static str {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        if kind.is_fifo() {
            return "a named pipe";
        }
        if kind.is_socket() {
            return "a socket";
        }
        if kind.is_block_device() {
            return "a block device";
        }
        if kind.is_char_device() {
            return "a character device";
        }
    }
    let _ = kind;
    "not a regular file"
}

/// One file, with its mode, flushed to disk before the tree can be renamed
/// into place — an executable entry point that reached the store without its
/// bytes would spawn and fail.
fn copy_file(from: &Path, to: &Path) -> Result<(), InstallError> {
    std::fs::copy(from, to).map_err(|e| io("cannot copy", from, e))?;
    let file = std::fs::File::open(to).map_err(|e| io("cannot reopen", to, e))?;
    file.sync_all().map_err(|e| io("cannot flush", to, e))?;
    Ok(())
}

// ============================================================================
// Trash
// ============================================================================

/// Move `dir` to `<plugins_root>/.trash/<name>-<stamp>/` and return where it
/// went.
///
/// §1.3 rule 3: a directory the owner dropped in is never deleted, so both the
/// replace half of an update and the uninstall land here. The stamp carries
/// nanoseconds, so two removals of one name in the same second do not collide.
pub fn trash(plugins_root: &Path, dir: &Path, name: &str) -> Result<PathBuf, InstallError> {
    let trash_root = plugins_root.join(TRASH_DIR);
    std::fs::create_dir_all(&trash_root).map_err(|e| io("cannot create", &trash_root, e))?;
    let target = trash_root.join(format!("{name}-{}", stamp()));
    std::fs::rename(dir, &target).map_err(|e| io("cannot move to the trash", dir, e))?;
    Ok(target)
}

/// A plugin's own data directory, whatever it holds.
pub fn data_dir(plugins_root: &Path, name: &str) -> PathBuf {
    plugins_root.join(DATA_DIR).join(name)
}

/// A fixed-width, sortable, filename-safe stamp — the one `config_io` uses.
fn stamp() -> String {
    chrono::Utc::now().format("%Y%m%dT%H%M%S%.9fZ").to_string()
}

#[cfg(test)]
mod tests;

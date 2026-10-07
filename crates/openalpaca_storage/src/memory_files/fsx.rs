//! File-system primitives hardened the way design §6 asks: private modes,
//! no-follow opens, a temp-fsync-rename-fsync atomic write, and the one
//! component check that confinement of the partition path is built from.
//!
//! `store::write_atomic` and `store::create_private_dir` are private to the
//! store module and lack the no-follow open, the 0600 mode and the directory
//! fsync, so this module carries its own copies rather than widening theirs.
//! These are `pub(crate)`: `store::projects` builds a project's folder and
//! writes its `project.json` with them (storage design, Stage 1).

use anyhow::{Context, Result, bail};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Creates `dir` and every missing parent, then sets it to 0700 on Unix
/// (best effort, like the store's own helper: a file system that refuses the
/// mode change must not refuse the write).
pub(crate) fn create_private_dir(dir: &Path) -> Result<()> {
    fs::create_dir_all(dir)
        .with_context(|| format!("Failed to create directory: {}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
    }
    Ok(())
}

/// What `symlink_metadata` says a path component on the way to a partition is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ComponentState {
    /// Nothing there yet: a lazy partition may create it.
    Absent,
    /// A real directory (not a link).
    Directory,
}

/// Design §3 rule 1: a component that exists must be a real directory. A
/// symbolic link, a junction (a reparse point reads as a symlink to Rust) or a
/// regular file refuses the partition. Returns the component and a reason so
/// the caller can name it.
pub(super) fn real_directory_state(path: &Path) -> Result<ComponentState, (PathBuf, String)> {
    match fs::symlink_metadata(path) {
        Ok(meta) => {
            let ft = meta.file_type();
            if ft.is_symlink() {
                Err((path.to_path_buf(), "a symbolic link".to_string()))
            } else if ft.is_dir() {
                Ok(ComponentState::Directory)
            } else {
                Err((path.to_path_buf(), "not a directory".to_string()))
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(ComponentState::Absent),
        Err(e) => Err((path.to_path_buf(), format!("unreadable ({e})"))),
    }
}

/// Reads a regular file without following a symlink at the leaf and refuses a
/// hardlink alias (more than one link) where the platform can say.
pub(super) fn read_regular_nofollow(path: &Path) -> Result<Vec<u8>> {
    let meta =
        fs::symlink_metadata(path).with_context(|| format!("Failed to stat {}", path.display()))?;
    if meta.file_type().is_symlink() {
        bail!("{} is a symbolic link", path.display());
    }
    if !meta.file_type().is_file() {
        bail!("{} is not a regular file", path.display());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.nlink() > 1 {
            bail!(
                "{} has {} hard links; a memory note must be the only name of its inode",
                path.display(),
                meta.nlink()
            );
        }
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options
        .open(path)
        .with_context(|| format!("Failed to open {}", path.display()))?;
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    std::io::Read::read_to_end(&mut file, &mut bytes)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    Ok(bytes)
}

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// The hidden sibling a write goes through: `.<name>.<pid>-<n>.tmp`. Hidden,
/// so no scanner lists it; unique, so a stranded one never blocks a retry.
pub(crate) fn temp_path_for(path: &Path) -> Result<PathBuf> {
    let name = path
        .file_name()
        .with_context(|| format!("Not a file path: {}", path.display()))?
        .to_string_lossy();
    let n = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    Ok(path.with_file_name(format!(".{name}.{}-{n}.tmp", std::process::id())))
}

/// Writes `bytes` to `path` atomically: a hidden temp sibling created with
/// `create_new` (never through a link), mode 0600, `fsync`, `rename` onto the
/// target, then an `fsync` of the directory where the platform supports it.
/// The temp file is removed on every failure path.
pub(crate) fn write_atomic_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = temp_path_for(path)?;
    let outcome = write_atomic_steps(path, &tmp, bytes);
    if outcome.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    outcome
}

fn write_atomic_steps(path: &Path, tmp: &Path, bytes: &[u8]) -> Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options
        .open(tmp)
        .with_context(|| format!("Failed to create {}", tmp.display()))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .with_context(|| format!("Failed to write {}", tmp.display()))?;
    drop(file);
    fs::rename(tmp, path)
        .with_context(|| format!("Failed to move {} into place", tmp.display()))?;
    if let Some(dir) = path.parent() {
        fsync_dir(dir)?;
    }
    Ok(())
}

/// Flushes a directory's entries to disk so a completed rename survives a
/// power loss. A no-op where directories cannot be opened for sync.
pub(crate) fn fsync_dir(dir: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        let file = fs::File::open(dir)
            .with_context(|| format!("Failed to open directory {}", dir.display()))?;
        file.sync_all()
            .with_context(|| format!("Failed to fsync directory {}", dir.display()))?;
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
    }
    Ok(())
}

/// Removes a file; absent is fine. Returns whether something was removed.
pub(super) fn remove_file_if_exists(path: &Path) -> Result<bool> {
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e).with_context(|| format!("Failed to remove {}", path.display())),
    }
}

/// Removes a directory tree; absent is fine. Returns whether something was
/// removed.
pub(crate) fn remove_dir_all_if_exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            // Never descend through a link: remove the link itself.
            fs::remove_file(path)
                .with_context(|| format!("Failed to remove link {}", path.display()))?;
            Ok(true)
        }
        Ok(_) => {
            fs::remove_dir_all(path)
                .with_context(|| format!("Failed to remove {}", path.display()))?;
            Ok(true)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e).with_context(|| format!("Failed to stat {}", path.display())),
    }
}

/// Reads a file if it exists, refusing to follow a symlink at the leaf.
pub(crate) fn read_if_exists(path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::symlink_metadata(path) {
        Ok(_) => read_regular_nofollow(path).map(Some),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("Failed to stat {}", path.display())),
    }
}

/// The canonical path of a directory that may not exist yet: the deepest
/// existing ancestor is canonicalized and the missing tail is appended as
/// written. A lazy partition therefore derives the same key before and after
/// it is created, and the caller has already refused any link on the way.
/// A missing tail that ends in `..` has no file name, so the walk refuses it
/// rather than leave a `..` in the answer.
pub(crate) fn canonical_path_lenient(path: &Path) -> Result<PathBuf> {
    let mut existing: &Path = path;
    let mut tail: Vec<&std::ffi::OsStr> = Vec::new();
    while !existing.exists() {
        let name = existing
            .file_name()
            .with_context(|| format!("path has no existing ancestor: {}", path.display()))?;
        tail.push(name);
        existing = existing
            .parent()
            .with_context(|| format!("path has no existing ancestor: {}", path.display()))?;
    }
    let mut result = existing
        .canonicalize()
        .with_context(|| format!("failed to canonicalize {}", existing.display()))?;
    for component in tail.into_iter().rev() {
        result.push(component);
    }
    Ok(result)
}

/// [`canonical_path_lenient`] as the string a partition key and a snapshot's
/// `root` are derived from.
pub(crate) fn canonical_string_lenient(path: &Path) -> Result<String> {
    Ok(canonical_path_lenient(path)?.to_string_lossy().into_owned())
}

/// A file's mode bits on Unix (`None` elsewhere), for the mode assertions.
#[cfg(test)]
pub(crate) fn mode_of(path: &Path) -> Option<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::symlink_metadata(path)
            .ok()
            .map(|m| m.permissions().mode() & 0o777)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

//! Open's copies of uploads: `state/cache/open/<file id>/<name>` (N-STORE-1;
//! the ZCode audit's Z8-03, applying D1).
//!
//! An upload is opened through a copy under the name it was uploaded with:
//! the stored name is the store's own (`03-quarterly-report.pdf`), and the
//! system opener chooses an application by the name it is handed. The copy is
//! derived data — one 0700 folder per file, replaced by every Open of that
//! file, removed with the file's row when its project's data is deleted
//! (`project_life::delete_project_data`), and swept whole at the daemon's next
//! start. It used to be written to `$TMPDIR/openalpaca-open/`, where nothing
//! removed it and neither a project's delete nor the factory reset reached it.
//!
//! No link is followed at `open/` or below it: a link at `open/` itself is
//! left where it is by the sweep and refused by Open, and one at a file's
//! folder is removed as the link it is. (`state/` and `state/cache/` are the
//! store's own folders, as they are for the memory index.)

use anyhow::{Context, Result, bail};
use std::fs;
use std::path::{Component, Path, PathBuf};

use super::{CACHE_DIR, create_private_dir, state_dir, state_dir_path};

/// The folder's name under `state/cache/`.
const OPEN_DIR: &str = "open";

/// `state/cache/open` — where Open stages its copies. A pure path query.
pub fn open_copies_dir() -> Result<PathBuf> {
    Ok(state_dir_path()?.join(CACHE_DIR).join(OPEN_DIR))
}

/// `name` when it is one plain path component — never empty, `.`, `..` or
/// anything holding a separator — so joining it stays inside its folder.
fn plain<'a>(what: &str, name: &'a str) -> Result<&'a str> {
    let mut parts = Path::new(name).components();
    match (parts.next(), parts.next()) {
        (Some(Component::Normal(part)), None) if part.to_str() == Some(name) => Ok(name),
        _ => bail!("{what} {name:?} is not one plain name"),
    }
}

/// What is at `path`, as `symlink_metadata` sees it — a link is a link, never
/// what it leads to; `None` when there is nothing.
fn entry_type(path: &Path) -> std::io::Result<Option<fs::FileType>> {
    match fs::symlink_metadata(path) {
        Ok(meta) => Ok(Some(meta.file_type())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Removes the entry at `path` without following it: a folder with its
/// contents, anything else by itself. `Ok(false)` when there was none.
fn remove_entry(path: &Path) -> std::io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_dir() => fs::remove_dir_all(path).map(|()| true),
        Ok(_) => fs::remove_file(path).map(|()| true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

/// Copies `source` to `state/cache/open/<file_id>/<name>` for Open and
/// returns the copy's path. Whatever an earlier Open of the same file left in
/// its folder goes first, so the folder holds this copy alone; the folders it
/// makes are 0700 on Unix. `file_id` and `name` must each be one plain name —
/// the caller makes the uploaded name safe for a file system.
pub fn stage_open_copy(source: &Path, file_id: &str, name: &str) -> Result<PathBuf> {
    let file_id = plain("file id", file_id)?;
    let name = plain("file name", name)?;
    // `state/` 0700 first: the copies sit in the machine's half of the store.
    state_dir()?;
    let open = open_copies_dir()?;
    // Never through a link: it would put the copy, and the 0700 mode, on the
    // folder it leads to.
    match entry_type(&open).with_context(|| format!("Failed to read {}", open.display()))? {
        Some(kind) if kind.is_symlink() => bail!(
            "{} is a link: Open never stages through one — remove it and Open again",
            open.display()
        ),
        Some(kind) if !kind.is_dir() => bail!(
            "{} is not a folder: remove it and Open again",
            open.display()
        ),
        _ => {}
    }
    create_private_dir(&open)?;
    let folder = open.join(file_id);
    remove_entry(&folder)
        .with_context(|| format!("Failed to clear {} for a fresh copy", folder.display()))?;
    create_private_dir(&folder)?;
    let target = folder.join(name);
    fs::copy(source, &target).with_context(|| {
        format!(
            "Failed to copy {} to {} for Open",
            source.display(),
            target.display()
        )
    })?;
    Ok(target)
}

/// Removes what Open staged for `file_id` — its folder under
/// `state/cache/open/` — and answers whether there was one. An id that is not
/// one plain name has no folder there, and neither has a link at `open/`,
/// which is never followed.
pub fn remove_open_copy(file_id: &str) -> Result<bool> {
    let Ok(file_id) = plain("file id", file_id) else {
        return Ok(false);
    };
    let open = open_copies_dir()?;
    if !entry_type(&open)
        .with_context(|| format!("Failed to read {}", open.display()))?
        .is_some_and(|kind| kind.is_dir())
    {
        return Ok(false);
    }
    let folder = open.join(file_id);
    remove_entry(&folder).with_context(|| format!("Failed to remove {}", folder.display()))
}

/// The boot sweep: every entry under `state/cache/open/` is removed — at a
/// start no Open of this daemon's life has staged anything yet, and an
/// earlier one's copies have had their use. Returns how many entries went
/// (none when the folder is missing); one that cannot be removed is logged
/// and the rest are still tried. A link at `open/` is never followed — the
/// folder it leads to is not Open's to empty — and is left with a warning.
pub fn sweep_open_copies() -> Result<usize> {
    let open = open_copies_dir()?;
    match entry_type(&open).with_context(|| format!("Failed to read {}", open.display()))? {
        None => return Ok(0),
        Some(kind) if !kind.is_dir() => {
            tracing::warn!(
                "{} is {}, not a folder: the sweep does not follow it and leaves it as it is",
                open.display(),
                if kind.is_symlink() {
                    "a link"
                } else {
                    "a file"
                }
            );
            return Ok(0);
        }
        Some(_) => {}
    }
    let entries = match fs::read_dir(&open) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e).with_context(|| format!("Failed to read {}", open.display())),
    };
    let mut removed = 0;
    for entry in entries {
        let path = match entry {
            Ok(entry) => entry.path(),
            Err(e) => {
                tracing::warn!("Failed to read an entry of {}: {e}", open.display());
                continue;
            }
        };
        match remove_entry(&path) {
            Ok(true) => removed += 1,
            Ok(false) => {}
            Err(e) => tracing::warn!("Failed to remove {}: {e}", path.display()),
        }
    }
    Ok(removed)
}

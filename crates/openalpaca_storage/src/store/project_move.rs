//! Moving a project's own store: `<old>/.openalpaca` → `<new>/.openalpaca`.
//!
//! One atomic rename, planned before anything is written, behind
//! `PATCH /v1/workspaces` (`apps/openalpacad/src/routes/workspaces.rs`).
//! Nothing here is a data-layout migration: a project moves because its owner
//! moved it, not because the layout changed.

use crate::store;
use anyhow::{Context, Result, bail};
use std::fs;
use std::io::ErrorKind;
use std::path::Path;
use tracing::{debug, info};

/// What moving `<old>/.openalpaca` to `<new>/.openalpaca` would do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreMove {
    /// Both roots hold a store. Which one is the project's is a question only
    /// its owner can answer, so this refuses rather than choose — refusing is
    /// the only answer that cannot lose the wrong one.
    Ambiguous,
    /// Nothing at the old root: the directory was already moved by hand, which
    /// is the ordinary way a project moves. Only the rows are left to re-base.
    NothingToMove,
    /// A single `rename(2)` from the old store root to the new one.
    Rename,
}

/// Decide the move **before** anything is written, so a re-base that cannot
/// finish is refused instead of half-applied.
///
/// The one failure a plan cannot foresee is a rename that turns out to cross a
/// volume; this checks for that too, by comparing the device of the store root
/// with the device of the directory that would receive it.
pub fn plan_project_store_move(old_project: &Path, new_project: &Path) -> Result<StoreMove> {
    let src = old_project.join(store::STORE_DIR_NAME);
    let dst = new_project.join(store::STORE_DIR_NAME);

    if !exists(&src) {
        return Ok(StoreMove::NothingToMove);
    }
    if same_dir(&src, &dst) {
        return Ok(StoreMove::NothingToMove);
    }
    if exists(&dst) {
        return Ok(StoreMove::Ambiguous);
    }
    guard_same_volume(&src, new_project)?;
    Ok(StoreMove::Rename)
}

/// Move `<old_project>/.openalpaca` to `<new_project>/.openalpaca` — one atomic
/// rename, on the terms [`plan_project_store_move`] already agreed.
///
/// Returns whether anything moved. Re-planned rather than trusting the caller's
/// plan: the two calls are separated by a database transaction, and a refusal
/// is always better than a rename onto a store that appeared in between.
pub fn move_project_store(old_project: &Path, new_project: &Path) -> Result<bool> {
    let src = old_project.join(store::STORE_DIR_NAME);
    let dst = new_project.join(store::STORE_DIR_NAME);
    match plan_project_store_move(old_project, new_project)? {
        StoreMove::NothingToMove => {
            debug!("No store at {}; nothing to move", src.display());
            return Ok(false);
        }
        StoreMove::Ambiguous => bail!(
            "two stores: {} and {} both exist. Refusing to choose between them — \
             keep the one you want, move the other aside, and try again",
            src.display(),
            dst.display()
        ),
        StoreMove::Rename => {}
    }

    fs::create_dir_all(new_project)
        .with_context(|| format!("Failed to create {}", new_project.display()))?;
    match fs::rename(&src, &dst) {
        Ok(()) => {
            info!("Moved {} → {}", src.display(), dst.display());
            Ok(true)
        }
        Err(e) if is_cross_device(&e) => bail!(
            "cannot move {} to {}: they are on different volumes, so the move \
             cannot be atomic; move the directory by hand",
            src.display(),
            dst.display()
        ),
        Err(e) => {
            Err(e).with_context(|| format!("Failed to move {} to {}", src.display(), dst.display()))
        }
    }
}

/// Refuse a rename across volumes up front. On a platform without device ids
/// this says nothing and the rename itself reports it.
#[cfg(unix)]
fn guard_same_volume(src: &Path, dst_parent: &Path) -> Result<()> {
    use std::os::unix::fs::MetadataExt;

    // The nearest existing ancestor: the destination project directory may not
    // exist yet, and the volume is the one it *would* be created on.
    let mut probe = dst_parent;
    let device = loop {
        match fs::metadata(probe) {
            Ok(meta) => break Some(meta.dev()),
            Err(_) => match probe.parent() {
                Some(parent) => probe = parent,
                None => break None,
            },
        }
    };
    let (Some(device), Ok(src_meta)) = (device, fs::metadata(src)) else {
        return Ok(());
    };
    if src_meta.dev() != device {
        bail!(
            "cannot move {} to {}: they are on different volumes, so the move \
             cannot be atomic; move the directory by hand",
            src.display(),
            dst_parent.join(store::STORE_DIR_NAME).display()
        );
    }
    Ok(())
}

#[cfg(not(unix))]
fn guard_same_volume(_src: &Path, _dst_parent: &Path) -> Result<()> {
    Ok(())
}

/// `exists()` that also sees a broken symlink.
pub(super) fn exists(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

pub(super) fn same_dir(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

#[cfg(unix)]
pub(super) fn is_cross_device(e: &std::io::Error) -> bool {
    e.kind() == ErrorKind::CrossesDevices || e.raw_os_error() == Some(libc::EXDEV)
}

#[cfg(not(unix))]
pub(super) fn is_cross_device(e: &std::io::Error) -> bool {
    e.kind() == ErrorKind::CrossesDevices
}

#[cfg(test)]
mod tests;

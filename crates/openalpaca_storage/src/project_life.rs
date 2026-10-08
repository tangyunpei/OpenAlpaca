//! A project's life once it is registered (design §6): the re-point, and the
//! boot recovery of one that did not finish (R24). Stage 1 step (5) adds the
//! delete here.
//!
//! Database and files only. What storage cannot see stays with the caller:
//! the memory service's `quiesce` and the lane registry's live-run check are
//! the route's (`PATCH /v1/workspaces`).
//!
//! Lock order (interfaces §1.5): [`projects::lock`] first, then the database
//! mutex — the re-point holds the first across its one transaction, so
//! nothing here may take the projects lock inside a `with_connection`.

use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::Database;
use crate::artifacts::{ArtifactStore, RebaseCounts};
use crate::content_io::fsync_dir;
use crate::store::projects::{self, Project, ProjectError, ProjectFile, ProjectsLock};

/// §6's four steps, each named for what has just happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepointStep {
    /// 1: `project.json` names the new root, with `previous_root` and
    /// `previous_name`.
    Marked,
    /// 2: the five path columns and both `storage_path` prefixes, in one
    /// transaction.
    RowsRewritten,
    /// 3: `projects/<previous_name>` renamed to the new name.
    Renamed,
    /// 4: `project.json` without the two keys.
    Cleared,
}

/// A finished re-point: the project as it was and as it is, and the rows
/// that moved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repointed {
    pub before: Project,
    pub after: Project,
    pub rows: RebaseCounts,
}

/// A re-point the test checkpoint stopped (R24's crash hook), returned inside
/// the `anyhow::Error`. The disk is exactly as that step left it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RepointInterrupted(pub RepointStep);

impl fmt::Display for RepointInterrupted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "the re-point stopped after {:?}", self.0)
    }
}

impl std::error::Error for RepointInterrupted {}

/// What [`recover_repoints`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecoveryReport {
    /// The new roots of the re-points finished.
    pub finished: Vec<PathBuf>,
    /// Re-points that failed again, by new root, with why. Tried again at the
    /// next boot.
    pub failed: Vec<(PathBuf, String)>,
}

/// §6 *Re-point*: the project `old_path` resolves to is now at `new_path`.
///
/// Under [`projects::lock`] for the whole call. `old_path` must resolve to a
/// registered project ([`ProjectError::NotRegistered`]). `new_path`,
/// canonicalized, must be a directory ([`ProjectError::NotADirectory`] — a
/// mistyped path is never created), must not overlap the home store
/// ([`ProjectError::IsHome`]), must differ from the project's root
/// ([`ProjectError::SamePath`]), and must be neither a registered project nor
/// named by any of the five path columns ([`ProjectError::Exists`], ST-20).
/// Then: 1 the file is marked; 2 one transaction moves the rows (a failure
/// clears the marker and returns the error); 3 the folder is renamed (its new
/// name is free or this project's own, R6); 4 the marker is cleared.
pub fn repoint(db: &Database, old_path: &Path, new_path: &Path) -> Result<Repointed> {
    repoint_with(db, old_path, new_path, &mut |_| true)
}

/// [`repoint`] with R24's crash hook: `checkpoint` runs after each step, and
/// `false` stops there with [`RepointInterrupted`] and the disk as that step
/// left it. Production passes `|_| true`.
#[doc(hidden)]
pub fn repoint_with(
    db: &Database,
    old_path: &Path,
    new_path: &Path,
    checkpoint: &mut dyn FnMut(RepointStep) -> bool,
) -> Result<Repointed> {
    let held = projects::lock();
    let before = registered(old_path)?;
    let new_root = target_root(db, &before, new_path)?;

    let mut marked = before.file();
    marked.root = new_root.to_string_lossy().into_owned();
    marked.previous_root = Some(before.root_string());
    marked.previous_name = Some(before.name.clone());
    projects::write_project_file(&before.dir, &marked)?;
    stop(checkpoint, RepointStep::Marked)?;

    let (after, rows) = finish(
        db,
        &before.dir,
        &marked,
        &held,
        checkpoint,
        OnRowsFailure::ClearMarker,
    )?;
    Ok(Repointed {
        before,
        after,
        rows,
    })
}

/// R24, at boot, before anything resolves a project: every folder under
/// `projects/` whose `project.json` carries `previous_root` is a re-point
/// that did not finish, and steps 2–4 run again — each idempotent. A failure
/// is reported and left for the next boot.
pub fn recover_repoints(db: &Database) -> RecoveryReport {
    let held = projects::lock();
    let mut report = RecoveryReport::default();
    let entries = match projects::list() {
        Ok(entries) => entries,
        Err(e) => {
            let dir = projects::projects_dir().unwrap_or_default();
            report.failed.push((dir, format!("{e:#}")));
            return report;
        }
    };
    for entry in entries.iter().filter(|p| p.previous_root.is_some()) {
        match finish(
            db,
            &entry.dir,
            &entry.file(),
            &held,
            &mut |_| true,
            OnRowsFailure::Leave,
        ) {
            Ok((after, _)) => report.finished.push(after.root),
            Err(e) => report.failed.push((entry.root.clone(), format!("{e:#}"))),
        }
    }
    report
}

/// What step 2's failure does to the marker: a re-point the caller asked for
/// clears it ("nothing else changed"); a recovery leaves it for the next boot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OnRowsFailure {
    ClearMarker,
    Leave,
}

/// The project `old_path` resolves to, or [`ProjectError::NotRegistered`].
fn registered(old_path: &Path) -> Result<Project> {
    match projects::resolve(old_path)? {
        Some(projects::ResolvedProject {
            project: Some(project),
            ..
        }) => Ok(project),
        Some(resolved) => Err(anyhow::Error::new(ProjectError::NotRegistered {
            root: resolved.root_string(),
        })),
        None => Err(anyhow::Error::new(ProjectError::NotRegistered {
            root: old_path.to_string_lossy().into_owned(),
        })),
    }
}

/// The canonical new root, or the refusal §6 names for it.
fn target_root(db: &Database, before: &Project, new_path: &Path) -> Result<PathBuf> {
    let written = new_path.to_string_lossy().into_owned();
    let root = match new_path
        .is_absolute()
        .then(|| std::fs::canonicalize(new_path))
    {
        Some(Ok(root)) if root.is_dir() => root,
        _ => {
            return Err(anyhow::Error::new(ProjectError::NotADirectory {
                path: written,
            }));
        }
    };
    let text = root.to_string_lossy().into_owned();
    if projects::overlaps_home_store(&root) {
        return Err(anyhow::Error::new(ProjectError::IsHome { path: text }));
    }
    if root == before.root {
        return Err(anyhow::Error::new(ProjectError::SamePath { path: text }));
    }
    let named = !ArtifactStore::new(db)
        .workspace_rows(&text, None)?
        .counts
        .is_empty();
    if named || projects::lookup(&root)?.is_some() {
        return Err(anyhow::Error::new(ProjectError::Exists { path: text }));
    }
    Ok(root)
}

/// Steps 2–4 for the marked `project.json` in `dir` (the folder it is in
/// now). Each step is idempotent, so recovery runs this same function.
fn finish(
    db: &Database,
    dir: &Path,
    marked: &ProjectFile,
    held: &ProjectsLock,
    checkpoint: &mut dyn FnMut(RepointStep) -> bool,
    on_rows_failure: OnRowsFailure,
) -> Result<(Project, RebaseCounts)> {
    let (Some(old_root), Some(old_name)) = (
        marked.previous_root.as_deref(),
        marked.previous_name.as_deref(),
    ) else {
        bail!("{} carries no re-point to finish", dir.display());
    };
    let parent = dir
        .parent()
        .with_context(|| format!("{} has no parent", dir.display()))?;
    let current = dir
        .file_name()
        .and_then(|name| name.to_str())
        .with_context(|| format!("{} has no folder name", dir.display()))?;
    let new_root = PathBuf::from(&marked.root);
    // After step 3 the folder already has its new name; before it, the name
    // is chosen now, under the lock — free, or this project's own (R6).
    let new_name = if current != old_name {
        current.to_string()
    } else {
        projects::name_for(&new_root, Some(&marked.id), held)?
    };
    let old_dir = parent.join(old_name);
    let new_dir = parent.join(&new_name);

    // Step 2.
    let rows = match ArtifactStore::new(db).repoint_rows(
        old_root,
        &marked.root,
        &old_dir.to_string_lossy(),
        &new_dir.to_string_lossy(),
    ) {
        Ok(rows) => rows,
        Err(e) => {
            let e = e.context(format!(
                "the rows of {old_root} could not be re-pointed onto {}",
                marked.root
            ));
            if on_rows_failure == OnRowsFailure::ClearMarker {
                let mut cleared = marked.clone();
                cleared.root = old_root.to_string();
                cleared.previous_root = None;
                cleared.previous_name = None;
                if let Err(undo) = projects::write_project_file(dir, &cleared) {
                    return Err(e.context(format!(
                        "and {} still names the new root ({undo:#}); the next start finishes the re-point",
                        dir.join(projects::PROJECT_FILE).display()
                    )));
                }
            }
            return Err(e);
        }
    };
    stop(checkpoint, RepointStep::RowsRewritten)?;

    // Step 3.
    if dir != new_dir {
        std::fs::rename(dir, &new_dir).with_context(|| {
            format!(
                "could not rename {} to {}",
                dir.display(),
                new_dir.display()
            )
        })?;
        fsync_dir(parent);
    }
    stop(checkpoint, RepointStep::Renamed)?;

    // Step 4.
    let mut cleared = marked.clone();
    cleared.previous_root = None;
    cleared.previous_name = None;
    projects::write_project_file(&new_dir, &cleared)?;
    stop(checkpoint, RepointStep::Cleared)?;

    let after = projects::read_project_dir(&new_dir)?.with_context(|| {
        format!(
            "{} has no readable project.json after the re-point",
            new_dir.display()
        )
    })?;
    Ok((after, rows))
}

fn stop(checkpoint: &mut dyn FnMut(RepointStep) -> bool, step: RepointStep) -> Result<()> {
    if checkpoint(step) {
        Ok(())
    } else {
        Err(anyhow::Error::new(RepointInterrupted(step)))
    }
}

mod delete;
pub use delete::{
    DeleteOutcome, DeletePlan, KEPT_DIR, KeptFiles, RunsInFlight, delete_plan, delete_project_data,
    remove_project_folder,
};

#[cfg(test)]
mod tests;

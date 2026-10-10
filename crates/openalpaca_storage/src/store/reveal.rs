//! Which paths the app may show in their folder (Round G, G4; storage design
//! §13, ST-18).
//!
//! The GUI's `Reveal in Finder` hands the Tauri shell an artifact's `path`,
//! which the webview read from the daemon. The shell trusts the file system,
//! not the webview: [`revealable_path`] resolves the path — symlinks included —
//! and admits only a regular file under the home store, where every store file
//! lives — a project's included, in its folder under `projects/` (ST-1) — or
//! under the root of a project `projects/` records, where a file an agent
//! wrote into the project lives (ST-12), unless that project is missing:
//! a missing project's root admits no file (ST-23, R-T18). The shell then
//! passes the answer to the platform's reveal as an argument, never through
//! a shell. Nothing here opens, reads or changes the file.

use std::fmt;
use std::path::{Path, PathBuf};

use super::{home_root, projects};

/// Why [`revealable_path`] refused a path. `Display` is one sentence, written
/// to be shown as it is in a toast.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RevealRefusal {
    /// Nothing is there, or the path cannot be resolved. Carries the path as
    /// it was asked for.
    Missing(PathBuf),
    /// A directory, or anything else that is not a regular file. Carries the
    /// resolved path.
    NotAFile(PathBuf),
    /// The resolved file is under neither the home root nor the root of a
    /// registered project that is not missing — a missing project's root
    /// admits no file (ST-23, R-T18) — or there is neither to compare with.
    /// A symlink inside either whose target is outside both lands here, with
    /// the target, and so does a file in an old in-project `.openalpaca/` of
    /// a folder no project records (ST-9). Inside a registered project such a
    /// file is under its root and is shown: showing a file in its folder
    /// reads nothing, and the file tools refuse `.openalpaca` themselves
    /// (R32).
    OutsideStore(PathBuf),
}

impl fmt::Display for RevealRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RevealRefusal::Missing(path) => write!(
                f,
                "{} is not there any more, so it cannot be shown in its folder.",
                path.display()
            ),
            RevealRefusal::NotAFile(path) => write!(
                f,
                "{} is not a file, so it cannot be shown in its folder.",
                path.display()
            ),
            RevealRefusal::OutsideStore(path) => write!(
                f,
                "{} is not in an OpenAlpaca store or one of your projects, so it is not shown in \
                 its folder.",
                path.display()
            ),
        }
    }
}

impl std::error::Error for RevealRefusal {}

/// The canonical form of `path` when it may be shown in its folder: a
/// regular file whose canonical path is under the canonical home root
/// ([`home_root`], `OPENALPACA_HOME_STORE` honoured) or under the canonical
/// root of a project [`projects::list`] returns (ST-18: Reveal is what a file
/// written into the project is offered whatever its kind). Refused: a path
/// that does not resolve, anything that is not a regular file, and a file
/// under neither. The canonical path decides, so a symlink is judged by where
/// it leads. Reads `projects/` — one `project.json` per project — so a caller
/// on an async runtime asks it on the blocking pool.
pub fn revealable_path(path: &Path) -> Result<PathBuf, RevealRefusal> {
    revealable_path_among(path, home_root().ok().as_deref(), &project_roots())
}

/// The roots of the projects `projects/` records, one mid re-point by the
/// root it is going to. A missing one is left out (the lead's ruling R-T18):
/// its root no longer leads to its own folder (ST-23), and what it reaches —
/// through a link left at the old path, or by the old spelling — is not its
/// project's to show; a root that is gone admits nothing anyway. A
/// `projects/` that cannot be read records none, with one WARN: a file in the
/// home store is still shown, and a project's file is refused as outside.
fn project_roots() -> Vec<PathBuf> {
    match projects::list() {
        Ok(found) => found
            .into_iter()
            .filter(|project| !project.missing)
            .map(|project| project.root)
            .collect(),
        Err(e) => {
            tracing::warn!("Reveal in Finder reads no project roots: {e:#}");
            Vec::new()
        }
    }
}

/// [`revealable_path`] with the home root and the project roots passed in, so
/// the rule is testable without reading the environment or `projects/`.
fn revealable_path_among(
    path: &Path,
    home: Option<&Path>,
    project_roots: &[PathBuf],
) -> Result<PathBuf, RevealRefusal> {
    let canonical = path
        .canonicalize()
        .map_err(|_| RevealRefusal::Missing(path.to_path_buf()))?;
    let is_file = std::fs::metadata(&canonical).is_ok_and(|meta| meta.is_file());
    if !is_file {
        return Err(RevealRefusal::NotAFile(canonical));
    }
    let admitted = home.is_some_and(|home| is_under(&canonical, home))
        || project_roots.iter().any(|root| is_under(&canonical, root));
    if admitted {
        Ok(canonical)
    } else {
        Err(RevealRefusal::OutsideStore(canonical))
    }
}

/// Whether the canonical `file` is under `root`, compared with `root` resolved
/// too (as written when it cannot be), component by component — so
/// `/dev/app-evil/notes.md` is not under `/dev/app`.
fn is_under(file: &Path, root: &Path) -> bool {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    file.starts_with(root)
}

#[cfg(test)]
mod tests;

//! Which paths the app may show in their folder (Round G, G4).
//!
//! The GUI's `Reveal in Finder` hands the Tauri shell an artifact's `path`,
//! which the webview read from the daemon. The shell trusts the file system,
//! not the webview: [`revealable_path`] resolves the path — symlinks included —
//! and admits only a regular file under the home store, where every store file
//! lives — a project's included, in its folder under `projects/` (ST-1). The
//! shell then passes the answer to the platform's reveal as an argument, never
//! through a shell. Nothing here opens, reads or changes the file.

use std::fmt;
use std::path::{Path, PathBuf};

use super::home_root;

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
    /// The resolved file is not under the home root, or there is no home root
    /// to compare with. A symlink inside the home store whose target is outside
    /// lands here, with the target, and so does a file in an old in-project
    /// `.openalpaca/` (ST-9).
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
                "{} is not in an OpenAlpaca store, so it is not shown in its folder.",
                path.display()
            ),
        }
    }
}

impl std::error::Error for RevealRefusal {}

/// The canonical form of `path` when it may be shown in its folder: a
/// regular file whose canonical path is under the canonical home root
/// ([`home_root`], `OPENALPACA_HOME_STORE` honoured). Refused: a path that
/// does not resolve, anything that is not a regular file, and a file outside
/// the home root. The canonical path decides, so a symlink is judged by where
/// it leads.
pub fn revealable_path(path: &Path) -> Result<PathBuf, RevealRefusal> {
    revealable_path_with_home(path, home_root().ok().as_deref())
}

/// [`revealable_path`] with the home root passed in, so the rule is testable
/// without reading the environment.
fn revealable_path_with_home(path: &Path, home: Option<&Path>) -> Result<PathBuf, RevealRefusal> {
    let canonical = path
        .canonicalize()
        .map_err(|_| RevealRefusal::Missing(path.to_path_buf()))?;
    let is_file = std::fs::metadata(&canonical).is_ok_and(|meta| meta.is_file());
    if !is_file {
        return Err(RevealRefusal::NotAFile(canonical));
    }
    let under_home = home.is_some_and(|home| {
        let home = home.canonicalize().unwrap_or_else(|_| home.to_path_buf());
        canonical.starts_with(home)
    });
    if under_home {
        Ok(canonical)
    } else {
        Err(RevealRefusal::OutsideStore(canonical))
    }
}

#[cfg(test)]
mod tests;

//! The atomic, comment-preserving writer both extension stores use
//! (extension design §2.1).
//!
//! `config/mcp.toml` and `<plugins root>/.permissions.toml` are hand-authored
//! files the daemon also writes. Three properties follow from that, and this
//! module is where they are enforced once:
//!
//! 1. **Comments survive.** The edit is a surgical `toml_edit` assignment, not
//!    a serialize-and-overwrite.
//! 2. **A malformed result is never written.** The rendered document is
//!    re-parsed with the *reader's own* parser before the rename; a failed
//!    re-parse aborts with the file untouched.
//! 3. **The previous version is recoverable.** §5.1 refuses to load an
//!    unparseable store and never overwrites it — correct, but on its own that
//!    turns one typo into "every integration is off and the approvals are
//!    unreadable" with nothing to copy back. Every rewrite keeps the five
//!    newest prior versions under `state/backups/`, and a parse failure copies
//!    the bad file once to `state/backups/<basename>.unparseable-<ts>`.
//!
//! It lives in `openalpaca_core` because both writers need it and
//! `openalpaca_core` is the one crate `apps/openalpacad` **and**
//! `crates/openalpaca_plugins` already depend on.

use std::io::Write;
use std::path::{Path, PathBuf};

/// How many rotated copies of a config file are kept.
pub const BACKUPS_KEPT: usize = 5;

/// Why a config write did not happen. Every variant leaves the file on disk
/// exactly as it was.
#[derive(Debug, thiserror::Error)]
pub enum ConfigWriteError {
    #[error("failed to lock {path}: {source}")]
    Lock {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to read {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{path} is not valid TOML: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: toml_edit::TomlError,
    },
    /// The caller's edit closure refused.
    #[error("{0}")]
    Edit(String),
    /// The rendered document did not survive the reader's own parser. The file
    /// is untouched.
    #[error("edit rejected: the result would not parse as {path}: {reason}")]
    Reparse { path: PathBuf, reason: String },
    #[error("failed to write {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Read `path`, apply `edit` to its parsed document, re-parse the result with
/// `reparse`, and replace the file atomically.
///
/// Order (design §2.1): acquire `<path>.lock` → read → edit → **re-parse the
/// result with the reader's own parser** → write a sibling temp file →
/// `sync_all` → rotate the file being replaced into `state/backups/` → rename.
///
/// `reparse` is the caller's own loader (`McpConfig::load`'s parser, the
/// permissions-table parser) applied to the rendered string, so the writer
/// never has to know what a valid file looks like.
///
/// A missing file is treated as an empty document, so the first write of a
/// store that does not exist yet creates it.
pub fn atomic_write_toml<E, V>(
    path: &Path,
    edit: E,
    reparse: V,
) -> Result<(), ConfigWriteError>
where
    E: FnOnce(&mut toml_edit::DocumentMut) -> Result<(), String>,
    V: FnOnce(&str) -> Result<(), String>,
{
    let _lock = acquire_write_lock(path)?;

    let original = match std::fs::read_to_string(path) {
        Ok(text) => Some(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(source) => {
            return Err(ConfigWriteError::Read {
                path: path.to_path_buf(),
                source,
            });
        }
    };

    let mut doc: toml_edit::DocumentMut = original
        .as_deref()
        .unwrap_or("")
        .parse()
        .map_err(|source| ConfigWriteError::Parse {
            path: path.to_path_buf(),
            source,
        })?;

    edit(&mut doc).map_err(ConfigWriteError::Edit)?;
    let rendered = doc.to_string();

    // The mandatory re-parse. A `toml_edit` index-assignment can synthesize a
    // structurally valid table the reader's own types reject — assigning
    // `enabled` into a `[servers.<n>]` block that no longer exists produces a
    // table with no `transport` tag — and that must abort the write, not land.
    reparse(&rendered).map_err(|reason| ConfigWriteError::Reparse {
        path: path.to_path_buf(),
        reason,
    })?;

    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent).map_err(|source| ConfigWriteError::Write {
        path: parent.to_path_buf(),
        source,
    })?;

    let basename = file_name(path);
    let mut temp = tempfile::Builder::new()
        .prefix(&format!("{basename}.{}.", std::process::id()))
        .suffix(".tmp")
        .tempfile_in(parent)
        .map_err(|source| ConfigWriteError::Write {
            path: path.to_path_buf(),
            source,
        })?;
    temp.write_all(rendered.as_bytes())
        .and_then(|()| temp.as_file().sync_all())
        .map_err(|source| ConfigWriteError::Write {
            path: path.to_path_buf(),
            source,
        })?;

    // Rotate the version being replaced *before* the rename, so a crash
    // between the two leaves the original in place and a spare copy beside it.
    if let Some(text) = &original {
        rotate_backup(path, text);
    }

    temp.persist(path).map_err(|e| ConfigWriteError::Write {
        path: path.to_path_buf(),
        source: e.error,
    })?;
    Ok(())
}

/// Copy an unparseable store to `state/backups/<basename>.unparseable-<ts>`,
/// **once** — a second call with identical bytes returns the existing copy
/// rather than filling the directory on every boot (design §2.1, X-27).
///
/// Returns the copy's path, or `None` when there is nowhere to put it (the
/// backups directory could not be created) — never an error the caller has to
/// handle, because this is a diagnostic, not the operation.
pub fn copy_unparseable_once(path: &Path) -> Option<PathBuf> {
    let bytes = std::fs::read(path).ok()?;
    let dir = openalpaca_storage::store::backups_dir().ok()?;
    let basename = file_name(path);
    let prefix = format!("{basename}.unparseable-");

    for entry in std::fs::read_dir(&dir).ok()?.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if name.starts_with(&prefix)
            && std::fs::read(entry.path()).is_ok_and(|existing| existing == bytes)
        {
            return Some(entry.path());
        }
    }

    let target = dir.join(format!("{prefix}{}", timestamp()));
    match std::fs::write(&target, &bytes) {
        Ok(()) => {
            tracing::error!(
                path = %path.display(),
                copy = %target.display(),
                "config file is unparseable; kept a copy for repair"
            );
            Some(target)
        }
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "could not keep an unparseable copy");
            None
        }
    }
}

/// The lock guarding writes to `path`. It sits beside the file it guards, so
/// every writer of the same config agrees on it without this module resolving
/// a directory of its own.
fn acquire_write_lock(path: &Path) -> Result<file_lock::FileLock, ConfigWriteError> {
    let mut name = std::ffi::OsString::from(file_name(path));
    name.push(".lock");
    let lock_path = path.with_file_name(name);
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| ConfigWriteError::Lock {
            path: lock_path.clone(),
            source,
        })?;
    }
    let options = file_lock::FileOptions::new().write(true).create(true);
    file_lock::FileLock::lock(&lock_path, true, options).map_err(|source| ConfigWriteError::Lock {
        path: lock_path,
        source,
    })
}

/// Keep the five newest prior versions under `state/backups/` — the machine's
/// area, never beside the human's file.
fn rotate_backup(path: &Path, contents: &str) {
    let Ok(dir) = openalpaca_storage::store::backups_dir() else {
        tracing::warn!(path = %path.display(), "no backups directory; skipping rotation");
        return;
    };
    let basename = file_name(path);
    let prefix = format!("{basename}.bak.");
    let target = dir.join(format!("{prefix}{}", timestamp()));
    if let Err(e) = std::fs::write(&target, contents) {
        tracing::warn!(path = %target.display(), error = %e, "config backup failed");
        return;
    }

    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    let mut existing: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(&prefix))
        })
        .collect();
    // The stamp is fixed-width and monotonic, so lexical order is age order.
    existing.sort();
    while existing.len() > BACKUPS_KEPT {
        let oldest = existing.remove(0);
        if let Err(e) = std::fs::remove_file(&oldest) {
            tracing::warn!(path = %oldest.display(), error = %e, "could not rotate out an old backup");
        }
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("config.toml")
        .to_string()
}

/// A fixed-width, sortable, filename-safe stamp. Nanoseconds, because two
/// writes in the same second are ordinary under a test or a rapid toggle.
fn timestamp() -> String {
    chrono::Utc::now().format("%Y%m%dT%H%M%S%.9fZ").to_string()
}

#[cfg(test)]
mod tests;

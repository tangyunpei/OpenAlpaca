//! Unified path management for OpenAlpaca components.
//!
//! All daemon/GUI/CLI share the same app directory via `directories` crate.
//! On macOS: ~/Library/Application Support/OpenAlpaca/

use anyhow::Context;
use directories::ProjectDirs;
use std::path::PathBuf;

const QUALIFIER: &str = "";
const ORG: &str = "";
const APP: &str = "OpenAlpaca";

/// Returns the application data directory.
/// - macOS: ~/Library/Application Support/OpenAlpaca/
/// - Linux: ~/.local/share/openalpaca/
/// - Windows: C:\Users\<User>\AppData\Roaming\OpenAlpaca\data\
pub fn app_dir() -> anyhow::Result<PathBuf> {
    let proj = ProjectDirs::from(QUALIFIER, ORG, APP)
        .context("Failed to determine project directories")?;
    Ok(proj.data_dir().to_path_buf())
}

/// One-time migration: rename legacy `com.openalpaca.OpenAlpaca` app dir to `OpenAlpaca`.
///
/// Must be called before any other `app_dir()` consumers (e.g. singleton lock,
/// database open) so the rename happens while no files are held open.
pub fn migrate_legacy_app_dir() {
    let old =
        ProjectDirs::from("com", "openalpaca", "OpenAlpaca").map(|p| p.data_dir().to_path_buf());
    let new = app_dir().ok();

    if let (Some(old), Some(new)) = (old, new) {
        if old == new {
            return; // constants unchanged, no-op
        }
        if old.exists() && !new.exists() {
            if let Some(parent) = new.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            match std::fs::rename(&old, &new) {
                Ok(()) => tracing::info!("Migrated app dir: {} → {}", old.display(), new.display()),
                Err(e) => tracing::warn!(
                    "Failed to migrate app dir: {e}. Old: {}, New: {}",
                    old.display(),
                    new.display()
                ),
            }
        }
    }
}

/// Path to discovery.json (daemon advertises its connection info here).
pub fn discovery_path() -> anyhow::Result<PathBuf> {
    Ok(app_dir()?.join("discovery.json"))
}

/// Path to the singleton lock file (prevents multiple daemon instances).
pub fn lock_path() -> anyhow::Result<PathBuf> {
    Ok(app_dir()?.join("openalpacad.lock"))
}

/// Path to the SQLite database file.
pub fn database_path() -> anyhow::Result<PathBuf> {
    Ok(app_dir()?.join("openalpaca.db"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_paths_are_consistent() {
        let app = app_dir().unwrap();
        let discovery = discovery_path().unwrap();
        let lock = lock_path().unwrap();

        assert!(discovery.starts_with(&app));
        assert!(lock.starts_with(&app));
        assert!(discovery.ends_with("discovery.json"));
        assert!(lock.ends_with("openalpacad.lock"));
    }
}

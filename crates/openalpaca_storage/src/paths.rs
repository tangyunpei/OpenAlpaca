//! Unified path management for OpenAlpaca components.
//!
//! All daemon/GUI/CLI share the same app directory via `directories` crate.
//! On macOS: ~/Library/Application Support/OpenAlpaca/

use anyhow::Context;
use directories::ProjectDirs;
use std::path::PathBuf;

const QUALIFIER: &str = "com";
const ORG: &str = "openalpaca";
const APP: &str = "OpenAlpaca";

/// Returns the application data directory.
/// - macOS: ~/Library/Application Support/com.openalpaca.OpenAlpaca/
/// - Linux: ~/.local/share/OpenAlpaca/
/// - Windows: C:\Users\<User>\AppData\Roaming\OpenAlpaca\
pub fn app_dir() -> anyhow::Result<PathBuf> {
    let proj = ProjectDirs::from(QUALIFIER, ORG, APP)
        .context("Failed to determine project directories")?;
    Ok(proj.data_dir().to_path_buf())
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

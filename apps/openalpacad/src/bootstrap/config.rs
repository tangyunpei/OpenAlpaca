//! Config directory resolution and default config seeding.

use std::path::{Path, PathBuf};
use tracing::{info, warn};

/// Resolve the config base directory.
///
/// Priority order:
/// 1. `OPENALPACA_CONFIG_DIR` env var (explicit override, e.g. set by Tauri)
/// 2. Walk upward from `current_exe()` looking for a parent that contains `config/llm.toml`
///    (handles `target/debug/openalpacad` in dev builds)
/// 3. Walk upward from `current_dir()` looking for the same sentinel
/// 4. Fallback: `current_dir()/config`
pub fn resolve_config_base_dir() -> PathBuf {
    // 1. Explicit env var override
    if let Ok(dir) = std::env::var("OPENALPACA_CONFIG_DIR") {
        let p = PathBuf::from(dir);
        if p.exists() {
            return p;
        }
        tracing::warn!(
            "OPENALPACA_CONFIG_DIR={} does not exist, ignoring",
            p.display()
        );
    }

    // Helper: walk up from `start` looking for a dir that contains config/llm.toml
    fn find_config_upward(start: &Path) -> Option<PathBuf> {
        let mut dir = start;
        loop {
            let candidate = dir.join("config");
            if candidate.join("llm.toml").exists() {
                return Some(candidate);
            }
            dir = dir.parent()?;
        }
    }

    // 2. Walk up from exe directory (handles target/debug/)
    if let Some(found) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().and_then(find_config_upward))
    {
        return found;
    }

    // 3. Walk up from CWD
    if let Some(found) = std::env::current_dir()
        .ok()
        .and_then(|p| find_config_upward(&p))
    {
        return found;
    }

    // 4. Last resort fallback
    warn!(
        "Config directory not found via OPENALPACA_CONFIG_DIR, exe path, or CWD walk; \
         falling back to $CWD/config which may not exist"
    );
    std::env::current_dir().unwrap_or_default().join("config")
}

const DEFAULT_LLM_TOML: &str =
    include_str!("../../../../scripts/release/templates/config/llm.toml");
const DEFAULT_DAEMON_TOML: &str =
    include_str!("../../../../scripts/release/templates/config/daemon.toml");
/// The MCP declaration store. Seeded fully commented, because it is also the
/// **toggle** store (extension design §5): every `watch_paths` push is guarded
/// by `if path.exists()`, so without a seeded file the watcher never binds and
/// a hand edit never applies.
const DEFAULT_MCP_TOML: &str =
    include_str!("../../../../scripts/release/templates/config/mcp.toml");

/// Seed default configuration files if they don't exist yet.
///
/// Called after `resolve_config_base_dir()` so that a fresh install (or GUI
/// launching the daemon before `install.sh` runs) gets working defaults
/// instead of an empty config directory.
pub fn seed_default_configs(config_dir: &Path) {
    if let Err(e) = std::fs::create_dir_all(config_dir) {
        warn!("Cannot create config dir {}: {e}", config_dir.display());
        return;
    }

    let llm_path = config_dir.join("llm.toml");
    if !llm_path.exists() {
        match std::fs::write(&llm_path, DEFAULT_LLM_TOML) {
            Ok(()) => info!("Seeded default config: {}", llm_path.display()),
            Err(e) => warn!("Failed to seed {}: {e}", llm_path.display()),
        }
    }

    let daemon_path = config_dir.join("daemon.toml");
    if !daemon_path.exists() {
        match std::fs::write(&daemon_path, DEFAULT_DAEMON_TOML) {
            Ok(()) => info!("Seeded default config: {}", daemon_path.display()),
            Err(e) => warn!("Failed to seed {}: {e}", daemon_path.display()),
        }
    }

    let mcp_path = config_dir.join("mcp.toml");
    if !mcp_path.exists() {
        match std::fs::write(&mcp_path, DEFAULT_MCP_TOML) {
            Ok(()) => info!("Seeded default config: {}", mcp_path.display()),
            Err(e) => warn!("Failed to seed {}: {e}", mcp_path.display()),
        }
    }
}

use std::path::PathBuf;

/// Load environment variables from `.env` file if present.
pub fn load_dotenv() {
    dotenvy::dotenv().ok();
}

/// Resolve the state directory. Checks `OPENALPACA_STATE_DIR` env var,
/// falls back to `~/.openalpaca`.
pub fn resolve_state_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("OPENALPACA_STATE_DIR") {
        return PathBuf::from(dir);
    }
    crate::home_dir::resolve_home_dir().join(".openalpaca")
}

/// Resolve the config file path. Checks `OPENALPACA_CONFIG` env var,
/// falls back to `<state_dir>/config.yaml`.
pub fn resolve_config_path() -> PathBuf {
    if let Ok(path) = std::env::var("OPENALPACA_CONFIG") {
        return PathBuf::from(path);
    }
    resolve_state_dir().join("config.yaml")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_state_dir_default() {
        // SAFETY: test runs single-threaded; no other thread reads this env var.
        unsafe { std::env::remove_var("OPENALPACA_STATE_DIR") };
        let dir = resolve_state_dir();
        assert!(dir.ends_with(".openalpaca"));
    }

    #[test]
    fn test_resolve_config_path_default() {
        // SAFETY: test runs single-threaded; no other thread reads these env vars.
        unsafe {
            std::env::remove_var("OPENALPACA_CONFIG");
            std::env::remove_var("OPENALPACA_STATE_DIR");
        }
        let path = resolve_config_path();
        assert!(path.ends_with("config.yaml"));
    }
}

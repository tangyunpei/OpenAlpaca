use std::path::PathBuf;

/// Resolve the user's home directory. Falls back to current directory if unavailable.
pub fn resolve_home_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_home_dir_returns_path() {
        let home = resolve_home_dir();
        assert!(home.is_absolute() || home == PathBuf::from("."));
    }
}

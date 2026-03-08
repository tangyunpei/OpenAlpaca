use std::path::{Path, PathBuf};

use openalpaca_core::CoreError;
use sha2::{Digest, Sha256};

use crate::env_subst::substitute_env_vars_in_value;
use crate::types::OpenAlpacaConfig;
use crate::validation::{Validate, ValidationIssue};

pub struct ConfigSnapshot {
    pub path: PathBuf,
    pub exists: bool,
    pub config: OpenAlpacaConfig,
    pub issues: Vec<ValidationIssue>,
    pub hash: Option<String>,
}

/// Load config from YAML file, apply env substitution, validate.
pub fn load_config(path: &Path) -> Result<ConfigSnapshot, CoreError> {
    if !path.exists() {
        return Ok(ConfigSnapshot {
            path: path.to_path_buf(),
            exists: false,
            config: OpenAlpacaConfig::default(),
            issues: vec![],
            hash: None,
        });
    }

    let raw = std::fs::read_to_string(path)?;
    let hash = compute_hash(raw.as_bytes());

    let value: serde_yaml::Value = serde_yaml::from_str(&raw)
        .map_err(|e| CoreError::InvalidConfig(format!("YAML parse error: {e}")))?;

    let mut value = crate::includes::resolve_includes(value, path)?;

    let migration_changes = crate::migration::apply_migrations(&mut value);
    for change in &migration_changes {
        tracing::warn!("config migration applied: {change}");
    }

    substitute_env_vars_in_value(&mut value);

    let config: OpenAlpacaConfig = serde_yaml::from_value(value)
        .map_err(|e| CoreError::InvalidConfig(format!("config deserialization error: {e}")))?;

    let issues = config.validate();

    Ok(ConfigSnapshot {
        path: path.to_path_buf(),
        exists: true,
        config,
        issues,
        hash: Some(hash),
    })
}

/// Write config back to YAML file.
pub fn save_config(path: &Path, config: &OpenAlpacaConfig) -> Result<(), CoreError> {
    let yaml = serde_yaml::to_string(config)
        .map_err(|e| CoreError::InvalidConfig(format!("config serialization error: {e}")))?;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Atomic write: write to temp file, then rename
    let parent = path.parent().unwrap_or(Path::new("."));
    let temp_path = parent.join(format!(".config_tmp_{}", std::process::id()));
    std::fs::write(&temp_path, &yaml)?;
    std::fs::rename(&temp_path, path)?;
    Ok(())
}

fn compute_hash(data: &[u8]) -> String {
    let digest = Sha256::digest(data);
    format!("{digest:x}")
}

/// Navigate a YAML value by a dot-separated key path.
pub fn get_value_at_path(config: &OpenAlpacaConfig, path: &str) -> Option<serde_yaml::Value> {
    let value = serde_yaml::to_value(config).ok()?;
    let mut current = &value;

    for key in path.split('.') {
        match current {
            serde_yaml::Value::Mapping(map) => {
                current = map.get(serde_yaml::Value::String(key.to_string()))?;
            }
            _ => return None,
        }
    }

    Some(current.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_load_nonexistent_config() {
        let result = load_config(Path::new("/nonexistent/config.yaml")).unwrap();
        assert!(!result.exists);
        assert!(result.hash.is_none());
    }

    #[test]
    fn test_load_and_save_roundtrip() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.yaml");

        let yaml = r#"
gateway:
  port: 3777
channels:
  telegram:
    bot_token: "test-token"
"#;
        std::fs::write(&path, yaml).unwrap();

        let snapshot = load_config(&path).unwrap();
        assert!(snapshot.exists);
        assert!(snapshot.hash.is_some());
        assert_eq!(snapshot.config.gateway.as_ref().unwrap().port, Some(3777));

        let save_path = dir.path().join("config2.yaml");
        save_config(&save_path, &snapshot.config).unwrap();

        let snapshot2 = load_config(&save_path).unwrap();
        assert_eq!(
            snapshot2.config.gateway.as_ref().unwrap().port,
            snapshot.config.gateway.as_ref().unwrap().port
        );
    }

    #[test]
    fn test_load_malformed_yaml() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("bad.yaml");
        std::fs::write(&path, "{{invalid yaml").unwrap();

        let result = load_config(&path);
        assert!(result.is_err());
    }

    #[test]
    fn test_load_with_env_substitution() {
        // SAFETY: test-only env var manipulation
        unsafe { std::env::set_var("TEST_CONFIG_TOKEN", "my-secret") };

        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.yaml");
        std::fs::write(
            &path,
            "channels:\n  telegram:\n    bot_token: \"${TEST_CONFIG_TOKEN}\"",
        )
        .unwrap();

        let snapshot = load_config(&path).unwrap();
        let tg = snapshot.config.channels.unwrap().telegram.unwrap();
        assert_eq!(tg.default.bot_token.as_deref(), Some("my-secret"));

        unsafe { std::env::remove_var("TEST_CONFIG_TOKEN") };
    }

    #[test]
    fn test_get_value_at_path() {
        let yaml = r#"
gateway:
  port: 3777
  host: "localhost"
channels:
  telegram:
    enabled: true
"#;
        let config: OpenAlpacaConfig = serde_yaml::from_str(yaml).unwrap();

        let port = get_value_at_path(&config, "gateway.port").unwrap();
        assert_eq!(port.as_u64(), Some(3777));

        let host = get_value_at_path(&config, "gateway.host").unwrap();
        assert_eq!(host.as_str(), Some("localhost"));

        assert!(get_value_at_path(&config, "nonexistent.path").is_none());
    }

    #[test]
    fn test_validation_issues_in_snapshot() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.yaml");
        std::fs::write(&path, "gateway:\n  port: 0").unwrap();

        let snapshot = load_config(&path).unwrap();
        assert_eq!(snapshot.issues.len(), 1);
    }
}

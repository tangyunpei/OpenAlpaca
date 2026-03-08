use std::path::Path;

use openalpaca_core::CoreError;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::fs_safe;

/// Read and deserialize a JSON file.
pub async fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T, CoreError> {
    let data = fs_safe::read_file_safe(path, None).await?;
    let value = serde_json::from_slice(&data)?;
    Ok(value)
}

/// Serialize and write a JSON file atomically.
pub async fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), CoreError> {
    let data = serde_json::to_vec_pretty(value)?;
    fs_safe::write_file_atomic(path, &data).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_json_roundtrip() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.json");

        let data = serde_json::json!({"key": "value", "num": 42});
        write_json(&path, &data).await.unwrap();

        let loaded: serde_json::Value = read_json(&path).await.unwrap();
        assert_eq!(loaded, data);
    }

    #[tokio::test]
    async fn test_read_json_invalid() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("bad.json");
        tokio::fs::write(&path, b"not json").await.unwrap();

        let result = read_json::<serde_json::Value>(&path).await;
        assert!(result.is_err());
    }
}

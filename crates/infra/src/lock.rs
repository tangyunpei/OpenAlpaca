use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use openalpaca_core::CoreError;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct LockInfo {
    pid: u32,
    created_at: u64,
    #[serde(default)]
    config_path: String,
}

/// Handle to an acquired lock file. Callers should use `release()` for async cleanup.
/// If dropped without calling `release()`, a sync best-effort cleanup runs in `Drop`.
pub struct LockHandle {
    path: Option<PathBuf>,
}

impl LockHandle {
    /// Async release — removes the lock file. Preferred over letting Drop handle it.
    pub async fn release(mut self) -> Result<(), CoreError> {
        if let Some(path) = self.path.take() {
            tokio::fs::remove_file(&path).await?;
        }
        Ok(())
    }
}

impl Drop for LockHandle {
    fn drop(&mut self) {
        if let Some(ref path) = self.path {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Acquire a lock file at `path`. Retries until `timeout` expires, checking for stale locks.
pub async fn acquire_lock(path: &Path, config_path: &Path, timeout: Duration) -> Result<LockHandle, CoreError> {
    let deadline = tokio::time::Instant::now() + timeout;

    loop {
        match try_create_lock(path, config_path) {
            Ok(handle) => return Ok(handle),
            Err(_) => {
                if try_remove_stale(path).await {
                    continue;
                }

                if tokio::time::Instant::now() >= deadline {
                    return Err(CoreError::Lock(format!(
                        "timed out waiting for lock at {}",
                        path.display()
                    )));
                }

                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
}

fn try_create_lock(path: &Path, config_path: &Path) -> Result<LockHandle, CoreError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let info = LockInfo {
        pid: std::process::id(),
        created_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        config_path: config_path.display().to_string(),
    };
    let data = serde_json::to_vec_pretty(&info).map_err(CoreError::Serialization)?;

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| CoreError::Lock(format!("failed to create lock file: {e}")))?;

    file.write_all(&data)?;

    Ok(LockHandle {
        path: Some(path.to_path_buf()),
    })
}

async fn try_remove_stale(path: &Path) -> bool {
    let data = match tokio::fs::read(path).await {
        Ok(data) => data,
        Err(_) => return false,
    };

    let info: LockInfo = match serde_json::from_slice(&data) {
        Ok(info) => info,
        Err(_) => {
            // Corrupted lock file — remove it
            let _ = tokio::fs::remove_file(path).await;
            return true;
        }
    };

    if !is_pid_alive(info.pid) {
        let _ = tokio::fs::remove_file(path).await;
        return true;
    }

    false
}

fn is_pid_alive(pid: u32) -> bool {
    std::process::Command::new("ps")
        .args(["-p", &pid.to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_acquire_and_release_lock() {
        let dir = TempDir::new().unwrap();
        let lock_path = dir.path().join("test.lock");

        let handle = acquire_lock(&lock_path, Path::new("/tmp/test-config.yaml"), Duration::from_secs(1))
            .await
            .unwrap();
        assert!(lock_path.exists());

        handle.release().await.unwrap();
        assert!(!lock_path.exists());
    }

    #[tokio::test]
    async fn test_lock_prevents_double_acquire() {
        let dir = TempDir::new().unwrap();
        let lock_path = dir.path().join("test.lock");

        let handle = acquire_lock(&lock_path, Path::new("/tmp/test-config.yaml"), Duration::from_secs(1))
            .await
            .unwrap();

        let result = acquire_lock(&lock_path, Path::new("/tmp/test-config.yaml"), Duration::from_millis(200)).await;
        assert!(result.is_err());

        handle.release().await.unwrap();
    }

    #[tokio::test]
    async fn test_drop_removes_lock_file() {
        let dir = TempDir::new().unwrap();
        let lock_path = dir.path().join("test.lock");

        {
            let _handle = acquire_lock(&lock_path, Path::new("/tmp/test-config.yaml"), Duration::from_secs(1))
                .await
                .unwrap();
            assert!(lock_path.exists());
        }
        assert!(!lock_path.exists());
    }

    #[test]
    fn test_is_pid_alive_current_process() {
        assert!(is_pid_alive(std::process::id()));
    }

    #[test]
    fn test_is_pid_alive_nonexistent() {
        assert!(!is_pid_alive(4_000_000));
    }

    #[tokio::test]
    async fn test_stale_lock_is_recovered() {
        let dir = TempDir::new().unwrap();
        let lock_path = dir.path().join("test.lock");

        // Create a lock with a dead PID
        let info = LockInfo {
            pid: 4_000_000,
            created_at: 0,
            config_path: String::new(),
        };
        let data = serde_json::to_vec_pretty(&info).unwrap();
        std::fs::write(&lock_path, &data).unwrap();

        // Should succeed because the stale lock gets cleaned up
        let handle = acquire_lock(&lock_path, Path::new("/tmp/test-config.yaml"), Duration::from_secs(1))
            .await
            .unwrap();
        handle.release().await.unwrap();
    }
}

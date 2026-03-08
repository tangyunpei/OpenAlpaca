use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use openalpaca_core::CoreError;

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Read a file, optionally enforcing a max size limit.
pub async fn read_file_safe(path: &Path, max_bytes: Option<u64>) -> Result<Vec<u8>, CoreError> {
    let metadata = tokio::fs::metadata(path).await?;
    if let Some(max) = max_bytes
        && metadata.len() > max
    {
        return Err(CoreError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("file {} exceeds max size of {max} bytes", path.display()),
        )));
    }
    let data = tokio::fs::read(path).await?;
    Ok(data)
}

/// Write data to a file atomically (write to temp file, then rename).
pub async fn write_file_atomic(path: &Path, data: &[u8]) -> Result<(), CoreError> {
    let parent = path.parent().ok_or_else(|| {
        CoreError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "path has no parent directory",
        ))
    })?;
    tokio::fs::create_dir_all(parent).await?;

    let temp_path = parent.join(format!(
        ".tmp.{}.{}",
        std::process::id(),
        TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    tokio::fs::write(&temp_path, data).await?;

    if let Err(e) = tokio::fs::rename(&temp_path, path).await {
        let _ = tokio::fs::remove_file(&temp_path).await;
        return Err(e.into());
    }

    Ok(())
}

/// Check whether `candidate` is inside `root` directory. Returns false if either
/// path cannot be canonicalized (e.g., does not exist).
pub fn is_path_inside(root: &Path, candidate: &Path) -> bool {
    match (root.canonicalize(), candidate.canonicalize()) {
        (Ok(r), Ok(c)) => c.starts_with(&r),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_write_file_atomic_creates_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.txt");

        write_file_atomic(&path, b"hello world").await.unwrap();

        let content = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(content, "hello world");
    }

    #[tokio::test]
    async fn test_write_file_atomic_creates_parent_dirs() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("sub").join("dir").join("test.txt");

        write_file_atomic(&path, b"nested").await.unwrap();

        let content = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(content, "nested");
    }

    #[tokio::test]
    async fn test_write_file_atomic_overwrites() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.txt");

        write_file_atomic(&path, b"first").await.unwrap();
        write_file_atomic(&path, b"second").await.unwrap();

        let content = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(content, "second");
    }

    #[tokio::test]
    async fn test_read_file_safe() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.txt");
        tokio::fs::write(&path, b"hello").await.unwrap();

        let data = read_file_safe(&path, None).await.unwrap();
        assert_eq!(data, b"hello");
    }

    #[tokio::test]
    async fn test_read_file_safe_rejects_oversized() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("big.txt");
        tokio::fs::write(&path, b"hello").await.unwrap();

        let result = read_file_safe(&path, Some(3)).await;
        assert!(result.is_err());
    }

    #[test]
    fn test_is_path_inside_true() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let child = root.join("sub").join("file.txt");
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(&child, "").unwrap();

        assert!(is_path_inside(root, &child));
    }

    #[test]
    fn test_is_path_inside_false_different_dirs() {
        let dir1 = TempDir::new().unwrap();
        let dir2 = TempDir::new().unwrap();

        assert!(!is_path_inside(dir1.path(), dir2.path()));
    }

    #[test]
    fn test_is_path_inside_false_nonexistent() {
        let dir = TempDir::new().unwrap();
        let fake = dir.path().join("nonexistent");

        assert!(!is_path_inside(dir.path(), &fake));
    }
}

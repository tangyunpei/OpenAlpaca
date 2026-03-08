use std::path::{Path, PathBuf};

use openalpaca_core::CoreError;
use serde::{Deserialize, Serialize};

use crate::transcript::LastRoute;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionEntry {
    pub session_key: String,
    pub channel: String,
    pub account_id: String,
    pub peer_id: Option<String>,
    pub chat_type: Option<String>,
    pub last_route: Option<LastRoute>,
    pub updated_at: String,
}

/// JSONL-based session persistence. Each line is a JSON-serialized `SessionEntry`.
pub struct SessionStore {
    path: PathBuf,
}

impl SessionStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// List all stored sessions.
    pub async fn list(&self) -> Result<Vec<SessionEntry>, CoreError> {
        if !self.path.exists() {
            return Ok(vec![]);
        }

        let content = tokio::fs::read_to_string(&self.path).await?;
        let mut entries = Vec::new();
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            match serde_json::from_str::<SessionEntry>(trimmed) {
                Ok(entry) => entries.push(entry),
                Err(e) => {
                    tracing::warn!("skipping invalid session entry: {e}");
                }
            }
        }
        Ok(entries)
    }

    /// Append a session entry to the store.
    pub async fn record(&self, entry: &SessionEntry) -> Result<(), CoreError> {
        Self::ensure_parent(&self.path).await?;

        let mut line = serde_json::to_string(entry)?;
        line.push('\n');

        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .await?;
        file.write_all(line.as_bytes()).await?;
        file.flush().await?;

        Ok(())
    }

    /// Remove a session by key. Rewrites the file without the matching entry.
    pub async fn remove(&self, session_key: &str) -> Result<(), CoreError> {
        if !self.path.exists() {
            return Ok(());
        }

        let entries = self.list().await?;
        let filtered: Vec<_> = entries
            .iter()
            .filter(|e| e.session_key != session_key)
            .collect();

        let mut content = String::new();
        for entry in &filtered {
            let line = serde_json::to_string(entry)?;
            content.push_str(&line);
            content.push('\n');
        }

        tokio::fs::write(&self.path, content).await?;
        Ok(())
    }

    async fn ensure_parent(path: &Path) -> Result<(), CoreError> {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        Ok(())
    }
}

// Need AsyncWriteExt for write_all_buf
use tokio::io::AsyncWriteExt;

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_entry(key: &str, channel: &str) -> SessionEntry {
        SessionEntry {
            session_key: key.to_string(),
            channel: channel.to_string(),
            account_id: "default".to_string(),
            peer_id: None,
            chat_type: Some("direct".to_string()),
            last_route: None,
            updated_at: "2026-01-01T00:00:00Z".to_string(),
        }
    }

    #[tokio::test]
    async fn test_empty_store() {
        let dir = TempDir::new().unwrap();
        let store = SessionStore::new(dir.path().join("sessions.jsonl"));
        let entries = store.list().await.unwrap();
        assert!(entries.is_empty());
    }

    #[tokio::test]
    async fn test_record_and_list() {
        let dir = TempDir::new().unwrap();
        let store = SessionStore::new(dir.path().join("sessions.jsonl"));

        let entry = make_entry("key1", "telegram");
        store.record(&entry).await.unwrap();

        let entries = store.list().await.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].session_key, "key1");
        assert_eq!(entries[0].channel, "telegram");
    }

    #[tokio::test]
    async fn test_record_multiple() {
        let dir = TempDir::new().unwrap();
        let store = SessionStore::new(dir.path().join("sessions.jsonl"));

        store.record(&make_entry("k1", "telegram")).await.unwrap();
        store.record(&make_entry("k2", "discord")).await.unwrap();

        let entries = store.list().await.unwrap();
        assert_eq!(entries.len(), 2);
    }

    #[tokio::test]
    async fn test_remove_session() {
        let dir = TempDir::new().unwrap();
        let store = SessionStore::new(dir.path().join("sessions.jsonl"));

        store.record(&make_entry("k1", "telegram")).await.unwrap();
        store.record(&make_entry("k2", "discord")).await.unwrap();

        store.remove("k1").await.unwrap();

        let entries = store.list().await.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].session_key, "k2");
    }

    #[tokio::test]
    async fn test_roundtrip_with_last_route() {
        let dir = TempDir::new().unwrap();
        let store = SessionStore::new(dir.path().join("sessions.jsonl"));

        let mut entry = make_entry("k1", "telegram");
        entry.last_route = Some(LastRoute {
            agent_id: "assistant".to_string(),
            resolved_at: "2026-01-01T12:00:00Z".to_string(),
        });
        store.record(&entry).await.unwrap();

        let entries = store.list().await.unwrap();
        let route = entries[0].last_route.as_ref().unwrap();
        assert_eq!(route.agent_id, "assistant");
    }
}

//! Shared workspace for task agents: key-value entries with eviction policy.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// The type of a workspace entry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum WorkspaceEntryType {
    #[default]
    Text,
    Artifact,
    Summary,
    Context,
}

/// A single entry in the shared workspace.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceEntry {
    pub key: String,
    pub content: String,
    pub author_agent_id: String,
    pub entry_type: WorkspaceEntryType,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Optional file asset ID for entries backed by uploaded/generated files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_asset_id: Option<String>,
}

/// The shared workspace for a task — all agents can read/write.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskWorkspace {
    pub entries: Vec<WorkspaceEntry>,
    pub max_entries: usize,
    pub max_entry_size: usize,
}

impl Default for TaskWorkspace {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            max_entries: 50,
            max_entry_size: 32768,
        }
    }
}

impl TaskWorkspace {
    /// Read a single entry by key, or return all entries if key is empty.
    pub fn read(&self, key: &str) -> Vec<&WorkspaceEntry> {
        if key.is_empty() {
            self.entries.iter().collect()
        } else {
            self.entries.iter().filter(|e| e.key == key).collect()
        }
    }

    /// List all keys with their types (for discovery).
    pub fn list_keys(&self) -> Vec<(&str, &WorkspaceEntryType)> {
        self.entries
            .iter()
            .map(|e| (e.key.as_str(), &e.entry_type))
            .collect()
    }

    /// Write (upsert) an entry. Returns Ok on success, Err if limits exceeded.
    pub fn write(
        &mut self,
        key: &str,
        content: &str,
        author_agent_id: &str,
        entry_type: WorkspaceEntryType,
        protected_keys: &[String],
    ) -> Result<(), String> {
        if content.len() > self.max_entry_size {
            tracing::warn!(
                "Workspace entry '{}' truncated from {} to {} chars",
                key,
                content.len(),
                self.max_entry_size
            );
        }
        let capped_content: String = content.chars().take(self.max_entry_size).collect();
        let now = Utc::now();

        // Upsert: update if key exists, insert otherwise
        if let Some(existing) = self.entries.iter_mut().find(|e| e.key == key) {
            existing.content = capped_content;
            existing.author_agent_id = author_agent_id.to_string();
            existing.entry_type = entry_type;
            existing.updated_at = now;
            return Ok(());
        }

        if self.entries.len() >= self.max_entries {
            if let Some(oldest_idx) = self
                .entries
                .iter()
                .enumerate()
                .filter(|(_, e)| !protected_keys.contains(&e.key))
                .min_by_key(|(_, e)| e.updated_at)
                .map(|(i, _)| i)
            {
                let evicted_key = self.entries[oldest_idx].key.clone();
                self.entries.remove(oldest_idx);
                tracing::debug!(
                    "Workspace full — evicted oldest entry '{}' to make room for '{}'",
                    evicted_key,
                    key
                );
            } else {
                return Err(format!(
                    "Workspace full ({} entries) and all are protected — cannot write '{}'",
                    self.max_entries, key
                ));
            }
        }

        self.entries.push(WorkspaceEntry {
            key: key.to_string(),
            content: capped_content,
            author_agent_id: author_agent_id.to_string(),
            entry_type,
            created_at: now,
            updated_at: now,
            file_asset_id: None,
        });
        Ok(())
    }

    /// Associate a file asset ID with an existing workspace entry.
    pub fn set_file_asset_id(&mut self, key: &str, file_asset_id: &str) {
        if let Some(entry) = self.entries.iter_mut().find(|e| e.key == key) {
            entry.file_asset_id = Some(file_asset_id.to_string());
        }
    }

    /// Format workspace contents as a context string for agent prompts.
    pub fn format_for_prompt(&self, keys: &[String]) -> String {
        let relevant: Vec<&WorkspaceEntry> = if keys.is_empty() {
            self.entries.iter().collect()
        } else {
            self.entries
                .iter()
                .filter(|e| keys.contains(&e.key))
                .collect()
        };

        if relevant.is_empty() {
            return String::new();
        }

        let mut out = String::from("## Shared Workspace\n\n");
        for entry in relevant {
            let preview: String = entry.content.chars().take(2000).collect();
            out.push_str(&format!(
                "### [{}] (by {}, type: {:?})\n{}\n\n",
                entry.key, entry.author_agent_id, entry.entry_type, preview
            ));
        }
        out
    }
}

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PreferenceValue {
    pub key: String,
    pub value: serde_json::Value,
    pub source: String,  // "user", "gui", "implicit"
    pub confidence: f32, // 0.0 - 1.0
    pub version: u64,    // Optimistic Lock version
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug)]
pub enum PreferenceError {
    Conflict {
        current_version: u64,
        attempted_version: u64,
    },
    Storage(String),
}

#[async_trait]
pub trait PreferenceBackend: Send + Sync {
    /// Get a value from persistence.
    async fn get(&self, user_id: &str, key: &str) -> Result<Option<PreferenceValue>, String>;

    /// Set a value. Backend must check version if overwrite behavior is "optimistic".
    /// If storage detects version mismatch, return error.
    async fn set(&self, user_id: &str, val: PreferenceValue) -> Result<(), String>;
}

/// The Logic Layer for Preferences.
/// Handles Conflict Resolution, Optimistic Locking check (pre-flight), and Audit hooks.
pub struct PreferenceStore {
    backend: Box<dyn PreferenceBackend>,
}

impl PreferenceStore {
    pub fn new(backend: Box<dyn PreferenceBackend>) -> Self {
        Self { backend }
    }

    pub async fn get(
        &self,
        user_id: &str,
        key: &str,
    ) -> Result<Option<PreferenceValue>, PreferenceError> {
        self.backend
            .get(user_id, key)
            .await
            .map_err(PreferenceError::Storage)
    }

    /// Set a preference with Optimistic Locking logic.
    pub async fn set(
        &self,
        user_id: &str,
        mut new_val: PreferenceValue,
    ) -> Result<(), PreferenceError> {
        // 1. Fetch existing to check version/confidence logic
        let existing = self
            .backend
            .get(user_id, &new_val.key)
            .await
            .map_err(PreferenceError::Storage)?;

        if let Some(current) = existing {
            // Optimistic Lock Check:
            // If new_val.version is specified, it MUST match current.version (Update based on X)
            // If new_val.version is 0, it might act as "Force" or "New", but safe to check.

            // Logic: The caller should have passed `current.version` in `new_val.version` to prove they saw it.
            // The storage will increment it to `current.version + 1`.
            if new_val.version != current.version {
                // Conflict!
                // Unless confidence overrides?
                // Rule: Explicit(User) overrides Implicit even if version mismatch?
                // For now, strict version check for safety.
                return Err(PreferenceError::Conflict {
                    current_version: current.version,
                    attempted_version: new_val.version,
                });
            }

            // Confidence Resolution
            if new_val.confidence < current.confidence && new_val.source == "implicit" {
                // Implicit guess cannot overwrite High Confidence value
                return Ok(()); // Ignore silently? Or return error?
            }

            // Increment version for storage
            new_val.version += 1;
        } else {
            // First create, version start at 1
            new_val.version = 1;
        }

        // TODO: Audit Log (History) before write

        self.backend
            .set(user_id, new_val)
            .await
            .map_err(PreferenceError::Storage)
    }
}

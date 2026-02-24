//! Identity repository - CRUD operations for identity-related entities
//!
//! Handles GlobalUser, ExternalIdentity, ConversationMap, and LinkToken.

use crate::Database;
use crate::models::identity::{ConversationMap, ExternalIdentity, GlobalUser, LinkToken};
use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};

/// Repository for Identity operations
pub struct IdentityRepository<'a> {
    db: &'a Database,
}

impl<'a> IdentityRepository<'a> {
    pub fn new(db: &'a Database) -> Self {
        Self { db }
    }

    // ========== GlobalUser ==========

    /// Create a new global user
    pub fn create_global_user(&self, id: &str, display_name: Option<&str>) -> Result<GlobalUser> {
        let now = Utc::now();
        self.db.with_connection(|conn| {
            conn.execute(
                "INSERT INTO global_user (id, display_name, created_at, updated_at) VALUES (?1, ?2, ?3, ?4)",
                (id, display_name, now.to_rfc3339(), now.to_rfc3339()),
            )?;
            Ok(GlobalUser {
                id: id.to_string(),
                display_name: display_name.map(|s| s.to_string()),
                created_at: now,
                updated_at: now,
            })
        })
    }

    /// Get global user by ID
    pub fn get_global_user(&self, id: &str) -> Result<Option<GlobalUser>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, display_name, created_at, updated_at FROM global_user WHERE id = ?1",
            )?;
            let mut rows = stmt.query([id])?;

            match rows.next()? {
                Some(row) => Ok(Some(Self::row_to_global_user(row)?)),
                None => Ok(None),
            }
        })
    }

    fn row_to_global_user(row: &rusqlite::Row<'_>) -> Result<GlobalUser> {
        let id: String = row.get(0)?;
        let display_name: Option<String> = row.get(1)?;
        let created_at_str: String = row.get(2)?;
        let updated_at_str: String = row.get(3)?;

        let created_at = DateTime::parse_from_rfc3339(&created_at_str)
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now());
        let updated_at = DateTime::parse_from_rfc3339(&updated_at_str)
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now());

        Ok(GlobalUser {
            id,
            display_name,
            created_at,
            updated_at,
        })
    }

    /// Get the first global user (ordered by created_at).
    /// Used by iMessage connector for auto-linking to the Mac owner.
    pub fn get_first_global_user(&self) -> Result<Option<GlobalUser>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, display_name, created_at, updated_at FROM global_user ORDER BY created_at ASC LIMIT 1",
            )?;
            let mut rows = stmt.query([])?;
            match rows.next()? {
                Some(row) => Ok(Some(Self::row_to_global_user(row)?)),
                None => Ok(None),
            }
        })
    }

    // ========== ExternalIdentity ==========

    /// Get or create external identity
    pub fn get_or_create_external_identity(
        &self,
        provider: &str,
        provider_user_id: &str,
        display_name: Option<&str>,
    ) -> Result<ExternalIdentity> {
        // Try to find existing
        if let Some(existing) = self.get_external_identity(provider, provider_user_id)? {
            return Ok(existing);
        }

        // Create new
        let now = Utc::now();
        self.db.with_connection(|conn| {
            conn.execute(
                "INSERT INTO external_identity (provider, provider_user_id, display_name, created_at) VALUES (?1, ?2, ?3, ?4)",
                (provider, provider_user_id, display_name, now.to_rfc3339()),
            )?;
            let id = conn.last_insert_rowid();
            Ok(ExternalIdentity {
                id,
                provider: provider.to_string(),
                provider_user_id: provider_user_id.to_string(),
                global_user_id: None,
                display_name: display_name.map(|s| s.to_string()),
                metadata: None,
                created_at: now,
                linked_at: None,
            })
        })
    }

    /// Get external identity by provider and user ID
    pub fn get_external_identity(
        &self,
        provider: &str,
        provider_user_id: &str,
    ) -> Result<Option<ExternalIdentity>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, provider, provider_user_id, global_user_id, display_name, metadata, created_at, linked_at 
                 FROM external_identity WHERE provider = ?1 AND provider_user_id = ?2",
            )?;
            let mut rows = stmt.query([provider, provider_user_id])?;

            match rows.next()? {
                Some(row) => Ok(Some(Self::row_to_external_identity(row)?)),
                None => Ok(None),
            }
        })
    }

    /// Link external identity to global user
    pub fn link_external_identity(
        &self,
        external_identity_id: i64,
        global_user_id: &str,
    ) -> Result<()> {
        let now = Utc::now();
        self.db.with_connection(|conn| {
            conn.execute(
                "UPDATE external_identity SET global_user_id = ?1, linked_at = ?2 WHERE id = ?3",
                (global_user_id, now.to_rfc3339(), external_identity_id),
            )?;
            Ok(())
        })
    }

    /// Unlink external identity (remove global user association)
    pub fn unlink_external_identity(&self, external_identity_id: i64) -> Result<()> {
        self.db.with_connection(|conn| {
            conn.execute(
                "UPDATE external_identity SET global_user_id = NULL, linked_at = NULL WHERE id = ?1",
                (external_identity_id,),
            )?;
            Ok(())
        })
    }

    fn row_to_external_identity(row: &rusqlite::Row<'_>) -> Result<ExternalIdentity> {
        let id: i64 = row.get(0)?;
        let provider: String = row.get(1)?;
        let provider_user_id: String = row.get(2)?;
        let global_user_id: Option<String> = row.get(3)?;
        let display_name: Option<String> = row.get(4)?;
        let metadata_str: Option<String> = row.get(5)?;
        let created_at_str: String = row.get(6)?;
        let linked_at_str: Option<String> = row.get(7)?;

        let metadata = metadata_str
            .map(|s| serde_json::from_str(&s))
            .transpose()
            .context("Failed to parse metadata JSON")?;

        let created_at = DateTime::parse_from_rfc3339(&created_at_str)
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now());

        let linked_at = linked_at_str.and_then(|s| {
            DateTime::parse_from_rfc3339(&s)
                .map(|dt| dt.with_timezone(&Utc))
                .ok()
        });

        Ok(ExternalIdentity {
            id,
            provider,
            provider_user_id,
            global_user_id,
            display_name,
            metadata,
            created_at,
            linked_at,
        })
    }

    // ========== LinkToken ==========

    /// Create a link token (with 5-minute expiry by default)
    pub fn create_link_token(&self, global_user_id: &str, token: &str) -> Result<LinkToken> {
        let now = Utc::now();
        let expires_at = now + Duration::minutes(5);

        self.db.with_connection(|conn| {
            conn.execute(
                "INSERT INTO link_token (token, global_user_id, expires_at, created_at) VALUES (?1, ?2, ?3, ?4)",
                (token, global_user_id, expires_at.to_rfc3339(), now.to_rfc3339()),
            )?;
            let id = conn.last_insert_rowid();
            Ok(LinkToken {
                id,
                token: token.to_string(),
                global_user_id: global_user_id.to_string(),
                expires_at,
                used_at: None,
                created_at: now,
            })
        })
    }

    /// Consume a link token (returns GlobalUser ID if valid)
    pub fn consume_link_token(&self, token: &str) -> Result<Option<String>> {
        let now = Utc::now();

        self.db.with_connection(|conn| {
            // Check if token exists, not expired, and not used
            let mut stmt = conn.prepare(
                "SELECT id, global_user_id, expires_at, used_at FROM link_token WHERE token = ?1",
            )?;
            let mut rows = stmt.query([token])?;

            match rows.next()? {
                Some(row) => {
                    let id: i64 = row.get(0)?;
                    let global_user_id: String = row.get(1)?;
                    let expires_at_str: String = row.get(2)?;
                    let used_at: Option<String> = row.get(3)?;

                    // Already used?
                    if used_at.is_some() {
                        return Ok(None);
                    }

                    // Expired?
                    let expires_at = DateTime::parse_from_rfc3339(&expires_at_str)
                        .map(|dt| dt.with_timezone(&Utc))
                        .unwrap_or_else(|_| Utc::now());
                    if now > expires_at {
                        return Ok(None);
                    }

                    // Mark as used
                    conn.execute(
                        "UPDATE link_token SET used_at = ?1 WHERE id = ?2",
                        (now.to_rfc3339(), id),
                    )?;

                    Ok(Some(global_user_id))
                }
                None => Ok(None),
            }
        })
    }

    // ========== ConversationMap ==========

    /// Get or create conversation map
    pub fn get_or_create_conversation_map(
        &self,
        provider: &str,
        provider_conversation_id: &str,
        global_user_id: Option<&str>,
    ) -> Result<ConversationMap> {
        // Try to find existing
        if let Some(existing) = self.get_conversation_map(provider, provider_conversation_id)? {
            return Ok(existing);
        }

        // Create new
        let now = Utc::now();
        self.db.with_connection(|conn| {
            conn.execute(
                "INSERT INTO conversation_map (provider, provider_conversation_id, global_user_id, created_at) VALUES (?1, ?2, ?3, ?4)",
                (provider, provider_conversation_id, global_user_id, now.to_rfc3339()),
            )?;
            let id = conn.last_insert_rowid();
            Ok(ConversationMap {
                id,
                provider: provider.to_string(),
                provider_conversation_id: provider_conversation_id.to_string(),
                global_user_id: global_user_id.map(|s| s.to_string()),
                lane_key: None,
                created_at: now,
            })
        })
    }

    /// Get conversation map
    pub fn get_conversation_map(
        &self,
        provider: &str,
        provider_conversation_id: &str,
    ) -> Result<Option<ConversationMap>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, provider, provider_conversation_id, global_user_id, lane_key, created_at
                 FROM conversation_map WHERE provider = ?1 AND provider_conversation_id = ?2",
            )?;
            let mut rows = stmt.query([provider, provider_conversation_id])?;

            match rows.next()? {
                Some(row) => {
                    let id: i64 = row.get(0)?;
                    let provider: String = row.get(1)?;
                    let provider_conversation_id: String = row.get(2)?;
                    let global_user_id: Option<String> = row.get(3)?;
                    let lane_key: Option<String> = row.get(4)?;
                    let created_at_str: String = row.get(5)?;

                    let created_at = DateTime::parse_from_rfc3339(&created_at_str)
                        .map(|dt| dt.with_timezone(&Utc))
                        .unwrap_or_else(|_| Utc::now());

                    Ok(Some(ConversationMap {
                        id,
                        provider,
                        provider_conversation_id,
                        global_user_id,
                        lane_key,
                        created_at,
                    }))
                }
                None => Ok(None),
            }
        })
    }

    /// Update the lane_key on a conversation_map entry.
    pub fn update_conversation_map_lane_key(
        &self,
        provider: &str,
        provider_conversation_id: &str,
        lane_key: &str,
    ) -> Result<()> {
        self.db.with_connection(|conn| {
            // Ensure the conversation_map entry exists
            let exists: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM conversation_map WHERE provider = ?1 AND provider_conversation_id = ?2)",
                [provider, provider_conversation_id],
                |row| row.get(0),
            )?;

            if exists {
                conn.execute(
                    "UPDATE conversation_map SET lane_key = ?1 WHERE provider = ?2 AND provider_conversation_id = ?3",
                    rusqlite::params![lane_key, provider, provider_conversation_id],
                )?;
            } else {
                conn.execute(
                    "INSERT INTO conversation_map (provider, provider_conversation_id, lane_key, created_at) VALUES (?1, ?2, ?3, ?4)",
                    rusqlite::params![provider, provider_conversation_id, lane_key, Utc::now().to_rfc3339()],
                )?;
            }
            Ok(())
        })
    }

    /// Get the provider_conversation_id (e.g. Telegram chat_id) for a given lane_key and provider.
    pub fn get_chat_id_by_lane_key(&self, lane_key: &str, provider: &str) -> Result<Option<i64>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT provider_conversation_id FROM conversation_map WHERE lane_key = ?1 AND provider = ?2",
            )?;
            let mut rows = stmt.query(rusqlite::params![lane_key, provider])?;
            match rows.next()? {
                Some(row) => {
                    let id_str: String = row.get(0)?;
                    Ok(id_str.parse::<i64>().ok())
                }
                None => Ok(None),
            }
        })
    }

    /// Migrate all lane data from old provider-keyed lane to new global-user-keyed lane.
    /// Called after a successful /link to ensure messages, tasks, and preferences
    /// follow the canonical identity.
    pub fn migrate_lane_on_link(
        &self,
        provider_user_id: &str,
        global_user_id: &str,
        provider: &str,
        provider_conversation_id: &str,
    ) -> Result<()> {
        let old_lane = format!("{provider_user_id}:{provider}");
        let new_lane = format!("{global_user_id}:{provider}");

        if old_lane == new_lane {
            return Ok(());
        }

        self.db.with_connection(|conn| {
            let tx = conn.unchecked_transaction()?;

            // 1. Move conversation_messages
            tx.execute(
                "UPDATE conversation_messages SET lane_key = ?1 WHERE lane_key = ?2",
                rusqlite::params![new_lane, old_lane],
            )?;

            // 2. Move tasks
            tx.execute(
                "UPDATE task SET source_lane = ?1 WHERE source_lane = ?2",
                rusqlite::params![new_lane, old_lane],
            )?;

            // 3. Update conversation_map
            tx.execute(
                "UPDATE conversation_map SET lane_key = ?1 WHERE provider = ?2 AND provider_conversation_id = ?3",
                rusqlite::params![new_lane, provider, provider_conversation_id],
            )?;

            // 4. Move conversation_summary preference
            tx.execute(
                "UPDATE OR IGNORE preference SET user_id = ?1 WHERE user_id = ?2 AND key = 'conversation_summary'",
                rusqlite::params![new_lane, old_lane],
            )?;
            // Clean up old row if OR IGNORE skipped due to UNIQUE conflict
            tx.execute(
                "DELETE FROM preference WHERE user_id = ?1 AND key = 'conversation_summary'",
                rusqlite::params![old_lane],
            )?;

            // 5. MERGE conversations table
            let old_exists: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM conversations WHERE lane_key = ?1)",
                rusqlite::params![old_lane],
                |row| row.get(0),
            )?;
            let new_exists: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM conversations WHERE lane_key = ?1)",
                rusqlite::params![new_lane],
                |row| row.get(0),
            )?;

            match (old_exists, new_exists) {
                (true, false) => {
                    // Only old exists: rename
                    tx.execute(
                        "UPDATE conversations SET lane_key = ?1 WHERE lane_key = ?2",
                        rusqlite::params![new_lane, old_lane],
                    )?;
                }
                (true, true) => {
                    // Both exist: delete old (messages already moved in step 1)
                    tx.execute(
                        "DELETE FROM conversations WHERE lane_key = ?1",
                        rusqlite::params![old_lane],
                    )?;
                }
                _ => {
                    // old doesn't exist: nothing to merge
                }
            }

            // 6. Recompute conversations stats from actual messages
            if old_exists || new_exists {
                let msg_count: i64 = tx.query_row(
                    "SELECT COUNT(*) FROM conversation_messages WHERE lane_key = ?1",
                    rusqlite::params![new_lane],
                    |row| row.get(0),
                )?;
                let last_msg_at: Option<String> = tx.query_row(
                    "SELECT MAX(created_at) FROM conversation_messages WHERE lane_key = ?1",
                    rusqlite::params![new_lane],
                    |row| row.get(0),
                )?;
                tx.execute(
                    "UPDATE conversations SET message_count = ?1, last_message_at = ?2, updated_at = ?3 WHERE lane_key = ?4",
                    rusqlite::params![msg_count, last_msg_at, Utc::now().to_rfc3339(), new_lane],
                )?;
            }

            tx.commit()?;
            Ok(())
        })
    }

    /// Delete all data associated with a provider (Factory Reset for single connector)
    pub fn delete_data_for_provider(&self, provider: &str) -> Result<()> {
        self.db.with_connection(|conn| {
            // Delete conversation maps first (FKs usually not an issue here but good practice)
            conn.execute(
                "DELETE FROM conversation_map WHERE provider = ?1",
                [provider],
            )?;

            // Delete external identities
            conn.execute(
                "DELETE FROM external_identity WHERE provider = ?1",
                [provider],
            )?;

            Ok(())
        })
    }
}

#[cfg(test)]
mod tests;

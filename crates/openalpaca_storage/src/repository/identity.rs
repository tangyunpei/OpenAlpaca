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
                "SELECT id, provider, provider_conversation_id, global_user_id, created_at 
                 FROM conversation_map WHERE provider = ?1 AND provider_conversation_id = ?2",
            )?;
            let mut rows = stmt.query([provider, provider_conversation_id])?;

            match rows.next()? {
                Some(row) => {
                    let id: i64 = row.get(0)?;
                    let provider: String = row.get(1)?;
                    let provider_conversation_id: String = row.get(2)?;
                    let global_user_id: Option<String> = row.get(3)?;
                    let created_at_str: String = row.get(4)?;

                    let created_at = DateTime::parse_from_rfc3339(&created_at_str)
                        .map(|dt| dt.with_timezone(&Utc))
                        .unwrap_or_else(|_| Utc::now());

                    Ok(Some(ConversationMap {
                        id,
                        provider,
                        provider_conversation_id,
                        global_user_id,
                        created_at,
                    }))
                }
                None => Ok(None),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn test_db() -> Database {
        let dir = tempdir().unwrap();
        Database::open(&dir.path().join("test.db")).unwrap()
    }

    #[test]
    fn test_global_user_crud() {
        let db = test_db();
        let repo = IdentityRepository::new(&db);

        // Create
        let user = repo.create_global_user("user-1", Some("Alice")).unwrap();
        assert_eq!(user.id, "user-1");
        assert_eq!(user.display_name, Some("Alice".to_string()));

        // Read
        let fetched = repo.get_global_user("user-1").unwrap().unwrap();
        assert_eq!(fetched.display_name, Some("Alice".to_string()));
    }

    #[test]
    fn test_external_identity_link() {
        let db = test_db();
        let repo = IdentityRepository::new(&db);

        // Create global user first
        let user = repo.create_global_user("user-1", Some("Alice")).unwrap();

        // Create external identity
        let ext = repo
            .get_or_create_external_identity("telegram", "12345", Some("TG Alice"))
            .unwrap();
        assert!(ext.global_user_id.is_none());
        assert_eq!(ext.provider, "telegram");

        // Link
        repo.link_external_identity(ext.id, &user.id).unwrap();

        // Verify link
        let linked = repo
            .get_external_identity("telegram", "12345")
            .unwrap()
            .unwrap();
        assert_eq!(linked.global_user_id, Some("user-1".to_string()));
        assert!(linked.linked_at.is_some());
    }

    #[test]
    fn test_link_token_flow() {
        let db = test_db();
        let repo = IdentityRepository::new(&db);

        // Create user
        repo.create_global_user("user-1", None).unwrap();

        // Create token
        let token = repo.create_link_token("user-1", "ABC123").unwrap();
        assert_eq!(token.token, "ABC123");

        // Consume token (should succeed)
        let result = repo.consume_link_token("ABC123").unwrap();
        assert_eq!(result, Some("user-1".to_string()));

        // Consume again (should fail - already used)
        let result2 = repo.consume_link_token("ABC123").unwrap();
        assert!(result2.is_none());
    }
}

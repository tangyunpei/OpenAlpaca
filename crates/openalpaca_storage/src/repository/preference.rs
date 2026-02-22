use crate::Database;
use anyhow::{Context, Result, bail};
use rusqlite::OptionalExtension;

/// Repository for user preferences (preference table).
pub struct PreferenceRepository<'a> {
    db: &'a Database,
}

/// A single preference entry.
#[derive(Debug, Clone)]
pub struct Preference {
    pub user_id: String,
    pub key: String,
    pub value: String,
    pub version: i64,
}

impl<'a> PreferenceRepository<'a> {
    pub fn new(db: &'a Database) -> Self {
        Self { db }
    }

    /// Get a preference value by user_id and key.
    pub fn get(&self, user_id: &str, key: &str) -> Result<Option<Preference>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT user_id, key, value, version FROM preference WHERE user_id = ? AND key = ?",
            )?;
            let pref = stmt
                .query_row([user_id, key], |row| {
                    Ok(Preference {
                        user_id: row.get(0)?,
                        key: row.get(1)?,
                        value: row.get(2)?,
                        version: row.get(3)?,
                    })
                })
                .optional()
                .context("Failed to get preference")?;
            Ok(pref)
        })
    }

    /// Set a preference (upsert). If `expected_version` is Some, performs optimistic
    /// locking: the update only succeeds if the current version matches.
    /// On insert, version starts at 1. On update, version is incremented.
    pub fn set(
        &self,
        user_id: &str,
        key: &str,
        value: &str,
        expected_version: Option<i64>,
    ) -> Result<()> {
        self.db.with_connection(|conn| {
            if let Some(expected) = expected_version {
                // Optimistic lock: update only if version matches
                let rows = conn.execute(
                    "UPDATE preference SET value = ?1, version = version + 1, updated_at = CURRENT_TIMESTAMP
                     WHERE user_id = ?2 AND key = ?3 AND version = ?4",
                    rusqlite::params![value, user_id, key, expected],
                ).context("Failed to update preference")?;

                if rows == 0 {
                    bail!("Optimistic lock conflict: preference '{key}' for user '{user_id}' was modified (expected version {expected})");
                }
                Ok(())
            } else {
                // Upsert without version check
                conn.execute(
                    "INSERT INTO preference (user_id, key, value, version, updated_at)
                     VALUES (?1, ?2, ?3, 1, CURRENT_TIMESTAMP)
                     ON CONFLICT(user_id, key) DO UPDATE SET
                     value = excluded.value,
                     version = preference.version + 1,
                     updated_at = excluded.updated_at",
                    rusqlite::params![user_id, key, value],
                )
                .context("Failed to set preference")?;
                Ok(())
            }
        })
    }

    /// Delete a preference by user_id and key.
    pub fn delete(&self, user_id: &str, key: &str) -> Result<()> {
        self.db.with_connection(|conn| {
            conn.execute(
                "DELETE FROM preference WHERE user_id = ? AND key = ?",
                [user_id, key],
            )
            .context("Failed to delete preference")?;
            Ok(())
        })
    }

    /// List all preferences for a user.
    pub fn list_for_user(&self, user_id: &str) -> Result<Vec<Preference>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT user_id, key, value, version FROM preference WHERE user_id = ? ORDER BY key ASC",
            )?;
            let rows = stmt.query_map([user_id], |row| {
                Ok(Preference {
                    user_id: row.get(0)?,
                    key: row.get(1)?,
                    value: row.get(2)?,
                    version: row.get(3)?,
                })
            })?;

            let mut prefs = Vec::new();
            for row in rows {
                prefs.push(row?);
            }
            Ok(prefs)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn setup_db() -> Database {
        let dir = tempdir().unwrap();
        Database::open(&dir.path().join("test.db")).unwrap()
    }

    #[test]
    fn test_set_and_get() {
        let db = setup_db();
        let repo = PreferenceRepository::new(&db);

        repo.set("user1", "theme", "dark", None).unwrap();

        let pref = repo.get("user1", "theme").unwrap().unwrap();
        assert_eq!(pref.value, "dark");
        assert_eq!(pref.version, 1);
    }

    #[test]
    fn test_upsert_increments_version() {
        let db = setup_db();
        let repo = PreferenceRepository::new(&db);

        repo.set("user1", "lang", "en", None).unwrap();
        repo.set("user1", "lang", "zh", None).unwrap();

        let pref = repo.get("user1", "lang").unwrap().unwrap();
        assert_eq!(pref.value, "zh");
        assert_eq!(pref.version, 2);
    }

    #[test]
    fn test_optimistic_lock_success() {
        let db = setup_db();
        let repo = PreferenceRepository::new(&db);

        repo.set("user1", "color", "blue", None).unwrap();
        // Version is 1, so expected_version=1 should succeed
        repo.set("user1", "color", "red", Some(1)).unwrap();

        let pref = repo.get("user1", "color").unwrap().unwrap();
        assert_eq!(pref.value, "red");
        assert_eq!(pref.version, 2);
    }

    #[test]
    fn test_optimistic_lock_conflict() {
        let db = setup_db();
        let repo = PreferenceRepository::new(&db);

        repo.set("user1", "color", "blue", None).unwrap();
        // Version is 1, but we pass expected_version=99 -> should fail
        let result = repo.set("user1", "color", "red", Some(99));
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Optimistic lock conflict")
        );
    }

    #[test]
    fn test_delete() {
        let db = setup_db();
        let repo = PreferenceRepository::new(&db);

        repo.set("user1", "key1", "val1", None).unwrap();
        repo.delete("user1", "key1").unwrap();

        assert!(repo.get("user1", "key1").unwrap().is_none());
    }

    #[test]
    fn test_list_for_user() {
        let db = setup_db();
        let repo = PreferenceRepository::new(&db);

        repo.set("user1", "alpha", "1", None).unwrap();
        repo.set("user1", "beta", "2", None).unwrap();
        repo.set("user2", "gamma", "3", None).unwrap();

        let prefs = repo.list_for_user("user1").unwrap();
        assert_eq!(prefs.len(), 2);
        assert_eq!(prefs[0].key, "alpha");
        assert_eq!(prefs[1].key, "beta");
    }

    #[test]
    fn test_get_nonexistent() {
        let db = setup_db();
        let repo = PreferenceRepository::new(&db);

        assert!(repo.get("nobody", "nothing").unwrap().is_none());
    }
}

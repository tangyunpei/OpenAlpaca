//! Utility functions and data migration helpers.

use openalpaca_storage::repository::TaskRepository;
use openalpaca_storage::{ConfigRepository, ConversationRepository, Database, IdentityRepository};
use std::path::Path;

/// Startup orphan sweep (Routing V2 Phase 3): mark every non-terminal task
/// (queued / running / paused) as failed. In-flight execution never survives
/// a daemon restart — the tokio tasks driving them are gone — so any task
/// left non-terminal in the DB is an orphan that would otherwise look alive
/// forever in /status output and lane workflow context.
///
/// CALL-ORDER GUARANTEE: this must run right after the database opens and
/// BEFORE any ingress can create or resume work — i.e. before
/// `WakeManager::start()` (scheduler/watcher events) and before
/// `ConnectorManager::start_all()` (chat ingress) in `async_main`. Tasks
/// created after the sweep belong to this daemon generation and must not be
/// touched.
///
/// Non-fatal: failure is logged but doesn't prevent daemon startup.
pub fn sweep_orphaned_tasks(db: &Database) {
    match TaskRepository::new(db).fail_all_non_terminal("daemon restarted — task orphaned") {
        Ok(0) => {}
        Ok(count) => tracing::info!("Startup orphan sweep: failed {count} orphaned task(s)"),
        Err(e) => tracing::warn!("Startup orphan sweep failed (non-fatal): {e}"),
    }
}

pub fn is_same_file_path(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a_abs), Ok(b_abs)) => a_abs == b_abs,
        _ => false,
    }
}

/// Resolve the stable local user ID from the database. On first run, if legacy
/// `gui_user:gui` messages exist, adopt `"gui_user"` to preserve history continuity.
/// Otherwise generate a UUID.
pub fn resolve_local_user_id(db: &Database) -> String {
    let config_repo = ConfigRepository::new(db);

    // Check if we already have a persisted local user ID
    if let Ok(Some(id)) = config_repo.get("identity.local_user_id") {
        return id;
    }

    // Check for legacy gui_user:gui history
    let conv_repo = ConversationRepository::new(db);
    let local_user_id = if conv_repo.count_by_lane("gui_user:gui").unwrap_or(0) > 0 {
        "gui_user".to_string() // Preserve existing history
    } else {
        uuid::Uuid::new_v4().to_string()
    };

    // Persist for future runs
    let _ = config_repo.set("identity.local_user_id", &local_user_id, "string");

    // Ensure global_user row exists
    let identity_repo = IdentityRepository::new(db);
    if identity_repo
        .get_global_user(&local_user_id)
        .unwrap_or(None)
        .is_none()
    {
        let display_name = std::env::var("USER")
            .or_else(|_| std::env::var("USERNAME"))
            .unwrap_or_else(|_| "Local User".to_string());
        let _ = identity_repo.create_global_user(&local_user_id, Some(&display_name));
    }

    local_user_id
}

/// Migrate conversation summaries from the preference table to the conversations table.
/// Idempotent: runs every startup but does nothing if no preference rows remain.
/// Non-fatal: failure is logged but doesn't prevent daemon startup.
pub fn migrate_preference_summaries(db: &Database) {
    if let Err(e) = db.with_connection(|conn| {
        // Find all preference rows with conversation_summary
        let mut stmt = conn.prepare(
            "SELECT user_id, value, version FROM preference WHERE key = 'conversation_summary'",
        )?;
        let rows: Vec<(String, String, i64)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
            .filter_map(Result::ok)
            .collect();

        if rows.is_empty() {
            return Ok(());
        }

        let tx = conn.unchecked_transaction()?;
        let mut migrated = 0usize;
        for (lane_key, value, pref_version) in &rows {
            let parsed: serde_json::Value = match serde_json::from_str(value) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let summary = parsed.get("summary").and_then(|s| s.as_str()).unwrap_or("");
            let last_id = parsed
                .get("last_summarized_message_id")
                .and_then(|n| n.as_i64())
                .unwrap_or(0);

            // Only update if conversation row exists
            let updated = tx.execute(
                "UPDATE conversations SET summary = ?1, summary_version = ?2,
                 last_summarized_message_id = ?3, summary_updated_at = datetime('now')
                 WHERE lane_key = ?4",
                (summary, pref_version, last_id, lane_key.as_str()),
            )?;

            if updated > 0 {
                tx.execute(
                    "DELETE FROM preference WHERE user_id = ?1 AND key = 'conversation_summary'",
                    [lane_key],
                )?;
                migrated += 1;
            }
        }
        tx.commit()?;
        if migrated > 0 {
            tracing::info!(
                "Migrated {migrated} conversation summaries from preference -> conversations"
            );
        }
        Ok(())
    }) {
        tracing::warn!("Summary migration failed (non-fatal): {e}");
    }
}

//! Utility functions and data migration helpers.

use openalpaca_storage::repository::TaskRepository;
use openalpaca_storage::{ConfigRepository, Database, IdentityRepository};
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

/// Resolve the stable local user ID from the database: the persisted id if one
/// exists, otherwise a freshly minted UUID that is persisted for future runs.
pub fn resolve_local_user_id(db: &Database) -> String {
    let config_repo = ConfigRepository::new(db);

    // Check if we already have a persisted local user ID
    if let Ok(Some(id)) = config_repo.get("identity.local_user_id") {
        return id;
    }

    let local_user_id = uuid::Uuid::new_v4().to_string();

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

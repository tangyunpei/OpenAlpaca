//! Utility functions and data migration helpers.

use openalpaca_storage::repository::SubagentSpanRepository;
use openalpaca_storage::{ConfigRepository, Database, IdentityRepository};
use std::path::Path;

/// Startup span sweep (plan Phase 4, GAP-09): close every `subagent_span`
/// left `running` on a task that is already terminal, as
/// `cancelled` / `"interrupted"`.
///
/// CALL-ORDER GUARANTEE: this must run immediately after
/// [`sweep_interrupted_runs`](super::sweep_interrupted_runs), which is what
/// makes the previous generation's tasks terminal in the first place, and
/// before the router serves — a span opened by *this* daemon must never be
/// swept.
///
/// Idempotent: the second boot after a crash matches nothing. Non-fatal.
pub fn close_orphaned_spans(db: &Database) {
    match SubagentSpanRepository::new(db).close_orphans() {
        Ok(0) => {}
        Ok(count) => tracing::info!("Startup span sweep: interrupted {count} orphaned span(s)"),
        Err(e) => tracing::warn!("Startup span sweep failed (non-fatal): {e}"),
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

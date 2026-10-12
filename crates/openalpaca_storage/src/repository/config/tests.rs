//! `system_config` holds two things: the user's configuration, and the
//! machine's record of who the owner is (`identity.local_user_id`). A reset
//! that promises to preserve data must keep the second: the daemon mints a new
//! owner whenever that key is missing (`resolve_local_user_id`), and every
//! task, message, session, artifact and memory row then belongs to an id
//! nobody is any more.

use super::*;
use crate::test_util::test_db;

const OWNER_KEY: &str = "identity.local_user_id";

#[test]
fn clear_all_except_identity_keeps_the_owner_identity() {
    let db = test_db();
    let repo = ConfigRepository::new(&db);
    repo.set(OWNER_KEY, "owner-1", "string").unwrap();
    repo.set("telegram.enabled", "true", "bool").unwrap();
    // Contains the word without the prefix: the rule is `identity.`, not
    // "mentions identity" (in production this key lives in `daemon.toml`;
    // here it stands for any key the rule must not mistake for the owner).
    repo.set("daemon.orchestrator.identity_budget", "900", "int")
        .unwrap();

    repo.clear_all_except_identity().unwrap();

    assert_eq!(
        repo.get(OWNER_KEY).unwrap().as_deref(),
        Some("owner-1"),
        "the owner identity must survive a configuration reset"
    );
    assert_eq!(repo.get("telegram.enabled").unwrap(), None);
    assert_eq!(
        repo.get("daemon.orchestrator.identity_budget").unwrap(),
        None
    );
    assert_eq!(
        repo.list().unwrap(),
        vec![(
            OWNER_KEY.to_string(),
            "owner-1".to_string(),
            "string".to_string()
        )]
    );
}

/// A store no daemon has booted yet has no owner row; the reset then empties
/// the table and does not fail.
#[test]
fn clear_all_except_identity_with_no_owner_row_empties_the_table() {
    let db = test_db();
    let repo = ConfigRepository::new(&db);
    repo.set("telegram.enabled", "true", "bool").unwrap();

    repo.clear_all_except_identity().unwrap();

    assert!(repo.list().unwrap().is_empty());
}

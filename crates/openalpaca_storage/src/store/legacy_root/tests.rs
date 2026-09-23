//! The classifier is pure and driven from temp dirs. The two cases that go
//! through the environment-resolving wrappers sandbox `HOME` as well as the
//! home store (`HomeStoreGuard::set_with_home`), because `legacy_app_dir()`
//! reads `HOME` — no test here can resolve the real legacy directory.

use super::*;
use crate::store::tests::HomeStoreGuard;
use std::fs;
use std::path::Path;
use tempfile::tempdir;

fn touch(path: &Path) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, "x").unwrap();
}

/// `(legacy, home_root, home_db)` under one temp dir, none of them created.
fn roots(tmp: &Path) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
    let legacy = tmp.join("legacy");
    let home = tmp.join("home");
    let home_db = home.join("state").join("openalpaca.db");
    (legacy, home, home_db)
}

#[test]
fn an_absent_legacy_root_says_nothing() {
    let tmp = tempdir().unwrap();
    let (legacy, home, home_db) = roots(tmp.path());

    assert_eq!(
        classify_legacy_root(&legacy, &home, &home_db),
        LegacyRoot::Absent
    );
}

#[test]
fn a_legacy_root_holding_only_junk_says_nothing() {
    let tmp = tempdir().unwrap();
    let (legacy, home, home_db) = roots(tmp.path());
    // Finder's droppings, a log and a stale lock: none of it is data, and a
    // refusal tuned on this would punish every install that migrated cleanly.
    touch(&legacy.join(".DS_Store"));
    touch(&legacy.join("daemon.log"));
    touch(&legacy.join("openalpacad.lock"));

    assert_eq!(
        classify_legacy_root(&legacy, &home, &home_db),
        LegacyRoot::Absent
    );
}

#[test]
fn a_legacy_database_with_no_database_here_is_fatal() {
    let tmp = tempdir().unwrap();
    let (legacy, home, home_db) = roots(tmp.path());
    touch(&legacy.join("openalpaca.db"));

    assert_eq!(
        classify_legacy_root(&legacy, &home, &home_db),
        LegacyRoot::Stranded
    );
}

#[test]
fn a_legacy_master_key_or_config_alone_is_fatal() {
    // The key and the config it decrypts carry the provider credentials:
    // losing them silently is the same failure as losing the database.
    for entry in [".master_key", "config/llm.toml"] {
        let tmp = tempdir().unwrap();
        let (legacy, home, home_db) = roots(tmp.path());
        touch(&legacy.join(entry));

        assert_eq!(
            classify_legacy_root(&legacy, &home, &home_db),
            LegacyRoot::Stranded,
            "{entry} alone must refuse"
        );
    }
}

#[test]
fn a_legacy_root_beside_an_existing_database_only_warns() {
    let tmp = tempdir().unwrap();
    let (legacy, home, home_db) = roots(tmp.path());
    touch(&legacy.join("openalpaca.db"));
    touch(&home_db);

    // Two databases used to refuse the boot, because the mover had to choose
    // one. Nothing moves or merges them now, so it is one line (T28).
    assert_eq!(
        classify_legacy_root(&legacy, &home, &home_db),
        LegacyRoot::Residue {
            holds_database: true
        }
    );

    let message = residue_message(&legacy, &home, true);
    assert!(message.contains(&legacy.display().to_string()), "{message}");
    assert!(message.contains(&home.display().to_string()), "{message}");
    assert!(
        message.contains("holds an openalpaca.db of its own"),
        "{message}"
    );
    // The warning names its own remedy and no other message's (C6).
    assert!(!message.contains("reset --factory"), "{message}");

    let without = residue_message(&legacy, &home, false);
    assert!(!without.contains("holds an openalpaca.db of its own"));
    assert!(without.contains("this warning stops"), "{without}");
}

#[test]
fn notable_leftovers_beside_a_database_only_warn() {
    let tmp = tempdir().unwrap();
    let (legacy, home, home_db) = roots(tmp.path());
    touch(&legacy.join("assets").join("ab").join("cd").join("blob"));
    touch(&legacy.join("openalpaca.db-wal"));

    // Not critical, so never a refusal — even with no database here yet.
    assert_eq!(
        classify_legacy_root(&legacy, &home, &home_db),
        LegacyRoot::Residue {
            holds_database: false
        }
    );
    touch(&home_db);
    assert_eq!(
        classify_legacy_root(&legacy, &home, &home_db),
        LegacyRoot::Residue {
            holds_database: false
        }
    );
}

#[test]
fn a_legacy_root_that_is_this_installs_root_says_nothing() {
    let tmp = tempdir().unwrap();
    let shared = tmp.path().join("one-root");
    // `OPENALPACA_HOME_STORE` pointed at the legacy path: there is no "older"
    // directory, only this install's own.
    touch(&shared.join("openalpaca.db"));
    let home_db = shared.join("state").join("openalpaca.db");

    assert_eq!(
        classify_legacy_root(&shared, &shared, &home_db),
        LegacyRoot::Absent
    );
}

#[test]
fn the_fatal_message_names_both_paths_and_both_ways_out() {
    let tmp = tempdir().unwrap();
    let (legacy, home, home_db) = roots(tmp.path());
    let message = stranded_message(&legacy, &home, &home_db);

    for needle in [
        legacy.display().to_string(),
        home.display().to_string(),
        home_db.display().to_string(),
        ".master_key".to_string(),
        "Leave assets/ where it is".to_string(),
        "rename or delete".to_string(),
        "does not move it for you".to_string(),
    ] {
        assert!(
            message.contains(&needle),
            "missing {needle:?} in:\n{message}"
        );
    }
    // Running the factory reset while stranded deletes nothing that exists and
    // does not clear this; the message must never suggest it (C6).
    assert!(!message.contains("reset --factory"), "{message}");
    assert!(!message.contains("config reset"), "{message}");
}

#[test]
fn open_home_database_refuses_while_a_legacy_root_is_stranded() {
    let tmp = tempdir().unwrap();
    let root = tmp.path().join("home");
    let _guard = HomeStoreGuard::set_with_home(&root, &tmp.path().join("user"));
    // Inside the sandboxed HOME — `set_with_home` has asserted as much.
    let legacy = legacy_app_dir().unwrap();
    touch(&legacy.join("openalpaca.db"));

    let err = match crate::store::open_home_database() {
        Err(e) => e,
        Ok(_) => panic!("opening must refuse while an older install's data is stranded"),
    };
    let text = format!("{err:#}");
    assert!(text.contains("does not move it for you"), "{text}");
    assert!(text.contains(&legacy.display().to_string()), "{text}");

    // The check ran before anything was opened: no database, no `state/`.
    assert!(!root.join("state").join("openalpaca.db").exists());
    assert!(!root.join("state").exists());
    assert_eq!(
        fs::read_to_string(legacy.join("openalpaca.db")).unwrap(),
        "x"
    );
}

#[test]
fn the_check_lets_a_residue_through() {
    let tmp = tempdir().unwrap();
    let root = tmp.path().join("home");
    let _guard = HomeStoreGuard::set_with_home(&root, &tmp.path().join("user"));
    let legacy = legacy_app_dir().unwrap();
    touch(&legacy.join("openalpaca.db"));
    touch(&root.join("state").join("openalpaca.db"));

    // A warning, not a refusal: this install already has its own database.
    check_legacy_root_result().unwrap();
    assert_eq!(
        fs::read_to_string(legacy.join("openalpaca.db")).unwrap(),
        "x"
    );
}

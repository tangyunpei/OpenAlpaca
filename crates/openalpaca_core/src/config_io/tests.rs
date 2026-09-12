use super::*;

/// `backups_dir()` reads `OPENALPACA_HOME_STORE` on every call, so the writer
/// tests must not run concurrently with each other — or with any other module
/// that re-points it. The guard (and its crate-wide lock) lives in
/// `crate::test_util`. No test ever touches the real `~/.openalpaca`.
use crate::test_util::HomeStoreGuard;

const HAND_AUTHORED: &str = r#"# The MCP servers this daemon connects out to.
# Every comment in this file is the owner's, and must survive a daemon write.

[servers.github]
transport = "stdio"          # trailing comment
command = "npx"
enabled = true

# A server the owner left off on purpose.
[servers.slack]
transport = "stdio"
command = "slack-mcp"
enabled = false
"#;

fn set_enabled(name: &'static str, value: bool) -> impl FnOnce(&mut toml_edit::DocumentMut) -> Result<(), String> {
    move |doc| {
        doc["servers"][name]["enabled"] = toml_edit::value(value);
        Ok(())
    }
}

/// Stands in for `McpConfig::load`'s own parser: every `[servers.<n>]` block
/// must carry a `transport` tag.
fn reparse_servers(rendered: &str) -> Result<(), String> {
    let doc: toml::Value = toml::from_str(rendered).map_err(|e| e.to_string())?;
    let Some(servers) = doc.get("servers").and_then(|s| s.as_table()) else {
        return Ok(());
    };
    for (name, block) in servers {
        if block.get("transport").is_none() {
            return Err(format!("server '{name}' has no transport"));
        }
    }
    Ok(())
}

#[test]
fn comments_and_layout_survive_a_surgical_edit() {
    let tmp = tempfile::tempdir().unwrap();
    let _home = HomeStoreGuard::set(&tmp.path().join("home"));
    let path = tmp.path().join("mcp.toml");
    std::fs::write(&path, HAND_AUTHORED).unwrap();

    atomic_write_toml(&path, set_enabled("github", false), reparse_servers).unwrap();

    let after = std::fs::read_to_string(&path).unwrap();
    assert!(after.contains("# The MCP servers this daemon connects out to."));
    assert!(after.contains("# A server the owner left off on purpose."));
    assert!(after.contains("# trailing comment"));
    assert!(after.contains(r#"command = "npx""#));
    // Byte-identical except the one assignment.
    assert_eq!(
        after,
        HAND_AUTHORED.replacen("enabled = true", "enabled = false", 1),
        "exactly one assignment may change:\n{after}"
    );
}

#[test]
fn a_malformed_edit_is_aborted_with_the_file_untouched() {
    let tmp = tempfile::tempdir().unwrap();
    let _home = HomeStoreGuard::set(&tmp.path().join("home"));
    let path = tmp.path().join("mcp.toml");
    std::fs::write(&path, HAND_AUTHORED).unwrap();

    // The T5-gone shape: assigning `enabled` into a block that is not there
    // makes `toml_edit` synthesize a table with no `transport` tag.
    let err = atomic_write_toml(&path, set_enabled("vanished", false), reparse_servers)
        .expect_err("the re-parse must reject a synthesized block");
    assert!(matches!(err, ConfigWriteError::Reparse { .. }), "{err}");
    assert!(err.to_string().contains("no transport"), "{err}");

    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        HAND_AUTHORED,
        "a failed re-parse leaves the file byte-identical"
    );
    // Nothing was rotated either — there was no successful write.
    let backups = openalpaca_storage::store::backups_dir().unwrap();
    assert_eq!(std::fs::read_dir(&backups).unwrap().count(), 0);

    // An edit closure that refuses is the same shape.
    let err = atomic_write_toml(&path, |_| Err("nope".to_string()), reparse_servers).unwrap_err();
    assert!(matches!(err, ConfigWriteError::Edit(_)), "{err}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), HAND_AUTHORED);
}

#[test]
fn five_backups_are_kept_and_the_sixth_is_rotated_out() {
    let tmp = tempfile::tempdir().unwrap();
    let _home = HomeStoreGuard::set(&tmp.path().join("home"));
    let path = tmp.path().join("mcp.toml");
    std::fs::write(&path, HAND_AUTHORED).unwrap();

    for i in 0..7 {
        atomic_write_toml(&path, set_enabled("github", i % 2 == 0), reparse_servers).unwrap();
    }

    let backups = openalpaca_storage::store::backups_dir().unwrap();
    let mut kept: Vec<String> = std::fs::read_dir(&backups)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("mcp.toml.bak."))
        .collect();
    kept.sort();
    assert_eq!(
        kept.len(),
        BACKUPS_KEPT,
        "seven writes must leave the five newest: {kept:?}"
    );
    // The survivors are the newest five, not the oldest.
    let newest = kept.last().unwrap();
    assert!(
        std::fs::read_to_string(backups.join(newest))
            .unwrap()
            .contains("enabled = false"),
        "the newest backup is the version the last write replaced"
    );
}

// ── The byte-level primitive (plan §1.4, P-11) ──────────────────────────────
//
// `llm.toml`'s writer (`LlmSettingsService::persist_only`) holds the whole
// file's text, because that config is serialised from its typed form rather
// than edited in place. It writes through the same tmp → fsync → rotate →
// rename tail as the two surgical writers, and these are that tail's tests.

#[test]
fn the_byte_writer_replaces_the_file_and_keeps_what_it_replaced() {
    let tmp = tempfile::tempdir().unwrap();
    let _home = HomeStoreGuard::set(&tmp.path().join("home"));
    let path = tmp.path().join("llm.toml");
    std::fs::write(&path, "first = 1\n").unwrap();

    atomic_write_with_backup(&path, "second = 2\n").unwrap();

    assert_eq!(std::fs::read_to_string(&path).unwrap(), "second = 2\n");
    let backups = openalpaca_storage::store::backups_dir().unwrap();
    let kept: Vec<PathBuf> = std::fs::read_dir(&backups)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| file_name(p).starts_with("llm.toml.bak."))
        .collect();
    assert_eq!(kept.len(), 1, "the replaced version is recoverable: {kept:?}");
    assert_eq!(std::fs::read_to_string(&kept[0]).unwrap(), "first = 1\n");
}

#[test]
fn the_byte_writer_keeps_five_versions_and_creates_a_missing_parent() {
    let tmp = tempfile::tempdir().unwrap();
    let _home = HomeStoreGuard::set(&tmp.path().join("home"));
    let path = tmp.path().join("nested").join("llm.toml");

    // A first write into a directory that does not exist yet rotates nothing.
    atomic_write_with_backup(&path, "n = 0\n").unwrap();
    let backups = openalpaca_storage::store::backups_dir().unwrap();
    assert_eq!(std::fs::read_dir(&backups).unwrap().count(), 0);

    for n in 1..=7 {
        atomic_write_with_backup(&path, &format!("n = {n}\n")).unwrap();
    }

    assert_eq!(std::fs::read_to_string(&path).unwrap(), "n = 7\n");
    let mut kept: Vec<String> = std::fs::read_dir(&backups)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("llm.toml.bak."))
        .collect();
    kept.sort();
    assert_eq!(kept.len(), BACKUPS_KEPT, "seven writes keep five: {kept:?}");
    assert_eq!(
        std::fs::read_to_string(backups.join(kept.last().unwrap())).unwrap(),
        "n = 6\n",
        "the newest backup is the version the last write replaced"
    );
}

#[test]
fn an_unparseable_file_is_copied_once() {
    let tmp = tempfile::tempdir().unwrap();
    let _home = HomeStoreGuard::set(&tmp.path().join("home"));
    let path = tmp.path().join("mcp.toml");
    std::fs::write(&path, "[servers.github\ntransport = \"stdio\"\n").unwrap();

    let first = copy_unparseable_once(&path).expect("a copy is kept for repair");
    let second = copy_unparseable_once(&path).expect("the same copy comes back");
    assert_eq!(first, second, "a boot loop must not fill the directory");

    let backups = openalpaca_storage::store::backups_dir().unwrap();
    let copies = std::fs::read_dir(&backups)
        .unwrap()
        .flatten()
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("mcp.toml.unparseable-")
        })
        .count();
    assert_eq!(copies, 1);
    assert_eq!(
        std::fs::read_to_string(&first).unwrap(),
        std::fs::read_to_string(&path).unwrap()
    );

    // A *different* broken version is worth its own copy.
    std::fs::write(&path, "[servers.slack\n").unwrap();
    let third = copy_unparseable_once(&path).unwrap();
    assert_ne!(third, first);
}

#[test]
fn a_store_that_does_not_exist_yet_is_created() {
    let tmp = tempfile::tempdir().unwrap();
    let _home = HomeStoreGuard::set(&tmp.path().join("home"));
    let path = tmp.path().join("nested").join(".permissions.toml");

    atomic_write_toml(
        &path,
        |doc| {
            doc["notion"]["enabled"] = toml_edit::value(false);
            Ok(())
        },
        |_| Ok(()),
    )
    .unwrap();

    assert!(std::fs::read_to_string(&path).unwrap().contains("enabled = false"));
    // Nothing to rotate on a first write.
    let backups = openalpaca_storage::store::backups_dir().unwrap();
    assert_eq!(std::fs::read_dir(&backups).unwrap().count(), 0);
}

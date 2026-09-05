use super::*;
use std::ffi::OsString;
use std::sync::{Mutex, MutexGuard};

/// `backups_dir()` reads `OPENALPACA_HOME_STORE` on every call, so the writer
/// tests must not run concurrently with each other. No test ever touches the
/// real `~/.openalpaca`.
static ENV_LOCK: Mutex<()> = Mutex::new(());

struct HomeStoreGuard {
    _lock: MutexGuard<'static, ()>,
    prev: Option<OsString>,
}

impl HomeStoreGuard {
    fn set(path: &Path) -> Self {
        let lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var_os(openalpaca_storage::store::HOME_STORE_ENV);
        // SAFETY: serialized by ENV_LOCK; these are the only tests in this
        // binary that touch the variable.
        unsafe { std::env::set_var(openalpaca_storage::store::HOME_STORE_ENV, path) };
        Self { _lock: lock, prev }
    }
}

impl Drop for HomeStoreGuard {
    fn drop(&mut self) {
        // SAFETY: as above — still holding ENV_LOCK.
        match self.prev.take() {
            Some(v) => unsafe {
                std::env::set_var(openalpaca_storage::store::HOME_STORE_ENV, v)
            },
            None => unsafe {
                std::env::remove_var(openalpaca_storage::store::HOME_STORE_ENV)
            },
        }
    }
}

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

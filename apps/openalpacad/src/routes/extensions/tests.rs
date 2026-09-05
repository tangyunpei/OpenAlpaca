//! The C6 verification cell for `/v1/extensions`: **every status code design
//! §8 fixes**, the row's shape, and R18.
//!
//! These are route tests, not supervisor tests. The transitions themselves are
//! C2's and C3's — with real child processes — and are not re-proved here. What
//! is proved here is the mapping the routes own: which refusal becomes which
//! status code and which error word, what the row serialises, and that a
//! dropped request cannot abandon a transition.

#![cfg(unix)]

use super::*;

use std::path::Path;
use std::time::Duration;

use axum::body::to_bytes;
use openalpaca_core::agent::AgentRegistry;
use openalpaca_core::bus::EventBus;
use openalpaca_core::daemon_config::DaemonConfig;
use openalpaca_core::orchestrator::skill_catalog::SkillCatalog;
use openalpaca_core::tools::ToolRegistry;
use openalpaca_core::tools::extensions::{ExtensionSupervisor, ExtensionState};
use openalpaca_plugins::PluginManager;
use tempfile::TempDir;

use crate::managers::mcp::McpSupervisor;
use crate::managers::mcp::tests::HomeStoreGuard;

// ============================================================================
// Harness
// ============================================================================

/// A minimal stdio MCP server: `initialize`, `tools/list`, `ping`. Enough to
/// reach `Enabled`; everything richer is C2's stub, in `managers/mcp/tests.rs`.
///
/// `slow_start_secs` holds the handshake open, which is how the R18 test gets a
/// verb that is still running when its request is dropped.
fn stub_server(dir: &Path, tag: &str, slow_start_secs: u64) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let script = dir.join(format!("{tag}-server.sh"));
    let slow = if slow_start_secs > 0 {
        format!("sleep {slow_start_secs}\n")
    } else {
        String::new()
    };
    let body = format!(
        r#"#!/bin/sh
{slow}while IFS= read -r line; do
  id=`printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p'`
  case "$line" in
    *'"method":"initialize"'*)
      pv=`printf '%s' "$line" | sed -n 's/.*"protocolVersion":"\([^"]*\)".*/\1/p'`
      printf '{{"jsonrpc":"2.0","id":%s,"result":{{"protocolVersion":"%s","capabilities":{{"tools":{{}}}},"serverInfo":{{"name":"stub","version":"9.9.9"}}}}}}\n' "$id" "$pv"
      ;;
    *'"method":"tools/list"'*)
      printf '{{"jsonrpc":"2.0","id":%s,"result":{{"tools":[{{"name":"echo","description":"stub","inputSchema":{{"type":"object","properties":{{}}}}}}]}}}}\n' "$id"
      ;;
    *'"method":"ping"'*)
      printf '{{"jsonrpc":"2.0","id":%s,"result":{{}}}}\n' "$id"
      ;;
  esac
done
"#
    );
    std::fs::write(&script, body).expect("write stub server");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
        .expect("chmod stub server");
    script
}

struct Harness {
    _home: TempDir,
    _env: HomeStoreGuard,
    config_dir: TempDir,
    plugins_root: TempDir,
    registry: Arc<ToolRegistry>,
    mcp: Arc<McpSupervisor>,
    plugins: Arc<PluginManager>,
    extensions: Arc<Extensions>,
}

impl Harness {
    fn new() -> Self {
        let home = tempfile::tempdir().expect("home store");
        let env = HomeStoreGuard::set(home.path());
        let config_dir = tempfile::tempdir().expect("config dir");
        let plugins_root = tempfile::tempdir().expect("plugins root");

        let bus = EventBus::new(256);
        let registry = Arc::new(ToolRegistry::with_event_bus(bus.clone()).expect("tool registry"));
        let mcp = McpSupervisor::new(
            config_dir.path().join("mcp.toml"),
            Arc::clone(&registry),
            Arc::new(arc_swap::ArcSwap::from_pointee(DaemonConfig::default())),
            bus.clone(),
            Some(Arc::new(SkillCatalog::new())),
            Some(Arc::new(AgentRegistry::new())),
            "owner:gui",
        );
        let plugins = Arc::new(PluginManager::new(
            plugins_root.path().to_path_buf(),
            Arc::clone(&registry),
            None,
            None,
        ));
        let extensions = Extensions::new(
            Arc::clone(&mcp),
            Arc::clone(&plugins),
            Arc::new(arc_swap::ArcSwap::from_pointee(DaemonConfig::default())),
        );

        Self {
            _home: home,
            _env: env,
            config_dir,
            plugins_root,
            registry,
            mcp,
            plugins,
            extensions,
        }
    }

    fn mcp_config_path(&self) -> std::path::PathBuf {
        self.config_dir.path().join("mcp.toml")
    }

    fn write_mcp(&self, body: &str) {
        std::fs::write(self.mcp_config_path(), body).expect("write mcp.toml");
    }

    /// A server whose command does not exist: bring-up fails and the row lands
    /// at `Failed` with the bit already `true` (design §3.4).
    fn declare_unreachable(&self, name: &str, enabled: bool) {
        self.write_mcp(&format!(
            "[servers.{name}]\ntransport = \"stdio\"\ncommand = \"/nonexistent/openalpaca-test-server\"\nenabled = {enabled}\nconnect_timeout_secs = 2\n"
        ));
    }

    fn declare_stub(&self, name: &str, slow_start_secs: u64) {
        let script = stub_server(self.config_dir.path(), name, slow_start_secs);
        self.write_mcp(&format!(
            "[servers.{name}]\ntransport = \"stdio\"\ncommand = \"{}\"\nenabled = true\nconnect_timeout_secs = 20\n",
            script.display()
        ));
    }

    /// A plugin directory with a manifest and nothing else — enough to be
    /// scanned, gated on consent, and never spawned.
    fn write_plugin(&self, name: &str) {
        self.write_plugin_with(name, "");
    }

    /// The same, plus extra manifest sections — `[config.<key>]` blocks for the
    /// config route's tests.
    fn write_plugin_with(&self, name: &str, extra: &str) {
        let dir = self.plugins_root.path().join(name);
        std::fs::create_dir_all(&dir).expect("plugin dir");
        std::fs::write(
            dir.join("plugin.toml"),
            format!(
                "[plugin]\nname = \"{name}\"\nversion = \"1.4.0\"\nentry = \"./nope\"\n{extra}"
            ),
        )
        .expect("plugin manifest");
    }

    fn plugin_config_dir(&self) -> std::path::PathBuf {
        self.plugins_root.path().join(".config")
    }

    fn plugin_config_path(&self, name: &str) -> std::path::PathBuf {
        self.plugin_config_dir().join(format!("{name}.toml"))
    }

    async fn get_config(&self, kind: &str, id: &str) -> (StatusCode, serde_json::Value) {
        split(super::get_config(self.extensions.clone(), kind, id).await).await
    }

    async fn set_config(
        &self,
        kind: &str,
        id: &str,
        key: &str,
        value: serde_json::Value,
    ) -> (StatusCode, serde_json::Value) {
        let request = SetConfigRequest {
            key: key.to_string(),
            value,
        };
        split(super::set_config(self.extensions.clone(), kind, id, request).await).await
    }

    fn permissions_path(&self) -> std::path::PathBuf {
        self.plugins_root.path().join(".permissions.toml")
    }

    async fn verb(&self, kind: &str, id: &str, verb: Verb) -> (StatusCode, serde_json::Value) {
        split(run_verb(self.extensions.clone(), kind, id, verb).await).await
    }

    async fn rows(&self, include_orphaned: bool) -> Vec<serde_json::Value> {
        self.extensions
            .list(include_orphaned)
            .await
            .iter()
            .map(row_json)
            .collect()
    }

    fn state(&self, kind: &str, id: &str) -> Option<ExtensionState> {
        let ext = match kind {
            "mcp" => ExtensionId::mcp(id),
            _ => ExtensionId::plugin(id),
        };
        self.registry.extensions().state(&ext)
    }
}

/// Split a `Response` into its status and its JSON body.
async fn split(response: Response) -> (StatusCode, serde_json::Value) {
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1 << 20)
        .await
        .expect("read the response body");
    let body = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, body)
}

fn error_word(body: &serde_json::Value) -> &str {
    body["error"].as_str().unwrap_or("<not a string>")
}

// ============================================================================
// 404 — unknown id, unknown kind
// ============================================================================

#[tokio::test]
async fn an_unknown_id_is_a_404() {
    let h = Harness::new();
    h.write_mcp("");
    h.mcp.reconcile_all().await;

    for verb in [Verb::Enable, Verb::Disable, Verb::Reload] {
        let (status, _) = h.verb("mcp", "no-such-server", verb).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{verb:?} on an unknown server");
    }
    for verb in [Verb::Enable, Verb::Approve, Verb::Deny] {
        let (status, _) = h.verb("plugin", "no-such-plugin", verb).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{verb:?} on an unknown plugin");
    }
}

/// `/v1/extensions/banana/x/enable` names no resource, so it is a `404` and not
/// a `400`: `{kind}` is part of the path, not of a request body.
#[tokio::test]
async fn an_unknown_kind_is_a_404() {
    let h = Harness::new();
    let (status, _) = h.verb("banana", "github", Verb::Enable).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ============================================================================
// 409 unsupported_for_kind — consent is plugins-only
// ============================================================================

/// Writing a server into your own `config/mcp.toml` **is** the consent, and
/// there is no untrusted binary to gate (design §8).
#[tokio::test]
async fn approve_and_deny_on_an_mcp_server_are_409_unsupported_for_kind() {
    let h = Harness::new();
    h.declare_stub("srv", 0);
    h.mcp.reconcile_all().await;

    for verb in [Verb::Approve, Verb::Deny] {
        let (status, body) = h.verb("mcp", "srv", verb).await;
        assert_eq!(status, StatusCode::CONFLICT, "{verb:?}");
        assert_eq!(error_word(&body), "unsupported_for_kind");
    }
    assert!(
        matches!(h.state("mcp", "srv"), Some(ExtensionState::Enabled)),
        "a refused verb must take no transition"
    );
}

// ============================================================================
// 200 even when bring-up fails
// ============================================================================

/// The write at W succeeded and the intent is durable; the connection outcome
/// is a separate fact in the body (design §8, §3.4).
#[tokio::test]
async fn a_bring_up_that_fails_is_still_a_200() {
    let h = Harness::new();
    h.declare_unreachable("srv", false);
    h.mcp.reconcile_all().await;

    let (status, body) = h.verb("mcp", "srv", Verb::Enable).await;
    assert_eq!(status, StatusCode::OK, "a failed bring-up is not an error");
    assert_eq!(body["state"], "failed");
    assert_eq!(body["enabled"], true, "the bit is on disk before E0");
    assert_eq!(body["reason"], "unreachable");
    assert_eq!(body["actionable"], false);
    assert!(
        body["detail"].as_str().is_some_and(|d| d.contains("connect")),
        "the row must carry the failure detail: {body}"
    );
}

// ============================================================================
// reload — 409 not_loaded, 200 from Enabled and Failed
// ============================================================================

#[tokio::test]
async fn reload_from_disabled_is_409_not_loaded() {
    let h = Harness::new();
    h.declare_unreachable("srv", false);
    h.mcp.reconcile_all().await;
    assert!(matches!(h.state("mcp", "srv"), Some(ExtensionState::Disabled)));

    let (status, body) = h.verb("mcp", "srv", Verb::Reload).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(error_word(&body), "not_loaded");
    assert!(
        matches!(h.state("mcp", "srv"), Some(ExtensionState::Disabled)),
        "a refused reload must take no transition"
    );
}

#[tokio::test]
async fn reload_from_unapproved_is_409_not_loaded() {
    let h = Harness::new();
    h.write_plugin("waiting");
    h.plugins.start().await.expect("plugin scan");
    assert!(matches!(
        h.state("plugin", "waiting"),
        Some(ExtensionState::Unapproved { .. })
    ));

    let (status, body) = h.verb("plugin", "waiting", Verb::Reload).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(error_word(&body), "not_loaded");
}

/// From `Enabled` reload is T0–T4 then E0–E5; from `Failed{*}` it is E0's CAS
/// then E-PRE then E1–E5. Both are `200` — including when the bring-up half
/// fails, exactly as `enable` from `Failed` is.
#[tokio::test]
async fn reload_from_enabled_and_from_failed_are_200() {
    let h = Harness::new();
    h.declare_stub("srv", 0);
    h.mcp.reconcile_all().await;
    assert!(
        matches!(h.state("mcp", "srv"), Some(ExtensionState::Enabled)),
        "the stub should have come up: {:?}",
        h.state("mcp", "srv")
    );

    let (status, body) = h.verb("mcp", "srv", Verb::Reload).await;
    assert_eq!(status, StatusCode::OK, "reload from Enabled");
    assert_eq!(body["state"], "enabled");
    assert_eq!(body["version"], "9.9.9", "the handshake's serverInfo.version");
    assert_eq!(body["transport"], "stdio");

    // Now break the declaration under it and reload again: the row goes to
    // `Failed`, and reloading a `Failed` row is still `200`.
    h.declare_unreachable("srv", true);
    let (status, body) = h.verb("mcp", "srv", Verb::Reload).await;
    assert_eq!(status, StatusCode::OK, "reload from Enabled onto a bad edit");
    assert_eq!(body["state"], "failed");

    let (status, body) = h.verb("mcp", "srv", Verb::Reload).await;
    assert_eq!(status, StatusCode::OK, "reload from Failed");
    assert_eq!(body["state"], "failed");
    assert_eq!(body["enabled"], true);
    assert_eq!(
        body["version"],
        serde_json::Value::Null,
        "a row that is not running reports no version"
    );

    h.mcp.shutdown_all().await;
}

// ============================================================================
// disable on an unapproved plugin
// ============================================================================

/// It clears the bit — writing a **decision-less** entry if the plugin had none
/// — and **stays `Unapproved`**: consent pre-empts the switch (design §4, §8).
#[tokio::test]
async fn disable_on_an_unapproved_plugin_returns_unapproved_and_enabled_false() {
    let h = Harness::new();
    h.write_plugin("waiting");
    h.plugins.start().await.expect("plugin scan");

    let (status, body) = h.verb("plugin", "waiting", Verb::Disable).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["state"], "unapproved");
    assert_eq!(body["enabled"], false);
    assert_eq!(body["consent"], "pending");
    assert_eq!(body["reason"], "never_seen");

    // The decision-less entry is what makes the pre-set bit survive a restart
    // while the row still reads `never_seen`.
    let stored = std::fs::read_to_string(h.permissions_path()).expect("permissions file");
    assert!(stored.contains("enabled = false"), "{stored}");
    assert!(
        !stored.contains("approved"),
        "disable must not record a consent decision: {stored}"
    );
}

// ============================================================================
// 409 store_unreadable
// ============================================================================

/// §4: a row whose disposition nobody can read reports `enabled: null`, and
/// every verb on it is `409 store_unreadable` **without entering a
/// transition** — the W write is refused up front.
#[tokio::test]
async fn an_unreadable_permissions_store_is_409_store_unreadable() {
    let h = Harness::new();
    h.write_plugin("parked");
    std::fs::write(h.permissions_path(), "this is not toml = = =").expect("corrupt the store");
    h.plugins.start().await.expect("plugin scan");

    for verb in [Verb::Enable, Verb::Disable, Verb::Approve, Verb::Deny] {
        let (status, body) = h.verb("plugin", "parked", verb).await;
        assert_eq!(status, StatusCode::CONFLICT, "{verb:?}");
        assert_eq!(error_word(&body), "store_unreadable", "{verb:?}");
    }

    let rows = h.rows(false).await;
    let row = rows
        .iter()
        .find(|r| r["id"] == "parked")
        .expect("the parked plugin should still be listed");
    assert_eq!(
        row["enabled"],
        serde_json::Value::Null,
        "a bit nobody can read is null, never false: {row}"
    );
    assert_eq!(row["state"], "failed");
    assert_eq!(row["reason"], "config_invalid");
}

/// The MCP half: an unparseable `config/mcp.toml` yields the pseudo-record,
/// whose row is `enabled: null` and whose every verb is `409`.
#[tokio::test]
async fn the_mcp_pseudo_record_reports_a_null_bit_and_refuses_every_verb() {
    let h = Harness::new();
    h.write_mcp("[servers.srv]\ncommand = = broken\n");
    h.mcp.reconcile_all().await;

    let rows = h.rows(false).await;
    let row = rows
        .iter()
        .find(|r| r["id"] == "config/mcp.toml")
        .unwrap_or_else(|| panic!("the pseudo-record should be listed: {rows:?}"));
    assert_eq!(row["enabled"], serde_json::Value::Null);
    assert_eq!(row["state"], "failed");
    assert_eq!(row["reason"], "config_invalid");

    for verb in [Verb::Enable, Verb::Disable, Verb::Reload] {
        let (status, body) = h.verb("mcp", "config/mcp.toml", verb).await;
        assert_eq!(status, StatusCode::CONFLICT, "{verb:?}");
        assert_eq!(error_word(&body), "store_unreadable", "{verb:?}");
    }
}

// ============================================================================
// 500 — the write-first rule
// ============================================================================

/// **Step W failed, so no CAS was taken and nothing changed.** Reporting it as
/// a `4xx` would tell the client to fix a request that was never the problem
/// (design §8, §3.2 W).
#[tokio::test]
async fn a_write_that_fails_is_a_500_and_takes_no_transition() {
    use std::os::unix::fs::PermissionsExt;

    let h = Harness::new();
    h.declare_unreachable("srv", true);
    h.mcp.reconcile_all().await;
    let before = h.state("mcp", "srv");
    assert!(matches!(before, Some(ExtensionState::Failed { .. })));

    // Readable, but not writable: this is a **write** failure, not the
    // `409 store_unreadable` of a store nobody can parse.
    std::fs::set_permissions(h.config_dir.path(), std::fs::Permissions::from_mode(0o555))
        .expect("chmod config dir");
    let (status, body) = h.verb("mcp", "srv", Verb::Disable).await;
    std::fs::set_permissions(h.config_dir.path(), std::fs::Permissions::from_mode(0o755))
        .expect("restore config dir");

    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(
        error_word(&body).contains("write failed"),
        "the body should name the failed write: {body}"
    );
    assert_eq!(
        h.state("mcp", "srv"),
        before,
        "a failed W must leave the record exactly as it was"
    );
    assert!(
        std::fs::read_to_string(h.mcp_config_path())
            .expect("mcp.toml")
            .contains("enabled = true"),
        "the bit on disk is unchanged"
    );
}

// ============================================================================
// DELETE — orphaned rows only
// ============================================================================

#[tokio::test]
async fn delete_on_a_row_that_is_not_orphaned_is_409_not_orphaned() {
    let h = Harness::new();
    h.write_plugin("present");
    h.plugins.start().await.expect("plugin scan");

    let extensions = h.extensions.clone();
    let error = extensions
        .remove(&ExtensionId::plugin("present"))
        .await
        .expect_err("a present plugin is not removable");
    assert!(matches!(error, ExtensionError::NotOrphaned));
    let (status, body) = split(extension_error(&error)).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(error_word(&body), "not_orphaned");
    assert!(
        body["message"].as_str().is_some_and(|m| m.contains("orphaned")),
        "the refusal should explain how a row becomes orphaned: {body}"
    );
}

#[tokio::test]
async fn delete_on_an_unknown_plugin_is_a_404() {
    let h = Harness::new();
    let error = h
        .extensions
        .remove(&ExtensionId::plugin("never-existed"))
        .await
        .expect_err("an unknown plugin has no row to remove");
    assert_eq!(
        extension_error_status(&error),
        StatusCode::NOT_FOUND,
        "{error:?}"
    );
}

/// The one path that ever deletes a permissions entry (design §5.1).
#[tokio::test]
async fn delete_on_an_orphan_removes_its_entry_and_its_record() {
    let h = Harness::new();
    h.write_plugin("gone");
    h.plugins.start().await.expect("plugin scan");
    h.plugins
        .approve_plugin("gone")
        .await
        .expect("record consent so the entry exists");

    std::fs::remove_dir_all(h.plugins_root.path().join("gone")).expect("remove the directory");
    h.plugins
        .reconcile(&ExtensionId::plugin("gone"))
        .await
        .expect("reconcile should park the vanished plugin");
    assert!(
        matches!(h.state("plugin", "gone"), Some(ExtensionState::Orphaned)),
        "a vanished directory parks as Orphaned: {:?}",
        h.state("plugin", "gone")
    );

    h.extensions
        .remove(&ExtensionId::plugin("gone"))
        .await
        .expect("an orphan is removable");
    assert_eq!(h.state("plugin", "gone"), None, "the record is dropped");
    let stored = std::fs::read_to_string(h.permissions_path()).unwrap_or_default();
    assert!(
        !stored.contains("gone"),
        "the permissions entry should be gone: {stored}"
    );
}

/// C3's review: `reconcile(id)` orphaned only map-tracked plugins, so after a
/// restart — when the map is empty — an entry whose directory is gone produced
/// no record at all and `DELETE` had nothing to target.
#[tokio::test]
async fn a_vanished_entry_orphans_after_a_restart_so_delete_has_a_target() {
    let h = Harness::new();
    h.write_plugin("gone");
    h.plugins.start().await.expect("plugin scan");
    h.plugins.approve_plugin("gone").await.expect("consent");
    std::fs::remove_dir_all(h.plugins_root.path().join("gone")).expect("remove the directory");

    // A *restart*: a fresh manager over the same store, with an empty map.
    let registry = Arc::new(ToolRegistry::new().expect("registry"));
    let restarted = Arc::new(PluginManager::new(
        h.plugins_root.path().to_path_buf(),
        Arc::clone(&registry),
        None,
        None,
    ));
    let row = restarted
        .reconcile(&ExtensionId::plugin("gone"))
        .await
        .expect("the entry alone should be enough to produce a row");
    assert_eq!(row.state, ExtensionState::Orphaned);
}

/// Every verb but `DELETE` is a `409` on an `Orphaned` row (design §4.1).
#[tokio::test]
async fn every_verb_on_an_orphaned_row_is_409_orphaned() {
    let h = Harness::new();
    h.write_plugin("gone");
    h.plugins.start().await.expect("plugin scan");
    h.plugins.approve_plugin("gone").await.expect("consent");
    std::fs::remove_dir_all(h.plugins_root.path().join("gone")).expect("remove the directory");
    h.plugins
        .reconcile(&ExtensionId::plugin("gone"))
        .await
        .expect("park it");

    for verb in [
        Verb::Enable,
        Verb::Disable,
        Verb::Reload,
        Verb::Approve,
        Verb::Deny,
    ] {
        let (status, body) = h.verb("plugin", "gone", verb).await;
        assert_eq!(status, StatusCode::CONFLICT, "{verb:?}");
        assert_eq!(error_word(&body), "orphaned", "{verb:?}");
    }
}

// ============================================================================
// The listing
// ============================================================================

/// `?include_orphaned=true`, default `false` (design §8).
#[tokio::test]
async fn orphaned_rows_are_hidden_unless_asked_for() {
    let h = Harness::new();
    h.write_plugin("gone");
    h.plugins.start().await.expect("plugin scan");
    h.plugins.approve_plugin("gone").await.expect("consent");
    std::fs::remove_dir_all(h.plugins_root.path().join("gone")).expect("remove the directory");
    h.plugins
        .reconcile(&ExtensionId::plugin("gone"))
        .await
        .expect("park it");

    assert!(
        h.rows(false).await.iter().all(|r| r["id"] != "gone"),
        "an orphan is hidden by default"
    );
    assert!(
        h.rows(true).await.iter().any(|r| r["id"] == "gone"),
        "?include_orphaned=true shows it"
    );
}

/// The row carries every field §8 names, with the two kinds' own halves
/// present as `null` rather than absent.
#[tokio::test]
async fn the_row_carries_every_field_of_section_8() {
    let h = Harness::new();
    h.declare_stub("srv", 0);
    h.mcp.reconcile_all().await;
    h.write_plugin("waiting");
    h.plugins.start().await.expect("plugin scan");

    let rows = h.rows(false).await;
    let mcp_row = rows.iter().find(|r| r["kind"] == "mcp").expect("an mcp row");
    let plugin_row = rows
        .iter()
        .find(|r| r["kind"] == "plugin")
        .expect("a plugin row");

    for field in [
        "kind",
        "id",
        "version",
        "transport",
        "enabled",
        "consent",
        "state",
        "reason",
        "actionable",
        "detail",
        "hint",
        "missing_config_keys",
        "added_capabilities",
        "tools",
        "skipped_tools",
        "withdrawn_by_server",
        "tools_changed_at",
        "declared",
        "skills",
        "agents",
        "connector",
        "provider",
        "since",
    ] {
        assert!(
            mcp_row.get(field).is_some(),
            "the mcp row is missing '{field}': {mcp_row}"
        );
        assert!(
            plugin_row.get(field).is_some(),
            "the plugin row is missing '{field}': {plugin_row}"
        );
    }

    // The kind-specific halves.
    assert_eq!(mcp_row["transport"], "stdio");
    assert_eq!(mcp_row["consent"], serde_json::Value::Null, "mcp has no consent");
    assert_eq!(
        mcp_row["declared"],
        serde_json::Value::Null,
        "`declared` is the plugin manifest's, not a server's"
    );
    assert_eq!(mcp_row["tools"], serde_json::json!(["srv__echo"]));
    assert_eq!(plugin_row["transport"], serde_json::Value::Null);
    assert_eq!(plugin_row["consent"], "pending");
    assert_eq!(plugin_row["version"], "1.4.0");
    assert!(plugin_row["declared"].is_object());

    // `warnings` is per-call, never row state.
    assert!(
        mcp_row.get("warnings").is_none(),
        "a listed row carries no warnings: {mcp_row}"
    );

    h.mcp.shutdown_all().await;
}

/// The list is one bare array over both kinds, sorted, so two reads of an
/// unchanged system are byte-identical.
#[tokio::test]
async fn the_listing_is_one_sorted_array_over_both_kinds() {
    let h = Harness::new();
    h.declare_unreachable("zeta", false);
    h.mcp.reconcile_all().await;
    h.write_plugin("alpha");
    h.plugins.start().await.expect("plugin scan");

    let rows = h.rows(false).await;
    let ids: Vec<(String, String)> = rows
        .iter()
        .map(|r| {
            (
                r["kind"].as_str().unwrap().to_string(),
                r["id"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    let mut sorted = ids.clone();
    sorted.sort();
    assert_eq!(ids, sorted, "the array must be sorted: {ids:?}");
    assert_eq!(rows, h.rows(false).await, "two reads must agree");
}

// ============================================================================
// GET / POST …/config — the pair design §8 adds in this commit
// ============================================================================

/// An MCP server has no daemon-managed config file — its declaration *is* its
/// block in `config/mcp.toml` — so both verbs are `409 unsupported_for_kind`,
/// and a `{kind}` word that is neither is the family's `404`.
#[tokio::test]
async fn config_on_an_mcp_server_is_409_unsupported_for_kind_on_both_verbs() {
    let h = Harness::new();
    h.declare_stub("srv", 0);
    h.mcp.reconcile_all().await;

    let (status, body) = h.get_config("mcp", "srv").await;
    assert_eq!(status, StatusCode::CONFLICT, "GET: {body}");
    assert_eq!(error_word(&body), "unsupported_for_kind");

    let (status, body) = h.set_config("mcp", "srv", "key", serde_json::json!("v")).await;
    assert_eq!(status, StatusCode::CONFLICT, "POST: {body}");
    assert_eq!(error_word(&body), "unsupported_for_kind");

    let (status, _) = h.get_config("banana", "srv").await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "an unknown kind names no resource, exactly as it does on a verb"
    );
}

/// A `GET` that answered `200 {}` could not be told from a plugin with no
/// configuration, and the `POST` half wrote `.config/<typo>.toml` for a plugin
/// that does not exist.
#[tokio::test]
async fn config_on_an_unknown_plugin_is_a_404_on_both_verbs() {
    let h = Harness::new();
    h.write_plugin("present");
    h.plugins.start().await.expect("plugin scan");

    let (status, _) = h.get_config("plugin", "never-existed").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "GET on an unknown plugin");

    let (status, _) = h
        .set_config("plugin", "never-existed", "endpoint", serde_json::json!("x"))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "POST on an unknown plugin");
    assert!(
        !h.plugin_config_path("never-existed").exists(),
        "the refused write created a config file for a plugin that does not exist"
    );
}

/// The `400` half of the pair, asserted rather than inferred:
/// `set_plugin_config` refuses a key the manifest declares `sensitive`
/// (`PluginError::PermissionDenied`), and `plugin_error_status`'s catch-all is
/// what turns that into a caller mistake.
#[tokio::test]
async fn a_sensitive_key_is_a_400_and_writes_nothing() {
    let h = Harness::new();
    h.write_plugin_with(
        "vault",
        "\n[config.api_key]\ntype = \"secret\"\nsensitive = true\n",
    );
    h.plugins.start().await.expect("plugin scan");

    let (status, body) = h
        .set_config("plugin", "vault", "api_key", serde_json::json!("sk-live"))
        .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a sensitive key is the caller's mistake, not the daemon's: {body}"
    );
    assert!(
        error_word(&body).contains("sensitive"),
        "the refusal should name why: {body}"
    );
    assert!(
        !h.plugin_config_path("vault").exists(),
        "the refused write created the file anyway"
    );
}

/// **Design §8: the `GET` "redacts sensitive keys".** Both halves of the
/// predicate: a stored secret *reference*, and a plaintext value under a key
/// the manifest *declares* sensitive — which is what a hand-edited
/// `.config/<name>.toml` from the pre-C6 CLI holds.
#[tokio::test]
async fn the_config_get_redacts_a_sensitive_key_however_it_is_stored() {
    let h = Harness::new();
    h.write_plugin_with(
        "vault",
        "\n[config.api_key]\ntype = \"secret\"\nsensitive = true\n\n\
         [config.endpoint]\ntype = \"string\"\n",
    );
    h.plugins.start().await.expect("plugin scan");

    std::fs::create_dir_all(h.plugin_config_dir()).expect("config dir");
    std::fs::write(
        h.plugin_config_path("vault"),
        "api_key = \"sk-hand-typed\"\n\
         token = { secret_ref = \"openalpaca-plugin-vault-token\" }\n\
         endpoint = \"https://example.test\"\n",
    )
    .expect("hand-written plugin config");

    let (status, body) = h.get_config("plugin", "vault").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["api_key"], "<redacted>",
        "a plaintext value under a declared-sensitive key: {body}"
    );
    assert_eq!(
        body["token"], "<redacted>",
        "a stored secret reference: {body}"
    );
    assert_eq!(
        body["endpoint"], "https://example.test",
        "a key that is neither is served as stored: {body}"
    );
}

/// The `200` half, end to end: the write lands and the redacting `GET` reads it
/// back.
#[tokio::test]
async fn a_config_write_is_a_200_and_the_get_reads_it_back() {
    let h = Harness::new();
    h.write_plugin("present");
    h.plugins.start().await.expect("plugin scan");

    let (status, body) = h
        .set_config(
            "plugin",
            "present",
            "endpoint",
            serde_json::json!("https://example.test"),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "ok");
    assert_eq!(body["name"], "present");
    assert_eq!(body["key"], "endpoint");

    let (status, body) = h.get_config("plugin", "present").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["endpoint"], "https://example.test");
}

/// A write the daemon could not perform is a `500`: nothing about the request
/// was wrong (design §8's write-first rule, the same split the verbs use).
#[tokio::test]
async fn a_config_write_that_fails_is_a_500() {
    use std::os::unix::fs::PermissionsExt;

    let h = Harness::new();
    h.write_plugin("present");
    h.plugins.start().await.expect("plugin scan");

    std::fs::create_dir_all(h.plugin_config_dir()).expect("config dir");
    std::fs::set_permissions(
        h.plugin_config_dir(),
        std::fs::Permissions::from_mode(0o555),
    )
    .expect("chmod config dir");
    let (status, body) = h
        .set_config("plugin", "present", "endpoint", serde_json::json!("x"))
        .await;
    std::fs::set_permissions(
        h.plugin_config_dir(),
        std::fs::Permissions::from_mode(0o755),
    )
    .expect("restore config dir");

    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
    assert!(
        error_word(&body).contains("write failed"),
        "the body should name the failed write: {body}"
    );
    assert!(
        !h.plugin_config_path("present").exists(),
        "a failed write must leave no file behind"
    );
}

/// The catch-all arm `plugin_error_status` ends with is the one that silently
/// reclassifies a new variant, so the cases the config pair can produce are
/// pinned here — including the three that fall through to it.
/// `StoreUnreadable` is not reachable through this pair today (the write path
/// never reads `.permissions.toml`), but the mapping is what the route table
/// promises.
#[test]
fn the_plugin_error_status_map_is_the_one_the_config_route_promises() {
    assert_eq!(
        plugin_error_status(&PluginError::StoreUnreadable("x".to_string())),
        StatusCode::CONFLICT
    );
    assert_eq!(
        plugin_error_status(&PluginError::StoreWriteFailed("x".to_string())),
        StatusCode::INTERNAL_SERVER_ERROR
    );
    assert_eq!(
        plugin_error_status(&PluginError::PermissionDenied("sensitive".to_string())),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        plugin_error_status(&PluginError::MissingConfig(vec!["api_key".to_string()])),
        StatusCode::BAD_REQUEST
    );
    // The catch-all itself: a caller's own mistake stays `400`, so the split
    // between "your request was wrong" and "our write failed" is a real one.
    for error in [
        PluginError::Unavailable("no such plugin".into()),
        PluginError::HandleHeld("echo".into()),
        PluginError::InvalidManifest("bad toml".into()),
    ] {
        assert_eq!(plugin_error_status(&error), StatusCode::BAD_REQUEST);
    }
}

/// Design §3.2 W-deny: a consent decision that cannot be persisted must answer
/// `500` and change nothing. The failure is the daemon's — here
/// `.permissions.toml` cannot be written at all — and reporting it as `400`
/// tells the client to fix a request that was fine.
///
/// (Carried from the deleted `routes/plugins.rs`, whose `deny` handler is now
/// the `POST /v1/extensions/plugin/{id}/deny` verb.)
#[tokio::test]
async fn a_denial_that_cannot_be_persisted_is_a_500() {
    use std::os::unix::fs::PermissionsExt;

    let h = Harness::new();
    h.write_plugin("some-plugin");
    h.plugins.start().await.expect("plugin scan");

    // Unwritable but readable — the writer cannot even create its lock file —
    // so this is a **write** failure and not `409 store_unreadable`.
    std::fs::set_permissions(
        h.plugins_root.path(),
        std::fs::Permissions::from_mode(0o555),
    )
    .expect("chmod plugins root");
    let (status, _body) = h.verb("plugin", "some-plugin", Verb::Deny).await;
    std::fs::set_permissions(
        h.plugins_root.path(),
        std::fs::Permissions::from_mode(0o755),
    )
    .expect("restore plugins root");

    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
}

// ============================================================================
// R18 — a verb outlives its request
// ============================================================================

/// **R18.** An axum-dropped request future must not abandon a transition: the
/// verb runs in a detached task whose handle the handler awaits, so aborting
/// the request cancels the *wait*, not the work.
///
/// Without the detach the abort lands inside `load`, the record stays
/// `Enabling` for good — nothing else ever CASes it out — and every call to
/// that server is refused as *"being turned on"* until the daemon restarts.
#[tokio::test]
async fn a_dropped_request_does_not_abandon_the_transition() {
    let h = Harness::new();
    // The stub sleeps before answering `initialize`, so the verb is reliably
    // still inside E2 when the request is dropped.
    h.declare_stub("slow", 2);
    h.write_mcp(&format!(
        "[servers.slow]\ntransport = \"stdio\"\ncommand = \"{}\"\nenabled = false\nconnect_timeout_secs = 20\n",
        h.config_dir.path().join("slow-server.sh").display()
    ));
    h.mcp.reconcile_all().await;
    assert!(matches!(
        h.state("mcp", "slow"),
        Some(ExtensionState::Disabled)
    ));

    let extensions = h.extensions.clone();
    let request = tokio::spawn(async move {
        run_verb(extensions, "mcp", "slow", Verb::Enable).await;
    });
    // Long enough to be past E0's CAS and inside the handshake.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        matches!(h.state("mcp", "slow"), Some(ExtensionState::Enabling)),
        "the verb should be mid-transition when the request is dropped: {:?}",
        h.state("mcp", "slow")
    );
    request.abort();
    assert!(request.await.unwrap_err().is_cancelled());

    // The transition completes anyway, and ends terminal.
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        match h.state("mcp", "slow") {
            Some(ExtensionState::Enabled) => break,
            other => {
                assert!(
                    std::time::Instant::now() < deadline,
                    "the abandoned transition never finished; it is stuck at {other:?}"
                );
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
    assert!(
        !h.registry.extensions().tool_names(&ExtensionId::mcp("slow")).is_empty(),
        "the completed load published its tools"
    );

    h.mcp.shutdown_all().await;
}

/// The same property at the seam, without a two-second server: `detached`
/// itself must run its future to completion after its awaiter is gone.
#[tokio::test]
async fn detached_runs_to_completion_after_its_awaiter_is_dropped() {
    use std::sync::atomic::{AtomicBool, Ordering};

    let finished = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&finished);
    let waiter = tokio::spawn(async move {
        let _ = detached(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            flag.store(true, Ordering::SeqCst);
            Ok::<(), ExtensionError>(())
        })
        .await;
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    waiter.abort();
    assert!(waiter.await.unwrap_err().is_cancelled());

    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(
        finished.load(Ordering::SeqCst),
        "the detached work was cancelled with its awaiter"
    );
}

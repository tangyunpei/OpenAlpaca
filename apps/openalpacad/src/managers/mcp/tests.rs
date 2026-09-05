//! The C2 verification cell, test by test.
//!
//! Everything here drives a **real child process**. That is not thoroughness
//! for its own sake: the daemon holds no handle on an MCP child (rmcp owns it
//! inside `RunningService` and kills it from a detached task on close), so the
//! S2 guarantee — disabled means the child is gone and nothing respawns it —
//! is only observable *externally*. Every server writes its pid to a log on
//! start, and liveness is `kill -0` on those pids.
//!
//! The server is a POSIX `sh` script rather than a Node or Python one so the
//! suite has no dependency the workspace does not already have.

#![cfg(unix)]

use super::*;

use std::ffi::OsString;
use std::path::Path;
use std::sync::MutexGuard;

use openalpaca_core::agent::AgentRegistry;
use openalpaca_core::orchestrator::skill_catalog::SkillCatalog;
use openalpaca_core::tools::registry::ToolBackend;
use tempfile::TempDir;

// ============================================================================
// Env harness
// ============================================================================

/// The config writer rotates the replaced version into `state/backups/`, which
/// resolves through `OPENALPACA_HOME_STORE` on every call. Tests that write the
/// store must therefore not run concurrently, and no test ever touches the real
/// `~/.openalpaca`.
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub(crate) struct HomeStoreGuard {
    _lock: MutexGuard<'static, ()>,
    prev: Option<OsString>,
}

impl HomeStoreGuard {
    pub(crate) fn set(path: &Path) -> Self {
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
            None => unsafe { std::env::remove_var(openalpaca_storage::store::HOME_STORE_ENV) },
        }
    }
}

// ============================================================================
// The stub MCP server
// ============================================================================

#[derive(Default, Clone, Copy)]
struct StubOpts {
    /// Sleep this many seconds before answering `initialize`, so a test can
    /// hold a handshake open across a `disable`.
    slow_start_secs: u64,
    /// Never answer `tools/call`, which is how a call enters `reconnect()`.
    hang_call: bool,
    /// Run the background notifier that turns a trigger file into
    /// `notifications/tools/list_changed`.
    notifier: bool,
}

struct Stub {
    script: PathBuf,
    pid_log: PathBuf,
    tools_file: PathBuf,
    notify: PathBuf,
    /// While this exists, `tools/list` never answers — the caller's
    /// `request_timeout` is what ends the fetch.
    hang_list: PathBuf,
    /// While this exists, `tools/list` answers a JSON-RPC error.
    break_list: PathBuf,
}

impl Stub {
    /// The pids this server's incarnations reported, oldest first.
    fn pids(&self) -> Vec<i32> {
        std::fs::read_to_string(&self.pid_log)
            .unwrap_or_default()
            .lines()
            .filter_map(|l| l.trim().parse::<i32>().ok())
            .collect()
    }

    fn spawn_count(&self) -> usize {
        self.pids().len()
    }

    fn any_alive(&self) -> bool {
        self.pids().iter().any(|pid| alive(*pid))
    }

    /// Replace the tool set the next `tools/list` will answer with.
    fn set_tools(&self, tools: &[&str]) {
        std::fs::write(&self.tools_file, tools_json(tools)).expect("write tools file");
    }

    /// Ask the server to push `notifications/tools/list_changed`.
    fn notify(&self) {
        std::fs::write(&self.notify, "").expect("write notify trigger");
    }

    fn hang_next_list(&self) {
        std::fs::write(&self.hang_list, "").expect("write hang trigger");
    }

    fn break_next_list(&self) {
        std::fs::write(&self.break_list, "").expect("write break trigger");
    }

    fn kill_child(&self) {
        for pid in self.pids() {
            if alive(pid) {
                // SAFETY: a plain SIGKILL to a pid this test owns.
                unsafe { libc::kill(pid, libc::SIGKILL) };
            }
        }
    }
}

fn alive(pid: i32) -> bool {
    // SAFETY: `kill(pid, 0)` performs no signal delivery; it only reports
    // whether the pid exists and is signallable.
    unsafe { libc::kill(pid, 0) == 0 }
}

fn tools_json(names: &[&str]) -> String {
    let entries: Vec<String> = names
        .iter()
        .map(|n| {
            format!(
                r#"{{"name":"{n}","description":"stub {n}","inputSchema":{{"type":"object","properties":{{}}}}}}"#
            )
        })
        .collect();
    format!("[{}]", entries.join(","))
}

/// Write a stdio MCP server: a `sh` loop that answers `initialize`,
/// `tools/list`, `tools/call` and `ping`, and records every spawn.
fn stub_server(dir: &Path, tag: &str, opts: StubOpts) -> Stub {
    use std::os::unix::fs::PermissionsExt;

    let script = dir.join(format!("{tag}-server.sh"));
    let pid_log = dir.join(format!("{tag}-pids.log"));
    let tools_file = dir.join(format!("{tag}-tools.json"));
    let notify = dir.join(format!("{tag}-notify"));
    let hang_list = dir.join(format!("{tag}-hang-list"));
    let break_list = dir.join(format!("{tag}-break-list"));

    std::fs::write(&pid_log, "").expect("create pid log");
    std::fs::write(&tools_file, tools_json(&["echo"])).expect("create tools file");

    let slow_start = if opts.slow_start_secs > 0 {
        format!("sleep {}\n", opts.slow_start_secs)
    } else {
        String::new()
    };
    let hang_call = if opts.hang_call { "sleep 3600\n" } else { "" };
    // The notifier is a child of this shell, and a SIGKILL of the shell will
    // not reach it — so it polls its parent and exits with it. Without that a
    // torn-down server would leave a stray process behind.
    let notifier = if opts.notifier {
        format!(
            r#"PARENT=$$
(
  while kill -0 "$PARENT" 2>/dev/null; do
    if [ -f '{notify}' ]; then
      rm -f '{notify}'
      printf '%s\n' '{{"jsonrpc":"2.0","method":"notifications/tools/list_changed"}}'
    fi
    sleep 0.05
  done
) &
"#,
            notify = notify.display()
        )
    } else {
        String::new()
    };

    let body = format!(
        r#"#!/bin/sh
echo "$$" >> '{pid_log}'
{slow_start}{notifier}while IFS= read -r line; do
  id=`printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p'`
  case "$line" in
    *'"method":"initialize"'*)
      pv=`printf '%s' "$line" | sed -n 's/.*"protocolVersion":"\([^"]*\)".*/\1/p'`
      printf '{{"jsonrpc":"2.0","id":%s,"result":{{"protocolVersion":"%s","capabilities":{{"tools":{{"listChanged":true}}}},"serverInfo":{{"name":"stub","version":"0.0.1"}}}}}}\n' "$id" "$pv"
      ;;
    *'"method":"tools/list"'*)
      if [ -f '{hang_list}' ]; then sleep 3600; fi
      if [ -f '{break_list}' ]; then
        printf '{{"jsonrpc":"2.0","id":%s,"error":{{"code":-32603,"message":"stub refuses to list"}}}}\n' "$id"
      else
        printf '{{"jsonrpc":"2.0","id":%s,"result":{{"tools":%s}}}}\n' "$id" "`cat '{tools_file}'`"
      fi
      ;;
    *'"method":"tools/call"'*)
      {hang_call}printf '{{"jsonrpc":"2.0","id":%s,"result":{{"content":[{{"type":"text","text":"ok"}}],"isError":false}}}}\n' "$id"
      ;;
    *'"method":"ping"'*)
      printf '{{"jsonrpc":"2.0","id":%s,"result":{{}}}}\n' "$id"
      ;;
  esac
done
"#,
        pid_log = pid_log.display(),
        tools_file = tools_file.display(),
        hang_list = hang_list.display(),
        break_list = break_list.display(),
    );

    std::fs::write(&script, body).expect("write stub server");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
        .expect("chmod stub server");

    Stub {
        script,
        pid_log,
        tools_file,
        notify,
        hang_list,
        break_list,
    }
}

// ============================================================================
// Harness
// ============================================================================

struct Harness {
    _home: TempDir,
    _env: HomeStoreGuard,
    _skills_dir: TempDir,
    dir: TempDir,
    config_path: PathBuf,
    registry: Arc<ToolRegistry>,
    supervisor: Arc<McpSupervisor>,
    bus: EventBus,
    agents: Arc<AgentRegistry>,
    skills: Arc<SkillCatalog>,
}

/// The default lane every harness scan writes its notice to.
const NOTICE_LANE: &str = "owner:gui";

impl Harness {
    /// A supervisor over a fresh temp config dir. `drain_secs` is
    /// `[extensions] drain_timeout_secs`.
    ///
    /// The registry is built with `with_event_bus` — the production shape from
    /// C4 (`services/tools.rs`) — so the ledger publishes `mark_failed`'s own
    /// `failed` event and T1 step 3's `ExtensionCapabilityWithdrawn`. The two
    /// dependent handles are installed the way `services/mcp.rs` installs them.
    fn new(drain_secs: u64) -> Self {
        let home = tempfile::tempdir().expect("home store");
        let env = HomeStoreGuard::set(home.path());
        let dir = tempfile::tempdir().expect("config dir");
        let config_path = dir.path().join("mcp.toml");

        let bus = EventBus::new(256);
        let registry = Arc::new(ToolRegistry::with_event_bus(bus.clone()).expect("tool registry"));
        let mut cfg = DaemonConfig::default();
        cfg.extensions.drain_timeout_secs = drain_secs;
        let daemon_config = Arc::new(ArcSwap::from_pointee(cfg));
        let agents = Arc::new(AgentRegistry::new());
        let skills_dir = tempfile::tempdir().expect("skills dir");
        let skills = Arc::new(SkillCatalog::new());
        let supervisor = McpSupervisor::new(
            config_path.clone(),
            Arc::clone(&registry),
            daemon_config,
            bus.clone(),
            Some(Arc::clone(&skills)),
            Some(Arc::clone(&agents)),
            NOTICE_LANE,
        );

        Self {
            _home: home,
            _env: env,
            _skills_dir: skills_dir,
            dir,
            config_path,
            registry,
            supervisor,
            bus,
            agents,
            skills,
        }
    }

    /// Register an agent template declaring `caps` — a dependent for T1 step 3.
    fn declare_template(&self, id: &str, caps: &[&str]) {
        self.agents
            .register_template(openalpaca_core::agent::template::AgentTemplate {
                frontmatter: openalpaca_core::agent::template::AgentTemplateFrontmatter {
                    id: id.into(),
                    name: id.into(),
                    description: String::new(),
                    icon: None,
                    singleton: false,
                    capabilities: caps.iter().map(|c| c.to_string()).collect(),
                    denied_capabilities: vec![],
                    temperature: 0.5,
                    verbosity: "normal".into(),
                    model: None,
                    fallback_models: vec![],
                    max_tool_calls: None,
                    timeout_seconds: None,
                    max_cost_per_task: None,
                    max_rounds: None,
                    require_confirmation_for: vec![],
                },
                body: String::new(),
                sections: std::collections::HashMap::new(),
                source: Default::default(),
            });
    }

    /// Scan one `SKILL.md` into the catalog — the other kind of dependent.
    fn declare_skill(&self, id: &str, body: &str) {
        let d = self._skills_dir.path().join(id);
        std::fs::create_dir_all(&d).expect("skill dir");
        std::fs::write(d.join("SKILL.md"), body).expect("write SKILL.md");
        self.skills.scan_directory(
            self._skills_dir.path(),
            openalpaca_core::middleware::skill::SkillScope::Project,
        );
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn write_config(&self, body: &str) {
        std::fs::write(&self.config_path, body).expect("write mcp.toml");
    }

    /// The declaration every test starts from: one stdio server named `srv`.
    fn declare(&self, stub: &Stub, enabled: bool) {
        self.write_config(&declaration(stub, enabled, ""));
    }

    fn record(&self, name: &str) -> Option<ExtensionRecord> {
        self.registry.extensions().record(&ExtensionId::mcp(name))
    }

    fn state(&self, name: &str) -> Option<ExtensionState> {
        self.record(name).map(|r| r.state)
    }

    /// The `Arc<McpClient>` a registry entry holds — how a test grabs the
    /// handle a stale snapshot would keep.
    fn client_of(&self, tool: &str) -> Option<Arc<openalpaca_mcp::McpClient>> {
        match self.registry.get(tool)?.backend {
            ToolBackend::Mcp { client, .. } => Some(client),
            _ => None,
        }
    }

    fn ledger(&self) -> &Arc<ExtensionLedger> {
        self.registry.extensions()
    }
}

/// The shipped declaration shape, with `extra` spliced into the server block.
fn declaration(stub: &Stub, enabled: bool, extra: &str) -> String {
    format!(
        r#"# a hand-authored comment that must survive every write
[defaults]
connect_timeout_secs = 10
request_timeout_secs = 3
max_reconnect_attempts = 3
reconnect_backoff_ms = 20

[servers.srv]
transport = "stdio"
command = "{command}"
enabled = {enabled}
{extra}
"#,
        command = stub.script.display(),
    )
}

/// Poll a condition to a deadline. Returns whether it came true.
async fn eventually(timeout: Duration, mut check: impl FnMut() -> bool) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if check() {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn mcp(name: &str) -> ExtensionId {
    ExtensionId::mcp(name)
}

// ============================================================================
// The integration test the cell names first
// ============================================================================

/// **Enable → disable → re-enable, against a real child.**
///
/// The whole S2 guarantee in one scenario: the child dies inside the T4 bound,
/// a snapshot taken before the disable refuses (Fact 1), the live registry
/// refuses with attribution (Fact 2), **no new pidfile appears** — and the
/// re-enable brings the tools back with no duplicate index edges while that
/// same pre-disable snapshot now refuses as `Stale`.
#[tokio::test]
async fn a_disable_kills_the_child_refuses_both_arms_and_a_re_enable_restores_cleanly() {
    let h = Harness::new(1);
    let stub = stub_server(h.path(), "toggle", StubOpts::default());
    h.declare(&stub, true);

    h.supervisor.reconcile_all().await;

    // Enabled: one child, one tool, one index edge.
    assert_eq!(h.state("srv"), Some(ExtensionState::Enabled));
    assert_eq!(stub.spawn_count(), 1, "one child for one load");
    assert!(stub.any_alive(), "the child is up");
    assert!(h.registry.get("srv__echo").is_some(), "the tool is registered");
    assert_eq!(h.registry.capability_index_entry("srv__echo").len(), 1);

    // A live call proves the transport really works before we tear it down.
    let ok = h
        .registry
        .execute("srv__echo", &serde_json::json!({}))
        .await
        .expect("the enabled server serves its tool");
    assert_eq!(ok, "ok");

    // Fact 1: the deep snapshot a lead agent holds for its whole run.
    let snapshot = (*h.registry).clone();
    let gen_one = h.record("srv").expect("record").generation;

    h.supervisor.disable(&mcp("srv")).await.expect("disable");

    assert_eq!(h.state("srv"), Some(ExtensionState::Disabled));
    assert!(
        eventually(Duration::from_secs(5), || !stub.any_alive()).await,
        "the child must be gone within the T4 bound"
    );

    // The snapshot still holds the entry and its (sealed) client — and is
    // refused with the S4 wording, not a transport string.
    let refusal = snapshot
        .execute("srv__echo", &serde_json::json!({}))
        .await
        .expect_err("a snapshot must be refused after a disable");
    assert!(
        refusal.contains("is disabled by the owner"),
        "expected the disabled row, got: {refusal}"
    );

    // The live registry lost the entry at T1; the miss arm attributes it.
    let live_refusal = h
        .registry
        .execute("srv__echo", &serde_json::json!({}))
        .await
        .expect_err("the live registry must refuse too");
    assert!(
        live_refusal.contains("is disabled by the owner"),
        "expected an attributed refusal, got: {live_refusal}"
    );

    assert_eq!(
        stub.spawn_count(),
        1,
        "no respawn: a disabled server must not resurrect itself"
    );

    // Re-enable.
    h.supervisor.enable(&mcp("srv")).await.expect("enable");
    assert_eq!(h.state("srv"), Some(ExtensionState::Enabled));
    let gen_two = h.record("srv").expect("record").generation;
    assert!(gen_two > gen_one, "a load bumps the generation");
    assert_eq!(stub.spawn_count(), 2, "exactly one new child");
    assert!(h.registry.get("srv__echo").is_some(), "the tool is back");
    assert_eq!(
        h.registry.capability_index_entry("srv__echo").len(),
        1,
        "remove-before-register: no duplicate index edges"
    );

    // The pre-disable snapshot now refuses as Stale while the live registry
    // serves the new load.
    let stale = snapshot
        .execute("srv__echo", &serde_json::json!({}))
        .await
        .expect_err("the old handle belongs to a previous load");
    assert!(
        stale.contains("belongs to a previous load"),
        "expected the stale row, got: {stale}"
    );
    let served = h
        .registry
        .execute("srv__echo", &serde_json::json!({}))
        .await
        .expect("the new load serves");
    assert_eq!(served, "ok");

    h.supervisor.shutdown_all().await;
}

// ============================================================================
// T4b, window 2 — the seal that closes the in-flight reconnect
// ============================================================================

/// **The race the seal exists for.** A hung call times out and enters
/// `reconnect()`, which spawns a *fresh* child; the drain expires while that
/// handshake is still in flight, so T4's `disconnect` finds the service lock
/// free and `service == None`, seals, and returns. The handshake then completes
/// — and must close the child it just spawned instead of installing it.
///
/// Without the install-point check the disabled server would be **running**:
/// the registry entry is gone, the supervisor never calls `disconnect` on that
/// incarnation again, and `McpClient` has no `Drop`.
#[tokio::test]
async fn a_disable_that_lands_mid_reconnect_seals_the_client_and_leaves_no_child() {
    let h = Harness::new(1);
    let stub = stub_server(
        h.path(),
        "seal",
        StubOpts {
            // Slow enough that the reconnect's handshake outlives the drain.
            slow_start_secs: 2,
            hang_call: true,
            ..StubOpts::default()
        },
    );
    h.write_config(&format!(
        r#"[defaults]
connect_timeout_secs = 10
request_timeout_secs = 1
max_reconnect_attempts = 3
reconnect_backoff_ms = 20

[servers.srv]
transport = "stdio"
command = "{}"
"#,
        stub.script.display()
    ));

    h.supervisor.reconcile_all().await;
    assert_eq!(h.state("srv"), Some(ExtensionState::Enabled));
    assert_eq!(stub.spawn_count(), 1);

    // The stale snapshot's handle — the one that must end up sealed.
    let client = h.client_of("srv__echo").expect("the entry holds a client");

    // A call that hangs: it times out after `request_timeout`, and `Timeout` is
    // retriable, so `call_tool` enters `reconnect()` — which spawns a second
    // child that will not finish its handshake for two seconds.
    let registry = Arc::clone(&h.registry);
    let call = tokio::spawn(async move {
        registry
            .execute("srv__echo", &serde_json::json!({}))
            .await
    });

    // Let the call time out and the reconnect's handshake get under way.
    assert!(
        eventually(Duration::from_secs(5), || stub.spawn_count() >= 2).await,
        "the retriable timeout should have entered reconnect and respawned"
    );

    h.supervisor.disable(&mcp("srv")).await.expect("disable");
    assert_eq!(h.state("srv"), Some(ExtensionState::Disabled));

    // The just-spawned child is closed by the sealed install point.
    assert!(
        eventually(Duration::from_secs(8), || !stub.any_alive()).await,
        "no child may survive a disable, including one a racing handshake spawned"
    );

    let _ = call.await;
    let spawns_after_disable = stub.spawn_count();

    // Seal type: terminal by type, and never retriable.
    let err = client
        .call_tool("echo", serde_json::json!({}), None)
        .await
        .expect_err("a sealed client must refuse");
    assert!(
        matches!(err, McpError::Closed),
        "expected Closed (not TransportClosed), got {err:?}"
    );
    assert!(!err.is_retriable(), "Closed must never be retriable");
    assert_eq!(
        stub.spawn_count(),
        spawns_after_disable,
        "a sealed client must not spawn"
    );
}

// ============================================================================
// Write-first
// ============================================================================

/// **W before T0.** A store that cannot be written aborts the verb: `500`, no
/// CAS, the extension still running and the row still reading the truth.
#[tokio::test]
async fn a_store_that_cannot_be_written_makes_disable_a_500_and_changes_nothing() {
    use std::os::unix::fs::PermissionsExt;

    let h = Harness::new(1);
    let stub = stub_server(h.path(), "readonly", StubOpts::default());
    h.declare(&stub, true);
    h.supervisor.reconcile_all().await;
    assert_eq!(h.state("srv"), Some(ExtensionState::Enabled));

    // The write needs to create `<path>.lock` beside the store; a directory it
    // cannot write is the honest "this store is not writable".
    let before = std::fs::read_to_string(&h.config_path).expect("read store");
    std::fs::set_permissions(h.path(), std::fs::Permissions::from_mode(0o555))
        .expect("make the config dir read-only");

    let err = h
        .supervisor
        .disable(&mcp("srv"))
        .await
        .expect_err("a failed write must abort the verb");

    std::fs::set_permissions(h.path(), std::fs::Permissions::from_mode(0o755))
        .expect("restore the config dir");

    assert!(
        matches!(err, ExtensionError::WriteFailed(_)),
        "expected the 500 shape, got {err:?}"
    );
    let record = h.record("srv").expect("the row survives");
    assert!(record.disposition.0, "the bit still reads true");
    assert_eq!(record.state, ExtensionState::Enabled, "no CAS was taken");
    assert!(stub.any_alive(), "the server is still up");
    assert!(
        h.registry.get("srv__echo").is_some(),
        "nothing was withdrawn"
    );
    assert_eq!(
        std::fs::read_to_string(&h.config_path).expect("read store"),
        before,
        "the store is byte-identical"
    );

    h.supervisor.shutdown_all().await;
}

/// **The enable half.** Recording the intent succeeded; the connection outcome
/// is a separate fact in the body. So: `200` with `enabled: true, state:
/// failed` — and a supervisor restart reads the bit as `true` and tries again,
/// which is what makes the owner's intent durable.
#[tokio::test]
async fn enabling_an_unreachable_command_returns_the_row_with_the_bit_true_and_state_failed() {
    let h = Harness::new(1);
    let stub = stub_server(h.path(), "gone", StubOpts::default());
    h.declare(&stub, false);
    h.supervisor.reconcile_all().await;
    assert_eq!(h.state("srv"), Some(ExtensionState::Disabled));

    // Make the command un-spawnable *before* the enable.
    std::fs::remove_file(&stub.script).expect("delete the server script");

    let row = h
        .supervisor
        .enable(&mcp("srv"))
        .await
        .expect("enable returns the row, not an error");
    assert!(row.disposition.0, "the bit is durable");
    assert!(
        matches!(
            row.state,
            ExtensionState::Failed {
                reason: FailureReason::Unreachable,
                ..
            }
        ),
        "a command that will not spawn is unreachable, got {:?}",
        row.state
    );
    assert!(
        std::fs::read_to_string(&h.config_path)
            .expect("read store")
            .contains("enabled = true"),
        "W wrote the bit before E0"
    );

    // A fresh supervisor over the same store — a restart — reads the bit and
    // re-tries, because a `Failed` record never persists across a boot.
    let registry = Arc::new(ToolRegistry::new().expect("registry"));
    let restarted = McpSupervisor::new(
        h.config_path.clone(),
        Arc::clone(&registry),
        Arc::new(ArcSwap::from_pointee(DaemonConfig::default())),
        EventBus::new(16),
        None,
        None,
        NOTICE_LANE,
    );
    restarted.reconcile_all().await;
    let after = registry
        .extensions()
        .record(&mcp("srv"))
        .expect("the restart built a row");
    assert!(after.disposition.0, "the restart read the bit as true");
    assert!(
        matches!(after.state, ExtensionState::Failed { .. }),
        "and tried again, got {:?}",
        after.state
    );
}

// ============================================================================
// The declaration is the toggle
// ============================================================================

/// **T5-gone.** The block left the file, so the bit left with it. T0–T4 runs
/// with **no file write** — the writer's mandatory re-parse would reject a
/// synthesized `[servers.<n>]` table with no `transport` tag — and the record
/// is dropped rather than parked.
#[tokio::test]
async fn a_deleted_declaration_tears_the_server_down_and_writes_nothing() {
    let h = Harness::new(1);
    let stub = stub_server(h.path(), "gone-decl", StubOpts::default());
    h.declare(&stub, true);
    h.supervisor.reconcile_all().await;
    assert!(stub.any_alive());

    // The owner's edit: the block is simply gone.
    let edited = "[defaults]\nconnect_timeout_secs = 10\n";
    h.write_config(edited);
    h.supervisor.reconcile_all().await;

    assert!(
        eventually(Duration::from_secs(5), || !stub.any_alive()).await,
        "the child goes with the declaration"
    );
    assert!(h.registry.get("srv__echo").is_none(), "tools withdrawn");
    assert!(h.record("srv").is_none(), "the record is dropped, not parked");
    assert!(
        h.supervisor.list().await.iter().all(|r| r.id.name != "srv"),
        "the row disappears from list()"
    );
    assert_eq!(
        std::fs::read_to_string(&h.config_path).expect("read store"),
        edited,
        "no write is attempted on the declaration-gone path"
    );
}

/// **A pre-reaper `Failed{Crashed}` whose declaration then vanishes.** The
/// residue must still come down — there is no T0 and no drain on this exit,
/// but a child that outlived its declaration would be an S2 hole.
#[tokio::test]
async fn a_crash_then_a_deleted_declaration_leaves_no_residue() {
    let h = Harness::new(1);
    let stub = stub_server(h.path(), "crash-gone", StubOpts::default());
    h.declare(&stub, true);
    h.supervisor.reconcile_all().await;
    let generation = h.record("srv").expect("record").generation;
    assert_eq!(h.supervisor.readers_running(), 1);

    // The crash lands but the reaper has not run: the record owns its handle.
    assert!(h.ledger().mark_failed(
        &mcp("srv"),
        generation,
        FailureReason::Crashed,
        "the child went away"
    ));

    h.write_config("[defaults]\nconnect_timeout_secs = 10\n");
    h.supervisor.reconcile_all().await;

    assert!(h.registry.get("srv__echo").is_none(), "no registry entry");
    assert!(
        h.registry.extension_tool_defs(&[]).is_empty(),
        "and nothing from it on any surface"
    );
    assert!(h.record("srv").is_none(), "the record is gone");
    assert!(
        eventually(Duration::from_secs(5), || !stub.any_alive()).await,
        "the residue's child is torn down"
    );
    assert!(
        eventually(Duration::from_secs(5), || h.supervisor.readers_running() == 0).await,
        "the reader task exited with its client"
    );
}

/// **Edge case 15's one disable path with no route behind it.** A hand edit to
/// `enabled = false` is authoritative, and produces exactly the same three
/// refusals a route disable does.
#[tokio::test]
async fn a_hand_edited_enabled_false_disables_through_the_watcher_path() {
    let h = Harness::new(1);
    let stub = stub_server(h.path(), "watcher", StubOpts::default());
    h.declare(&stub, true);
    h.supervisor.reconcile_all().await;
    assert_eq!(h.state("srv"), Some(ExtensionState::Enabled));
    let snapshot = (*h.registry).clone();

    h.declare(&stub, false);
    h.supervisor.reconcile_all().await;

    assert_eq!(h.state("srv"), Some(ExtensionState::Disabled));
    assert!(
        eventually(Duration::from_secs(5), || !stub.any_alive()).await,
        "the child is gone"
    );
    let from_snapshot = snapshot
        .execute("srv__echo", &serde_json::json!({}))
        .await
        .expect_err("the snapshot is refused");
    assert!(from_snapshot.contains("is disabled by the owner"));
    let from_live = h
        .registry
        .execute("srv__echo", &serde_json::json!({}))
        .await
        .expect_err("the live registry is refused");
    assert!(from_live.contains("is disabled by the owner"));
    assert_eq!(stub.spawn_count(), 1, "no respawn");
}

// ============================================================================
// The crash reaper
// ============================================================================

/// **A reap that arrives after a Retry.** The mutex prevents interleaving, not
/// reordering: by the time the reaper takes it, `enable` has already built load
/// N+1. An unconditional T1 → T4 here would unpublish its tools and kill its
/// live process while leaving the row `Enabled`.
#[tokio::test]
async fn a_reap_that_lands_after_a_retry_is_superseded_and_touches_nothing() {
    let h = Harness::new(1);
    let stub = stub_server(h.path(), "superseded", StubOpts::default());
    h.declare(&stub, true);
    h.supervisor.reconcile_all().await;
    let gen_n = h.record("srv").expect("record").generation;

    // The crash. Its reaper message is queued, not delivered — this test *is*
    // the reaper, released by hand.
    assert!(h.ledger().mark_failed(&mcp("srv"), gen_n, FailureReason::Crashed, "boom"));

    // The owner clicks Retry first.
    h.supervisor.enable(&mcp("srv")).await.expect("enable");
    let gen_next = h.record("srv").expect("record").generation;
    assert!(gen_next > gen_n, "load N+1");
    assert_eq!(h.state("srv"), Some(ExtensionState::Enabled));

    // E-PRE tore load N down before building N+1, so the reaper *finds*
    // nothing rather than being trusted to do nothing.
    assert!(
        eventually(Duration::from_secs(5), || h.supervisor.readers_running() == 1).await,
        "load N's reader task exited at E-PRE"
    );
    for (_, tool) in h.registry.iter_registered_tools() {
        assert_ne!(
            tool.incarnation(),
            Some(gen_n),
            "no registry entry may carry the superseded generation"
        );
    }
    assert!(
        h.ledger().audit(&h.registry).is_empty(),
        "every registered extension tool has a ledger record"
    );

    // Now release the reaper.
    h.supervisor.reap(&mcp("srv"), gen_n).await;

    assert_eq!(h.state("srv"), Some(ExtensionState::Enabled), "the row is untouched");
    assert!(h.registry.get("srv__echo").is_some(), "load N+1's tools remain");
    assert!(stub.any_alive(), "load N+1's child is alive");
    assert!(
        h.record("srv").expect("record").disposition.0,
        "the row still reads enabled"
    );

    h.supervisor.shutdown_all().await;
}

/// **`mcp_supervisor_records_every_registered_tool`** — the §6.2a fail-open is
/// only safe while an unrecorded registration is visible.
#[tokio::test]
async fn mcp_supervisor_records_every_registered_tool() {
    let h = Harness::new(1);
    let stub = stub_server(h.path(), "audit", StubOpts::default());
    stub.set_tools(&["echo", "second", "third"]);
    h.declare(&stub, true);
    h.supervisor.reconcile_all().await;

    for name in ["srv__echo", "srv__second", "srv__third"] {
        assert!(h.registry.get(name).is_some(), "{name} registered");
    }
    assert!(
        h.ledger().audit(&h.registry).is_empty(),
        "audit must be empty after reconcile_all"
    );

    h.supervisor.shutdown_all().await;
}

// ============================================================================
// Runtime death — written to what `reconnect` actually does
// ============================================================================

/// **The crash sequence, stated rather than hidden (§3.6 item 1).**
///
/// A stdio server's `reconnect()` **respawns the child**, and a successful
/// handshake resets the attempt counter — so a child killed out of band while
/// its command still runs is transparently recovered and the row correctly
/// stays `Enabled`. Only after **four** consecutive `reconnect()` entries with
/// no successful handshake does the client report `ReconnectExhausted`, and
/// only that variant marks the row `failed/crashed`.
///
/// This is the opposite of Claude Code's stdio policy, and deliberately so
/// (§13 Q8, not applied): the cost is that a crashed child can be respawned
/// three times before anyone learns it crashed. The test is written to that
/// sequence so nobody "fixes" it into a first-failure rule by accident.
#[tokio::test]
async fn a_stdio_server_respawns_transparently_then_reports_crashed_after_four_failures() {
    let h = Harness::new(1);
    let stub = stub_server(h.path(), "crash", StubOpts::default());
    h.write_config(&format!(
        r#"[defaults]
connect_timeout_secs = 10
request_timeout_secs = 1
max_reconnect_attempts = 3
reconnect_backoff_ms = 10

[servers.srv]
transport = "stdio"
command = "{}"
"#,
        stub.script.display()
    ));

    // This test *is* the reaper: take its channel so nothing runs behind us.
    let mut reaper = h
        .supervisor
        .reaper_rx
        .lock_or_recover()
        .take()
        .expect("the reaper channel is parked until spawn_reaper");

    h.supervisor.reconcile_all().await;
    let generation = h.record("srv").expect("record").generation;
    assert_eq!(stub.spawn_count(), 1);

    // (1) The recovery. The command still runs, so the next call respawns and
    // succeeds — and the row stays `Enabled`, because the server *is* running.
    stub.kill_child();
    let recovered = h
        .registry
        .execute("srv__echo", &serde_json::json!({}))
        .await
        .expect("a live client respawns its child and the call succeeds");
    assert_eq!(recovered, "ok");
    assert_eq!(stub.spawn_count(), 2, "the child was respawned");
    assert_eq!(h.state("srv"), Some(ExtensionState::Enabled), "still active");

    // (2) Now make the command un-spawnable and drive four calls.
    stub.kill_child();
    std::fs::remove_file(&stub.script).expect("delete the server script");

    for attempt in 1..=3 {
        let err = h
            .registry
            .execute("srv__echo", &serde_json::json!({}))
            .await
            .expect_err("the handshake cannot succeed");
        assert!(
            !err.contains("is not running"),
            "attempt {attempt} must fail with its own handshake error, got: {err}"
        );
        assert_eq!(
            h.state("srv"),
            Some(ExtensionState::Enabled),
            "the row is still active after {attempt} failed reconnect(s)"
        );
    }

    let fourth = h
        .registry
        .execute("srv__echo", &serde_json::json!({}))
        .await
        .expect_err("the fourth entry exhausts the budget");
    assert!(
        fourth.contains("reconnect attempts exhausted"),
        "expected ReconnectExhausted, got: {fourth}"
    );
    assert!(
        matches!(
            h.state("srv"),
            Some(ExtensionState::Failed {
                reason: FailureReason::Crashed,
                ..
            })
        ),
        "the row now reads failed/crashed, got {:?}",
        h.state("srv")
    );

    // The reaper is what unpublishes and seals.
    let queued = reaper.try_recv().expect("mark_failed queued a reap");
    assert_eq!(queued, (mcp("srv"), generation));
    h.supervisor.reap(&mcp("srv"), generation).await;

    assert!(h.registry.get("srv__echo").is_none(), "tools unpublished");
    assert!(
        h.record("srv").expect("record").disposition.0,
        "the reaper writes no state: the bit stays true and the row stays failed"
    );
    let spawns = stub.spawn_count();
    assert!(
        h.registry
            .execute("srv__echo", &serde_json::json!({}))
            .await
            .expect_err("refused")
            .contains("stopped unexpectedly"),
        "the S4 crash row, not a transport string"
    );
    assert_eq!(stub.spawn_count(), spawns, "a failed server never respawns again");

    // (3) Retry recovers, with a new generation. Rewriting the same script
    // under the same tag restores the command the declaration names.
    let stub2 = stub_server(h.path(), "crash", StubOpts::default());
    let row = h.supervisor.enable(&mcp("srv")).await.expect("enable");
    assert_eq!(row.state, ExtensionState::Enabled, "Retry recovers");
    assert!(row.generation > generation, "with a new generation");
    assert!(stub2.any_alive(), "and a new child");

    h.supervisor.shutdown_all().await;
}

// ============================================================================
// Reload
// ============================================================================

/// **`reload` is the third verb.** Enable on `Enabled` is a CAS no-op and must
/// stay one, so this is the one-step way to pick up a rotated credential or an
/// edited `command`: T0–T4 then E0–E5 under one hold of the mutex, bit
/// untouched, no W.
#[tokio::test]
async fn reload_bumps_the_generation_keeps_the_bit_and_staleifies_a_snapshot() {
    let h = Harness::new(1);
    let stub = stub_server(h.path(), "reload", StubOpts::default());
    h.declare(&stub, true);
    h.supervisor.reconcile_all().await;
    let before = h.record("srv").expect("record");
    let snapshot = (*h.registry).clone();
    let store_before = std::fs::read_to_string(&h.config_path).expect("read store");

    let row = h.supervisor.reload(&mcp("srv")).await.expect("reload");

    assert_eq!(row.state, ExtensionState::Enabled, "it ends Enabled");
    assert!(row.disposition.0, "the bit is untouched");
    assert!(row.generation > before.generation, "the generation bumps");
    assert_eq!(
        std::fs::read_to_string(&h.config_path).expect("read store"),
        store_before,
        "reload performs no W"
    );
    assert_eq!(stub.spawn_count(), 2, "a fresh client, a fresh child");

    let stale = snapshot
        .execute("srv__echo", &serde_json::json!({}))
        .await
        .expect_err("the pre-reload handle belongs to a previous load");
    assert!(
        stale.contains("belongs to a previous load"),
        "expected the stale row, got: {stale}"
    );
    assert_eq!(
        h.registry
            .execute("srv__echo", &serde_json::json!({}))
            .await
            .expect("the new load serves"),
        "ok"
    );

    // `reload` is not the verb that turns things on.
    h.supervisor.disable(&mcp("srv")).await.expect("disable");
    assert!(
        matches!(
            h.supervisor.reload(&mcp("srv")).await,
            Err(ExtensionError::NotLoaded)
        ),
        "reload from Disabled is not_loaded"
    );
}

/// **Fix round 1, finding 1 (the second face).** `reload`'s job is *"apply my
/// edit"* (§3.4.1), and §10 case 15 says the route path never depends on the
/// watcher — filesystem events are `try_send` with drop-on-full and losing one
/// is explicitly tolerated. So `reload` must read the declaration **on disk**
/// under its own mutex hold, not the set the last reconcile happened to cache:
/// otherwise a dropped event makes it reconnect the *old* declaration, bump the
/// generation and report `Enabled` — a silent lie.
#[tokio::test]
async fn a_reload_applies_an_edit_the_watcher_never_delivered() {
    let h = Harness::new(1);
    let old = stub_server(h.path(), "reload-old", StubOpts::default());
    let new = stub_server(h.path(), "reload-new", StubOpts::default());
    h.declare(&old, true);
    h.supervisor.reconcile_all().await;
    assert_eq!(h.state("srv"), Some(ExtensionState::Enabled));
    assert_eq!(old.spawn_count(), 1, "the declared command started");

    // The owner edits `command`. The watcher event is dropped: no reconcile
    // runs, so the supervisor's cached declaration still names the old command.
    h.write_config(&declaration(&new, true, ""));

    let row = h.supervisor.reload(&mcp("srv")).await.expect("reload");

    assert_eq!(row.state, ExtensionState::Enabled);
    assert_eq!(new.spawn_count(), 1, "reload started the command on disk");
    assert!(new.any_alive(), "and that is the live child");
    assert_eq!(old.spawn_count(), 1, "the old command was not started again");
    assert!(
        eventually(Duration::from_secs(5), || !old.any_alive()).await,
        "the previous load's child is gone"
    );

    // E2 stamped the fingerprint of the block it actually built from.
    let on_disk = McpConfig::load(&h.config_path).expect("parse");
    assert_eq!(
        h.record("srv").expect("record").config_fingerprint,
        Some(config_fingerprint(on_disk.servers.get("srv").expect("srv"))),
        "the stored fingerprint is the on-disk declaration's"
    );

    h.supervisor.shutdown_all().await;
}

// ============================================================================
// The fingerprint half of edge case 15's diff key
// ============================================================================

/// **§3.4 trigger 2.** For a `Failed` record the fingerprint half is consulted
/// **regardless** of §13 Q9 — it is what makes "edit the declaration to retry"
/// work without retrying every failed server on any edit.
#[tokio::test]
async fn editing_a_failed_servers_command_reloads_it_through_the_watcher() {
    let h = Harness::new(1);
    let stub = stub_server(h.path(), "fp-failed", StubOpts::default());
    h.write_config(
        r#"[defaults]
connect_timeout_secs = 5
request_timeout_secs = 3

[servers.srv]
transport = "stdio"
command = "/definitely/not/a/command/xyzzy"
"#,
    );
    h.supervisor.reconcile_all().await;
    assert!(
        matches!(h.state("srv"), Some(ExtensionState::Failed { .. })),
        "the declared command does not exist"
    );
    let failed = h.record("srv").expect("record");

    // The owner fixes the declaration. Nothing else changes.
    h.declare(&stub, true);
    h.supervisor.reconcile_all().await;

    let after = h.record("srv").expect("record");
    assert_eq!(after.state, ExtensionState::Enabled, "the edit is the retry");
    assert!(after.generation > failed.generation);
    assert_ne!(after.config_fingerprint, failed.config_fingerprint);

    h.supervisor.shutdown_all().await;
}

/// **§13 Q9 is pending, so it is not applied.** A changed block on a *live*
/// server takes effect at the next `reload`/`enable`; the watcher applies the
/// bit alone and says so in the log. Auto-reloading here would be adopting an
/// owner decision by default.
#[tokio::test]
async fn editing_an_enabled_servers_command_changes_nothing_until_a_reload() {
    let h = Harness::new(1);
    let stub = stub_server(h.path(), "fp-enabled", StubOpts::default());
    h.declare(&stub, true);
    h.supervisor.reconcile_all().await;
    let before = h.record("srv").expect("record");
    let pids_before = stub.pids();

    // A real value edit: an added argument.
    h.write_config(&declaration(&stub, true, r#"args = ["--verbose"]"#));
    h.supervisor.reconcile_all().await;

    let after = h.record("srv").expect("record");
    assert_eq!(after.state, ExtensionState::Enabled, "the live server is untouched");
    assert_eq!(after.generation, before.generation, "no reload");
    assert_eq!(
        after.config_fingerprint, before.config_fingerprint,
        "the stored fingerprint is the one the *live* load was built from, so the \
         notice repeats until the edit is actually applied"
    );
    assert_eq!(stub.pids(), pids_before, "and no new child");

    h.supervisor.shutdown_all().await;
}

/// **The mask.** A rotated credential *value* under an unchanged name changes
/// the fingerprint of nothing — by design, which is why env-var indirection
/// stays the recommended declaration shape and why `reload` exists.
#[tokio::test]
async fn a_rotated_env_value_changes_the_fingerprint_of_nothing() {
    let h = Harness::new(1);
    let stub = stub_server(h.path(), "fp-env", StubOpts::default());
    h.write_config(&declaration(&stub, true, r#"env = { TOKEN = "first" }"#));
    h.supervisor.reconcile_all().await;
    let before = h.record("srv").expect("record");

    h.write_config(&declaration(&stub, true, r#"env = { TOKEN = "second" }"#));
    h.supervisor.reconcile_all().await;

    let after = h.record("srv").expect("record");
    assert_eq!(after.generation, before.generation, "no reload was triggered");
    assert_eq!(after.config_fingerprint, before.config_fingerprint);

    // And the fingerprint of the *new* declaration is the same number, which is
    // the property rather than the consequence.
    let parsed = McpConfig::load(&h.config_path).expect("parse");
    assert_eq!(
        Some(config_fingerprint(parsed.servers.get("srv").expect("srv"))),
        after.config_fingerprint
    );

    h.supervisor.shutdown_all().await;
}

/// **Fix round 1, finding 1.** A route `enable` must leave the next reconcile
/// nothing to do for a declaration nobody touched.
///
/// Two things make that true, and this test fails without either: the `enabled`
/// bit is **not** in the fingerprint preimage (§3.3 E2 lists the covered fields
/// and the bit is edge case 15's *other* diff-key half, driving a different
/// verb), and E2 stamps the declaration **on disk** rather than the pre-W
/// snapshot the supervisor happened to be holding. Otherwise the stored
/// fingerprint disagrees with the file for as long as the row lives, and the
/// next `mcp.toml` change — an edit to a *different* server — reads
/// `changed = true` for this one: on a `Failed` row a retry outside §3.4's
/// closed list of four, on an `Enabled` row a false
/// *"declaration changed; reload to apply"* that repeats forever.
#[tokio::test]
async fn a_route_enable_leaves_the_next_reconcile_nothing_to_retry() {
    let h = Harness::new(1);
    let other = stub_server(h.path(), "fp-other", StubOpts::default());
    let config = |srv_enabled: bool, other_extra: &str| {
        format!(
            r#"[defaults]
connect_timeout_secs = 5
request_timeout_secs = 3

[servers.srv]
transport = "stdio"
command = "/definitely/not/a/command/xyzzy"
enabled = {srv_enabled}

[servers.other]
transport = "stdio"
command = "{other}"
enabled = false
{other_extra}
"#,
            other = other.script.display()
        )
    };

    h.write_config(&config(false, ""));
    h.supervisor.reconcile_all().await;
    assert_eq!(h.state("srv"), Some(ExtensionState::Disabled));

    // The route the GUI drives: W flips the bit on disk, then E0–E5 cannot
    // connect — the `Failed` row §3.4 trigger 2 would retry on a real change.
    let row = h.supervisor.enable(&mcp("srv")).await.expect("enable");
    assert!(
        matches!(row.state, ExtensionState::Failed { .. }),
        "the declared command does not exist: {:?}",
        row.state
    );
    let after_enable = row.generation;

    let on_disk = McpConfig::load(&h.config_path).expect("parse");
    assert_eq!(
        h.record("srv").expect("record").config_fingerprint,
        Some(config_fingerprint(on_disk.servers.get("srv").expect("srv"))),
        "E2 stamps the declaration on disk, not the pre-W snapshot"
    );

    // A hand edit to the **other** server is what drives the next reconcile.
    h.write_config(&config(true, r#"args = ["--verbose"]"#));
    h.supervisor.reconcile_all().await;

    let after = h.record("srv").expect("record");
    assert_eq!(
        after.generation, after_enable,
        "nothing about srv changed, so §3.4's trigger 2 must not fire"
    );
    assert!(
        matches!(after.state, ExtensionState::Failed { .. }),
        "and the row is where the enable left it"
    );
    assert!(!other.any_alive(), "the untouched disabled server never started");
}

// ============================================================================
// An unparseable store
// ============================================================================

/// **§10 case 15's parse-failure branch.** Under the watcher the last-good
/// desired set is kept and the diff is **skipped**, so an editor's intermediate
/// save cannot tear down every running server; the pseudo-record is what makes
/// the breakage visible. Composing "a block that vanished → T5-gone" with the
/// boot rule would do the opposite.
#[tokio::test]
async fn an_unparseable_store_keeps_the_server_up_and_parks_a_pseudo_record() {
    let h = Harness::new(1);
    let stub = stub_server(h.path(), "unparseable", StubOpts::default());
    h.declare(&stub, true);
    h.supervisor.reconcile_all().await;
    let before = h.record("srv").expect("record");

    h.write_config("this is not = = toml [[[");
    h.supervisor.reconcile_all().await;

    assert_eq!(
        h.state("srv"),
        Some(ExtensionState::Enabled),
        "nothing running is torn down by a half-typed block"
    );
    assert!(stub.any_alive(), "the child is still up");
    assert!(h.registry.get("srv__echo").is_some(), "and still serving");
    let pseudo = h
        .record(CONFIG_PSEUDO_ID)
        .expect("the whole-file pseudo-record appears");
    assert!(
        matches!(
            pseudo.state,
            ExtensionState::Failed {
                reason: FailureReason::ConfigInvalid,
                ..
            }
        ),
        "with the parse error, got {:?}",
        pseudo.state
    );
    // Two different refusals, and the difference is exactly whose disposition
    // can be read (§4, §8):
    //
    //  * the **pseudo-record** has no bit anyone can read, so every verb on it
    //    is `409 store_unreadable` with no transition;
    //  * a **live server** still has a readable bit — the last-good parse — so
    //    its `disable` is not refused up front; it runs W, W cannot write into
    //    a file that does not parse, and that is the `500` write-first
    //    promises, with nothing torn down.
    assert!(
        matches!(
            h.supervisor.disable(&mcp(CONFIG_PSEUDO_ID)).await,
            Err(ExtensionError::StoreUnreadable(_))
        ),
        "the pseudo-record row is 409 store_unreadable"
    );
    assert!(
        matches!(
            h.supervisor.disable(&mcp("srv")).await,
            Err(ExtensionError::WriteFailed(_))
        ),
        "a live server's disable is the write-first 500"
    );
    assert_eq!(
        h.state("srv"),
        Some(ExtensionState::Enabled),
        "and it took no CAS"
    );

    // Repair it — and the edit made in the same sitting lands.
    h.declare(&stub, false);
    h.supervisor.reconcile_all().await;

    assert!(
        h.record(CONFIG_PSEUDO_ID).is_none(),
        "the pseudo-record is dropped by the next reload that parses"
    );
    assert_eq!(
        h.state("srv"),
        Some(ExtensionState::Disabled),
        "and the diff applies against the last-good set"
    );
    assert_eq!(
        h.record("srv").expect("record").generation,
        before.generation,
        "a disable does not bump the generation"
    );
    assert!(eventually(Duration::from_secs(5), || !stub.any_alive()).await);
}

// ============================================================================
// §3.7 — a connected server changes its own tool set
// ============================================================================

/// Wait for the refresh the notification triggers to land.
async fn wait_for_tools(h: &Harness, expected: &[&str]) -> bool {
    eventually(Duration::from_secs(10), || {
        let mut live: Vec<String> = h
            .registry
            .registered_tool_names()
            .into_iter()
            .filter(|n| n.starts_with("srv__"))
            .collect();
        live.sort();
        let mut want: Vec<String> = expected.iter().map(|n| format!("srv__{n}")).collect();
        want.sort();
        live == want
    })
    .await
}

/// A server with the notifier running and one tool (`echo`).
async fn notifying_harness(tag: &str) -> (Harness, Stub) {
    let h = Harness::new(1);
    let stub = stub_server(
        h.path(),
        tag,
        StubOpts {
            notifier: true,
            ..StubOpts::default()
        },
    );
    h.declare(&stub, true);
    h.supervisor.reconcile_all().await;
    assert_eq!(h.state("srv"), Some(ExtensionState::Enabled));
    (h, stub)
}

/// **(1) Added.** The tool is registered under the **same** generation — same
/// incarnation, same client — and appears in the row.
#[tokio::test]
async fn a_tool_the_server_adds_is_registered_under_the_same_generation() {
    let (h, stub) = notifying_harness("add").await;
    let before = h.record("srv").expect("record");

    stub.set_tools(&["echo", "added"]);
    stub.notify();

    assert!(
        wait_for_tools(&h, &["echo", "added"]).await,
        "the added tool should be registered within request_timeout"
    );
    let after = h.record("srv").expect("record");
    assert_eq!(after.generation, before.generation, "no generation bump");
    assert_eq!(
        h.registry.get("srv__added").and_then(|t| t.incarnation()),
        Some(before.generation),
        "the new literal carries the current generation"
    );
    assert!(after.tools.contains(&"srv__added".to_string()), "and the row lists it");
    assert!(after.tools_changed_at.is_some(), "stamped");

    h.supervisor.shutdown_all().await;
}

/// **(2) Removed.** The S4 withdrawal path, T1 verbatim minus the state change:
/// the name stays retained and *flagged*, which is what both gate arms read to
/// refuse it with the server-withdrawn wording instead of an unattributed
/// not-found.
#[tokio::test]
async fn a_tool_the_server_drops_is_refused_on_both_arms_with_the_server_withdrawn_wording() {
    let (h, stub) = notifying_harness("remove").await;
    let snapshot = (*h.registry).clone();

    stub.set_tools(&["other"]);
    stub.notify();
    assert!(wait_for_tools(&h, &["other"]).await, "echo is withdrawn");

    // Miss arm: the live registry no longer has the entry.
    let live = h
        .registry
        .execute("srv__echo", &serde_json::json!({}))
        .await
        .expect_err("refused");
    assert!(
        live.contains("was withdrawn by 'srv' itself, which is still enabled"),
        "expected the server-withdrawn row, got: {live}"
    );
    // Hit arm: a pre-change snapshot still holds the entry, at the *current*
    // generation, so state and generation both pass and only the flag refuses.
    let held = snapshot
        .execute("srv__echo", &serde_json::json!({}))
        .await
        .expect_err("refused");
    assert!(
        held.contains("was withdrawn by 'srv' itself, which is still enabled"),
        "expected the same row on the hit arm, got: {held}"
    );

    let row = h.record("srv").expect("record");
    assert_eq!(
        row.withdrawn_by_server,
        vec!["srv__echo".to_string()],
        "the row lists it under withdrawn_by_server"
    );
    assert!(
        row.tools.contains(&"srv__echo".to_string()),
        "the *retained* set keeps it — that is what attributes the refusal"
    );
    assert!(
        !row.live_tools().contains(&"srv__echo".to_string()),
        "but the row's live tools do not advertise a name the gate refuses"
    );
    assert_eq!(row.live_tools(), vec!["srv__other".to_string()]);
    assert_eq!(
        h.ledger().recorded_providers("srv__echo"),
        vec![mcp("srv")],
        "its capability is tombstoned under the server's id"
    );
    assert_eq!(row.state, ExtensionState::Enabled, "the server is still enabled");

    h.supervisor.shutdown_all().await;
}

/// **(3) A removal and an addition in one change.** The tombstone index is
/// cleared **per capability**: a whole-extension `restore` would erase the
/// tombstone step 5 just wrote for the tool removed in the same change.
#[tokio::test]
async fn a_removal_and_an_addition_in_one_change_keep_their_own_tombstones() {
    let (h, stub) = notifying_harness("swap").await;

    stub.set_tools(&["fresh"]);
    stub.notify();
    assert!(wait_for_tools(&h, &["fresh"]).await);

    assert_eq!(
        h.ledger().recorded_providers("srv__echo"),
        vec![mcp("srv")],
        "the removed tool's capability stays tombstoned"
    );
    assert!(
        h.ledger().recorded_providers("srv__fresh").is_empty(),
        "the added tool's capability is live, not tombstoned"
    );

    h.supervisor.shutdown_all().await;
}

/// **(4) Superseded.** A notification emitted just before a disable changes
/// nothing: step 3 takes the mutex the disable holds and then fails the
/// `Enabled` re-check. There is no path by which a non-`Enabled` server can
/// change the registry.
///
/// The hand-off is **driven, not raced**. Firing the notifier and disabling
/// straight after asserts only the outcome: the notifier polls on a 50 ms tick,
/// so the refresh may never reach step 3 at all — its receiver ends at T4
/// first — and "nothing changed" then passes for the wrong reason. Here the
/// test holds the per-extension mutex itself, so the refresh fetches against
/// the still-live child and queues on step 3's `lock_for` while the whole
/// disable runs underneath it; the returned [`ToolListRefresh`] names the
/// branch that actually ran.
#[tokio::test]
async fn a_notification_that_arrives_around_a_disable_is_superseded() {
    let (h, stub) = notifying_harness("superseded-list").await;
    let ext = mcp("srv");
    let generation = h.record("srv").expect("record").generation;
    // The incarnation's own client — the one its reader task would hand to the
    // refresh, taken from the map so this is the real hand-off.
    let client = h
        .supervisor
        .handles
        .lock_or_recover()
        .get("srv")
        .map(|handle| Arc::clone(&handle.client))
        .expect("a live handle");

    stub.set_tools(&["echo", "late"]);

    let guard = h.supervisor.lock_for("srv").await;
    let sup = Arc::clone(&h.supervisor);
    let refreshing =
        tokio::spawn(async move { sup.on_tool_list_changed(&mcp("srv"), generation, &client).await });
    // Long enough for one line of `sh` over a pipe. If the fetch had not landed
    // the refresh would report `FetchFailed` and this test would fail loudly
    // rather than pass without exercising step 3.
    tokio::time::sleep(Duration::from_millis(500)).await;

    // The disable, verbatim from the verb, under the hold the test already has.
    h.supervisor.write_bit("srv", false).expect("W");
    h.supervisor.ledger.set_disposition(&ext, false);
    let warnings = h
        .supervisor
        .run_disable("srv", WithdrawalCause::Disable)
        .await;
    assert!(warnings.is_empty(), "a clean teardown: {warnings:?}");
    drop(guard);

    assert_eq!(
        refreshing.await.expect("the refresh task"),
        ToolListRefresh::Superseded,
        "step 3's re-check is the branch this test is named for"
    );
    assert_eq!(h.state("srv"), Some(ExtensionState::Disabled));
    assert!(
        h.registry
            .registered_tool_names()
            .iter()
            .all(|n| !n.starts_with("srv__")),
        "after T5 the registry holds nothing from the server"
    );
    assert!(h.registry.get("srv__late").is_none(), "least of all a new tool");
    assert_eq!(
        h.record("srv").expect("record").tools_changed_at,
        None,
        "and a superseded refresh stamps nothing"
    );
}

/// **(5) A failed refresh keeps the recorded set.** A transient error must not
/// unpublish a working server — and a JSON-RPC error is not one
/// `classify_call_failure` maps, so the row does not move either.
#[tokio::test]
async fn a_refresh_whose_fetch_fails_leaves_the_set_and_the_row_unchanged() {
    let (h, stub) = notifying_harness("broken").await;
    let before = h.record("srv").expect("record");

    stub.break_next_list();
    stub.set_tools(&["would-have-been-added"]);
    stub.notify();
    tokio::time::sleep(Duration::from_millis(600)).await;

    let after = h.record("srv").expect("record");
    assert_eq!(after.state, ExtensionState::Enabled, "the row does not move");
    assert_eq!(after.tools, before.tools, "the recorded set is kept");
    assert_eq!(after.tools_changed_at, None, "and nothing is stamped");
    assert!(h.registry.get("srv__echo").is_some(), "the working tool still serves");

    h.supervisor.shutdown_all().await;
}

/// **(6) The reason the fetch runs outside the mutex.** A server that hangs on
/// `tools/list` holds only the transport; a `disable` issued during the fetch
/// completes W and T0 **without waiting for it**. Under rev 6's design — the
/// fetch under the per-extension mutex — the owner's switch would have hung
/// while calls kept succeeding.
#[tokio::test]
async fn a_server_hanging_on_tools_list_does_not_hold_up_a_disable() {
    let (h, stub) = notifying_harness("hang").await;

    stub.hang_next_list();
    stub.notify();
    // Let the refresh get as far as the (hanging) fetch.
    tokio::time::sleep(Duration::from_millis(300)).await;

    let supervisor = Arc::clone(&h.supervisor);
    let disable = tokio::spawn(async move { supervisor.disable(&mcp("srv")).await });

    // W and T0 are the first two steps and neither waits on the fetch.
    assert!(
        eventually(Duration::from_secs(2), || matches!(
            h.state("srv"),
            Some(ExtensionState::Disabling) | Some(ExtensionState::Disabled)
        ))
        .await,
        "the gate must flip while the fetch is still hanging"
    );

    let row = disable.await.expect("task").expect("disable");
    assert_eq!(row.state, ExtensionState::Disabled, "and T1–T5 complete normally");
    assert!(eventually(Duration::from_secs(10), || !stub.any_alive()).await);
}

/// **(7) Remove, then re-add across two notifications.** The step-4 diff base
/// is the **live subset** (`retained \ server_withdrawn`), so a re-added name
/// lands in `added` and goes through step 6. Diffed against the whole retained
/// set it would fall into `kept`, never be re-registered, keep its tombstone,
/// and stay refused until a `reload`.
#[tokio::test]
async fn a_name_removed_then_re_added_comes_back_under_the_same_generation() {
    let (h, stub) = notifying_harness("readd").await;
    let generation = h.record("srv").expect("record").generation;

    stub.set_tools(&["other"]);
    stub.notify();
    assert!(wait_for_tools(&h, &["other"]).await, "echo is withdrawn");
    assert_eq!(
        h.record("srv").expect("record").withdrawn_by_server,
        vec!["srv__echo".to_string()]
    );

    stub.set_tools(&["other", "echo"]);
    stub.notify();
    assert!(wait_for_tools(&h, &["other", "echo"]).await, "echo comes back");

    let row = h.record("srv").expect("record");
    assert_eq!(row.generation, generation, "the same incarnation throughout");
    assert!(
        row.withdrawn_by_server.is_empty(),
        "and it is no longer flagged"
    );
    assert_eq!(
        h.registry.get("srv__echo").and_then(|t| t.incarnation()),
        Some(generation)
    );
    assert!(
        h.ledger().recorded_providers("srv__echo").is_empty(),
        "its tombstone is restored per capability"
    );
    assert_eq!(
        h.registry
            .execute("srv__echo", &serde_json::json!({}))
            .await
            .expect("a call to the re-added tool succeeds"),
        "ok"
    );

    h.supervisor.shutdown_all().await;
}

// ============================================================================
// The event T5 has to emit
// ============================================================================

/// C2 is the first commit with a transition to announce, so this is where the
/// `ExtensionStateChanged` frame starts being produced. Nothing is ever
/// *rendered* from the payload — events only invalidate (§8, X-18) — but the
/// state word and the generation have to be right or the event log is a lie.
#[tokio::test]
async fn every_transition_announces_itself_with_its_state_word_and_generation() {
    let h = Harness::new(1);
    let mut events = h.bus.subscribe();
    let stub = stub_server(h.path(), "events", StubOpts::default());
    h.declare(&stub, true);

    h.supervisor.reconcile_all().await;
    h.supervisor.disable(&mcp("srv")).await.expect("disable");
    h.write_config("[defaults]\nconnect_timeout_secs = 10\n");
    h.supervisor.reconcile_all().await;

    let mut seen: Vec<(String, u64)> = Vec::new();
    while let Ok(event) = events.try_recv() {
        if let openalpaca_core::events::SystemEvent::ExtensionStateChanged {
            extension,
            state,
            generation,
            tools_changed,
            ..
        } = event
        {
            assert_eq!(extension, mcp("srv"));
            assert!(!tools_changed, "only a §3.7 refresh sets that flag");
            seen.push((state, generation));
        }
    }

    assert_eq!(
        seen,
        vec![
            ("enabled".to_string(), 1),
            ("disabled".to_string(), 1),
            // T5-gone emits the literal `removed`: the row disappears rather
            // than moving to a state.
            ("removed".to_string(), 1),
        ],
        "one transition, one announcement"
    );
}

// ============================================================================
// E-FAIL — a bring-up that fails *after* connect
// ============================================================================

/// **The S2 hole rev 10 closed.** E2's "nothing to unwind" holds only when
/// `connect`/`spawn` itself failed. A handshake that succeeded and a
/// `tools/list` that then failed leaves a **live, unsealed client and a live
/// child** — and a later `disable` from `Failed`, which §4.1 allows, would run
/// T4 on nothing while that child outlived the switch.
#[tokio::test]
async fn a_bring_up_that_fails_after_the_handshake_tears_its_own_handle_down() {
    let h = Harness::new(1);
    let stub = stub_server(h.path(), "efail", StubOpts::default());
    // The handshake will succeed; `tools/list` will answer a JSON-RPC error.
    stub.break_next_list();
    h.declare(&stub, true);

    h.supervisor.reconcile_all().await;

    assert!(
        matches!(h.state("srv"), Some(ExtensionState::Failed { .. })),
        "the row reads failed, got {:?}",
        h.state("srv")
    );
    assert_eq!(stub.spawn_count(), 1, "the handshake really did happen");
    assert!(
        eventually(Duration::from_secs(5), || !stub.any_alive()).await,
        "and E-FAIL tore the just-built handle down"
    );
    assert!(
        eventually(Duration::from_secs(5), || h.supervisor.readers_running() == 0).await,
        "disconnect takes changes_tx, so the reader's receiver ends and it exits"
    );
    assert!(
        h.record("srv").expect("record").disposition.0,
        "the bit stays true: the owner asked for it on"
    );

    // `disable` from that row finds nothing to tear down and still writes.
    let row = h.supervisor.disable(&mcp("srv")).await.expect("disable");
    assert_eq!(row.state, ExtensionState::Disabled);
    assert!(!row.disposition.0);
    assert!(row.warnings.is_empty(), "nothing was in flight, nothing detached");
    assert!(
        std::fs::read_to_string(&h.config_path)
            .expect("read store")
            .contains("enabled = false"),
        "and the bit is on disk"
    );
    assert_eq!(stub.spawn_count(), 1, "no respawn anywhere along that path");
}

// ============================================================================
// C4 — T1 step 3: the dependent scan (§3.2 T1, §7.3)
// ============================================================================

const CAP_SKILL: &str = "---
id: triage
name: Triage
description: Triage inbound work
invoke:
  mode: auto
requires_capabilities:
  - srv__echo
---
Body.
";

const CRON_SKILL: &str = "---
id: nightly
name: Nightly
description: Runs unattended
invoke:
  mode: scheduled
  cron: \"0 3 * * *\"
requires_capabilities:
  - srv__echo
---
Body.
";

const LEGACY_SKILL: &str = "---
id: legacy-echoer
name: Legacy Echoer
description: Resolves by tool name, not capability
invoke:
  mode: manual
tools:
  allow:
    - srv__echo
---
Body.
";

/// Every `ExtensionCapabilityWithdrawn` frame the bus holds.
#[allow(clippy::type_complexity)]
fn withdrawn_frames(
    rx: &mut tokio::sync::broadcast::Receiver<openalpaca_core::events::SystemEvent>,
) -> Vec<(ExtensionId, ExtensionState, WithdrawalCause, Vec<String>, Vec<String>, Vec<String>, String)>
{
    let mut out = Vec::new();
    while let Ok(event) = rx.try_recv() {
        if let openalpaca_core::events::SystemEvent::ExtensionCapabilityWithdrawn {
            extension,
            state,
            cause,
            affected_templates,
            affected_skills,
            affected_cron_skills,
            notice_lane,
            ..
        } = event
        {
            out.push((
                extension,
                state,
                cause,
                affected_templates,
                affected_skills,
                affected_cron_skills,
                notice_lane,
            ));
        }
    }
    out
}

/// **The transition the owner is looking at.** One `ExtensionCapabilityWithdrawn`
/// per disable, naming every template and skill that just stopped resolving —
/// including a legacy `tools.allow` skill whose only allowed name was withdrawn.
#[tokio::test]
async fn a_disable_announces_its_dependents_exactly_once() {
    let h = Harness::new(1);
    let stub = stub_server(h.path(), "dependents", StubOpts::default());
    h.declare(&stub, true);
    h.declare_template("echo_agent", &["srv__echo"]);
    h.declare_template("unrelated_agent", &["file_write"]);
    h.declare_skill("triage", CAP_SKILL);
    h.declare_skill("nightly", CRON_SKILL);
    h.declare_skill("legacy-echoer", LEGACY_SKILL);

    h.supervisor.reconcile_all().await;
    assert_eq!(h.state("srv"), Some(ExtensionState::Enabled));

    let mut events = h.bus.subscribe();
    h.supervisor.disable(&mcp("srv")).await.expect("disable");

    let frames = withdrawn_frames(&mut events);
    assert_eq!(frames.len(), 1, "one transition, one announcement: {frames:?}");
    let (extension, state, cause, templates, skills, cron, lane) = &frames[0];
    assert_eq!(*extension, mcp("srv"));
    assert_eq!(*state, ExtensionState::Disabling, "T1 runs inside the window");
    assert_eq!(*cause, WithdrawalCause::Disable);
    assert_eq!(templates, &vec!["echo_agent".to_string()]);
    assert_eq!(
        skills,
        &vec![
            "legacy-echoer".to_string(),
            "nightly".to_string(),
            "triage".to_string(),
        ],
        "the legacy `tools.allow` skill is named too — its allowed name was withdrawn"
    );
    assert_eq!(cron, &vec!["nightly".to_string()]);
    assert_eq!(lane, NOTICE_LANE);

    // Idempotence: a second disable withdraws nothing, so it announces nothing.
    let mut events = h.bus.subscribe();
    h.supervisor.disable(&mcp("srv")).await.expect("redundant disable");
    assert!(withdrawn_frames(&mut events).is_empty());
}

/// **The list-change dependent event, moved here from C2** (§3.7 step 5): a
/// server-driven removal names its dependent template in exactly one
/// `ExtensionCapabilityWithdrawn { cause: ServerListChange, state: Enabled }` —
/// the owner did not do this, and the wording must not say *"disabled"*.
#[tokio::test]
async fn a_server_driven_removal_names_a_dependent_template_in_one_event() {
    let (h, stub) = notifying_harness("listchange-dependents").await;
    h.declare_template("echo_agent", &["srv__echo"]);
    h.declare_skill("nightly", CRON_SKILL);

    let mut events = h.bus.subscribe();
    stub.set_tools(&["other"]);
    stub.notify();
    assert!(wait_for_tools(&h, &["other"]).await, "echo is withdrawn");

    let frames = withdrawn_frames(&mut events);
    assert_eq!(frames.len(), 1, "one change, one announcement: {frames:?}");
    let (extension, state, cause, templates, skills, cron, _) = &frames[0];
    assert_eq!(*extension, mcp("srv"));
    assert_eq!(
        *state,
        ExtensionState::Enabled,
        "the server is still enabled — only the tool went"
    );
    assert_eq!(*cause, WithdrawalCause::ServerListChange);
    assert_eq!(templates, &vec!["echo_agent".to_string()]);
    assert_eq!(skills, &vec!["nightly".to_string()]);
    assert_eq!(
        cron,
        &vec!["nightly".to_string()],
        "a cron skill that lost its only tool to the server is the same \
         unattended failure as one that lost it to a toggle"
    );
    assert_eq!(
        WithdrawalCause::ServerListChange.wording(&mcp("srv"), ""),
        "withdrawn by the server 'srv' (still enabled)"
    );

    h.supervisor.shutdown_all().await;
}

/// **§3.4.1's suppression**, implemented as the design's option (a): a reload
/// publishes its scan **once**, after the outcome is known, and empties
/// `affected_cron_skills` when it ended `Enabled` — so the dispatcher's rule
/// stays "post when `affected_cron_skills` is non-empty", with no cause
/// special case.
#[tokio::test]
async fn a_reload_that_ends_enabled_fires_no_cron_notice() {
    let h = Harness::new(1);
    let stub = stub_server(h.path(), "reload-notice", StubOpts::default());
    h.declare(&stub, true);
    h.declare_skill("nightly", CRON_SKILL);
    h.supervisor.reconcile_all().await;

    let mut events = h.bus.subscribe();
    h.supervisor.reload(&mcp("srv")).await.expect("reload");
    assert_eq!(h.state("srv"), Some(ExtensionState::Enabled));

    let frames = withdrawn_frames(&mut events);
    assert_eq!(frames.len(), 1, "one transition, one announcement: {frames:?}");
    let (_, state, cause, _, skills, cron, _) = &frames[0];
    assert_eq!(*cause, WithdrawalCause::Reload);
    assert_eq!(*state, ExtensionState::Enabled, "the outcome, not the transient");
    assert_eq!(skills, &vec!["nightly".to_string()], "the scan still ran");
    assert!(
        cron.is_empty(),
        "but a reload that ended `Enabled` did not take the capability away"
    );

    h.supervisor.shutdown_all().await;
}

/// The other half of §3.4.1: a reload that ends `Failed` **does** notify.
#[tokio::test]
async fn a_reload_that_ends_failed_fires_the_cron_notice() {
    let h = Harness::new(1);
    let stub = stub_server(h.path(), "reload-fail", StubOpts::default());
    h.declare(&stub, true);
    h.declare_skill("nightly", CRON_SKILL);
    h.supervisor.reconcile_all().await;

    // The edit the reload will read: a command that cannot start.
    std::fs::remove_file(&stub.script).expect("remove the server script");

    let mut events = h.bus.subscribe();
    h.supervisor.reload(&mcp("srv")).await.expect("reload");
    assert!(
        matches!(h.state("srv"), Some(ExtensionState::Failed { .. })),
        "got {:?}",
        h.state("srv")
    );

    let frames = withdrawn_frames(&mut events);
    assert_eq!(frames.len(), 1, "one transition, one announcement: {frames:?}");
    let (_, state, cause, _, _, cron, _) = &frames[0];
    assert_eq!(*cause, WithdrawalCause::Reload);
    assert!(matches!(state, ExtensionState::Failed { .. }));
    assert_eq!(
        cron,
        &vec!["nightly".to_string()],
        "the reload did take the capability away, so the owner is told"
    );
}

/// **The notice reaches the default lane.** The one failure mode with no human
/// in the loop: a cron skill that just lost its only tool.
///
/// Not `SystemEvent::WorkflowProgress`, which rev 1 named: `handle_progress`
/// dispatches only to `:telegram` / `:imessage` / `:discord` lane keys, so the
/// default `:gui` lane falls through all three branches and the notice would
/// have been a silent no-op for the default user. This asserts the replacement
/// end to end — supervisor → bus → `NotificationDispatcher` → conversation row,
/// and supervisor → bus → `event_bridge` → `ServerEvent`.
#[tokio::test]
async fn a_disable_with_a_cron_dependent_writes_one_notice_to_the_default_lane() {
    use openalpaca_api::events::ServerEvent;
    use openalpaca_storage::{ConversationRepository, Database};

    let h = Harness::new(1);
    let one = stub_server(h.path(), "notice-one", StubOpts::default());
    let two = stub_server(h.path(), "notice-two", StubOpts::default());
    // Two servers; only `srv` provides the cron skill's capability.
    h.write_config(&format!(
        r#"[defaults]
connect_timeout_secs = 10
request_timeout_secs = 3
max_reconnect_attempts = 3
reconnect_backoff_ms = 20

[servers.srv]
transport = "stdio"
command = "{one}"
enabled = true

[servers.other]
transport = "stdio"
command = "{two}"
enabled = true
"#,
        one = one.script.display(),
        two = two.script.display(),
    ));
    h.declare_skill("nightly", CRON_SKILL);
    h.supervisor.reconcile_all().await;
    assert_eq!(h.state("srv"), Some(ExtensionState::Enabled));
    assert_eq!(h.state("other"), Some(ExtensionState::Enabled));

    // The two consumers `main.rs` wires onto the same bus.
    let db_dir = tempfile::tempdir().expect("db dir");
    let db = Database::open(&db_dir.path().join("notice.db")).expect("db");
    let cancel = tokio_util::sync::CancellationToken::new();
    let broadcaster =
        crate::events::EventBroadcaster::new(64, "instance-1".to_string(), Some(db.clone()));
    let mut server_events = broadcaster.subscribe();
    crate::event_bridge::spawn_event_bridge(broadcaster, &h.bus, None, cancel.clone());
    tokio::spawn(
        crate::notification::NotificationDispatcher::new(
            h.bus.subscribe(),
            db.clone(),
            cancel.clone(),
            None,
        )
        .run(),
    );

    let conversations = ConversationRepository::new(&db);
    let rows = |repo: &ConversationRepository| {
        repo.list_by_lane(NOTICE_LANE, 50, 0).expect("list_by_lane")
    };
    assert!(rows(&conversations).is_empty(), "nothing before the toggle");

    h.supervisor.disable(&mcp("srv")).await.expect("disable");

    assert!(
        eventually(Duration::from_secs(5), || !rows(&conversations).is_empty()).await,
        "the notice must reach the default lane"
    );
    let written = rows(&conversations);
    assert_eq!(written.len(), 1, "exactly one row: {written:?}");
    assert_eq!(
        written[0].role, "assistant",
        "`role` is hardcoded `assistant`, which is why the GUI transcript renders it"
    );
    assert_eq!(
        written[0].source.as_deref(),
        Some("gui"),
        "the lane's own source — a `system`-sourced default lane would be wrong forever after"
    );
    assert!(
        written[0].content.contains("nightly"),
        "it names the skill: {}",
        written[0].content
    );
    assert!(
        written[0].content.contains("disabled"),
        "and is worded from the cause: {}",
        written[0].content
    );

    // Exactly one `ServerEvent` peer, carrying `ts` and `instance_id`.
    let mut withdrawn = Vec::new();
    while let Ok(event) = server_events.try_recv() {
        if let ServerEvent::ExtensionCapabilityWithdrawn {
            id,
            cause,
            affected_cron_skills,
            instance_id,
            ..
        } = event
        {
            withdrawn.push((id, cause, affected_cron_skills, instance_id));
        }
    }
    assert_eq!(withdrawn.len(), 1, "one broadcast: {withdrawn:?}");
    assert_eq!(withdrawn[0].0, "srv");
    assert_eq!(withdrawn[0].1, "disable");
    assert_eq!(withdrawn[0].2, vec!["nightly".to_string()]);
    assert_eq!(withdrawn[0].3, "instance-1");

    // A second disable of an **unrelated** extension has no cron dependent, so
    // it announces on the bus but writes nothing to the lane.
    h.supervisor
        .disable(&mcp("other"))
        .await
        .expect("disable the unrelated server");
    assert!(
        eventually(Duration::from_secs(2), || h.state("other")
            == Some(ExtensionState::Disabled))
        .await
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        rows(&conversations).len(),
        1,
        "S4 is about withdrawn capabilities, not announcing inventory"
    );

    cancel.cancel();
    h.supervisor.shutdown_all().await;
}

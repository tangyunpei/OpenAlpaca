//! Tests for the plugin supervisor.
//!
//! They live beside `manager.rs` rather than inside it: the supervisor is the
//! biggest file in the crate and the tests are the larger half of it, so a
//! reviewer reading a change to T4 should not have to scroll past a stub
//! plugin's shell script to find it.

#[cfg(test)]
mod unit_tests {
    use crate::manager::*;

    /// Design §10 case 5: a plugin skill can never carry a cron expression.
    ///
    /// `scheduled_skills::sync_all` iterates the whole catalog, plugin entries
    /// included, so a mapped `invoke.cron` **would** register a wake job — and
    /// T2 could not withdraw it, because `PluginManager` holds no
    /// `WakeManager`. The withdrawn skill's job would then fire into an
    /// unattributed *"no longer in the catalog"* warn with no notice: exactly
    /// the unattended failure §7.3 says the log alone cannot cover.
    #[test]
    fn plugin_skill_frontmatter_never_carries_cron() {
        let info = serde_json::json!({
            "id": "digest",
            "name": "Daily digest",
            "invoke": { "mode": "auto", "slash": "/digest", "cron": "0 9 * * *" },
        });
        let fm = build_skill_frontmatter_from_info(&info, "notion");
        assert_eq!(fm.invoke.mode, "auto");
        assert_eq!(fm.invoke.slash.as_deref(), Some("/digest"));
        assert!(
            fm.invoke.cron.is_none(),
            "a plugin skill acquired a cron job T2 cannot withdraw"
        );
    }

    #[test]
    fn test_toml_to_json_primitives() {
        assert_eq!(
            toml_to_json(&toml::Value::String("hello".into())),
            Value::String("hello".into())
        );
        assert_eq!(
            toml_to_json(&toml::Value::Integer(42)),
            Value::Number(42.into())
        );
        assert_eq!(toml_to_json(&toml::Value::Boolean(true)), Value::Bool(true));
    }

    #[test]
    fn test_toml_to_json_nested() {
        let mut tbl = toml::map::Map::new();
        tbl.insert("key".into(), toml::Value::String("val".into()));
        tbl.insert(
            "arr".into(),
            toml::Value::Array(vec![toml::Value::Integer(1), toml::Value::Integer(2)]),
        );
        let json = toml_to_json(&toml::Value::Table(tbl));
        assert!(json.is_object());
        assert_eq!(json["key"], "val");
        assert_eq!(json["arr"], serde_json::json!([1, 2]));
    }

    /// R17: `error.data` now reaches the classifier, so §4.2's
    /// `NeedsAuthorization` arm is reachable and the row's `hint` is populated.
    ///
    /// It fires **only** on the plugin's own declared reason. Nothing is
    /// inferred from the message text — a misclassification here would put an
    /// "Authorize" button on a crash.
    #[test]
    fn a_declared_needs_authorization_classifies_and_carries_its_hint() {
        let (reason, hint) = classify_bringup(&PluginError::RpcError {
            code: -32001,
            message: "not authorized".into(),
            data: Some(serde_json::json!({
                "reason": "needs_authorization",
                "hint": "https://example.com/authorize",
            })),
        });
        assert_eq!(reason, FailureReason::NeedsAuthorization);
        assert!(reason.actionable(), "the GUI must render a CTA");
        assert_eq!(hint.as_deref(), Some("https://example.com/authorize"));

        // The hint is optional; the reason is not conditional on it.
        let (reason, hint) = classify_bringup(&PluginError::RpcError {
            code: -32001,
            message: "not authorized".into(),
            data: Some(serde_json::json!({"reason": "needs_authorization"})),
        });
        assert_eq!(reason, FailureReason::NeedsAuthorization);
        assert_eq!(hint, None);
    }

    /// Absent the signal a bring-up failure degrades to `Unreachable` — which
    /// is exactly what §4.2 says happens.
    #[test]
    fn an_rpc_error_without_the_signal_stays_unreachable() {
        for data in [
            None,
            Some(serde_json::json!({})),
            Some(serde_json::json!({"reason": "something else"})),
            // The words appear in the *message*, which is deliberately not read.
            Some(serde_json::json!({"detail": "needs_authorization"})),
        ] {
            let (reason, hint) = classify_bringup(&PluginError::RpcError {
                code: -1,
                message: "needs_authorization, surely".into(),
                data,
            });
            assert_eq!(reason, FailureReason::Unreachable);
            assert_eq!(hint, None);
        }
        assert_eq!(
            classify_bringup(&PluginError::ProcessCrashed).0,
            FailureReason::Crashed
        );
        assert_eq!(
            classify_bringup(&PluginError::MissingConfig(vec!["token".into()])).0,
            FailureReason::NeedsConfig {
                missing: vec!["token".into()]
            }
        );
    }
}

/// Supervisor tests that drive a real child process.
///
/// The committed stub plugin at `tests/fixtures/echo-plugin/` is what makes
/// them possible: it holds a process, a tool, a skill, an agent template and a
/// capability provider, so every teardown step has something to tear down.
#[cfg(test)]
mod lifecycle_tests {
    use crate::manager::*;
    use openalpaca_core::agent::registry::AgentRegistry;
    use openalpaca_core::bus::EventBus;
    use openalpaca_core::orchestrator::skill_catalog::SkillCatalog;
    use openalpaca_core::tools::ToolRegistry;
    use openalpaca_core::tools::extensions::{Consent, UnapprovedReason};

    /// The home-store sandbox is `permission_gate::tests`' — one lock for the
    /// whole crate, because both modules re-point `OPENALPACA_HOME_STORE` and a
    /// lock each would serialize only against itself.
    use crate::permission_gate::tests::HomeStoreGuard;

    /// What one plugin has live on the supervisor's surfaces right now.
    #[derive(Debug)]
    struct Registered {
        tools: Vec<String>,
        skills: Vec<String>,
        agents: Vec<String>,
        models: Vec<String>,
    }

    /// The committed stub: Content-Length-framed JSON-RPC over stdio, answers
    /// `tools/list` with one `echo` tool and every other method with an empty
    /// result — enough for `skill/info` and `agent/info` to register a skill
    /// and an agent template under the plugin's own name.
    fn stub_script() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/echo-plugin/echo-server.sh")
    }

    /// Lay out a plugin directory under `root` holding the stub script and a
    /// manifest with `extra` appended (types, virtual capabilities, …).
    ///
    /// `manifest_name` defaults to the directory name — the identity rule of
    /// design §2.2 (X-3) — and the two tests that need them to differ pass it
    /// explicitly through [`install_stub_plugin_named`].
    fn install_stub_plugin(root: &Path, name: &str, extra: &str) -> PathBuf {
        install_stub_plugin_named(root, name, name, extra)
    }

    fn install_stub_plugin_named(
        root: &Path,
        dir_name: &str,
        manifest_name: &str,
        extra: &str,
    ) -> PathBuf {
        let dir = root.join(dir_name);
        std::fs::create_dir_all(&dir).unwrap();
        // fs::copy carries the mode across on Unix, so the entry stays executable.
        std::fs::copy(stub_script(), dir.join("echo-server.sh")).unwrap();
        std::fs::write(
            dir.join("plugin.toml"),
            format!(
                r#"
[plugin]
name = "{manifest_name}"
version = "0.1.0"
entry = "./echo-server.sh"
mcp_compatible = true
{extra}
"#
            ),
        )
        .unwrap();
        dir
    }

    /// Make the stub's startup slow and countable.
    ///
    /// The entry becomes a wrapper that appends its own pid to `spawns.log`
    /// and sleeps before it `exec`s the stub (`exec` keeps the pid, so the
    /// logged number is the stub's). The log names *every* child the plugin
    /// ever started, which the manager's own bookkeeping cannot show.
    fn slow_the_entry(dir: &Path) {
        use std::os::unix::fs::PermissionsExt;

        let wrapper = dir.join("slow-entry.sh");
        // The child's cwd is its plugin directory (`PluginProcess::spawn`),
        // so both relative paths resolve there.
        std::fs::write(
            &wrapper,
            "#!/bin/sh\necho $$ >> spawns.log\nsleep 0.5\nexec ./echo-server.sh\n",
        )
        .unwrap();
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();

        let manifest = dir.join("plugin.toml");
        let text = std::fs::read_to_string(&manifest).unwrap();
        std::fs::write(
            &manifest,
            text.replace("./echo-server.sh", "./slow-entry.sh"),
        )
        .unwrap();
    }

    /// The pids of every child the plugin at `dir` has spawned (see
    /// `slow_the_entry`), oldest first.
    fn spawned_pids(dir: &Path) -> Vec<u32> {
        std::fs::read_to_string(dir.join("spawns.log"))
            .map(|log| log.lines().filter_map(|l| l.trim().parse().ok()).collect())
            .unwrap_or_default()
    }

    /// How many children the plugin at `dir` has spawned.
    fn spawn_count(dir: &Path) -> usize {
        spawned_pids(dir).len()
    }

    /// The default lane every harness scan carries on its event.
    const NOTICE_LANE: &str = "owner:gui";

    struct Harness {
        manager: Arc<PluginManager>,
        tools: Arc<ToolRegistry>,
        skills: Arc<SkillCatalog>,
        agents: Arc<AgentRegistry>,
        bus: EventBus,
        /// `None` for a harness that *shares* an already-sandboxed home store —
        /// the "restart the daemon over the same plugins root" shape. The guard
        /// holds a std mutex for its whole life, so a second one taken inside
        /// the same test would deadlock rather than nest.
        _home: Option<HomeStoreGuard>,
    }

    impl Harness {
        /// Every harness sandboxes the home store: `atomic_write_toml` rotates
        /// backups into `state/backups/`, which resolves through
        /// `OPENALPACA_HOME_STORE` on every call.
        fn new(root: &Path) -> Self {
            Self::build(root, Some(HomeStoreGuard::set(&root.join(".home"))))
        }

        /// A second supervisor over the same plugins root, as a restart is —
        /// fresh in-memory state, the same two files on disk.
        fn restart(root: &Path) -> Self {
            Self::build(root, None)
        }

        fn build(root: &Path, home: Option<HomeStoreGuard>) -> Self {
            let bus = EventBus::new(256);
            // The production shape from C4 (`services/tools.rs`): the ledger
            // holds the bus, so `mark_failed` announces its own `failed` and
            // T1 step 3 publishes `ExtensionCapabilityWithdrawn`.
            let tools = Arc::new(ToolRegistry::with_event_bus(bus.clone()).unwrap());
            let skills = Arc::new(SkillCatalog::new());
            let agents = Arc::new(AgentRegistry::new());
            let manager = Arc::new(
                PluginManager::new(
                    root.to_path_buf(),
                    Arc::clone(&tools),
                    Some(Arc::clone(&skills)),
                    Some(Arc::clone(&agents)),
                )
                .with_event_bus(bus.clone())
                .with_notice_lane(NOTICE_LANE),
            );
            Self {
                manager,
                tools,
                skills,
                agents,
                bus,
                _home: home,
            }
        }

        /// One plugin's **live** registrations, read straight from
        /// `PluginState` — what E4 published, as distinct from the ledger's
        /// retained (attribution) set, which survives a teardown on purpose.
        async fn registered(&self, name: &str) -> Registered {
            let plugins = self.manager.plugins.read().await;
            let state = plugins
                .get(name)
                .unwrap_or_else(|| panic!("plugin '{name}' is not tracked"));
            Registered {
                tools: state.registered_tools.clone(),
                skills: state.registered_skills.clone(),
                agents: state.registered_agents.clone(),
                models: state.registered_models.clone(),
            }
        }

        /// The extension row, as `GET /v1/extensions` will read it.
        async fn row(&self, name: &str) -> ExtensionRecord {
            self.manager
                .row(&ExtensionId::plugin(name.to_string()))
                .await
                .unwrap_or_else(|e| panic!("no row for '{name}': {e}"))
        }

        async fn state(&self, name: &str) -> ExtensionState {
            self.row(name).await.state
        }

        /// PID of the plugin's child process. Panics unless one is held.
        async fn child_pid(&self, name: &str) -> u32 {
            let plugins = self.manager.plugins.read().await;
            plugins
                .get(name)
                .and_then(|s| s.process.as_ref())
                .and_then(|p| p.child.id())
                .unwrap_or_else(|| panic!("plugin '{name}' holds no child process"))
        }

        async fn holds_process(&self, name: &str) -> bool {
            self.manager
                .plugins
                .read()
                .await
                .get(name)
                .is_some_and(|s| s.process.is_some())
        }

        /// The capability-provider handle the manager tracks for this plugin.
        async fn provider_handle(&self, name: &str) -> ProviderHandle {
            let plugins = self.manager.plugins.read().await;
            plugins
                .get(name)
                .and_then(|s| s.capability_provider_handle)
                .unwrap_or_else(|| panic!("plugin '{name}' registered no capability provider"))
        }

        /// How many providers currently emit the stub's virtual capability.
        /// `known_virtual_capabilities` does not de-duplicate, so a duplicate
        /// provider shows up as a second occurrence.
        fn stub_caps(&self) -> usize {
            self.tools
                .known_virtual_capabilities()
                .iter()
                .filter(|c| *c == "annotation:echo_stub")
                .count()
        }

        fn has_tool(&self, name: &str) -> bool {
            self.tools.registered_tool_names().iter().any(|n| n == name)
        }

        /// Drive the boot scan.
        async fn scan(&self) {
            self.manager.start().await.unwrap();
        }
    }

    /// Is `pid` still a live process? `sh -c "kill -0"` uses the shell builtin,
    /// so this needs no `/bin/kill` and no extra dependency.
    fn pid_alive(pid: u32) -> bool {
        std::process::Command::new("sh")
            .arg("-c")
            .arg(format!("kill -0 {pid} 2>/dev/null"))
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// Kill a plugin's child out of band and wait until the supervisor's
    /// read-side sweep has noticed.
    ///
    /// **Not** `kill -0`: the daemon holds the `Child` and does not `wait()` on
    /// a crash, so a SIGKILLed child is a *zombie* — it still exists, and
    /// `kill -0` still succeeds. The only honest observable is the one the
    /// design names: `try_wait` on the read path (design §3.6 item 3). (After a
    /// **teardown** the pid really is gone, because T4 awaits `child.wait()` —
    /// which is why `pid_alive` is the right assertion there and only there.)
    async fn kill_out_of_band_and_sweep(h: &Harness, name: &str) {
        let pid = h.child_pid(name).await;
        std::process::Command::new("sh")
            .arg("-c")
            .arg(format!("kill -9 {pid}"))
            .status()
            .unwrap();
        for _ in 0..200 {
            h.manager.sweep().await;
            if matches!(
                h.state(name).await,
                ExtensionState::Failed {
                    reason: FailureReason::Crashed,
                    ..
                }
            ) {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("the sweep never observed '{name}' exiting");
    }

    /// Approve the stub and bring it up through the scan, asserting it came up
    /// running with its tool registered and its child alive.
    async fn load_running_stub(h: &Harness, name: &str) -> Registered {
        load_running_stub_with_caps(h, name, &[]).await
    }

    /// The same, for a stub whose manifest declares `capabilities.provides`:
    /// consent has to be recorded against that list or E1's drift check parks
    /// it at `Unapproved{CapabilitiesGrew}` instead of loading it.
    async fn load_running_stub_with_caps(h: &Harness, name: &str, caps: &[&str]) -> Registered {
        let caps: Vec<String> = caps.iter().map(|c| c.to_string()).collect();
        h.manager
            .permission_gate
            .approve(name, &caps)
            .expect("approve the stub");
        h.scan().await;

        assert_eq!(
            h.state(name).await,
            ExtensionState::Enabled,
            "stub plugin failed to start"
        );
        let registered = h.registered(name).await;
        assert_eq!(registered.tools, vec![format!("{name}::echo")]);
        assert!(h.holds_process(name).await, "no child is held");
        registered
    }

    fn ext(name: &str) -> ExtensionId {
        ExtensionId::plugin(name.to_string())
    }

    // ── The binding cell, item by item ───────────────────────────────

    /// **Deny on a running plugin kills the child and unregisters
    /// tools/skills/templates** (design §4.1, §6.2 #8).
    #[tokio::test]
    async fn deny_unloads_the_plugin_and_kills_its_child() {
        let tmp = tempfile::tempdir().unwrap();
        install_stub_plugin(
            tmp.path(),
            "echo-test",
            "[types]\ntools = true\nskill = true\nagent = true\n",
        );
        let h = Harness::new(tmp.path());
        let loaded = load_running_stub(&h, "echo-test").await;
        assert_eq!(loaded.skills, vec!["echo-test".to_string()]);
        assert_eq!(loaded.agents, vec!["echo-test".to_string()]);
        let pid = h.child_pid("echo-test").await;

        h.manager.deny_plugin("echo-test").await.unwrap();

        // The consent decision is persisted, and it is a *consent* word.
        let row = h.row("echo-test").await;
        assert_eq!(row.consent, Some(Consent::Denied));
        assert_eq!(
            row.state,
            ExtensionState::Unapproved {
                reason: UnapprovedReason::Denied
            }
        );
        // The bit is untouched, so a later approve restores the toggle.
        assert!(row.disposition.0, "deny cleared the owner's toggle");

        // Nothing of the plugin is left on any surface.
        let live = h.registered("echo-test").await;
        assert!(live.tools.is_empty(), "tools survived deny: {:?}", live.tools);
        assert!(live.skills.is_empty(), "skills survived deny");
        assert!(live.agents.is_empty(), "agents survived deny");
        assert!(!h.has_tool("echo-test::echo"), "the tool is still registered");
        assert!(h.skills.get("echo-test").is_none(), "skill still catalogued");
        assert!(
            h.agents.get_template("echo-test").is_none(),
            "agent template still registered"
        );

        // And the child is gone, not merely forgotten: T4 awaited `wait()`.
        assert!(!pid_alive(pid), "plugin child {pid} outlived the denial");
        assert!(!h.holds_process("echo-test").await);
    }

    /// **W-deny write-first** (design §3.2): a denial that cannot be persisted
    /// tears nothing down. A half-applied deny would leave a plugin running
    /// that the next boot considers approved.
    #[tokio::test]
    async fn deny_that_cannot_be_persisted_changes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        install_stub_plugin(tmp.path(), "echo-test", "[types]\ntools = true\n");
        let h = Harness::new(tmp.path());
        load_running_stub(&h, "echo-test").await;
        let pid = h.child_pid("echo-test").await;

        // A directory cannot be overwritten by a file, so every write fails.
        let permissions = tmp.path().join(".permissions.toml");
        std::fs::remove_file(&permissions).unwrap();
        std::fs::create_dir(&permissions).unwrap();

        let err = h.manager.deny_plugin("echo-test").await.unwrap_err();
        assert!(
            matches!(err, ExtensionError::StoreUnreadable(_) | ExtensionError::WriteFailed(_)),
            "expected the failed write to surface, got {err:?}"
        );

        assert_eq!(h.state("echo-test").await, ExtensionState::Enabled);
        assert!(h.has_tool("echo-test::echo"));
        assert!(pid_alive(pid), "plugin child {pid} was killed anyway");
    }

    /// **Disable on an unapproved plugin** leaves `unapproved`, `enabled:
    /// false`, writes a **decision-less** entry, and a restart reads the same —
    /// `never_seen`, not `denied` (design §4, §5.1).
    #[tokio::test]
    async fn disabling_an_unapproved_plugin_writes_a_decision_less_entry() {
        let tmp = tempfile::tempdir().unwrap();
        install_stub_plugin(tmp.path(), "echo-test", "[types]\ntools = true\n");
        let h = Harness::new(tmp.path());
        h.scan().await;
        assert_eq!(
            h.state("echo-test").await,
            ExtensionState::Unapproved {
                reason: UnapprovedReason::NeverSeen
            }
        );

        let row = h.manager.disable(&ext("echo-test")).await.unwrap();
        assert_eq!(
            row.state,
            ExtensionState::Unapproved {
                reason: UnapprovedReason::NeverSeen
            },
            "disable turned a consent state into a toggle state"
        );
        assert!(!row.disposition.0);
        assert_eq!(row.consent, Some(Consent::Pending));

        // The entry on disk carries the bit and **no** decision.
        let raw = std::fs::read_to_string(tmp.path().join(".permissions.toml")).unwrap();
        assert!(raw.contains("enabled = false"), "{raw}");
        assert!(
            !raw.contains("approved"),
            "disable recorded a consent decision: {raw}"
        );

        // A restart reads the same two facts back.
        let restarted = Harness::restart(tmp.path());
        restarted.scan().await;
        let row = restarted.row("echo-test").await;
        assert_eq!(
            row.state,
            ExtensionState::Unapproved {
                reason: UnapprovedReason::NeverSeen
            },
            "the decision-less entry read back as a denial"
        );
        assert!(!row.disposition.0, "the pre-set bit did not survive");
        assert_eq!(spawn_count(tmp.path().join("echo-test").as_path()), 0);
    }

    /// **`stale_proxy_channel_closed_after_reenable_does_not_flip_row`**
    /// (design §3.0 Fact 3, §10 case 17).
    ///
    /// Hold a proxy from load N, disable, re-enable (load N+1), then call the
    /// old proxy: it returns the `Stale` refusal and the row stays `Enabled`
    /// with load N+1's process alive. Without the generation, that
    /// `ChannelClosed` would `mark_failed` the **healthy** incarnation and the
    /// reaper would kill it.
    #[tokio::test]
    async fn stale_proxy_channel_closed_after_reenable_does_not_flip_row() {
        let tmp = tempfile::tempdir().unwrap();
        install_stub_plugin(tmp.path(), "echo-test", "[types]\ntools = true\n");
        let h = Harness::new(tmp.path());
        load_running_stub(&h, "echo-test").await;

        // A deep registry snapshot, exactly as a lead agent takes one.
        let snapshot = (*h.tools).clone();
        let stale = snapshot.get("echo-test::echo").expect("the tool");
        let generation_n = h.manager.ledger.generation(&ext("echo-test")).unwrap();

        h.manager.disable(&ext("echo-test")).await.unwrap();
        h.manager.enable(&ext("echo-test")).await.unwrap();
        let generation_n1 = h.manager.ledger.generation(&ext("echo-test")).unwrap();
        assert!(generation_n1 > generation_n, "the load did not bump");
        let live_pid = h.child_pid("echo-test").await;

        // The stale proxy's own call: its channel's writer is gone.
        let ToolBackend::Plugin(executor) = &stale.backend else {
            panic!("the stub's tool is not plugin-backed")
        };
        let refusal = executor
            .execute("echo-test::echo", &serde_json::json!({}))
            .await
            .expect_err("a dead channel must not answer");
        assert!(
            refusal.contains("previous load"),
            "expected the Stale refusal, got: {refusal}"
        );

        // The row is untouched and load N+1 is still running.
        assert_eq!(h.state("echo-test").await, ExtensionState::Enabled);
        assert_eq!(h.manager.ledger.generation(&ext("echo-test")), Some(generation_n1));
        assert!(pid_alive(live_pid), "the healthy child was torn down");

        // And the gate refuses the same handle before it is ever reached.
        let gated = snapshot
            .execute("echo-test::echo", &serde_json::json!({}))
            .await
            .expect_err("the gate must refuse a stale handle");
        assert!(gated.contains("previous load"), "{gated}");
    }

    /// **Redundant enable registers no second capability provider** (design
    /// §3.3 E0). Enable on `Enabled` is a CAS failure, never a reload.
    #[tokio::test]
    async fn redundant_enable_registers_no_second_capability_provider() {
        let tmp = tempfile::tempdir().unwrap();
        install_stub_plugin(
            tmp.path(),
            "echo-test",
            "[types]\ntools = true\n\n[capabilities.virtual]\nprovides = [\"annotation:echo_stub\"]\n",
        );
        let h = Harness::new(tmp.path());
        load_running_stub(&h, "echo-test").await;

        let pid = h.child_pid("echo-test").await;
        let handle = h.provider_handle("echo-test").await;
        let providers = h.tools.provider_handles().len();
        let generation = h.manager.ledger.generation(&ext("echo-test")).unwrap();
        assert_eq!(h.stub_caps(), 1, "the stub's virtual cap is registered once");

        h.manager.enable(&ext("echo-test")).await.unwrap();

        assert_eq!(
            h.tools.provider_handles().len(),
            providers,
            "redundant enable registered a second capability provider"
        );
        assert_eq!(h.stub_caps(), 1, "the stub's virtual cap is duplicated");
        assert_eq!(h.provider_handle("echo-test").await, handle);
        assert_eq!(h.child_pid("echo-test").await, pid, "the child was restarted");
        assert_eq!(
            h.manager.ledger.generation(&ext("echo-test")),
            Some(generation),
            "the CAS failure bumped the generation"
        );

        // Decisive: a leaked provider would survive the teardown, because
        // nothing holds its handle any more.
        h.manager.disable(&ext("echo-test")).await.unwrap();
        assert_eq!(h.stub_caps(), 0, "a capability provider outlived the plugin");
    }

    /// **Manifest capability growth re-prompts** (design §3.3 E1, §10 case 12).
    ///
    /// The list recorded at approval time is read back for the first time; a
    /// manifest that has grown since parks at `Unapproved{CapabilitiesGrew}`
    /// with the **delta**, and the bit stays true.
    #[tokio::test]
    async fn capability_growth_reprompts_with_only_the_delta() {
        let tmp = tempfile::tempdir().unwrap();
        install_stub_plugin(
            tmp.path(),
            "echo-test",
            "[types]\ntools = true\n\n[capabilities]\nprovides = [\"net_read\"]\n",
        );
        let h = Harness::new(tmp.path());
        h.manager
            .permission_gate
            .approve("echo-test", &["net_read".to_string()])
            .unwrap();
        h.scan().await;
        assert_eq!(h.state("echo-test").await, ExtensionState::Enabled);
        h.manager.disable(&ext("echo-test")).await.unwrap();

        // The plugin updates itself and now asks for more.
        install_stub_plugin(
            tmp.path(),
            "echo-test",
            "[types]\ntools = true\n\n[capabilities]\nprovides = [\"net_read\", \"fs_write\"]\n",
        );
        let row = h.manager.enable(&ext("echo-test")).await.unwrap();

        assert_eq!(
            row.state,
            ExtensionState::Unapproved {
                reason: UnapprovedReason::CapabilitiesGrew {
                    added: vec!["fs_write".to_string()]
                }
            },
            "the re-prompt did not carry only the new capability"
        );
        assert!(row.disposition.0, "the owner's toggle was cleared");
        assert!(!h.holds_process("echo-test").await, "it started anyway");

        // A fresh approve records the wider list and starts it.
        let row = h.manager.approve_plugin("echo-test").await.unwrap();
        assert_eq!(row.state, ExtensionState::Enabled);
    }

    /// **A corrupt `.permissions.toml` loads nothing and overwrites nothing**
    /// (design §5.1, §10 case 10). Fail-closed on corruption, open on absence.
    #[tokio::test]
    async fn a_corrupt_permissions_store_loads_nothing_and_overwrites_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        install_stub_plugin(tmp.path(), "echo-test", "[types]\ntools = true\n");
        let h = Harness::new(tmp.path());
        h.manager.permission_gate.approve("echo-test", &[]).unwrap();

        let store = tmp.path().join(".permissions.toml");
        let garbage = "[echo-test\napproved = yes";
        std::fs::write(&store, garbage).unwrap();

        h.scan().await;

        let row = h.row("echo-test").await;
        assert!(
            matches!(
                row.state,
                ExtensionState::Failed {
                    reason: FailureReason::ConfigInvalid,
                    ..
                }
            ),
            "expected config_invalid, got {:?}",
            row.state
        );
        assert!(
            !row.disposition_readable,
            "a row over an unreadable store claimed to know its bit"
        );
        assert!(!h.has_tool("echo-test::echo"), "a plugin loaded anyway");
        assert_eq!(
            std::fs::read_to_string(&store).unwrap(),
            garbage,
            "the unreadable store was overwritten"
        );

        // And every verb refuses without a transition.
        for result in [
            h.manager.enable(&ext("echo-test")).await,
            h.manager.disable(&ext("echo-test")).await,
            h.manager.deny_plugin("echo-test").await,
            h.manager.approve_plugin("echo-test").await,
        ] {
            assert!(
                matches!(result, Err(ExtensionError::StoreUnreadable(_))),
                "a verb took a transition over an unreadable store: {result:?}"
            );
        }
        assert_eq!(std::fs::read_to_string(&store).unwrap(), garbage);
    }

    /// **A plugin skill with a mixed-case id is reachable by `/slash` and is
    /// removed on unload** (design §6.2 #14).
    ///
    /// Insert used to be verbatim while every reader lowercases, so the entry
    /// was unreachable by `/slash` *and* survived `remove` whenever the display
    /// name differed from the id — leaking an executor for a killed process.
    #[tokio::test]
    async fn a_mixed_case_plugin_skill_is_reachable_and_is_removed_on_unload() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = install_stub_plugin(tmp.path(), "echo-test", "[types]\ntools = true\nskill = true\n");
        // The stub answers `skill/info` from a file when one is present.
        std::fs::write(
            dir.join("skill-info.json"),
            r#"{"id":"MixedCase","name":"Echo Display Name","invoke":{"slash":"/Mixed"}}"#,
        )
        .unwrap();
        let h = Harness::new(tmp.path());
        load_running_stub(&h, "echo-test").await;

        // `/slash` resolves through the command index and then `get_by_id`,
        // which lowercases with no name fallback — the exact lookup that missed.
        assert!(
            h.skills.get_by_command("Mixed").is_some(),
            "the mixed-case skill is unreachable by /slash"
        );
        assert_eq!(
            h.registered("echo-test").await.skills,
            vec!["mixedcase".to_string()]
        );

        h.manager.disable(&ext("echo-test")).await.unwrap();
        assert!(
            h.skills.get_by_command("Mixed").is_none(),
            "a killed plugin's skill entry survived the unload"
        );
        assert!(
            h.skills.get("MixedCase").is_none(),
            "the entry itself survived, holding an executor for a dead process"
        );
    }

    /// **C5 — the tombstone** (design §10 case 5(a)). T2 scrubs the command and
    /// alias indices, so without one a `/slash` or `invoke_skill` for a
    /// withdrawn plugin skill reads as an unknown name. The tombstone survives
    /// the removal, names the plugin, and is dropped when the skill comes back.
    #[tokio::test]
    async fn t2_leaves_a_tombstone_naming_the_plugin_that_provided_the_skill() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = install_stub_plugin(tmp.path(), "echo-test", "[types]\ntools = true\nskill = true\n");
        std::fs::write(
            dir.join("skill-info.json"),
            r#"{"id":"triage","name":"Triage","invoke":{"slash":"/triage"}}"#,
        )
        .unwrap();
        let h = Harness::new(tmp.path());
        load_running_stub(&h, "echo-test").await;
        assert!(h.skills.get_by_command("triage").is_some());
        assert!(h.skills.tombstone("triage").is_none());

        h.manager.disable(&ext("echo-test")).await.unwrap();

        assert!(
            h.skills.get_by_command("triage").is_none(),
            "the live indices stay scrubbed exactly as today"
        );
        for key in ["triage", "/triage"] {
            let tomb = h
                .skills
                .tombstone(key)
                .unwrap_or_else(|| panic!("no tombstone under '{key}'"));
            assert_eq!(tomb.skill_id, "triage");
            assert_eq!(tomb.plugin_id, "echo-test");
        }

        // Re-enable: the skill is registered again and the tombstone goes.
        h.manager.enable(&ext("echo-test")).await.unwrap();
        assert!(h.skills.get_by_command("triage").is_some());
        assert!(
            h.skills.tombstone("triage").is_none(),
            "a skill that came back invalidates its own tombstone"
        );
    }

    /// The same for an agent template: `spawn_subagent` naming a withdrawn one
    /// is attributed to its plugin rather than reading as an unknown template.
    #[tokio::test]
    async fn t2_leaves_a_tombstone_for_a_withdrawn_agent_template() {
        let tmp = tempfile::tempdir().unwrap();
        install_stub_plugin(tmp.path(), "echo-test", "[types]\ntools = true\n");
        let h = Harness::new(tmp.path());
        load_running_stub(&h, "echo-test").await;

        // Stand in for E4's template registration, which needs `agent/info`.
        h.agents.register_template(agent_template("reader", &[]));
        assert!(h.agents.template_tombstone("reader").is_none());

        h.agents.remove_plugin_template("reader", "echo-test");
        assert!(h.agents.get_template("reader").is_none());
        assert_eq!(
            h.agents.template_tombstone("reader").as_deref(),
            Some("echo-test")
        );

        h.agents.register_template(agent_template("reader", &[]));
        assert!(
            h.agents.template_tombstone("reader").is_none(),
            "a template that came back invalidates its own tombstone"
        );
    }

    // ── C4 — T1 step 3: the dependent scan (§3.2 T1, §7.3) ───────────

    fn agent_template(id: &str, caps: &[&str]) -> openalpaca_core::agent::template::AgentTemplate {
        openalpaca_core::agent::template::AgentTemplate {
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
        }
    }

    #[allow(clippy::type_complexity)]
    fn withdrawn_frames(
        rx: &mut tokio::sync::broadcast::Receiver<openalpaca_core::events::SystemEvent>,
    ) -> Vec<(ExtensionId, ExtensionState, WithdrawalCause, Vec<String>, String)> {
        let mut out = Vec::new();
        while let Ok(event) = rx.try_recv() {
            if let openalpaca_core::events::SystemEvent::ExtensionCapabilityWithdrawn {
                extension,
                state,
                cause,
                affected_templates,
                notice_lane,
                ..
            } = event
            {
                out.push((extension, state, cause, affected_templates, notice_lane));
            }
        }
        out
    }

    /// **`deny` produces a scan worded *"denied"*, never *"disabled"*** (design
    /// §3.2 T1 step 3, §6.2 #8): the owner revoked *trust*, not a toggle, and
    /// the wording is keyed on the cause rather than on the transient state,
    /// which is `Disabling` for both verbs.
    #[tokio::test]
    async fn deny_announces_its_dependents_worded_denied_not_disabled() {
        let tmp = tempfile::tempdir().unwrap();
        install_stub_plugin(
            tmp.path(),
            "echo-test",
            "[types]\ntools = true\n\n[capabilities]\nprovides = [\"net_read\"]\n",
        );
        let h = Harness::new(tmp.path());
        load_running_stub_with_caps(&h, "echo-test", &["net_read"]).await;
        h.agents.register_template(agent_template("reader", &["net_read"]));
        h.agents.register_template(agent_template("writer", &["fs_write"]));

        let mut events = h.bus.subscribe();
        h.manager.deny_plugin("echo-test").await.unwrap();

        let frames = withdrawn_frames(&mut events);
        assert_eq!(frames.len(), 1, "one transition, one announcement: {frames:?}");
        let (extension, state, cause, templates, lane) = &frames[0];
        assert_eq!(*extension, ext("echo-test"));
        assert_eq!(*state, ExtensionState::Disabling, "T1 runs inside the window");
        assert_eq!(*cause, WithdrawalCause::Deny);
        assert_eq!(templates, &vec!["reader".to_string()]);
        assert_eq!(lane, NOTICE_LANE);

        // The wording the `warn!` and the owner notice render, keyed on the
        // cause the event carries — a consent word, never "disabled".
        assert_eq!(cause.wording(extension, ""), "denied");
        assert_ne!(cause.wording(extension, ""), "disabled");
    }

    /// The same scan under `disable` reads *"disabled"* — the two verbs differ
    /// only by the cause, and both pass through `Disabling`.
    #[tokio::test]
    async fn disable_announces_its_dependents_worded_disabled() {
        let tmp = tempfile::tempdir().unwrap();
        install_stub_plugin(
            tmp.path(),
            "echo-test",
            "[types]\ntools = true\n\n[capabilities]\nprovides = [\"net_read\"]\n",
        );
        let h = Harness::new(tmp.path());
        load_running_stub_with_caps(&h, "echo-test", &["net_read"]).await;
        h.agents.register_template(agent_template("reader", &["net_read"]));

        let mut events = h.bus.subscribe();
        h.manager.disable(&ext("echo-test")).await.unwrap();

        let frames = withdrawn_frames(&mut events);
        assert_eq!(frames.len(), 1, "{frames:?}");
        assert_eq!(frames[0].2, WithdrawalCause::Disable);
        assert_eq!(frames[0].3, vec!["reader".to_string()]);
        assert_eq!(frames[0].2.wording(&frames[0].0, ""), "disabled");
    }

    /// **T2 step 1's virtual capabilities are in the scanned set too** — T1's
    /// per-tool recording never sees them, so the scan cannot run until T2 has
    /// withdrawn them (design §3.2 T1 step 3, T2 step 1).
    #[tokio::test]
    async fn the_scan_covers_a_virtual_capability_only_template() {
        let tmp = tempfile::tempdir().unwrap();
        install_stub_plugin(
            tmp.path(),
            "echo-test",
            "[types]\ntools = true\n\n[capabilities.virtual]\nprovides = [\"annotation:echo_stub\"]\n",
        );
        let h = Harness::new(tmp.path());
        load_running_stub(&h, "echo-test").await;
        h.agents
            .register_template(agent_template("annotator", &["annotation:echo_stub"]));

        let mut events = h.bus.subscribe();
        h.manager.disable(&ext("echo-test")).await.unwrap();

        let frames = withdrawn_frames(&mut events);
        assert_eq!(frames.len(), 1, "{frames:?}");
        assert_eq!(
            frames[0].3,
            vec!["annotator".to_string()],
            "a scan run before T2 would have missed it entirely"
        );
    }

    /// **A template naming only a virtual capability classifies `withheld`,
    /// not `unknown`, after a disable** (design §3.2 T2 step 1).
    ///
    /// The provider's virtual list is separate from `capabilities.provides`, so
    /// T1's per-tool recording never sees it: without T2's tombstone the loss
    /// would be a `debug!` and nothing else.
    #[tokio::test]
    async fn a_virtual_capability_only_template_classifies_withheld_after_disable() {
        let tmp = tempfile::tempdir().unwrap();
        install_stub_plugin(
            tmp.path(),
            "echo-test",
            "[types]\ntools = true\n\n[capabilities.virtual]\nprovides = [\"annotation:echo_stub\"]\n",
        );
        let h = Harness::new(tmp.path());
        load_running_stub(&h, "echo-test").await;

        let caps = vec!["annotation:echo_stub".to_string()];
        assert!(
            !h.tools.resolve_capabilities(&caps, &[]).defs.is_empty(),
            "the virtual capability does not resolve while enabled"
        );

        h.manager.disable(&ext("echo-test")).await.unwrap();

        let resolution = h.tools.resolve_capabilities(&caps, &[]);
        assert!(resolution.defs.is_empty());
        let withheld: Vec<String> = resolution
            .withheld
            .iter()
            .map(|w| w.capability.clone())
            .collect();
        assert_eq!(
            withheld, caps,
            "a virtual-capability-only template classified `unknown`, not `withheld`"
        );
        assert!(
            resolution.withheld[0]
                .providers
                .iter()
                .any(|p| p.extension == ext("echo-test")),
            "the withholding is not attributed to the plugin"
        );
        assert!(resolution.unknown.is_empty());
    }

    /// **Kill the child out-of-band → the next `list()` reads `failed/crashed`
    /// and the next call refuses with attribution** (design §3.6 item 3).
    #[tokio::test]
    async fn an_out_of_band_kill_is_read_as_crashed_on_the_next_list() {
        let tmp = tempfile::tempdir().unwrap();
        install_stub_plugin(tmp.path(), "echo-test", "[types]\ntools = true\n");
        let h = Harness::new(tmp.path());
        load_running_stub(&h, "echo-test").await;

        // The sweep is on the read path — the row is correct whenever someone
        // looks at it, with no poller anywhere.
        kill_out_of_band_and_sweep(&h, "echo-test").await;
        let rows = ExtensionSupervisor::list(&*h.manager).await;
        let row = rows.iter().find(|r| r.id.name == "echo-test").unwrap();
        assert!(
            matches!(
                row.state,
                ExtensionState::Failed {
                    reason: FailureReason::Crashed,
                    ..
                }
            ),
            "expected failed/crashed, got {:?}",
            row.state
        );

        let refusal = h
            .tools
            .execute("echo-test::echo", &serde_json::json!({}))
            .await
            .expect_err("a crashed plugin's tool must refuse");
        assert!(
            refusal.contains("stopped unexpectedly"),
            "the refusal is unattributed: {refusal}"
        );
    }

    /// **Reaper superseded** (design §3.6, §3.3 E-PRE).
    ///
    /// `mark_failed` → `enable` (load N+1) **before** the reaper runs → reaper
    /// runs → load N+1's process is alive, its tools are registered, the row
    /// reads `enabled`, exactly one capability provider is registered, there is
    /// no `PluginState` residue, and load N's `child.wait()` was observed — no
    /// *"failed to kill"* line.
    #[tokio::test]
    async fn a_reap_that_lands_after_a_retry_is_superseded_and_touches_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        install_stub_plugin(
            tmp.path(),
            "echo-test",
            "[types]\ntools = true\n\n[capabilities.virtual]\nprovides = [\"annotation:echo_stub\"]\n",
        );
        let h = Harness::new(tmp.path());
        load_running_stub(&h, "echo-test").await;
        let gen_n = h.manager.ledger.generation(&ext("echo-test")).unwrap();
        let pid_n = h.child_pid("echo-test").await;
        let providers = h.tools.provider_handles().len();

        // The crash, without releasing the reaper: `reaper_rx` is parked until
        // `spawn_reaper`, so nothing drains it and the ordering is ours.
        h.manager.ledger.mark_failed(
            &ext("echo-test"),
            gen_n,
            FailureReason::Crashed,
            "killed for the test",
        );

        // Retry wins the mutex: E-PRE tears load N's residue down first.
        h.manager.enable(&ext("echo-test")).await.unwrap();
        let gen_n1 = h.manager.ledger.generation(&ext("echo-test")).unwrap();
        assert!(gen_n1 > gen_n);
        let pid_n1 = h.child_pid("echo-test").await;
        assert_ne!(pid_n, pid_n1, "E-PRE reused load N's child");
        assert!(
            !pid_alive(pid_n),
            "E-PRE left load N's child running (its `wait()` was not observed)"
        );
        assert_eq!(
            h.tools.provider_handles().len(),
            providers,
            "E-PRE left load N's capability provider registered"
        );
        assert_eq!(h.stub_caps(), 1);
        assert!(
            h.manager.ledger.audit(&h.tools).is_empty(),
            "a registered tool has no ledger record"
        );

        // Now the reaper, carrying load N's generation.
        h.manager.reap(&ext("echo-test"), gen_n).await;

        assert_eq!(h.state("echo-test").await, ExtensionState::Enabled);
        assert!(h.has_tool("echo-test::echo"), "the reaper unpublished load N+1");
        assert_eq!(h.child_pid("echo-test").await, pid_n1);
        assert!(pid_alive(pid_n1), "the reaper killed the live child");
        assert_eq!(h.stub_caps(), 1, "the reaper scrubbed load N+1's provider");
    }

    /// **Crash, then deny, then approve** (design §3.3.1, §3.2 W-deny).
    ///
    /// `mark_failed` → `deny` before the reaper → the reaper released → then
    /// `approve`: exactly one capability provider, no gen-N residue, and the
    /// T1-step-3 scan's inputs recorded under the plugin's id.
    ///
    /// The *cause* on that scan is asserted in C4, with the event: the ledger
    /// has no structure that records one today (`pending_cause` is written only
    /// by `begin(Disabling, cause)`, and this residue exit has no T0).
    #[tokio::test]
    async fn a_crash_then_a_deny_then_an_approve_leaves_exactly_one_load() {
        let tmp = tempfile::tempdir().unwrap();
        install_stub_plugin(
            tmp.path(),
            "echo-test",
            "[types]\ntools = true\n\n[capabilities.virtual]\nprovides = [\"annotation:echo_stub\"]\n",
        );
        let h = Harness::new(tmp.path());
        load_running_stub(&h, "echo-test").await;
        let gen_n = h.manager.ledger.generation(&ext("echo-test")).unwrap();
        let pid_n = h.child_pid("echo-test").await;
        let providers = h.tools.provider_handles().len();

        h.manager.ledger.mark_failed(
            &ext("echo-test"),
            gen_n,
            FailureReason::Crashed,
            "killed for the test",
        );

        // `deny` from a pre-reaper `Failed{Crashed}`: W-deny, then T1→T2→T4 on
        // the residue with no T0, then the plain T5-deny store.
        let row = h.manager.deny_plugin("echo-test").await.unwrap();
        assert_eq!(
            row.state,
            ExtensionState::Unapproved {
                reason: UnapprovedReason::Denied
            }
        );
        assert!(!pid_alive(pid_n), "deny carried the crash residue out");
        assert!(!h.has_tool("echo-test::echo"));
        assert_eq!(h.stub_caps(), 0, "the residue's provider survived deny");
        // T1 kept the retained name, which is what the dependent scan and the
        // gate's miss arm read (design §3.2 T1).
        assert_eq!(
            h.manager.ledger.tool_names(&ext("echo-test")),
            vec!["echo-test::echo".to_string()]
        );
        assert!(
            !h.manager
                .ledger
                .recorded_providers("annotation:echo_stub")
                .is_empty(),
            "T2 step 1 did not tombstone the virtual capability"
        );

        // The reaper arrives late and finds the state changed.
        h.manager.reap(&ext("echo-test"), gen_n).await;

        let row = h.manager.approve_plugin("echo-test").await.unwrap();
        assert_eq!(row.state, ExtensionState::Enabled);
        assert_eq!(
            h.tools.provider_handles().len(),
            providers,
            "a second capability provider is registered"
        );
        assert_eq!(h.stub_caps(), 1);
        assert!(h.manager.ledger.audit(&h.tools).is_empty());
        assert!(pid_alive(h.child_pid("echo-test").await));
    }

    /// **`plugin_supervisor_records_every_registered_tool`** (design §6.2a):
    /// after the scan, `audit()` is empty, so the fail-open path is unreachable
    /// for anything this supervisor loaded.
    #[tokio::test]
    async fn plugin_supervisor_records_every_registered_tool() {
        let tmp = tempfile::tempdir().unwrap();
        install_stub_plugin(tmp.path(), "echo-test", "[types]\ntools = true\n");
        let h = Harness::new(tmp.path());
        load_running_stub(&h, "echo-test").await;

        assert!(
            h.manager.ledger.audit(&h.tools).is_empty(),
            "a registered plugin tool has no ledger record"
        );
        // C1's review: the `author`-derived id and the ledger key must be the
        // same string, which is what X-3's name rule buys.
        let tool = h.tools.get("echo-test::echo").unwrap();
        assert_eq!(tool.extension_id(), Some(ext("echo-test")));
    }

    /// **S2 residue guard** (design §3.2 T2 step 4): after `disable` the row
    /// reads `connector: null, provider: null` and the plugin's recorded model
    /// set is empty.
    ///
    /// The models third is asserted against `PluginState.registered_models` —
    /// what E3 recorded and T2 clears — because that is the only place a plugin
    /// provider's models exist today: the provider bridge is not wired into
    /// `LlmRouter` yet, so a freshly built `ModelRegistry` is empty whatever the
    /// supervisor does, and `LlmRouter::list_models_for_provider` is a live
    /// network call (design §3.2 T2). When the bridge lands, the deregistration
    /// goes beside T2's clear and this assertion gains a second half.
    #[tokio::test]
    async fn a_disabled_plugin_leaves_no_connector_or_provider_residue() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = install_stub_plugin(
            tmp.path(),
            "echo-test",
            "[types]\ntools = true\nconnector = true\nprovider = true\n",
        );
        // The stub answers `provider/info` from a file when one is present, so
        // the load has real models to record.
        std::fs::write(
            dir.join("provider-info.json"),
            r#"{"provider_name":"echo-provider","models":[{"id":"echo-small"},{"id":"echo-large"}]}"#,
        )
        .unwrap();
        let h = Harness::new(tmp.path());
        load_running_stub(&h, "echo-test").await;

        // Whatever E4 registered is on the row while it is enabled.
        let row = h.row("echo-test").await;
        assert!(row.connector.is_some(), "the stub declared a connector");
        assert_eq!(row.provider.as_deref(), Some("echo-provider"));
        assert_eq!(
            h.registered("echo-test").await.models,
            vec!["echo-small".to_string(), "echo-large".to_string()],
            "the load recorded no models to withdraw"
        );

        h.manager.disable(&ext("echo-test")).await.unwrap();

        let row = h.row("echo-test").await;
        assert_eq!(row.state, ExtensionState::Disabled);
        assert_eq!(row.connector, None, "a disabled row still names a connector");
        assert_eq!(row.provider, None, "a disabled row still names a provider");
        assert!(
            h.registered("echo-test").await.models.is_empty(),
            "a disabled plugin's provider models are still registered"
        );
    }

    /// **`two_dirs_same_manifest_name_second_is_config_invalid`** (design §2.2
    /// X-3, §10 case 19), and the `enabled = false` variant of the same row.
    ///
    /// The directory is the id. A manifest that disagrees is parked before
    /// consent and before the bit, with **no spawn**, and its row reports the
    /// bit *as read* — the one `Failed` cell exempt from `Failed ⇒ bit == true`,
    /// because it reached neither W nor E0.
    #[tokio::test]
    async fn two_dirs_same_manifest_name_second_is_config_invalid() {
        let tmp = tempfile::tempdir().unwrap();
        install_stub_plugin(tmp.path(), "echo-test", "[types]\ntools = true\n");
        let impostor = install_stub_plugin_named(
            tmp.path(),
            "echo-copy",
            "echo-test",
            "[types]\ntools = true\n",
        );
        slow_the_entry(&impostor);

        let h = Harness::new(tmp.path());
        h.manager.permission_gate.approve("echo-test", &[]).unwrap();
        h.manager.permission_gate.approve("echo-copy", &[]).unwrap();
        // The impostor's toggle was pre-set to off before it was ever seen.
        h.manager.permission_gate.set_enabled("echo-copy", false).unwrap();

        h.scan().await;

        // The honest directory runs.
        assert_eq!(h.state("echo-test").await, ExtensionState::Enabled);
        assert!(h.has_tool("echo-test::echo"));

        // The impostor is parked, with the bit as read and no child.
        let row = h.row("echo-copy").await;
        assert!(
            matches!(
                &row.state,
                ExtensionState::Failed {
                    reason: FailureReason::ConfigInvalid,
                    detail,
                    ..
                } if detail == "manifest name does not match directory"
            ),
            "expected the name-mismatch park, got {:?}",
            row.state
        );
        assert!(
            !row.disposition.0,
            "the parked row did not report the bit as read"
        );
        assert_eq!(
            spawn_count(&impostor),
            0,
            "a directory whose manifest lies about its name was spawned"
        );
        // And the two never shared an entry: a second scan parks the impostor
        // again and the honest plugin's child is the same one, still running.
        let honest_pid = h.child_pid("echo-test").await;
        h.scan().await;
        assert_eq!(
            h.child_pid("echo-test").await,
            honest_pid,
            "the impostor's scan replaced the honest plugin's entry"
        );
        assert!(pid_alive(honest_pid), "the honest plugin's child was killed");
        assert_eq!(spawn_count(&impostor), 0);
    }

    /// **A sweep-detected crash followed by the reaper's T4 produces no
    /// *"failed to kill plugin process"* line** (design §3.2 T4).
    ///
    /// After a reaped exit `Child::start_kill` returns `InvalidInput` and
    /// `PluginProcess::kill` logs it at `error!`. The observable here is the
    /// skip itself: the sweep records the `ExitStatus`, and T4 must consume it
    /// without calling `shutdown()` or `kill()`.
    #[tokio::test]
    async fn a_sweep_detected_crash_is_reaped_without_a_second_kill() {
        let tmp = tempfile::tempdir().unwrap();
        install_stub_plugin(tmp.path(), "echo-test", "[types]\ntools = true\n");
        let h = Harness::new(tmp.path());
        load_running_stub(&h, "echo-test").await;

        // The sweep observes the exit and records it on the state.
        kill_out_of_band_and_sweep(&h, "echo-test").await;
        let exit_seen = h
            .manager
            .plugins
            .read()
            .await
            .get("echo-test")
            .and_then(|s| s.exit_status)
            .is_some();
        assert!(exit_seen, "the sweep did not record the observed exit");

        let generation = h.manager.ledger.generation(&ext("echo-test")).unwrap();
        h.manager.reap(&ext("echo-test"), generation).await;

        assert!(!h.holds_process("echo-test").await, "T4 kept the dead handle");
        assert!(!h.has_tool("echo-test::echo"), "the reaper published nothing");
        assert!(
            matches!(
                h.state("echo-test").await,
                ExtensionState::Failed {
                    reason: FailureReason::Crashed,
                    ..
                }
            ),
            "the reaper wrote state"
        );
    }

    /// **Drain sees runs** (design §3.2 T3(b)): a disable waits for an
    /// in-flight plugin-skill run, and the run's caller receives the S4
    /// refusal, never a channel-error string.
    #[tokio::test]
    async fn a_disable_waits_for_an_in_flight_run_and_refuses_it_by_name() {
        let tmp = tempfile::tempdir().unwrap();
        install_stub_plugin(tmp.path(), "echo-test", "[types]\ntools = true\n");
        let h = Harness::new(tmp.path());
        load_running_stub(&h, "echo-test").await;
        let generation = h.manager.ledger.generation(&ext("echo-test")).unwrap();

        // Stand in for a multi-second `skill/invoke`: the run-guard is what
        // makes it visible to T3 at all.
        let guard = h
            .manager
            .ledger
            .begin_run(&ext("echo-test"), generation)
            .expect("a run against an enabled plugin");
        assert_eq!(h.manager.ledger.in_flight(&ext("echo-test")), 1);

        let manager = Arc::clone(&h.manager);
        let disable = tokio::spawn(async move { manager.disable(&ext("echo-test")).await });

        // The gate flips at T0, so the run's own exit is already the S4 refusal
        // while the drain is still waiting for it. Wait for that flip by
        // reading it, not by sleeping: on a loaded machine a fixed sleep can
        // expire while the row still says `Enabled`, and nothing would rewrite.
        for _ in 0..500 {
            if h.manager.ledger.state(&ext("echo-test")) == Some(ExtensionState::Disabling) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert_eq!(
            h.manager.ledger.state(&ext("echo-test")),
            Some(ExtensionState::Disabling),
            "the disable never reached T0"
        );
        let refusal = h
            .manager
            .ledger
            .run_scoped(&ext("echo-test"), async {
                Err::<(), String>("plugin echo-test::skill/invoke: channel closed".to_string())
            })
            .await
            .expect_err("the run must not succeed");
        assert!(
            refusal.contains("turned off") || refusal.contains("disabled"),
            "the run got a channel string, not the S4 refusal: {refusal}"
        );
        assert!(!refusal.contains("channel closed"), "{refusal}");
        assert!(!disable.is_finished(), "the drain did not wait for the run");

        drop(guard);
        let row = disable.await.unwrap().unwrap();
        assert_eq!(row.state, ExtensionState::Disabled);
        assert!(row.warnings.is_empty(), "a clean drain reported stragglers");
    }

    /// The three states one plugin walks through, read as `ExtensionState`.
    ///
    /// This was `the_legacy_route_still_reads_the_words_it_always_did`, which
    /// asserted the `running`/`waiting-approval`/`disabled` vocabulary of
    /// `GET /v1/plugins`. C7 deleted that route and its shim; the sequence is
    /// still worth pinning, so it now reads the ledger's own words (design
    /// §4.3).
    #[tokio::test]
    async fn one_plugin_walks_unapproved_then_enabled_then_disabled() {
        let tmp = tempfile::tempdir().unwrap();
        install_stub_plugin(tmp.path(), "echo-test", "[types]\ntools = true\n");
        let h = Harness::new(tmp.path());

        h.scan().await;
        assert_eq!(
            h.state("echo-test").await,
            ExtensionState::Unapproved {
                reason: UnapprovedReason::NeverSeen
            }
        );

        h.manager.approve_plugin("echo-test").await.unwrap();
        assert_eq!(h.state("echo-test").await, ExtensionState::Enabled);

        h.manager.disable(&ext("echo-test")).await.unwrap();
        assert_eq!(h.state("echo-test").await, ExtensionState::Disabled);
    }

    /// **A sensitive config key never appears in `.config/<name>.toml` and is
    /// redacted on read** (design §8, X-29).
    ///
    /// Which of the two stores holds the value is the caller's choice: §13 Q12
    /// has not fixed a default, so `set_plugin_config` refuses a sensitive key
    /// outright rather than picking one.
    #[tokio::test]
    async fn a_sensitive_config_value_never_lands_in_the_plugin_toml() {
        let tmp = tempfile::tempdir().unwrap();
        install_stub_plugin(
            tmp.path(),
            "echo-test",
            "[types]\ntools = true\n\n[config.api_key]\ntype = \"secret\"\nrequired = true\nsensitive = true\n",
        );
        let h = Harness::new(tmp.path());
        h.scan().await;

        // The plaintext path refuses and names the alternative.
        let err = h
            .manager
            .set_plugin_config("echo-test", "api_key", toml::Value::String("sk-live".into()))
            .await
            .expect_err("a sensitive key must not be written in the clear");
        assert!(err.to_string().contains("sensitive"), "{err}");
        assert!(
            !tmp.path().join(".config/echo-test.toml").exists(),
            "the refused write created the file anyway"
        );

        // The reference path stores only the reference.
        h.manager
            .set_plugin_secret("echo-test", "api_key", "sk-live", SecretStorage::Encrypted)
            .await
            .unwrap();

        let raw = std::fs::read_to_string(tmp.path().join(".config/echo-test.toml")).unwrap();
        assert!(
            !raw.contains("sk-live"),
            "the secret is in the plugin's TOML: {raw}"
        );
        assert!(raw.contains("secret_encrypted"), "{raw}");

        // A read redacts it.
        let shown = h.manager.plugin_config_redacted("echo-test").await;
        assert_eq!(
            shown.get("api_key"),
            Some(&toml::Value::String("<redacted>".to_string()))
        );

        // And the plugin's own `initialize` still gets the plaintext back.
        let resolved = h
            .manager
            .resolve_config("echo-test", h.manager.permission_gate.load_plugin_config("echo-test"))
            .expect("the reference resolves");
        assert_eq!(
            resolved.get("api_key"),
            Some(&toml::Value::String("sk-live".to_string()))
        );
    }

    /// **Redaction is the manifest's declaration, not the stored shape.**
    ///
    /// Design §8 says the `GET` *"redacts sensitive keys"*. Redacting only
    /// values that parse as a secret reference is a proxy for that: a plain
    /// string sitting under a key the manifest marks `sensitive` reads back in
    /// the clear. That state is realistic rather than hypothetical — the CLI
    /// C6 replaced told the owner to manage plugin config by editing
    /// `plugins/.config/<name>.toml` by hand, and C6 is the commit that first
    /// serves that file over HTTP. The predicate is the **union**: a secret
    /// reference ∪ a key the manifest declares sensitive.
    #[tokio::test]
    async fn a_hand_typed_value_under_a_sensitive_key_is_redacted_too() {
        let tmp = tempfile::tempdir().unwrap();
        install_stub_plugin(
            tmp.path(),
            "echo-test",
            "[types]\ntools = true\n\n[config.api_key]\ntype = \"secret\"\nsensitive = true\n\n\
             [config.endpoint]\ntype = \"string\"\n",
        );
        let h = Harness::new(tmp.path());
        h.scan().await;

        // Hand-written exactly as the pre-C6 CLI instructed: a plaintext token
        // under the sensitive key, no reference table anywhere in the file.
        std::fs::create_dir_all(tmp.path().join(".config")).unwrap();
        std::fs::write(
            tmp.path().join(".config/echo-test.toml"),
            "api_key = \"sk-hand-typed\"\nendpoint = \"https://example.test\"\n",
        )
        .unwrap();

        let shown = h.manager.plugin_config_redacted("echo-test").await;
        assert_eq!(
            shown.get("api_key"),
            Some(&toml::Value::String("<redacted>".to_string())),
            "a key the manifest declares sensitive must be redacted however it \
             is stored: {shown:?}"
        );
        assert_eq!(
            shown.get("endpoint"),
            Some(&toml::Value::String("https://example.test".to_string())),
            "a key that is not sensitive is served as stored: {shown:?}"
        );
    }

    /// Every transition announces itself on the bus with its state word and the
    /// load's generation (design §7.3).
    #[tokio::test]
    async fn every_transition_announces_itself_with_its_state_word() {
        use openalpaca_core::events::SystemEvent;

        let tmp = tempfile::tempdir().unwrap();
        install_stub_plugin(tmp.path(), "echo-test", "[types]\ntools = true\n");
        let h = Harness::new(tmp.path());
        let mut rx = h.bus.subscribe();

        h.manager.permission_gate.approve("echo-test", &[]).unwrap();
        h.scan().await;
        h.manager.disable(&ext("echo-test")).await.unwrap();

        let mut words = Vec::new();
        while let Ok(event) = rx.try_recv() {
            if let SystemEvent::ExtensionStateChanged {
                extension, state, ..
            } = event
                && extension == ext("echo-test")
            {
                words.push(state);
            }
        }
        assert!(
            words.contains(&"enabled".to_string()) && words.contains(&"disabled".to_string()),
            "the transitions were not announced: {words:?}"
        );
    }

    /// `shutdown_all` closes every live child (design §3.5). Nothing did before:
    /// `kill_on_drop` does not fire on `process::exit`.
    #[tokio::test]
    async fn shutdown_all_closes_every_live_child() {
        let tmp = tempfile::tempdir().unwrap();
        install_stub_plugin(tmp.path(), "echo-one", "[types]\ntools = true\n");
        install_stub_plugin(tmp.path(), "echo-two", "[types]\ntools = true\n");
        let h = Harness::new(tmp.path());
        h.manager.permission_gate.approve("echo-one", &[]).unwrap();
        h.manager.permission_gate.approve("echo-two", &[]).unwrap();
        h.scan().await;
        let pids = [
            h.child_pid("echo-one").await,
            h.child_pid("echo-two").await,
        ];

        ExtensionSupervisor::shutdown_all(&*h.manager).await;

        for pid in pids {
            assert!(!pid_alive(pid), "plugin child {pid} outlived the daemon");
        }
        assert!(!h.holds_process("echo-one").await);
        assert!(!h.holds_process("echo-two").await);
    }

    /// A directory that vanishes parks as `Orphaned`, and its permissions entry
    /// is **never** deleted (design §5.1).
    #[tokio::test]
    async fn a_vanished_directory_parks_as_orphaned_and_keeps_its_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = install_stub_plugin(tmp.path(), "echo-test", "[types]\ntools = true\n");
        let h = Harness::new(tmp.path());
        load_running_stub(&h, "echo-test").await;
        let pid = h.child_pid("echo-test").await;

        std::fs::remove_dir_all(&dir).unwrap();
        h.scan().await;

        assert_eq!(h.state("echo-test").await, ExtensionState::Orphaned);
        assert!(!pid_alive(pid), "the orphaned plugin's child was left running");
        assert!(!h.has_tool("echo-test::echo"));
        let raw = std::fs::read_to_string(tmp.path().join(".permissions.toml")).unwrap();
        assert!(
            raw.contains("echo-test"),
            "the permissions entry was deleted: {raw}"
        );

        // §4.1's `Orphaned` row: every verb is a 409 and only `DELETE` applies.
        // The word is `orphaned` — `not_orphaned` is `DELETE`'s refusal on a row
        // that is *not* one (design §8; C6 split the two).
        for result in [
            h.manager.enable(&ext("echo-test")).await,
            h.manager.disable(&ext("echo-test")).await,
            h.manager.reload(&ext("echo-test")).await,
            h.manager.approve_plugin("echo-test").await,
            h.manager.deny_plugin("echo-test").await,
        ] {
            assert!(
                matches!(result, Err(ExtensionError::Orphaned)),
                "an orphaned row accepted a verb: {result:?}"
            );
        }

        // …and `DELETE` does apply, removing the one entry §5.1 says nothing
        // else may ever delete.
        h.manager
            .remove_orphan("echo-test")
            .await
            .expect("an orphan is removable");
        let raw = std::fs::read_to_string(tmp.path().join(".permissions.toml")).unwrap();
        assert!(
            !raw.contains("echo-test"),
            "the owner's explicit Remove should delete the entry: {raw}"
        );
    }

    /// **`Orphaned` at a cold start** — the state's only production trigger
    /// (design §5.1 row 2, §4.1 *declaration gone — plugin*).
    ///
    /// A daemon that boots with an entry in `.permissions.toml` whose directory
    /// is gone starts with an **empty** map, so a vanished set computed from the
    /// map alone produces no record at all: nothing for `?include_orphaned=true`
    /// to show and nothing for C6's `DELETE` to target. The entry is the only
    /// thing that remembers the plugin, so it is the set the scan must read.
    #[tokio::test]
    async fn a_cold_start_parks_a_vanished_directorys_entry_as_orphaned() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = install_stub_plugin(tmp.path(), "echo-test", "[types]\ntools = true\n");
        let h = Harness::new(tmp.path());
        load_running_stub(&h, "echo-test").await;
        ExtensionSupervisor::shutdown_all(&*h.manager).await;

        // The directory goes while the daemon is down.
        std::fs::remove_dir_all(&dir).unwrap();

        // A restart: fresh in-memory state, the same `.permissions.toml`.
        let restarted = Harness::restart(tmp.path());
        restarted.scan().await;

        let row = restarted.row("echo-test").await;
        assert_eq!(
            row.state,
            ExtensionState::Orphaned,
            "the boot scan produced no orphan for an entry whose directory is gone"
        );
        assert!(row.disposition.0, "the orphan lost the owner's toggle");
        assert_eq!(row.consent, Some(Consent::Approved), "consent was lost");

        // The row is what C6's `?include_orphaned=true` and `DELETE` read.
        let rows = ExtensionSupervisor::list(&*restarted.manager).await;
        assert!(
            rows.iter()
                .any(|r| r.id == ext("echo-test") && r.state == ExtensionState::Orphaned),
            "the orphan is invisible to `list()`: {rows:?}"
        );

        // And the entry itself is never deleted.
        let raw = std::fs::read_to_string(tmp.path().join(".permissions.toml")).unwrap();
        assert!(raw.contains("echo-test"), "the entry was deleted: {raw}");
    }

    /// **A park never leaves a live handle behind** (design §3.3.1's S2
    /// invariant, §4.1 "a live hole in the approval gate").
    ///
    /// The store's consent is revoked out of band — an owner editing the file,
    /// or C6's aggregator reconciling — and the next `reconcile` reads
    /// `approved = false`. Parking `Unapproved{Denied}` over a running child
    /// would leave its tools registered and its capability provider installed
    /// under a row that says the owner refused it.
    #[tokio::test]
    async fn a_reconcile_over_a_revoked_approval_tears_the_live_plugin_down() {
        let tmp = tempfile::tempdir().unwrap();
        install_stub_plugin(
            tmp.path(),
            "echo-test",
            "[types]\ntools = true\n\n[capabilities.virtual]\nprovides = [\"annotation:echo_stub\"]\n",
        );
        let h = Harness::new(tmp.path());
        load_running_stub(&h, "echo-test").await;
        let pid = h.child_pid("echo-test").await;

        // The store alone changes: no verb, no teardown yet.
        h.manager.permission_gate.deny("echo-test").unwrap();
        h.manager.reconcile(&ext("echo-test")).await.unwrap();

        let row = h.row("echo-test").await;
        assert_eq!(
            row.state,
            ExtensionState::Unapproved {
                reason: UnapprovedReason::Denied
            }
        );
        assert!(!pid_alive(pid), "the denied plugin's child {pid} is alive");
        assert!(!h.holds_process("echo-test").await);
        assert!(!h.has_tool("echo-test::echo"), "its tool is still callable");
        assert_eq!(h.stub_caps(), 0, "its capability provider is still installed");
    }

    /// The same hole through the toggle: the bit is cleared out of band and the
    /// reconcile reads `enabled = false` (design §3.3.1, §5.1's watcher row).
    #[tokio::test]
    async fn a_reconcile_over_a_cleared_bit_tears_the_live_plugin_down() {
        let tmp = tempfile::tempdir().unwrap();
        install_stub_plugin(
            tmp.path(),
            "echo-test",
            "[types]\ntools = true\n\n[capabilities.virtual]\nprovides = [\"annotation:echo_stub\"]\n",
        );
        let h = Harness::new(tmp.path());
        load_running_stub(&h, "echo-test").await;
        let pid = h.child_pid("echo-test").await;

        h.manager
            .permission_gate
            .set_enabled("echo-test", false)
            .unwrap();
        h.manager.reconcile(&ext("echo-test")).await.unwrap();

        assert_eq!(h.state("echo-test").await, ExtensionState::Disabled);
        assert!(!pid_alive(pid), "the disabled plugin's child {pid} is alive");
        assert!(!h.holds_process("echo-test").await);
        assert!(!h.has_tool("echo-test::echo"));
        assert_eq!(h.stub_caps(), 0);
    }

    /// **Two overlapping enables load the plugin once** — the property A2's
    /// claim token used to carry and the per-extension mutex plus E0's CAS now
    /// carry (design §3, §3.3 E0).
    ///
    /// The entry is made slow *and countable*: `spawns.log` names every child
    /// the plugin ever started, which the manager's own bookkeeping cannot show,
    /// so a second bring-up that was torn down again would still be visible.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn concurrent_enables_load_the_plugin_once() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = install_stub_plugin(
            tmp.path(),
            "echo-test",
            "[types]\ntools = true\n\n[capabilities.virtual]\nprovides = [\"annotation:echo_stub\"]\n",
        );
        slow_the_entry(&dir);

        let h = Harness::new(tmp.path());
        h.manager.permission_gate.approve("echo-test", &[]).unwrap();
        h.manager
            .permission_gate
            .set_enabled("echo-test", false)
            .unwrap();
        h.scan().await;
        assert_eq!(h.state("echo-test").await, ExtensionState::Disabled);
        assert_eq!(spawn_count(&dir), 0, "a disabled plugin was started");
        let generation = h.manager.ledger.generation(&ext("echo-test")).unwrap();
        // The registry carries built-in providers of its own, so the plugin's
        // is counted as a delta.
        let providers = h.tools.provider_handles().len();

        let first = tokio::spawn({
            let manager = Arc::clone(&h.manager);
            async move { manager.enable(&ext("echo-test")).await }
        });
        let second = tokio::spawn({
            let manager = Arc::clone(&h.manager);
            async move { manager.enable(&ext("echo-test")).await }
        });
        let (first, second) = tokio::join!(first, second);
        first.unwrap().unwrap();
        second.unwrap().unwrap();
        assert_eq!(h.state("echo-test").await, ExtensionState::Enabled);
        assert_eq!(
            spawn_count(&dir),
            1,
            "the second enable spawned a child of its own: {:?}",
            spawned_pids(&dir)
        );
        assert_eq!(
            h.child_pid("echo-test").await,
            spawned_pids(&dir)[0],
            "the map holds a child the log does not name"
        );
        assert_eq!(
            h.tools.provider_handles().len(),
            providers + 1,
            "the losing enable installed a capability provider of its own"
        );
        assert_eq!(h.stub_caps(), 1, "the stub's virtual cap is registered twice");
        assert_eq!(
            h.manager.ledger.generation(&ext("echo-test")),
            Some(generation + 1),
            "the losing enable took an E0 CAS of its own"
        );
        assert!(h.manager.ledger.audit(&h.tools).is_empty());
    }

    /// **The `HandleHeld` insert guard** (design §2.2), driven rather than
    /// argued: E5's map write refuses to replace an entry that still holds a
    /// live handle, and E4b takes back everything the refused attempt published.
    ///
    /// It is an assertion — E-PRE and T4 both run under the mutex this load
    /// holds, so no legitimate path reaches it — but an untested `error!` +
    /// unwind branch is not an assertion, it is dead weight. This calls E4/E5
    /// directly, which is the only way to stand a second load up beside a live
    /// one.
    #[tokio::test]
    async fn publishing_over_a_live_handle_is_refused_and_unwinds() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = install_stub_plugin(
            tmp.path(),
            "echo-test",
            "[types]\ntools = true\n\n[capabilities.virtual]\nprovides = [\"annotation:echo_stub\"]\n",
        );
        let h = Harness::new(tmp.path());
        load_running_stub(&h, "echo-test").await;
        let live_pid = h.child_pid("echo-test").await;
        let live_handle = h.provider_handle("echo-test").await;
        let providers = h.tools.provider_handles().len();

        // A second bring-up of the same plugin, published behind the live one's
        // back.
        let manifest = PluginManifest::from_dir(&dir).unwrap();
        let intruder = PluginProcess::spawn(&manifest, &dir).unwrap();
        let intruder_pid = intruder.child.id().expect("the second child has a pid");
        let discovered = Discovered {
            tools: vec![mock_plugin_tool("echo-test::intruder", "echo-test")],
            connector: None,
            provider: None,
            models: Vec::new(),
            skill: None,
            agent: None,
        };

        let err = h
            .manager
            .publish(&ext("echo-test"), &dir, manifest, intruder, discovered)
            .await
            .expect_err("E5 published over a live handle");
        assert!(
            matches!(err, PluginError::HandleHeld(ref id) if id == "echo-test"),
            "expected HandleHeld, got {err:?}"
        );

        // E4b: everything the refused attempt published is back off, and its
        // child is gone.
        assert!(
            !h.has_tool("echo-test::intruder"),
            "the refused attempt's tool stayed registered"
        );
        assert_eq!(
            h.tools.provider_handles().len(),
            providers,
            "the refused attempt's capability provider stayed installed"
        );
        assert!(
            !pid_alive(intruder_pid),
            "the refused attempt's child {intruder_pid} was left running"
        );

        // The live load is untouched.
        assert_eq!(h.child_pid("echo-test").await, live_pid);
        assert!(pid_alive(live_pid));
        assert_eq!(h.provider_handle("echo-test").await, live_handle);
        assert!(h.has_tool("echo-test::echo"));
        assert_eq!(h.stub_caps(), 1);
    }

    /// **A reload over an unreadable store leaves the row alone** (design §3.2:
    /// "Nothing is ever left in `Enabling`/`Disabling` because of a disk
    /// error").
    ///
    /// `reload` used to read the store *after* T0–T4, so a file that became
    /// unparseable in between returned `409` with the child already killed,
    /// every contribution withdrawn and the record stranded in `Disabling` —
    /// after which the gate refused everything for that extension with *"is
    /// being reloaded right now"*. The read belongs before T0, the way
    /// `enable`/`disable` put W before their CAS.
    #[tokio::test]
    async fn a_reload_over_an_unreadable_store_leaves_the_row_alone() {
        let tmp = tempfile::tempdir().unwrap();
        install_stub_plugin(tmp.path(), "echo-test", "[types]\ntools = true\n");
        let h = Harness::new(tmp.path());
        load_running_stub(&h, "echo-test").await;
        let pid = h.child_pid("echo-test").await;

        let store = tmp.path().join(".permissions.toml");
        let garbage = "[echo-test\napproved = yes";
        std::fs::write(&store, garbage).unwrap();

        let err = h
            .manager
            .reload(&ext("echo-test"))
            .await
            .expect_err("a reload over an unreadable store must refuse");
        assert!(
            matches!(err, ExtensionError::StoreUnreadable(_)),
            "expected store_unreadable, got {err:?}"
        );

        let state = h.manager.ledger.state(&ext("echo-test")).unwrap();
        assert_ne!(
            state,
            ExtensionState::Disabling,
            "the record is stranded mid-reload; every verb now reads `reloading`"
        );
        assert_eq!(state, ExtensionState::Enabled);
        assert!(pid_alive(pid), "the child was killed by a refused reload");
        assert!(h.has_tool("echo-test::echo"));
    }

    /// A plugin-authored tool, for the tests that publish one by hand.
    fn mock_plugin_tool(name: &str, plugin: &str) -> RegisteredTool {
        RegisteredTool {
            definition: ToolDefinition {
                name: name.to_string(),
                description: "test".into(),
                parameters: serde_json::json!({"type": "object"}),
                strict: None,
                input_examples: None,
            },
            backend: ToolBackend::Http {
                method: "GET".into(),
                url: "http://example.com".into(),
                headers: Default::default(),
                timeout_secs: 10,
            },
            provides_capabilities: vec![],
            exempt_from_timeout: false,
            annotations: None,
            version: "0.0.0".into(),
            author: format!("plugin:{plugin}"),
            created_at: chrono::Utc::now(),
        }
    }
}

#[cfg(test)]
mod p3e_provider_tests {
    use crate::manager::*;
    use openalpaca_core::tools::registry::{RegisteredTool, ToolBackend};
    use openalpaca_llm::ToolDefinition;

    fn mock_tool(name: &str, author: &str) -> RegisteredTool {
        RegisteredTool {
            definition: ToolDefinition {
                name: name.to_string(),
                description: "test".to_string(),
                parameters: serde_json::json!({"type": "object"}),
                strict: None,
                input_examples: None,
            },
            backend: ToolBackend::Http {
                method: "GET".into(),
                url: "http://example.com".into(),
                headers: Default::default(),
                timeout_secs: 10,
            },
            provides_capabilities: vec![],
            exempt_from_timeout: false,
            annotations: None,
            version: "0.0.0".into(),
            author: author.to_string(),
            created_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn plugin_provider_emits_caps_for_matching_author() {
        let p =
            PluginCapabilityProvider::new("foo".to_string(), vec!["annotation:test".to_string()]);
        let tool_match = mock_tool("x", "plugin:foo");
        let tool_nomatch = mock_tool("y", "plugin:bar");
        let tool_builtin = mock_tool("z", "builtin");

        assert_eq!(
            p.derive_capabilities(&tool_match),
            vec!["annotation:test".to_string()]
        );
        assert!(p.derive_capabilities(&tool_nomatch).is_empty());
        assert!(p.derive_capabilities(&tool_builtin).is_empty());
    }

    #[test]
    fn plugin_provider_known_names_returns_declared_list() {
        let p = PluginCapabilityProvider::new(
            "foo".to_string(),
            vec!["annotation:a".to_string(), "annotation:b".to_string()],
        );
        let names = p.known_capability_names();
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"annotation:a".to_string()));
        assert!(names.contains(&"annotation:b".to_string()));
    }

    #[test]
    fn plugin_provider_with_empty_caps_is_noop() {
        let p = PluginCapabilityProvider::new("foo".to_string(), vec![]);
        let tool = mock_tool("x", "plugin:foo");
        assert!(p.derive_capabilities(&tool).is_empty());
        assert!(p.known_capability_names().is_empty());
    }

    #[test]
    fn plugin_provider_handles_non_annotation_caps() {
        let p = PluginCapabilityProvider::new(
            "foo".to_string(),
            vec!["plugin:mytag".to_string(), "annotation:safe".to_string()],
        );
        let tool = mock_tool("x", "plugin:foo");
        let caps = p.derive_capabilities(&tool);
        assert_eq!(caps.len(), 2);
        assert!(caps.contains(&"plugin:mytag".to_string()));
        assert!(caps.contains(&"annotation:safe".to_string()));
    }
}

#[cfg(test)]
mod p3e_integration_tests {
    use crate::manager::*;
    use openalpaca_core::tools::registry::{RegisteredTool, ToolBackend, ToolRegistry};
    use openalpaca_llm::ToolDefinition;
    use std::sync::Arc;

    fn mock_plugin_tool(name: &str, plugin_name: &str) -> RegisteredTool {
        RegisteredTool {
            definition: ToolDefinition {
                name: name.to_string(),
                description: "test".into(),
                parameters: serde_json::json!({"type": "object"}),
                strict: None,
                input_examples: None,
            },
            backend: ToolBackend::Http {
                method: "GET".into(),
                url: "http://example.com".into(),
                headers: Default::default(),
                timeout_secs: 10,
            },
            provides_capabilities: vec![],
            exempt_from_timeout: false,
            annotations: None,
            version: "0.0.0".into(),
            author: format!("plugin:{}", plugin_name),
            created_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn plugin_provider_integrates_with_tool_registry() {
        let registry = ToolRegistry::new().unwrap();
        registry
            .register(mock_plugin_tool("foo_read", "myplugin"))
            .unwrap();

        let provider = PluginCapabilityProvider::new(
            "myplugin".to_string(),
            vec!["annotation:test_tag".to_string()],
        );
        let _handle = registry.register_capability_provider(Arc::new(provider));

        let known = registry.known_virtual_capabilities();
        assert!(known.iter().any(|k| k == "annotation:test_tag"));

        let tools = registry.tools_for_capabilities(&["annotation:test_tag".to_string()]);
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "foo_read");
    }

    #[test]
    fn plugin_provider_removal_scrubs_virtual_caps() {
        let registry = ToolRegistry::new().unwrap();
        registry
            .register(mock_plugin_tool("foo_read", "myplugin"))
            .unwrap();

        let provider = PluginCapabilityProvider::new(
            "myplugin".to_string(),
            vec!["annotation:test_tag".to_string()],
        );
        let handle = registry.register_capability_provider(Arc::new(provider));

        let before = registry.tools_for_capabilities(&["annotation:test_tag".to_string()]);
        assert_eq!(before.len(), 1);

        registry.remove_capability_provider(handle);

        let after = registry.tools_for_capabilities(&["annotation:test_tag".to_string()]);
        assert!(after.is_empty());

        // Tool itself still registered
        assert!(
            registry
                .registered_tool_names()
                .iter()
                .any(|n| n == "foo_read")
        );

        // Known virtual caps no longer includes the plugin's tag
        let known = registry.known_virtual_capabilities();
        assert!(!known.iter().any(|k| k == "annotation:test_tag"));
    }

    #[test]
    fn plugin_provider_reload_issues_fresh_handle() {
        let registry = ToolRegistry::new().unwrap();
        registry
            .register(mock_plugin_tool("foo_read", "myplugin"))
            .unwrap();

        let provider1 = PluginCapabilityProvider::new(
            "myplugin".to_string(),
            vec!["annotation:test_tag".to_string()],
        );
        let h1 = registry.register_capability_provider(Arc::new(provider1));
        registry.remove_capability_provider(h1);

        let provider2 = PluginCapabilityProvider::new(
            "myplugin".to_string(),
            vec!["annotation:test_tag".to_string()],
        );
        let h2 = registry.register_capability_provider(Arc::new(provider2));
        assert_ne!(h1, h2);

        let tools = registry.tools_for_capabilities(&["annotation:test_tag".to_string()]);
        assert_eq!(tools.len(), 1);
    }
}

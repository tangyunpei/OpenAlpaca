//! Agent template loading: the bundled templates, the `.md` files of
//! `config/agents/` over them, and the legacy `.toml` agents.

use openalpaca_core::agent::file_layer::{LoadMode, rescan_file_layer};
use openalpaca_core::bundled::bundled_templates;
use openalpaca_core::context::SharedContext;
use openalpaca_core::tools::registry::validate_annotation_capability;
use openalpaca_storage::Database;
use std::path::Path;
use std::sync::Arc;
use tracing::warn;

pub(super) fn load_agent_templates(
    config_base_dir: &Path,
    db: &Database,
    shared_context: &Arc<SharedContext>,
    tool_registry: &Arc<openalpaca_core::tools::ToolRegistry>,
) -> anyhow::Result<()> {
    let config_dir = config_base_dir.join("agents");
    let known = tool_registry.known_virtual_capabilities();

    // The templates the binary carries (subagent design §2.1 B-1), set
    // before the files are read: a file of the person's overrides the
    // bundled template of its id, and an unedited copy of a shipped version
    // is set aside so the bundled one runs (§2.2 V-1).
    shared_context
        .agent_registry
        .set_bundled_layer(bundled_templates());

    // The Markdown templates: the file layer's rescan (subagent design §2.4
    // L-3), the one reader the watcher and the template routes share. At
    // start an unknown `annotation:` capability is an error, so the daemon
    // does not start on such a file (extension design §10 case 2). It also
    // writes every resolved template's `agent` row, a bundled one's
    // included (§2.11), and logs what is set aside and what overrides.
    rescan_file_layer(
        &config_dir,
        LoadMode::Start,
        &shared_context.agent_registry,
        tool_registry,
        db,
    )
    .map_err(|e| anyhow::anyhow!(e))?;

    let entries = match std::fs::read_dir(&config_dir) {
        Ok(e) => e,
        Err(_) => return Ok(()),
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let ext = path.extension().and_then(|e| e.to_str());

        // Legacy: TOML configs (.toml) — backward compat; read at start only
        if ext != Some("toml") {
            continue; // skip non-agent files (the .md ones were read above)
        }
        match std::fs::read_to_string(&path) {
            Ok(content) => {
                match toml::from_str::<openalpaca_core::agent::AgentConfigFile>(&content) {
                    Ok(agent_config) => {
                        // Validate annotation: capabilities from the TOML config
                        // across both assigned/denied buckets and the constraints
                        // allowed/denied fields.
                        for cap in &agent_config.capabilities.assigned {
                            validate_annotation_capability(cap, &known).map_err(|e| {
                                anyhow::anyhow!("{}: capabilities.assigned: {e}", path.display())
                            })?;
                        }
                        if let Some(denied) = &agent_config.capabilities.denied {
                            for cap in denied {
                                validate_annotation_capability(cap, &known).map_err(|e| {
                                    anyhow::anyhow!("{}: capabilities.denied: {e}", path.display())
                                })?;
                            }
                        }
                        if let Some(c) = &agent_config.constraints {
                            if let Some(allowed) = &c.allowed_capabilities {
                                for cap in allowed {
                                    validate_annotation_capability(cap, &known).map_err(|e| {
                                        anyhow::anyhow!(
                                            "{}: constraints.allowed_capabilities: {e}",
                                            path.display()
                                        )
                                    })?;
                                }
                            }
                            if let Some(denied) = &c.denied_capabilities {
                                for cap in denied {
                                    validate_annotation_capability(cap, &known).map_err(|e| {
                                        anyhow::anyhow!(
                                            "{}: constraints.denied_capabilities: {e}",
                                            path.display()
                                        )
                                    })?;
                                }
                            }
                        }

                        // Register in-memory
                        let subagent = agent_config.clone().into_subagent();
                        shared_context.agent_registry.register(subagent);

                        // Persist to DB
                        let storage_config = agent_config.into_storage_config();
                        let agent_id = storage_config.id.clone();
                        let repo = openalpaca_storage::SubAgentRepository::new(db);
                        let _ = repo.upsert(&storage_config);

                        // Initialize metrics row if not exists
                        if let Ok(None) = repo.get_metrics(&agent_id) {
                            let _ = repo.upsert_metrics(
                                &openalpaca_storage::AgentMetrics::new_empty(&agent_id),
                            );
                        }

                        warn!(
                            "Loaded agent config (legacy TOML): {} — please convert to .md format",
                            path.display()
                        );
                    }
                    Err(e) => {
                        warn!("Failed to parse agent config {}: {}", path.display(), e);
                    }
                }
            }
            Err(e) => {
                warn!("Failed to read agent config {}: {}", path.display(), e);
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::HomeStoreGuard;
    use openalpaca_core::tools::ToolRegistry;

    struct Start {
        // Declared first, so it drops first.
        _guard: HomeStoreGuard,
        dir: tempfile::TempDir,
        db: Database,
        shared_context: Arc<SharedContext>,
        tool_registry: Arc<ToolRegistry>,
    }

    impl Start {
        fn with(files: &[(&str, &str)]) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let agents = dir.path().join("agents");
            std::fs::create_dir_all(&agents).unwrap();
            for (name, contents) in files {
                std::fs::write(agents.join(name), contents).unwrap();
            }
            let db = Database::open(&dir.path().join("test.db")).unwrap();
            let home = dir.path().canonicalize().unwrap().join("home");
            Self {
                _guard: HomeStoreGuard::set(&home),
                dir,
                db,
                shared_context: Arc::new(SharedContext::new()),
                tool_registry: Arc::new(ToolRegistry::default()),
            }
        }

        fn load(&self) -> anyhow::Result<()> {
            load_agent_templates(
                self.dir.path(),
                &self.db,
                &self.shared_context,
                &self.tool_registry,
            )
        }
    }

    const READER: &str = "---\nid: reader\nname: Reader\ndescription: reads\ncapabilities:\n  - \"file_read\"\n---\n";

    const LEGACY: &str = "[agent]\nid = \"legacy\"\nname = \"Legacy\"\n\
                          description = \"an agent from before the templates\"\n\n\
                          [capabilities]\nassigned = [\"web_search\"]\n\n\
                          [preset]\npersona = \"Old.\"\n";

    /// Characterization (extension design §10 case 2): an unknown
    /// `annotation:` capability in a template still stops the start, with
    /// the file and the capability named, as before the rescan.
    #[test]
    fn an_unknown_annotation_capability_still_stops_the_start() {
        let start = Start::with(&[(
            "reader.md",
            "---\nid: reader\nname: Reader\ndescription: reads\ncapabilities:\n  - \
             \"annotation:bogus\"\n---\n",
        )]);

        let err = start.load().unwrap_err().to_string();

        assert!(
            err.starts_with(&format!(
                "{}: capabilities: unknown annotation capability 'annotation:bogus'",
                start.dir.path().join("agents/reader.md").display()
            )),
            "{err}"
        );
    }

    /// Characterization (L-1a): an instance a legacy `.toml` file registered
    /// is no template's, and no rescan removes it.
    #[test]
    fn a_legacy_toml_instance_survives_a_rescan() {
        let start = Start::with(&[("reader.md", READER), ("legacy.toml", LEGACY)]);
        start.load().unwrap();
        let registry = &start.shared_context.agent_registry;
        assert!(registry.get("legacy").is_some());
        assert!(registry.get("reader").is_some());

        std::fs::remove_file(start.dir.path().join("agents/reader.md")).unwrap();
        crate::hot_reload::reload_agent_templates(
            &start.dir.path().join("agents"),
            registry,
            &start.tool_registry,
            &start.db,
        );

        assert!(
            registry.get("reader").is_none(),
            "the template's instance went"
        );
        assert!(registry.get("legacy").is_some(), "the legacy one stays");
    }

    // ── The bundled layer at start (subagent design §1, §2) ───────────

    /// `eb98b820`'s `system_agent.md`, as an earlier build seeded it.
    const SEEDED_SYSTEM_AGENT: &str = include_str!(
        "../../../../crates/openalpaca_core/src/bundled/fixtures/eb98b820/agents/system_agent.md"
    );
    /// `eb98b820`'s `writing_agent.md`, likewise.
    const SEEDED_WRITING_AGENT: &str = include_str!(
        "../../../../crates/openalpaca_core/src/bundled/fixtures/eb98b820/agents/writing_agent.md"
    );

    /// Every directory and file under a root: each file's bytes, mode and
    /// modification time.
    type Tree = std::collections::BTreeMap<
        std::path::PathBuf,
        Option<(Vec<u8>, u32, std::time::SystemTime)>,
    >;

    fn tree_of(root: &Path) -> Tree {
        fn walk(dir: &Path, root: &Path, out: &mut Tree) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                let relative = path.strip_prefix(root).unwrap().to_path_buf();
                if path.is_dir() {
                    out.insert(relative, None);
                    walk(&path, root, out);
                } else {
                    let meta = std::fs::metadata(&path).unwrap();
                    #[cfg(unix)]
                    let mode = std::os::unix::fs::PermissionsExt::mode(&meta.permissions());
                    #[cfg(not(unix))]
                    let mode = 0;
                    let bytes = std::fs::read(&path).unwrap();
                    out.insert(relative, Some((bytes, mode, meta.modified().unwrap())));
                }
            }
        }
        let mut out = Tree::new();
        walk(root, root, &mut out);
        out
    }

    /// U1: a store an earlier build seeded — unedited copies of earlier and
    /// current versions, an edited one, its skills and tools and the three
    /// TOML files — is read and nothing under `config/` is written, moved or
    /// deleted: not by the start (seeding included), not by a hot reload.
    #[test]
    fn a_start_writes_moves_and_deletes_nothing_under_config() {
        let mut files: Vec<(&str, String)> = openalpaca_core::bundled::BUNDLED_AGENTS
            .iter()
            .map(|f| {
                (
                    f.path.strip_prefix("agents/").unwrap(),
                    f.contents.to_string(),
                )
            })
            .filter(|(name, _)| !["system_agent.md", "writing_agent.md"].contains(name))
            .collect();
        files.push(("system_agent.md", SEEDED_SYSTEM_AGENT.to_string()));
        files.push(("writing_agent.md", SEEDED_WRITING_AGENT.to_string()));
        for (name, contents) in files.iter_mut() {
            if *name == "code_agent.md" {
                contents.push_str("\nMy own note.\n");
            }
        }
        let borrowed: Vec<(&str, &str)> = files.iter().map(|(n, c)| (*n, c.as_str())).collect();
        let start = Start::with(&borrowed);
        let config = start.dir.path();
        for name in ["llm.toml", "daemon.toml", "mcp.toml"] {
            std::fs::write(
                config.join(name),
                format!("# {name}, as a person left it\n"),
            )
            .unwrap();
        }
        std::fs::create_dir_all(config.join("skills/mine")).unwrap();
        std::fs::write(
            config.join("skills/mine/SKILL.md"),
            "---\nname: mine\n---\n",
        )
        .unwrap();
        std::fs::create_dir_all(config.join("tools")).unwrap();
        std::fs::write(config.join("tools/example.toml"), "# mine\n").unwrap();
        // The database lives beside `config/` in this test; it is no part of it.
        let before: Tree = tree_of(config)
            .into_iter()
            .filter(|(path, _)| !path.to_string_lossy().starts_with("test.db"))
            .collect();

        crate::bootstrap::seed_default_configs(config);
        start.load().unwrap();
        crate::hot_reload::reload_agent_templates(
            &config.join("agents"),
            &start.shared_context.agent_registry,
            &start.tool_registry,
            &start.db,
        );

        let after: Tree = tree_of(config)
            .into_iter()
            .filter(|(path, _)| !path.to_string_lossy().starts_with("test.db"))
            .collect();
        assert_eq!(before, after);
    }

    /// L-1a, §1.5: on a fresh store — `agents/` created empty — every
    /// bundled template has its idle instance, so `GET
    /// /v1/agents/general_agent/config` (`agents config`) answers, and its
    /// `agent` row (`GET /v1/agents`).
    #[test]
    fn a_bundled_template_has_an_idle_instance_and_a_config_at_start() {
        let start = Start::with(&[]);
        start.load().unwrap();

        let service = openalpaca_core::agent::AgentConfigService::new(
            start.shared_context.agent_registry.clone(),
            start.dir.path().join("agents"),
            start.db.clone(),
        );
        let (config, _) = service
            .get_agent_config("general_agent")
            .expect("the bundled template's config");
        assert_eq!(config.agent.id, "general_agent");
        let row = openalpaca_storage::SubAgentRepository::new(&start.db)
            .get("general_agent")
            .unwrap();
        assert!(row.is_some(), "its agent row is written");
    }

    /// L-1a: `PUT /v1/agents/{id}/config` on a bundled id updates the idle
    /// instance and writes no `<id>.toml` into the person's directory — a
    /// file the next start would register as a legacy agent.
    #[test]
    fn put_agent_config_on_a_bundled_id_writes_no_toml() {
        let start = Start::with(&[]);
        start.load().unwrap();
        let agents = start.dir.path().join("agents");
        let service = openalpaca_core::agent::AgentConfigService::new(
            start.shared_context.agent_registry.clone(),
            agents.clone(),
            start.db.clone(),
        );
        let (mut config, version) = service.get_agent_config("general_agent").unwrap();
        config.agent.name = "General, renamed".to_string();

        service
            .update_agent_config("general_agent", config, version)
            .unwrap();

        assert!(!agents.join("general_agent.toml").exists());
        assert_eq!(std::fs::read_dir(&agents).unwrap().count(), 0);
    }

    struct IdleExecutor;

    #[async_trait::async_trait]
    impl openalpaca_api::plugin_traits::PluginAgentExecutor for IdleExecutor {
        async fn spawn(
            &self,
            _: &str,
            _: &str,
            _: &str,
            _: &serde_json::Value,
        ) -> Result<bool, String> {
            Ok(true)
        }
        async fn step(
            &self,
            _: &str,
            _: Option<&serde_json::Value>,
        ) -> Result<(String, String, Vec<serde_json::Value>), String> {
            Ok(("complete".to_string(), String::new(), vec![]))
        }
        async fn stop(&self, _: &str) -> Result<(), String> {
            Ok(())
        }
        fn plugin_id(&self) -> &str {
            "acme"
        }
        fn agent_id(&self) -> &str {
            "general_agent"
        }
    }

    /// §7 O-4 (a), P-2: on a store with no `config/agents/` a plugin's
    /// template named like a shipped one is refused — the bundled template
    /// is an incumbent — where the parent's start registered no template and
    /// let the plugin take the id.
    #[test]
    fn a_plugin_template_cannot_take_a_bundled_id() {
        let start = Start::with(&[]);
        std::fs::remove_dir(start.dir.path().join("agents")).unwrap();
        start.load().unwrap();

        let mut template = openalpaca_core::agent::template::parse_agent_markdown(
            "---\nid: general_agent\nname: Acme\ndescription: the plugin's\n---\n",
        )
        .unwrap();
        template.source = openalpaca_core::agent::AgentSource::Plugin {
            plugin_id: "acme".to_string(),
            executor: Arc::new(IdleExecutor),
        };
        let registry = &start.shared_context.agent_registry;

        assert!(!registry.register_template(template));
        let general = registry.get_template("general_agent").unwrap();
        assert!(matches!(
            general.source,
            openalpaca_core::agent::AgentSource::Internal
        ));
        assert_ne!(general.frontmatter.name, "Acme");
    }
}

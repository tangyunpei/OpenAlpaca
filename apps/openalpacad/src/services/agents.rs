//! Agent template loading from .md and legacy .toml config files.

use openalpaca_core::agent::file_layer::{LoadMode, rescan_file_layer};
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
    if !config_dir.exists() {
        return Ok(());
    }

    let known = tool_registry.known_virtual_capabilities();

    // The Markdown templates: the file layer's rescan (subagent design §2.4
    // L-3), the one reader the watcher and the template routes share. At
    // start an unknown `annotation:` capability is an error, so the daemon
    // does not start on such a file (extension design §10 case 2).
    rescan_file_layer(
        &config_dir,
        LoadMode::Start,
        &shared_context.agent_registry,
        &known,
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
}

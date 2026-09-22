//! AgentConfigService — CRUD operations with Write-Ahead + optimistic locking.
//!
//! Supports both legacy TOML agent configs and new Markdown agent templates.

use super::config::AgentConfigFile;
use super::registry::AgentRegistry;
use super::template::{self, AgentTemplate, render_agent_markdown};
use openalpaca_storage::{AgentMetrics, Database, SubAgentRepository};
use std::path::PathBuf;
use std::sync::Arc;

/// The longest id this service will turn into a file name.
///
/// Every shipped template id is under 16 bytes (`planning_agent` is the
/// longest at 14), so 64 is generous while staying far inside the shortest
/// filesystem component limit we have to survive (255 bytes, less the
/// `.toml` / `.md` suffix and an archive timestamp).
const MAX_AGENT_ID_LEN: usize = 64;

/// Refuses an id that is not safe to interpolate into a file name.
///
/// Every write in this service builds its path as `<config_dir>/<id>.toml`,
/// `<id>.md` or `.archived/<id>.<timestamp>.<ext>`, so the id *is* the file
/// name. The grammar is deliberately narrower than "contains no traversal" —
/// ASCII letters, digits, `-` and `_`, 1..=[`MAX_AGENT_ID_LEN`] bytes — because
/// a rule that can be stated in one sentence is a rule a caller can satisfy,
/// and it closes separators, `..`, absolute paths, NUL, control characters,
/// leading dots and Unicode look-alikes in one predicate.
///
/// It **rejects**, never rewrites: slugifying `../../x` into `x` would store
/// the agent under an id its author never asked for, and the next lookup by
/// the original id would miss.
fn validate_agent_id(kind: &str, id: &str) -> Result<(), String> {
    if id.is_empty() {
        return Err(format!("{kind} id must not be empty"));
    }
    if id.len() > MAX_AGENT_ID_LEN {
        return Err(format!(
            "{kind} id is too long: {} bytes, maximum is {MAX_AGENT_ID_LEN}",
            id.len()
        ));
    }
    if !id
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(format!(
            "{kind} id '{}' is invalid: ids may contain ASCII letters, digits, '-' and '_' only",
            id.escape_default()
        ));
    }
    Ok(())
}

/// Service for managing agent configurations with Write-Ahead persistence.
pub struct AgentConfigService {
    registry: Arc<AgentRegistry>,
    config_dir: PathBuf,
    db: Database,
}

impl AgentConfigService {
    pub fn new(registry: Arc<AgentRegistry>, config_dir: PathBuf, db: Database) -> Self {
        Self {
            registry,
            config_dir,
            db,
        }
    }

    /// Get agent config and its version from the in-memory registry.
    pub fn get_agent_config(&self, id: &str) -> Result<(AgentConfigFile, u64), String> {
        let (agent, version) = self
            .registry
            .get_with_version(id)
            .ok_or_else(|| "Agent not found".to_string())?;
        let config = AgentConfigFile::from_subagent(&agent);
        Ok((config, version))
    }

    /// Update an agent config with optimistic locking.
    /// Write-Ahead: disk first, then memory, then DB.
    pub fn update_agent_config(
        &self,
        id: &str,
        config: AgentConfigFile,
        expected_version: u64,
    ) -> Result<u64, String> {
        validate_agent_id("Agent", id)?;

        // 1. Write TOML to disk (Write-Ahead) — only if no .md template exists.
        // Agents with .md templates use Markdown as the source of truth.
        let md_path = self.config_dir.join(format!("{id}.md"));
        if !md_path.exists() {
            let toml_content = toml::to_string_pretty(&config)
                .map_err(|e| format!("Failed to serialize config: {e}"))?;
            let toml_path = self.config_dir.join(format!("{id}.toml"));
            std::fs::write(&toml_path, &toml_content)
                .map_err(|e| format!("Failed to write config: {e}"))?;
        }

        // 2. Update in-memory registry with optimistic locking
        let subagent = config.clone().into_subagent();
        let new_version = self
            .registry
            .update_config(id, subagent, expected_version)?;

        // 3. Persist to DB
        let storage_config = config.into_storage_config();
        let repo = SubAgentRepository::new(&self.db);
        let _ = repo.upsert(&storage_config);

        Ok(new_version)
    }

    /// Create a new agent. Returns the agent_id.
    pub fn create_agent(&self, config: AgentConfigFile) -> Result<String, String> {
        let agent_id = config.agent.id.clone();
        validate_agent_id("Agent", &agent_id)?;

        // Check not already registered
        if self.registry.get(&agent_id).is_some() {
            return Err(format!("Agent '{}' already exists", agent_id));
        }

        // 1. Write TOML to disk (Write-Ahead) — only if no .md template exists.
        // Agents with .md templates use Markdown as the source of truth.
        let md_path = self.config_dir.join(format!("{agent_id}.md"));
        if !md_path.exists() {
            std::fs::create_dir_all(&self.config_dir)
                .map_err(|e| format!("Failed to create config dir: {e}"))?;
            let toml_content = toml::to_string_pretty(&config)
                .map_err(|e| format!("Failed to serialize config: {e}"))?;
            let toml_path = self.config_dir.join(format!("{agent_id}.toml"));
            std::fs::write(&toml_path, &toml_content)
                .map_err(|e| format!("Failed to write config: {e}"))?;
        }

        // 2. Register in memory
        let subagent = config.clone().into_subagent();
        self.registry.register(subagent);

        // 3. Persist to DB + init metrics
        let storage_config = config.into_storage_config();
        let repo = SubAgentRepository::new(&self.db);
        let _ = repo.upsert(&storage_config);
        if let Ok(None) = repo.get_metrics(&agent_id) {
            let _ = repo.upsert_metrics(&AgentMetrics::new_empty(&agent_id));
        }

        Ok(agent_id)
    }

    /// Create an agent from raw TOML content.
    pub fn create_agent_from_toml(&self, toml_content: &str) -> Result<String, String> {
        let config: AgentConfigFile =
            toml::from_str(toml_content).map_err(|e| format!("Invalid TOML: {e}"))?;
        self.create_agent(config)
    }

    /// Delete (archive) an agent.
    pub fn delete_agent(&self, id: &str) -> Result<(), String> {
        validate_agent_id("Agent", id)?;

        // Check agent exists
        if self.registry.get(id).is_none() {
            return Err("Agent not found".to_string());
        }

        // 1. Archive TOML file
        let toml_path = self.config_dir.join(format!("{id}.toml"));
        if toml_path.exists() {
            let archive_dir = self.config_dir.join(".archived");
            std::fs::create_dir_all(&archive_dir)
                .map_err(|e| format!("Failed to create archive dir: {e}"))?;
            let timestamp = chrono::Utc::now().format("%Y%m%d%H%M%S");
            let archive_path = archive_dir.join(format!("{id}.{timestamp}.toml"));
            std::fs::rename(&toml_path, &archive_path)
                .map_err(|e| format!("Failed to archive config: {e}"))?;
        }

        // 2. Remove from in-memory registry
        self.registry.remove(id);

        // 3. Update DB status to archived
        let repo = SubAgentRepository::new(&self.db);
        let _ = repo.update_status(id, "archived", None);

        Ok(())
    }

    // ── Template-aware CRUD ────────────────────────────────────────

    /// Get a template by id from the template catalog.
    pub fn get_template(&self, id: &str) -> Result<AgentTemplate, String> {
        self.registry
            .get_template(id)
            .ok_or_else(|| format!("Template '{}' not found", id))
    }

    /// List all registered templates.
    pub fn list_templates(&self) -> Vec<AgentTemplate> {
        self.registry.list_templates()
    }

    /// Create a template from an `AgentTemplate`.
    ///
    /// Write-Ahead: Markdown file → template catalog → legacy instance → DB.
    pub fn create_template(&self, template: AgentTemplate) -> Result<String, String> {
        let template_id = template.frontmatter.id.clone();
        validate_agent_id("Template", &template_id)?;

        // Check not already registered
        if self.registry.get_template(&template_id).is_some() {
            return Err(format!("Template '{}' already exists", template_id));
        }

        // 1. Write Markdown to disk (Write-Ahead)
        std::fs::create_dir_all(&self.config_dir)
            .map_err(|e| format!("Failed to create config dir: {e}"))?;
        let md_content = render_agent_markdown(&template);
        let md_path = self.config_dir.join(format!("{template_id}.md"));
        std::fs::write(&md_path, &md_content)
            .map_err(|e| format!("Failed to write template: {e}"))?;

        // 2. Register in template catalog
        self.registry.register_template(template.clone());

        // 3. Register as legacy instance for backward compat
        let mut idle_agent = template.to_subagent(&template_id, "");
        idle_agent.status = super::AgentStatus::Idle;
        idle_agent.current_task = None;
        self.registry.register(idle_agent);

        // 4. Persist to DB + init metrics
        let config = AgentConfigFile::from_template(&template);
        let storage_config = config.into_storage_config();
        let repo = SubAgentRepository::new(&self.db);
        let _ = repo.upsert(&storage_config);
        if let Ok(None) = repo.get_metrics(&template_id) {
            let _ = repo.upsert_metrics(&AgentMetrics::new_empty(&template_id));
        }

        Ok(template_id)
    }

    /// Create a template from raw Markdown content.
    pub fn create_template_from_markdown(&self, markdown: &str) -> Result<String, String> {
        let template = template::parse_agent_markdown(markdown)
            .map_err(|e| format!("Invalid agent markdown: {e}"))?;
        self.create_template(template)
    }

    /// Create a template from a TOML `AgentConfigFile`.
    ///
    /// Converts the TOML config to a template, writes .md, and registers it.
    /// This is the migration bridge: old TOML → new Markdown.
    pub fn create_template_from_toml_config(
        &self,
        config: AgentConfigFile,
    ) -> Result<String, String> {
        let template = config.into_template();
        self.create_template(template)
    }

    /// Update an existing template.
    ///
    /// Write-Ahead: Markdown file → template catalog → legacy instance → DB.
    pub fn update_template(&self, id: &str, template: AgentTemplate) -> Result<(), String> {
        validate_agent_id("Template", id)?;

        // Verify template exists
        if self.registry.get_template(id).is_none() {
            return Err(format!("Template '{}' not found", id));
        }

        // Ensure the template id matches the target
        if template.frontmatter.id != id {
            return Err(format!(
                "Template id mismatch: expected '{}', got '{}'",
                id, template.frontmatter.id
            ));
        }

        // 1. Write Markdown to disk (Write-Ahead)
        let md_content = render_agent_markdown(&template);
        let md_path = self.config_dir.join(format!("{id}.md"));
        std::fs::write(&md_path, &md_content)
            .map_err(|e| format!("Failed to write template: {e}"))?;

        // 2. Re-register template (replaces old one)
        self.registry.register_template(template.clone());

        // 3. Update legacy instance via config bridge
        let config = AgentConfigFile::from_template(&template);
        let subagent = config.clone().into_subagent();
        // Use update_config with version 0 to force-update (no optimistic locking for templates)
        if let Ok((_agent, version)) = self.registry.get_with_version(id).ok_or(()) {
            let _ = self.registry.update_config(id, subagent, version);
        }

        // 4. Persist to DB
        let storage_config = config.into_storage_config();
        let repo = SubAgentRepository::new(&self.db);
        let _ = repo.upsert(&storage_config);

        Ok(())
    }

    /// Delete (archive) a template.
    ///
    /// Fails if any busy instances are currently spawned from this template.
    pub fn delete_template(&self, id: &str) -> Result<(), String> {
        validate_agent_id("Template", id)?;

        // Check template exists
        if self.registry.get_template(id).is_none() {
            return Err(format!("Template '{}' not found", id));
        }

        // Reject deletion if any busy instances exist for this template
        let busy_count = self
            .registry
            .list_instances()
            .iter()
            .filter(|a| a.template_id == id && !a.status.is_available())
            .count();
        if busy_count > 0 {
            return Err(format!(
                "Cannot delete template '{}': {} busy instance(s)",
                id, busy_count
            ));
        }

        // 1. Archive Markdown file
        let md_path = self.config_dir.join(format!("{id}.md"));
        if md_path.exists() {
            let archive_dir = self.config_dir.join(".archived");
            std::fs::create_dir_all(&archive_dir)
                .map_err(|e| format!("Failed to create archive dir: {e}"))?;
            let timestamp = chrono::Utc::now().format("%Y%m%d%H%M%S");
            let archive_path = archive_dir.join(format!("{id}.{timestamp}.md"));
            std::fs::rename(&md_path, &archive_path)
                .map_err(|e| format!("Failed to archive template: {e}"))?;
        }

        // Also archive any leftover TOML file
        let toml_path = self.config_dir.join(format!("{id}.toml"));
        if toml_path.exists() {
            let archive_dir = self.config_dir.join(".archived");
            std::fs::create_dir_all(&archive_dir)
                .map_err(|e| format!("Failed to create archive dir: {e}"))?;
            let timestamp = chrono::Utc::now().format("%Y%m%d%H%M%S");
            let archive_path = archive_dir.join(format!("{id}.{timestamp}.toml"));
            let _ = std::fs::rename(&toml_path, &archive_path);
        }

        // 2. Remove from template catalog + instance registry
        self.registry.remove_template(id);
        self.registry.remove(id);

        // 3. Update DB status to archived
        let repo = SubAgentRepository::new(&self.db);
        let _ = repo.update_status(id, "archived", None);

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::config::{AgentCapabilitiesConfig, AgentMeta, AgentPresetConfig};
    use tempfile::TempDir;

    /// A service whose `config_dir` is `<tmp>/root/config`, so a traversal that
    /// escaped it would land at `<tmp>/root` or `<tmp>` — both asserted empty.
    fn service() -> (TempDir, AgentConfigService) {
        let tmp = TempDir::new().expect("tempdir");
        let config_dir = tmp.path().join("root").join("config");
        std::fs::create_dir_all(&config_dir).expect("config dir");
        let db = Database::open(&tmp.path().join("test.db")).expect("db");
        let service = AgentConfigService::new(Arc::new(AgentRegistry::new()), config_dir, db);
        (tmp, service)
    }

    fn config_with_id(id: &str) -> AgentConfigFile {
        AgentConfigFile {
            agent: AgentMeta {
                id: id.to_string(),
                name: "Test Agent".to_string(),
                description: "A test agent".to_string(),
                icon: None,
            },
            capabilities: AgentCapabilitiesConfig {
                assigned: vec!["web_search".to_string()],
                denied: None,
            },
            preset: AgentPresetConfig {
                persona: "You are a test assistant.".to_string(),
                temperature: None,
                verbosity: None,
            },
            constraints: None,
            llm: None,
        }
    }

    /// Every file under `dir`, recursively, as paths relative to it.
    fn tree(dir: &std::path::Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(next) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&next) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else {
                    out.push(path.strip_prefix(dir).unwrap_or(&path).to_path_buf());
                }
            }
        }
        out.sort();
        out
    }

    /// The ids of the nine shipped `config/agents/*.md` templates, verbatim —
    /// the guard must not reject anything that already ships.
    #[test]
    fn shipped_template_ids_are_accepted() {
        for id in [
            "code_agent",
            "explore_agent",
            "general_agent",
            "lead_agent",
            "planning_agent",
            "research_agent",
            "review_agent",
            "system_agent",
            "writing_agent",
        ] {
            assert!(
                validate_agent_id("Template", id).is_ok(),
                "shipped template id '{id}' must stay valid"
            );
        }
    }

    #[test]
    fn create_agent_writes_the_config_and_registers_it() {
        let (tmp, service) = service();

        let id = service
            .create_agent(config_with_id("test_agent"))
            .expect("create");

        assert_eq!(id, "test_agent");
        assert_eq!(
            tree(&tmp.path().join("root").join("config")),
            vec![PathBuf::from("test_agent.toml")]
        );
        assert!(service.registry.get("test_agent").is_some());
    }

    /// The table of ids the guard exists for. Each must be refused with nothing
    /// written anywhere under the temp root and nothing left in the registry.
    #[test]
    fn create_agent_refuses_an_unsafe_id() {
        let long_id = "a".repeat(MAX_AGENT_ID_LEN + 1);
        let cases: Vec<(&str, &str)> = vec![
            ("../../x", "traversal"),
            ("a/b", "separator"),
            ("a\\b", "windows separator"),
            ("/etc/passwd", "absolute path"),
            ("", "empty"),
            (long_id.as_str(), "too long"),
            ("a\u{7f}b", "control character"),
            ("a\nb", "newline"),
            (".hidden", "leading dot"),
            ("agent\u{0}", "NUL"),
        ];

        for (id, why) in cases {
            let (tmp, service) = service();

            let err = service
                .create_agent(config_with_id(id))
                .expect_err(&format!("{why} id must be refused"));
            assert!(
                err.contains("Agent id"),
                "{why}: error should name the id rule, got: {err}"
            );

            assert!(
                tree(tmp.path()).iter().all(|p| p.starts_with("test.db")),
                "{why}: nothing but the test database may be written, found {:?}",
                tree(tmp.path())
            );
            assert!(
                service.registry.get(id).is_none(),
                "{why}: nothing may be registered"
            );
            assert!(
                service.registry.list_instances().is_empty(),
                "{why}: the registry must stay empty"
            );
        }
    }

    #[test]
    fn create_agent_from_toml_inherits_the_guard() {
        let (tmp, service) = service();

        let err = service
            .create_agent_from_toml(
                r#"
[agent]
id = "../../escape"
name = "Escape"
description = "d"

[capabilities]
assigned = []

[preset]
persona = "p"
"#,
            )
            .expect_err("traversal id must be refused");

        assert!(err.contains("Agent id"), "unexpected error: {err}");
        assert!(!tmp.path().join("escape.toml").exists());
    }

    #[test]
    fn create_template_writes_the_markdown_and_registers_it() {
        let (tmp, service) = service();

        let id = service
            .create_template_from_markdown(
                "---\nid: \"test_agent\"\nname: \"Test\"\ndescription: \"d\"\n---\n\nBody.\n",
            )
            .expect("create");

        assert_eq!(id, "test_agent");
        assert_eq!(
            tree(&tmp.path().join("root").join("config")),
            vec![PathBuf::from("test_agent.md")]
        );
        assert!(service.registry.get_template("test_agent").is_some());
    }

    #[test]
    fn create_template_refuses_an_unsafe_id() {
        let (tmp, service) = service();

        let err = service
            .create_template_from_markdown(
                "---\nid: \"../../escape\"\nname: \"Escape\"\ndescription: \"d\"\n---\n\nBody.\n",
            )
            .expect_err("traversal id must be refused");

        assert!(err.contains("Template id"), "unexpected error: {err}");
        assert!(tree(tmp.path()).iter().all(|p| p.starts_with("test.db")));
        assert!(service.registry.list_templates().is_empty());
    }

    /// `delete_*` renames a file built from the id, so it is guarded too — and
    /// the guard runs before the existence check, so an unsafe id can never
    /// reach `std::fs::rename` even if something registered it.
    #[test]
    fn delete_refuses_an_unsafe_id_before_touching_disk() {
        let (tmp, service) = service();
        let outside = tmp.path().join("escape.toml");
        std::fs::write(&outside, "victim").expect("write victim");

        let err = service.delete_agent("../../escape").expect_err("refused");
        assert!(err.contains("Agent id"), "unexpected error: {err}");

        let err = service.delete_template("../../escape").expect_err("refused");
        assert!(err.contains("Template id"), "unexpected error: {err}");

        assert_eq!(
            std::fs::read_to_string(&outside).expect("victim survives"),
            "victim"
        );
    }

    #[test]
    fn update_refuses_an_unsafe_id() {
        let (_tmp, service) = service();

        let err = service
            .update_agent_config("../../escape", config_with_id("escape"), 0)
            .expect_err("refused");
        assert!(err.contains("Agent id"), "unexpected error: {err}");
    }
}

//! AgentConfigService — CRUD operations with Write-Ahead + optimistic locking.
//!
//! Supports both legacy TOML agent configs and new Markdown agent templates.

use super::config::AgentConfigFile;
use super::registry::AgentRegistry;
use super::template::{self, AgentTemplate, render_agent_markdown};
use openalpaca_storage::{AgentMetrics, Database, SubAgentRepository};
use std::path::PathBuf;
use std::sync::Arc;

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
        // 1. Write TOML to disk (Write-Ahead)
        let toml_content = toml::to_string_pretty(&config)
            .map_err(|e| format!("Failed to serialize config: {e}"))?;
        let toml_path = self.config_dir.join(format!("{id}.toml"));
        std::fs::write(&toml_path, &toml_content)
            .map_err(|e| format!("Failed to write config: {e}"))?;

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

        // Check not already registered
        if self.registry.get(&agent_id).is_some() {
            return Err(format!("Agent '{}' already exists", agent_id));
        }

        // 1. Write TOML to disk (Write-Ahead)
        std::fs::create_dir_all(&self.config_dir)
            .map_err(|e| format!("Failed to create config dir: {e}"))?;
        let toml_content = toml::to_string_pretty(&config)
            .map_err(|e| format!("Failed to serialize config: {e}"))?;
        let toml_path = self.config_dir.join(format!("{agent_id}.toml"));
        std::fs::write(&toml_path, &toml_content)
            .map_err(|e| format!("Failed to write config: {e}"))?;

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
        let config: AgentConfigFile = toml::from_str(toml_content)
            .map_err(|e| format!("Invalid TOML: {e}"))?;
        self.create_agent(config)
    }

    /// Delete (archive) an agent.
    pub fn delete_agent(&self, id: &str) -> Result<(), String> {
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
    pub fn create_template_from_toml_config(&self, config: AgentConfigFile) -> Result<String, String> {
        let template = config.into_template();
        self.create_template(template)
    }

    /// Update an existing template.
    ///
    /// Write-Ahead: Markdown file → template catalog → legacy instance → DB.
    pub fn update_template(&self, id: &str, template: AgentTemplate) -> Result<(), String> {
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
    pub fn delete_template(&self, id: &str) -> Result<(), String> {
        // Check template exists
        if self.registry.get_template(id).is_none() {
            return Err(format!("Template '{}' not found", id));
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

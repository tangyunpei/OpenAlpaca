//! AgentConfigService — CRUD operations with Write-Ahead + optimistic locking.
//!
//! Supports both legacy TOML agent configs and new Markdown agent templates.

use super::config::AgentConfigFile;
use super::registry::{AgentRegistry, TemplateLayer};
use super::template::{self, AgentTemplate, render_agent_markdown};
use openalpaca_storage::{AgentMetrics, Database, SubAgentRepository};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Why `delete_template` refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeleteTemplateError {
    /// The id is a bundled template no file of the person's holds: a
    /// built-in template cannot be removed, only overridden or switched
    /// off by a file saying `disabled: true` (subagent design §2.10 T-2,
    /// §2.4 L-4a).
    Bundled,
    /// Any other refusal or failure, worded for the person.
    Other(String),
}

impl From<String> for DeleteTemplateError {
    fn from(message: String) -> Self {
        Self::Other(message)
    }
}

impl std::fmt::Display for DeleteTemplateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bundled => f.write_str(
                "a built-in template cannot be removed; write a file of the same id in \
                 config/agents to override it, or one holding its id and `disabled: true` to \
                 switch it off",
            ),
            Self::Other(message) => f.write_str(message),
        }
    }
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

    /// The `agents/` directory the template files are written in — the one
    /// the file layer's rescan reads (subagent design §2.4 L-3).
    pub fn agents_dir(&self) -> &Path {
        &self.config_dir
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
        // 1. Write TOML to disk (Write-Ahead) — only when the id is no
        // template of any layer and no .md file holds it: a template's file
        // (or, for a bundled one, the binary) is its source of truth, and a
        // `<id>.toml` written for a bundled id would be read at the next
        // start as a legacy agent the singleton lead could claim (subagent
        // design §2.4 L-1a).
        let md_path = self.config_dir.join(format!("{id}.md"));
        if self.registry.provenance(id).is_none() && !md_path.exists() {
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

    /// Create a template from an `AgentTemplate`: its Markdown file is
    /// written, and nothing else. The caller rescans the file layer
    /// (`agent::file_layer::rescan_file_layer`), which puts it in the
    /// catalog, writes its `agent` row and registers its idle instance
    /// (subagent design §2.10 T-1).
    ///
    /// A bundled id with no file of the person's is not "already there":
    /// the file written is an override of the bundled template. It is
    /// `<id>.md`, so where that name holds an unedited copy set aside, the
    /// copy is rewritten — the person asked for the write.
    pub fn create_template(&self, template: AgentTemplate) -> Result<String, String> {
        let template_id = template.frontmatter.id.clone();

        // Refuse an id the file or plugin layer holds, a withheld file
        // included; a bundled one takes an override.
        if self
            .registry
            .provenance(&template_id)
            .is_some_and(|p| p.layer != TemplateLayer::Bundled)
        {
            return Err(format!("Template '{}' already exists", template_id));
        }

        std::fs::create_dir_all(&self.config_dir)
            .map_err(|e| format!("Failed to create config dir: {e}"))?;
        let md_content = render_agent_markdown(&template);
        let md_path = self.config_dir.join(format!("{template_id}.md"));
        std::fs::write(&md_path, &md_content)
            .map_err(|e| format!("Failed to write template: {e}"))?;

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

    /// Update an existing template: its Markdown file is rewritten, and
    /// nothing else. The caller rescans the file layer, which puts the new
    /// definition in the catalog, its `agent` row and its idle instance — the
    /// singleton lead's included (subagent design §2.10 T-1, §2.4 L-1a).
    ///
    /// The file rewritten is the one that holds the id, whatever it is
    /// called (a template's identity is its `id:`); a bundled template with
    /// no file of the person's gets `<id>.md`, an override — over an
    /// unedited copy set aside under that name, which is then the person's.
    pub fn update_template(&self, id: &str, template: AgentTemplate) -> Result<(), String> {
        // Verify template exists (a withheld file's id included: rewriting
        // it is how a route fixes it)
        let Some(provenance) = self.registry.provenance(id) else {
            return Err(format!("Template '{}' not found", id));
        };

        // Ensure the template id matches the target
        if template.frontmatter.id != id {
            return Err(format!(
                "Template id mismatch: expected '{}', got '{}'",
                id, template.frontmatter.id
            ));
        }

        let md_content = render_agent_markdown(&template);
        let md_path = match provenance.path {
            Some(path) if provenance.layer == TemplateLayer::File => path,
            _ => self.config_dir.join(format!("{id}.md")),
        };
        std::fs::create_dir_all(&self.config_dir)
            .map_err(|e| format!("Failed to create config dir: {e}"))?;
        std::fs::write(&md_path, &md_content)
            .map_err(|e| format!("Failed to write template: {e}"))?;

        Ok(())
    }

    /// Delete (archive) a template: the file that holds its id, whatever it
    /// is called, moves to `.archived/<file stem>.<UTC timestamp>.md` and its
    /// `agent` row reads `archived`. The caller rescans the file layer, which
    /// takes the template and its idle instance out of the catalog — or
    /// brings back the bundled template the file overrode or switched off
    /// (§2.10 T-2, §2.4 L-4a), whose row the rescan then sets back.
    ///
    /// Refused for a bundled template no file holds
    /// (`DeleteTemplateError::Bundled`), and if any busy instances are
    /// currently spawned from this template.
    pub fn delete_template(&self, id: &str) -> Result<(), DeleteTemplateError> {
        // Check template exists (a withheld file's id included)
        let Some(provenance) = self.registry.provenance(id) else {
            return Err(format!("Template '{}' not found", id).into());
        };
        if provenance.layer == TemplateLayer::Bundled {
            return Err(DeleteTemplateError::Bundled);
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
            )
            .into());
        }

        // 1. Archive the Markdown file that holds the id (`<id>.md` for a
        // template no file is known for: a plugin's leaves nothing to move)
        let md_path = match &provenance.path {
            Some(path) if provenance.layer == TemplateLayer::File => path.clone(),
            _ => self.config_dir.join(format!("{id}.md")),
        };
        if md_path.exists() {
            let archive_dir = self.config_dir.join(".archived");
            std::fs::create_dir_all(&archive_dir)
                .map_err(|e| format!("Failed to create archive dir: {e}"))?;
            let timestamp = chrono::Utc::now().format("%Y%m%d%H%M%S");
            let stem = md_path
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or_else(|| id.to_string());
            let archive_path = archive_dir.join(format!("{stem}.{timestamp}.md"));
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

        // 2. Update DB status to archived — unless a bundled template stands
        // under the file, overridden or switched off by it: the id is then in
        // the catalog after the rescan, which writes the bundled template's
        // definition into its row.
        if !self.registry.is_bundled(id) {
            let repo = SubAgentRepository::new(&self.db);
            let _ = repo.update_status(id, "archived", None);
        }

        Ok(())
    }
}

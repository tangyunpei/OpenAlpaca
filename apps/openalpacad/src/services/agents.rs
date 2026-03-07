//! Agent template loading from .md and legacy .toml config files.

use openalpaca_core::context::SharedContext;
use openalpaca_storage::Database;
use std::path::Path;
use std::sync::Arc;
use tracing::{info, warn};

pub(super) fn load_agent_templates(
    config_base_dir: &Path,
    db: &Database,
    shared_context: &Arc<SharedContext>,
) -> anyhow::Result<()> {
    let config_dir = config_base_dir.join("agents");
    if !config_dir.exists() {
        return Ok(());
    }
    let entries = match std::fs::read_dir(&config_dir) {
        Ok(e) => e,
        Err(_) => return Ok(()),
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let ext = path.extension().and_then(|e| e.to_str());

        match ext {
            // ── New: Markdown templates (.md) ────────────────────
            Some("md") => {
                match std::fs::read_to_string(&path) {
                    Ok(content) => {
                        match openalpaca_core::agent::template::parse_agent_markdown(&content) {
                            Ok(template) => {
                                let template_id = template.frontmatter.id.clone();
                                let is_singleton = template.frontmatter.singleton;

                                // Register template in the template catalog
                                shared_context
                                    .agent_registry
                                    .register_template(template.clone());

                                // For singleton templates, also register as a legacy
                                // agent so existing code (config service, REST API) can
                                // find it by template_id until fully migrated.
                                let subagent = template.to_subagent(&template_id, "");
                                // Reset to Idle (to_subagent sets Busy)
                                let mut idle_agent = subagent;
                                idle_agent.status = openalpaca_core::agent::AgentStatus::Idle;
                                idle_agent.current_task = None;
                                shared_context.agent_registry.register(idle_agent);

                                // Persist template metadata to DB as SubAgentConfig
                                persist_template_to_db(db, &template, &template_id);

                                info!(
                                    "Loaded agent template: {} (singleton={})",
                                    path.display(),
                                    is_singleton
                                );
                            }
                            Err(e) => {
                                warn!("Failed to parse agent template {}: {}", path.display(), e);
                            }
                        }
                    }
                    Err(e) => {
                        warn!("Failed to read agent template {}: {}", path.display(), e);
                    }
                }
            }

            // ── Legacy: TOML configs (.toml) — backward compat ───
            Some("toml") => {
                match std::fs::read_to_string(&path) {
                    Ok(content) => {
                        match toml::from_str::<openalpaca_core::agent::AgentConfigFile>(&content) {
                            Ok(agent_config) => {
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

            _ => {} // skip non-agent files
        }
    }

    Ok(())
}

fn persist_template_to_db(
    db: &Database,
    template: &openalpaca_core::agent::template::AgentTemplate,
    template_id: &str,
) {
    let repo = openalpaca_storage::SubAgentRepository::new(db);
    let persona = openalpaca_core::agent::template::extract_persona(template);
    let fm = &template.frontmatter;
    let skills: Vec<openalpaca_core::agent::Skill> = fm
        .skills
        .iter()
        .map(|s| openalpaca_core::agent::Skill {
            name: s.clone(),
            category: "assigned".to_string(),
            proficiency: 1.0,
        })
        .collect();
    let preset = openalpaca_core::agent::AgentPreset {
        persona: persona.clone(),
        temperature: fm.temperature,
        verbosity: fm.verbosity.clone(),
    };
    let constraints = openalpaca_core::agent::AgentConstraints {
        max_tool_calls: fm.max_tool_calls,
        timeout_seconds: fm.timeout_seconds,
        max_cost_per_task: fm.max_cost_per_task,
        require_confirmation_for: fm.require_confirmation_for.clone(),
        allowed_capabilities: fm.skills.clone(),
        denied_capabilities: fm.denied_skills.clone(),
        ..Default::default()
    };
    let llm_config = openalpaca_core::agent::AgentLlmConfig {
        model: fm.model.clone(),
        fallback_models: fm.fallback_models.clone(),
        ..Default::default()
    };
    let now = chrono::Utc::now();
    let storage_config = openalpaca_storage::SubAgentConfig {
        id: template_id.to_string(),
        template_id: template_id.to_string(),
        name: fm.name.clone(),
        description: Some(fm.description.clone()),
        icon: fm.icon.clone(),
        status: "idle".to_string(),
        current_task_id: None,
        skills_json: serde_json::to_string(&skills).unwrap_or_else(|_| "[]".to_string()),
        preset_json: serde_json::to_string(&preset).unwrap_or_else(|_| "{}".to_string()),
        constraints_json: Some(
            serde_json::to_string(&constraints).unwrap_or_else(|_| "{}".to_string()),
        ),
        llm_config_json: Some(
            serde_json::to_string(&llm_config).unwrap_or_else(|_| "{}".to_string()),
        ),
        persona: Some(persona),
        created_at: now,
        updated_at: Some(now),
    };
    let _ = repo.upsert(&storage_config);

    // Initialize metrics row if not exists
    if let Ok(None) = repo.get_metrics(template_id) {
        let _ = repo.upsert_metrics(&openalpaca_storage::AgentMetrics::new_empty(template_id));
    }
}

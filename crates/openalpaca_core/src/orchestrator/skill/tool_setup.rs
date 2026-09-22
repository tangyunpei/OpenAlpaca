//! Common skill tool mechanics. Caller-specific constraints and executor ownership stay at the call sites.
use super::catalog::SkillCatalog;
use super::invoke_executor::{SkillInvocationBuiltInAdapter, SkillInvocationToolExecutor};
use crate::middleware::skill::{ScriptConfig, SkillFrontmatter};
use crate::tools::builtins::ScriptToolBuiltIn;
use crate::tools::registry::{RegisteredTool, ToolBackend, ToolContext, ToolRegistry};
use openalpaca_llm::ToolDefinition;
use std::path::Path;
use std::sync::Arc;

/// Resolve the base surface freshly and attribute unavailable extensions.
/// Never merges intent suggestions into the skill's declared tools.
pub(super) fn resolve_skill_tools(
    registry: &ToolRegistry,
    frontmatter: &SkillFrontmatter,
    skill: &str,
    context: Option<&ToolContext>,
    scope: Option<&str>,
) -> Vec<ToolDefinition> {
    if !frontmatter.requires_capabilities.is_empty() {
        let resolution = registry.resolve_capabilities(&frontmatter.requires_capabilities, &[]);
        registry.announce_withheld(&resolution, context, scope);
        return resolution.defs;
    }
    let names = &frontmatter.tools.allow;
    let resolved: Vec<_> = names
        .iter()
        .filter_map(|name| registry.get(name).map(|tool| tool.definition))
        .collect();
    if resolved.len() < names.len() {
        let missing = names
            .iter()
            .filter(|name| !resolved.iter().any(|tool| &tool.name == *name))
            .map(String::as_str);
        let unattributed = registry.announce_withheld_names(missing, context, scope);
        if !unattributed.is_empty() {
            tracing::warn!(
                "Skill '{}' references unknown tools: {:?}",
                skill,
                unattributed
            );
        }
    }
    resolved
}

pub(super) fn dependency_definition(skill: &str, description: String) -> ToolDefinition {
    ToolDefinition {
        name: format!("invoke_skill:{skill}"),
        description,
        parameters: serde_json::json!({
            "type": "object",
            "properties": { "query": { "type": "string", "description": "The input/query to pass to the skill" } },
            "required": ["query"]
        }),
        strict: None,
        input_examples: None,
    }
}

pub(super) fn register_scripts(
    registry: &ToolRegistry,
    skill_dir: Option<&Path>,
    scripts: &[ScriptConfig],
    skill: &str,
) -> Result<(), String> {
    if let Some(skill_dir) = skill_dir {
        for config in scripts {
            let tool = ScriptToolBuiltIn::new(skill_dir, config)?;
            registry.register(RegisteredTool {
                // Keep the runtime schema permissive; changing it to the advertised
                // ScriptConfig schema is a separate validation-policy change.
                definition: ScriptToolBuiltIn::tool_definition(&config.name),
                backend: ToolBackend::BuiltIn(Arc::new(tool)),
                provides_capabilities: vec![],
                exempt_from_timeout: false,
                annotations: None,
                version: env!("CARGO_PKG_VERSION").to_string(),
                author: format!("skill:{skill}"),
                created_at: chrono::Utc::now(),
            })?;
        }
    }
    Ok(())
}

pub(super) fn register_dependencies(
    registry: &ToolRegistry,
    catalog: &SkillCatalog,
    dependencies: &[String],
    executor: Arc<SkillInvocationToolExecutor>,
    skill: &str,
) -> Result<(), String> {
    for dependency in dependencies {
        if catalog.get(dependency).is_some() {
            registry.register(RegisteredTool {
                definition: dependency_definition(
                    dependency,
                    format!("Invoke the '{}' skill", dependency),
                ),
                backend: ToolBackend::BuiltIn(Arc::new(SkillInvocationBuiltInAdapter {
                    executor: executor.clone(),
                    skill_id: dependency.clone(),
                })),
                provides_capabilities: vec![],
                exempt_from_timeout: true,
                annotations: None,
                version: env!("CARGO_PKG_VERSION").to_string(),
                author: format!("skill:{skill}"),
                created_at: chrono::Utc::now(),
            })?;
        }
    }
    Ok(())
}

//! Agent template definition: Markdown + YAML frontmatter parser.
//!
//! An agent template is a Markdown file (`AGENT.md`) with YAML frontmatter
//! defining the agent's capabilities, constraints, and LLM configuration.
//! The markdown body provides the persona and additional instructions.
//!
//! Follows the same two-level loading pattern as `middleware/skill.rs`:
//! - Level 1: frontmatter only (lightweight catalog scan at startup)
//! - Level 2: full document with body sections (on demand)

use std::collections::HashMap;
use std::fmt;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// YAML frontmatter metadata for an agent template.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentTemplateFrontmatter {
    /// Unique template identifier (e.g. "code_agent").
    pub id: String,
    /// Human-readable name (e.g. "Code Agent").
    pub name: String,
    /// Short description for catalog display.
    pub description: String,
    /// Optional icon identifier.
    pub icon: Option<String>,
    /// If true, only one instance can be active at a time.
    pub singleton: bool,
    /// Tool/skill names this agent can use.
    pub skills: Vec<String>,
    /// Tool/skill names explicitly denied.
    pub denied_skills: Vec<String>,
    /// LLM temperature (default 0.5).
    pub temperature: f32,
    /// Verbosity level: "concise", "normal", "detailed" (default "normal").
    pub verbosity: String,
    /// Primary LLM model override.
    pub model: Option<String>,
    /// Fallback models if primary is unavailable.
    pub fallback_models: Vec<String>,
    /// Maximum tool calls per task.
    pub max_tool_calls: Option<u32>,
    /// Task timeout in seconds.
    pub timeout_seconds: Option<u64>,
    /// Maximum cost per task in USD.
    pub max_cost_per_task: Option<f64>,
    /// Maximum agentic loop rounds (overrides daemon default).
    pub max_rounds: Option<usize>,
    /// Tools requiring user confirmation before execution.
    pub require_confirmation_for: Vec<String>,
}

/// Full parsed agent template document.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentTemplate {
    pub frontmatter: AgentTemplateFrontmatter,
    /// The full markdown body (persona, guidelines, examples).
    pub body: String,
    /// Parsed `## Section` headings -> body text.
    pub sections: HashMap<String, String>,
}

/// Errors that can occur while parsing an agent template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentParseError {
    MissingFrontmatter,
    UnterminatedFrontmatter,
    MissingField(&'static str),
    InvalidValue {
        field: &'static str,
        message: String,
    },
}

impl fmt::Display for AgentParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingFrontmatter => write!(f, "Missing YAML frontmatter"),
            Self::UnterminatedFrontmatter => write!(f, "Unterminated YAML frontmatter"),
            Self::MissingField(field) => {
                write!(f, "Missing required frontmatter field '{}'", field)
            }
            Self::InvalidValue { field, message } => {
                write!(f, "Invalid value for '{}': {}", field, message)
            }
        }
    }
}

impl std::error::Error for AgentParseError {}

// ---------------------------------------------------------------------------
// Frontmatter helpers (same patterns as middleware/skill.rs)
// ---------------------------------------------------------------------------

fn strip_outer_quotes(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.len() >= 2
        && ((trimmed.starts_with('"') && trimmed.ends_with('"'))
            || (trimmed.starts_with('\'') && trimmed.ends_with('\'')))
    {
        trimmed[1..trimmed.len() - 1].trim().to_string()
    } else {
        trimmed.to_string()
    }
}

fn split_frontmatter(input: &str) -> Result<(Vec<String>, Vec<String>), AgentParseError> {
    let mut lines = input.lines();
    let first = lines.next().unwrap_or_default();
    if first.trim() != "---" {
        return Err(AgentParseError::MissingFrontmatter);
    }

    let mut frontmatter = Vec::new();
    let mut body = Vec::new();
    let mut in_frontmatter = true;

    for line in lines {
        if in_frontmatter {
            if line.trim() == "---" {
                in_frontmatter = false;
                continue;
            }
            frontmatter.push(line.to_string());
            continue;
        }
        body.push(line.to_string());
    }

    if in_frontmatter {
        return Err(AgentParseError::UnterminatedFrontmatter);
    }

    Ok((frontmatter, body))
}

/// Parse a YAML list of `- "item"` lines starting at `idx + 1`.
fn parse_yaml_list(lines: &[String], idx: &mut usize) -> Vec<String> {
    let mut items = Vec::new();
    *idx += 1;
    while *idx < lines.len() {
        let item = lines[*idx].trim();
        if item.is_empty() {
            *idx += 1;
            continue;
        }
        if let Some(v) = item.strip_prefix("- ") {
            items.push(strip_outer_quotes(v));
            *idx += 1;
            continue;
        }
        break; // Non-list-item line -> stop
    }
    items
}

/// Parse a scalar boolean value from YAML.
fn parse_yaml_bool(value: &str) -> bool {
    let v = strip_outer_quotes(value).to_lowercase();
    matches!(v.as_str(), "true" | "yes" | "1")
}

/// Parse a scalar f32 from YAML.
fn parse_yaml_f32(value: &str) -> Option<f32> {
    strip_outer_quotes(value).parse::<f32>().ok()
}

/// Parse a scalar u32 from YAML.
fn parse_yaml_u32(value: &str) -> Option<u32> {
    strip_outer_quotes(value).parse::<u32>().ok()
}

/// Parse a scalar u64 from YAML.
fn parse_yaml_u64(value: &str) -> Option<u64> {
    strip_outer_quotes(value).parse::<u64>().ok()
}

/// Parse a scalar f64 from YAML.
fn parse_yaml_f64(value: &str) -> Option<f64> {
    strip_outer_quotes(value).parse::<f64>().ok()
}

fn parse_agent_frontmatter_lines(
    lines: &[String],
) -> Result<AgentTemplateFrontmatter, AgentParseError> {
    let mut id: Option<String> = None;
    let mut name: Option<String> = None;
    let mut description: Option<String> = None;
    let mut icon: Option<String> = None;
    let mut singleton: bool = false;
    let mut skills: Vec<String> = Vec::new();
    let mut denied_skills: Vec<String> = Vec::new();
    let mut temperature: f32 = 0.5;
    let mut verbosity: String = "normal".to_string();
    let mut model: Option<String> = None;
    let mut fallback_models: Vec<String> = Vec::new();
    let mut max_tool_calls: Option<u32> = None;
    let mut timeout_seconds: Option<u64> = None;
    let mut max_cost_per_task: Option<f64> = None;
    let mut max_rounds: Option<usize> = None;
    let mut require_confirmation_for: Vec<String> = Vec::new();

    let mut idx = 0usize;
    while idx < lines.len() {
        let trimmed = lines[idx].trim();
        if trimmed.is_empty() {
            idx += 1;
            continue;
        }

        if let Some(rest) = trimmed.strip_prefix("id:") {
            id = Some(strip_outer_quotes(rest));
            idx += 1;
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("name:") {
            name = Some(strip_outer_quotes(rest));
            idx += 1;
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("description:") {
            description = Some(strip_outer_quotes(rest));
            idx += 1;
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("icon:") {
            let v = strip_outer_quotes(rest);
            if !v.is_empty() {
                icon = Some(v);
            }
            idx += 1;
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("singleton:") {
            singleton = parse_yaml_bool(rest);
            idx += 1;
            continue;
        }
        // "skills:" list (must come before "denied_skills:" check)
        if trimmed == "skills:" {
            skills = parse_yaml_list(lines, &mut idx);
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("skills: ") {
            skills = vec![strip_outer_quotes(rest)];
            idx += 1;
            continue;
        }
        if trimmed == "denied_skills:" {
            denied_skills = parse_yaml_list(lines, &mut idx);
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("denied_skills: ") {
            denied_skills = vec![strip_outer_quotes(rest)];
            idx += 1;
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("temperature:") {
            if let Some(v) = parse_yaml_f32(rest) {
                temperature = v;
            }
            idx += 1;
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("verbosity:") {
            let v = strip_outer_quotes(rest);
            if !v.is_empty() {
                verbosity = v;
            }
            idx += 1;
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("model:") {
            let v = strip_outer_quotes(rest);
            if !v.is_empty() {
                model = Some(v);
            }
            idx += 1;
            continue;
        }
        if trimmed.starts_with("fallback_models:") {
            fallback_models = parse_yaml_list(lines, &mut idx);
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("max_tool_calls:") {
            max_tool_calls = parse_yaml_u32(rest);
            idx += 1;
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("timeout_seconds:") {
            timeout_seconds = parse_yaml_u64(rest);
            idx += 1;
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("max_cost_per_task:") {
            max_cost_per_task = parse_yaml_f64(rest);
            idx += 1;
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("max_rounds:") {
            max_rounds = strip_outer_quotes(rest).parse::<usize>().ok();
            idx += 1;
            continue;
        }
        if trimmed.starts_with("require_confirmation_for:") {
            require_confirmation_for = parse_yaml_list(lines, &mut idx);
            continue;
        }

        // Unknown field — skip silently (lenient parsing)
        idx += 1;
    }

    let id = id.ok_or(AgentParseError::MissingField("id"))?;
    let name = name.ok_or(AgentParseError::MissingField("name"))?;
    let description = description.ok_or(AgentParseError::MissingField("description"))?;

    Ok(AgentTemplateFrontmatter {
        id,
        name,
        description,
        icon,
        singleton,
        skills,
        denied_skills,
        temperature,
        verbosity,
        model,
        fallback_models,
        max_tool_calls,
        timeout_seconds,
        max_cost_per_task,
        max_rounds,
        require_confirmation_for,
    })
}

// ---------------------------------------------------------------------------
// Body parsing (same as middleware/skill.rs — lenient, all sections optional)
// ---------------------------------------------------------------------------

fn parse_body_sections(lines: &[String]) -> (String, HashMap<String, String>) {
    let mut sections: HashMap<String, String> = HashMap::new();
    let mut current_section: Option<String> = None;
    let mut current_lines: Vec<String> = Vec::new();
    let mut full_body = String::new();

    for line in lines {
        // Build full body text
        if !full_body.is_empty() || !line.trim().is_empty() {
            if !full_body.is_empty() {
                full_body.push('\n');
            }
            full_body.push_str(line);
        }

        if let Some(heading) = line.trim().strip_prefix("## ") {
            // Save previous section
            if let Some(ref name) = current_section {
                let content = current_lines.join("\n").trim().to_string();
                if !content.is_empty() {
                    sections.insert(name.clone(), content);
                }
            }
            current_section = Some(heading.trim().to_string());
            current_lines.clear();
        } else if current_section.is_some() {
            current_lines.push(line.to_string());
        }
    }

    // Save last section
    if let Some(ref name) = current_section {
        let content = current_lines.join("\n").trim().to_string();
        if !content.is_empty() {
            sections.insert(name.clone(), content);
        }
    }

    let body = full_body.trim_end().to_string();
    (body, sections)
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Parse only the YAML frontmatter from an agent template file (Level 1).
///
/// Use this for lightweight catalog scanning at startup.
pub fn parse_agent_frontmatter(input: &str) -> Result<AgentTemplateFrontmatter, AgentParseError> {
    let (frontmatter_lines, _body_lines) = split_frontmatter(input)?;
    parse_agent_frontmatter_lines(&frontmatter_lines)
}

/// Parse the full agent template file including body sections (Level 2).
pub fn parse_agent_markdown(input: &str) -> Result<AgentTemplate, AgentParseError> {
    let (frontmatter_lines, body_lines) = split_frontmatter(input)?;
    let frontmatter = parse_agent_frontmatter_lines(&frontmatter_lines)?;
    let (body, sections) = parse_body_sections(&body_lines);

    Ok(AgentTemplate {
        frontmatter,
        body,
        sections,
    })
}

/// Render an `AgentTemplate` back to valid Markdown with YAML frontmatter.
///
/// Designed to round-trip: `parse_agent_markdown(render_agent_markdown(t))`
/// should produce a semantically-equal `AgentTemplate`.
pub fn render_agent_markdown(template: &AgentTemplate) -> String {
    let mut out = String::new();
    let fm = &template.frontmatter;

    // -- Frontmatter --
    out.push_str("---\n");
    out.push_str(&format!("id: \"{}\"\n", fm.id));
    out.push_str(&format!("name: \"{}\"\n", fm.name));
    out.push_str(&format!("description: \"{}\"\n", fm.description));
    if let Some(ref icon) = fm.icon {
        out.push_str(&format!("icon: \"{}\"\n", icon));
    }
    if fm.singleton {
        out.push_str("singleton: true\n");
    }
    if !fm.skills.is_empty() {
        out.push_str("skills:\n");
        for skill in &fm.skills {
            out.push_str(&format!("  - \"{}\"\n", skill));
        }
    }
    if !fm.denied_skills.is_empty() {
        out.push_str("denied_skills:\n");
        for skill in &fm.denied_skills {
            out.push_str(&format!("  - \"{}\"\n", skill));
        }
    }
    out.push_str(&format!("temperature: {}\n", fm.temperature));
    out.push_str(&format!("verbosity: \"{}\"\n", fm.verbosity));
    if let Some(ref model) = fm.model {
        out.push_str(&format!("model: \"{}\"\n", model));
    }
    if !fm.fallback_models.is_empty() {
        out.push_str("fallback_models:\n");
        for model in &fm.fallback_models {
            out.push_str(&format!("  - \"{}\"\n", model));
        }
    }
    if let Some(v) = fm.max_tool_calls {
        out.push_str(&format!("max_tool_calls: {}\n", v));
    }
    if let Some(v) = fm.timeout_seconds {
        out.push_str(&format!("timeout_seconds: {}\n", v));
    }
    if let Some(v) = fm.max_cost_per_task {
        out.push_str(&format!("max_cost_per_task: {}\n", v));
    }
    if let Some(v) = fm.max_rounds {
        out.push_str(&format!("max_rounds: {}\n", v));
    }
    if !fm.require_confirmation_for.is_empty() {
        out.push_str("require_confirmation_for:\n");
        for tool in &fm.require_confirmation_for {
            out.push_str(&format!("  - \"{}\"\n", tool));
        }
    }
    out.push_str("---\n");

    // -- Body --
    if !template.body.is_empty() {
        out.push('\n');
        out.push_str(&template.body);
        out.push('\n');
    }

    out
}

/// Extract the persona text from the template's `## Persona` section.
///
/// Returns the section content or a default persona string.
pub fn extract_persona(template: &AgentTemplate) -> String {
    template
        .sections
        .get("Persona")
        .cloned()
        .unwrap_or_else(|| "You are a helpful assistant.".to_string())
}

impl AgentTemplate {
    /// Create a `SubAgent` instance from this template.
    ///
    /// The instance gets `instance_id` as its `id`, the template's `id` as `template_id`,
    /// and is set to `Busy` with the given `task_id`.
    pub fn to_subagent(&self, instance_id: &str, task_id: &str) -> super::subagent::SubAgent {
        use super::subagent::*;

        let fm = &self.frontmatter;
        let persona = extract_persona(self);

        let mut capabilities: Vec<Capability> = fm
            .skills
            .iter()
            .map(|name| Capability {
                name: name.clone(),
                category: "assigned".to_string(),
                proficiency: 1.0,
            })
            .collect();

        // Inject workspace tools into capabilities so they appear in tool resolution
        // (resolve_agent_tools uses agent.capabilities to look up definitions).
        for ws_tool in ["workspace_read", "workspace_write"] {
            if !capabilities.iter().any(|s| s.name == ws_tool) {
                capabilities.push(Capability {
                    name: ws_tool.to_string(),
                    category: "infrastructure".to_string(),
                    proficiency: 1.0,
                });
            }
        }

        // Merge denied_skills into denied_capabilities.
        // Always include workspace tools — they are infrastructure tools
        // available to any agent executing in a task context (pipeline/DAG).
        // The ContextualToolExecutor already gates them on task_id presence.
        let mut allowed_capabilities = fm.skills.clone();
        for ws_tool in ["workspace_read", "workspace_write"] {
            if !allowed_capabilities.iter().any(|c| c == ws_tool) {
                allowed_capabilities.push(ws_tool.to_string());
            }
        }
        let denied_capabilities = fm.denied_skills.clone();

        SubAgent {
            id: instance_id.to_string(),
            template_id: fm.id.clone(),
            name: fm.name.clone(),
            description: Some(fm.description.clone()),
            icon: fm.icon.clone(),
            status: AgentStatus::Busy {
                task_id: task_id.to_string(),
            },
            current_task: Some(task_id.to_string()),
            capabilities,
            preset: AgentPreset {
                persona,
                temperature: fm.temperature,
                verbosity: fm.verbosity.clone(),
            },
            constraints: {
                let mut c = AgentConstraints {
                    max_tool_calls: fm.max_tool_calls,
                    timeout_seconds: fm.timeout_seconds,
                    max_cost_per_task: fm.max_cost_per_task,
                    max_rounds: fm.max_rounds,
                    require_confirmation_for: fm.require_confirmation_for.clone(),
                    allowed_capabilities,
                    denied_capabilities,
                    ..Default::default()
                };
                c.normalize();
                c
            },
            llm_config: AgentLlmConfig {
                model: fm.model.clone(),
                fallback_models: fm.fallback_models.clone(),
                ..Default::default()
            },
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;

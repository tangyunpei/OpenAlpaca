//! SKILL.md parser, renderer, and prompt block formatter.
//!
//! A Skill is a folder on disk containing a `SKILL.md` file with YAML frontmatter
//! and a markdown body. The frontmatter provides lightweight metadata for catalog
//! discovery (Level 1), while the full body provides instructions loaded on demand
//! (Level 2).
//!
//! Parsing is **lenient**: only `name` and `description` are required in the
//! frontmatter. All body sections are optional.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;

// ---------------------------------------------------------------------------
// Scope
// ---------------------------------------------------------------------------

/// Where a skill was loaded from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SkillScope {
    /// Project-level skills (e.g. `config/skills/`).
    Project,
    /// User-level skills (e.g. `~/.config/openalpaca/skills/`).
    User,
}

// ---------------------------------------------------------------------------
// Sub-config types
// ---------------------------------------------------------------------------

fn default_invoke_mode() -> String {
    "manual".to_string()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct InvokeConfig {
    /// "manual" | "auto" | "scheduled" | "disabled"
    #[serde(default = "default_invoke_mode")]
    pub mode: String,
    /// Slash command (e.g. "/review")
    pub slash: Option<String>,
    /// Alternative slash commands that also invoke this skill.
    pub aliases: Vec<String>,
    /// Hotkey binding
    pub hotkey: Option<String>,
    /// Cron expression for scheduled mode
    pub cron: Option<String>,
}

impl Default for InvokeConfig {
    fn default() -> Self {
        Self {
            mode: default_invoke_mode(),
            slash: None,
            aliases: Vec::new(),
            hotkey: None,
            cron: None,
        }
    }
}

fn default_base() -> f64 {
    0.2
}
fn default_intent_weight() -> f64 {
    0.45
}
fn default_keyword_weight() -> f64 {
    0.35
}
fn default_recency_weight() -> f64 {
    0.2
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScoreWeights {
    #[serde(default = "default_base")]
    pub base: f64,
    #[serde(default = "default_intent_weight")]
    pub intent_weight: f64,
    #[serde(default = "default_keyword_weight")]
    pub keyword_weight: f64,
    #[serde(default = "default_recency_weight")]
    pub recency_weight: f64,
}

impl Default for ScoreWeights {
    fn default() -> Self {
        Self {
            base: default_base(),
            intent_weight: default_intent_weight(),
            keyword_weight: default_keyword_weight(),
            recency_weight: default_recency_weight(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RoutingExamples {
    pub positive: Vec<String>,
    pub negative: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RoutingConfig {
    /// Intent patterns (regex) for auto-routing
    pub intent: Vec<String>,
    /// Keywords for keyword-based scoring
    pub keywords: Vec<String>,
    /// Negative keywords — if any match, the skill is penalized heavily.
    pub negative_keywords: Vec<String>,
    /// Score weights for routing (also accepts `score` as alias for backward compat).
    #[serde(alias = "score")]
    pub weights: ScoreWeights,
    /// Example queries for intent classification
    pub examples: RoutingExamples,
}

fn default_max_files() -> usize {
    10
}
fn default_max_bytes_each() -> usize {
    200_000
}
fn default_max_bytes() -> usize {
    50_000
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContextSource {
    File {
        path: String,
        #[serde(default = "default_max_bytes")]
        max_bytes: usize,
    },
    FileGlob {
        pattern: String,
        #[serde(default = "default_max_files")]
        max_files: usize,
        #[serde(default = "default_max_bytes_each")]
        max_bytes_each: usize,
    },
    Shell {
        command: String,
        #[serde(default = "default_max_bytes")]
        max_bytes: usize,
    },
}

// TODO(P2-3): SummarizeConfig is parsed but not yet enforced at runtime.
// When enabled, context injection should summarize large context blocks
// before injecting them into the prompt.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SummarizeConfig {
    pub enabled: bool,
    pub max_tokens: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ContextConfig {
    /// Additional context sources injected before the skill prompt
    #[serde(default)]
    pub sources: Vec<ContextSource>,
    /// Summarization settings for context
    pub summarize: SummarizeConfig,
    /// Controls when the skill description appears in the LLM catalog prompt.
    pub read_when: Vec<String>,
    /// Token budget for context injection (estimated as chars/4). 0 = default 4000.
    pub budget_tokens: usize,
}

fn default_permission_level() -> String {
    "readonly".to_string()
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ConfirmAction {
    pub tools: Vec<String>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SandboxConfig {
    pub enabled: bool,
    pub net: bool,
    pub fs_writable: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PermissionsConfig {
    /// "readonly" | "readwrite" | "admin"
    #[serde(default = "default_permission_level")]
    pub level: String,
    /// Actions that require user confirmation
    pub confirm: ConfirmAction,
    /// Sandbox settings
    pub sandbox: SandboxConfig,
}

impl Default for PermissionsConfig {
    fn default() -> Self {
        Self {
            level: default_permission_level(),
            confirm: ConfirmAction::default(),
            sandbox: SandboxConfig::default(),
        }
    }
}

// TODO(P2-3): RateLimitConfig is parsed but not yet enforced at runtime.
// When implemented, tool calls within a skill invocation should be rate-limited
// according to these settings.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RateLimitConfig {
    pub max_calls: Option<usize>,
    pub window_secs: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolsConfig {
    /// Allowed tools (whitelist)
    pub allow: Vec<String>,
    /// Denied tools (blacklist)
    pub deny: Vec<String>,
    /// Per-tool default parameters
    /// TODO(P2-3): defaults are parsed but not yet injected into tool calls at runtime.
    #[serde(default)]
    pub defaults: HashMap<String, serde_json::Value>,
    /// Rate limiting for tool calls
    pub rate_limit: RateLimitConfig,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ExpectConfig {
    pub contains: Vec<String>,
    pub format: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct OutputConfig {
    /// Output format hint ("text" | "json" | "markdown")
    pub format: Option<String>,
    /// Max output length in characters
    /// TODO(P2-3): max_length is parsed but not yet enforced at runtime.
    pub max_length: Option<usize>,
    /// Required H2 section headings in the output (for markdown format).
    pub required_sections: Vec<String>,
    /// Max output tokens (estimated as chars/4).
    pub max_tokens: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TestsConfig {
    /// Test input prompts
    pub inputs: Vec<String>,
    /// Expected output conditions
    pub expect: ExpectConfig,
    /// Smoke test input file paths (relative to skill directory)
    pub smoke: Vec<String>,
}

// ---------------------------------------------------------------------------
// Main Types
// ---------------------------------------------------------------------------

/// YAML frontmatter metadata — loaded at Level 1 (startup catalog scan).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SkillFrontmatter {
    // Identity
    pub id: Option<String>,
    /// Human-readable skill name (e.g. "Code Review").
    pub name: String,
    pub version: Option<String>,
    /// Short description for catalog display.
    pub description: String,

    // New spec sections
    pub invoke: InvokeConfig,
    pub routing: RoutingConfig,
    pub context: ContextConfig,
    pub permissions: PermissionsConfig,
    pub tools: ToolsConfig,
    pub output: OutputConfig,
    pub tests: TestsConfig,

    // Legacy compat (deserialized but not serialized)
    /// Slash command that invokes this skill (e.g. "review"). Legacy field.
    #[serde(skip_serializing)]
    pub command: Option<String>,
    /// Regex patterns that auto-detect when this skill should activate. Legacy field.
    #[serde(skip_serializing)]
    pub trigger_patterns: Vec<String>,
    /// Tool names this skill requires to function. Legacy field.
    #[serde(skip_serializing)]
    pub tools_required: Vec<String>,
    /// If true, this skill's instructions are always loaded into context. Legacy field.
    #[serde(skip_serializing)]
    pub auto_load: bool,
    /// Controls when the skill description appears in the LLM catalog prompt. Legacy field.
    #[serde(skip_serializing)]
    pub read_when: Vec<String>,
}

impl SkillFrontmatter {
    /// Bridge legacy fields to the new schema sections.
    fn apply_legacy_compat(&mut self) {
        // command -> invoke.slash (add "/" prefix)
        if self.invoke.slash.is_none()
            && let Some(ref cmd) = self.command
        {
            self.invoke.slash = Some(format!("/{}", cmd));
        }
        // auto_load=true -> invoke.mode="auto" (only if mode is still default)
        if self.invoke.mode == "manual" && self.auto_load {
            self.invoke.mode = "auto".to_string();
        }
        // trigger_patterns -> routing.intent
        if self.routing.intent.is_empty() && !self.trigger_patterns.is_empty() {
            self.routing.intent = self.trigger_patterns.clone();
        }
        // tools_required -> tools.allow
        if self.tools.allow.is_empty() && !self.tools_required.is_empty() {
            self.tools.allow = self.tools_required.clone();
        }
        // read_when -> context.read_when
        if self.context.read_when.is_empty() && !self.read_when.is_empty() {
            self.context.read_when = self.read_when.clone();
        }
    }

    fn validate(&self) -> Result<(), SkillParseError> {
        if self.name.is_empty() {
            return Err(SkillParseError::MissingField("name"));
        }
        if self.description.is_empty() {
            return Err(SkillParseError::MissingField("description"));
        }
        Ok(())
    }

    /// Get the effective slash command (without "/" prefix), checking both
    /// new `invoke.slash` and legacy `command` fields.
    pub fn effective_slash_command(&self) -> Option<String> {
        self.invoke
            .slash
            .as_ref()
            .map(|s| s.strip_prefix('/').unwrap_or(s).to_string())
            .or_else(|| self.command.clone())
    }
}

/// Full parsed SKILL.md document — loaded at Level 2 (on invocation).
#[derive(Debug, Clone, PartialEq)]
pub struct SkillDocument {
    pub frontmatter: SkillFrontmatter,
    /// The full markdown body (instructions, examples, templates).
    pub body: String,
    /// Parsed `## Section` headings → body text.
    pub sections: HashMap<String, String>,
}

/// Errors that can occur while parsing a SKILL.md file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillParseError {
    MissingFrontmatter,
    UnterminatedFrontmatter,
    MissingField(&'static str),
    InvalidYaml(String),
}

impl fmt::Display for SkillParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingFrontmatter => write!(f, "Missing YAML frontmatter"),
            Self::UnterminatedFrontmatter => write!(f, "Unterminated YAML frontmatter"),
            Self::MissingField(field) => {
                write!(f, "Missing required frontmatter field '{}'", field)
            }
            Self::InvalidYaml(msg) => write!(f, "Invalid YAML: {}", msg),
        }
    }
}

impl std::error::Error for SkillParseError {}

// ---------------------------------------------------------------------------
// Frontmatter extraction
// ---------------------------------------------------------------------------

/// Extract the raw YAML string between `---` delimiters.
/// Returns (yaml_str, body_lines) on success.
fn extract_frontmatter_str(input: &str) -> Result<(&str, Vec<String>), SkillParseError> {
    let trimmed = input.trim_start();
    if !trimmed.starts_with("---") {
        return Err(SkillParseError::MissingFrontmatter);
    }

    // Find the opening ---
    let after_first = &trimmed[3..];
    // Skip the rest of the opening line (should be just newline)
    let after_first = after_first.strip_prefix('\n').unwrap_or(after_first);

    // Find the closing ---
    let close_pos = after_first.find("\n---");
    match close_pos {
        Some(pos) => {
            let yaml_str = &after_first[..pos];
            let remainder = &after_first[pos + 4..]; // skip "\n---"
            // Skip the rest of the closing line
            let remainder = remainder.strip_prefix('\n').unwrap_or(remainder);
            let body_lines: Vec<String> = remainder.lines().map(|l| l.to_string()).collect();
            Ok((yaml_str, body_lines))
        }
        None => Err(SkillParseError::UnterminatedFrontmatter),
    }
}

// ---------------------------------------------------------------------------
// Body parsing (lenient — all sections optional)
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

    // Trim trailing whitespace from full body
    let body = full_body.trim_end().to_string();
    (body, sections)
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Parse only the YAML frontmatter from a SKILL.md file (Level 1).
///
/// Use this for lightweight catalog scanning at startup — does NOT parse the body.
pub fn parse_skill_frontmatter(input: &str) -> Result<SkillFrontmatter, SkillParseError> {
    let (yaml_str, _body_lines) = extract_frontmatter_str(input)?;
    let mut fm: SkillFrontmatter =
        serde_yaml::from_str(yaml_str).map_err(|e| SkillParseError::InvalidYaml(e.to_string()))?;
    fm.apply_legacy_compat();
    fm.validate()?;
    Ok(fm)
}

/// Parse the full SKILL.md file including body sections (Level 2).
pub fn parse_skill_markdown(input: &str) -> Result<SkillDocument, SkillParseError> {
    let (yaml_str, body_lines) = extract_frontmatter_str(input)?;
    let mut frontmatter: SkillFrontmatter =
        serde_yaml::from_str(yaml_str).map_err(|e| SkillParseError::InvalidYaml(e.to_string()))?;
    frontmatter.apply_legacy_compat();
    frontmatter.validate()?;
    let (body, sections) = parse_body_sections(&body_lines);

    Ok(SkillDocument {
        frontmatter,
        body,
        sections,
    })
}

/// Render a `SkillDocument` back to valid SKILL.md markdown.
///
/// Emits the new schema format. Legacy fields are not serialized (skip_serializing).
pub fn render_skill_markdown(doc: &SkillDocument) -> String {
    let mut out = String::new();

    // -- Frontmatter --
    out.push_str("---\n");

    // Use serde_yaml for serialization. Fall back to manual if it fails.
    match serde_yaml::to_string(&doc.frontmatter) {
        Ok(yaml) => out.push_str(&yaml),
        Err(_) => {
            // Manual fallback for basic fields
            out.push_str(&format!("name: \"{}\"\n", doc.frontmatter.name));
            out.push_str(&format!(
                "description: \"{}\"\n",
                doc.frontmatter.description
            ));
        }
    }
    out.push_str("---\n");

    // -- Body --
    if !doc.body.is_empty() {
        out.push('\n');
        out.push_str(&doc.body);
        out.push('\n');
    }

    out
}

/// Render a skill's instructions into a prompt block for LLM context injection.
///
/// Budget: 4000 characters. Returns empty string if body is empty.
pub fn skill_to_prompt_block(doc: &SkillDocument) -> String {
    if doc.body.trim().is_empty() {
        return String::new();
    }

    let mut block = format!("### SKILL CONTEXT: {} ###\n", doc.frontmatter.name);
    let truncated: String = doc.body.chars().take(4000).collect();
    block.push_str(&truncated);
    block
}

/// Check whether a skill document has meaningful content (non-empty body).
pub fn skill_document_has_content(doc: &SkillDocument) -> bool {
    !doc.body.trim().is_empty()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;

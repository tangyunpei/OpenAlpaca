use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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
    #[serde(default)]
    pub health_weight: f64,
}

impl Default for ScoreWeights {
    fn default() -> Self {
        Self {
            base: default_base(),
            intent_weight: default_intent_weight(),
            keyword_weight: default_keyword_weight(),
            recency_weight: default_recency_weight(),
            health_weight: 0.0,
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

/// DEPRECATED: `context.summarize` is parsed for backward compatibility but has
/// no runtime effect. Use `context.budget_tokens` for context size control.
/// Kept with `#[serde(default)]` to prevent YAML deserialization breakage.
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

/// Rate limiting for tool calls within a skill invocation.
/// `max_calls` is propagated to `SandboxPolicy.max_tool_calls` in the handler.
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
    /// DEPRECATED: `tools.defaults` is parsed for backward compatibility but has
    /// no runtime effect. Tool default arguments are not injected at runtime.
    #[serde(default)]
    pub defaults: HashMap<String, serde_json::Value>,
    /// Rate limiting for tool calls
    pub rate_limit: RateLimitConfig,
}

/// A script bundled with a skill that becomes a callable tool during invocation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScriptConfig {
    /// Script filename relative to the skill's `scripts/` directory.
    pub file: String,
    /// Tool name exposed to the LLM (will be prefixed with `skill_script:`).
    pub name: String,
    /// Human-readable description for the LLM tool definition.
    pub description: String,
    /// JSON Schema for the tool's parameters (passed to LLM).
    #[serde(default = "default_empty_object")]
    pub parameters: serde_json::Value,
    /// Optional interpreter override (e.g. "python3", "node"). Auto-detected from shebang/extension if omitted.
    pub interpreter: Option<String>,
    /// Execution timeout in seconds. Default: 30.
    #[serde(default = "default_script_timeout")]
    pub timeout_secs: u64,
}

impl ScriptConfig {
    /// Convert to an LLM ToolDefinition with the `skill_script:` namespace prefix.
    pub fn to_tool_definition(&self) -> openalpaca_llm::ToolDefinition {
        openalpaca_llm::ToolDefinition {
            name: format!("skill_script:{}", self.name),
            description: self.description.clone(),
            parameters: self.parameters.clone(),
            strict: None,
            input_examples: None,
        }
    }
}

fn default_empty_object() -> serde_json::Value {
    serde_json::json!({"type": "object", "properties": {}})
}

fn default_script_timeout() -> u64 {
    30
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
    /// Max output length in characters. Enforced as hard truncation in handler.
    pub max_length: Option<usize>,
    /// Required H2 section headings in the output (for markdown format).
    pub required_sections: Vec<String>,
    /// Max output tokens (estimated as chars/4).
    pub max_tokens: Option<usize>,
    /// When true, attempt deterministic repair of validation failures.
    #[serde(default)]
    pub auto_repair: bool,
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

    /// Executable scripts bundled with this skill, exposed as callable tools.
    #[serde(default)]
    pub scripts: Vec<ScriptConfig>,

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
    pub(super) fn apply_legacy_compat(&mut self) {
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

    pub(super) fn validate(&self) -> Result<(), super::SkillParseError> {
        if self.name.is_empty() {
            return Err(super::SkillParseError::MissingField("name"));
        }
        if self.description.is_empty() {
            return Err(super::SkillParseError::MissingField("description"));
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

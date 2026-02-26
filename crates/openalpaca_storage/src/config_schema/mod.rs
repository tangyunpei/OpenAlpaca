//! Config key schema registry with validation, normalization, and category helpers.
//!
//! Provides a centralized definition of all known configuration keys,
//! their types, defaults, and validation rules. Used by the CLI for
//! validated writes (`set_checked`) and schema-driven TUI.

/// Which storage backend owns this key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigBackend {
    /// SQLite system_config table.
    SystemConfig,
    /// config/llm.toml file (AI/LLM settings).
    LlmToml,
    /// config/daemon.toml file (orchestrator, execution, DAG, server settings).
    DaemonToml,
}

/// Value type for validation.
#[derive(Debug, Clone)]
pub enum ConfigKind {
    String,
    /// Accepts true/false/yes/no/1/0, normalizes to "true"/"false".
    Bool,
    /// Accepts only the listed values.
    Enum(&'static [&'static str]),
    /// Validated integer range.
    Int {
        min: Option<i64>,
        max: Option<i64>,
    },
    /// Validated floating-point range.
    Float {
        min: Option<f64>,
        max: Option<f64>,
    },
}

impl ConfigKind {
    /// Returns the DB `kind` column value.
    pub fn as_db_kind(&self) -> &'static str {
        match self {
            ConfigKind::String => "string",
            ConfigKind::Bool => "bool",
            ConfigKind::Enum(_) => "enum",
            ConfigKind::Int { .. } => "int",
            ConfigKind::Float { .. } => "float",
        }
    }

    /// Type-specific validation. Returns `Ok(())` or an error message.
    pub fn validate_value(&self, value: &str) -> Result<(), String> {
        match self {
            ConfigKind::String => Ok(()),
            ConfigKind::Bool => {
                let lower = value.trim().to_lowercase();
                if matches!(lower.as_str(), "true" | "false" | "yes" | "no" | "1" | "0") {
                    Ok(())
                } else {
                    Err(format!(
                        "expected a boolean (true/false/yes/no/1/0), got '{}'",
                        value
                    ))
                }
            }
            ConfigKind::Enum(choices) => {
                let lower = value.trim().to_lowercase();
                if choices.iter().any(|c| c.to_lowercase() == lower) {
                    Ok(())
                } else {
                    Err(format!(
                        "expected one of [{}], got '{}'",
                        choices.join(", "),
                        value
                    ))
                }
            }
            ConfigKind::Int { min, max } => {
                let n: i64 = value
                    .trim()
                    .parse()
                    .map_err(|_| format!("expected an integer, got '{}'", value))?;
                if let Some(lo) = min
                    && n < *lo
                {
                    return Err(format!("value {} is below minimum {}", n, lo));
                }
                if let Some(hi) = max
                    && n > *hi
                {
                    return Err(format!("value {} is above maximum {}", n, hi));
                }
                Ok(())
            }
            ConfigKind::Float { min, max } => {
                let n: f64 = value
                    .trim()
                    .parse()
                    .map_err(|_| format!("expected a number, got '{}'", value))?;
                if let Some(lo) = min
                    && n < *lo
                {
                    return Err(format!("value {} is below minimum {}", n, lo));
                }
                if let Some(hi) = max
                    && n > *hi
                {
                    return Err(format!("value {} is above maximum {}", n, hi));
                }
                Ok(())
            }
        }
    }

    /// Normalize a value to its canonical form.
    pub fn normalize_value(&self, value: &str) -> String {
        match self {
            ConfigKind::Bool => {
                let lower = value.trim().to_lowercase();
                match lower.as_str() {
                    "true" | "yes" | "1" => "true".to_string(),
                    _ => "false".to_string(),
                }
            }
            ConfigKind::Enum(choices) => {
                let lower = value.trim().to_lowercase();
                choices
                    .iter()
                    .find(|c| c.to_lowercase() == lower)
                    .unwrap_or(&value)
                    .to_string()
            }
            ConfigKind::Int { .. } => value.trim().to_string(),
            ConfigKind::Float { .. } => value.trim().to_string(),
            ConfigKind::String => value.trim().to_string(),
        }
    }
}

/// A registered config key definition.
#[derive(Debug, Clone)]
pub struct ConfigKeyDef {
    pub key: &'static str,
    pub kind: ConfigKind,
    pub default: Option<&'static str>,
    pub description: &'static str,
    /// Display category: "Connectors", "System", "API-Keys", "Agents"
    pub category: &'static str,
    /// Provider/tab grouping within a category (e.g. "Anthropic", "Orchestrator").
    pub subcategory: Option<&'static str>,
    /// Mask in output, use Password in TUI
    pub sensitive: bool,
    /// Which backend stores this key.
    pub backend: ConfigBackend,
}

/// Static registry of all known config keys.
pub static CONFIG_KEYS: &[ConfigKeyDef] = &[
    // -- Connectors --
    ConfigKeyDef {
        key: "telegram.token",
        kind: ConfigKind::String,
        default: None,
        description: "Telegram Bot API token from @BotFather",
        category: "Connectors",
        subcategory: None,
        sensitive: true,
        backend: ConfigBackend::SystemConfig,
    },
    ConfigKeyDef {
        key: "telegram.enabled",
        kind: ConfigKind::Bool,
        default: Some("false"),
        description: "Enable Telegram connector",
        category: "Connectors",
        subcategory: None,
        sensitive: false,
        backend: ConfigBackend::SystemConfig,
    },
    ConfigKeyDef {
        key: "imessage.token",
        kind: ConfigKind::String,
        default: None,
        description: "iMessage connector placeholder (not required — iMessage uses chat.db polling)",
        category: "Connectors",
        subcategory: None,
        sensitive: false,
        backend: ConfigBackend::SystemConfig,
    },
    ConfigKeyDef {
        key: "imessage.enabled",
        kind: ConfigKind::Bool,
        default: Some("false"),
        description: "Enable iMessage connector",
        category: "Connectors",
        subcategory: None,
        sensitive: false,
        backend: ConfigBackend::SystemConfig,
    },
    ConfigKeyDef {
        key: "imessage.allow_from_me",
        kind: ConfigKind::Bool,
        default: Some("true"),
        description: "Allow iMessage connector to process messages sent by this Mac user (useful for self-chat)",
        category: "Connectors",
        subcategory: None,
        sensitive: false,
        backend: ConfigBackend::SystemConfig,
    },
    ConfigKeyDef {
        key: "discord.token",
        kind: ConfigKind::String,
        default: None,
        description: "Discord bot token",
        category: "Connectors",
        subcategory: None,
        sensitive: true,
        backend: ConfigBackend::SystemConfig,
    },
    ConfigKeyDef {
        key: "discord.enabled",
        kind: ConfigKind::Bool,
        default: Some("false"),
        description: "Enable Discord connector",
        category: "Connectors",
        subcategory: None,
        sensitive: false,
        backend: ConfigBackend::SystemConfig,
    },
    // -- System --
    ConfigKeyDef {
        key: "system.debug_level",
        kind: ConfigKind::Enum(&["error", "warn", "info", "debug", "trace"]),
        default: Some("info"),
        description: "Log verbosity level",
        category: "System",
        subcategory: None,
        sensitive: false,
        backend: ConfigBackend::SystemConfig,
    },
    ConfigKeyDef {
        key: "system.language",
        kind: ConfigKind::String,
        default: Some("en"),
        description: "Preferred language for responses",
        category: "System",
        subcategory: None,
        sensitive: false,
        backend: ConfigBackend::SystemConfig,
    },
    ConfigKeyDef {
        key: "system.max_agents",
        kind: ConfigKind::Int {
            min: Some(1),
            max: Some(32),
        },
        default: Some("3"),
        description: "Maximum concurrent DAG agents (alias for daemon.dag.max_concurrent_agents)",
        category: "Daemon",
        subcategory: Some("DAG"),
        sensitive: false,
        backend: ConfigBackend::DaemonToml,
    },
    ConfigKeyDef {
        key: "system.data_dir",
        kind: ConfigKind::String,
        default: None,
        description: "Override data directory path",
        category: "System",
        subcategory: None,
        sensitive: false,
        backend: ConfigBackend::SystemConfig,
    },
    // -- Daemon: Orchestrator --
    ConfigKeyDef {
        key: "daemon.orchestrator.prompt_recent_messages",
        kind: ConfigKind::Int {
            min: Some(1),
            max: Some(200),
        },
        default: Some("40"),
        description: "Number of recent messages kept in prompt context",
        category: "Daemon",
        subcategory: Some("Orchestrator"),
        sensitive: false,
        backend: ConfigBackend::DaemonToml,
    },
    ConfigKeyDef {
        key: "daemon.orchestrator.summary_max_chars",
        kind: ConfigKind::Int {
            min: Some(100),
            max: Some(32000),
        },
        default: Some("4000"),
        description: "Maximum summary length in characters",
        category: "Daemon",
        subcategory: Some("Orchestrator"),
        sensitive: false,
        backend: ConfigBackend::DaemonToml,
    },
    ConfigKeyDef {
        key: "daemon.orchestrator.summary_max_daily_cost_usd",
        kind: ConfigKind::Float {
            min: Some(0.0),
            max: Some(100.0),
        },
        default: Some("0.50"),
        description: "Daily cost cap for summarization (USD)",
        category: "Daemon",
        subcategory: Some("Orchestrator"),
        sensitive: false,
        backend: ConfigBackend::DaemonToml,
    },
    ConfigKeyDef {
        key: "daemon.orchestrator.identity_budget",
        kind: ConfigKind::Int {
            min: Some(50),
            max: Some(5000),
        },
        default: Some("300"),
        description: "Identity prompt budget in characters",
        category: "Daemon",
        subcategory: Some("Orchestrator"),
        sensitive: false,
        backend: ConfigBackend::DaemonToml,
    },
    ConfigKeyDef {
        key: "daemon.orchestrator.user_profile_budget",
        kind: ConfigKind::Int {
            min: Some(100),
            max: Some(10000),
        },
        default: Some("1000"),
        description: "User profile prompt budget in characters",
        category: "Daemon",
        subcategory: Some("Orchestrator"),
        sensitive: false,
        backend: ConfigBackend::DaemonToml,
    },
    // -- Daemon: Execution --
    ConfigKeyDef {
        key: "daemon.execution.max_rounds",
        kind: ConfigKind::Int {
            min: Some(1),
            max: Some(100),
        },
        default: Some("15"),
        description: "Default max rounds per agent",
        category: "Daemon",
        subcategory: Some("Execution"),
        sensitive: false,
        backend: ConfigBackend::DaemonToml,
    },
    ConfigKeyDef {
        key: "daemon.execution.max_cost",
        kind: ConfigKind::Float {
            min: Some(0.0),
            max: Some(1000.0),
        },
        default: Some("1.00"),
        description: "Default max cost per agent (USD)",
        category: "Daemon",
        subcategory: Some("Execution"),
        sensitive: false,
        backend: ConfigBackend::DaemonToml,
    },
    ConfigKeyDef {
        key: "daemon.execution.lead_max_rounds",
        kind: ConfigKind::Int {
            min: Some(1),
            max: Some(200),
        },
        default: Some("30"),
        description: "Lead agent max rounds",
        category: "Daemon",
        subcategory: Some("Execution"),
        sensitive: false,
        backend: ConfigBackend::DaemonToml,
    },
    ConfigKeyDef {
        key: "daemon.execution.lead_max_cost",
        kind: ConfigKind::Float {
            min: Some(0.0),
            max: Some(1000.0),
        },
        default: Some("5.0"),
        description: "Lead agent max cost (USD)",
        category: "Daemon",
        subcategory: Some("Execution"),
        sensitive: false,
        backend: ConfigBackend::DaemonToml,
    },
    // -- Daemon: DAG --
    ConfigKeyDef {
        key: "daemon.dag.max_concurrent_agents",
        kind: ConfigKind::Int {
            min: Some(1),
            max: Some(32),
        },
        default: Some("3"),
        description: "Maximum concurrent DAG agents",
        category: "Daemon",
        subcategory: Some("DAG"),
        sensitive: false,
        backend: ConfigBackend::DaemonToml,
    },
    ConfigKeyDef {
        key: "daemon.dag.total_timeout_secs",
        kind: ConfigKind::Int {
            min: Some(60),
            max: Some(7200),
        },
        default: Some("1800"),
        description: "DAG total timeout in seconds",
        category: "Daemon",
        subcategory: Some("DAG"),
        sensitive: false,
        backend: ConfigBackend::DaemonToml,
    },
    // -- Daemon: Orchestrator (cont.) --
    ConfigKeyDef {
        key: "daemon.orchestrator.summary_min_new_older_messages",
        kind: ConfigKind::Int {
            min: Some(1),
            max: Some(200),
        },
        default: Some("12"),
        description: "Min new older messages before triggering summary",
        category: "Daemon",
        subcategory: Some("Orchestrator"),
        sensitive: false,
        backend: ConfigBackend::DaemonToml,
    },
    ConfigKeyDef {
        key: "daemon.orchestrator.msg_trunc_chars",
        kind: ConfigKind::Int {
            min: Some(100),
            max: Some(32000),
        },
        default: Some("1500"),
        description: "Character limit for message truncation in summary input",
        category: "Daemon",
        subcategory: Some("Orchestrator"),
        sensitive: false,
        backend: ConfigBackend::DaemonToml,
    },
    ConfigKeyDef {
        key: "daemon.orchestrator.extract_max_daily_cost_usd",
        kind: ConfigKind::Float {
            min: Some(0.0),
            max: Some(100.0),
        },
        default: Some("0.25"),
        description: "Daily cost cap for memory extractions (USD)",
        category: "Daemon",
        subcategory: Some("Orchestrator"),
        sensitive: false,
        backend: ConfigBackend::DaemonToml,
    },
    ConfigKeyDef {
        key: "daemon.orchestrator.extract_every_n_turns",
        kind: ConfigKind::Int {
            min: Some(1),
            max: Some(100),
        },
        default: Some("5"),
        description: "Run memory extraction every N user turns",
        category: "Daemon",
        subcategory: Some("Orchestrator"),
        sensitive: false,
        backend: ConfigBackend::DaemonToml,
    },
    ConfigKeyDef {
        key: "daemon.orchestrator.extract_min_content_len",
        kind: ConfigKind::Int {
            min: Some(1),
            max: Some(10000),
        },
        default: Some("20"),
        description: "Min content length (chars) to trigger extraction",
        category: "Daemon",
        subcategory: Some("Orchestrator"),
        sensitive: false,
        backend: ConfigBackend::DaemonToml,
    },
    // -- Daemon: Orchestrator – Task Extraction --
    ConfigKeyDef {
        key: "daemon.orchestrator.task_extract_enabled",
        kind: ConfigKind::Bool,
        default: Some("true"),
        description: "Enable task output memory extraction",
        category: "Daemon",
        subcategory: Some("Orchestrator"),
        sensitive: false,
        backend: ConfigBackend::DaemonToml,
    },
    ConfigKeyDef {
        key: "daemon.orchestrator.task_extract_max_daily_cost_usd",
        kind: ConfigKind::Float {
            min: Some(0.0),
            max: Some(100.0),
        },
        default: Some("0.50"),
        description: "Daily cost cap for task output extraction (USD)",
        category: "Daemon",
        subcategory: Some("Orchestrator"),
        sensitive: false,
        backend: ConfigBackend::DaemonToml,
    },
    ConfigKeyDef {
        key: "daemon.orchestrator.task_extract_min_content_len",
        kind: ConfigKind::Int {
            min: Some(1),
            max: Some(10000),
        },
        default: Some("100"),
        description: "Min task output length (chars) for extraction",
        category: "Daemon",
        subcategory: Some("Orchestrator"),
        sensitive: false,
        backend: ConfigBackend::DaemonToml,
    },
    // -- Daemon: Orchestrator – Supersession --
    ConfigKeyDef {
        key: "daemon.orchestrator.supersession_distance_threshold",
        kind: ConfigKind::Float {
            min: Some(0.0),
            max: Some(10.0),
        },
        default: Some("1.0"),
        description: "L2 distance threshold for semantic supersession",
        category: "Daemon",
        subcategory: Some("Orchestrator"),
        sensitive: false,
        backend: ConfigBackend::DaemonToml,
    },
    ConfigKeyDef {
        key: "daemon.orchestrator.fts_jaccard_threshold",
        kind: ConfigKind::Float {
            min: Some(0.0),
            max: Some(1.0),
        },
        default: Some("0.4"),
        description: "Jaccard overlap threshold for FTS supersession fallback",
        category: "Daemon",
        subcategory: Some("Orchestrator"),
        sensitive: false,
        backend: ConfigBackend::DaemonToml,
    },
    // -- Daemon: Orchestrator – Decay --
    ConfigKeyDef {
        key: "daemon.orchestrator.decay_poll_interval_secs",
        kind: ConfigKind::Int {
            min: Some(60),
            max: Some(86400),
        },
        default: Some("3600"),
        description: "How often to run the decay task (seconds)",
        category: "Daemon",
        subcategory: Some("Orchestrator"),
        sensitive: false,
        backend: ConfigBackend::DaemonToml,
    },
    ConfigKeyDef {
        key: "daemon.orchestrator.decay_half_life_days",
        kind: ConfigKind::Float {
            min: Some(1.0),
            max: Some(365.0),
        },
        default: Some("30.0"),
        description: "Half-life in days for importance decay",
        category: "Daemon",
        subcategory: Some("Orchestrator"),
        sensitive: false,
        backend: ConfigBackend::DaemonToml,
    },
    ConfigKeyDef {
        key: "daemon.orchestrator.decay_min_importance",
        kind: ConfigKind::Float {
            min: Some(0.0),
            max: Some(1.0),
        },
        default: Some("0.05"),
        description: "Min importance floor; below this memories are prune-eligible",
        category: "Daemon",
        subcategory: Some("Orchestrator"),
        sensitive: false,
        backend: ConfigBackend::DaemonToml,
    },
    ConfigKeyDef {
        key: "daemon.orchestrator.decay_soft_cap",
        kind: ConfigKind::Int {
            min: Some(10),
            max: Some(100000),
        },
        default: Some("500"),
        description: "Soft cap on total memories per owner",
        category: "Daemon",
        subcategory: Some("Orchestrator"),
        sensitive: false,
        backend: ConfigBackend::DaemonToml,
    },
    // -- Daemon: Execution (cont.) --
    ConfigKeyDef {
        key: "daemon.execution.max_tools_per_round",
        kind: ConfigKind::Int {
            min: Some(1),
            max: Some(50),
        },
        default: Some("5"),
        description: "Default max tool calls per agent round",
        category: "Daemon",
        subcategory: Some("Execution"),
        sensitive: false,
        backend: ConfigBackend::DaemonToml,
    },
    ConfigKeyDef {
        key: "daemon.execution.max_tool_runtime_secs",
        kind: ConfigKind::Int {
            min: Some(1),
            max: Some(600),
        },
        default: Some("60"),
        description: "Default max tool runtime in seconds",
        category: "Daemon",
        subcategory: Some("Execution"),
        sensitive: false,
        backend: ConfigBackend::DaemonToml,
    },
    ConfigKeyDef {
        key: "daemon.execution.lead_max_tools_per_round",
        kind: ConfigKind::Int {
            min: Some(1),
            max: Some(50),
        },
        default: Some("3"),
        description: "Lead agent max tool calls per round",
        category: "Daemon",
        subcategory: Some("Execution"),
        sensitive: false,
        backend: ConfigBackend::DaemonToml,
    },
    ConfigKeyDef {
        key: "daemon.execution.lead_max_tool_runtime_secs",
        kind: ConfigKind::Int {
            min: Some(1),
            max: Some(3600),
        },
        default: Some("300"),
        description: "Lead agent max tool runtime in seconds",
        category: "Daemon",
        subcategory: Some("Execution"),
        sensitive: false,
        backend: ConfigBackend::DaemonToml,
    },
    ConfigKeyDef {
        key: "daemon.execution.lead_max_concurrent_subagents",
        kind: ConfigKind::Int {
            min: Some(1),
            max: Some(32),
        },
        default: Some("5"),
        description: "Maximum concurrent subagents per lead agent",
        category: "Daemon",
        subcategory: Some("Execution"),
        sensitive: false,
        backend: ConfigBackend::DaemonToml,
    },
    // -- Daemon: DAG (cont.) --
    ConfigKeyDef {
        key: "daemon.dag.node_timeout_secs",
        kind: ConfigKind::Int {
            min: Some(10),
            max: Some(3600),
        },
        default: Some("300"),
        description: "Timeout per DAG node in seconds",
        category: "Daemon",
        subcategory: Some("DAG"),
        sensitive: false,
        backend: ConfigBackend::DaemonToml,
    },
    ConfigKeyDef {
        key: "daemon.dag.max_retries_per_node",
        kind: ConfigKind::Int {
            min: Some(0),
            max: Some(10),
        },
        default: Some("1"),
        description: "Max retries per DAG node on failure",
        category: "Daemon",
        subcategory: Some("DAG"),
        sensitive: false,
        backend: ConfigBackend::DaemonToml,
    },
    ConfigKeyDef {
        key: "daemon.dag.replan_after_every_n_nodes",
        kind: ConfigKind::Int {
            min: Some(1),
            max: Some(50),
        },
        default: Some("2"),
        description: "Trigger DAG replan after every N completed nodes",
        category: "Daemon",
        subcategory: Some("DAG"),
        sensitive: false,
        backend: ConfigBackend::DaemonToml,
    },
    ConfigKeyDef {
        key: "daemon.dag.max_replans",
        kind: ConfigKind::Int {
            min: Some(0),
            max: Some(50),
        },
        default: Some("3"),
        description: "Maximum number of DAG replans allowed",
        category: "Daemon",
        subcategory: Some("DAG"),
        sensitive: false,
        backend: ConfigBackend::DaemonToml,
    },
    // -- Daemon: Security --
    ConfigKeyDef {
        key: "daemon.security.max_input_length",
        kind: ConfigKind::Int {
            min: Some(1024),
            max: Some(1_048_576),
        },
        default: Some("32768"),
        description: "Maximum user input length in bytes",
        category: "Daemon",
        subcategory: Some("Security"),
        sensitive: false,
        backend: ConfigBackend::DaemonToml,
    },
    // -- Daemon: Server --
    ConfigKeyDef {
        key: "daemon.server.heartbeat_interval_secs",
        kind: ConfigKind::Int {
            min: Some(1),
            max: Some(300),
        },
        default: Some("5"),
        description: "WebSocket heartbeat interval in seconds",
        category: "Daemon",
        subcategory: Some("Server"),
        sensitive: false,
        backend: ConfigBackend::DaemonToml,
    },
    ConfigKeyDef {
        key: "daemon.server.sse_keep_alive_secs",
        kind: ConfigKind::Int {
            min: Some(1),
            max: Some(300),
        },
        default: Some("15"),
        description: "SSE keep-alive interval in seconds",
        category: "Daemon",
        subcategory: Some("Server"),
        sensitive: false,
        backend: ConfigBackend::DaemonToml,
    },
    ConfigKeyDef {
        key: "daemon.server.event_broadcaster_capacity",
        kind: ConfigKind::Int {
            min: Some(8),
            max: Some(1024),
        },
        default: Some("64"),
        description: "Event broadcaster channel capacity (restart-only)",
        category: "Daemon",
        subcategory: Some("Server"),
        sensitive: false,
        backend: ConfigBackend::DaemonToml,
    },
    ConfigKeyDef {
        key: "daemon.server.wake_channel_capacity",
        kind: ConfigKind::Int {
            min: Some(8),
            max: Some(4096),
        },
        default: Some("256"),
        description: "Wake event channel capacity (restart-only)",
        category: "Daemon",
        subcategory: Some("Server"),
        sensitive: false,
        backend: ConfigBackend::DaemonToml,
    },
    ConfigKeyDef {
        key: "daemon.server.cleanup_interval_secs",
        kind: ConfigKind::Int {
            min: Some(5),
            max: Some(3600),
        },
        default: Some("60"),
        description: "Chat stream stale cleanup interval in seconds",
        category: "Daemon",
        subcategory: Some("Server"),
        sensitive: false,
        backend: ConfigBackend::DaemonToml,
    },
    ConfigKeyDef {
        key: "daemon.server.stale_timeout_secs",
        kind: ConfigKind::Int {
            min: Some(5),
            max: Some(3600),
        },
        default: Some("30"),
        description: "Chat stream stale timeout in seconds",
        category: "Daemon",
        subcategory: Some("Server"),
        sensitive: false,
        backend: ConfigBackend::DaemonToml,
    },
    ConfigKeyDef {
        key: "daemon.server.stream_chunk_delay_ms",
        kind: ConfigKind::Int {
            min: Some(0),
            max: Some(500),
        },
        default: Some("30"),
        description: "Delay in milliseconds between streaming word chunks (0 = no delay)",
        category: "Daemon",
        subcategory: Some("Server"),
        sensitive: false,
        backend: ConfigBackend::DaemonToml,
    },
    ConfigKeyDef {
        key: "daemon.server.stream_chunk_words",
        kind: ConfigKind::Int {
            min: Some(1),
            max: Some(50),
        },
        default: Some("3"),
        description: "Number of words per streaming delta chunk",
        category: "Daemon",
        subcategory: Some("Server"),
        sensitive: false,
        backend: ConfigBackend::DaemonToml,
    },
    ConfigKeyDef {
        key: "daemon.server.embedding_poll_interval_secs",
        kind: ConfigKind::Int {
            min: Some(5),
            max: Some(3600),
        },
        default: Some("30"),
        description: "Embedding indexer poll interval in seconds",
        category: "Daemon",
        subcategory: Some("Server"),
        sensitive: false,
        backend: ConfigBackend::DaemonToml,
    },
    ConfigKeyDef {
        key: "daemon.server.embedding_batch_size",
        kind: ConfigKind::Int {
            min: Some(1),
            max: Some(500),
        },
        default: Some("50"),
        description: "Embedding indexer batch size per run",
        category: "Daemon",
        subcategory: Some("Server"),
        sensitive: false,
        backend: ConfigBackend::DaemonToml,
    },
    // -- Web Search (stored in llm.toml) --
    ConfigKeyDef {
        key: "ai.web_search.api_key",
        kind: ConfigKind::String,
        default: None,
        description: "Brave Search API key (get one at https://brave.com/search/api/)",
        category: "AI",
        subcategory: Some("Web Search"),
        sensitive: true,
        backend: ConfigBackend::LlmToml,
    },
    ConfigKeyDef {
        key: "ai.web_search.timeout_secs",
        kind: ConfigKind::Int {
            min: Some(1),
            max: Some(60),
        },
        default: Some("15"),
        description: "Brave Search request timeout in seconds",
        category: "AI",
        subcategory: Some("Web Search"),
        sensitive: false,
        backend: ConfigBackend::LlmToml,
    },
    // -- Agents --
    ConfigKeyDef {
        key: "ai.default_model",
        kind: ConfigKind::String,
        default: Some("claude-sonnet-4-5-20250929"),
        description: "Default LLM model for orchestration",
        category: "Agents",
        subcategory: Some("Orchestrator"),
        sensitive: false,
        backend: ConfigBackend::LlmToml,
    },
    ConfigKeyDef {
        key: "ai.fallback_models",
        kind: ConfigKind::String,
        default: None,
        description: "Comma-separated fallback model list",
        category: "Agents",
        subcategory: Some("Orchestrator"),
        sensitive: false,
        backend: ConfigBackend::LlmToml,
    },
    // -- Agents: Embeddings --
    ConfigKeyDef {
        key: "ai.embeddings.enabled",
        kind: ConfigKind::Bool,
        default: Some("false"),
        description: "Enable embedding generation for semantic memory search",
        category: "Agents",
        subcategory: Some("Embeddings"),
        sensitive: false,
        backend: ConfigBackend::LlmToml,
    },
    ConfigKeyDef {
        key: "ai.embeddings.provider",
        kind: ConfigKind::String,
        default: None,
        description: "Embedding provider (e.g. openai, ollama)",
        category: "Agents",
        subcategory: Some("Embeddings"),
        sensitive: false,
        backend: ConfigBackend::LlmToml,
    },
    ConfigKeyDef {
        key: "ai.embeddings.model",
        kind: ConfigKind::String,
        default: Some("text-embedding-3-small"),
        description: "Embedding model name",
        category: "Agents",
        subcategory: Some("Embeddings"),
        sensitive: false,
        backend: ConfigBackend::LlmToml,
    },
    ConfigKeyDef {
        key: "ai.embeddings.dimensions",
        kind: ConfigKind::Int {
            min: Some(64),
            max: Some(4096),
        },
        default: Some("1536"),
        description: "Embedding vector dimensions",
        category: "Agents",
        subcategory: Some("Embeddings"),
        sensitive: false,
        backend: ConfigBackend::LlmToml,
    },
    // -- API-Keys: Anthropic --
    ConfigKeyDef {
        key: "ai.anthropic.enabled",
        kind: ConfigKind::Bool,
        default: None,
        description: "Enable Anthropic provider",
        category: "API-Keys",
        subcategory: Some("Anthropic"),
        sensitive: false,
        backend: ConfigBackend::LlmToml,
    },
    ConfigKeyDef {
        key: "ai.anthropic.api_key",
        kind: ConfigKind::String,
        default: None,
        description: "Anthropic API key (encrypted at rest)",
        category: "API-Keys",
        subcategory: Some("Anthropic"),
        sensitive: true,
        backend: ConfigBackend::LlmToml,
    },
    // -- API-Keys: OpenAI --
    ConfigKeyDef {
        key: "ai.openai.enabled",
        kind: ConfigKind::Bool,
        default: None,
        description: "Enable OpenAI provider",
        category: "API-Keys",
        subcategory: Some("OpenAI"),
        sensitive: false,
        backend: ConfigBackend::LlmToml,
    },
    ConfigKeyDef {
        key: "ai.openai.api_key",
        kind: ConfigKind::String,
        default: None,
        description: "OpenAI API key (encrypted at rest)",
        category: "API-Keys",
        subcategory: Some("OpenAI"),
        sensitive: true,
        backend: ConfigBackend::LlmToml,
    },
    // -- API-Keys: Ollama --
    ConfigKeyDef {
        key: "ai.ollama.enabled",
        kind: ConfigKind::Bool,
        default: None,
        description: "Enable Ollama provider",
        category: "API-Keys",
        subcategory: Some("Ollama"),
        sensitive: false,
        backend: ConfigBackend::LlmToml,
    },
    ConfigKeyDef {
        key: "ai.ollama.base_url",
        kind: ConfigKind::String,
        default: None,
        description: "Ollama server base URL",
        category: "API-Keys",
        subcategory: Some("Ollama"),
        sensitive: false,
        backend: ConfigBackend::LlmToml,
    },
    // -- API-Keys: Claude Code --
    ConfigKeyDef {
        key: "ai.claude_code.discovery",
        kind: ConfigKind::Bool,
        default: Some("true"),
        description: "Auto-discover Claude Code OAuth token",
        category: "API-Keys",
        subcategory: Some("Claude Code"),
        sensitive: false,
        backend: ConfigBackend::LlmToml,
    },
    ConfigKeyDef {
        key: "ai.claude_code.cli_enabled",
        kind: ConfigKind::Bool,
        default: Some("true"),
        description: "Enable Claude Code CLI fallback",
        category: "API-Keys",
        subcategory: Some("Claude Code"),
        sensitive: false,
        backend: ConfigBackend::LlmToml,
    },
    ConfigKeyDef {
        key: "ai.claude_code.cli_path",
        kind: ConfigKind::String,
        default: None,
        description: "Override Claude Code CLI binary path",
        category: "API-Keys",
        subcategory: Some("Claude Code"),
        sensitive: false,
        backend: ConfigBackend::LlmToml,
    },
    // -- API-Keys: Codex --
    ConfigKeyDef {
        key: "ai.codex.discovery",
        kind: ConfigKind::Bool,
        default: Some("true"),
        description: "Auto-discover Codex OAuth token",
        category: "API-Keys",
        subcategory: Some("Codex"),
        sensitive: false,
        backend: ConfigBackend::LlmToml,
    },
    ConfigKeyDef {
        key: "ai.codex.cli_enabled",
        kind: ConfigKind::Bool,
        default: Some("true"),
        description: "Enable Codex CLI fallback",
        category: "API-Keys",
        subcategory: Some("Codex"),
        sensitive: false,
        backend: ConfigBackend::LlmToml,
    },
    ConfigKeyDef {
        key: "ai.codex.cli_path",
        kind: ConfigKind::String,
        default: None,
        description: "Override Codex CLI binary path",
        category: "API-Keys",
        subcategory: Some("Codex"),
        sensitive: false,
        backend: ConfigBackend::LlmToml,
    },
];

/// Look up a config key definition by exact match, then pattern-based fallback.
///
/// Pattern-based fallback: if key matches `<word>.token` or `<word>.enabled`,
/// treat it as a dynamic connector key with the appropriate kind.
pub fn lookup(key: &str) -> Option<ConfigKeyDef> {
    // Exact match first
    if let Some(def) = CONFIG_KEYS.iter().find(|d| d.key == key) {
        return Some(def.clone());
    }

    // Pattern-based fallback for dynamic connector keys
    if let Some(def) = pattern_match(key) {
        return Some(def);
    }

    None
}

/// Check if a key matches the dynamic connector pattern `<word>.token` or `<word>.enabled`.
fn pattern_match(key: &str) -> Option<ConfigKeyDef> {
    let parts: Vec<&str> = key.splitn(2, '.').collect();
    if parts.len() != 2 {
        return None;
    }

    let prefix = parts[0];
    let suffix = parts[1];

    // Validate prefix is lowercase alphanumeric + underscore
    if prefix.is_empty() || !prefix.chars().all(|c| c.is_ascii_lowercase() || c == '_') {
        return None;
    }

    match suffix {
        "token" => Some(ConfigKeyDef {
            key: "", // dynamic, not static
            kind: ConfigKind::String,
            default: None,
            description: "Connector token",
            category: "Connectors",
            subcategory: None,
            sensitive: true,
            backend: ConfigBackend::SystemConfig,
        }),
        "enabled" => Some(ConfigKeyDef {
            key: "", // dynamic, not static
            kind: ConfigKind::Bool,
            default: Some("false"),
            description: "Enable connector",
            category: "Connectors",
            subcategory: None,
            sensitive: false,
            backend: ConfigBackend::SystemConfig,
        }),
        _ => None,
    }
}

/// Validate a value against the schema for the given key.
pub fn validate(key: &str, value: &str) -> Result<(), String> {
    match lookup(key) {
        Some(def) => def.kind.validate_value(value),
        None => Err(format!("unknown config key '{}'", key)),
    }
}

/// Normalize a value to its canonical form for the given key.
pub fn normalize(key: &str, value: &str) -> String {
    match lookup(key) {
        Some(def) => def.kind.normalize_value(value),
        None => value.trim().to_string(),
    }
}

/// Return unique sorted category names from the static registry.
pub fn categories() -> Vec<&'static str> {
    let mut cats: Vec<&'static str> = CONFIG_KEYS.iter().map(|d| d.category).collect();
    cats.sort();
    cats.dedup();
    cats
}

/// Return all static keys belonging to a category.
pub fn keys_in_category(cat: &str) -> Vec<&'static ConfigKeyDef> {
    CONFIG_KEYS.iter().filter(|d| d.category == cat).collect()
}

/// Return unique sorted subcategory names within a category.
pub fn subcategories_in_category(cat: &str) -> Vec<&'static str> {
    let mut subs: Vec<&'static str> = CONFIG_KEYS
        .iter()
        .filter(|d| d.category == cat)
        .filter_map(|d| d.subcategory)
        .collect();
    subs.sort();
    subs.dedup();
    subs
}

/// Return all static keys belonging to a specific category + subcategory pair.
pub fn keys_in_subcategory(cat: &str, sub: &str) -> Vec<&'static ConfigKeyDef> {
    CONFIG_KEYS
        .iter()
        .filter(|d| d.category == cat && d.subcategory == Some(sub))
        .collect()
}

/// Mask a sensitive value, showing only the last 4 characters.
pub fn mask_value(value: &str) -> String {
    if value.len() <= 4 {
        "****".to_string()
    } else {
        format!("****{}", &value[value.len() - 4..])
    }
}

/// Validate an Anthropic setup-token (from `claude setup-token`).
///
/// Must start with `sk-ant-oat01-` and be at least 80 characters.
pub fn validate_anthropic_setup_token(value: &str) -> Result<(), String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err("Token cannot be empty. Run `claude setup-token` to generate one.".to_string());
    }
    if !trimmed.starts_with("sk-ant-oat01-") {
        return Err(
            "Invalid token prefix. Anthropic setup-tokens start with `sk-ant-oat01-`.\n\
             Run `claude setup-token` in your terminal to generate a valid token."
                .to_string(),
        );
    }
    if trimmed.len() < 80 {
        return Err(format!(
            "Token too short ({} chars, expected >= 80). \
             Make sure you copied the full token from `claude setup-token`.",
            trimmed.len()
        ));
    }
    Ok(())
}

/// Validate an OpenAI API key.
///
/// Must start with `sk-` (but NOT `sk-ant-`, which is Anthropic) and be at least 20 characters.
pub fn validate_openai_api_key(value: &str) -> Result<(), String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err("API key cannot be empty.".to_string());
    }
    if trimmed.starts_with("sk-ant-") {
        return Err(
            "This looks like an Anthropic key (sk-ant-*), not an OpenAI key.\n\
             OpenAI keys start with `sk-` without the `ant-` prefix."
                .to_string(),
        );
    }
    if !trimmed.starts_with("sk-") {
        return Err("Invalid key prefix. OpenAI API keys start with `sk-`.\n\
             Get your key at https://platform.openai.com/api-keys"
            .to_string());
    }
    if trimmed.len() < 20 {
        return Err(format!(
            "API key too short ({} chars, expected >= 20). \
             Make sure you copied the full key.",
            trimmed.len()
        ));
    }
    Ok(())
}

/// Validate an Anthropic API key (NOT a setup-token).
///
/// Must start with `sk-ant-` (but NOT `sk-ant-oat`, which is a setup-token)
/// and be at least 40 characters.
pub fn validate_anthropic_api_key(value: &str) -> Result<(), String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err("API key cannot be empty.".into());
    }
    if trimmed.starts_with("sk-ant-oat") {
        return Err(
            "This looks like a Claude Code setup-token (sk-ant-oat*), not an Anthropic API key.\n\
             Use the Claude Code guided setup instead, or add via the Claude Code source."
                .into(),
        );
    }
    if !trimmed.starts_with("sk-ant-") {
        return Err("Invalid key prefix. Anthropic API keys start with `sk-ant-`.".into());
    }
    if trimmed.len() < 40 {
        return Err(format!(
            "API key too short ({} chars, expected >= 40).",
            trimmed.len()
        ));
    }
    Ok(())
}

/// Validate a key for a given provider, dispatching to the correct validator.
pub fn validate_key_for_provider(provider: &str, value: &str) -> Result<(), String> {
    match provider {
        "anthropic" => validate_anthropic_api_key(value),
        "openai" => validate_openai_api_key(value),
        "ollama" => Ok(()),
        _ => Ok(()),
    }
}

/// Simple substring-based key suggestion (no external crate needed).
pub fn suggest_key(input: &str) -> Vec<&'static str> {
    let lower = input.to_lowercase();
    CONFIG_KEYS
        .iter()
        .filter(|d| {
            d.key.contains(&lower)
                || lower.contains(d.key)
                || d.key.split('.').any(|part| part.contains(&lower))
                || lower.split('.').any(|part| d.key.contains(part))
        })
        .map(|d| d.key)
        .collect()
}

#[cfg(test)]
mod tests;

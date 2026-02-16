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
    Int { min: Option<i64>, max: Option<i64> },
    /// Validated floating-point range.
    Float { min: Option<f64>, max: Option<f64> },
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
                if let Some(lo) = min {
                    if n < *lo {
                        return Err(format!("value {} is below minimum {}", n, lo));
                    }
                }
                if let Some(hi) = max {
                    if n > *hi {
                        return Err(format!("value {} is above maximum {}", n, hi));
                    }
                }
                Ok(())
            }
            ConfigKind::Float { min, max } => {
                let n: f64 = value
                    .trim()
                    .parse()
                    .map_err(|_| format!("expected a number, got '{}'", value))?;
                if let Some(lo) = min {
                    if n < *lo {
                        return Err(format!("value {} is below minimum {}", n, lo));
                    }
                }
                if let Some(hi) = max {
                    if n > *hi {
                        return Err(format!("value {} is above maximum {}", n, hi));
                    }
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
        description: "iMessage connector token",
        category: "Connectors",
        subcategory: None,
        sensitive: true,
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
        return Err(
            "Invalid key prefix. OpenAI API keys start with `sk-`.\n\
             Get your key at https://platform.openai.com/api-keys"
                .to_string(),
        );
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
mod tests {
    use super::*;

    #[test]
    fn test_lookup_exact() {
        let def = lookup("telegram.token").unwrap();
        assert_eq!(def.key, "telegram.token");
        assert!(def.sensitive);
    }

    #[test]
    fn test_lookup_pattern_token() {
        let def = lookup("wechat.token").unwrap();
        assert!(def.sensitive);
        assert!(matches!(def.kind, ConfigKind::String));
    }

    #[test]
    fn test_lookup_pattern_enabled() {
        let def = lookup("wechat.enabled").unwrap();
        assert!(!def.sensitive);
        assert!(matches!(def.kind, ConfigKind::Bool));
    }

    #[test]
    fn test_lookup_unknown() {
        assert!(lookup("wechat.xyz").is_none());
        assert!(lookup("unknown.key").is_none());
    }

    #[test]
    fn test_validate_bool() {
        assert!(validate("telegram.enabled", "true").is_ok());
        assert!(validate("telegram.enabled", "TRUE").is_ok());
        assert!(validate("telegram.enabled", "yes").is_ok());
        assert!(validate("telegram.enabled", "1").is_ok());
        assert!(validate("telegram.enabled", "potato").is_err());
    }

    #[test]
    fn test_validate_enum() {
        assert!(validate("system.debug_level", "info").is_ok());
        assert!(validate("system.debug_level", "INFO").is_ok());
        assert!(validate("system.debug_level", "potato").is_err());
    }

    #[test]
    fn test_validate_int_range() {
        assert!(validate("system.max_agents", "8").is_ok());
        assert!(validate("system.max_agents", "1").is_ok());
        assert!(validate("system.max_agents", "32").is_ok());
        assert!(validate("system.max_agents", "0").is_err());
        assert!(validate("system.max_agents", "99").is_err());
        assert!(validate("system.max_agents", "abc").is_err());
    }

    #[test]
    fn test_normalize_bool() {
        assert_eq!(normalize("telegram.enabled", "TRUE"), "true");
        assert_eq!(normalize("telegram.enabled", "Yes"), "true");
        assert_eq!(normalize("telegram.enabled", "1"), "true");
        assert_eq!(normalize("telegram.enabled", "false"), "false");
        assert_eq!(normalize("telegram.enabled", "no"), "false");
        assert_eq!(normalize("telegram.enabled", "0"), "false");
    }

    #[test]
    fn test_normalize_enum() {
        assert_eq!(normalize("system.debug_level", "INFO"), "info");
    }

    #[test]
    fn test_categories() {
        let cats = categories();
        assert!(cats.contains(&"Connectors"));
        assert!(cats.contains(&"System"));
        assert!(cats.contains(&"API-Keys"));
        assert!(cats.contains(&"Agents"));
        assert!(cats.contains(&"Daemon"));
        assert!(!cats.contains(&"AI"));
    }

    #[test]
    fn test_keys_in_category() {
        let keys = keys_in_category("System");
        assert!(keys.iter().any(|d| d.key == "system.debug_level"));
        // system.max_agents moved to "Daemon" category
        assert!(!keys.iter().any(|d| d.key == "system.max_agents"));
    }

    #[test]
    fn test_daemon_keys_in_category() {
        let keys = keys_in_category("Daemon");
        // 42 daemon keys + 1 alias (system.max_agents) + 2 streaming keys = 45 total
        assert_eq!(keys.len(), 45);
        assert!(keys.iter().any(|d| d.key == "system.max_agents"));
        assert!(keys.iter().any(|d| d.key == "daemon.dag.max_concurrent_agents"));
        assert!(keys.iter().any(|d| d.key == "daemon.orchestrator.prompt_recent_messages"));
        assert!(keys.iter().any(|d| d.key == "daemon.execution.max_rounds"));
        assert!(keys.iter().any(|d| d.key == "daemon.security.max_input_length"));
        // New keys
        assert!(keys.iter().any(|d| d.key == "daemon.orchestrator.summary_min_new_older_messages"));
        assert!(keys.iter().any(|d| d.key == "daemon.orchestrator.msg_trunc_chars"));
        assert!(keys.iter().any(|d| d.key == "daemon.orchestrator.extract_max_daily_cost_usd"));
        assert!(keys.iter().any(|d| d.key == "daemon.execution.max_tools_per_round"));
        assert!(keys.iter().any(|d| d.key == "daemon.execution.lead_max_tools_per_round"));
        assert!(keys.iter().any(|d| d.key == "daemon.dag.node_timeout_secs"));
        assert!(keys.iter().any(|d| d.key == "daemon.dag.max_retries_per_node"));
        assert!(keys.iter().any(|d| d.key == "daemon.server.heartbeat_interval_secs"));
        assert!(keys.iter().any(|d| d.key == "daemon.server.embedding_batch_size"));
        // Memory lifecycle keys
        assert!(keys.iter().any(|d| d.key == "daemon.orchestrator.task_extract_enabled"));
        assert!(keys.iter().any(|d| d.key == "daemon.orchestrator.task_extract_max_daily_cost_usd"));
        assert!(keys.iter().any(|d| d.key == "daemon.orchestrator.task_extract_min_content_len"));
        assert!(keys.iter().any(|d| d.key == "daemon.orchestrator.supersession_distance_threshold"));
        assert!(keys.iter().any(|d| d.key == "daemon.orchestrator.fts_jaccard_threshold"));
        assert!(keys.iter().any(|d| d.key == "daemon.orchestrator.decay_poll_interval_secs"));
        assert!(keys.iter().any(|d| d.key == "daemon.orchestrator.decay_half_life_days"));
        assert!(keys.iter().any(|d| d.key == "daemon.orchestrator.decay_min_importance"));
        assert!(keys.iter().any(|d| d.key == "daemon.orchestrator.decay_soft_cap"));
        // Streaming keys
        assert!(keys.iter().any(|d| d.key == "daemon.server.stream_chunk_delay_ms"));
        assert!(keys.iter().any(|d| d.key == "daemon.server.stream_chunk_words"));
        assert!(keys.iter().all(|d| d.backend == ConfigBackend::DaemonToml));
    }

    #[test]
    fn test_api_keys_in_category() {
        let keys = keys_in_category("API-Keys");
        assert_eq!(keys.len(), 12);
        assert!(keys.iter().any(|d| d.key == "ai.anthropic.api_key"));
        assert!(keys.iter().any(|d| d.key == "ai.openai.api_key"));
        assert!(keys.iter().any(|d| d.key == "ai.ollama.base_url"));
        assert!(keys.iter().any(|d| d.key == "ai.claude_code.discovery"));
        assert!(keys.iter().any(|d| d.key == "ai.codex.cli_enabled"));
        assert!(keys.iter().all(|d| d.backend == ConfigBackend::LlmToml));
    }

    #[test]
    fn test_agents_in_category() {
        let keys = keys_in_category("Agents");
        assert_eq!(keys.len(), 2);
        assert!(keys.iter().any(|d| d.key == "ai.default_model"));
        assert!(keys.iter().any(|d| d.key == "ai.fallback_models"));
        assert!(keys.iter().all(|d| d.backend == ConfigBackend::LlmToml));
    }

    #[test]
    fn test_ai_key_lookup() {
        let def = lookup("ai.anthropic.api_key").unwrap();
        assert!(def.sensitive);
        assert_eq!(def.backend, ConfigBackend::LlmToml);
        assert_eq!(def.category, "API-Keys");

        let def = lookup("ai.anthropic.enabled").unwrap();
        assert!(!def.sensitive);
        assert!(matches!(def.kind, ConfigKind::Bool));
    }

    #[test]
    fn test_subcategories() {
        let api_subs = subcategories_in_category("API-Keys");
        assert_eq!(api_subs.len(), 5);
        assert!(api_subs.contains(&"Anthropic"));
        assert!(api_subs.contains(&"OpenAI"));
        assert!(api_subs.contains(&"Ollama"));
        assert!(api_subs.contains(&"Claude Code"));
        assert!(api_subs.contains(&"Codex"));

        let agent_subs = subcategories_in_category("Agents");
        assert_eq!(agent_subs.len(), 1);
        assert!(agent_subs.contains(&"Orchestrator"));

        // Connectors/System have no subcategories
        assert!(subcategories_in_category("Connectors").is_empty());
        assert!(subcategories_in_category("System").is_empty());

        // Daemon subcategories
        let daemon_subs = subcategories_in_category("Daemon");
        assert_eq!(daemon_subs.len(), 5);
        assert!(daemon_subs.contains(&"Orchestrator"));
        assert!(daemon_subs.contains(&"Execution"));
        assert!(daemon_subs.contains(&"DAG"));
        assert!(daemon_subs.contains(&"Security"));
        assert!(daemon_subs.contains(&"Server"));
    }

    #[test]
    fn test_keys_in_subcategory() {
        let anthropic = keys_in_subcategory("API-Keys", "Anthropic");
        assert_eq!(anthropic.len(), 2);
        assert!(anthropic.iter().any(|d| d.key == "ai.anthropic.enabled"));
        assert!(anthropic.iter().any(|d| d.key == "ai.anthropic.api_key"));

        let orch = keys_in_subcategory("Agents", "Orchestrator");
        assert_eq!(orch.len(), 2);
        assert!(orch.iter().any(|d| d.key == "ai.default_model"));
        assert!(orch.iter().any(|d| d.key == "ai.fallback_models"));
    }

    #[test]
    fn test_ai_validate_bool() {
        assert!(validate("ai.anthropic.enabled", "true").is_ok());
        assert!(validate("ai.anthropic.enabled", "potato").is_err());
    }

    #[test]
    fn test_mask_value() {
        assert_eq!(mask_value("abcdef5678"), "****5678");
        assert_eq!(mask_value("abc"), "****");
        assert_eq!(mask_value(""), "****");
    }

    #[test]
    fn test_suggest_key() {
        let suggestions = suggest_key("telegram");
        assert!(suggestions.contains(&"telegram.token"));
        assert!(suggestions.contains(&"telegram.enabled"));

        let suggestions = suggest_key("debug");
        assert!(suggestions.contains(&"system.debug_level"));
    }

    #[test]
    fn test_daemon_key_lookup() {
        let def = lookup("daemon.dag.max_concurrent_agents").unwrap();
        assert_eq!(def.backend, ConfigBackend::DaemonToml);
        assert_eq!(def.category, "Daemon");
        assert_eq!(def.default, Some("3"));

        let def = lookup("system.max_agents").unwrap();
        assert_eq!(def.backend, ConfigBackend::DaemonToml);

        let def = lookup("daemon.orchestrator.prompt_recent_messages").unwrap();
        assert_eq!(def.default, Some("40"));
    }

    #[test]
    fn test_validate_daemon_int_range() {
        assert!(validate("daemon.dag.max_concurrent_agents", "3").is_ok());
        assert!(validate("daemon.dag.max_concurrent_agents", "1").is_ok());
        assert!(validate("daemon.dag.max_concurrent_agents", "32").is_ok());
        assert!(validate("daemon.dag.max_concurrent_agents", "0").is_err());
        assert!(validate("daemon.dag.max_concurrent_agents", "99").is_err());
    }

    #[test]
    fn test_daemon_server_keys() {
        let server_keys = keys_in_subcategory("Daemon", "Server");
        assert_eq!(server_keys.len(), 10);
        assert!(server_keys.iter().any(|d| d.key == "daemon.server.heartbeat_interval_secs"));
        assert!(server_keys.iter().any(|d| d.key == "daemon.server.sse_keep_alive_secs"));
        assert!(server_keys.iter().any(|d| d.key == "daemon.server.event_broadcaster_capacity"));
        assert!(server_keys.iter().any(|d| d.key == "daemon.server.wake_channel_capacity"));
        assert!(server_keys.iter().any(|d| d.key == "daemon.server.cleanup_interval_secs"));
        assert!(server_keys.iter().any(|d| d.key == "daemon.server.stale_timeout_secs"));
        assert!(server_keys.iter().any(|d| d.key == "daemon.server.embedding_poll_interval_secs"));
        assert!(server_keys.iter().any(|d| d.key == "daemon.server.embedding_batch_size"));
        assert!(server_keys.iter().all(|d| d.backend == ConfigBackend::DaemonToml));
    }

    #[test]
    fn test_validate_new_daemon_keys() {
        // Server keys
        assert!(validate("daemon.server.heartbeat_interval_secs", "5").is_ok());
        assert!(validate("daemon.server.heartbeat_interval_secs", "0").is_err());
        assert!(validate("daemon.server.heartbeat_interval_secs", "301").is_err());
        assert!(validate("daemon.server.embedding_batch_size", "50").is_ok());
        assert!(validate("daemon.server.embedding_batch_size", "0").is_err());

        // Orchestrator keys
        assert!(validate("daemon.orchestrator.summary_min_new_older_messages", "12").is_ok());
        assert!(validate("daemon.orchestrator.extract_every_n_turns", "5").is_ok());

        // Execution keys
        assert!(validate("daemon.execution.max_tools_per_round", "5").is_ok());
        assert!(validate("daemon.execution.max_tools_per_round", "0").is_err());

        // DAG keys
        assert!(validate("daemon.dag.node_timeout_secs", "300").is_ok());
        assert!(validate("daemon.dag.max_retries_per_node", "0").is_ok());
        assert!(validate("daemon.dag.max_retries_per_node", "11").is_err());
    }

    #[test]
    fn test_validate_dynamic_connector() {
        assert!(validate("wechat.enabled", "true").is_ok());
        assert!(validate("wechat.enabled", "potato").is_err());
        assert!(validate("wechat.token", "anything").is_ok());
        assert!(validate("wechat.xyz", "anything").is_err());
    }

    // -- Anthropic setup-token validation --

    #[test]
    fn test_validate_anthropic_setup_token_valid() {
        // 80+ chars with correct prefix
        let token = format!("sk-ant-oat01-{}", "a".repeat(67));
        assert!(validate_anthropic_setup_token(&token).is_ok());
    }

    #[test]
    fn test_validate_anthropic_setup_token_bad_prefix() {
        let token = format!("sk-openai-{}", "a".repeat(80));
        let err = validate_anthropic_setup_token(&token).unwrap_err();
        assert!(err.contains("sk-ant-oat01-"));
    }

    #[test]
    fn test_validate_anthropic_setup_token_too_short() {
        let token = "sk-ant-oat01-short";
        let err = validate_anthropic_setup_token(token).unwrap_err();
        assert!(err.contains("too short"));
    }

    #[test]
    fn test_validate_anthropic_setup_token_empty() {
        assert!(validate_anthropic_setup_token("").is_err());
        assert!(validate_anthropic_setup_token("   ").is_err());
    }

    // -- OpenAI API key validation --

    #[test]
    fn test_validate_openai_api_key_valid() {
        let key = format!("sk-{}", "x".repeat(40));
        assert!(validate_openai_api_key(&key).is_ok());
    }

    #[test]
    fn test_validate_openai_api_key_bad_prefix() {
        let key = format!("pk-{}", "x".repeat(40));
        let err = validate_openai_api_key(&key).unwrap_err();
        assert!(err.contains("sk-"));
    }

    #[test]
    fn test_validate_openai_api_key_too_short() {
        let key = "sk-short";
        let err = validate_openai_api_key(key).unwrap_err();
        assert!(err.contains("too short"));
    }

    #[test]
    fn test_validate_openai_rejects_anthropic_key() {
        let key = format!("sk-ant-oat01-{}", "a".repeat(67));
        let err = validate_openai_api_key(&key).unwrap_err();
        assert!(err.contains("Anthropic"));
    }

    // -- Anthropic API key validation --

    #[test]
    fn test_validate_anthropic_api_key_valid() {
        let key = format!("sk-ant-api03-{}", "x".repeat(27));
        assert!(validate_anthropic_api_key(&key).is_ok());
    }

    #[test]
    fn test_validate_anthropic_api_key_rejects_oat() {
        let key = format!("sk-ant-oat01-{}", "a".repeat(67));
        let err = validate_anthropic_api_key(&key).unwrap_err();
        assert!(err.contains("setup-token"));
    }

    #[test]
    fn test_validate_anthropic_api_key_rejects_openai() {
        let key = format!("sk-{}", "x".repeat(40));
        let err = validate_anthropic_api_key(&key).unwrap_err();
        assert!(err.contains("sk-ant-"));
    }

    #[test]
    fn test_validate_anthropic_api_key_too_short() {
        let key = "sk-ant-api03-abc";
        let err = validate_anthropic_api_key(key).unwrap_err();
        assert!(err.contains("too short"));
    }

    #[test]
    fn test_validate_anthropic_api_key_empty() {
        assert!(validate_anthropic_api_key("").is_err());
        assert!(validate_anthropic_api_key("   ").is_err());
    }

    // -- validate_key_for_provider dispatch --

    #[test]
    fn test_validate_key_for_provider_dispatches() {
        // anthropic → API key validator
        let key = format!("sk-ant-api03-{}", "x".repeat(27));
        assert!(validate_key_for_provider("anthropic", &key).is_ok());

        // anthropic rejects oat
        let oat = format!("sk-ant-oat01-{}", "a".repeat(67));
        assert!(validate_key_for_provider("anthropic", &oat).is_err());

        // openai → OpenAI validator
        let oai = format!("sk-{}", "x".repeat(40));
        assert!(validate_key_for_provider("openai", &oai).is_ok());

        // ollama → always Ok
        assert!(validate_key_for_provider("ollama", "anything").is_ok());

        // unknown → always Ok
        assert!(validate_key_for_provider("unknown_provider", "anything").is_ok());
    }
}

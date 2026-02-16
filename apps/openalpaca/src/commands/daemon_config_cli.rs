//! Bridge between the `config` CLI subcommand and `config/daemon.toml`.
//!
//! Reads/writes daemon configuration values directly via `toml` serde,
//! without requiring a running daemon. Mirrors the `ai_config` module pattern.

use anyhow::{Context, Result};
use std::path::PathBuf;

/// Schema key → TOML path mapping.
///
/// Each entry maps a dotted schema key (as registered in `config_schema.rs`)
/// to the TOML section path and field name within `daemon.toml`.
struct TomlMapping {
    /// Schema key, e.g. `"daemon.dag.max_concurrent_agents"`.
    schema_key: &'static str,
    /// TOML dotted section path, e.g. `["execution", "dag"]`.
    section: &'static [&'static str],
    /// Field name within the section, e.g. `"max_concurrent_agents"`.
    field: &'static str,
}

/// All known schema key → TOML path mappings.
static MAPPINGS: &[TomlMapping] = &[
    // Orchestrator: Memory
    TomlMapping {
        schema_key: "daemon.orchestrator.prompt_recent_messages",
        section: &["orchestrator", "memory"],
        field: "prompt_recent_messages",
    },
    TomlMapping {
        schema_key: "daemon.orchestrator.summary_max_chars",
        section: &["orchestrator", "memory"],
        field: "summary_max_chars",
    },
    // Orchestrator: Costs
    TomlMapping {
        schema_key: "daemon.orchestrator.summary_max_daily_cost_usd",
        section: &["orchestrator", "costs"],
        field: "summary_max_daily_cost_usd",
    },
    // Orchestrator: Prompt Budgets
    TomlMapping {
        schema_key: "daemon.orchestrator.identity_budget",
        section: &["orchestrator", "prompt_budgets"],
        field: "identity_budget",
    },
    TomlMapping {
        schema_key: "daemon.orchestrator.user_profile_budget",
        section: &["orchestrator", "prompt_budgets"],
        field: "user_profile_budget",
    },
    // Execution: Agent Defaults
    TomlMapping {
        schema_key: "daemon.execution.max_rounds",
        section: &["execution", "agent_defaults"],
        field: "max_rounds",
    },
    TomlMapping {
        schema_key: "daemon.execution.max_cost",
        section: &["execution", "agent_defaults"],
        field: "max_cost",
    },
    // Execution: Lead Agent Defaults
    TomlMapping {
        schema_key: "daemon.execution.lead_max_rounds",
        section: &["execution", "lead_agent_defaults"],
        field: "max_rounds",
    },
    TomlMapping {
        schema_key: "daemon.execution.lead_max_cost",
        section: &["execution", "lead_agent_defaults"],
        field: "max_cost",
    },
    // DAG
    TomlMapping {
        schema_key: "daemon.dag.max_concurrent_agents",
        section: &["execution", "dag"],
        field: "max_concurrent_agents",
    },
    TomlMapping {
        schema_key: "daemon.dag.total_timeout_secs",
        section: &["execution", "dag"],
        field: "total_timeout_secs",
    },
    // Orchestrator: Memory (cont.)
    TomlMapping {
        schema_key: "daemon.orchestrator.summary_min_new_older_messages",
        section: &["orchestrator", "memory"],
        field: "summary_min_new_older_messages",
    },
    TomlMapping {
        schema_key: "daemon.orchestrator.msg_trunc_chars",
        section: &["orchestrator", "memory"],
        field: "msg_trunc_chars",
    },
    // Orchestrator: Costs (cont.)
    TomlMapping {
        schema_key: "daemon.orchestrator.extract_max_daily_cost_usd",
        section: &["orchestrator", "costs"],
        field: "extract_max_daily_cost_usd",
    },
    TomlMapping {
        schema_key: "daemon.orchestrator.extract_every_n_turns",
        section: &["orchestrator", "costs"],
        field: "extract_every_n_turns",
    },
    TomlMapping {
        schema_key: "daemon.orchestrator.extract_min_content_len",
        section: &["orchestrator", "costs"],
        field: "extract_min_content_len",
    },
    // Orchestrator: Task Extraction
    TomlMapping {
        schema_key: "daemon.orchestrator.task_extract_enabled",
        section: &["orchestrator", "costs"],
        field: "task_extract_enabled",
    },
    TomlMapping {
        schema_key: "daemon.orchestrator.task_extract_max_daily_cost_usd",
        section: &["orchestrator", "costs"],
        field: "task_extract_max_daily_cost_usd",
    },
    TomlMapping {
        schema_key: "daemon.orchestrator.task_extract_min_content_len",
        section: &["orchestrator", "costs"],
        field: "task_extract_min_content_len",
    },
    // Orchestrator: Supersession
    TomlMapping {
        schema_key: "daemon.orchestrator.supersession_distance_threshold",
        section: &["orchestrator", "memory"],
        field: "supersession_distance_threshold",
    },
    TomlMapping {
        schema_key: "daemon.orchestrator.fts_jaccard_threshold",
        section: &["orchestrator", "memory"],
        field: "fts_jaccard_threshold",
    },
    // Orchestrator: Decay
    TomlMapping {
        schema_key: "daemon.orchestrator.decay_poll_interval_secs",
        section: &["orchestrator", "memory", "decay"],
        field: "poll_interval_secs",
    },
    TomlMapping {
        schema_key: "daemon.orchestrator.decay_half_life_days",
        section: &["orchestrator", "memory", "decay"],
        field: "half_life_days",
    },
    TomlMapping {
        schema_key: "daemon.orchestrator.decay_min_importance",
        section: &["orchestrator", "memory", "decay"],
        field: "min_importance",
    },
    TomlMapping {
        schema_key: "daemon.orchestrator.decay_soft_cap",
        section: &["orchestrator", "memory", "decay"],
        field: "soft_cap",
    },
    // Execution: Agent Defaults (cont.)
    TomlMapping {
        schema_key: "daemon.execution.max_tools_per_round",
        section: &["execution", "agent_defaults"],
        field: "max_tools_per_round",
    },
    TomlMapping {
        schema_key: "daemon.execution.max_tool_runtime_secs",
        section: &["execution", "agent_defaults"],
        field: "max_tool_runtime_secs",
    },
    // Execution: Lead Agent Defaults (cont.)
    TomlMapping {
        schema_key: "daemon.execution.lead_max_tools_per_round",
        section: &["execution", "lead_agent_defaults"],
        field: "max_tools_per_round",
    },
    TomlMapping {
        schema_key: "daemon.execution.lead_max_tool_runtime_secs",
        section: &["execution", "lead_agent_defaults"],
        field: "max_tool_runtime_secs",
    },
    // DAG (cont.)
    TomlMapping {
        schema_key: "daemon.dag.node_timeout_secs",
        section: &["execution", "dag"],
        field: "node_timeout_secs",
    },
    TomlMapping {
        schema_key: "daemon.dag.max_retries_per_node",
        section: &["execution", "dag"],
        field: "max_retries_per_node",
    },
    TomlMapping {
        schema_key: "daemon.dag.replan_after_every_n_nodes",
        section: &["execution", "dag"],
        field: "replan_after_every_n_nodes",
    },
    TomlMapping {
        schema_key: "daemon.dag.max_replans",
        section: &["execution", "dag"],
        field: "max_replans",
    },
    // Security
    TomlMapping {
        schema_key: "daemon.security.max_input_length",
        section: &["security"],
        field: "max_input_length",
    },
    // Server
    TomlMapping {
        schema_key: "daemon.server.heartbeat_interval_secs",
        section: &["server"],
        field: "heartbeat_interval_secs",
    },
    TomlMapping {
        schema_key: "daemon.server.sse_keep_alive_secs",
        section: &["server"],
        field: "sse_keep_alive_secs",
    },
    TomlMapping {
        schema_key: "daemon.server.event_broadcaster_capacity",
        section: &["server"],
        field: "event_broadcaster_capacity",
    },
    TomlMapping {
        schema_key: "daemon.server.wake_channel_capacity",
        section: &["server"],
        field: "wake_channel_capacity",
    },
    // Server: Chat Streams
    TomlMapping {
        schema_key: "daemon.server.cleanup_interval_secs",
        section: &["server", "chat_streams"],
        field: "cleanup_interval_secs",
    },
    TomlMapping {
        schema_key: "daemon.server.stale_timeout_secs",
        section: &["server", "chat_streams"],
        field: "stale_timeout_secs",
    },
    TomlMapping {
        schema_key: "daemon.server.stream_chunk_delay_ms",
        section: &["server", "chat_streams"],
        field: "stream_chunk_delay_ms",
    },
    TomlMapping {
        schema_key: "daemon.server.stream_chunk_words",
        section: &["server", "chat_streams"],
        field: "stream_chunk_words",
    },
    // Server: Embedding Indexer
    TomlMapping {
        schema_key: "daemon.server.embedding_poll_interval_secs",
        section: &["server", "embedding_indexer"],
        field: "poll_interval_secs",
    },
    TomlMapping {
        schema_key: "daemon.server.embedding_batch_size",
        section: &["server", "embedding_indexer"],
        field: "batch_size",
    },
    // Alias: system.max_agents → same as daemon.dag.max_concurrent_agents
    TomlMapping {
        schema_key: "system.max_agents",
        section: &["execution", "dag"],
        field: "max_concurrent_agents",
    },
];

/// Find the TOML mapping for a given schema key.
fn find_mapping(schema_key: &str) -> Option<&'static TomlMapping> {
    MAPPINGS.iter().find(|m| m.schema_key == schema_key)
}

/// Returns the path to `config/daemon.toml`.
///
/// Resolution order:
/// 1. `OPENALPACA_CONFIG_DIR` env var override
/// 2. `app_dir()/config/daemon.toml` (writable, created if needed)
/// 3. Walk up from CWD looking for `config/daemon.toml` (dev/repo fallback)
pub fn daemon_config_path() -> Result<PathBuf> {
    // 1. OPENALPACA_CONFIG_DIR override
    if let Ok(dir) = std::env::var("OPENALPACA_CONFIG_DIR") {
        let p = PathBuf::from(&dir);
        if p.is_dir() {
            return Ok(p.join("daemon.toml"));
        }
    }
    // 2. Writable app_dir/config/
    if let Ok(app) = openalpaca_storage::paths::app_dir() {
        let p = app.join("config").join("daemon.toml");
        if p.exists() {
            return Ok(p);
        }
    }
    // 3. Walk up from CWD (dev/repo)
    let cwd = std::env::current_dir().context("Failed to get current directory")?;
    Ok(cwd.join("config").join("daemon.toml"))
}

/// Load config as a `toml::Value` tree (for reading individual fields).
fn load_toml_value() -> Result<toml::Value> {
    let path = daemon_config_path()?;
    if !path.exists() {
        // Return empty table
        return Ok(toml::Value::Table(toml::map::Map::new()));
    }
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    toml::from_str(&content)
        .with_context(|| format!("Failed to parse {}", path.display()))
}

/// Navigate into a TOML value tree by section path, returning the leaf table.
fn navigate_to_section<'a>(
    root: &'a toml::Value,
    section: &[&str],
) -> Option<&'a toml::Value> {
    let mut current = root;
    for key in section {
        current = current.get(key)?;
    }
    Some(current)
}

/// Navigate into a mutable TOML value tree, creating tables as needed.
fn navigate_to_section_mut<'a>(
    root: &'a mut toml::Value,
    section: &[&str],
) -> &'a mut toml::Value {
    let mut current = root;
    for key in section {
        // Ensure each level is a table
        if !current.is_table() {
            *current = toml::Value::Table(toml::map::Map::new());
        }
        let table = current.as_table_mut().unwrap();
        if !table.contains_key(*key) {
            table.insert(key.to_string(), toml::Value::Table(toml::map::Map::new()));
        }
        current = table.get_mut(*key).unwrap();
    }
    current
}

/// Get a single daemon config value by schema key.
pub fn get_daemon_value(key: &str) -> Result<Option<String>> {
    let mapping = find_mapping(key).ok_or_else(|| {
        anyhow::anyhow!("Unknown daemon config key '{}'", key)
    })?;

    let root = load_toml_value()?;
    let section = match navigate_to_section(&root, mapping.section) {
        Some(s) => s,
        None => return Ok(None),
    };

    match section.get(mapping.field) {
        Some(val) => Ok(Some(toml_value_to_string(val))),
        None => Ok(None),
    }
}

/// Set a single daemon config value by schema key.
pub fn set_daemon_value(key: &str, value: &str) -> Result<()> {
    let mapping = find_mapping(key).ok_or_else(|| {
        anyhow::anyhow!("Unknown daemon config key '{}'", key)
    })?;

    let path = daemon_config_path()?;

    // Load existing config or start with empty table
    let mut root = if path.exists() {
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read {}", path.display()))?;
        toml::from_str::<toml::Value>(&content)
            .with_context(|| format!("Failed to parse {}", path.display()))?
    } else {
        toml::Value::Table(toml::map::Map::new())
    };

    // Navigate to the correct section and set the value
    let section = navigate_to_section_mut(&mut root, mapping.section);
    let table = section.as_table_mut().ok_or_else(|| {
        anyhow::anyhow!("TOML section is not a table")
    })?;

    // Infer TOML type: try integer first, then float, then string
    let toml_val = string_to_toml_value(value);
    table.insert(mapping.field.to_string(), toml_val);

    // Write back
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("Failed to create config directory")?;
    }
    let output = toml::to_string_pretty(&root)
        .context("Failed to serialize daemon config")?;
    std::fs::write(&path, output)
        .with_context(|| format!("Failed to write {}", path.display()))?;

    Ok(())
}

/// Delete a daemon config value by schema key.
///
/// Removes the field from daemon.toml. The runtime will fall back to the default.
pub fn delete_daemon_value(key: &str) -> Result<()> {
    let mapping = find_mapping(key).ok_or_else(|| {
        anyhow::anyhow!("Unknown daemon config key '{}'", key)
    })?;

    let path = daemon_config_path()?;
    if !path.exists() {
        return Ok(()); // nothing to delete
    }

    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    let mut root: toml::Value = toml::from_str(&content)
        .with_context(|| format!("Failed to parse {}", path.display()))?;

    // Navigate to the section and remove the field
    if let Some(section) = navigate_to_section_mut_opt(&mut root, mapping.section) {
        if let Some(table) = section.as_table_mut() {
            table.remove(mapping.field);
        }
    }

    let output = toml::to_string_pretty(&root)
        .context("Failed to serialize daemon config")?;
    std::fs::write(&path, output)
        .with_context(|| format!("Failed to write {}", path.display()))?;

    Ok(())
}

/// List all currently set daemon config values.
///
/// Returns `Vec<(key, value, kind)>` matching the format used by `ai_config::list_ai_entries`.
pub fn list_daemon_entries() -> Result<Vec<(String, String, String)>> {
    let root = load_toml_value()?;
    let mut entries = Vec::new();

    for mapping in MAPPINGS {
        // Skip alias keys to avoid duplicates
        if mapping.schema_key == "system.max_agents" {
            continue;
        }

        if let Some(section) = navigate_to_section(&root, mapping.section) {
            if let Some(val) = section.get(mapping.field) {
                let kind = match val {
                    toml::Value::Integer(_) => "int",
                    toml::Value::Float(_) => "float",
                    toml::Value::Boolean(_) => "bool",
                    _ => "string",
                };
                entries.push((
                    mapping.schema_key.to_string(),
                    toml_value_to_string(val),
                    kind.to_string(),
                ));
            }
        }
    }

    Ok(entries)
}

/// Navigate into a mutable TOML tree, but return None if any level is missing.
fn navigate_to_section_mut_opt<'a>(
    root: &'a mut toml::Value,
    section: &[&str],
) -> Option<&'a mut toml::Value> {
    let mut current = root;
    for key in section {
        current = current.as_table_mut()?.get_mut(*key)?;
    }
    Some(current)
}

/// Convert a TOML value to its string representation.
fn toml_value_to_string(val: &toml::Value) -> String {
    match val {
        toml::Value::String(s) => s.clone(),
        toml::Value::Integer(n) => n.to_string(),
        toml::Value::Float(f) => format!("{}", f),
        toml::Value::Boolean(b) => b.to_string(),
        other => other.to_string(),
    }
}

/// Convert a string to an appropriate TOML value.
///
/// Tries integer first, then float, then falls back to string.
fn string_to_toml_value(s: &str) -> toml::Value {
    // Try integer
    if let Ok(n) = s.parse::<i64>() {
        return toml::Value::Integer(n);
    }
    // Try float
    if let Ok(f) = s.parse::<f64>() {
        return toml::Value::Float(f);
    }
    // Try boolean
    match s {
        "true" | "yes" | "1" => return toml::Value::Boolean(true),
        "false" | "no" | "0" => return toml::Value::Boolean(false),
        _ => {}
    }
    // Fallback to string
    toml::Value::String(s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_mapping_known_key() {
        let m = find_mapping("daemon.dag.max_concurrent_agents").unwrap();
        assert_eq!(m.section, &["execution", "dag"]);
        assert_eq!(m.field, "max_concurrent_agents");
    }

    #[test]
    fn test_find_mapping_alias() {
        let m = find_mapping("system.max_agents").unwrap();
        assert_eq!(m.section, &["execution", "dag"]);
        assert_eq!(m.field, "max_concurrent_agents");
    }

    #[test]
    fn test_find_mapping_unknown() {
        assert!(find_mapping("unknown.key").is_none());
    }

    #[test]
    fn test_string_to_toml_value_int() {
        assert_eq!(string_to_toml_value("42"), toml::Value::Integer(42));
    }

    #[test]
    fn test_string_to_toml_value_float() {
        assert_eq!(string_to_toml_value("3.14"), toml::Value::Float(3.14));
    }

    #[test]
    fn test_string_to_toml_value_bool() {
        assert_eq!(string_to_toml_value("true"), toml::Value::Boolean(true));
        assert_eq!(string_to_toml_value("false"), toml::Value::Boolean(false));
    }

    #[test]
    fn test_string_to_toml_value_string() {
        assert_eq!(
            string_to_toml_value("hello"),
            toml::Value::String("hello".to_string())
        );
    }

    #[test]
    fn test_toml_value_to_string() {
        assert_eq!(toml_value_to_string(&toml::Value::Integer(42)), "42");
        assert_eq!(toml_value_to_string(&toml::Value::Float(0.5)), "0.5");
        assert_eq!(
            toml_value_to_string(&toml::Value::String("hi".to_string())),
            "hi"
        );
    }

    #[test]
    fn test_navigate_to_section() {
        let toml_str = r#"
[execution.dag]
max_concurrent_agents = 5
"#;
        let root: toml::Value = toml::from_str(toml_str).unwrap();
        let section = navigate_to_section(&root, &["execution", "dag"]).unwrap();
        assert_eq!(
            section.get("max_concurrent_agents").unwrap(),
            &toml::Value::Integer(5)
        );
    }

    #[test]
    fn test_navigate_to_section_missing() {
        let root: toml::Value = toml::from_str("").unwrap();
        assert!(navigate_to_section(&root, &["execution", "dag"]).is_none());
    }
}

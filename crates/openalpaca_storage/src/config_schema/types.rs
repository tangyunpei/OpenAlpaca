//! Config schema types: backend, kind, and key definition.

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

//! Validation, normalization, categorization, and key suggestion logic.

use super::{ConfigBackend, ConfigKeyDef, ConfigKind, CONFIG_KEYS};

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
    // Count/slice by chars, not bytes: a value with multi-byte characters
    // would panic on a non-char-boundary byte slice.
    let char_count = value.chars().count();
    if char_count <= 4 {
        "****".to_string()
    } else {
        let suffix: String = value.chars().skip(char_count - 4).collect();
        format!("****{}", suffix)
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

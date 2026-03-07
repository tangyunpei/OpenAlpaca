//! Non-interactive config subcommand handlers: set, get, list, reset.

use anyhow::Result;
use console::style;
use dialoguer::{Confirm, theme::ColorfulTheme};
use openalpaca_storage::config_schema::{self, ConfigBackend};
use openalpaca_storage::{ConfigRepository, Database};
use serde::Serialize;
use std::collections::HashMap;

use super::ai_config;
use super::daemon_config_cli;
use crate::output::OutputFormat;

#[derive(Serialize)]
pub(super) struct ConfigEntry {
    pub key: String,
    pub value: String,
    pub kind: String,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

pub(super) fn cmd_set(repo: &ConfigRepository, key: &str, value: &str) -> Result<()> {
    let def = config_schema::lookup(key);

    if def.is_none() {
        eprintln!("Error: Unknown config key '{}'", key);
        let suggestions = config_schema::suggest_key(key);
        if !suggestions.is_empty() {
            eprintln!("Did you mean: {}?", suggestions.join(", "));
        }
        std::process::exit(1);
    }

    let def = def.unwrap();
    if let Err(e) = config_schema::validate(key, value) {
        eprintln!("Error: Invalid value for '{}': {}", key, e);
        std::process::exit(1);
    }
    let normalized = config_schema::normalize(key, value);

    match def.backend {
        ConfigBackend::LlmToml => ai_config::set_ai_value(key, &normalized)?,
        ConfigBackend::DaemonToml => daemon_config_cli::set_daemon_value(key, &normalized)?,
        ConfigBackend::SystemConfig => {
            let kind = def.kind.as_db_kind();
            repo.set(key, &normalized, kind)?;
        }
    }

    let display = if def.sensitive {
        config_schema::mask_value(&normalized)
    } else {
        normalized
    };
    println!("Config set: {} = {}", key, display);
    Ok(())
}

pub(super) fn cmd_get(repo: &ConfigRepository, key: &str) -> Result<()> {
    let def = config_schema::lookup(key);
    let backend = def
        .as_ref()
        .map(|d| d.backend)
        .unwrap_or(ConfigBackend::SystemConfig);
    let sensitive = def.as_ref().is_some_and(|d| d.sensitive);

    let value = match backend {
        ConfigBackend::LlmToml => ai_config::get_ai_value(key)?,
        ConfigBackend::DaemonToml => daemon_config_cli::get_daemon_value(key)?,
        ConfigBackend::SystemConfig => repo.get(key)?,
    };

    match value {
        Some(val) => {
            if sensitive {
                println!("{}", config_schema::mask_value(&val));
            } else {
                println!("{}", val);
            }
        }
        None => {
            let default = def.and_then(|d| d.default.map(|v| v.to_string()));
            match default {
                Some(val) => println!("{} (default)", val),
                None => {
                    eprintln!("Key not found: {}", key);
                    std::process::exit(1);
                }
            }
        }
    }
    Ok(())
}

pub(super) fn cmd_list(
    repo: &ConfigRepository,
    all: bool,
    format: OutputFormat,
    verbose: bool,
) -> Result<()> {
    // Gather values from all backends
    let db_items = repo.list()?;
    let db_map: HashMap<String, (String, String)> = db_items
        .into_iter()
        .map(|(k, v, kind)| (k, (v, kind)))
        .collect();
    let ai_map: HashMap<String, String> = ai_config::list_ai_entries()
        .unwrap_or_default()
        .into_iter()
        .map(|(k, v, _)| (k, v))
        .collect();
    let daemon_map: HashMap<String, String> = daemon_config_cli::list_daemon_entries()
        .unwrap_or_default()
        .into_iter()
        .map(|(k, v, _)| (k, v))
        .collect();

    // Resolve each key to (display_value, source, is_set)
    let mut resolved: HashMap<String, (String, String, bool)> = HashMap::new();

    for def in config_schema::CONFIG_KEYS {
        let (raw_val, source, is_set) = match def.backend {
            ConfigBackend::LlmToml => {
                if let Some(val) = ai_map.get(def.key) {
                    (val.clone(), "llm.toml", true)
                } else if all {
                    if let Some(d) = def.default {
                        (d.to_string(), "default", false)
                    } else {
                        (String::new(), "—", false)
                    }
                } else {
                    continue;
                }
            }
            ConfigBackend::DaemonToml => {
                if let Some(val) = daemon_map.get(def.key) {
                    (val.clone(), "daemon.toml", true)
                } else if all {
                    if let Some(d) = def.default {
                        (d.to_string(), "default", false)
                    } else {
                        (String::new(), "—", false)
                    }
                } else {
                    continue;
                }
            }
            ConfigBackend::SystemConfig => {
                if let Some((val, _)) = db_map.get(def.key) {
                    (val.clone(), "db", true)
                } else if all {
                    if let Some(d) = def.default {
                        (d.to_string(), "default", false)
                    } else {
                        (String::new(), "—", false)
                    }
                } else {
                    continue;
                }
            }
        };

        let display = if def.sensitive {
            config_schema::mask_value(&raw_val)
        } else {
            raw_val
        };
        resolved.insert(def.key.to_string(), (display, source.to_string(), is_set));
    }

    // Dynamic connector keys from DB not in the static registry
    for (k, (v, _kind)) in &db_map {
        if config_schema::CONFIG_KEYS.iter().any(|d| d.key == k) {
            continue;
        }
        let def = config_schema::lookup(k);
        let sensitive = def.as_ref().is_some_and(|d| d.sensitive);
        let display = if sensitive {
            config_schema::mask_value(v)
        } else {
            v.clone()
        };
        resolved.insert(k.clone(), (display, "db".to_string(), true));
    }

    match format {
        OutputFormat::Json => {
            let entries: Vec<ConfigEntry> = resolved
                .iter()
                .map(|(k, (v, src, _))| {
                    let def = config_schema::lookup(k);
                    ConfigEntry {
                        key: k.clone(),
                        value: v.clone(),
                        kind: def
                            .as_ref()
                            .map_or("string".to_string(), |d| d.kind.as_db_kind().to_string()),
                        source: src.clone(),
                        category: def.as_ref().map(|d| d.category.to_string()),
                        description: def.as_ref().map(|d| d.description.to_string()),
                    }
                })
                .collect();
            println!(
                "{}",
                serde_json::to_string_pretty(&entries).unwrap_or_default()
            );
        }
        OutputFormat::Table => {
            print_grouped_table(&resolved, verbose);
        }
    }

    Ok(())
}

/// Print config values grouped by category and subcategory with colored output.
fn print_grouped_table(
    resolved: &HashMap<String, (String, String, bool)>, // key → (display_value, source, is_set)
    verbose: bool,
) {
    let category_order = ["Agents", "API-Keys", "Daemon", "Connectors", "System"];

    // Compute column widths from actual data
    let key_width = resolved.keys().map(|k| k.len()).max().unwrap_or(30).max(20);
    let val_width = resolved
        .values()
        .map(|(v, _, _)| v.len())
        .max()
        .unwrap_or(20)
        .clamp(10, 36);

    let mut total_keys = 0usize;
    let mut set_keys = 0usize;
    let mut first_category = true;

    for cat in &category_order {
        let subcats = config_schema::subcategories_in_category(cat);

        // Collect keys for this category
        let cat_keys = config_schema::keys_in_category(cat);
        let has_any = cat_keys.iter().any(|d| resolved.contains_key(d.key));
        if !has_any {
            continue;
        }

        // Category header
        if !first_category {
            println!();
        }
        first_category = false;
        let header = format!("━━━ {} ", cat);
        let pad = 60usize.saturating_sub(header.len());
        println!(
            "{}",
            style(format!("{}{}", header, "━".repeat(pad)))
                .bold()
                .cyan()
        );

        if subcats.is_empty() {
            // Flat category (Connectors, System)
            print_category_keys(
                &cat_keys,
                resolved,
                key_width,
                val_width,
                verbose,
                &mut total_keys,
                &mut set_keys,
            );
        } else {
            // Subcategory-based rendering
            for sub in &subcats {
                let sub_keys = config_schema::keys_in_subcategory(cat, sub);
                let has_sub = sub_keys.iter().any(|d| resolved.contains_key(d.key));
                if !has_sub {
                    continue;
                }
                println!("  {}", style(sub).bold());
                print_category_keys(
                    &sub_keys,
                    resolved,
                    key_width,
                    val_width,
                    verbose,
                    &mut total_keys,
                    &mut set_keys,
                );
            }
        }
    }

    // Dynamic connector keys not in the static registry
    let mut dynamic_keys: Vec<&String> = resolved
        .keys()
        .filter(|k| {
            config_schema::CONFIG_KEYS
                .iter()
                .all(|d| d.key != k.as_str())
        })
        .collect();
    dynamic_keys.sort();

    if !dynamic_keys.is_empty() {
        if !first_category {
            println!();
        }
        let header = "━━━ Dynamic ";
        let pad = 60usize.saturating_sub(header.len());
        println!(
            "{}",
            style(format!("{}{}", header, "━".repeat(pad)))
                .bold()
                .cyan()
        );

        for k in &dynamic_keys {
            let (display_val, source, is_set) = &resolved[k.as_str()];
            total_keys += 1;
            if *is_set {
                set_keys += 1;
            }
            let desc = config_schema::lookup(k)
                .map(|d| d.description.to_string())
                .unwrap_or_default();
            print_key_row(
                k,
                display_val,
                *is_set,
                source,
                &desc,
                key_width,
                val_width,
                verbose,
            );
        }
    }

    // Footer
    let default_count = total_keys - set_keys;
    println!();
    println!(
        "{}",
        style(format!(
            "{} keys ({} set, {} default)",
            total_keys, set_keys, default_count
        ))
        .dim()
    );
}

fn print_category_keys(
    defs: &[&config_schema::ConfigKeyDef],
    resolved: &HashMap<String, (String, String, bool)>,
    key_width: usize,
    val_width: usize,
    verbose: bool,
    total: &mut usize,
    set: &mut usize,
) {
    for def in defs {
        if let Some((display_val, source, is_set)) = resolved.get(def.key) {
            *total += 1;
            if *is_set {
                *set += 1;
            }
            print_key_row(
                def.key,
                display_val,
                *is_set,
                source,
                def.description,
                key_width,
                val_width,
                verbose,
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn print_key_row(
    key: &str,
    value: &str,
    is_set: bool,
    source: &str,
    description: &str,
    key_width: usize,
    val_width: usize,
    verbose: bool,
) {
    let styled_val = if !is_set {
        if value.is_empty() {
            style("(not set)".to_string()).yellow().dim()
        } else {
            style(format!("(default: {})", value)).yellow().dim()
        }
    } else {
        style(value.to_string()).green()
    };

    let desc_styled = style(description).dim();

    if verbose {
        let source_styled = style(format!("[{}]", source)).dim();
        println!(
            "    {:<kw$} {:<vw$} {} {}",
            key,
            styled_val,
            desc_styled,
            source_styled,
            kw = key_width,
            vw = val_width,
        );
    } else {
        println!(
            "    {:<kw$} {:<vw$} {}",
            key,
            styled_val,
            desc_styled,
            kw = key_width,
            vw = val_width,
        );
    }
}

pub(super) fn cmd_reset(
    repo: &ConfigRepository,
    db: &Database,
    key: Option<String>,
    factory: bool,
) -> Result<()> {
    if factory {
        let confirm = Confirm::with_theme(&ColorfulTheme::default())
            .with_prompt("DANGER: This will wipe ALL data (agents, memories, config). Continue?")
            .default(false)
            .interact()?;

        if confirm {
            db.factory_reset()?;
            ai_config::clear_ai_config()?;
            daemon_config_cli::clear_daemon_config()?;
            println!("All configuration and data wiped (Factory Reset).");
        } else {
            println!("Cancelled.");
        }
        return Ok(());
    }

    if let Some(k) = key {
        let def = config_schema::lookup(&k);
        let backend = def
            .as_ref()
            .map(|d| d.backend)
            .unwrap_or(ConfigBackend::SystemConfig);
        match backend {
            ConfigBackend::LlmToml => ai_config::delete_ai_value(&k)?,
            ConfigBackend::DaemonToml => daemon_config_cli::delete_daemon_value(&k)?,
            ConfigBackend::SystemConfig => repo.delete(&k)?,
        }
        println!("Key '{}' reset (deleted).", k);
    } else {
        let confirm = Confirm::with_theme(&ColorfulTheme::default())
            .with_prompt("Reset ALL configuration? (agents and data will be preserved)")
            .default(false)
            .interact()?;

        if confirm {
            repo.clear_all()?;
            ai_config::clear_ai_config()?;
            daemon_config_cli::clear_daemon_config()?;
            println!("Config reset (agents and data preserved).");
        } else {
            println!("Cancelled.");
        }
    }
    Ok(())
}

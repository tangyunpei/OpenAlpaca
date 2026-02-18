use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use console::style;
use dialoguer::{Confirm, Input, Password, Select, theme::ColorfulTheme};
use openalpaca_llm::{ClaudeCodeCliProvider, CodexCliProvider};
use openalpaca_storage::{ConfigRepository, Database, config_schema, paths};
use openalpaca_storage::config_schema::ConfigBackend;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

use super::ai_config;
use super::daemon_config_cli;
use crate::client::DaemonClient;
use crate::output::OutputFormat;

#[derive(Args)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub action: Option<ConfigAction>,
}

#[derive(Subcommand)]
pub enum ConfigAction {
    /// Set a configuration value (validates against schema)
    Set { key: String, value: String },
    /// Get a configuration value
    Get { key: String },
    /// List configuration values
    List {
        /// Show all registered keys (including unset, with defaults)
        #[arg(long)]
        all: bool,
        /// Output format
        #[arg(long, value_enum, default_value = "table")]
        format: OutputFormat,
        /// Show source column (db, llm.toml, daemon.toml)
        #[arg(long, short)]
        verbose: bool,
    },
    /// Reset configuration
    Reset {
        /// Key to reset (omit for all config)
        key: Option<String>,
        /// Full factory reset (wipes agents, memories, everything)
        #[arg(long)]
        factory: bool,
    },
}

#[derive(Serialize)]
struct ConfigEntry {
    key: String,
    value: String,
    kind: String,
    source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    category: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
}

pub async fn run(args: ConfigArgs) -> Result<()> {
    let db_path = paths::database_path()?;
    let db = Database::open(&db_path).context("Failed to open database")?;
    let repo = ConfigRepository::new(&db);

    match args.action {
        Some(ConfigAction::Set { key, value }) => cmd_set(&repo, &key, &value)?,
        Some(ConfigAction::Get { key }) => cmd_get(&repo, &key)?,
        Some(ConfigAction::List { all, format, verbose }) => cmd_list(&repo, all, format, verbose)?,
        Some(ConfigAction::Reset { key, factory }) => cmd_reset(&repo, &db, key, factory)?,
        None => run_interactive(&repo, &db)?,
    }

    Ok(())
}

fn cmd_set(repo: &ConfigRepository, key: &str, value: &str) -> Result<()> {
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

fn cmd_get(repo: &ConfigRepository, key: &str) -> Result<()> {
    let def = config_schema::lookup(key);
    let backend = def.as_ref().map(|d| d.backend).unwrap_or(ConfigBackend::SystemConfig);
    let sensitive = def.as_ref().map_or(false, |d| d.sensitive);

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

fn cmd_list(repo: &ConfigRepository, all: bool, format: OutputFormat, verbose: bool) -> Result<()> {
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
        let sensitive = def.as_ref().map_or(false, |d| d.sensitive);
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
    let key_width = resolved
        .keys()
        .map(|k| k.len())
        .max()
        .unwrap_or(30)
        .max(20);
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
        println!("{}", style(format!("{}{}", header, "━".repeat(pad))).bold().cyan());

        if subcats.is_empty() {
            // Flat category (Connectors, System)
            print_category_keys(&cat_keys, resolved, key_width, val_width, verbose, &mut total_keys, &mut set_keys);
        } else {
            // Subcategory-based rendering
            for sub in &subcats {
                let sub_keys = config_schema::keys_in_subcategory(cat, sub);
                let has_sub = sub_keys.iter().any(|d| resolved.contains_key(d.key));
                if !has_sub {
                    continue;
                }
                println!("  {}", style(sub).bold());
                print_category_keys(&sub_keys, resolved, key_width, val_width, verbose, &mut total_keys, &mut set_keys);
            }
        }
    }

    // Dynamic connector keys not in the static registry
    let mut dynamic_keys: Vec<&String> = resolved
        .keys()
        .filter(|k| config_schema::CONFIG_KEYS.iter().all(|d| d.key != k.as_str()))
        .collect();
    dynamic_keys.sort();

    if !dynamic_keys.is_empty() {
        if !first_category {
            println!();
        }
        let header = "━━━ Dynamic ";
        let pad = 60usize.saturating_sub(header.len());
        println!("{}", style(format!("{}{}", header, "━".repeat(pad))).bold().cyan());

        for k in &dynamic_keys {
            let (display_val, source, is_set) = &resolved[k.as_str()];
            total_keys += 1;
            if *is_set {
                set_keys += 1;
            }
            let desc = config_schema::lookup(k)
                .map(|d| d.description.to_string())
                .unwrap_or_default();
            print_key_row(k, display_val, *is_set, source, &desc, key_width, val_width, verbose);
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
            print_key_row(def.key, display_val, *is_set, source, def.description, key_width, val_width, verbose);
        }
    }
}

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

fn cmd_reset(
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
        let backend = def.as_ref().map(|d| d.backend).unwrap_or(ConfigBackend::SystemConfig);
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

// ── Interactive TUI ──────────────────────────────────────────────────────

fn run_interactive(repo: &ConfigRepository, db: &Database) -> Result<()> {
    println!(
        "{}",
        style("OpenAlpaca Configuration Mode").bold().cyan()
    );

    let mut config_map: HashMap<String, String> = HashMap::new();
    for (k, v, _) in repo.list()? {
        config_map.insert(k, v);
    }
    for (k, v, _) in ai_config::list_ai_entries().unwrap_or_default() {
        config_map.insert(k, v);
    }
    for (k, v, _) in daemon_config_cli::list_daemon_entries().unwrap_or_default() {
        config_map.insert(k, v);
    }

    let mut did_write_llm_toml = false;

    loop {
        let mut categories = config_schema::categories();
        let order = ["Agents", "API-Keys", "Daemon", "Connectors", "System"];
        categories.sort_by_key(|c| order.iter().position(|o| o == c).unwrap_or(99));

        let mut menu_items: Vec<String> = categories
            .iter()
            .map(|c| format_category_label(c, &config_map))
            .collect();
        menu_items.push(style("Save & Exit").bold().green().to_string());
        menu_items.push(style("Discard & Exit").red().to_string());
        menu_items.push(style("Reset All Config").red().dim().to_string());

        let selection = Select::with_theme(&ColorfulTheme::default())
            .with_prompt("Main Menu")
            .default(0)
            .items(&menu_items)
            .interact()?;

        if selection < categories.len() {
            let cat = categories[selection];
            let result = match cat {
                "API-Keys" => interactive_api_keys(repo, &mut config_map, &mut did_write_llm_toml)?,
                "Agents" => interactive_agents(repo, &mut config_map)?,
                "Daemon" => interactive_daemon(repo, &mut config_map)?,
                _ => interactive_category(repo, &mut config_map, cat)?,
            };
            if result {
                break; // saved
            }
        } else {
            let offset = selection - categories.len();
            match offset {
                0 => {
                    save_and_exit(repo, &config_map)?;
                    break;
                }
                1 => {
                    if did_write_llm_toml {
                        println!(
                            "{}",
                            style("Note: API key changes were already saved to config/llm.toml. Other changes discarded.").yellow()
                        );
                    } else {
                        println!("{}", style("Changes discarded.").yellow());
                    }
                    break;
                }
                2 => {
                    let input: String = Input::with_theme(&ColorfulTheme::default())
                        .with_prompt("DANGER: Type 'yes' to WIPE ALL configuration")
                        .interact_text()?;

                    if input == "yes" || input == "y" {
                        db.factory_reset()?;
                        let _ = ai_config::clear_ai_config();
                        let _ = daemon_config_cli::clear_daemon_config();
                        config_map.clear();
                        println!(
                            "{}",
                            style("All configuration wiped (Factory Reset). Exiting.").green()
                        );
                        break;
                    } else {
                        println!("{}", style("Cancelled.").red());
                    }
                }
                _ => unreachable!(),
            }
        }
    }

    Ok(())
}

fn format_category_label(category: &str, config_map: &HashMap<String, String>) -> String {
    let keys = config_schema::keys_in_category(category);
    let set_count = keys.iter().filter(|d| config_map.contains_key(d.key)).count();
    let total = keys.len();
    format!(
        "{}  {}",
        style(category).bold(),
        style(format!("({}/{})", set_count, total)).dim()
    )
}

/// Format a config key as a human-readable menu item with value, description, and backend.
///
/// Example output:
///   max retries per node          1          Max retries per DAG node on failure  [daemon.toml]
fn format_key_menu_item(
    def: &config_schema::ConfigKeyDef,
    config: &HashMap<String, String>,
) -> String {
    // Extract leaf key name and humanize: "daemon.dag.max_retries_per_node" → "max retries per node"
    let leaf = def
        .key
        .rsplit('.')
        .next()
        .unwrap_or(def.key)
        .replace('_', " ");

    // Resolve display value
    let (display_val, is_set) = if let Some(v) = config.get(def.key) {
        let val = if def.sensitive {
            config_schema::mask_value(v)
        } else {
            v.clone()
        };
        (val, true)
    } else if let Some(d) = def.default {
        if def.sensitive {
            (format!("(default: {})", config_schema::mask_value(d)), false)
        } else {
            (format!("(default: {})", d), false)
        }
    } else {
        ("(not set)".to_string(), false)
    };

    // Color the value
    let styled_val = if is_set {
        style(display_val).green().to_string()
    } else {
        style(display_val).yellow().dim().to_string()
    };

    // Backend badge
    let backend = match def.backend {
        ConfigBackend::SystemConfig => "db",
        ConfigBackend::LlmToml => "llm.toml",
        ConfigBackend::DaemonToml => "daemon.toml",
    };

    format!(
        "{:<30} {:<28} {}  {}",
        style(&leaf).bold(),
        styled_val,
        style(def.description).dim(),
        style(format!("[{}]", backend)).dim(),
    )
}

fn interactive_category(
    repo: &ConfigRepository,
    config: &mut HashMap<String, String>,
    category: &str,
) -> Result<bool> {
    let keys = config_schema::keys_in_category(category);
    loop {
        let mut items: Vec<String> = keys
            .iter()
            .map(|def| format_key_menu_item(def, config))
            .collect();
        items.push(style("Save & Exit").bold().green().to_string());
        items.push(style("Back").dim().to_string());

        let sel = Select::with_theme(&ColorfulTheme::default())
            .with_prompt(format!("{} Configuration", category))
            .default(0)
            .items(&items)
            .interact()?;

        if sel < keys.len() {
            configure_key(config, keys[sel])?;
        } else if sel == keys.len() {
            save_and_exit(repo, config)?;
            return Ok(true);
        } else {
            return Ok(false);
        }
    }
}

fn interactive_api_keys(
    repo: &ConfigRepository,
    config: &mut HashMap<String, String>,
    did_write_llm_toml: &mut bool,
) -> Result<bool> {
    let providers = config_schema::subcategories_in_category("API-Keys");
    loop {
        let mut items: Vec<String> = providers
            .iter()
            .map(|sub| {
                let keys = config_schema::keys_in_subcategory("API-Keys", sub);
                let set_count = keys.iter().filter(|d| config.contains_key(d.key)).count();
                let total = keys.len();

                // Check for [active]/[disabled] status based on .enabled key
                let status = keys
                    .iter()
                    .find(|d| d.key.ends_with(".enabled"))
                    .and_then(|d| config.get(d.key))
                    .map(|v| {
                        if v == "true" {
                            style(" [active]").green().to_string()
                        } else {
                            style(" [disabled]").red().to_string()
                        }
                    })
                    .unwrap_or_default();

                format!(
                    "{}  {}{}",
                    style(sub).bold(),
                    style(format!("({}/{})", set_count, total)).dim(),
                    status,
                )
            })
            .collect();
        items.push(style("Save & Exit").bold().green().to_string());
        items.push(style("Back").dim().to_string());

        let sel = Select::with_theme(&ColorfulTheme::default())
            .with_prompt("API-Key Configuration")
            .default(0)
            .items(&items)
            .interact()?;

        if sel < providers.len() {
            interactive_provider(repo, config, providers[sel], did_write_llm_toml)?;
        } else if sel == providers.len() {
            save_and_exit(repo, config)?;
            return Ok(true);
        } else {
            return Ok(false);
        }
    }
}

fn interactive_provider(
    _repo: &ConfigRepository,
    config: &mut HashMap<String, String>,
    provider: &str,
    did_write_llm_toml: &mut bool,
) -> Result<()> {
    match provider {
        "Claude Code" => guided_claude_code_setup(config),
        "Codex" => guided_codex_setup(config),
        "Anthropic" => interactive_provider_keys("anthropic", config, did_write_llm_toml),
        "OpenAI" => interactive_provider_keys("openai", config, did_write_llm_toml),
        _ => interactive_provider_flat(config, provider),
    }
}

/// Flat key-list editor — the original `interactive_provider` body.
/// Used for Anthropic, OpenAI, Ollama (and as fallback from guided wizards' "Configure Settings").
fn interactive_provider_flat(
    config: &mut HashMap<String, String>,
    provider: &str,
) -> Result<()> {
    let keys = config_schema::keys_in_subcategory("API-Keys", provider);
    loop {
        let mut items: Vec<String> = keys
            .iter()
            .map(|def| format_key_menu_item(def, config))
            .collect();
        items.push(style("Back").dim().to_string());

        let sel = Select::with_theme(&ColorfulTheme::default())
            .with_prompt(format!("{} Provider", provider))
            .default(0)
            .items(&items)
            .interact()?;

        if sel < keys.len() {
            let def = keys[sel];
            let old_val = config.get(def.key).cloned().unwrap_or_default();
            configure_key(config, def)?;
            let new_val = config.get(def.key).cloned().unwrap_or_default();
            apply_auto_enable(config, def, &old_val, &new_val);
        } else {
            return Ok(());
        }
    }
}

// ── Multi-key TUI for Anthropic / OpenAI ─────────────────────────────

/// Sync AI entries from disk back into the in-memory config_map.
fn sync_ai_config_map(config: &mut HashMap<String, String>) {
    // Remove stale AI keys first
    config.retain(|k, _| !k.starts_with("ai.") || {
        config_schema::lookup(k)
            .map(|d| d.backend != ConfigBackend::LlmToml)
            .unwrap_or(true)
    });
    // Re-read from disk
    for (k, v, _) in ai_config::list_ai_entries().unwrap_or_default() {
        config.insert(k, v);
    }
}

/// Multi-key TUI for a provider (Anthropic, OpenAI).
/// Key edits write to disk immediately; enabled toggle stays in-memory.
fn interactive_provider_keys(
    provider: &str,
    config: &mut HashMap<String, String>,
    did_write_llm_toml: &mut bool,
) -> Result<()> {
    let display_name = match provider {
        "anthropic" => "Anthropic",
        "openai" => "OpenAI",
        _ => provider,
    };

    loop {
        let keys = ai_config::list_provider_keys(provider).unwrap_or_default();

        // Build menu
        let mut items: Vec<String> = Vec::new();

        if keys.is_empty() {
            items.push(style("  (no keys configured)").dim().to_string());
        } else {
            for k in &keys {
                let tier_str = if k.tier != "-" {
                    format!("  {}", style(format!("tier={}", k.tier)).dim())
                } else {
                    String::new()
                };
                items.push(format!(
                    "{}  {}  {}  {}{}",
                    style(&k.id).bold(),
                    style(&k.display_secret).dim(),
                    style(format!("src={}", k.source)).dim(),
                    style(format!("pri={}", k.priority)).dim(),
                    tier_str,
                ));
            }
        }

        // Enabled toggle
        let enabled_key = format!("ai.{}.enabled", provider);
        let is_enabled = config
            .get(&enabled_key)
            .map(|v| v == "true")
            .unwrap_or(false);
        let toggle_label = if is_enabled {
            format!(
                "{}  {}",
                style("Toggle Enabled").bold(),
                style("[active]").green()
            )
        } else {
            format!(
                "{}  {}",
                style("Toggle Enabled").bold(),
                style("[disabled]").red()
            )
        };

        items.push(format!(
            "{}  {}",
            style("Add Key").bold(),
            style("(add a new API key)").dim()
        ));
        items.push(toggle_label);
        items.push(style("Back").dim().to_string());

        let sel = Select::with_theme(&ColorfulTheme::default())
            .with_prompt(format!("{} Keys", display_name))
            .default(0)
            .items(&items)
            .interact()?;

        let key_count = keys.len();
        let has_keys = !keys.is_empty();

        if has_keys && sel < key_count {
            // Edit existing key
            let key_info = &keys[sel];
            interactive_edit_key(provider, &key_info.id, config, did_write_llm_toml)?;
        } else {
            // Determine which action based on offset after keys
            let action_offset = if has_keys { sel - key_count } else { sel - 1 };
            match action_offset {
                0 if !has_keys => {
                    // "(no keys configured)" was sel=0, Add Key is sel=1
                    // But we need to handle the "(no keys)" placeholder
                    // Recalculate: sel=0 is placeholder, sel=1 is Add Key, sel=2 is Toggle, sel=3 is Back
                    if sel == 0 {
                        // Clicked on "(no keys configured)" — do nothing
                    } else if sel == 1 {
                        interactive_add_key(provider, config, did_write_llm_toml)?;
                    } else if sel == 2 {
                        let new_val = if is_enabled { "false" } else { "true" };
                        config.insert(enabled_key.clone(), new_val.to_string());
                    } else {
                        return Ok(());
                    }
                }
                0 if has_keys => {
                    // Add Key
                    interactive_add_key(provider, config, did_write_llm_toml)?;
                }
                1 if has_keys => {
                    // Toggle Enabled
                    let new_val = if is_enabled { "false" } else { "true" };
                    config.insert(enabled_key.clone(), new_val.to_string());
                }
                2 if has_keys => {
                    // Back
                    return Ok(());
                }
                _ => {
                    return Ok(());
                }
            }
        }
    }
}

/// Sub-menu for editing an existing key: Update Secret / Delete / Back.
fn interactive_edit_key(
    provider: &str,
    key_id: &str,
    config: &mut HashMap<String, String>,
    did_write_llm_toml: &mut bool,
) -> Result<()> {
    let items = vec![
        format!(
            "{}  {}",
            style("Update Secret").bold(),
            style("(replace the API key value)").dim()
        ),
        format!(
            "{}  {}",
            style("Delete Key").bold().red(),
            style("(remove this key permanently)").dim()
        ),
        style("Back").dim().to_string(),
    ];

    let sel = Select::with_theme(&ColorfulTheme::default())
        .with_prompt(format!("Key: {}", key_id))
        .default(0)
        .items(&items)
        .interact()?;

    match sel {
        0 => {
            // Update Secret
            let secret: String = Password::with_theme(&ColorfulTheme::default())
                .with_prompt("Enter new API key")
                .allow_empty_password(true)
                .interact()?;
            if secret.trim().is_empty() {
                println!("{}", style("Cancelled.").dim());
                return Ok(());
            }

            // Soft validation: warn if key format looks wrong
            if let Err(e) = config_schema::validate_key_for_provider(provider, secret.trim()) {
                println!("{}", style(format!("Warning: {}", e)).yellow());
                if provider == "anthropic" && secret.trim().starts_with("sk-ant-oat") {
                    println!("{}", style("Tip: Use the Claude Code > Quick Setup wizard to add setup-tokens correctly.").dim());
                }
                let proceed = Confirm::with_theme(&ColorfulTheme::default())
                    .with_prompt("Update this key anyway?")
                    .default(false)
                    .interact()?;
                if !proceed {
                    println!("{}", style("Cancelled.").dim());
                    return Ok(());
                }
            }

            ai_config::upsert_provider_key(
                provider,
                key_id,
                secret.trim(),
                None,
                None,
                None,
                None,
            )?;
            *did_write_llm_toml = true;
            sync_ai_config_map(config);
            println!(
                "{} Key '{}' updated.",
                style("[ok]").green().bold(),
                key_id
            );
        }
        1 => {
            // Delete Key
            let confirm = Confirm::with_theme(&ColorfulTheme::default())
                .with_prompt(format!("Delete key '{}'?", key_id))
                .default(false)
                .interact()?;
            if confirm {
                ai_config::remove_provider_key(provider, key_id)?;
                *did_write_llm_toml = true;
                sync_ai_config_map(config);
                println!(
                    "{} Key '{}' deleted.",
                    style("[ok]").green().bold(),
                    key_id
                );
            } else {
                println!("{}", style("Cancelled.").dim());
            }
        }
        _ => {} // Back
    }
    Ok(())
}

/// Prompt to add a new key for a provider.
fn interactive_add_key(
    provider: &str,
    config: &mut HashMap<String, String>,
    did_write_llm_toml: &mut bool,
) -> Result<()> {
    // ID
    let default_id = format!("{}_key", provider);
    let key_id: String = Input::with_theme(&ColorfulTheme::default())
        .with_prompt("Key ID (unique name)")
        .default(default_id)
        .interact_text()?;

    if key_id.trim().is_empty() {
        println!("{}", style("Cancelled.").dim());
        return Ok(());
    }

    // Secret
    let secret: String = Password::with_theme(&ColorfulTheme::default())
        .with_prompt("API key secret")
        .allow_empty_password(true)
        .interact()?;

    if secret.trim().is_empty() {
        println!("{}", style("Cancelled.").dim());
        return Ok(());
    }

    // Soft validation: warn if key format looks wrong
    if let Err(e) = config_schema::validate_key_for_provider(provider, secret.trim()) {
        println!("{}", style(format!("Warning: {}", e)).yellow());
        if provider == "anthropic" && secret.trim().starts_with("sk-ant-oat") {
            println!("{}", style("Tip: Use the Claude Code > Quick Setup wizard to add setup-tokens correctly.").dim());
        }
        let proceed = Confirm::with_theme(&ColorfulTheme::default())
            .with_prompt("Add this key anyway?")
            .default(false)
            .interact()?;
        if !proceed {
            println!("{}", style("Cancelled.").dim());
            return Ok(());
        }
    }

    // Source
    let source_choices = ["api_console", "other"];
    let source_sel = Select::with_theme(&ColorfulTheme::default())
        .with_prompt("Key source")
        .default(0)
        .items(&source_choices)
        .interact()?;

    // Priority
    let priority_choices = ["primary", "fallback"];
    let priority_sel = Select::with_theme(&ColorfulTheme::default())
        .with_prompt("Priority")
        .default(0)
        .items(&priority_choices)
        .interact()?;

    ai_config::upsert_provider_key(
        provider,
        key_id.trim(),
        secret.trim(),
        Some(source_choices[source_sel]),
        Some(priority_choices[priority_sel]),
        None,
        None,
    )?;
    *did_write_llm_toml = true;

    // Auto-enable if this is the first key
    let enabled_key = format!("ai.{}.enabled", provider);
    if !config.contains_key(&enabled_key) || config.get(&enabled_key).map(|v| v == "false").unwrap_or(false) {
        config.insert(enabled_key, "true".to_string());
    }

    sync_ai_config_map(config);
    println!(
        "{} Key '{}' added.",
        style("[ok]").green().bold(),
        key_id.trim()
    );
    Ok(())
}

// ── Guided Claude Code Setup ─────────────────────────────────────────

fn guided_claude_code_setup(config: &mut HashMap<String, String>) -> Result<()> {
    loop {
        // Print header + status
        println!();
        println!("{}", style("Claude Code Setup").bold().cyan());
        println!("{}", style("──────────────────────────────────").dim());
        println!("{}", format_provider_status("claude", "claude_code", config));
        println!();

        let items = vec![
            format!(
                "{}  {}",
                style("Quick Setup").bold(),
                style("(paste a setup-token from `claude setup-token`)").dim()
            ),
            format!(
                "{}  {}",
                style("Configure Settings").bold(),
                style("(discovery, CLI fallback, CLI path)").dim()
            ),
            style("Back").dim().to_string(),
        ];

        let sel = Select::with_theme(&ColorfulTheme::default())
            .with_prompt("Claude Code")
            .default(0)
            .items(&items)
            .interact()?;

        match sel {
            0 => guided_claude_code_quick_setup(config)?,
            1 => interactive_provider_flat(config, "Claude Code")?,
            _ => return Ok(()),
        }
    }
}

fn guided_claude_code_quick_setup(config: &mut HashMap<String, String>) -> Result<()> {
    println!();
    println!(
        "{}",
        style("Run `claude setup-token` in your terminal, then paste the generated token below.")
            .yellow()
    );
    println!(
        "{}",
        style("(Leave empty to cancel)").dim()
    );
    println!();

    loop {
        let token: String = Password::with_theme(&ColorfulTheme::default())
            .with_prompt("Paste Anthropic setup-token")
            .allow_empty_password(true)
            .interact()?;

        if token.trim().is_empty() {
            println!("{}", style("Cancelled.").dim());
            return Ok(());
        }

        match config_schema::validate_anthropic_setup_token(&token) {
            Ok(()) => {
                let trimmed = token.trim().to_string();

                // Store as anthropic API key with claude_code source
                let old_key = config.get("ai.anthropic.api_key").cloned().unwrap_or_default();
                config.insert("ai.anthropic.api_key".to_string(), trimmed);
                config.insert("__source:ai.anthropic.api_key".to_string(), "claude_code".to_string());

                // Auto-enable anthropic if key was previously empty
                if old_key.is_empty() {
                    config.insert("ai.anthropic.enabled".to_string(), "true".to_string());
                }

                // Enable Claude Code discovery
                config.insert("ai.claude_code.discovery".to_string(), "true".to_string());

                println!();
                println!("{}", style("Setup complete!").bold().green());
                println!(
                    "  {} Anthropic API key stored ({})",
                    style("[ok]").green().bold(),
                    config_schema::mask_value(config.get("ai.anthropic.api_key").unwrap())
                );
                if old_key.is_empty() {
                    println!(
                        "  {} Anthropic provider auto-enabled",
                        style("[ok]").green().bold()
                    );
                }
                println!(
                    "  {} Claude Code discovery enabled",
                    style("[ok]").green().bold()
                );
                println!();
                println!(
                    "{}",
                    style("Changes are in memory. Use 'Save & Exit' from the main menu to persist.")
                        .dim()
                );
                return Ok(());
            }
            Err(e) => {
                println!("{}", style(format!("Invalid: {}", e)).red());
                println!();
            }
        }
    }
}

// ── Guided Codex Setup ───────────────────────────────────────────────

fn guided_codex_setup(config: &mut HashMap<String, String>) -> Result<()> {
    loop {
        // Print header + status
        println!();
        println!("{}", style("Codex Setup").bold().cyan());
        println!("{}", style("──────────────────────────────────").dim());
        println!("{}", format_provider_status("codex", "codex", config));
        println!();

        let items = vec![
            format!(
                "{}  {}",
                style("Quick Setup").bold(),
                style("(paste an OpenAI API key)").dim()
            ),
            format!(
                "{}  {}",
                style("Codex OAuth").bold(),
                style("(ChatGPT OAuth flow - coming soon)").dim()
            ),
            format!(
                "{}  {}",
                style("Configure Settings").bold(),
                style("(discovery, CLI fallback, CLI path)").dim()
            ),
            style("Back").dim().to_string(),
        ];

        let sel = Select::with_theme(&ColorfulTheme::default())
            .with_prompt("Codex")
            .default(0)
            .items(&items)
            .interact()?;

        match sel {
            0 => guided_codex_quick_setup(config)?,
            1 => {
                println!();
                println!(
                    "{}",
                    style("Codex OAuth (ChatGPT OAuth flow) is coming soon.").yellow()
                );
                println!();
            }
            2 => interactive_provider_flat(config, "Codex")?,
            _ => return Ok(()),
        }
    }
}

fn guided_codex_quick_setup(config: &mut HashMap<String, String>) -> Result<()> {
    println!();

    // Check OPENAI_API_KEY env var
    if let Ok(env_key) = std::env::var("OPENAI_API_KEY") {
        if !env_key.trim().is_empty() {
            match config_schema::validate_openai_api_key(&env_key) {
                Ok(()) => {
                    println!(
                        "{} Detected OPENAI_API_KEY environment variable ({})",
                        style("[ok]").green().bold(),
                        config_schema::mask_value(&env_key)
                    );

                    let use_env = Confirm::with_theme(&ColorfulTheme::default())
                        .with_prompt("Use this key?")
                        .default(true)
                        .interact()?;

                    if use_env {
                        let trimmed = env_key.trim().to_string();
                        let old_key = config.get("ai.openai.api_key").cloned().unwrap_or_default();
                        config.insert("ai.openai.api_key".to_string(), trimmed);

                        if old_key.is_empty() {
                            config.insert("ai.openai.enabled".to_string(), "true".to_string());
                        }
                        config.insert("ai.codex.discovery".to_string(), "true".to_string());

                        println!();
                        println!("{}", style("Setup complete!").bold().green());
                        println!(
                            "  {} OpenAI API key stored ({})",
                            style("[ok]").green().bold(),
                            config_schema::mask_value(config.get("ai.openai.api_key").unwrap())
                        );
                        if old_key.is_empty() {
                            println!(
                                "  {} OpenAI provider auto-enabled",
                                style("[ok]").green().bold()
                            );
                        }
                        println!(
                            "  {} Codex discovery enabled",
                            style("[ok]").green().bold()
                        );
                        println!();
                        println!(
                            "{}",
                            style("Changes are in memory. Use 'Save & Exit' from the main menu to persist.")
                                .dim()
                        );
                        return Ok(());
                    }
                    // User declined env var — fall through to manual paste
                    println!();
                }
                Err(e) => {
                    println!(
                        "{} OPENAI_API_KEY env var is set but invalid: {}",
                        style("[!!]").red().bold(),
                        e
                    );
                    println!();
                }
            }
        }
    }

    println!(
        "{}",
        style("Get your API key at: https://platform.openai.com/api-keys").yellow()
    );
    println!(
        "{}",
        style("(Leave empty to cancel)").dim()
    );
    println!();

    loop {
        let key: String = Password::with_theme(&ColorfulTheme::default())
            .with_prompt("Paste OpenAI API key")
            .allow_empty_password(true)
            .interact()?;

        if key.trim().is_empty() {
            println!("{}", style("Cancelled.").dim());
            return Ok(());
        }

        match config_schema::validate_openai_api_key(&key) {
            Ok(()) => {
                let trimmed = key.trim().to_string();

                let old_key = config.get("ai.openai.api_key").cloned().unwrap_or_default();
                config.insert("ai.openai.api_key".to_string(), trimmed);

                if old_key.is_empty() {
                    config.insert("ai.openai.enabled".to_string(), "true".to_string());
                }
                config.insert("ai.codex.discovery".to_string(), "true".to_string());

                println!();
                println!("{}", style("Setup complete!").bold().green());
                println!(
                    "  {} OpenAI API key stored ({})",
                    style("[ok]").green().bold(),
                    config_schema::mask_value(config.get("ai.openai.api_key").unwrap())
                );
                if old_key.is_empty() {
                    println!(
                        "  {} OpenAI provider auto-enabled",
                        style("[ok]").green().bold()
                    );
                }
                println!(
                    "  {} Codex discovery enabled",
                    style("[ok]").green().bold()
                );
                println!();
                println!(
                    "{}",
                    style("Changes are in memory. Use 'Save & Exit' from the main menu to persist.")
                        .dim()
                );
                return Ok(());
            }
            Err(e) => {
                println!("{}", style(format!("Invalid: {}", e)).red());
                println!();
            }
        }
    }
}

fn interactive_daemon(
    repo: &ConfigRepository,
    config: &mut HashMap<String, String>,
) -> Result<bool> {
    let subcats = config_schema::subcategories_in_category("Daemon");
    loop {
        let mut items: Vec<String> = Vec::new();

        for sub in &subcats {
            let keys = config_schema::keys_in_subcategory("Daemon", sub);
            let set_count = keys.iter().filter(|d| config.contains_key(d.key)).count();
            let total = keys.len();
            items.push(format!(
                "{}  {}",
                style(sub).bold(),
                style(format!("({}/{})", set_count, total)).dim(),
            ));
        }

        items.push(style("Save & Exit").bold().green().to_string());
        items.push(style("Back").dim().to_string());

        let sel = Select::with_theme(&ColorfulTheme::default())
            .with_prompt("Daemon Configuration")
            .default(0)
            .items(&items)
            .interact()?;

        if sel < subcats.len() {
            if interactive_subcategory(repo, config, "Daemon", subcats[sel])? {
                return Ok(true); // saved from subcategory
            }
        } else if sel == subcats.len() {
            save_and_exit(repo, config)?;
            return Ok(true);
        } else {
            return Ok(false);
        }
    }
}

// ── Agent management structs (local deserialization) ────────────────────
//
// These mirror the structs in agents.rs but are kept local to avoid coupling.

#[derive(Debug, Deserialize)]
struct TuiAgentItem {
    id: String,
    name: String,
    status: String,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    skills: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct TuiAgentConfigResponse {
    config: serde_json::Value,
    config_version: u64,
}

/// Run an async future on the current tokio runtime from synchronous code.
///
/// Uses `block_in_place` to safely move off the async worker thread before
/// blocking, which avoids the "Cannot start a runtime from within a runtime"
/// panic when called inside `#[tokio::main]`.
fn block_on<F: std::future::Future>(f: F) -> F::Output {
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(f))
}

/// Fetch agents from the daemon. Returns empty vec on connection failure.
fn fetch_agents() -> Vec<TuiAgentItem> {
    let client = match DaemonClient::connect() {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    block_on(async {
        client
            .get::<Vec<TuiAgentItem>>("/v1/agents?limit=50")
            .await
            .unwrap_or_default()
    })
}

/// Format an agent as a menu item line.
fn format_agent_menu_item(agent: &TuiAgentItem) -> String {
    let status_styled = match agent.status.to_lowercase().as_str() {
        "idle" | "active" | "running" => style(&agent.status).green(),
        "paused" | "waiting" => style(&agent.status).yellow(),
        "error" | "failed" | "archived" => style(&agent.status).red(),
        _ => style(&agent.status).dim(),
    };

    let model = agent.model.as_deref().unwrap_or("-");
    let skills = agent
        .skills
        .as_ref()
        .map(|s| s.join(", "))
        .unwrap_or_else(|| "-".to_string());

    format!(
        "{:<22} {:<10} {:<24} {}",
        style(&agent.name).bold(),
        status_styled,
        style(model).dim(),
        style(&skills).dim(),
    )
}

/// Interactive agent management sub-menu.
fn interactive_manage_agents(_repo: &ConfigRepository, _config: &HashMap<String, String>) -> Result<bool> {
    loop {
        // Fetch fresh agent list every iteration
        let agents = fetch_agents();

        if agents.is_empty() {
            let daemon_hint = if DaemonClient::connect().is_err() {
                " (daemon is not running)"
            } else {
                ""
            };

            println!(
                "\n{}{}",
                style("No agents found.").yellow(),
                style(daemon_hint).dim(),
            );
        }

        let mut items: Vec<String> = Vec::new();

        // Agent list
        for agent in &agents {
            items.push(format_agent_menu_item(agent));
        }

        // Actions at the bottom
        items.push(style("+ Create New Agent").bold().cyan().to_string());
        items.push(style("Back").dim().to_string());

        let sel = Select::with_theme(&ColorfulTheme::default())
            .with_prompt("Manage Agents")
            .default(0)
            .items(&items)
            .interact()?;

        if sel < agents.len() {
            // Selected an existing agent
            interactive_agent_detail(&agents[sel])?;
        } else if sel == agents.len() {
            // Create new agent
            interactive_create_agent()?;
        } else {
            // Back
            return Ok(false);
        }
    }
}

/// Show agent detail and allow actions.
fn interactive_agent_detail(agent: &TuiAgentItem) -> Result<()> {
    loop {
        // Print agent summary
        println!();
        println!("{} {}", style("Agent:").dim(), style(&agent.name).bold());
        println!("{} {}", style("ID:").dim(), &agent.id);
        if let Some(ref desc) = agent.description {
            if !desc.is_empty() {
                println!("{} {}", style("Description:").dim(), desc);
            }
        }

        let status_styled = match agent.status.to_lowercase().as_str() {
            "idle" | "active" | "running" => style(&agent.status).green(),
            "paused" | "waiting" => style(&agent.status).yellow(),
            "error" | "failed" | "archived" => style(&agent.status).red(),
            _ => style(&agent.status).dim(),
        };
        println!("{} {}", style("Status:").dim(), status_styled);
        println!(
            "{} {}",
            style("Model:").dim(),
            agent.model.as_deref().unwrap_or("-")
        );
        if let Some(ref skills) = agent.skills {
            println!("{} {}", style("Skills:").dim(), skills.join(", "));
        }
        println!();

        let mut items = vec![
            format!("{}", style("Edit Configuration").bold()),
            format!("{}", style("View Full Config (JSON)").bold()),
        ];

        // Show pause/resume based on status
        let is_paused = agent.status.to_lowercase() == "paused";
        if is_paused {
            items.push(style("Resume Agent").green().to_string());
        } else {
            items.push(style("Pause Agent").yellow().to_string());
        }

        items.push(style("Remove Agent").red().to_string());
        items.push(style("Back").dim().to_string());

        let sel = Select::with_theme(&ColorfulTheme::default())
            .with_prompt(format!("Agent: {}", &agent.name))
            .default(0)
            .items(&items)
            .interact()?;

        match sel {
            0 => interactive_edit_agent_config(&agent.id, &agent.name)?,
            1 => view_agent_config_json(&agent.id)?,
            2 => {
                let action = if is_paused { "resume" } else { "pause" };
                agent_action_tui(&agent.id, action)?;
                return Ok(()); // Return to refresh list with new status
            }
            3 => {
                if remove_agent_tui(&agent.id, &agent.name)? {
                    return Ok(()); // Agent removed, go back to list
                }
            }
            _ => return Ok(()), // Back
        }
    }
}

/// Show full agent config as pretty-printed JSON.
fn view_agent_config_json(agent_id: &str) -> Result<()> {
    let client = DaemonClient::connect().context("Daemon is not running")?;
    let config: TuiAgentConfigResponse = block_on(async {
        client
            .get(&format!("/v1/agents/{}/config", agent_id))
            .await
    })?;

    println!();
    println!(
        "{} {}",
        style("Config version:").dim(),
        config.config_version
    );
    println!(
        "{}",
        serde_json::to_string_pretty(&config.config).unwrap_or_default()
    );
    println!();

    // Wait for user to press enter
    let _: String = Input::with_theme(&ColorfulTheme::default())
        .with_prompt("Press Enter to continue")
        .allow_empty(true)
        .interact_text()?;

    Ok(())
}

/// Interactive editor for an agent's config (key-by-key editing).
fn interactive_edit_agent_config(agent_id: &str, agent_name: &str) -> Result<()> {
    let client = DaemonClient::connect().context("Daemon is not running")?;
    let resp: TuiAgentConfigResponse = block_on(async {
        client
            .get(&format!("/v1/agents/{}/config", agent_id))
            .await
    })?;

    let mut config = resp.config;
    let version = resp.config_version;

    loop {
        // Build editable fields from the config
        let fields = flatten_agent_config(&config);

        let mut items: Vec<String> = fields
            .iter()
            .map(|(path, value)| {
                let display_val = if value.is_null() {
                    style("(not set)").dim().to_string()
                } else if let Some(s) = value.as_str() {
                    style(s).green().to_string()
                } else {
                    style(value.to_string()).green().to_string()
                };

                let leaf = path.rsplit('.').next().unwrap_or(path).replace('_', " ");
                format!(
                    "{:<30} {}  {}",
                    style(&leaf).bold(),
                    display_val,
                    style(path).dim(),
                )
            })
            .collect();

        items.push(style("Save Changes").bold().green().to_string());
        items.push(style("Discard & Back").dim().to_string());

        let sel = Select::with_theme(&ColorfulTheme::default())
            .with_prompt(format!("Edit: {}", agent_name))
            .default(0)
            .items(&items)
            .interact()?;

        if sel < fields.len() {
            let (path, old_value) = &fields[sel];
            let current = if old_value.is_null() {
                String::new()
            } else if let Some(s) = old_value.as_str() {
                s.to_string()
            } else {
                old_value.to_string()
            };

            let new_value: String = Input::with_theme(&ColorfulTheme::default())
                .with_prompt(path)
                .default(current)
                .allow_empty(true)
                .interact_text()?;

            // Parse the value and set it
            let json_val = parse_json_value(&new_value);
            set_nested_value(&mut config, path, json_val);
        } else if sel == fields.len() {
            // Save
            let body = serde_json::json!({
                "config": config,
                "config_version": version,
            });
            let save_result: Result<serde_json::Value> = block_on(async {
                client
                    .put(&format!("/v1/agents/{}/config", agent_id), &body)
                    .await
            });
            match save_result {
                Ok(result) => {
                    let new_version = result["config_version"].as_u64().unwrap_or(version + 1);
                    println!(
                        "{} Agent config saved (version {})",
                        style("✓").green(),
                        new_version
                    );
                }
                Err(e) => {
                    let msg = e.to_string();
                    if msg.contains("409") || msg.contains("conflict") {
                        println!(
                            "{} Config was modified by another client — please try again",
                            style("✗").red()
                        );
                    } else {
                        println!("{} Failed to save: {}", style("✗").red(), msg);
                    }
                }
            }
            return Ok(());
        } else {
            // Discard
            println!("{}", style("Changes discarded.").yellow());
            return Ok(());
        }
    }
}

/// Flatten a JSON agent config into (dotted.path, value) pairs for editing.
fn flatten_agent_config(config: &serde_json::Value) -> Vec<(String, serde_json::Value)> {
    let mut result = Vec::new();
    flatten_json("", config, &mut result);
    result
}

fn flatten_json(
    prefix: &str,
    value: &serde_json::Value,
    out: &mut Vec<(String, serde_json::Value)>,
) {
    match value {
        serde_json::Value::Object(map) => {
            for (k, v) in map {
                let path = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{}.{}", prefix, k)
                };
                // Don't recurse into arrays or deeply nested objects —
                // keep objects one level deep for usability
                if v.is_object() {
                    flatten_json(&path, v, out);
                } else {
                    out.push((path, v.clone()));
                }
            }
        }
        _ => {
            if !prefix.is_empty() {
                out.push((prefix.to_string(), value.clone()));
            }
        }
    }
}

/// Set a value at a dotted path inside a JSON object.
fn set_nested_value(root: &mut serde_json::Value, path: &str, value: serde_json::Value) {
    let parts: Vec<&str> = path.split('.').collect();
    let mut current = root;
    for (i, part) in parts.iter().enumerate() {
        if i == parts.len() - 1 {
            if let Some(obj) = current.as_object_mut() {
                obj.insert(part.to_string(), value);
            }
            return;
        }
        // Navigate deeper, creating objects as needed
        if !current.get(*part).map_or(false, |v| v.is_object()) {
            if let Some(obj) = current.as_object_mut() {
                obj.insert(part.to_string(), serde_json::json!({}));
            }
        }
        current = current.get_mut(*part).unwrap();
    }
}

/// Parse a string into a JSON value (number, bool, array, or string).
fn parse_json_value(s: &str) -> serde_json::Value {
    if s.is_empty() {
        return serde_json::Value::Null;
    }
    // Try number
    if let Ok(n) = s.parse::<i64>() {
        return serde_json::Value::Number(n.into());
    }
    if let Ok(n) = s.parse::<f64>() {
        return serde_json::json!(n);
    }
    // Try bool
    if s == "true" {
        return serde_json::Value::Bool(true);
    }
    if s == "false" {
        return serde_json::Value::Bool(false);
    }
    // Try JSON array (e.g., ["a","b"])
    if s.starts_with('[') {
        if let Ok(arr) = serde_json::from_str::<serde_json::Value>(s) {
            return arr;
        }
    }
    // Comma-separated → array
    if s.contains(',') && !s.contains('"') {
        let items: Vec<serde_json::Value> = s
            .split(',')
            .map(|item| serde_json::Value::String(item.trim().to_string()))
            .filter(|v| v.as_str().map_or(true, |s| !s.is_empty()))
            .collect();
        if items.len() > 1 {
            return serde_json::Value::Array(items);
        }
    }
    serde_json::Value::String(s.to_string())
}

/// Perform a pause/resume action on an agent.
fn agent_action_tui(agent_id: &str, action: &str) -> Result<()> {
    let client = DaemonClient::connect().context("Daemon is not running")?;
    let body = serde_json::json!({ "action": action });
    let result: serde_json::Value = block_on(async {
        client
            .post(&format!("/v1/agents/{}/action", agent_id), &body)
            .await
    })?;

    let status = result["status"].as_str().unwrap_or("unknown");
    let status_styled = match status.to_lowercase().as_str() {
        "idle" | "active" | "running" => style(status).green(),
        "paused" => style(status).yellow(),
        _ => style(status).dim(),
    };
    println!("{} Agent {} → {}", style("✓").green(), agent_id, status_styled);
    Ok(())
}

/// Remove (archive) an agent with confirmation.
fn remove_agent_tui(agent_id: &str, agent_name: &str) -> Result<bool> {
    let confirm = Confirm::with_theme(&ColorfulTheme::default())
        .with_prompt(format!("Remove agent '{}'? This will archive it.", agent_name))
        .default(false)
        .interact()?;

    if !confirm {
        println!("{}", style("Cancelled.").dim());
        return Ok(false);
    }

    let client = DaemonClient::connect().context("Daemon is not running")?;
    let result: serde_json::Value = block_on(async {
        client
            .delete_req(&format!("/v1/agents/{}", agent_id))
            .await
    })?;

    let status = result["status"].as_str().unwrap_or("archived");
    println!(
        "{} Agent '{}' → {}",
        style("✓").green(),
        agent_name,
        style(status).dim()
    );
    Ok(true)
}

/// Interactive agent creation wizard (mirrors agents.rs create_interactive).
fn interactive_create_agent() -> Result<()> {
    let theme = ColorfulTheme::default();

    println!();
    println!("{}", style("Create New Agent").bold().cyan());
    println!();

    // 1. Agent name
    let name: String = Input::with_theme(&theme)
        .with_prompt("Agent name")
        .interact_text()?;

    if name.trim().is_empty() {
        println!("{}", style("Cancelled.").dim());
        return Ok(());
    }

    // 2. Description
    let description: String = Input::with_theme(&theme)
        .with_prompt("Description")
        .allow_empty(true)
        .interact_text()?;

    // 3. Model — try to fetch available models, fall back to free text
    let model = match fetch_model_list() {
        Ok(models) if !models.is_empty() => {
            let mut display = models.clone();
            display.push("(enter manually)".to_string());
            let idx = Select::with_theme(&theme)
                .with_prompt("Model")
                .items(&display)
                .default(0)
                .interact()?;
            if idx < models.len() {
                models[idx].clone()
            } else {
                Input::<String>::with_theme(&theme)
                    .with_prompt("Model name")
                    .interact_text()?
            }
        }
        _ => {
            Input::<String>::with_theme(&theme)
                .with_prompt("Model (e.g., claude-sonnet-4-5-20250929)")
                .interact_text()?
        }
    };

    // 4. Persona
    let persona: String = Input::with_theme(&theme)
        .with_prompt("Persona / system prompt")
        .default("You are a helpful assistant.".to_string())
        .interact_text()?;

    // 5. Skills
    let skills_input: String = Input::with_theme(&theme)
        .with_prompt("Skills (comma-separated, e.g., research,code_review)")
        .allow_empty(true)
        .interact_text()?;
    let skills: Vec<String> = skills_input
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    // 6. Max cost per task
    let max_cost_input: String = Input::with_theme(&theme)
        .with_prompt("Max cost per task (USD, 0 for unlimited)")
        .default("0".to_string())
        .interact_text()?;
    let max_cost: f64 = max_cost_input.parse().unwrap_or(0.0);

    // 7. Temperature
    let temp_input: String = Input::with_theme(&theme)
        .with_prompt("Temperature (0.0-1.0)")
        .default("0.5".to_string())
        .interact_text()?;
    let temperature: f64 = temp_input.parse().unwrap_or(0.5);

    // Build the config JSON
    let mut agent_config = serde_json::json!({
        "name": name,
        "description": description,
        "llm": {
            "model": model,
        },
        "skills": skills,
        "persona": persona,
        "preset": {
            "temperature": temperature,
        }
    });

    if max_cost > 0.0 {
        agent_config["constraints"] = serde_json::json!({
            "max_cost_per_task": max_cost,
        });
    }

    // Confirm
    println!();
    println!("{}", style("Agent summary:").bold());
    println!("  {} {}", style("Name:").dim(), &name);
    if !description.is_empty() {
        println!("  {} {}", style("Desc:").dim(), &description);
    }
    println!("  {} {}", style("Model:").dim(), &model);
    if !skills.is_empty() {
        println!("  {} {}", style("Skills:").dim(), skills.join(", "));
    }
    if max_cost > 0.0 {
        println!("  {} ${:.2}", style("Max cost:").dim(), max_cost);
    }
    println!();

    let confirm = Confirm::with_theme(&theme)
        .with_prompt("Create this agent?")
        .default(true)
        .interact()?;

    if !confirm {
        println!("{}", style("Cancelled.").dim());
        return Ok(());
    }

    let client = DaemonClient::connect().context("Daemon is not running")?;
    let body = serde_json::json!({ "config": agent_config });
    let result: Result<serde_json::Value> = block_on(async {
        client.post("/v1/agents", &body).await
    });

    match result {
        Ok(resp) => {
            let agent_id = resp["agent_id"].as_str().unwrap_or("unknown");
            println!(
                "{} Agent created: {}",
                style("✓").green(),
                style(agent_id).bold()
            );
        }
        Err(e) => {
            println!(
                "{} Failed to create agent: {}",
                style("✗").red(),
                e
            );
        }
    }

    Ok(())
}

/// Try to fetch the list of available models from the daemon.
fn fetch_model_list() -> Result<Vec<String>> {
    let client = DaemonClient::connect()?;
    let models: Vec<serde_json::Value> = block_on(async { client.get("/v1/models").await })?;
    let names: Vec<String> = models
        .iter()
        .filter_map(|m| m["id"].as_str().map(|s| s.to_string()))
        .collect();
    Ok(names)
}

fn interactive_agents(
    repo: &ConfigRepository,
    config: &mut HashMap<String, String>,
) -> Result<bool> {
    let subcats = config_schema::subcategories_in_category("Agents");
    loop {
        let mut items: Vec<String> = Vec::new();

        // Orchestrator sub-tab
        for sub in &subcats {
            let keys = config_schema::keys_in_subcategory("Agents", sub);
            let set_count = keys.iter().filter(|d| config.contains_key(d.key)).count();
            let total = keys.len();
            items.push(format!(
                "{}  {}",
                style(sub).bold(),
                style(format!("({}/{})", set_count, total)).dim(),
            ));
        }

        // Manage individual agents (now functional!)
        let agent_count = fetch_agents().len();
        items.push(format!(
            "{}  {}",
            style("Manage Agents").bold(),
            style(format!("({} agents)", agent_count)).dim(),
        ));
        items.push(style("Save & Exit").bold().green().to_string());
        items.push(style("Back").dim().to_string());

        let sel = Select::with_theme(&ColorfulTheme::default())
            .with_prompt("Agent Configuration")
            .default(0)
            .items(&items)
            .interact()?;

        if sel < subcats.len() {
            if interactive_subcategory(repo, config, "Agents", subcats[sel])? {
                return Ok(true); // saved from subcategory
            }
        } else if sel == subcats.len() {
            // Manage individual agents
            if interactive_manage_agents(repo, config)? {
                return Ok(true);
            }
        } else if sel == subcats.len() + 1 {
            save_and_exit(repo, config)?;
            return Ok(true);
        } else {
            return Ok(false);
        }
    }
}

fn interactive_subcategory(
    repo: &ConfigRepository,
    config: &mut HashMap<String, String>,
    category: &str,
    subcategory: &str,
) -> Result<bool> {
    let keys = config_schema::keys_in_subcategory(category, subcategory);
    loop {
        let mut items: Vec<String> = keys
            .iter()
            .map(|def| format_key_menu_item(def, config))
            .collect();
        items.push(style("Save & Exit").bold().green().to_string());
        items.push(style("Back").dim().to_string());

        let sel = Select::with_theme(&ColorfulTheme::default())
            .with_prompt(format!("{} > {}", category, subcategory))
            .default(0)
            .items(&items)
            .interact()?;

        if sel < keys.len() {
            configure_key(config, keys[sel])?;
        } else if sel == keys.len() {
            save_and_exit(repo, config)?;
            return Ok(true);
        } else {
            return Ok(false);
        }
    }
}

// ── Status helpers for guided wizards ─────────────────────────────────

/// Result of resolving a CLI binary path.
enum CliPathStatus {
    /// Binary exists at this path.
    Found(String),
    /// cli_path configured but the file doesn't exist on disk.
    BrokenOverride(String),
    /// No override and not found on PATH.
    NotFound,
}

/// Resolve a CLI binary path, checking config override first, then PATH detection.
fn resolve_cli_path(
    binary: &str,
    provider_key: &str,
    config: &HashMap<String, String>,
) -> CliPathStatus {
    let cli_path_key = format!("ai.{}.cli_path", provider_key);

    // Check for config override
    if let Some(override_path) = config.get(&cli_path_key) {
        let trimmed = override_path.trim();
        if !trimmed.is_empty() {
            if Path::new(trimmed).exists() {
                return CliPathStatus::Found(trimmed.to_string());
            } else {
                return CliPathStatus::BrokenOverride(trimmed.to_string());
            }
        }
    }

    // Fall back to PATH detection
    let detected = match binary {
        "claude" => ClaudeCodeCliProvider::detect(),
        "codex" => CodexCliProvider::detect(),
        _ => None,
    };

    match detected {
        Some(path) => CliPathStatus::Found(path.to_string_lossy().to_string()),
        None => CliPathStatus::NotFound,
    }
}

/// Check if a credential file exists for the given provider.
fn check_credential_file(provider: &str) -> bool {
    let home = match std::env::var("HOME") {
        Ok(h) => std::path::PathBuf::from(h),
        Err(_) => return false,
    };
    let cred_path = match provider {
        "claude_code" => home.join(".claude").join(".credentials.json"),
        "codex" => home.join(".codex").join("auth.json"),
        _ => return false,
    };
    cred_path.exists()
}

/// Read a boolean from config with schema-default fallback.
fn effective_bool(config: &HashMap<String, String>, key: &str) -> bool {
    config
        .get(key)
        .map(|v| v == "true")
        .unwrap_or_else(|| {
            config_schema::lookup(key)
                .and_then(|d| d.default)
                .map(|d| d == "true")
                .unwrap_or(false)
        })
}

/// Build a multi-line status block for a CLI-based provider.
fn format_provider_status(
    binary: &str,
    provider_key: &str,
    config: &HashMap<String, String>,
) -> String {
    let mut lines = Vec::new();

    // CLI binary status
    match resolve_cli_path(binary, provider_key, config) {
        CliPathStatus::Found(path) => {
            lines.push(format!(
                "  {} `{}` found at {}",
                style("[ok]").green().bold(),
                binary,
                path
            ));
        }
        CliPathStatus::BrokenOverride(path) => {
            lines.push(format!(
                "  {} `{}` configured at {} but file not found",
                style("[!!]").red().bold(),
                binary,
                path
            ));
        }
        CliPathStatus::NotFound => {
            lines.push(format!(
                "  {} `{}` not found on PATH",
                style("[--]").dim(),
                binary
            ));
        }
    }

    // Credential file status
    let has_cred_file = check_credential_file(provider_key);
    let cred_path_display = match provider_key {
        "claude_code" => "~/.claude/.credentials.json",
        "codex" => "~/.codex/auth.json",
        _ => "unknown",
    };

    if has_cred_file {
        lines.push(format!(
            "  {} Credential file found ({})",
            style("[ok]").green().bold(),
            cred_path_display
        ));
    } else if provider_key == "claude_code" && cfg!(target_os = "macos") {
        lines.push(format!(
            "  {} Credential file not found (macOS Keychain may still provide credentials)",
            style("[--]").dim()
        ));
    } else {
        lines.push(format!(
            "  {} Credential file not found ({})",
            style("[--]").dim(),
            cred_path_display
        ));
    }

    // Discovery toggle
    let discovery_key = format!("ai.{}.discovery", provider_key);
    if effective_bool(config, &discovery_key) {
        lines.push(format!(
            "  {} Token discovery: enabled",
            style("[on]").green()
        ));
    } else {
        lines.push(format!(
            "  {} Token discovery: disabled",
            style("[--]").dim()
        ));
    }

    // CLI fallback toggle
    let cli_enabled_key = format!("ai.{}.cli_enabled", provider_key);
    if effective_bool(config, &cli_enabled_key) {
        lines.push(format!(
            "  {} CLI fallback: enabled",
            style("[on]").green()
        ));
    } else {
        lines.push(format!(
            "  {} CLI fallback: disabled",
            style("[--]").dim()
        ));
    }

    // API key status (sibling provider)
    let (api_key_key, api_label) = match provider_key {
        "claude_code" => ("ai.anthropic.api_key", "Anthropic API key"),
        "codex" => ("ai.openai.api_key", "OpenAI API key"),
        _ => ("", ""),
    };

    if !api_key_key.is_empty() {
        let has_key = config
            .get(api_key_key)
            .map(|v| !v.trim().is_empty())
            .unwrap_or(false);
        if has_key {
            lines.push(format!(
                "  {} {} is set",
                style("[ok]").green().bold(),
                api_label
            ));
        } else {
            lines.push(format!(
                "  {} {} not set",
                style("[--]").dim(),
                api_label
            ));
        }
    }

    lines.join("\n")
}

/// Auto-enable/disable a provider when its credential key changes.
///
/// Only applies to providers that have an `.enabled` key (Anthropic, OpenAI, Ollama).
/// When a credential key (`.api_key` or `.base_url`) goes from empty → set, auto-enable.
/// When it goes from set → empty, auto-disable.
fn apply_auto_enable(
    config: &mut HashMap<String, String>,
    def: &config_schema::ConfigKeyDef,
    old_val: &str,
    new_val: &str,
) {
    // Only act on credential keys
    let is_credential = def.key.ends_with(".api_key") || def.key.ends_with(".base_url");
    if !is_credential {
        return;
    }

    // Derive the corresponding .enabled key from the credential key
    // e.g. "ai.anthropic.api_key" → "ai.anthropic.enabled"
    let prefix = match def.key.rsplit_once('.') {
        Some((p, _)) => p,
        None => return,
    };
    let enabled_key = format!("{}.enabled", prefix);

    // Only act if the enabled key exists in the schema
    if config_schema::lookup(&enabled_key).is_none() {
        return;
    }

    let was_empty = old_val.is_empty();
    let is_empty = new_val.is_empty();

    if was_empty && !is_empty {
        config.insert(enabled_key, "true".to_string());
    } else if !was_empty && is_empty {
        config.insert(enabled_key, "false".to_string());
    }
}

fn configure_key(
    config: &mut HashMap<String, String>,
    def: &config_schema::ConfigKeyDef,
) -> Result<()> {
    let current = config
        .get(def.key)
        .map(|s| s.as_str())
        .unwrap_or(def.default.unwrap_or(""));

    // Show key path + description as the prompt so users can reference the key name
    let prompt = format!("{} ({})", def.key, def.description);

    let new_val = match &def.kind {
        config_schema::ConfigKind::Bool => {
            let choices = ["true", "false"];
            let idx = choices.iter().position(|&x| x == current).unwrap_or(1);
            let sel = Select::with_theme(&ColorfulTheme::default())
                .with_prompt(&prompt)
                .default(idx)
                .items(&choices)
                .interact()?;
            choices[sel].to_string()
        }
        config_schema::ConfigKind::Enum(choices) => {
            let idx = choices.iter().position(|&x| x == current).unwrap_or(0);
            let sel = Select::with_theme(&ColorfulTheme::default())
                .with_prompt(&prompt)
                .default(idx)
                .items(*choices)
                .interact()?;
            choices[sel].to_string()
        }
        config_schema::ConfigKind::String if def.sensitive => {
            let sensitive_prompt = if current.is_empty() {
                format!("{} ({})", def.key, def.description)
            } else {
                format!(
                    "{} ({}, currently set — leave empty to keep)",
                    def.key, def.description
                )
            };
            let val: String = Password::with_theme(&ColorfulTheme::default())
                .with_prompt(sensitive_prompt)
                .allow_empty_password(true)
                .interact()?;
            if val.is_empty() {
                return Ok(()); // keep existing
            }
            val
        }
        config_schema::ConfigKind::String => Input::with_theme(&ColorfulTheme::default())
            .with_prompt(&prompt)
            .default(current.to_string())
            .allow_empty(true)
            .interact_text()?,
        config_schema::ConfigKind::Int { .. } | config_schema::ConfigKind::Float { .. } => loop {
            let val: String = Input::with_theme(&ColorfulTheme::default())
                .with_prompt(&prompt)
                .default(current.to_string())
                .interact_text()?;
            match config_schema::validate(def.key, &val) {
                Ok(()) => break val,
                Err(e) => println!("{}", style(format!("Invalid: {}", e)).red()),
            }
        },
    };

    config.insert(def.key.to_string(), new_val);
    Ok(())
}

fn save_and_exit(repo: &ConfigRepository, config_map: &HashMap<String, String>) -> Result<()> {
    let mut ai_entries: Vec<(&str, &str)> = Vec::new();

    for (k, v) in config_map {
        // Source-hint metadata entries go into the AI batch (processed there)
        if k.starts_with("__source:") {
            ai_entries.push((k.as_str(), v.as_str()));
            continue;
        }
        let def = config_schema::lookup(k);
        let backend = def.as_ref().map(|d| d.backend).unwrap_or(ConfigBackend::SystemConfig);
        match backend {
            ConfigBackend::LlmToml => ai_entries.push((k.as_str(), v.as_str())),
            ConfigBackend::DaemonToml => {
                // Write daemon.toml entries one at a time
                daemon_config_cli::set_daemon_value(k, v)?;
            }
            ConfigBackend::SystemConfig => {
                let kind = def.map(|d| d.kind.as_db_kind()).unwrap_or("string");
                repo.set(k, v, kind)?;
            }
        }
    }

    if !ai_entries.is_empty() {
        ai_config::set_ai_values_batch(&ai_entries)?;
    }

    println!("{}", style("Configuration saved!").bold().green());
    Ok(())
}

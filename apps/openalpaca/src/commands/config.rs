use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use console::style;
use dialoguer::{Confirm, Input, Select, theme::ColorfulTheme};
use openalpaca_storage::{ConfigRepository, Database, paths};
use std::collections::HashMap;

#[derive(Args)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub action: Option<ConfigAction>,
}

#[derive(Subcommand)]
pub enum ConfigAction {
    /// Set a configuration value
    Set { key: String, value: String },
    /// Get a configuration value
    Get { key: String },
    /// List all configuration values
    List,
    /// Reset configuration (specific key or all)
    Reset {
        /// The key to reset (optional: if omitted, resets all with confirmation)
        key: Option<String>,
    },
}

pub async fn run(args: ConfigArgs) -> Result<()> {
    // 1. Initialize Database
    let db_path = paths::database_path()?;
    let db = Database::open(&db_path).context("Failed to open database")?;
    let repo = ConfigRepository::new(&db);

    // 2. Dispatch Action
    match args.action {
        Some(ConfigAction::Set { key, value }) => {
            repo.set(&key, &value, "string")?;
            println!("✅ Config set: {} = {}", key, value);
        }
        Some(ConfigAction::Get { key }) => match repo.get(&key)? {
            Some(val) => println!("{}", val),
            None => {
                eprintln!("❌ Key not found: {}", key);
                std::process::exit(1);
            }
        },
        Some(ConfigAction::List) => {
            let items = repo.list()?;
            if items.is_empty() {
                println!("(empty)");
            } else {
                for (k, v, _) in items {
                    println!("{} = {}", k, v);
                }
            }
        }
        Some(ConfigAction::Reset { key }) => {
            if let Some(k) = key {
                // Delete specific key
                repo.delete(&k)?;
                println!("✅ Key '{}' reset (deleted).", k);
            } else {
                // Reset all with confirmation
                let confirm = Confirm::with_theme(&ColorfulTheme::default())
                    .with_prompt("⚠️  Are you sure you want to RESET ALL configuration? This cannot be undone.")
                    .default(false)
                    .interact()?;

                if confirm {
                    db.factory_reset()?;
                    println!("✅ All configuration and data wiped (Factory Reset).");
                } else {
                    println!("Cancelled.");
                }
            }
        }
        None => {
            run_interactive(&repo, &db)?;
        }
    }

    Ok(())
}

/// Run interactive configuration TUI
fn run_interactive(repo: &ConfigRepository, db: &Database) -> Result<()> {
    println!(
        "{}",
        style("⚙️  OpenAlpaca Configuration Mode").bold().cyan()
    );

    // Load initial state
    let mut config_map: HashMap<String, String> = HashMap::new();
    let current_list = repo.list()?;
    for (k, v, _) in current_list {
        config_map.insert(k, v);
    }

    loop {
        let menu_items = vec![
            format!(
                "{}  {}",
                style("🔌 Connectors").bold(),
                style("(Telegram, iMessage...)").dim()
            ),
            format!(
                "{}     {}",
                style("🛠️  System").bold(),
                style("(Debug level, Language...)").dim()
            ),
            style("💾 Save & Exit").bold().green().to_string(),
            style("❌ Discard & Exit").red().to_string(),
            style("🗑️  Reset All Config").red().dim().to_string(),
        ];

        let selection = Select::with_theme(&ColorfulTheme::default())
            .with_prompt("Main Menu")
            .default(0)
            .items(&menu_items)
            .interact()?;

        match selection {
            0 => {
                if interactive_connectors(repo, &mut config_map)? {
                    break;
                }
            } // Connectors (returns true if saved/break)
            1 => {
                if interactive_system(repo, &mut config_map)? {
                    break;
                }
            } // System (returns true if saved/break)
            2 => {
                // Save & Exit
                save_and_exit(repo, &config_map)?;
                break;
            }
            3 => {
                // Discard
                println!("{}", style("Changes discarded.").yellow());
                break;
            }
            4 => {
                // Reset All
                let input: String = Input::with_theme(&ColorfulTheme::default())
                    .with_prompt("⚠️  DANGER: Type 'yes' to WIPE ALL configuration")
                    .interact_text()?;

                if input == "yes" || input == "y" {
                    db.factory_reset()?;
                    config_map.clear();
                    println!(
                        "{}",
                        style("✅ Database wiped (Factory Reset). Exiting.").green()
                    );
                    break;
                } else {
                    println!("{}", style("❌ Cancelled.").red());
                }
            }
            _ => unreachable!(),
        }
    }

    Ok(())
}

fn save_and_exit(repo: &ConfigRepository, config_map: &HashMap<String, String>) -> Result<()> {
    for (k, v) in config_map {
        repo.set(&k, &v, "string")?;
    }
    println!("{}", style("✅ Configuration saved!").bold().green());
    Ok(())
}

// Returns true if user chose "Save & Exit" from submenu
fn interactive_connectors(
    repo: &ConfigRepository,
    config: &mut HashMap<String, String>,
) -> Result<bool> {
    loop {
        let menu_items = vec![
            "Telegram".to_string(),
            "iMessage (Coming Soon)".to_string(),
            style("💾 Save & Exit").bold().green().to_string(),
            style("🔙 Back").dim().to_string(),
        ];

        let selection = Select::with_theme(&ColorfulTheme::default())
            .with_prompt("Select Connector to Configure")
            .items(&menu_items)
            .interact()?;

        match selection {
            0 => configure_telegram(config)?,
            1 => {
                println!("ℹ️  iMessage connector is not yet implemented.");
            }
            2 => {
                save_and_exit(repo, config)?;
                return Ok(true);
            }
            3 => return Ok(false),
            _ => unreachable!(),
        }
    }
}

fn configure_telegram(config: &mut HashMap<String, String>) -> Result<()> {
    let key = "telegram.token";
    let current_val = config.get(key).map(|s| s.as_str()).unwrap_or("");

    println!("{}", style("Telegram Configuration").bold().blue());
    println!("You need a Bot Token from @BotFather to enable Telegram support.");

    let new_val: String = Input::with_theme(&ColorfulTheme::default())
        .with_prompt("Enter Telegram Bot Token")
        .default(current_val.to_string())
        .allow_empty(true)
        .interact_text()?;

    if !new_val.trim().is_empty() {
        config.insert(key.to_string(), new_val);
        println!("{}", style("✓ Telegram token updated").green());
    } else {
        println!("(No change)");
    }

    Ok(())
}

// Returns true if user chose "Save & Exit" from submenu
fn interactive_system(
    repo: &ConfigRepository,
    config: &mut HashMap<String, String>,
) -> Result<bool> {
    loop {
        let menu_items = vec![
            "Debug Level".to_string(),
            style("💾 Save & Exit").bold().green().to_string(),
            style("🔙 Back").dim().to_string(),
        ];

        let selection = Select::with_theme(&ColorfulTheme::default())
            .with_prompt("System Configuration")
            .items(&menu_items)
            .interact()?;

        match selection {
            0 => configure_debug_level(config)?,
            1 => {
                save_and_exit(repo, config)?;
                return Ok(true);
            }
            2 => return Ok(false),
            _ => unreachable!(),
        }
    }
}

fn configure_debug_level(config: &mut HashMap<String, String>) -> Result<()> {
    // Example: Debug Level
    let key = "system.debug_level";
    let current_val = config.get(key).map(|s| s.as_str()).unwrap_or("info");

    let levels = vec!["error", "warn", "info", "debug", "trace"];

    let default_idx = levels.iter().position(|&x| x == current_val).unwrap_or(2); // info

    let selection = Select::with_theme(&ColorfulTheme::default())
        .with_prompt("System Debug Level")
        .default(default_idx)
        .items(&levels)
        .interact()?;

    let new_val = levels[selection];
    config.insert(key.to_string(), new_val.to_string());
    println!(
        "{}",
        style(format!("✓ Debug level set to '{}'", new_val)).green()
    );

    Ok(())
}

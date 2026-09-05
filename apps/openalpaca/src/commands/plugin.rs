//! Plugin command — list, approve, deny, enable, disable, info, config
//!
//! **Re-pointed at `/v1/extensions` in C6** (extension design §8). The verbs
//! keep their names but their meanings are now the design's: `enable` no longer
//! records consent and `deny` performs a full unload. `openalpaca ext` is the
//! surface that also covers MCP; this one stays as the plugin-shaped shortcut.
//!
//! The legacy `/v1/plugins*` routes still exist for the GUI until C7 — nothing
//! here calls them.

use anyhow::Result;
use clap::{Args, Subcommand};
use colored::Colorize;
use std::collections::BTreeMap;

use crate::client::DaemonClient;
use crate::commands::ext::{ExtensionRow, fetch_rows, print_row};
use crate::output::{OutputFormat, print_list, print_table_header};

#[derive(Args)]
pub struct PluginArgs {
    #[command(subcommand)]
    pub command: PluginCommands,
}

#[derive(Subcommand)]
pub enum PluginCommands {
    /// List all plugins
    List {
        /// Output format
        #[arg(long, value_enum, default_value = "table")]
        format: OutputFormat,
    },
    /// Approve a plugin for loading
    Approve {
        /// Plugin name
        name: String,
    },
    /// Deny a plugin (prevent loading)
    Deny {
        /// Plugin name
        name: String,
    },
    /// Enable a disabled plugin
    Enable {
        /// Plugin name
        name: String,
    },
    /// Disable a running plugin
    Disable {
        /// Plugin name
        name: String,
    },
    /// Show detailed plugin info
    Info {
        /// Plugin name
        name: String,
    },
    /// Manage plugin configuration
    Config {
        /// Plugin name
        name: String,
        #[command(subcommand)]
        action: ConfigAction,
    },
}

#[derive(Subcommand)]
pub enum ConfigAction {
    /// Set a configuration key
    Set {
        /// Config key
        key: String,
        /// Config value
        value: String,
    },
    /// Get configuration (all keys, or a specific key)
    Get {
        /// Config key (omit to show all)
        key: Option<String>,
    },
}

// ── Command runner ───────────────────────────────────────────────

pub async fn run(args: PluginArgs) -> Result<()> {
    match args.command {
        PluginCommands::List { format } => list_plugins(format).await,
        PluginCommands::Approve { name } => crate::commands::ext::verb("plugin", &name, "approve").await,
        PluginCommands::Deny { name } => crate::commands::ext::verb("plugin", &name, "deny").await,
        PluginCommands::Enable { name } => crate::commands::ext::verb("plugin", &name, "enable").await,
        PluginCommands::Disable { name } => crate::commands::ext::verb("plugin", &name, "disable").await,
        PluginCommands::Info { name } => plugin_info(&name).await,
        PluginCommands::Config { name, action } => plugin_config(&name, action).await,
    }
}

/// `GET /v1/extensions`, filtered to plugins.
async fn plugin_rows() -> Result<Vec<ExtensionRow>> {
    let mut rows = fetch_rows(true).await?;
    rows.retain(|row| row.kind == "plugin");
    Ok(rows)
}

async fn list_plugins(format: OutputFormat) -> Result<()> {
    let plugins = plugin_rows().await?;
    print_list(&plugins, format);
    Ok(())
}

async fn plugin_info(name: &str) -> Result<()> {
    let plugins = plugin_rows().await?;
    match plugins.iter().find(|p| p.id == name) {
        Some(plugin) => print_row(plugin),
        None => println!("{} Plugin '{}' not found", "error:".red(), name),
    }
    Ok(())
}

async fn plugin_config(name: &str, action: ConfigAction) -> Result<()> {
    let client = DaemonClient::connect()?;

    match action {
        ConfigAction::Set { key, value } => {
            // Parse value: try number, then bool, then string
            let json_value = if let Ok(n) = value.parse::<i64>() {
                serde_json::Value::Number(n.into())
            } else if let Ok(n) = value.parse::<f64>() {
                serde_json::json!(n)
            } else if value == "true" {
                serde_json::Value::Bool(true)
            } else if value == "false" {
                serde_json::Value::Bool(false)
            } else {
                serde_json::Value::String(value.clone())
            };

            let body = serde_json::json!({ "key": key, "value": json_value });
            let _result: serde_json::Value = client
                .post(&format!("/v1/extensions/plugin/{}/config", name), &body)
                .await?;

            println!(
                "{} Set {}.{} = {}",
                "ok".green(),
                name,
                key,
                value,
            );
        }
        // Backed by the redacting `GET` added in C6: a value stored as a secret
        // reference reads `<redacted>` here and nowhere reads it in the clear.
        ConfigAction::Get { key } => {
            let config: BTreeMap<String, serde_json::Value> = client
                .get(&format!("/v1/extensions/plugin/{}/config", name))
                .await?;

            match key {
                Some(key) => match config.get(&key) {
                    Some(value) => println!("{} = {}", key, render_value(value)),
                    None => println!("{} '{}' is not set", "info:".dimmed(), key),
                },
                None if config.is_empty() => {
                    println!("{}", "No configuration set.".dimmed());
                }
                None => {
                    print_table_header(&[("KEY", 28), ("VALUE", 40)]);
                    for (key, value) in &config {
                        println!("{:<28} {:<40}", key, render_value(value));
                    }
                }
            }
        }
    }
    Ok(())
}

/// A config value as one line. A string prints bare; anything else prints as
/// compact JSON, so a redaction marker and a nested table are both legible.
fn render_value(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

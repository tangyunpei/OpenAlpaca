//! Plugin command — list, approve, deny, enable, disable, info, config

use anyhow::Result;
use clap::{Args, Subcommand};
use colored::Colorize;
use serde::{Deserialize, Serialize};

use crate::client::DaemonClient;
use crate::output::{OutputFormat, TableRow, print_list, status_color};

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

// ── Local deserialization structs ────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
struct PluginItem {
    name: String,
    version: String,
    status: String,
    #[serde(default)]
    tools: Vec<String>,
}

impl TableRow for PluginItem {
    fn headers() -> Vec<(&'static str, usize)> {
        vec![
            ("NAME", 20),
            ("VERSION", 12),
            ("STATUS", 20),
            ("TOOLS", 30),
        ]
    }

    fn table_row(&self) -> String {
        let tools = if self.tools.is_empty() {
            "-".to_string()
        } else {
            self.tools.join(", ")
        };

        format!(
            "{:<20} {:<12} {:<20} {:<30}",
            truncate(&self.name, 18),
            truncate(&self.version, 10),
            status_color(&self.status),
            truncate(&tools, 28),
        )
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() > max {
        format!("{}...", &s[..max.saturating_sub(3)])
    } else {
        s.to_string()
    }
}

// ── Command runner ───────────────────────────────────────────────

pub async fn run(args: PluginArgs) -> Result<()> {
    match args.command {
        PluginCommands::List { format } => list_plugins(format).await,
        PluginCommands::Approve { name } => plugin_action(&name, "approve").await,
        PluginCommands::Deny { name } => plugin_action(&name, "deny").await,
        PluginCommands::Enable { name } => plugin_action(&name, "enable").await,
        PluginCommands::Disable { name } => plugin_action(&name, "disable").await,
        PluginCommands::Info { name } => plugin_info(&name).await,
        PluginCommands::Config { name, action } => plugin_config(&name, action).await,
    }
}

async fn list_plugins(format: OutputFormat) -> Result<()> {
    let client = DaemonClient::connect()?;
    let plugins: Vec<PluginItem> = client.get("/v1/plugins").await?;
    print_list(&plugins, format);
    Ok(())
}

async fn plugin_action(name: &str, action: &str) -> Result<()> {
    let client = DaemonClient::connect()?;
    let body = serde_json::json!({});
    let result: serde_json::Value = client
        .post(&format!("/v1/plugins/{}/{}", name, action), &body)
        .await?;

    let status = result["status"].as_str().unwrap_or("unknown");
    println!("{} Plugin {} -> {}", "ok".green(), name, status_color(status));
    Ok(())
}

async fn plugin_info(name: &str) -> Result<()> {
    let client = DaemonClient::connect()?;
    let plugins: Vec<PluginItem> = client.get("/v1/plugins").await?;

    match plugins.iter().find(|p| p.name == name) {
        Some(plugin) => {
            println!("{} {}", "Name:".dimmed(), plugin.name);
            println!("{} {}", "Version:".dimmed(), plugin.version);
            println!("{} {}", "Status:".dimmed(), status_color(&plugin.status));
            if plugin.tools.is_empty() {
                println!("{} -", "Tools:".dimmed());
            } else {
                println!("{}", "Tools:".dimmed());
                for tool in &plugin.tools {
                    println!("  - {}", tool);
                }
            }
        }
        None => {
            println!(
                "{} Plugin '{}' not found",
                "error:".red(),
                name
            );
        }
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
                .post(&format!("/v1/plugins/{}/config", name), &body)
                .await?;

            println!(
                "{} Set {}.{} = {}",
                "ok".green(),
                name,
                key,
                value,
            );
        }
        ConfigAction::Get { key } => {
            // Config get is not a separate API endpoint — it would need a GET
            // endpoint. For now, show plugin info which includes status.
            // The plugin config is managed server-side; the CLI set command
            // is the primary interaction pattern.
            println!(
                "{} Plugin config viewing is managed via the plugin directory.",
                "info:".dimmed()
            );
            if let Some(ref k) = key {
                println!(
                    "  To view key '{}' for plugin '{}', check ~/.openalpaca/plugins/.config/{}.toml",
                    k, name, name
                );
            } else {
                println!(
                    "  To view config for plugin '{}', check ~/.openalpaca/plugins/.config/{}.toml",
                    name, name
                );
            }
        }
    }
    Ok(())
}

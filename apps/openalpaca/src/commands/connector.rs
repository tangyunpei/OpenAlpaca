use anyhow::Result;
use clap::{Args, Subcommand};
use colored::Colorize;
use serde::Deserialize;

use crate::client::DaemonClient;

#[derive(Args)]
pub struct ConnectorArgs {
    #[command(subcommand)]
    pub command: ConnectorCommands,
}

#[derive(Subcommand)]
pub enum ConnectorCommands {
    /// List all connectors and their status
    List,
    /// Enable and start a connector
    Enable { name: String },
    /// Disable and stop a connector
    Disable { name: String },
    /// Delete connector configuration and stop it
    Delete { name: String },
}

#[derive(Deserialize)]
struct ConnectorStatus {
    id: String,
    name: String,
    status: String,
    #[allow(dead_code)]
    configured: bool,
}

pub async fn run(args: ConnectorArgs) -> Result<()> {
    match args.command {
        ConnectorCommands::List => list_connectors().await,
        ConnectorCommands::Enable { name } => perform_action(&name, "enable").await,
        ConnectorCommands::Disable { name } => perform_action(&name, "disable").await,
        ConnectorCommands::Delete { name } => perform_action(&name, "delete").await,
    }
}

async fn list_connectors() -> Result<()> {
    let client = DaemonClient::connect()?;
    let connectors: Vec<ConnectorStatus> = client.get("/v1/connectors").await?;

    println!(
        "{:<15} {:<15} {:<15}",
        "ID".dimmed(),
        "NAME".dimmed(),
        "STATUS".dimmed()
    );
    println!("{}", "-".repeat(45).dimmed());

    for c in connectors {
        let status_colored = match c.status.as_str() {
            "active" => c.status.green(),
            "error" => c.status.yellow(),
            "disabled" => c.status.red(),
            _ => c.status.dimmed(),
        };
        println!("{:<15} {:<15} {:<15}", c.id, c.name, status_colored);
    }

    Ok(())
}

async fn perform_action(name: &str, action: &str) -> Result<()> {
    let client = DaemonClient::connect()?;
    let body = serde_json::json!({ "action": action });
    let result: serde_json::Value = client
        .post(&format!("/v1/connectors/{}/action", name), &body)
        .await?;

    let _ = result; // Action succeeded (check_response handles errors)
    println!(
        "{} Action '{}' on connector '{}' successful",
        "✓".green(),
        action,
        name
    );

    Ok(())
}

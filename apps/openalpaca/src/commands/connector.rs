use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use colored::Colorize;
use openalpaca_storage::discovery;
use serde::Deserialize;

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
    let (info, client) = get_client().await?;
    let url = format!("{}/v1/connectors", info.base_url);

    let resp = client.get(&url).send().await?;

    if !resp.status().is_success() {
        println!("{} Failed to list connectors: {}", "✗".red(), resp.status());
        return Ok(());
    }

    let connectors: Vec<ConnectorStatus> = resp.json().await?;

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
    let (info, client) = get_client().await?;
    let url = format!("{}/v1/connectors/{}/action", info.base_url, name);

    let body = serde_json::json!({ "action": action });
    let resp = client.post(&url).json(&body).send().await?;

    if resp.status().is_success() {
        println!(
            "{} Action '{}' on connector '{}' successful",
            "✓".green(),
            action,
            name
        );
    } else {
        let err: serde_json::Value = resp.json().await.unwrap_or_default();
        let msg = err["error"].as_str().unwrap_or("Unknown error");
        println!("{} Action '{}' failed: {}", "✗".red(), action, msg);
    }

    Ok(())
}

async fn get_client() -> Result<(discovery::ConnectionInfo, reqwest::Client)> {
    let disc = discovery::read_discovery()?.context("Daemon is not running (no discovery file)")?;

    discovery::ensure_not_expired(&disc)?;

    let info = discovery::ConnectionInfo::from(&disc);

    let mut headers = reqwest::header::HeaderMap::new();
    let auth_value = format!("Bearer {}", info.token);
    headers.insert(
        reqwest::header::AUTHORIZATION,
        reqwest::header::HeaderValue::from_str(&auth_value)?,
    );

    let client = reqwest::Client::builder()
        .default_headers(headers)
        .build()?;

    Ok((info, client))
}

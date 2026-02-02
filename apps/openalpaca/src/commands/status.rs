//! Status command - Show daemon status
//!
//! Reads discovery.json, calls /v1/health, and displays info.

use anyhow::Result;
use colored::Colorize;
use openalpaca_storage::discovery;

pub async fn run() -> Result<()> {
    // Read discovery file
    let disc = match discovery::read_discovery()? {
        Some(d) => d,
        None => {
            println!("{} Daemon is not running (no discovery file)", "✗".red());
            return Ok(());
        }
    };

    // Check if discovery is expired
    if let Err(e) = discovery::ensure_not_expired(&disc) {
        println!("{} Discovery expired: {}", "✗".red(), e);
        return Ok(());
    }

    // Get connection info
    let info = discovery::ConnectionInfo::from(&disc);

    // Call health endpoint
    let health_url = format!("{}/v1/health", info.base_url);
    let client = reqwest::Client::new();

    match client.get(&health_url).send().await {
        Ok(resp) if resp.status().is_success() => {
            let health: serde_json::Value = resp.json().await?;

            println!("{} Daemon is running", "✓".green().bold());
            println!();
            println!(
                "  {} {}",
                "Status:".dimmed(),
                health["status"].as_str().unwrap_or("unknown").green()
            );
            println!(
                "  {} {}",
                "Version:".dimmed(),
                health["version"].as_str().unwrap_or("unknown")
            );
            println!(
                "  {} {}",
                "PID:".dimmed(),
                health["pid"].as_u64().unwrap_or(0)
            );
            println!("  {} {}", "Instance:".dimmed(), &info.instance_id[..8]);
            println!("  {} {}", "URL:".dimmed(), info.base_url);
        }
        Ok(resp) => {
            println!("{} Daemon returned error: {}", "✗".red(), resp.status());
        }
        Err(e) => {
            println!("{} Cannot connect to daemon: {}", "✗".red(), e);
            println!("  Discovery file exists but daemon may have crashed.");
        }
    }

    Ok(())
}

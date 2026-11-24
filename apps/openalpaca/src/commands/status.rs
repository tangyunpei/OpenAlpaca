//! Status command - Show daemon status
//!
//! One look at the daemon (`crate::reuse`: discovery.json, `/v1/health`'s
//! protocol, `protocol::decide_reuse`), then `/v1/health` asked and shown.

use crate::reuse;
use anyhow::Result;
use colored::Colorize;
use openalpaca_app_support::discovery;
use openalpaca_app_support::protocol::{ClientBuild, DiscoveryRead, Verdict, decide_reuse};

/// What `openalpaca status` found.
#[derive(Debug)]
pub(crate) enum Found {
    /// No daemon's file to read: none at all, one that names nothing, or
    /// another build's whose process is gone — with why, as every other
    /// command says it.
    NotRunning(String),
    /// A live daemon this CLI cannot use (123-3 (b)): the sentence every
    /// other command refuses it with, said where its health was printed.
    Mismatch(String),
    /// The file's token has expired — the client's check, which a running
    /// daemon's file no longer meets since T21 (2) rotates it.
    Expired(String),
    /// `/v1/health` answered.
    Running {
        health: serde_json::Value,
        info: discovery::ConnectionInfo,
    },
    /// `/v1/health` answered with an error status.
    Refused(reqwest::StatusCode),
    /// `/v1/health` could not be reached.
    Unreachable(reqwest::Error),
}

/// One look over `world`, judged as `cli` by `decide_reuse` — the check
/// every other command runs (123-3 (b)), without its wait: a daemon this
/// CLI cannot use is reported with its sentence; a file this build reads
/// has its expiry checked and `/v1/health` asked, as before.
pub(crate) async fn look_with<W: reuse::World>(world: &mut W, cli: &ClientBuild) -> Result<Found> {
    let look = world.look().await;
    if let Verdict::Mismatch(m) = decide_reuse(&look, cli) {
        return Ok(Found::Mismatch(reuse::mismatch_sentence(&m)));
    }
    let DiscoveryRead::Parsed(disc) = &look.discovery else {
        return Ok(Found::NotRunning(reuse::not_running(&look)));
    };
    if let Err(e) = discovery::ensure_not_expired(disc) {
        return Ok(Found::Expired(e.to_string()));
    }
    let info = discovery::ConnectionInfo::from(disc);

    // Call health endpoint (unauthenticated)
    let health_url = format!("{}/v1/health", info.base_url);
    let client = reqwest::Client::new();
    Ok(match client.get(&health_url).send().await {
        Ok(resp) if resp.status().is_success() => Found::Running {
            health: resp.json().await?,
            info,
        },
        Ok(resp) => Found::Refused(resp.status()),
        Err(e) => Found::Unreachable(e),
    })
}

pub async fn run() -> Result<()> {
    match look_with(&mut reuse::RealWorld, &reuse::THIS_CLI).await? {
        Found::NotRunning(why) => {
            println!("{} {}", "✗".red(), why);
        }
        Found::Mismatch(sentence) => {
            println!("{} {}", "✗".red(), sentence);
        }
        Found::Expired(e) => {
            println!("{} Discovery expired: {}", "✗".red(), e);
        }
        Found::Running { health, info } => {
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
        Found::Refused(status) => {
            println!("{} Daemon returned error: {}", "✗".red(), status);
        }
        Found::Unreachable(e) => {
            println!("{} Cannot connect to daemon: {}", "✗".red(), e);
            println!("  Discovery file exists but daemon may have crashed.");
        }
    }

    Ok(())
}

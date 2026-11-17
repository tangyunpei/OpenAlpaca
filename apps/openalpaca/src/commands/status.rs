//! Status command - Show daemon status
//!
//! Reads discovery.json, calls /v1/health, and displays info.

use anyhow::Result;
use colored::Colorize;
use openalpaca_app_support::discovery;

/// What `openalpaca status` found.
#[derive(Debug)]
pub(crate) enum Found {
    /// No `discovery.json`.
    NotRunning,
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

/// Reads discovery.json, checks its expiry and asks `/v1/health`.
pub(crate) async fn look() -> Result<Found> {
    let Some(disc) = discovery::read_discovery()? else {
        return Ok(Found::NotRunning);
    };
    if let Err(e) = discovery::ensure_not_expired(&disc) {
        return Ok(Found::Expired(e.to_string()));
    }
    let info = discovery::ConnectionInfo::from(&disc);

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
    match look().await? {
        Found::NotRunning => {
            println!("{} Daemon is not running (no discovery file)", "✗".red());
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{EnvSandbox, discovery_of_a_daemon_up_for, ok_json, serve};

    /// T21's second case: `openalpaca status` against a daemon up for more
    /// than a day — its `discovery.json` rotated on the daemon's schedule —
    /// finds it running, where the file as the start wrote it was refused
    /// with "Discovery token has expired" after 24 h.
    #[tokio::test]
    async fn status_finds_a_daemon_up_for_more_than_a_day_running() {
        let tmp = tempfile::tempdir().unwrap();
        let _env = EnvSandbox::enter(tmp.path());
        let health =
            r#"{"status":"ok","version":"0.1.0","pid":51823,"instance_id":"instance-t21"}"#;
        let (addr, server) = serve(vec![ok_json(health)]);
        let d = discovery_of_a_daemon_up_for(addr.port(), 25);
        assert!(d.auth.issued_at > d.started_at, "the file was rotated");

        match look().await.expect("status looks") {
            Found::Running { health, info } => {
                assert_eq!(health["status"], "ok");
                assert_eq!(info.token, d.auth.token);
            }
            other => panic!("not running: {other:?}"),
        }
        server.join().expect("the health check was answered");
    }

    /// The client's check is kept: a file whose `expires_at` has passed —
    /// what a daemon wrote before T21, a day after its start — is still
    /// refused before any request.
    #[tokio::test]
    async fn status_still_refuses_an_expired_file() {
        let tmp = tempfile::tempdir().unwrap();
        let _env = EnvSandbox::enter(tmp.path());
        let started = chrono::Utc::now() - chrono::TimeDelta::hours(25);
        let d = discovery::make_discovery("127.0.0.1", 1, "instance-t21".into(), "0.1.0")
            .with_rotated_auth(started);
        discovery::write_discovery_atomic(&d).unwrap();

        match look().await.expect("status looks") {
            Found::Expired(reason) => assert_eq!(reason, "Discovery token has expired"),
            other => panic!("not refused as expired: {other:?}"),
        }
    }
}

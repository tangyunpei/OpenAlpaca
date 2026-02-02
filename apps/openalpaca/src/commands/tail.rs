//! Tail command - Stream events from daemon
//!
//! Connects to /v1/events WebSocket and prints events in real-time.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use colored::Colorize;
use futures_util::StreamExt;
use openalpaca_storage::discovery;
use serde::Deserialize;
use tokio_tungstenite::{connect_async, tungstenite::Message};

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ServerEvent {
    Heartbeat {
        ts: DateTime<Utc>,
        instance_id: String,
    },
    Log {
        level: String,
        message: String,
        ts: DateTime<Utc>,
        #[allow(dead_code)]
        instance_id: String,
    },
    CommandReceived {
        request_id: String,
        command: String,
        ts: DateTime<Utc>,
        #[allow(dead_code)]
        instance_id: String,
    },
    #[serde(other)]
    Unknown,
}

pub async fn run(count: usize) -> Result<()> {
    // Read discovery file
    let disc = discovery::read_discovery()?
        .context("Daemon is not running (no discovery file)")?;

    discovery::ensure_not_expired(&disc)?;

    let info = discovery::ConnectionInfo::from(&disc);

    // Build WebSocket URL
    let ws_url = format!(
        "{}/v1/events?token={}",
        info.base_url.replace("http", "ws"),
        urlencoding::encode(&info.token)
    );

    println!("{} Connecting to daemon...", "→".cyan());

    // Connect to WebSocket
    let (ws_stream, _) = connect_async(&ws_url)
        .await
        .context("Failed to connect to daemon WebSocket")?;

    println!("{} Connected! Streaming events (Ctrl+C to stop)", "✓".green());
    println!();

    let (_, mut read) = ws_stream.split();
    let mut event_count = 0;

    // Handle Ctrl+C gracefully
    let ctrl_c = tokio::signal::ctrl_c();
    tokio::pin!(ctrl_c);

    loop {
        tokio::select! {
            msg = read.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        if let Ok(event) = serde_json::from_str::<ServerEvent>(&text) {
                            print_event(&event);
                            event_count += 1;

                            if count > 0 && event_count >= count {
                                println!();
                                println!("{} Received {} events", "→".cyan(), event_count);
                                break;
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) => {
                        println!("{} Connection closed by server", "→".yellow());
                        break;
                    }
                    Some(Err(e)) => {
                        println!("{} WebSocket error: {}", "✗".red(), e);
                        break;
                    }
                    None => {
                        println!("{} Connection closed", "→".yellow());
                        break;
                    }
                    _ => {}
                }
            }
            _ = &mut ctrl_c => {
                println!();
                println!("{} Interrupted, received {} events", "→".cyan(), event_count);
                break;
            }
        }
    }

    Ok(())
}

fn print_event(event: &ServerEvent) {
    match event {
        ServerEvent::Heartbeat { ts, instance_id } => {
            let time = ts.format("%H:%M:%S").to_string();
            println!(
                "{} {} {} {}",
                time.dimmed(),
                "💓".to_string(),
                "heartbeat".cyan(),
                format!("[{}...]", &instance_id[..8]).dimmed()
            );
        }
        ServerEvent::Log { level, message, ts, .. } => {
            let time = ts.format("%H:%M:%S").to_string();
            let level_colored = match level.as_str() {
                "error" => level.red(),
                "warn" => level.yellow(),
                "info" => level.green(),
                _ => level.normal(),
            };
            println!(
                "{} {} {} {}",
                time.dimmed(),
                "📝",
                level_colored,
                message
            );
        }
        ServerEvent::CommandReceived { request_id, command, ts, .. } => {
            let time = ts.format("%H:%M:%S").to_string();
            println!(
                "{} {} {} {} {}",
                time.dimmed(),
                "⚡",
                "command".magenta(),
                command.bold(),
                format!("[{}...]", &request_id[..8]).dimmed()
            );
        }
        ServerEvent::Unknown => {
            println!("{} unknown event", "?".dimmed());
        }
    }
}

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
    CommandReceived {
        request_id: String,
        command: String,
        ts: DateTime<Utc>,
        #[allow(dead_code)]
        instance_id: String,
    },
    /// A produced artifact (plan §4.9). `task_id`/`agent_id` are absent for a
    /// loose artifact — a chat turn that ran no workflow.
    ArtifactWritten {
        #[allow(dead_code)]
        artifact_id: String,
        #[allow(dead_code)]
        task_id: Option<String>,
        #[allow(dead_code)]
        agent_id: Option<String>,
        name: String,
        kind: String,
        version: u32,
        path: String,
        ts: DateTime<Utc>,
        #[allow(dead_code)]
        instance_id: String,
    },
    #[serde(other)]
    Unknown,
}

pub async fn run(count: usize) -> Result<()> {
    // Read discovery file
    let disc = discovery::read_discovery()?.context("Daemon is not running (no discovery file)")?;

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

    println!(
        "{} Connected! Streaming events (Ctrl+C to stop)",
        "✓".green()
    );
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
                "{} 💓 {} {}",
                time.dimmed(),
                "heartbeat".cyan(),
                format!("[{}...]", &instance_id[..8]).dimmed()
            );
        }
        ServerEvent::CommandReceived {
            request_id,
            command,
            ts,
            ..
        } => {
            let time = ts.format("%H:%M:%S").to_string();
            println!(
                "{} ⚡ {} {} {}",
                time.dimmed(),
                "command".magenta(),
                command.bold(),
                format!("[{}...]", &request_id[..8]).dimmed()
            );
        }
        ServerEvent::ArtifactWritten {
            name,
            kind,
            version,
            path,
            ts,
            ..
        } => {
            let time = ts.format("%H:%M:%S").to_string();
            println!(
                "{} 📄 {} {} {} {}",
                time.dimmed(),
                "artifact".green(),
                name.bold(),
                format!("[{kind} v{version}]").cyan(),
                path.dimmed()
            );
        }
        ServerEvent::Unknown => {
            println!("{} unknown event", "?".dimmed());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// T28 — `artifact_written` has a real arm, so `openalpaca tail` names the
    /// deliverable instead of printing "unknown event".
    #[test]
    fn artifact_written_deserializes_into_its_own_variant() {
        let frame = r#"{
            "type": "artifact_written",
            "artifact_id": "a-1",
            "task_id": "t-1",
            "agent_id": "writing_agent",
            "name": "01-quarterly-report.md",
            "kind": "markdown",
            "version": 2,
            "path": "/p/.openalpaca/artifacts/run/01-quarterly-report.md",
            "ts": "2026-09-05T10:00:00Z",
            "instance_id": "inst-1"
        }"#;
        match serde_json::from_str::<ServerEvent>(frame).unwrap() {
            ServerEvent::ArtifactWritten {
                artifact_id,
                name,
                kind,
                version,
                path,
                ..
            } => {
                assert_eq!(artifact_id, "a-1");
                assert_eq!(name, "01-quarterly-report.md");
                assert_eq!(kind, "markdown");
                assert_eq!(version, 2);
                assert_eq!(path, "/p/.openalpaca/artifacts/run/01-quarterly-report.md");
            }
            other => panic!("Expected ArtifactWritten, got {other:?}"),
        }
    }

    /// The loose case: no run, no agent. Both are optional on the wire.
    #[test]
    fn artifact_written_tolerates_a_missing_task_and_agent() {
        let frame = r#"{
            "type": "artifact_written",
            "artifact_id": "a-2",
            "task_id": null,
            "agent_id": null,
            "name": "01-notes.md",
            "kind": "markdown",
            "version": 1,
            "path": "/h/.openalpaca/artifacts/loose/01-notes.md",
            "ts": "2026-09-05T10:00:00Z",
            "instance_id": "inst-1"
        }"#;
        let event = serde_json::from_str::<ServerEvent>(frame).unwrap();
        assert!(matches!(
            event,
            ServerEvent::ArtifactWritten {
                task_id: None,
                agent_id: None,
                ..
            }
        ));
        // The print arm must survive both halves being absent.
        print_event(&event);
    }
}

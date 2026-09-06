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
    /// One subagent lane of a run opening or closing (GAP-09).
    SubagentSpan {
        #[allow(dead_code)]
        task_id: String,
        #[allow(dead_code)]
        span_id: String,
        label: String,
        #[allow(dead_code)]
        template_id: String,
        #[allow(dead_code)]
        agent_instance_id: String,
        state: String,
        detail: Option<String>,
        #[allow(dead_code)]
        started_at: String,
        #[allow(dead_code)]
        ended_at: Option<String>,
        duration_ms: Option<i64>,
        #[allow(dead_code)]
        output_preview: Option<String>,
        ts: DateTime<Utc>,
        #[allow(dead_code)]
        instance_id: String,
    },
    /// A conversation was created, activated, archived or deleted (§5.7) — or
    /// one of its runs was found interrupted at boot (§5.6b), in which case
    /// `task_id` names the run.
    SessionChanged {
        #[allow(dead_code)]
        session_id: String,
        lane_key: String,
        status: String,
        task_id: Option<String>,
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
        ServerEvent::SubagentSpan {
            label,
            state,
            detail,
            duration_ms,
            ts,
            ..
        } => {
            let time = ts.format("%H:%M:%S").to_string();
            let took = match duration_ms {
                Some(ms) => format!(" {}", format_args!("{:.1}s", *ms as f64 / 1000.0)),
                None => String::new(),
            };
            let why = match detail {
                Some(d) if !d.is_empty() => format!(" — {d}"),
                _ => String::new(),
            };
            println!(
                "{} 🧵 {} {} {}{}",
                time.dimmed(),
                "lane".blue(),
                label.bold(),
                format!("[{state}{took}]").cyan(),
                why.dimmed()
            );
        }
        ServerEvent::SessionChanged {
            lane_key,
            status,
            task_id,
            ts,
            ..
        } => {
            let time = ts.format("%H:%M:%S").to_string();
            // §5.6b — the one status that is about a *run* rather than the
            // conversation's own lifecycle, so it names the run.
            let what = match task_id {
                Some(id) => format!("[{status} {id}]"),
                None => format!("[{status}]"),
            };
            let tint = if status == "interrupted" {
                what.yellow()
            } else {
                what.cyan()
            };
            println!(
                "{} 💬 {} {} {}",
                time.dimmed(),
                "session".magenta(),
                lane_key.bold(),
                tint
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

    /// GAP-09 — a lane transition has its own arm, so `openalpaca tail` shows
    /// the swimlane changing instead of "unknown event".
    #[test]
    fn subagent_span_deserializes_into_its_own_variant() {
        let frame = r#"{
            "type": "subagent_span",
            "task_id": "t-1",
            "span_id": "node-1",
            "label": "review\u00b71",
            "template_id": "review_agent",
            "agent_instance_id": "review_agent::a1b2",
            "state": "cancelled",
            "detail": "cancelled before starting",
            "started_at": "2026-09-05T10:00:00.000Z",
            "ended_at": "2026-09-05T10:00:04.500Z",
            "duration_ms": 4500,
            "output_preview": null,
            "ts": "2026-09-05T10:00:04Z",
            "instance_id": "inst-1"
        }"#;
        let event = serde_json::from_str::<ServerEvent>(frame).unwrap();
        match &event {
            ServerEvent::SubagentSpan {
                span_id,
                label,
                state,
                detail,
                duration_ms,
                ..
            } => {
                assert_eq!(span_id, "node-1");
                assert_eq!(label, "review\u{b7}1");
                assert_eq!(state, "cancelled");
                assert_eq!(detail.as_deref(), Some("cancelled before starting"));
                assert_eq!(*duration_ms, Some(4_500));
            }
            other => panic!("Expected SubagentSpan, got {other:?}"),
        }
        print_event(&event);
    }

    /// The open frame: no end, no duration, no detail. The print arm must not
    /// invent any of them.
    #[test]
    fn subagent_span_tolerates_an_open_lane() {
        let frame = r#"{
            "type": "subagent_span",
            "task_id": "t-1",
            "span_id": "node-1",
            "label": "review\u00b71",
            "template_id": "review_agent",
            "agent_instance_id": "review_agent::a1b2",
            "state": "running",
            "detail": null,
            "started_at": "2026-09-05T10:00:00.000Z",
            "ended_at": null,
            "duration_ms": null,
            "output_preview": null,
            "ts": "2026-09-05T10:00:00Z",
            "instance_id": "inst-1"
        }"#;
        let event = serde_json::from_str::<ServerEvent>(frame).unwrap();
        assert!(matches!(
            event,
            ServerEvent::SubagentSpan {
                ended_at: None,
                duration_ms: None,
                detail: None,
                ..
            }
        ));
        print_event(&event);
    }

    /// §5.6b — the boot sweep's frame names the run it interrupted, and
    /// `openalpaca tail` prints it instead of "unknown event".
    #[test]
    fn an_interrupted_session_frame_names_its_run() {
        let frame = r#"{
            "type": "session_changed",
            "session_id": "s-1",
            "lane_key": "user1:gui",
            "status": "interrupted",
            "task_id": "t-9",
            "ts": "2026-09-05T10:00:00Z",
            "instance_id": "inst-1"
        }"#;
        let event = serde_json::from_str::<ServerEvent>(frame).unwrap();
        match event {
            ServerEvent::SessionChanged {
                ref session_id,
                ref status,
                ref task_id,
                ..
            } => {
                assert_eq!(session_id, "s-1");
                assert_eq!(status, "interrupted");
                assert_eq!(task_id.as_deref(), Some("t-9"));
            }
            other => panic!("Expected SessionChanged, got {other:?}"),
        }
        print_event(&event);
    }

    /// A lifecycle transition carries no run; the arm must survive that.
    #[test]
    fn a_lifecycle_session_frame_carries_no_run() {
        let frame = r#"{
            "type": "session_changed",
            "session_id": "s-1",
            "lane_key": "user1:gui",
            "status": "archived",
            "task_id": null,
            "ts": "2026-09-05T10:00:00Z",
            "instance_id": "inst-1"
        }"#;
        let event = serde_json::from_str::<ServerEvent>(frame).unwrap();
        assert!(matches!(
            event,
            ServerEvent::SessionChanged { task_id: None, .. }
        ));
        print_event(&event);
    }
}

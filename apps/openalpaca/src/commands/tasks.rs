//! Tasks command — list, status, log, create, cancel, pause, resume
//!
//! `resume` is one word over two verbs, exactly as the daemon route is: it
//! un-pauses a paused run, and — when the daemon has §5.6c's experimental
//! replay resume enabled — continues an `interrupted` one from the history
//! its session log kept.

use anyhow::Result;
use clap::{Args, Subcommand};
use colored::Colorize;
use serde::{Deserialize, Serialize};

use crate::client::DaemonClient;
use crate::output::{OutputFormat, TableRow, print_list, status_color};

#[derive(Args)]
pub struct TasksArgs {
    #[command(subcommand)]
    pub command: TasksCommands,
}

/// The help text below may hold commas, apostrophes and `//` lines again:
/// `scripts/gen_api_docs.py` blanks comment text before it splits a
/// `Subcommand` enum body, so prose no longer invents or shadows a variant.
/// Covered by `scripts/tests/test_gen_api_docs.py`.
#[derive(Subcommand)]
pub enum TasksCommands {
    /// List tasks
    List {
        /// Filter by status (queued, running, completed, failed, cancelled, paused, interrupted, active)
        #[arg(long)]
        status: Option<String>,
        /// Maximum number of results
        #[arg(long, default_value = "50")]
        limit: usize,
        /// Output format
        #[arg(long, value_enum, default_value = "table")]
        format: OutputFormat,
    },
    /// Show detailed task status
    Status {
        /// Task ID
        task_id: String,
        /// Output format
        #[arg(long, value_enum, default_value = "table")]
        format: OutputFormat,
    },
    /// Show task event log
    Log {
        /// Task ID
        task_id: String,
        /// Maximum number of log entries
        #[arg(long, default_value = "50")]
        limit: usize,
    },
    /// Create a new task
    Create {
        /// Task description/title
        description: Option<String>,
        /// Task priority (0 = normal)
        #[arg(long, default_value = "0")]
        priority: i32,
    },
    /// Cancel a task
    Cancel {
        /// Task ID
        task_id: String,
    },
    /// Pause a running task
    Pause {
        /// Task ID
        task_id: String,
    },
    /// Resume a paused task. On an interrupted run this is instead the
    /// experimental replay resume — off by default; enable it with
    /// resume_enabled under [orchestrator.routing] in daemon.toml
    Resume {
        /// Task ID
        task_id: String,
    },
    /// List or answer the tool-approval prompts a run is waiting on
    Confirmations {
        #[command(subcommand)]
        command: crate::commands::confirmations::ConfirmationCommands,
    },
}

// ── Local deserialization structs ────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
struct TaskItem {
    id: String,
    title: String,
    status: String,
    #[serde(default)]
    priority: i32,
    created_at: Option<String>,
    /// How many agents the run spawned, from one grouped `subagent_span`
    /// query per page (R38). `#[serde(default)]` because a daemon older than
    /// the field omits it — 0 then reads the same as a run that spawned none,
    /// which is the honest thing a count can say without a second call.
    #[serde(default)]
    subagent_count: i64,
}

#[derive(Debug, Serialize, Deserialize)]
struct TaskDetail {
    task: TaskInner,
}

#[derive(Debug, Serialize, Deserialize)]
struct TaskInner {
    id: String,
    title: String,
    #[serde(default)]
    description: Option<String>,
    status: String,
    priority: i32,
    #[serde(default)]
    progress_current: Option<i64>,
    #[serde(default)]
    progress_total: Option<i64>,
    #[serde(default)]
    result_summary: Option<String>,
    created_by: String,
    source_lane: String,
    created_at: String,
    updated_at: String,
    #[serde(default)]
    completed_at: Option<String>,
}

/// One lane of `GET /v1/tasks/{id}/timeline` — one spawned subagent.
///
/// This replaces the task routes' deleted `assignments` array (P8): a span is
/// opened when the subagent is spawned, so a lane still working has a row here
/// where `agent_task_history` had none at all.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TimelineLane {
    label: String,
    template_id: String,
    /// `running` / `done` / `failed` / `blocked` / `cancelled`.
    state: String,
    /// Why, when the state needs one: `interrupted`, `waiting on <tool>`, …
    #[serde(default)]
    detail: Option<String>,
    started_at: String,
    #[serde(default)]
    ended_at: Option<String>,
}

/// `GET /v1/tasks/{id}/timeline` (GAP-09).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TaskTimeline {
    lanes: Vec<TimelineLane>,
}

/// `openalpaca tasks status --format json` — the task plus its lanes, so the
/// JSON path shows the runs the table path does.
///
/// `lanes_error` is present, and `lanes` empty, when the timeline call failed:
/// a script has to be able to tell "this run spawned nothing" from "the lanes
/// could not be read". It is absent whenever the timeline loaded.
#[derive(Debug, Serialize)]
struct TaskStatusJson<'a> {
    task: &'a TaskInner,
    lanes: &'a [TimelineLane],
    #[serde(skip_serializing_if = "Option::is_none")]
    lanes_error: Option<&'a str>,
}

/// One row of `GET /v1/events/history`.
#[derive(Debug, Serialize, Deserialize)]
struct EventLogEntry {
    id: Option<i64>,
    timestamp: String,
    event_type: String,
    #[serde(default)]
    agent_id: Option<String>,
    /// The run the row belongs to — the indexed column, filled since
    /// migration 037 (GAP-10). NULL on rows written before it, and on events
    /// that belong to no run.
    #[serde(default)]
    task_id: Option<String>,
    /// The event's payload. Named `detail` on the wire; older rows keep the
    /// run id in here as well as in the column.
    #[serde(default)]
    detail: Option<serde_json::Value>,
}

/// `GET /v1/events/history` — always this envelope, never a bare array (P20).
#[derive(Debug, Serialize, Deserialize)]
struct EventHistoryPage {
    events: Vec<EventLogEntry>,
    /// Pass back as `?before=` to walk to the older page; `null` at the end.
    #[serde(default)]
    next_before: Option<i64>,
}

impl TableRow for TaskItem {
    /// AGENTS is a *count* now, not the names the deleted `assigned_agents`
    /// array carried (P8): the list route serves `subagent_count` from one
    /// grouped `subagent_span` query per page, so the column costs no extra
    /// request. `openalpaca tasks status <id>` names the agents.
    fn headers() -> Vec<(&'static str, usize)> {
        vec![
            ("ID", 10),
            ("TITLE", 30),
            ("STATUS", 12),
            ("AGENTS", 8),
            ("CREATED", 20),
        ]
    }

    fn table_row(&self) -> String {
        let short_id = if self.id.len() > 8 {
            &self.id[..8]
        } else {
            &self.id
        };
        // `-`, as the empty array read before P8: a bare `0` in a column of
        // numbers looks like a lookup that failed.
        let agents = if self.subagent_count > 0 {
            self.subagent_count.to_string()
        } else {
            "-".to_string()
        };
        let created = self
            .created_at
            .as_deref()
            .unwrap_or("-")
            .chars()
            .take(19)
            .collect::<String>();

        format!(
            "{:<10} {:<30} {:<12} {:<8} {:<20}",
            short_id,
            truncate(&self.title, 28),
            status_color(&self.status),
            agents,
            created,
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

pub async fn run(args: TasksArgs) -> Result<()> {
    match args.command {
        TasksCommands::List {
            status,
            limit,
            format,
        } => list_tasks(status, limit, format).await,
        TasksCommands::Status { task_id, format } => task_status(&task_id, format).await,
        TasksCommands::Log { task_id, limit } => task_log(&task_id, limit).await,
        TasksCommands::Create {
            description,
            priority,
        } => create_task(description, priority).await,
        TasksCommands::Cancel { task_id } => task_action(&task_id, "cancel").await,
        TasksCommands::Pause { task_id } => task_action(&task_id, "pause").await,
        TasksCommands::Resume { task_id } => task_action(&task_id, "resume").await,
        TasksCommands::Confirmations { command } => {
            crate::commands::confirmations::run(command).await
        }
    }
}

async fn list_tasks(status: Option<String>, limit: usize, format: OutputFormat) -> Result<()> {
    let client = DaemonClient::connect()?;
    let mut path = format!("/v1/tasks?limit={}", limit);
    if let Some(ref s) = status {
        path.push_str(&format!("&status={}", s));
    }
    let tasks: Vec<TaskItem> = client.get(&path).await?;
    print_list(&tasks, format);
    Ok(())
}

/// The table form of `tasks status`. `lanes` is `Err(message)` when the
/// timeline call failed.
///
/// Rendering is separated from fetching so both outcomes are testable, and so
/// the task — which the *first* call already returned in full — is printed
/// before anything can go wrong with the second.
fn render_task_status(t: &TaskInner, lanes: Result<&[TimelineLane], &str>) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();

    let _ = writeln!(out, "{} {}", "Task:".dimmed(), t.id);
    let _ = writeln!(out, "{} {}", "Title:".dimmed(), t.title);
    if let Some(ref desc) = t.description {
        let _ = writeln!(out, "{} {}", "Description:".dimmed(), desc);
    }
    let _ = writeln!(out, "{} {}", "Status:".dimmed(), status_color(&t.status));
    let _ = writeln!(out, "{} {}", "Priority:".dimmed(), t.priority);
    if let (Some(cur), Some(total)) = (t.progress_current, t.progress_total) {
        let pct = if total > 0 { (cur * 100) / total } else { 0 };
        let filled = (pct as usize) / 5; // 20-char bar
        let empty = 20_usize.saturating_sub(filled);
        let _ = writeln!(
            out,
            "{} [{}{}] {}/{}",
            "Progress:".dimmed(),
            "█".repeat(filled).green(),
            "░".repeat(empty),
            cur,
            total
        );
    }
    if let Some(ref summary) = t.result_summary {
        let _ = writeln!(out, "{} {}", "Result:".dimmed(), summary);
    }
    let _ = writeln!(out, "{} {}", "Created by:".dimmed(), t.created_by);
    let _ = writeln!(out, "{} {}", "Source:".dimmed(), t.source_lane);
    let _ = writeln!(out, "{} {}", "Created:".dimmed(), t.created_at);
    let _ = writeln!(out, "{} {}", "Updated:".dimmed(), t.updated_at);
    if let Some(ref completed) = t.completed_at {
        let _ = writeln!(out, "{} {}", "Completed:".dimmed(), completed);
    }

    match lanes {
        // A run that spawned nothing gets no `Lanes:` block at all — the
        // absence is the answer, and there is nothing to announce.
        Ok(lanes) if lanes.is_empty() => {}
        Ok(lanes) => {
            let _ = writeln!(out);
            let _ = writeln!(out, "{}", "Lanes:".dimmed());
            // Served oldest-first by the daemon.
            for lane in lanes {
                let started = lane.started_at.chars().take(19).collect::<String>();
                let window = match lane.ended_at.as_deref() {
                    Some(ended) => format!(
                        " {} → {}",
                        started,
                        ended.chars().take(19).collect::<String>()
                    ),
                    None => format!(" {} → …", started),
                };
                let detail_label = lane
                    .detail
                    .as_deref()
                    .map(|d| format!(" — {}", d))
                    .unwrap_or_default();
                let _ = writeln!(
                    out,
                    "  {} {} ({}){}{}",
                    status_color(&lane.state),
                    lane.label,
                    lane.template_id,
                    window.dimmed(),
                    detail_label.dimmed()
                );
            }
        }
        // Withheld, and said so: an empty lane list here would report the run
        // as having spawned nothing.
        Err(message) => {
            let _ = writeln!(out);
            let _ = writeln!(
                out,
                "{}",
                format!("Lanes: could not be loaded — {message}").yellow()
            );
        }
    }

    out
}

async fn task_status(task_id: &str, format: OutputFormat) -> Result<()> {
    let client = DaemonClient::connect()?;
    let detail: TaskDetail = client.get(&format!("/v1/tasks/{}", task_id)).await?;
    // The agent runs come from the timeline now — the task routes stopped
    // carrying them with P8. It is a second call, so it fails independently:
    // a transient error, or a CLI newer than the daemon it is talking to, must
    // not cost the user the task the first call already returned.
    let timeline: std::result::Result<TaskTimeline, String> = client
        .get(&format!("/v1/tasks/{}/timeline", task_id))
        .await
        .map_err(|e| e.to_string());

    match format {
        OutputFormat::Json => {
            let (lanes, lanes_error) = match timeline {
                Ok(ref timeline) => (timeline.lanes.as_slice(), None),
                Err(ref message) => (&[][..], Some(message.as_str())),
            };
            println!(
                "{}",
                serde_json::to_string_pretty(&TaskStatusJson {
                    task: &detail.task,
                    lanes,
                    lanes_error,
                })
                .unwrap_or_default()
            );
        }
        OutputFormat::Table => {
            let lanes = match timeline {
                Ok(ref timeline) => Ok(timeline.lanes.as_slice()),
                Err(ref message) => Err(message.as_str()),
            };
            print!("{}", render_task_status(&detail.task, lanes));
        }
    }
    Ok(())
}

async fn task_log(task_id: &str, limit: usize) -> Result<()> {
    let client = DaemonClient::connect()?;
    // The server filters by run now (GAP-10) — the whole page is this task's,
    // so `--limit` is a limit on *its* rows rather than on a global window
    // that a busy daemon would fill with other runs' events.
    let page: EventHistoryPage = client
        .get(&format!(
            "/v1/events/history?task_id={}&limit={}",
            urlencoding::encode(task_id),
            limit
        ))
        .await?;

    if page.events.is_empty() {
        println!("{}", "No log entries found for this task.".dimmed());
        return Ok(());
    }

    // Newest first from the server; read the run forwards.
    for event in page.events.iter().rev() {
        let ts = &event.timestamp[..19.min(event.timestamp.len())];
        let agent = event.agent_id.as_deref().unwrap_or("-");
        println!(
            "{} {} [{}] {}",
            ts.dimmed(),
            status_color(&event.event_type),
            agent,
            format_event_payload(event.detail.as_ref().unwrap_or(&serde_json::Value::Null)),
        );
    }

    if page.next_before.is_some() {
        println!(
            "{}",
            format!("… more entries; raise --limit above {limit} to see them").dimmed()
        );
    }

    Ok(())
}

fn format_event_payload(payload: &serde_json::Value) -> String {
    if let Some(status) = payload.get("status").and_then(|v| v.as_str()) {
        return format!("status -> {}", status);
    }
    if let Some(title) = payload.get("title").and_then(|v| v.as_str()) {
        return format!("created: {}", title);
    }
    if let Some(summary) = payload.get("result_summary").and_then(|v| v.as_str()) {
        return format!("result: {}", summary);
    }
    payload.to_string()
}

/// The owner half of a lane key — `{owner}:{source}` (`is_lane_owned_by`,
/// `routes/chat_types.rs`).
///
/// A lane with no `:` at all is passed through whole rather than blanked: the
/// daemon is the one that decides whether it owns a lane, and a client that
/// guesses a different identity than the one it is about to send would only
/// make the refusal harder to read.
fn owner_of(lane_key: &str) -> &str {
    lane_key.split_once(':').map_or(lane_key, |(owner, _)| owner)
}

/// The body `POST /v1/tasks` takes.
///
/// **The lane is the caller's own** (V6). `"cli_user:cli"` was a lane nobody
/// owns, and since the create route began refusing a lane the local owner does
/// not hold (R79, `404 LANE_NOT_FOUND`) every `openalpaca tasks create` failed
/// on it. The lane is therefore read the same way `openalpaca chat` reads it —
/// `GET /v1/me`'s `default_lane_key`, which is the lane a CLI turn actually
/// lands on — and `created_by` is that lane's owner rather than a second
/// invented identity. (The route overwrites `created_by` with the local user
/// anyway; sending the same thing keeps the body honest about who parked it.)
///
/// `unattended` is the S10 declaration, **stored on the row**: this route only
/// parks the work, and whoever starts it later — `openalpaca tasks resume`,
/// the GUI, the follow-up runner — inherits what the parker said unless it
/// declares for itself. Sent only when it is true, so an attended create's
/// body is what it was but for the lane.
fn create_body(
    title: &str,
    priority: i32,
    unattended: bool,
    lane_key: &str,
) -> serde_json::Value {
    let mut body = serde_json::json!({
        "title": title,
        "created_by": owner_of(lane_key),
        "source_lane": lane_key,
        "priority": priority,
    });
    if unattended {
        body["unattended"] = serde_json::json!(true);
    }
    body
}

async fn create_task(description: Option<String>, priority: i32) -> Result<()> {
    let title = match description {
        Some(d) => d,
        None => {
            let input: String =
                dialoguer::Input::with_theme(&dialoguer::theme::ColorfulTheme::default())
                    .with_prompt("Task title")
                    .interact_text()?;
            input
        }
    };

    let client = DaemonClient::connect()?;
    // V6: the lane this CLI talks on, as the daemon names it — the same read
    // `openalpaca chat` does, not a second resolver.
    let lane_key = crate::commands::sessions::default_lane_key(&client).await?;
    // S10: a run parked from a script has nobody to answer its tool prompts,
    // and the row remembers that rather than discovering it 300 s at a time.
    let body = create_body(
        &title,
        priority,
        crate::unattended::declared_unattended(),
        &lane_key,
    );

    let result: serde_json::Value = client.post("/v1/tasks", &body).await?;
    let task_id = result["task_id"].as_str().unwrap_or("unknown");
    println!("{} Task created: {}", "✓".green(), task_id);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `colored` decides by tty at first use; pin it off so a table row's
    /// cells can be read literally.
    fn plain() {
        colored::control::set_override(false);
    }

    fn item(subagent_count: i64) -> TaskItem {
        TaskItem {
            id: "task-1234-5678".to_string(),
            title: "Audit".to_string(),
            status: "running".to_string(),
            priority: 0,
            created_at: Some("2026-09-04T09:15:00.000Z".to_string()),
            subagent_count,
        }
    }

    /// S10: a run parked by a script carries the declaration on the row, so
    /// whoever starts it later does not have to rediscover that nobody can
    /// answer its prompts. A create from a terminal says nothing at all — the
    /// route's own default is `false`.
    #[test]
    fn a_parked_run_carries_the_declaration_only_when_it_is_true() {
        let attended = create_body("Audit the connectors", 0, false, OWNER_LANE);
        assert_eq!(
            attended,
            serde_json::json!({
                "title": "Audit the connectors",
                "created_by": "8f1c5b0e-0000-4000-8000-000000000001",
                "source_lane": OWNER_LANE,
                "priority": 0,
            }),
            "an attended create's body is the lane and nothing more"
        );

        let scripted = create_body("Audit the connectors", 2, true, OWNER_LANE);
        assert_eq!(scripted["unattended"], true);
        assert_eq!(scripted["priority"], 2);
        assert_eq!(scripted["title"], "Audit the connectors");
    }

    /// The lane `GET /v1/me` reports, in the shape the daemon reports it.
    const OWNER_LANE: &str = "8f1c5b0e-0000-4000-8000-000000000001:gui";

    /// V6: `POST /v1/tasks` refuses a `source_lane` the local owner does not
    /// hold (R79 — `404 LANE_NOT_FOUND`, never a `403` that would confirm the
    /// lane exists), and `"cli_user:cli"` is a lane nobody owns, so every
    /// `openalpaca tasks create` died on it. The body carries the lane this
    /// CLI actually talks on.
    #[test]
    fn a_created_task_is_parked_on_a_lane_this_owner_holds() {
        let body = create_body("Audit the connectors", 0, false, OWNER_LANE);
        let lane = body["source_lane"].as_str().expect("a lane is sent");

        assert_eq!(lane, OWNER_LANE);
        assert!(
            is_lane_owned_by(lane, "8f1c5b0e-0000-4000-8000-000000000001"),
            "the daemon's own ownership test passes on what the CLI sends"
        );
        assert_ne!(lane, "cli_user:cli", "the invented lane is gone");
        assert_eq!(
            body["created_by"], "8f1c5b0e-0000-4000-8000-000000000001",
            "one identity, the lane's own owner"
        );
    }

    /// `is_lane_owned_by` as `apps/openalpacad/src/routes/chat_types.rs:186`
    /// defines it — copied rather than imported because the daemon is not a
    /// dependency of the CLI, and pinned here so a drift in either is a
    /// failing test rather than a 404 nobody can explain.
    fn is_lane_owned_by(lane_key: &str, user_id: &str) -> bool {
        lane_key.starts_with(&format!("{}:", user_id))
    }

    /// A lane whose owner cannot be read is sent whole: guessing a different
    /// identity than the one in `source_lane` would only make the daemon's
    /// refusal harder to read.
    #[test]
    fn a_lane_without_a_source_still_names_its_owner() {
        assert_eq!(owner_of("owner-1:gui"), "owner-1");
        assert_eq!(owner_of("owner-1:cli:extra"), "owner-1");
        assert_eq!(owner_of("owner-1"), "owner-1");
        assert_eq!(owner_of(""), "");
    }

    /// R38 — the AGENTS column is back, as the count the list route now
    /// carries (`subagent_count`, one grouped `subagent_span` query per page)
    /// rather than the per-row `agent_task_history` array P8 deleted.
    #[test]
    fn task_list_rows_count_the_agents_a_run_spawned() {
        plain();
        assert!(
            TaskItem::headers().iter().any(|(name, _)| *name == "AGENTS"),
            "the list table names the column"
        );

        let row = item(3).table_row();
        let cells: Vec<&str> = row.split_whitespace().collect();
        assert_eq!(
            cells,
            vec!["task-123", "Audit", "running", "3", "2026-09-04T09:15:00"]
        );
    }

    /// A run that spawned nothing reads `-`, the same as the deleted array's
    /// empty case — not a bare `0`, which looks like a failed lookup.
    #[test]
    fn task_list_rows_show_a_dash_for_a_run_that_spawned_nothing() {
        plain();
        let row = item(0).table_row();
        let cells: Vec<&str> = row.split_whitespace().collect();
        assert_eq!(cells[3], "-");
    }

    /// A daemon older than the field omits it; the row must still render.
    #[test]
    fn a_task_row_from_a_daemon_without_the_count_reads_as_none() {
        plain();
        let item: TaskItem = serde_json::from_value(serde_json::json!({
            "id": "task-1234-5678",
            "title": "Audit",
            "status": "running",
            "created_at": "2026-09-04T09:15:00.000Z",
        }))
        .expect("a row without subagent_count still deserializes");
        assert_eq!(item.subagent_count, 0);
        let row = item.table_row();
        let cells: Vec<&str> = row.split_whitespace().collect();
        assert_eq!(cells[3], "-");
    }

    fn task() -> TaskInner {
        TaskInner {
            id: "task-1".to_string(),
            title: "Audit the connectors".to_string(),
            description: Some("Read every adapter".to_string()),
            status: "running".to_string(),
            priority: 0,
            progress_current: Some(5),
            progress_total: Some(8),
            result_summary: None,
            // What the daemon stores and reads back: the local owner and one
            // of that owner's lanes (R79, V6) — never the `cli_user` the CLI
            // used to invent.
            created_by: "8f1c5b0e-0000-4000-8000-000000000001".to_string(),
            source_lane: OWNER_LANE.to_string(),
            created_at: "2026-09-04T09:15:00.000Z".to_string(),
            updated_at: "2026-09-04T09:18:00.000Z".to_string(),
            completed_at: None,
        }
    }

    fn lane() -> TimelineLane {
        TimelineLane {
            label: "review·1".to_string(),
            template_id: "review_agent".to_string(),
            state: "running".to_string(),
            detail: None,
            started_at: "2026-09-04T09:15:00.000Z".to_string(),
            ended_at: None,
        }
    }

    /// The happy path: the task, then its lanes under a `Lanes:` heading.
    #[test]
    fn task_status_renders_the_lanes_under_the_task() {
        let lanes = vec![lane()];
        let out = render_task_status(&task(), Ok(&lanes));

        assert!(out.contains("Audit the connectors"), "{out}");
        assert!(out.contains("Lanes:"), "{out}");
        assert!(out.contains("review·1"), "{out}");
        assert!(out.contains("review_agent"), "{out}");
        assert!(!out.contains("could not be loaded"), "{out}");
    }

    /// The timeline is a *second* daemon call, and it can fail on its own — a
    /// transient DB error, or a CLI newer than the daemon it is talking to.
    /// The task the first call already returned must still print, and the
    /// missing lanes must be announced rather than silently read as "none".
    #[test]
    fn task_status_prints_the_task_when_the_timeline_call_fails() {
        let out = render_task_status(&task(), Err("HTTP 404: Not Found"));

        assert!(out.contains("task-1"), "{out}");
        assert!(out.contains("Audit the connectors"), "{out}");
        assert!(out.contains("Status:"), "{out}");
        assert!(
            out.contains("Lanes: could not be loaded — HTTP 404: Not Found"),
            "{out}"
        );
    }

    /// A run that spawned nothing is not the same as a run whose lanes could
    /// not be read: it says neither.
    #[test]
    fn task_status_says_nothing_about_lanes_when_a_run_spawned_none() {
        let out = render_task_status(&task(), Ok(&[]));

        assert!(out.contains("Audit the connectors"), "{out}");
        assert!(!out.contains("Lanes:"), "{out}");
    }

    /// `--format json` must stay valid JSON when the timeline fails, and must
    /// distinguish "no lanes" from "lanes unknown" — an empty array alone
    /// would tell a script the run spawned nothing.
    #[test]
    fn task_status_json_names_the_timeline_failure_instead_of_an_empty_array() {
        let detail = task();
        let value = serde_json::to_value(TaskStatusJson {
            task: &detail,
            lanes: &[],
            lanes_error: Some("HTTP 500: Internal Server Error"),
        })
        .unwrap();

        assert_eq!(value["task"]["id"], "task-1");
        assert_eq!(value["lanes"], serde_json::json!([]));
        assert_eq!(value["lanes_error"], "HTTP 500: Internal Server Error");

        // A run whose timeline loaded carries no error key at all.
        let lanes = vec![lane()];
        let ok = serde_json::to_value(TaskStatusJson {
            task: &detail,
            lanes: &lanes,
            lanes_error: None,
        })
        .unwrap();
        assert!(ok.get("lanes_error").is_none());
        assert_eq!(ok["lanes"].as_array().unwrap().len(), 1);
    }

    /// `GET /v1/tasks?status=` parses through `TaskStatus::from_str`, which
    /// accepts `interrupted` (§5.6b) — the daemon writes it at boot for a run
    /// it was driving when it went away. `--help` is what a user reads first,
    /// and it must not disagree with `docs/CLI_Manual.md`'s documented set.
    #[test]
    fn tasks_list_status_help_names_interrupted() {
        use clap::Args as _;

        let cmd = TasksArgs::augment_args(clap::Command::new("tasks"));
        let list = cmd
            .find_subcommand("list")
            .expect("tasks has a list subcommand");
        let status_arg = list
            .get_arguments()
            .find(|arg| arg.get_id() == "status")
            .expect("list has a --status flag");
        let help = status_arg
            .get_help()
            .expect("--status carries help text")
            .to_string();

        assert!(
            help.contains("interrupted"),
            "the --status help must list interrupted, matching docs/CLI_Manual.md: {help}"
        );
    }

    /// An un-pause prints what it always printed: one line, no replay clause.
    #[test]
    fn a_plain_action_prints_one_line() {
        plain();
        let body = serde_json::json!({"task_id": "task-1234-5678", "status": "running"});
        assert_eq!(
            action_line("task-1234-5678", &body),
            "\u{2713} Task task-123 -> running"
        );
    }

    /// §5.6c — a replay resume says how much of the run came back. Without
    /// this the line cannot be told from a run that started over.
    #[test]
    fn a_replay_resume_names_the_rounds_it_recovered() {
        plain();
        let body = serde_json::json!({
            "task_id": "task-1234-5678",
            "status": "running",
            "session_id": "s1",
            "rounds_replayed": 3,
            "from_seq": 1,
            "to_seq": 12,
        });
        let line = action_line("task-1234-5678", &body);
        assert!(line.starts_with("\u{2713} Task task-123 -> running"), "{line}");
        assert!(line.contains("replayed 3 rounds from session s1"), "{line}");

        let one =
            serde_json::json!({"status": "running", "session_id": "s1", "rounds_replayed": 1});
        assert!(
            action_line("task-1", &one).contains("replayed 1 round from session s1"),
            "one round is singular"
        );
    }

    /// `openalpaca tasks resume --help` has to say that the word grew a second
    /// meaning and that it is off by default — otherwise an operator reading
    /// the help cannot tell why an interrupted run refuses.
    #[test]
    fn tasks_resume_help_names_the_experimental_replay() {
        use clap::Args as _;
        let command = TasksArgs::augment_args(clap::Command::new("tasks"));
        let resume = command
            .get_subcommands()
            .find(|c| c.get_name() == "resume")
            .expect("the resume subcommand");
        let about = resume.get_about().expect("resume has help").to_string();
        for word in ["paused", "interrupted", "resume_enabled", "off by default"] {
            assert!(
                about.contains(word),
                "resume --help must mention {word}: {about}"
            );
        }
    }
}

async fn task_action(task_id: &str, action: &str) -> Result<()> {
    let client = DaemonClient::connect()?;
    let body = serde_json::json!({ "action": action });
    let result: serde_json::Value = client
        .post(&format!("/v1/tasks/{}/action", task_id), &body)
        .await?;

    println!("{}", action_line(task_id, &result));
    Ok(())
}

/// The one line an action prints, split out so its shape is testable without
/// a daemon.
///
/// §5.6c gives `resume` a second meaning, and the extra clause is why it is
/// worth a function: "Task abc12345 -> running" reads the same whether a
/// transcript came back or the run started from nothing, and those are very
/// different things to have just done. The clause appears only when the
/// response carries the replay's own numbers, so an un-pause — and a daemon
/// too old to send them — prints exactly what it printed before.
fn action_line(task_id: &str, result: &serde_json::Value) -> String {
    let status = result["status"].as_str().unwrap_or("unknown");
    let mut line = format!(
        "{} Task {} -> {}",
        "✓".green(),
        &task_id[..8.min(task_id.len())],
        status_color(status)
    );
    if let Some(rounds) = result["rounds_replayed"].as_u64() {
        line.push_str(&format!(
            "\n  replayed {rounds} round{} from session {}",
            if rounds == 1 { "" } else { "s" },
            result["session_id"].as_str().unwrap_or("?")
        ));
    }
    line
}

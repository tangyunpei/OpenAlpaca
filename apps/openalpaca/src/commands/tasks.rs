//! Tasks command — list, status, log, create, cancel, pause, resume

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

#[derive(Subcommand)]
pub enum TasksCommands {
    /// List tasks
    List {
        /// Filter by status (queued, running, completed, failed, cancelled, paused, active)
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
    /// Resume a paused task
    Resume {
        /// Task ID
        task_id: String,
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
#[derive(Debug, Serialize)]
struct TaskStatusJson<'a> {
    task: &'a TaskInner,
    lanes: &'a [TimelineLane],
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
    /// No AGENTS column: the list route no longer carries `assigned_agents`
    /// (P8), and a per-row timeline call would be one request per row.
    /// `openalpaca tasks status <id>` shows a run's lanes.
    fn headers() -> Vec<(&'static str, usize)> {
        vec![("ID", 10), ("TITLE", 30), ("STATUS", 12), ("CREATED", 20)]
    }

    fn table_row(&self) -> String {
        let short_id = if self.id.len() > 8 {
            &self.id[..8]
        } else {
            &self.id
        };
        let created = self
            .created_at
            .as_deref()
            .unwrap_or("-")
            .chars()
            .take(19)
            .collect::<String>();

        format!(
            "{:<10} {:<30} {:<12} {:<20}",
            short_id,
            truncate(&self.title, 28),
            status_color(&self.status),
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

async fn task_status(task_id: &str, format: OutputFormat) -> Result<()> {
    let client = DaemonClient::connect()?;
    let detail: TaskDetail = client.get(&format!("/v1/tasks/{}", task_id)).await?;
    // The agent runs come from the timeline now — the task routes stopped
    // carrying them with P8.
    let timeline: TaskTimeline = client
        .get(&format!("/v1/tasks/{}/timeline", task_id))
        .await?;

    match format {
        OutputFormat::Json => {
            println!(
                "{}",
                serde_json::to_string_pretty(&TaskStatusJson {
                    task: &detail.task,
                    lanes: &timeline.lanes,
                })
                .unwrap_or_default()
            );
        }
        OutputFormat::Table => {
            let t = &detail.task;
            println!("{} {}", "Task:".dimmed(), t.id);
            println!("{} {}", "Title:".dimmed(), t.title);
            if let Some(ref desc) = t.description {
                println!("{} {}", "Description:".dimmed(), desc);
            }
            println!("{} {}", "Status:".dimmed(), status_color(&t.status));
            println!("{} {}", "Priority:".dimmed(), t.priority);
            if let (Some(cur), Some(total)) = (t.progress_current, t.progress_total) {
                let pct = if total > 0 { (cur * 100) / total } else { 0 };
                let filled = (pct as usize) / 5; // 20-char bar
                let empty = 20_usize.saturating_sub(filled);
                println!(
                    "{} [{}{}] {}/{}",
                    "Progress:".dimmed(),
                    "█".repeat(filled).green(),
                    "░".repeat(empty),
                    cur,
                    total
                );
            }
            if let Some(ref summary) = t.result_summary {
                println!("{} {}", "Result:".dimmed(), summary);
            }
            println!("{} {}", "Created by:".dimmed(), t.created_by);
            println!("{} {}", "Source:".dimmed(), t.source_lane);
            println!("{} {}", "Created:".dimmed(), t.created_at);
            println!("{} {}", "Updated:".dimmed(), t.updated_at);
            if let Some(ref completed) = t.completed_at {
                println!("{} {}", "Completed:".dimmed(), completed);
            }

            if !timeline.lanes.is_empty() {
                println!();
                println!("{}", "Lanes:".dimmed());
                // Served oldest-first by the daemon.
                for lane in &timeline.lanes {
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
                    println!(
                        "  {} {} ({}){}{}",
                        status_color(&lane.state),
                        lane.label,
                        lane.template_id,
                        window.dimmed(),
                        detail_label.dimmed()
                    );
                }
            }
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
    let body = serde_json::json!({
        "title": title,
        "created_by": "cli_user",
        "source_lane": "cli_user:cli",
        "priority": priority,
    });

    let result: serde_json::Value = client.post("/v1/tasks", &body).await?;
    let task_id = result["task_id"].as_str().unwrap_or("unknown");
    println!("{} Task created: {}", "✓".green(), task_id);
    Ok(())
}

async fn task_action(task_id: &str, action: &str) -> Result<()> {
    let client = DaemonClient::connect()?;
    let body = serde_json::json!({ "action": action });
    let result: serde_json::Value = client
        .post(&format!("/v1/tasks/{}/action", task_id), &body)
        .await?;

    let status = result["status"].as_str().unwrap_or("unknown");
    println!(
        "{} Task {} -> {}",
        "✓".green(),
        &task_id[..8.min(task_id.len())],
        status_color(status)
    );
    Ok(())
}

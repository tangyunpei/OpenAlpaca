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
    #[serde(default)]
    assigned_agents: Vec<AssignedAgent>,
    created_at: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct AssignedAgent {
    agent_id: String,
    role: String,
    status: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct TaskDetail {
    task: TaskInner,
    #[serde(default)]
    assignments: Option<Vec<AssignmentDetail>>,
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

#[derive(Debug, Serialize, Deserialize)]
struct AssignmentDetail {
    agent_id: String,
    role: String,
    status: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct EventLogEntry {
    id: Option<i64>,
    timestamp: String,
    event_type: String,
    #[serde(default)]
    agent_id: Option<String>,
    #[serde(default)]
    task_id: Option<String>,
    payload: serde_json::Value,
}

impl TableRow for TaskItem {
    fn headers() -> Vec<(&'static str, usize)> {
        vec![
            ("ID", 10),
            ("TITLE", 30),
            ("STATUS", 12),
            ("AGENTS", 15),
            ("CREATED", 20),
        ]
    }

    fn table_row(&self) -> String {
        let short_id = if self.id.len() > 8 {
            &self.id[..8]
        } else {
            &self.id
        };
        let agents = if self.assigned_agents.is_empty() {
            "-".to_string()
        } else {
            self.assigned_agents
                .iter()
                .map(|a| a.agent_id.clone())
                .collect::<Vec<_>>()
                .join(", ")
        };
        let created = self
            .created_at
            .as_deref()
            .unwrap_or("-")
            .chars()
            .take(19)
            .collect::<String>();

        format!(
            "{:<10} {:<30} {:<12} {:<15} {:<20}",
            short_id,
            truncate(&self.title, 28),
            status_color(&self.status),
            truncate(&agents, 13),
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

    match format {
        OutputFormat::Json => {
            println!(
                "{}",
                serde_json::to_string_pretty(&detail).unwrap_or_default()
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
                println!("{} {}/{}", "Progress:".dimmed(), cur, total);
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

            if let Some(ref assignments) = detail.assignments {
                if !assignments.is_empty() {
                    println!();
                    println!("{}", "Assigned Agents:".dimmed());
                    for a in assignments {
                        println!(
                            "  {} {} ({})",
                            status_color(&a.status),
                            a.agent_id,
                            a.role
                        );
                    }
                }
            }
        }
    }
    Ok(())
}

async fn task_log(task_id: &str, limit: usize) -> Result<()> {
    let client = DaemonClient::connect()?;
    let events: Vec<EventLogEntry> = client
        .get(&format!("/v1/events/history?limit={}", limit))
        .await?;

    // Filter to task-related events
    let task_events: Vec<&EventLogEntry> = events
        .iter()
        .filter(|e| {
            // Match by task_id in event or in payload
            if let Some(ref tid) = e.task_id {
                if tid == task_id {
                    return true;
                }
            }
            if let Some(tid) = e.payload.get("task_id").and_then(|v| v.as_str()) {
                if tid == task_id {
                    return true;
                }
            }
            false
        })
        .collect();

    if task_events.is_empty() {
        println!("{}", "No log entries found for this task.".dimmed());
        return Ok(());
    }

    for event in task_events {
        let ts = &event.timestamp[..19.min(event.timestamp.len())];
        let agent = event.agent_id.as_deref().unwrap_or("-");
        println!(
            "{} {} [{}] {}",
            ts.dimmed(),
            status_color(&event.event_type),
            agent,
            format_event_payload(&event.payload),
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
            let input: String = dialoguer::Input::with_theme(&dialoguer::theme::ColorfulTheme::default())
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

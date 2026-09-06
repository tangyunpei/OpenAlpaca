//! Sessions command — the conversations a lane holds (plan §5.7).
//!
//! Migration 039 made a lane hold many conversations with exactly one `active`
//! at a time. `openalpaca sessions` is the read-back: the ids `chat --session`
//! takes, which of them is live, the project each is bound to and when it last
//! moved.
//!
//! Two flags, and they pull in opposite directions on purpose:
//!   * `--workspace <path>` **narrows** to one project. The path is resolved
//!     the same way the daemon resolves a turn's `x-workspace-path` — up to the
//!     nearest `.openalpaca`/`.git` — so `--workspace .` works from anywhere
//!     inside a repository, and a path under no project marker is an error
//!     rather than a filter that silently matches nothing.
//!   * `--all` **widens** past the lane the CLI and the GUI share (both talk on
//!     `{local_user}:gui`) to every lane the daemon holds, connectors included.
//!
//! `GET /v1/sessions` has no lane filter — its filters are workspace, source,
//! status and a text query — so the lane restriction is applied here, against
//! the `default_lane_key` the daemon reports from `GET /v1/me`. That is the
//! exact lane a CLI turn lands on, not an assumption about how lanes are named.

use anyhow::{Context, Result, bail};
use clap::Args;
use colored::Colorize;
use serde::{Deserialize, Serialize};

use crate::client::DaemonClient;
use crate::output::{OutputFormat, TableRow, print_list, status_color};

#[derive(Args)]
pub struct SessionsArgs {
    /// Only conversations bound to this project (`.` for the working directory)
    #[arg(long, value_name = "PATH")]
    pub workspace: Option<String>,

    /// Every lane, not just the one the CLI and GUI share
    #[arg(long)]
    pub all: bool,

    /// Maximum number of results
    #[arg(long, default_value = "50")]
    pub limit: usize,

    /// Output format
    #[arg(long, value_enum, default_value = "table")]
    pub format: OutputFormat,
}

/// One row of `GET /v1/sessions` — the daemon's `SessionView`.
///
/// The counts and the summary columns it also carries are left off: a list is
/// for finding a conversation, and `interrupted_task_count` is structurally 0
/// until Phase 7b's boot sweep writes that status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionItem {
    pub id: String,
    pub lane_key: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub workspace_id: Option<String>,
    pub status: String,
    #[serde(default)]
    pub message_count: i64,
    pub updated_at: String,
}

#[derive(Debug, Deserialize)]
struct SessionsResponse {
    sessions: Vec<SessionItem>,
    #[allow(dead_code)]
    total: i64,
}

#[derive(Debug, Deserialize)]
struct MeResponse {
    default_lane_key: String,
}

/// `""` is what an unrenamed conversation carries; never print a blank cell.
fn display_title(title: &str) -> &str {
    if title.trim().is_empty() {
        "(untitled)"
    } else {
        title
    }
}

/// `/Users/dev/openalpaca` → `openalpaca`; a conversation with none says so.
fn display_workspace(workspace_id: Option<&str>) -> String {
    match workspace_id {
        None => "-".to_string(),
        Some(path) => path
            .rsplit(['/', '\\'])
            .find(|part| !part.is_empty())
            .unwrap_or(path)
            .to_string(),
    }
}

/// `2026-09-06 10:00:00` / RFC3339 → `2026-09-06 10:00`, trimmed to the table.
fn display_stamp(updated_at: &str) -> String {
    updated_at
        .replace('T', " ")
        .chars()
        .take(16)
        .collect::<String>()
}

impl TableRow for SessionItem {
    fn headers() -> Vec<(&'static str, usize)> {
        vec![
            ("ID", 36),
            ("STATUS", 9),
            ("TITLE", 28),
            ("WORKSPACE", 20),
            ("UPDATED", 16),
        ]
    }

    fn table_row(&self) -> String {
        let title = display_title(&self.title);
        format!(
            "{:<36} {:<9} {:<28} {:<20} {:<16}",
            self.id,
            status_color(&self.status),
            title.chars().take(28).collect::<String>(),
            display_workspace(self.workspace_id.as_deref())
                .chars()
                .take(20)
                .collect::<String>(),
            display_stamp(&self.updated_at),
        )
    }
}

/// The query `GET /v1/sessions` is asked, for these flags.
///
/// `--all` drops the `source` filter; without it the source is the one the
/// caller's own lane uses, so a connector's conversations stay out of a list
/// whose ids are meant for `chat --session`.
pub(crate) fn sessions_query(
    source: Option<&str>,
    workspace_id: Option<&str>,
    limit: usize,
) -> String {
    let mut parts = vec![format!("limit={limit}")];
    if let Some(source) = source {
        parts.push(format!("source={}", urlencoding::encode(source)));
    }
    if let Some(workspace_id) = workspace_id {
        parts.push(format!(
            "workspace_id={}",
            urlencoding::encode(workspace_id)
        ));
    }
    format!("/v1/sessions?{}", parts.join("&"))
}

/// The `source` half of a lane key: `alice:gui` → `gui`.
pub(crate) fn lane_source(lane_key: &str) -> Option<&str> {
    lane_key.rsplit_once(':').map(|(_, source)| source)
}

/// Resolve a `--workspace` argument to the project root the daemon stores.
///
/// The same resolver a turn's `x-workspace-path` goes through (R22), so the
/// value compared here is the one `session.workspace_id` holds. A path under
/// no project marker — or one that resolves to the home store — has no project
/// root, and saying so beats filtering on a path nothing can match.
pub(crate) fn resolve_workspace_filter(path: &str) -> Result<String> {
    let absolute = std::path::Path::new(path)
        .canonicalize()
        .with_context(|| format!("No such directory: {path}"))?;
    openalpaca_core::memory::scope_context::MemoryScopeContext::for_request(Some(
        &absolute.to_string_lossy(),
    ))
    .request_workspace_root
    .ok_or_else(|| {
        anyhow::anyhow!(
            "{} is not inside a project — conversations are bound to a root marked by .openalpaca or .git",
            absolute.display()
        )
    })
}

/// The conversations on the caller's own lane, newest first.
///
/// Shared with `chat --resume`, whose picker is this list.
pub(crate) async fn lane_sessions(
    client: &DaemonClient,
    workspace_id: Option<&str>,
    limit: usize,
) -> Result<Vec<SessionItem>> {
    let me: MeResponse = client.get("/v1/me").await?;
    let source = lane_source(&me.default_lane_key);
    let page: SessionsResponse = client
        .get(&sessions_query(source, workspace_id, limit))
        .await?;
    Ok(page
        .sessions
        .into_iter()
        .filter(|row| row.lane_key == me.default_lane_key)
        .collect())
}

pub async fn run(args: SessionsArgs) -> Result<()> {
    let client = DaemonClient::connect()?;

    let workspace_id = match args.workspace.as_deref() {
        Some(path) => Some(resolve_workspace_filter(path)?),
        None => None,
    };

    let sessions = if args.all {
        let page: SessionsResponse = client
            .get(&sessions_query(None, workspace_id.as_deref(), args.limit))
            .await?;
        page.sessions
    } else {
        lane_sessions(&client, workspace_id.as_deref(), args.limit).await?
    };

    if sessions.is_empty() && matches!(args.format, OutputFormat::Table) {
        // Why it is empty matters: a filter that matched nothing reads very
        // differently from a daemon that holds no conversations at all.
        match workspace_id.as_deref() {
            Some(root) => println!("{}", format!("No conversations in {root}.").dimmed()),
            None => println!("{}", "No conversations yet.".dimmed()),
        }
        return Ok(());
    }

    print_list(&sessions, args.format);
    Ok(())
}

/// Guard for a verb that needs exactly one conversation to act on.
pub(crate) fn require_one(sessions: &[SessionItem]) -> Result<&SessionItem> {
    match sessions.first() {
        Some(session) => Ok(session),
        None => bail!("No stored conversations on this lane yet — send a message first"),
    }
}

/// The one-line description the `--resume` picker shows per row.
pub(crate) fn picker_label(session: &SessionItem) -> String {
    format!(
        "{} · {} · {} messages · {} · {}",
        display_title(&session.title),
        session.status,
        session.message_count,
        display_workspace(session.workspace_id.as_deref()),
        display_stamp(&session.updated_at),
    )
}

#[cfg(test)]
mod tests;

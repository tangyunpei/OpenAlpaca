//! Sessions command — the conversations a lane holds (plan §5.7).
//!
//! Migration 039 made a lane hold many conversations with exactly one `active`
//! at a time. `openalpaca sessions` is the read-back: the ids `chat --session`
//! takes, which of them is live, the project each is bound to and when it last
//! moved. `openalpaca sessions delete <id>` is the one verb over that list —
//! `DELETE /v1/sessions/{id}`, rows and transcript — and the counterpart to the
//! GUI sidebar's delete, for conversations with no project that a
//! `store purge` deliberately never touches.
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
use clap::{Args, Subcommand};
use colored::Colorize;
use serde::{Deserialize, Serialize};

use crate::client::DaemonClient;
use crate::output::{OutputFormat, TableRow, print_list, status_color};

#[derive(Args)]
pub struct SessionsArgs {
    /// What to do with a conversation; listing is the default
    #[command(subcommand)]
    pub command: Option<SessionsCommands>,

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

#[derive(Subcommand)]
pub enum SessionsCommands {
    /// Delete a conversation: its rows and its transcript on disk
    Delete {
        /// The conversation's id (`openalpaca sessions` lists them)
        id: String,
    },
}

/// One row of `GET /v1/sessions` — the daemon's `SessionView`.
///
/// The counts and the summary columns it also carries are left off: a list is
/// for finding a conversation. (`interrupted_task_count` is real as of §5.6b's
/// boot sweep — `openalpaca tasks --status interrupted` is where to read it.)
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

/// The lane a CLI turn lands on, as the daemon reports it.
///
/// Read rather than assumed: `{user}:gui` is how the lane is named today, and
/// the callers that compare against it (`chat --session`) must compare against
/// the daemon's own answer, not a shape hard-coded here.
pub(crate) async fn default_lane_key(client: &DaemonClient) -> Result<String> {
    let me: MeResponse = client.get("/v1/me").await?;
    Ok(me.default_lane_key)
}

/// The rows of a page that belong here: this lane, and — when a project was
/// named — this project.
///
/// The lane half is client-side because `GET /v1/sessions` has no lane filter
/// (its filters are workspace, source, status and a query). The project half
/// *is* sent as `workspace_id=`; re-checking it here is what makes the promise
/// local rather than a trust in the query string: a row bound to another
/// project, or to none at all, is never offered as something to continue
/// (plan §5.7).
pub(crate) fn rows_here(
    rows: Vec<SessionItem>,
    lane_key: &str,
    workspace_id: Option<&str>,
) -> Vec<SessionItem> {
    rows.into_iter()
        .filter(|row| row.lane_key == lane_key)
        .filter(|row| match workspace_id {
            None => true,
            Some(root) => row.workspace_id.as_deref() == Some(root),
        })
        .collect()
}

/// The conversations on the caller's own lane, newest first.
///
/// Shared with `chat --resume`, whose picker is this list.
pub(crate) async fn lane_sessions(
    client: &DaemonClient,
    workspace_id: Option<&str>,
    limit: usize,
) -> Result<Vec<SessionItem>> {
    let lane_key = default_lane_key(client).await?;
    lane_sessions_on(client, &lane_key, workspace_id, limit).await
}

/// [`lane_sessions`] for a caller that already knows its lane — `chat` reads it
/// once and uses it both to list and to refuse a conversation on another one.
pub(crate) async fn lane_sessions_on(
    client: &DaemonClient,
    lane_key: &str,
    workspace_id: Option<&str>,
    limit: usize,
) -> Result<Vec<SessionItem>> {
    let page: SessionsResponse = client
        .get(&sessions_query(lane_source(lane_key), workspace_id, limit))
        .await?;
    Ok(rows_here(page.sessions, lane_key, workspace_id))
}

pub async fn run(args: SessionsArgs) -> Result<()> {
    match args.command {
        Some(SessionsCommands::Delete { ref id }) => delete(id).await,
        None => list(args).await,
    }
}

/// The path of one conversation — an id is a path segment, never a second path.
fn session_path(id: &str) -> String {
    format!("/v1/sessions/{}", urlencoding::encode(id))
}

/// Delete one conversation, rows and transcript.
///
/// `DELETE /v1/sessions/{id}` is the whole of it: the daemon owns the database
/// and the session-log writers, so the CLI never removes either itself. The
/// route takes the messages, the tool-call index rows and the queued
/// follow-ups with the session, stands the log writer down and removes
/// `~/.openalpaca/sessions/<id>/`.
///
/// No `-y`. The one thing a confirmation could protect — deleting the
/// transcript a run is still writing into — the route already refuses
/// (`409 SESSION_HAS_ACTIVE_WORKFLOWS`), and this verb names a single id that
/// `openalpaca sessions` had to list first. The conversation is read before it
/// goes so the line printed afterwards says *what* went, not just which id:
/// the read is unscoped (R40) while the delete is owner-scoped, so a
/// conversation this owner cannot delete answers `404` and nothing is printed.
async fn delete(id: &str) -> Result<()> {
    let client = DaemonClient::connect()?;
    let path = session_path(id);
    let session: SessionItem = client
        .get(&path)
        .await
        .with_context(|| format!("Could not read conversation {id}"))?;
    client
        .delete_no_content(&path)
        .await
        .with_context(|| format!("Could not delete conversation {id}"))?;
    println!("{}", deleted_line(&session));
    Ok(())
}

/// What a delete says it took.
fn deleted_line(session: &SessionItem) -> String {
    format!(
        "{} {} ({}) · {} messages · {}",
        "Deleted".red().bold(),
        display_title(&session.title),
        session.id,
        session.message_count,
        display_workspace(session.workspace_id.as_deref()),
    )
}

async fn list(args: SessionsArgs) -> Result<()> {
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
///
/// The two empties are different answers and are told apart: a lane with no
/// conversations at all wants a first message, while a *project* with none —
/// which is the scope `chat --resume` asks in (plan §5.7) — wants to know that
/// other projects' conversations exist and are deliberately not on offer.
pub(crate) fn require_one<'a>(
    sessions: &'a [SessionItem],
    workspace_id: Option<&str>,
) -> Result<&'a SessionItem> {
    match (sessions.first(), workspace_id) {
        (Some(session), _) => Ok(session),
        (None, Some(root)) => bail!(
            "No stored conversations for {root} — this project's are the only ones \
             --resume continues. `openalpaca sessions --all` lists every one the daemon \
             holds, and `chat --session <id>` continues one by id"
        ),
        (None, None) => bail!("No stored conversations on this lane yet — send a message first"),
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

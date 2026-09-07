//! Repository for `subagent_span` — the per-subagent spans the run timeline
//! is drawn from (migration 037, GAP-09).
//!
//! One row per spawned agent, opened the moment the spawn is announced and
//! closed when its loop returns. `agent_task_history` cannot serve this: it is
//! written only *after* a run finishes, so an in-flight span has no row there
//! at all, and it stores no start time.
//!
//! `blocked` is a legal state word but is never written here. A lane is
//! blocked only while the confirmation broker actually holds a pending request
//! for it, which is live process state — deriving it at read time means a
//! confirmation answered while the daemon was down can never leave a lane
//! stuck red.

use crate::Database;
use anyhow::{Context, Result};
use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

#[cfg(test)]
mod tests;

/// The state word a span carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpanState {
    Running,
    Done,
    Failed,
    /// Derived at read time from the confirmation broker; never persisted.
    Blocked,
    Cancelled,
}

impl SpanState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Blocked => "blocked",
            Self::Cancelled => "cancelled",
        }
    }
}

impl std::fmt::Display for SpanState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The detail a span closed by [`SubagentSpanRepository::close_orphans`] carries.
pub const SPAN_DETAIL_INTERRUPTED: &str = "interrupted";

/// One span row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentSpanRecord {
    /// The spawn's `node_id` — the same id its `SubagentSpan` events carry.
    pub id: String,
    pub task_id: String,
    /// The agent template ("research_agent").
    pub template_id: String,
    /// The runtime instance ("research_agent::a1b2c3d4").
    pub agent_instance_id: String,
    /// `<short template>·<ordinal>`, unique within the task.
    pub label: String,
    pub objective: Option<String>,
    pub state: String,
    pub detail: Option<String>,
    /// RFC 3339, UTC.
    pub started_at: String,
    pub ended_at: Option<String>,
    pub duration_ms: Option<i64>,
    pub output_preview: Option<String>,
}

/// How often one agent template has completed a run, and when the newest of
/// those started (GAP-20, T48). Counted from `subagent_span` rather than
/// `agent_task_history`, which only ever knew about runs that had already
/// returned and carried no start time.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TemplateRunCount {
    /// Completed spans this template opened within the query's window —
    /// `state != 'running'`. A run still in flight is not a run yet.
    pub run_count: i64,
    /// The newest counted span's `started_at` (RFC 3339, UTC); `None` is
    /// impossible for a template that has an entry at all, and is carried as
    /// an `Option` only because `MAX()` is nullable in SQL.
    pub last_run_at: Option<String>,
}

/// The fields a caller supplies when a span opens. `label`, `started_at` and
/// `state` are the repository's to assign.
#[derive(Debug, Clone, Copy)]
pub struct NewSubagentSpan<'a> {
    pub id: &'a str,
    pub task_id: &'a str,
    pub template_id: &'a str,
    pub agent_instance_id: &'a str,
    pub objective: Option<&'a str>,
}

const SELECT_COLUMNS: &str = "id, task_id, template_id, agent_instance_id, label, objective, \
     state, detail, started_at, ended_at, duration_ms, output_preview";

fn row_to_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<SubagentSpanRecord> {
    Ok(SubagentSpanRecord {
        id: row.get(0)?,
        task_id: row.get(1)?,
        template_id: row.get(2)?,
        agent_instance_id: row.get(3)?,
        label: row.get(4)?,
        objective: row.get(5)?,
        state: row.get(6)?,
        detail: row.get(7)?,
        started_at: row.get(8)?,
        ended_at: row.get(9)?,
        duration_ms: row.get(10)?,
        output_preview: row.get(11)?,
    })
}

/// The lane label's left half: a template id shortened to something that fits
/// the 70px mono lane gutter (DESIGN_SPEC §3.21's `review·3`).
///
/// The `_agent` suffix every template carries is noise once every lane has it,
/// and a multi-word id is cut to its first word rather than truncated
/// mid-syllable. Case and separators are normalised so two spellings of the
/// same template cannot open two ordinal series.
pub fn short_template(template_id: &str) -> String {
    let lowered = template_id.to_ascii_lowercase();
    let base = lowered.strip_suffix("_agent").unwrap_or(&lowered);
    let first = base
        .split(['_', '-', ' ', ':'])
        .find(|segment| !segment.is_empty())
        .unwrap_or("");
    let cleaned: String = first
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(12)
        .collect();
    if cleaned.is_empty() {
        "agent".to_string()
    } else {
        cleaned
    }
}

/// Row mapper for [`SubagentSpanRepository::run_counts_by_template`], shared
/// between its cutoff and no-cutoff query shapes so the two branches cannot
/// drift.
fn row_to_run_count(row: &rusqlite::Row<'_>) -> rusqlite::Result<(String, TemplateRunCount)> {
    Ok((
        row.get::<_, String>(0)?,
        TemplateRunCount {
            run_count: row.get(1)?,
            last_run_at: row.get(2)?,
        },
    ))
}

fn now_rfc3339() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// Milliseconds between two RFC 3339 stamps, clamped at zero. `None` when the
/// stored start cannot be parsed (a hand-edited row).
fn elapsed_ms(started_at: &str, ended_at: &DateTime<Utc>) -> Option<i64> {
    let start = DateTime::parse_from_rfc3339(started_at).ok()?;
    Some((ended_at.timestamp_millis() - start.timestamp_millis()).max(0))
}

/// Repository for subagent span operations.
pub struct SubagentSpanRepository<'a> {
    db: &'a Database,
}

impl<'a> SubagentSpanRepository<'a> {
    pub fn new(db: &'a Database) -> Self {
        Self { db }
    }

    /// Open a span. Assigns the run-unique label and returns the stored row.
    ///
    /// The label is computed and inserted inside one `with_connection`, which
    /// holds the connection mutex, so two subagents spawned in the same batch
    /// cannot be handed the same ordinal.
    pub fn open(&self, span: NewSubagentSpan<'_>) -> Result<SubagentSpanRecord> {
        let started_at = now_rfc3339();
        let short = short_template(span.template_id);

        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare("SELECT label FROM subagent_span WHERE task_id = ?1")?;
            let existing: Vec<String> = stmt
                .query_map(rusqlite::params![span.task_id], |row| row.get(0))?
                .collect::<std::result::Result<Vec<String>, _>>()?;

            let mut ordinal = existing
                .iter()
                .filter(|label| label.rsplit_once('·').is_some_and(|(head, _)| head == short))
                .count()
                + 1;
            // Defensive: a hand-inserted row could already hold this label.
            while existing.iter().any(|l| l == &format!("{short}·{ordinal}")) {
                ordinal += 1;
            }
            let label = format!("{short}·{ordinal}");

            conn.execute(
                "INSERT INTO subagent_span \
                 (id, task_id, template_id, agent_instance_id, label, objective, state, started_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'running', ?7)",
                rusqlite::params![
                    span.id,
                    span.task_id,
                    span.template_id,
                    span.agent_instance_id,
                    label,
                    span.objective,
                    started_at,
                ],
            )
            .context("Failed to insert subagent span")?;

            Ok(SubagentSpanRecord {
                id: span.id.to_string(),
                task_id: span.task_id.to_string(),
                template_id: span.template_id.to_string(),
                agent_instance_id: span.agent_instance_id.to_string(),
                label,
                objective: span.objective.map(str::to_string),
                state: SpanState::Running.as_str().to_string(),
                detail: None,
                started_at,
                ended_at: None,
                duration_ms: None,
                output_preview: None,
            })
        })
    }

    /// Close a still-running span. Returns the closed row, or `None` when the
    /// span is unknown or already closed — closing is idempotent, and the
    /// first terminal state wins so a late arrival cannot rewrite history.
    pub fn close(
        &self,
        id: &str,
        state: SpanState,
        detail: Option<&str>,
        output_preview: Option<&str>,
    ) -> Result<Option<SubagentSpanRecord>> {
        let ended = Utc::now();
        let ended_at = ended.to_rfc3339_opts(SecondsFormat::Millis, true);

        self.db.with_connection(|conn| {
            let started_at: Option<String> = conn
                .query_row(
                    "SELECT started_at FROM subagent_span WHERE id = ?1 AND state = 'running'",
                    rusqlite::params![id],
                    |row| row.get(0),
                )
                .optional()
                .context("Failed to read subagent span")?;
            let Some(started_at) = started_at else {
                return Ok(None);
            };
            let duration_ms = elapsed_ms(&started_at, &ended);

            conn.execute(
                "UPDATE subagent_span \
                 SET state = ?1, detail = ?2, output_preview = ?3, ended_at = ?4, duration_ms = ?5 \
                 WHERE id = ?6 AND state = 'running'",
                rusqlite::params![
                    state.as_str(),
                    detail,
                    output_preview,
                    ended_at,
                    duration_ms,
                    id
                ],
            )
            .context("Failed to close subagent span")?;

            let record = conn
                .query_row(
                    &format!("SELECT {SELECT_COLUMNS} FROM subagent_span WHERE id = ?1"),
                    rusqlite::params![id],
                    row_to_record,
                )
                .optional()
                .context("Failed to re-read closed subagent span")?;
            Ok(record)
        })
    }

    /// Every span of one run, oldest first.
    pub fn list_for_task(&self, task_id: &str) -> Result<Vec<SubagentSpanRecord>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(&format!(
                "SELECT {SELECT_COLUMNS} FROM subagent_span \
                 WHERE task_id = ?1 ORDER BY started_at, id"
            ))?;
            let rows = stmt
                .query_map(rusqlite::params![task_id], row_to_record)?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }

    /// Run counts for every template that has at least one *completed* span
    /// within the window, keyed by template id (GAP-20, T48).
    ///
    /// "Completed" is `state != 'running'` — done, failed, blocked-resolved
    /// and cancelled all count; a span still open does not, because a
    /// subagent the lead is still waiting on has not produced a run yet.
    /// `since` is the UTC instant `started_at` must fall at or after; `None`
    /// is `?window=all` — no time filter, just the completed-only one.
    ///
    /// One grouped query for the whole list — the Agents panel renders every
    /// template from one call, and a per-template count would be an N+1 over a
    /// list that grows with the config directory. A template with no matching
    /// span is absent from the map; the caller reports it as `0` rather than
    /// this query inventing rows for ids it has never seen.
    pub fn run_counts_by_template(
        &self,
        since: Option<DateTime<Utc>>,
    ) -> Result<std::collections::HashMap<String, TemplateRunCount>> {
        self.db.with_connection(|conn| {
            let cutoff = since.map(|dt| dt.to_rfc3339_opts(SecondsFormat::Millis, true));
            let mut sql = String::from(
                "SELECT template_id, COUNT(*), MAX(started_at) \
                 FROM subagent_span WHERE state != 'running'",
            );
            if cutoff.is_some() {
                sql.push_str(" AND started_at >= ?1");
            }
            sql.push_str(" GROUP BY template_id");

            let mut stmt = conn.prepare(&sql)?;
            let rows = match &cutoff {
                Some(cutoff) => stmt
                    .query_map(rusqlite::params![cutoff], row_to_run_count)?
                    .collect::<std::result::Result<std::collections::HashMap<_, _>, _>>(),
                None => stmt
                    .query_map([], row_to_run_count)?
                    .collect::<std::result::Result<std::collections::HashMap<_, _>, _>>(),
            }
            .context("Failed to read subagent span run counts")?;
            Ok(rows)
        })
    }

    /// How many spans each of the given runs opened, keyed by task id — the
    /// per-run agent count `GET /v1/tasks` serves.
    ///
    /// One grouped query for the whole page, the same shape
    /// [`LlmUsageRepository::cost_for_tasks`] uses for the page's costs: the
    /// `assigned_agents` array P8 deleted cost one `agent_task_history` read
    /// per row of every list page. A run with no spans is absent from the map,
    /// and the caller reports `0`.
    ///
    /// [`LlmUsageRepository::cost_for_tasks`]: crate::LlmUsageRepository::cost_for_tasks
    pub fn counts_for_tasks(
        &self,
        task_ids: &[String],
    ) -> Result<std::collections::HashMap<String, i64>> {
        if task_ids.is_empty() {
            return Ok(std::collections::HashMap::new());
        }
        self.db.with_connection(|conn| {
            let placeholders: String = task_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            let sql = format!(
                "SELECT task_id, COUNT(*) FROM subagent_span \
                 WHERE task_id IN ({placeholders}) GROUP BY task_id"
            );
            let mut stmt = conn.prepare(&sql)?;
            let params: Vec<&dyn rusqlite::ToSql> = task_ids
                .iter()
                .map(|id| id as &dyn rusqlite::ToSql)
                .collect();
            let rows = stmt
                .query_map(params.as_slice(), |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
                })?
                .collect::<std::result::Result<std::collections::HashMap<_, _>, _>>()
                .context("Failed to read subagent span counts by task")?;
            Ok(rows)
        })
    }

    /// Boot-time sweep: a span still `running` on a task that has already
    /// reached a terminal state belongs to a dead daemon generation — the
    /// tokio task that would have closed it is gone. Report it as `cancelled`
    /// / `"interrupted"` rather than leaving a lane running forever.
    ///
    /// "Terminal" includes `interrupted` (§5.6b): that is the status the boot
    /// sweep now writes for a run the previous incarnation left in flight, and
    /// it is precisely the run whose spans this pass exists to close. It used
    /// to be `failed`, so leaving it out of the list here would have made this
    /// sweep match nothing on exactly the boot it matters.
    ///
    /// Idempotent: it matches nothing on the next boot. Returns the number of
    /// spans closed.
    pub fn close_orphans(&self) -> Result<usize> {
        let ended = Utc::now();
        let ended_at = ended.to_rfc3339_opts(SecondsFormat::Millis, true);

        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT s.id, s.started_at FROM subagent_span s \
                 JOIN task t ON t.id = s.task_id \
                 WHERE s.state = 'running' \
                   AND t.status IN ('completed', 'failed', 'cancelled', 'interrupted')",
            )?;
            let stale: Vec<(String, String)> = stmt
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<std::result::Result<Vec<_>, _>>()?;

            let mut closed = 0usize;
            for (id, started_at) in stale {
                let duration_ms = elapsed_ms(&started_at, &ended);
                closed += conn
                    .execute(
                        "UPDATE subagent_span \
                         SET state = 'cancelled', detail = ?1, ended_at = ?2, duration_ms = ?3 \
                         WHERE id = ?4 AND state = 'running'",
                        rusqlite::params![SPAN_DETAIL_INTERRUPTED, ended_at, duration_ms, id],
                    )
                    .context("Failed to close orphaned subagent span")?;
            }
            Ok(closed)
        })
    }
}

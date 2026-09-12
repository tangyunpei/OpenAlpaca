//! Repository for skill and tool execution telemetry

use crate::models::skill_execution::{PREVIEW_CHARS, SkillExecutionEntry, ToolExecutionEntry};
use crate::models::skill_health::SkillHealthMetrics;
use crate::Database;
use anyhow::{Context, Result};
use chrono::{DateTime, NaiveDateTime, Utc};
use std::collections::HashMap;


/// Repository for skill/tool execution log operations.
pub struct SkillExecutionRepository<'a> {
    db: &'a Database,
}

impl<'a> SkillExecutionRepository<'a> {
    pub fn new(db: &'a Database) -> Self {
        Self { db }
    }

    /// Record a skill execution entry. Returns the row id.
    pub fn record(&self, entry: &SkillExecutionEntry) -> Result<i64> {
        self.db.with_connection(|conn| {
            conn.execute(
                "INSERT INTO skill_execution_log (
                    request_id, skill_id, agent_id, status, finish_reason,
                    error_message, validation_failures, duration_ms,
                    rounds_used, tool_calls_made, input_tokens, output_tokens,
                    cost_usd, model_used, query_preview, route_score,
                    was_auto_selected, repair_attempted, repair_succeeded
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)",
                rusqlite::params![
                    entry.request_id,
                    entry.skill_id,
                    entry.agent_id,
                    entry.status,
                    entry.finish_reason,
                    entry.error_message,
                    entry.validation_failures,
                    entry.duration_ms,
                    entry.rounds_used,
                    entry.tool_calls_made,
                    entry.input_tokens,
                    entry.output_tokens,
                    entry.cost_usd,
                    entry.model_used,
                    entry.query_preview,
                    entry.route_score,
                    entry.was_auto_selected as i32,
                    entry.repair_attempted as i32,
                    entry.repair_succeeded as i32,
                ],
            )
            .context("Failed to insert skill execution log")?;
            Ok(conn.last_insert_rowid())
        })
    }

    /// Record a tool execution entry — the **audit half** of the row, written
    /// unconditionally by the daemon for every executed call (R51). Returns
    /// the row id.
    ///
    /// The row is never best-effort: `GET /v1/tools`' `invocations_today`
    /// counts these, and the session writer's copy can be dropped by a full
    /// channel or lost with a cancelled round. What the writer adds is the
    /// *index* half — `log_seq`, the previews, `result_ref` — merged onto this
    /// same row by [`attach_session_index`](Self::attach_session_index).
    ///
    /// Whichever half arrives second merges rather than inserting, so the two
    /// can race without producing two rows or losing a `log_seq` that already
    /// landed. Both previews and `error_message` are clamped to
    /// [`PREVIEW_CHARS`] here rather than at the call sites: the bound belongs
    /// to the column, and the payload it previews is stored in full in the
    /// session event log.
    pub fn record_tool(&self, entry: &ToolExecutionEntry) -> Result<i64> {
        self.db.with_connection(|conn| {
            // The session writer may have created this call's row first — its
            // record can beat the daemon's event to the table. Merging the
            // audit half onto it keeps the row count honest and leaves the
            // writer's `log_seq` and previews alone.
            if let Some(id) = counterpart(conn, entry, Counterpart::WriterRow)? {
                conn.execute(
                    "UPDATE tool_execution_log
                        SET agent_id = ?1, tool_name = ?2, success = ?3, duration_ms = ?4,
                            error_message = COALESCE(?5, error_message),
                            task_id = COALESCE(task_id, ?6)
                      WHERE id = ?7",
                    rusqlite::params![
                        entry.agent_id,
                        entry.tool_name,
                        entry.success as i32,
                        entry.duration_ms,
                        entry.error_message.as_deref().map(clamp_preview),
                        entry.task_id,
                        id,
                    ],
                )
                .context("Failed to merge a tool execution audit row")?;
                return Ok(id);
            }
            insert_tool_row(conn, entry)
        })
    }

    /// Attach the session log's **index half** to a call's row (§5.4, R51):
    /// `log_seq`, the two previews and `result_ref`, which only the session
    /// writer knows because it is the only party that assigns a `seq`.
    ///
    /// Merges onto the daemon's audit row for the same call — matched by the
    /// call's tool-use id within the session — and inserts a whole row only
    /// when there is none to merge onto (the audit event was lost, or the
    /// call never reached the sandbox). Never touches the audit columns of a
    /// row it did not create: that half is the daemon's.
    pub fn attach_session_index(&self, entry: &ToolExecutionEntry) -> Result<i64> {
        self.db.with_connection(|conn| {
            if let Some(id) = counterpart(conn, entry, Counterpart::AuditRow)? {
                conn.execute(
                    "UPDATE tool_execution_log
                        SET session_id = ?1,
                            task_id = COALESCE(?2, task_id),
                            log_seq = ?3, args_preview = ?4, result_preview = ?5,
                            result_ref = ?6,
                            error_message = COALESCE(error_message, ?7)
                      WHERE id = ?8",
                    rusqlite::params![
                        entry.session_id,
                        entry.task_id,
                        entry.log_seq,
                        entry.args_preview.as_deref().map(clamp_preview),
                        entry.result_preview.as_deref().map(clamp_preview),
                        entry.result_ref,
                        entry.error_message.as_deref().map(clamp_preview),
                        id,
                    ],
                )
                .context("Failed to attach a session index to a tool execution row")?;
                return Ok(id);
            }
            insert_tool_row(conn, entry)
        })
    }

    /// Drop the session-log pointers on every `tool_execution_log` row of a
    /// session whose log has been evicted (T42 re-review, Minor 2).
    ///
    /// `log_seq` and `result_ref` are *addresses* inside
    /// `sessions/<id>/log.jsonl` and `sessions/<id>/results/`. When the boot
    /// sweep takes an archived session's live segment (R54), those files are
    /// gone — and if that session is ever reopened its `seq` restarts at 1, so
    /// the surviving rows would not merely dangle, they would name records of a
    /// *different* generation. Clearing them is the write-first half of the
    /// eviction: the caller runs this **before** removing the file, so no
    /// window exists in which a row describes a record that is not there.
    ///
    /// The audit half of the row is untouched — `agent_id`, `tool_name`,
    /// `success`, `duration_ms`, the previews. That a tool ran is still true,
    /// and `GET /v1/tools`' `invocations_today` must not change because a
    /// narrative was trimmed; what stopped being true is *where to read it*.
    ///
    /// One statement, so one transaction. Returns the number of rows changed.
    pub fn clear_session_log_index(&self, session_id: &str) -> Result<usize> {
        self.db.with_connection(|conn| {
            let rows = conn
                .execute(
                    "UPDATE tool_execution_log SET log_seq = NULL, result_ref = NULL \
                     WHERE session_id = ?1 AND (log_seq IS NOT NULL OR result_ref IS NOT NULL)",
                    rusqlite::params![session_id],
                )
                .context("Failed to clear a session's tool-call index pointers")?;
            Ok(rows)
        })
    }

    /// `tool_name → COUNT(*)` over `tool_execution_log` since `since_utc`, for
    /// `GET /v1/tools`' `invocations_today` (GAP-18).
    ///
    /// `since_utc` is **already UTC** in the table's own `%Y-%m-%d %H:%M:%S`
    /// text form: `timestamp` defaults to `datetime('now')`, which is UTC, so a
    /// bare `date('now')` predicate would be off by the daemon's UTC offset.
    /// The caller converts local midnight.
    ///
    /// `INDEXED BY idx_tel_timestamp` (migration 041) because the planner does
    /// not choose it on its own: 030's `idx_tel_tool_ts (tool_name, timestamp
    /// DESC)` satisfies the `GROUP BY` in index order, so an unhinted plan
    /// trades a temp B-tree for a **full covering scan of an append-only log**
    /// — work that grows for ever, under the daemon's one connection lock
    /// (review R80). Today's rows are a vanishing fraction of the log, so the
    /// range search plus the sort is the plan that stays bounded. The hint is
    /// safe by construction: the index is created by the migration, not by a
    /// tuning pass, and SQLite refuses to prepare the statement if it is gone.
    ///
    /// One grouped query rather than one per tool: the registry has hundreds of
    /// names and the route renders all of them.
    pub fn tool_invocations_since(&self, since_utc: &str) -> Result<HashMap<String, i64>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT tool_name, COUNT(*) FROM tool_execution_log
                 INDEXED BY idx_tel_timestamp
                 WHERE timestamp >= ?1 GROUP BY tool_name",
            )?;
            let rows = stmt.query_map([since_utc], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?;
            let mut counts = HashMap::new();
            for row in rows {
                let (name, count) = row?;
                counts.insert(name, count);
            }
            Ok(counts)
        })
    }

    /// `skill_id → COUNT(*)` over `skill_execution_log` since `since_utc`, for
    /// `GET /v1/skills`' `invocations_today`.
    ///
    /// The tool-log sibling above, one table over: `skill_execution_log`
    /// carries the same `datetime('now')` UTC text `timestamp` (migration 030),
    /// so the caller converts local midnight to UTC exactly the same way, the
    /// predicate is the same plain text comparison, and 041's
    /// `idx_sel_timestamp` is named for the same reason — `idx_sel_skill_ts
    /// (skill_id, timestamp DESC)` would otherwise be scanned whole.
    ///
    /// Deliberately **not** the `all_skill_health` query: that one is lifetime
    /// totals per skill, and the catalog wants today's.
    pub fn skill_invocations_since(&self, since_utc: &str) -> Result<HashMap<String, i64>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT skill_id, COUNT(*) FROM skill_execution_log
                 INDEXED BY idx_sel_timestamp
                 WHERE timestamp >= ?1 GROUP BY skill_id",
            )?;
            let rows = stmt.query_map([since_utc], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?;
            let mut counts = HashMap::new();
            for row in rows {
                let (id, count) = row?;
                counts.insert(id, count);
            }
            Ok(counts)
        })
    }

    /// Get health metrics for all skills, enriched with user feedback data.
    pub fn all_skill_health(&self) -> Result<Vec<SkillHealthMetrics>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT
                    sel.skill_id,
                    COUNT(*) AS total,
                    CAST(SUM(CASE WHEN sel.finish_reason = 'complete' AND sel.repair_attempted = 0
                         THEN 1 ELSE 0 END) AS REAL) / COUNT(*) AS clean_success_rate,
                    CAST(SUM(CASE WHEN sel.finish_reason = 'complete' AND sel.repair_attempted = 0
                         AND sel.timestamp >= datetime('now', '-7 days') THEN 1 ELSE 0 END) AS REAL)
                    / NULLIF(SUM(CASE WHEN sel.timestamp >= datetime('now', '-7 days') THEN 1 ELSE 0 END), 0)
                        AS clean_success_rate_7d,
                    CAST(SUM(sel.repair_attempted) AS REAL) / COUNT(*) AS repair_rate,
                    CASE WHEN SUM(sel.repair_attempted) > 0
                         THEN CAST(SUM(sel.repair_succeeded) AS REAL) / SUM(sel.repair_attempted)
                         ELSE 0.0 END AS repair_effectiveness,
                    CAST(SUM(CASE WHEN sel.finish_reason IN ('max_rounds', 'truncated')
                         THEN 1 ELSE 0 END) AS REAL) / COUNT(*) AS degraded_rate,
                    AVG(sel.duration_ms) AS avg_duration_ms,
                    AVG(sel.cost_usd) AS avg_cost_usd,
                    AVG(sel.rounds_used) AS avg_rounds,
                    MAX(sel.timestamp) AS last_invoked_at,
                    fb.fb_count,
                    fb.satisfaction,
                    fb.coverage
                FROM skill_execution_log sel
                LEFT JOIN (
                    SELECT sel2.skill_id,
                        COUNT(mf.id) AS fb_count,
                        CAST(SUM(CASE WHEN mf.feedback = 'positive' THEN 1 ELSE 0 END) AS REAL)
                            / NULLIF(COUNT(mf.id), 0) AS satisfaction,
                        CAST(COUNT(mf.id) AS REAL) / COUNT(sel2.id) AS coverage
                    FROM skill_execution_log sel2
                    LEFT JOIN message_feedback mf ON mf.message_id = sel2.response_message_id
                    GROUP BY sel2.skill_id
                ) fb ON fb.skill_id = sel.skill_id
                GROUP BY sel.skill_id
                HAVING COUNT(*) >= 1",
            )?;
            let rows = stmt.query_map([], |row| {
                let clean_success_rate: f64 = row.get(2)?;
                let clean_success_rate_7d: Option<f64> = row.get(3)?;
                let last_ts: Option<String> = row.get(10)?;
                let fb_count: Option<i64> = row.get(11)?;
                let satisfaction: Option<f64> = row.get(12)?;
                let coverage: Option<f64> = row.get(13)?;
                Ok(SkillHealthMetrics {
                    skill_id: row.get(0)?,
                    total_invocations: row.get::<_, i64>(1)? as u64,
                    clean_success_rate,
                    clean_success_rate_7d: clean_success_rate_7d.unwrap_or(clean_success_rate),
                    repair_rate: row.get(4)?,
                    repair_effectiveness: row.get(5)?,
                    degraded_rate: row.get(6)?,
                    avg_duration_ms: row.get(7)?,
                    avg_cost_usd: row.get(8)?,
                    avg_rounds: row.get::<_, Option<f64>>(9)?.unwrap_or(0.0),
                    last_invoked_at: last_ts.and_then(|s| parse_datetime(&s)),
                    user_satisfaction_rate: satisfaction,
                    feedback_count: fb_count.unwrap_or(0) as u64,
                    feedback_coverage: coverage.unwrap_or(0.0),
                })
            })?;
            let mut metrics = Vec::new();
            for row in rows {
                metrics.push(row?);
            }
            Ok(metrics)
        })
    }

    /// Link a skill execution to its response message (for feedback correlation).
    pub fn link_response(&self, request_id: &str, message_id: i64) -> Result<bool> {
        self.db.with_connection(|conn| {
            let changed = conn.execute(
                "UPDATE skill_execution_log SET response_message_id = ?1 WHERE request_id = ?2",
                rusqlite::params![message_id, request_id],
            )?;
            Ok(changed > 0)
        })
    }

    /// Delete old telemetry rows. Returns (skill_rows_deleted, tool_rows_deleted).
    pub fn cleanup_old(&self, skill_days: u32, tool_days: u32) -> Result<(usize, usize)> {
        self.db.with_connection(|conn| {
            let skill_deleted = conn.execute(
                "DELETE FROM skill_execution_log WHERE timestamp < datetime('now', ?)",
                rusqlite::params![format!("-{skill_days} days")],
            )?;
            let tool_deleted = conn.execute(
                "DELETE FROM tool_execution_log WHERE timestamp < datetime('now', ?)",
                rusqlite::params![format!("-{tool_days} days")],
            )?;
            Ok((skill_deleted, tool_deleted))
        })
    }
}

/// Which half of a call's row we are looking for: the one the *other* writer
/// would have made (R51).
#[derive(Clone, Copy)]
enum Counterpart {
    /// A row the session writer created — it carries a `log_seq`.
    WriterRow,
    /// A row the daemon's audit path created — it has no `log_seq` yet.
    AuditRow,
}

/// The other half's row for this call, if there is one to merge onto.
///
/// Matched by the call's tool-use id **within its session**: both are needed,
/// because `request_id` is the provider's id for the call and only unique
/// beside the session it was made in. Without both — a call outside any
/// session log, or a provider that emitted no id — there is nothing safe to
/// match on and the caller inserts a fresh row.
fn counterpart(
    conn: &rusqlite::Connection,
    entry: &ToolExecutionEntry,
    want: Counterpart,
) -> Result<Option<i64>> {
    let (Some(session_id), Some(request_id)) =
        (entry.session_id.as_deref(), entry.request_id.as_deref())
    else {
        return Ok(None);
    };
    if request_id.is_empty() {
        return Ok(None);
    }
    let sql = match want {
        Counterpart::WriterRow => {
            "SELECT id FROM tool_execution_log
              WHERE request_id = ?1 AND session_id = ?2 AND log_seq IS NOT NULL
              ORDER BY id DESC LIMIT 1"
        }
        Counterpart::AuditRow => {
            "SELECT id FROM tool_execution_log
              WHERE request_id = ?1 AND session_id = ?2 AND log_seq IS NULL
              ORDER BY id DESC LIMIT 1"
        }
    };
    use rusqlite::OptionalExtension;
    conn.query_row(sql, rusqlite::params![request_id, session_id], |r| r.get(0))
        .optional()
        .context("Failed to look up a tool execution row")
}

/// The one INSERT both halves fall back to when there is nothing to merge.
fn insert_tool_row(conn: &rusqlite::Connection, entry: &ToolExecutionEntry) -> Result<i64> {
    conn.execute(
        "INSERT INTO tool_execution_log (
            request_id, agent_id, tool_name, success, duration_ms, error_message,
            session_id, task_id, log_seq, args_preview, result_preview, result_ref
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        rusqlite::params![
            entry.request_id,
            entry.agent_id,
            entry.tool_name,
            entry.success as i32,
            entry.duration_ms,
            entry.error_message.as_deref().map(clamp_preview),
            entry.session_id,
            entry.task_id,
            entry.log_seq,
            entry.args_preview.as_deref().map(clamp_preview),
            entry.result_preview.as_deref().map(clamp_preview),
            entry.result_ref,
        ],
    )
    .context("Failed to insert tool execution log")?;
    Ok(conn.last_insert_rowid())
}

/// Clamp a preview to the column's documented bound, on a character boundary.
fn clamp_preview(s: &str) -> String {
    s.chars().take(PREVIEW_CHARS).collect()
}

fn parse_datetime(s: &str) -> Option<DateTime<Utc>> {
    NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
        .ok()
        .map(|ndt| ndt.and_utc())
}

/// Resolve a value read out of `skill_execution_log.skill_id` onto the catalog
/// id that owns it. Returns `None` when no entry claims the key.
///
/// # Why this exists: the column does not hold what it is named
///
/// `skill_execution_log.skill_id` holds the skill's **`frontmatter.name`**, not
/// its catalog id (the lowercased directory name). Every live invocation path
/// resolves the catalog entry and then passes the display name on as the
/// invocation identity — `/slash` and router selection through
/// `Intent::SkillInvocation` (`orchestrator/intent/skill_match.rs`), and the
/// model's `invoke_skill` tool — and both writers persist that string:
/// `orchestrator/skill/handler.rs` and `tools/builtins/invoke_skill.rs`. The
/// scheduled-skill path injects `/{command}` through the gateway, so it logs
/// the frontmatter name too. Only historical rows carry the id spelling.
///
/// Two consequences the callers of this function are working around, both real:
///
///  * renaming a skill silently **splits its history** — new rows land under
///    the new display name while the old ones keep the old one;
///  * `GET /v1/skills/health` groups on a display name, so the two catalogs
///    disagree about what a "skill id" is.
///
/// # The rule
///
/// A logged key is resolved the way `SkillCatalog::get`
/// (`openalpaca_core/src/orchestrator/skill/catalog/mod.rs`) resolves one:
/// lowercased, **id first, then frontmatter name**. Ids win outright — the name
/// arm is only consulted after the whole catalog has failed to match on id — so
/// one entry's id is never shadowed by another entry's name. A name collision
/// (possible across scopes, and between a file skill and a plugin one) is
/// broken on the **lowest id**, which is what makes two reads of an unchanged
/// catalog agree however the caller's `HashMap` ordered the entries.
///
/// # The fix this defers
///
/// The real repair is to log the catalog id and migrate the column
/// (`UPDATE skill_execution_log SET skill_id = <catalog id>` for the rows whose
/// value matches a known frontmatter name, or an accepted split), plus changing
/// the two writers above. That is a change to an invocation identity shared
/// with the skill executor's catalog lookups and its cycle checks, and it needs
/// a backfill decision, so it is **not** done here. The plan's §11 migration
/// ledger reserves 035–039 for other work and holds **no number for it** — one
/// has to be allocated when the owner takes it.
///
/// `entries` is `(catalog id, frontmatter name)`, the shape every caller can
/// produce; the returned `&str` borrows from it.
pub fn resolve_skill_key<'a, I>(id_or_name: &str, entries: I) -> Option<&'a str>
where
    I: IntoIterator<Item = (&'a str, &'a str)>,
{
    let key = id_or_name.to_lowercase();
    let mut by_name: Option<&'a str> = None;
    for (id, name) in entries {
        if id.to_lowercase() == key {
            return Some(id);
        }
        if name.to_lowercase() == key && by_name.is_none_or(|prev| id < prev) {
            by_name = Some(id);
        }
    }
    by_name
}

#[cfg(test)]
mod tests;

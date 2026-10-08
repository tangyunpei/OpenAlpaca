//! ST-19's delete core: the plan, the rows, the folder (design §6, *Delete
//! its data*). DB and files only — the live-run guard, the memory slot and
//! the partitions are the route's (`apps/openalpacad/src/routes/workspaces.rs`).
//!
//! One rule decides every statement: a row belongs to the project when one of
//! the five path columns names its root, or it hangs off a session or a run
//! that does. A message dies with its session **or** with its own
//! `workspace_root`, and a file is kept exactly when a message the delete
//! leaves still links it — so the plan, the kept set and the transaction
//! read one predicate ([`DOOMED_MESSAGE`], [`kept_ids`]), and a dry run and
//! the real call cannot disagree.

use std::fs;
use std::path::Path;

use anyhow::Result;

use crate::Database;
use crate::artifacts::{PurgeCounts, PurgeSession};
use crate::memory_files::{NOTES_DIR, OWNERS_DIR, SnapshotScope, list_snapshots};
use crate::store::ContentKind;
use crate::store::projects::{Project, ProjectId};

/// The project's conversations.
const DOOMED_SESSIONS: &str = "SELECT id FROM session WHERE workspace_id = ?1";
/// The project's runs.
const DOOMED_TASKS: &str = "SELECT id FROM task WHERE workspace_id = ?1";
/// A message the delete takes: one of the project's conversations, or one
/// naming the project itself — a turn in another conversation that ran in it.
const DOOMED_MESSAGE: &str =
    "workspace_root = ?1 OR session_id IN (SELECT id FROM session WHERE workspace_id = ?1)";

/// Every file a message the delete leaves still links, by attachment or as
/// a run's artifact.
fn kept_ids() -> String {
    format!(
        "SELECT a.file_id FROM conversation_message_attachments a
          WHERE a.message_id NOT IN (SELECT id FROM conversation_messages WHERE {DOOMED_MESSAGE})"
    )
}

/// Files of the project another conversation still links (step 1).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct KeptFiles {
    pub uploads: usize,
    pub artifacts: usize,
}

/// What deleting a project's data would do, read without changing anything.
#[derive(Debug, Clone)]
pub struct DeletePlan {
    pub project: Project,
    /// What goes. Uploads and by-products here are the ones **not** kept.
    pub counts: PurgeCounts,
    pub kept: KeptFiles,
    /// The project's conversations, for the route's live-run guard.
    pub sessions: Vec<PurgeSession>,
    /// Control snapshots carrying this project's id: one per owner.
    pub memory_partitions: usize,
    /// Note files under `<dir>/memory/owners/*/notes/`.
    pub memory_notes: usize,
}

/// The plan of [`delete_project_data`] for `project` — reads only, and never
/// iterates `storage_path`.
pub fn delete_plan(db: &Database, project: &Project) -> Result<DeletePlan> {
    let root = project.root_string();
    let (counts, kept, sessions) = db.with_connection(|conn| {
        let count = |sql: &str| -> Result<usize> {
            Ok(conn.query_row(sql, rusqlite::params![root], |row| row.get::<_, i64>(0))? as usize)
        };
        let sessions = {
            let mut stmt = conn
                .prepare("SELECT id, lane_key FROM session WHERE workspace_id = ?1 ORDER BY id")?;
            stmt.query_map(rusqlite::params![root], |row| {
                Ok(PurgeSession {
                    id: row.get(0)?,
                    lane_key: row.get(1)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<PurgeSession>>>()?
        };
        let linked = kept_ids();
        let kept = KeptFiles {
            uploads: count(&format!(
                "SELECT COUNT(*) FROM file_assets
                  WHERE project_root = ?1 AND origin = 'upload' AND id IN ({linked})"
            ))?,
            artifacts: count(&format!(
                "SELECT COUNT(*) FROM file_assets
                  WHERE project_root = ?1 AND origin <> 'upload' AND id IN ({linked})"
            ))?,
        };
        let counts = PurgeCounts {
            sessions: sessions.len(),
            messages: count(&format!(
                "SELECT COUNT(*) FROM conversation_messages WHERE {DOOMED_MESSAGE}"
            ))?,
            tool_calls: count(&format!(
                "SELECT COUNT(*) FROM tool_execution_log
                  WHERE session_id IN ({DOOMED_SESSIONS}) OR task_id IN ({DOOMED_TASKS})"
            ))?,
            followups: count(&format!(
                "SELECT COUNT(*) FROM lane_followups
                  WHERE workspace_path = ?1 OR session_id IN ({DOOMED_SESSIONS})"
            ))?,
            tasks: count("SELECT COUNT(*) FROM task WHERE workspace_id = ?1")?,
            spans: count(&format!(
                "SELECT COUNT(*) FROM subagent_span WHERE task_id IN ({DOOMED_TASKS})"
            ))?,
            run_events: count(&format!(
                "SELECT COUNT(*) FROM event_log WHERE task_id IN ({DOOMED_TASKS})"
            ))?,
            uploads: count(&format!(
                "SELECT COUNT(*) FROM file_assets
                  WHERE project_root = ?1 AND origin = 'upload' AND id NOT IN ({linked})"
            ))?,
            artifacts: count(&format!(
                "SELECT COUNT(*) FROM file_assets
                  WHERE project_root = ?1 AND origin <> 'upload' AND id NOT IN ({linked})"
            ))?,
        };
        Ok((counts, kept, sessions))
    })?;
    Ok(DeletePlan {
        project: project.clone(),
        counts,
        kept,
        sessions,
        memory_partitions: project_partitions(&project.id)?,
        memory_notes: note_files(&project.dir),
    })
}

/// Control snapshots of `id`'s partitions, every owner (R18, R19).
fn project_partitions(id: &ProjectId) -> Result<usize> {
    Ok(list_snapshots()?
        .into_iter()
        .filter_map(|(_, parsed)| parsed.ok())
        .filter(|s| s.scope == SnapshotScope::Project && s.project_id.as_ref() == Some(id))
        .count())
}

/// Note files under `dir/memory/owners/*/notes/`: regular `*.md` files, not
/// dot-prefixed, reached through no link (`DirEntry::file_type` does not
/// follow one).
fn note_files(dir: &Path) -> usize {
    let owners = dir.join(ContentKind::Memory.dir_name()).join(OWNERS_DIR);
    let Ok(entries) = fs::read_dir(&owners) else {
        return 0;
    };
    entries
        .flatten()
        .filter(|owner| owner.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|owner| fs::read_dir(owner.path().join(NOTES_DIR)).ok())
        .flat_map(|notes| notes.flatten())
        .filter(|note| {
            let name = note.file_name();
            let name = name.to_string_lossy();
            !name.starts_with('.')
                && name.ends_with(".md")
                && note.file_type().is_ok_and(|t| t.is_file())
        })
        .count()
}

#[cfg(test)]
mod tests;

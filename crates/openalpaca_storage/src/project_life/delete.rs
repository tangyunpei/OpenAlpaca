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

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use rusqlite::Connection;

use crate::Database;
use crate::artifacts::{PurgeCounts, PurgeSession};
use crate::content_io::{fsync_dir, remove_best_effort};
use crate::memory_files::{NOTES_DIR, OWNERS_DIR, SnapshotScope, list_snapshots};
use crate::store::projects::{self, Project, ProjectId};
use crate::store::{ContentKind, StoreScope, confine_to_root, content_dir};

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

/// Where a kept file lands in the root store:
/// `<uploads|artifacts>/kept/<project folder>-<project id>/<its old rel_path>`.
/// No grammar path starts with it, the id makes it fresh for every project
/// ever deleted, and the old `rel_path` was unique within the project.
pub const KEPT_DIR: &str = "kept";

/// What [`delete_project_data`] did — read inside its transaction.
#[derive(Debug, Clone, Default)]
pub struct DeleteOutcome {
    pub counts: PurgeCounts,
    pub kept: KeptFiles,
    /// The sessions whose `sessions/<id>/` directory is now rowless.
    pub session_ids: Vec<String>,
    /// Every message the transaction deleted, in id order — the route forgets
    /// those conversations' summaries by them (design 09 §13.3).
    pub message_ids: Vec<i64>,
}

/// [`delete_project_data`] refused: inside its own transaction, `runs` runs
/// of the project — under its root, or started from one of its
/// conversations — were `queued`, `running` or `paused`. The route checks
/// for runs before it waits for the project's memory slot; this is the check
/// a run started meanwhile cannot slip past. Returned inside the
/// `anyhow::Error`: nothing was deleted, and the copies were taken back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunsInFlight {
    pub root: String,
    pub runs: usize,
}

impl fmt::Display for RunsInFlight {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} run(s) of {} are queued, running or paused; nothing was deleted — delete its \
             data once they finish",
            self.runs, self.root
        )
    }
}

impl std::error::Error for RunsInFlight {}

/// A file step 1 keeps, as its row names it.
#[derive(Debug, Clone)]
struct KeptRow {
    id: String,
    upload: bool,
    rel_path: Option<String>,
}

/// Where step 1 put it.
#[derive(Debug, Clone)]
struct Placed {
    id: String,
    upload: bool,
    rel_path: String,
    storage_path: String,
    /// The copy this call wrote; `None` when the project held no bytes for it.
    written: Option<PathBuf>,
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

fn kept_rows(conn: &Connection, root: &str) -> Result<Vec<KeptRow>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT id, origin, rel_path FROM file_assets
          WHERE project_root = ?1 AND id IN ({}) ORDER BY id",
        kept_ids()
    ))?;
    let rows = stmt
        .query_map(rusqlite::params![root], |row| {
            let origin: String = row.get(1)?;
            Ok(KeptRow {
                id: row.get(0)?,
                upload: origin == "upload",
                rel_path: row.get(2)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<KeptRow>>>()?;
    Ok(rows)
}

/// Copies one kept file into the root store. The source is read from
/// `projects/<name>/<kind>/<rel_path>` confined to that directory — never from
/// `storage_path` — and a link out of it reads as no bytes.
fn place_one(project: &Project, row: &KeptRow) -> Result<Placed> {
    let kind = if row.upload {
        ContentKind::Uploads
    } else {
        ContentKind::Artifacts
    };
    let home_dir = content_dir(&StoreScope::Home, kind)?;
    let home_dir = home_dir
        .canonicalize()
        .with_context(|| format!("failed to canonicalize {}", home_dir.display()))?;
    let tail = row.rel_path.clone().unwrap_or_else(|| row.id.clone());
    let rel_path = format!("{KEPT_DIR}/{}-{}/{tail}", project.name, project.id);
    let joined = home_dir.join(&rel_path);
    let target = confine_to_root(&home_dir, &joined)?;
    // `confine_to_root` resolves a link it meets inside the root; the kept
    // address is written, and a stale copy at it replaced, through none.
    if target != joined {
        bail!(
            "{} has a link on its path; it was left in place",
            joined.display()
        );
    }
    let source_root = project.dir.join(kind.dir_name());
    let source = match row.rel_path.as_deref() {
        Some(rel) if source_root.is_dir() => confine_to_root(&source_root, &source_root.join(rel))
            .ok()
            .filter(|path| path.is_file()),
        _ => None,
    };
    let written = match source {
        Some(source) => {
            copy_fresh(&source, &target)?;
            Some(target.clone())
        }
        None => None,
    };
    Ok(Placed {
        id: row.id.clone(),
        upload: row.upload,
        rel_path,
        storage_path: target.to_string_lossy().into_owned(),
        written,
    })
}

/// `source` to `target`, synced. A regular file already at `target` is a
/// copy an earlier attempt wrote before the daemon died ahead of its
/// transaction — whole or cut short — and it is replaced: left, it would make
/// every later delete of the project fail. Nothing else can be there: no row
/// names `kept/<folder>-<id>/` until a transaction commits, and one that
/// committed re-addressed its kept rows away from the project, so no later
/// attempt copies onto them. Anything but a regular file is refused.
fn copy_fresh(source: &Path, target: &Path) -> Result<()> {
    let dir = target
        .parent()
        .with_context(|| format!("{} has no parent", target.display()))?;
    fs::create_dir_all(dir).with_context(|| format!("failed to create {}", dir.display()))?;
    let mut from =
        fs::File::open(source).with_context(|| format!("failed to open {}", source.display()))?;
    match fs::symlink_metadata(target) {
        Ok(meta) if meta.file_type().is_file() => fs::remove_file(target)
            .with_context(|| format!("failed to replace the stale copy {}", target.display()))?,
        Ok(_) => bail!(
            "{} is not a regular file; it was left in place",
            target.display()
        ),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e).with_context(|| format!("failed to stat {}", target.display())),
    }
    let mut to = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)
        .with_context(|| format!("failed to create {}", target.display()))?;
    let copied = std::io::copy(&mut from, &mut to).and_then(|_| to.sync_all());
    if let Err(e) = copied {
        drop(to);
        remove_best_effort(target);
        return Err(e).with_context(|| {
            format!(
                "failed to copy {} to {}",
                source.display(),
                target.display()
            )
        });
    }
    fsync_dir(dir);
    Ok(())
}

fn unplace(placed: &[Placed]) {
    for p in placed {
        if let Some(path) = &p.written {
            remove_best_effort(path);
        }
    }
}

fn strings(conn: &Connection, sql: &str, root: &str) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(sql)?;
    let out = stmt
        .query_map(rusqlite::params![root], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<String>>>()?;
    Ok(out)
}

/// §6 *Delete its data*, steps 1 and 2: the files a surviving message links
/// are copied into the root store, then one transaction re-addresses them
/// with no project (a by-product as its newest version only) and deletes
/// every row naming the project in the five path columns and the rows that
/// hang off its sessions and runs, `learning_cursor` included. A failure
/// anywhere deletes nothing and takes the copies back — [`RunsInFlight`]
/// when the transaction finds a run of the project in flight. Never iterates
/// `storage_path`; removes no file of the project (step 3 is the route's).
pub fn delete_project_data(db: &Database, project: &Project) -> Result<DeleteOutcome> {
    let root = project.root_string();
    let rows = db.with_connection(|conn| kept_rows(conn, &root))?;
    let mut placed: Vec<Placed> = Vec::with_capacity(rows.len());
    for row in &rows {
        match place_one(project, row) {
            Ok(p) => placed.push(p),
            Err(e) => {
                unplace(&placed);
                return Err(e.context(format!(
                    "keeping file {} of {root}; nothing was deleted",
                    row.id
                )));
            }
        }
    }
    let result = db.with_connection(|conn| {
        let tx = conn.unchecked_transaction()?;
        let outcome = delete_within(&tx, &root, &placed)?;
        tx.commit()?;
        Ok(outcome)
    });
    if result.is_err() {
        unplace(&placed);
    }
    result
}

fn delete_within(tx: &Connection, root: &str, placed: &[Placed]) -> Result<DeleteOutcome> {
    let bind = rusqlite::params![root];
    // Refused while a run is in flight (§6), checked again here: the route's
    // checks run before it waits for the memory slot, and a run started since
    // would have its row deleted under its lead. Every write of the daemon
    // goes through the one connection this transaction holds, so none can
    // start between this count and the commit.
    let runs = tx.query_row(
        &format!(
            "SELECT COUNT(*) FROM task
              WHERE status IN ('queued', 'running', 'paused')
                AND (workspace_id = ?1 OR session_id IN ({DOOMED_SESSIONS}))"
        ),
        bind,
        |row| row.get::<_, i64>(0),
    )? as usize;
    if runs > 0 {
        return Err(anyhow::Error::new(RunsInFlight {
            root: root.to_string(),
            runs,
        }));
    }
    // A file linked from a surviving message after the copies were made would
    // lose that link with the project's rows: refuse. The other direction — a
    // copied file whose surviving link went meanwhile — is still re-addressed
    // into the root store; the orphan-upload sweep takes an unlinked upload
    // once its grace period has passed.
    if let Some(row) = kept_rows(tx, root)?
        .iter()
        .find(|row| !placed.iter().any(|p| p.id == row.id))
    {
        bail!(
            "file {} of {root} was attached to another conversation while its data was being \
             deleted; nothing was deleted — try again",
            row.id
        );
    }
    let session_ids = strings(tx, &format!("{DOOMED_SESSIONS} ORDER BY id"), root)?;
    let message_ids: Vec<i64> = {
        let mut stmt = tx.prepare(&format!(
            "SELECT id FROM conversation_messages WHERE {DOOMED_MESSAGE} ORDER BY id"
        ))?;
        stmt.query_map(bind, |row| row.get::<_, i64>(0))?
            .collect::<rusqlite::Result<Vec<i64>>>()?
    };
    // Conversations that stay but lose a message naming the project.
    let touched = strings(
        tx,
        &format!(
            "SELECT DISTINCT session_id FROM conversation_messages
              WHERE workspace_root = ?1 AND session_id IS NOT NULL
                AND session_id NOT IN ({DOOMED_SESSIONS})"
        ),
        root,
    )?;

    // Step 1's rows, re-addressed before anything they hang off goes.
    for p in placed {
        tx.execute(
            "UPDATE file_assets
                SET project_root = NULL, rel_path = ?2, storage_path = ?3,
                    missing_since = CASE WHEN ?4 THEN NULL ELSE datetime('now') END,
                    updated_at = datetime('now')
              WHERE id = ?1 AND project_root = ?5",
            rusqlite::params![p.id, p.rel_path, p.storage_path, p.written.is_some(), root],
        )?;
        if !p.upload {
            tx.execute(
                "DELETE FROM artifact_versions
                  WHERE artifact_id = ?1
                    AND version <> (SELECT version FROM file_assets WHERE id = ?1)",
                rusqlite::params![p.id],
            )?;
            tx.execute(
                "UPDATE artifact_versions SET rel_path = ?2 WHERE artifact_id = ?1",
                rusqlite::params![p.id, p.rel_path],
            )?;
            tx.execute(
                "UPDATE file_assets SET version_count = 1 WHERE id = ?1",
                rusqlite::params![p.id],
            )?;
        }
    }

    // Messages first (their feedback and attachment links cascade), then
    // what hangs off the sessions, then the sessions.
    let messages = tx.execute(
        &format!("DELETE FROM conversation_messages WHERE {DOOMED_MESSAGE}"),
        bind,
    )?;
    for id in &touched {
        tx.execute(
            "UPDATE session
                SET message_count = (SELECT COUNT(*) FROM conversation_messages WHERE session_id = ?1),
                    updated_at = datetime('now')
              WHERE id = ?1",
            rusqlite::params![id],
        )?;
    }
    let tool_calls = tx.execute(
        &format!(
            "DELETE FROM tool_execution_log
              WHERE session_id IN ({DOOMED_SESSIONS}) OR task_id IN ({DOOMED_TASKS})"
        ),
        bind,
    )?;
    // A queued follow-up naming the project goes even when its session is
    // another project's: left, it would start later in a deleted project.
    let followups = tx.execute(
        &format!(
            "DELETE FROM lane_followups
              WHERE workspace_path = ?1 OR session_id IN ({DOOMED_SESSIONS})"
        ),
        bind,
    )?;
    tx.execute(
        &format!("DELETE FROM learning_cursor WHERE session_id IN ({DOOMED_SESSIONS})"),
        bind,
    )?;
    // A run of another project started from one of these conversations
    // keeps its row and loses the link, as `delete_session` does it.
    tx.execute(
        &format!(
            "UPDATE task SET session_id = NULL, updated_at = datetime('now')
              WHERE session_id IN ({DOOMED_SESSIONS})"
        ),
        bind,
    )?;
    let sessions = tx.execute("DELETE FROM session WHERE workspace_id = ?1", bind)?;

    let spans = tx.execute(
        &format!("DELETE FROM subagent_span WHERE task_id IN ({DOOMED_TASKS})"),
        bind,
    )?;
    let run_events = tx.execute(
        &format!("DELETE FROM event_log WHERE task_id IN ({DOOMED_TASKS})"),
        bind,
    )?;
    tx.execute(
        &format!("DELETE FROM dispatch_decisions WHERE task_id IN ({DOOMED_TASKS})"),
        bind,
    )?;
    tx.execute(
        &format!("DELETE FROM llm_call_log WHERE task_id IN ({DOOMED_TASKS})"),
        bind,
    )?;
    let tasks = tx.execute("DELETE FROM task WHERE workspace_id = ?1", bind)?;

    // The project's files the kept set did not take (their versions cascade).
    let uploads = tx.execute(
        "DELETE FROM file_assets WHERE project_root = ?1 AND origin = 'upload'",
        bind,
    )?;
    let artifacts = tx.execute("DELETE FROM file_assets WHERE project_root = ?1", bind)?;

    Ok(DeleteOutcome {
        counts: PurgeCounts {
            sessions,
            messages,
            tool_calls,
            followups,
            tasks,
            spans,
            run_events,
            uploads,
            artifacts,
        },
        kept: KeptFiles {
            uploads: placed.iter().filter(|p| p.upload).count(),
            artifacts: placed.iter().filter(|p| !p.upload).count(),
        },
        session_ids,
        message_ids,
    })
}

/// §6 step 3's last part: `projects/<name>/` removed, under
/// [`projects::lock`] so no `register` races it — every entry but
/// `project.json`, then `project.json`, then the folder, so a removal that
/// fails part-way leaves the project registered and a second delete
/// finishes it (R3). The folder is reached by
/// name under `projects_dir()`, never through `project.dir` (canonical, so
/// through a link it would be the link's target): `projects/` and the
/// folder must both be real directories, and the folder must still record
/// this project's id. `Ok(false)` when it is already gone.
pub fn remove_project_folder(project: &Project) -> Result<bool> {
    let _held = projects::lock();
    let projects_dir = projects::projects_dir()?;
    for path in [projects_dir.clone(), projects_dir.join(&project.name)] {
        match fs::symlink_metadata(&path) {
            Ok(meta) if meta.file_type().is_dir() => {}
            Ok(_) => bail!(
                "{} is not a real directory (a link or a file); it was left in place",
                path.display()
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e) => {
                return Err(e).with_context(|| format!("failed to stat {}", path.display()));
            }
        }
    }
    let folder = projects_dir.join(&project.name);
    match projects::read_project_dir(&folder)? {
        Some(found) if found.id == project.id => {}
        _ => bail!(
            "{} no longer records project {}; it was left in place",
            folder.display(),
            project.id
        ),
    }
    // `project.json` last (R3): a removal that stops part-way — a file that
    // cannot be unlinked, a permission changed — then leaves a folder that
    // still records the project, so it stays listed and deleting its data
    // again finishes, instead of an orphan no verb lists or removes holding
    // the plain name.
    let remove = |what: &Path| -> Result<()> {
        // `symlink_metadata` is the entry's own: a link inside the folder
        // is unlinked, never followed.
        let removed = match fs::symlink_metadata(what) {
            Ok(meta) if meta.file_type().is_dir() => fs::remove_dir_all(what),
            Ok(_) => fs::remove_file(what),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        };
        removed.with_context(|| format!("failed to remove {}", what.display()))
    };
    let entries =
        fs::read_dir(&folder).with_context(|| format!("failed to read {}", folder.display()))?;
    for entry in entries {
        let entry = entry.with_context(|| format!("failed to read {}", folder.display()))?;
        if entry.file_name() != projects::PROJECT_FILE {
            remove(&entry.path())?;
        }
    }
    remove(&folder.join(projects::PROJECT_FILE))?;
    fs::remove_dir(&folder).with_context(|| format!("failed to remove {}", folder.display()))?;
    fsync_dir(&projects_dir);
    Ok(true)
}

#[cfg(test)]
mod tests;

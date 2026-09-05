//! `ArtifactStore` — the one writer for produced artifact rows (plan §4.8).
//!
//! `file_assets` is one table with two writers by `origin`:
//!
//! - [`crate::UploadStore`] is the only thing that writes **upload** bytes and
//!   upload rows (`origin` defaulted to `'upload'` by migration 036);
//!   [`crate::FileAssetRepository`] remains their read/CRUD surface, untouched.
//! - `ArtifactStore` is the only thing in the workspace that ever writes
//!   `origin = 'produced'`, and the only thing that writes `artifact_versions`.
//!
//! The address of record is `project_root` + `rel_path` (§4.3); `storage_path`
//! stays the resolved absolute path so the existing content routes need no
//! change. Bytes are placed by the §4.2 grammar
//! (`crate::store::{run_dir, loose_dir, artifact_file_name, version_file_path}`) —
//! this module never joins a literal directory name onto a store root.
//!
//! ## Write protocol (§4.2), as implemented by [`ArtifactStore::put`]
//!
//! ```text
//! 1. write  <dir>/.<stem>.tmp
//! 2. fsync  the tmp file
//! 3. rename <dir>/<NN-stem.ext>  ->  <dir>/.versions/<NN-stem>/v<N-1>.<ext>   (supersede only)
//! 4. rename <dir>/.<stem>.tmp    ->  <dir>/<NN-stem.ext>
//! ```
//!
//! All four steps run inside one `with_connection` transaction, which is also
//! what serialises concurrent `put`s (the `Database` mutex is held for the whole
//! call). The database statements run *after* the bytes are in place and the
//! transaction commits last, so a failure anywhere rolls the row back to the
//! version whose bytes are still the ones a reader would find.
//!
//! A crash between steps 3 and 4 leaves the head path momentarily absent with
//! the previous version's bytes intact under `.versions/` — the invariant the
//! protocol buys is that no reader ever sees a *truncated* file, and that the
//! old bytes are never destroyed before the new ones are complete on disk.
//!
//! **Two additions to the plan's literal protocol.** Every failure path removes
//! `.<stem>.tmp`, so an abandoned write strands nothing in the run directory;
//! and each rename is followed by an `fsync` of the directory it changed, since
//! a rename is directory metadata and step 3's data `fsync` does not force it
//! (without this the two renames could persist out of order under power loss).
//! Both are best-effort: neither failing turns a completed write into an error.
//!
//! ## Recovering from an interrupted put
//!
//! The two file-system questions `put` asks before rotating, and what it does:
//!
//! | head file | `.versions/<stem>/v<N-1>.<ext>` | Action |
//! |---|---|---|
//! | present | absent  | Healthy store — rotate the head into `v<N-1>` (the ordinary supersede). |
//! | present | present | An **interrupted put**: the head holds bytes no committed row describes, `v<N-1>` is the committed previous version. Do **not** rotate — step 4's rename discards the orphaned head, and `v<N-1>` is reported as the rotated path. |
//! | absent  | present | A put died between steps 3 and 4. The bytes are already where they belong; report them so v(N-1)'s row stops claiming the head path. |
//! | absent  | absent  | The head was removed outside the store. Nothing to rotate. |
//!
//! The discriminator is exact rather than heuristic: in a healthy store
//! v(N-1)'s bytes *are* the head until the rotate moves them, so
//! `.versions/<stem>/v<N-1>.<ext>` cannot exist while the head does. Testing the
//! head first would let `fs::rename` — which silently replaces its destination —
//! overwrite a committed version with uncommitted bytes.
//!
//! The interrupted state is not exclusive to power loss: any failure after the
//! bytes land and before `tx.commit()` (an FK violation on `task_id`, a unique
//! index conflict, a disk-full `INSERT`) leaves exactly the same thing on disk.

use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use rusqlite::Connection;

use crate::Database;
use crate::content_io::{fsync_dir, remove_best_effort, sha256_hex};
use crate::models::file_asset::FileAssetStatus;
use crate::models::{ArtifactKind, ArtifactOrigin};
use crate::store::{
    ContentKind, StoreScope, artifact_extension, artifact_file_name, confine_to_root, content_dir,
    loose_dir, run_dir, version_file_path,
};

/// `[execution.artifacts] max_versions_per_artifact` (plan §4.6). The config key
/// itself lands with the `artifact_write` tool; until then every caller passes
/// its own value through [`NewArtifact::max_versions`] and `None` means this.
pub const DEFAULT_MAX_VERSIONS_PER_ARTIFACT: u32 = 20;

/// [`ArtifactQuery::limit`]'s default page size when the caller leaves it unset.
pub const DEFAULT_LIST_LIMIT: i64 = 50;

// ============================================================================
// Errors
// ============================================================================

/// The typed failures callers branch on. Returned inside `anyhow::Error`, so a
/// route recovers them with `err.downcast_ref::<ArtifactError>()` and maps
/// [`ArtifactError::code`] onto the status codes of §4.9.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtifactError {
    /// No such artifact id (or it is not visible to this owner).
    NotFound { id: String },
    /// The row exists but its bytes are gone — §4.9's **410** `ARTIFACT_GONE`.
    Gone { id: String, path: String },
    /// The artifact exists but has no such version.
    VersionNotFound { id: String, version: u32 },
    /// §4.9: `kind ∈ {image, binary}` → **409** `NOT_DIFFABLE`.
    NotDiffable { id: String, kind: &'static str },
    /// A text artifact that *is* diffable, but the unified patch needs the
    /// `similar` crate, which Phase 3 adds. Typed rather than faked so no
    /// caller can mistake an empty patch for "no changes".
    DiffUnavailable { id: String },
}

impl ArtifactError {
    /// The stable error code a route puts in `{error:{code:…}}`.
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotFound { .. } => "ARTIFACT_NOT_FOUND",
            Self::Gone { .. } => "ARTIFACT_GONE",
            Self::VersionNotFound { .. } => "ARTIFACT_VERSION_NOT_FOUND",
            Self::NotDiffable { .. } => "NOT_DIFFABLE",
            Self::DiffUnavailable { .. } => "DIFF_UNAVAILABLE",
        }
    }
}

impl fmt::Display for ArtifactError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound { id } => write!(f, "artifact {id} not found"),
            Self::Gone { id, path } => {
                write!(f, "artifact {id} is gone: {path} no longer exists")
            }
            Self::VersionNotFound { id, version } => {
                write!(f, "artifact {id} has no version {version}")
            }
            Self::NotDiffable { id, kind } => {
                write!(f, "artifact {id} of kind {kind} is not diffable")
            }
            Self::DiffUnavailable { id } => write!(
                f,
                "a unified diff for artifact {id} is not available in this build"
            ),
        }
    }
}

impl std::error::Error for ArtifactError {}

// ============================================================================
// Types
// ============================================================================

/// One produced-artifact write. `title` is the human name the grammar
/// slugifies; the `NN-` sequence prefix, the extension and the placement are
/// all derived, never supplied.
#[derive(Debug, Clone)]
pub struct NewArtifact<'a> {
    /// `file_assets.owner_id` — every read is owner-scoped.
    pub owner_id: &'a str,
    /// Which store the bytes land in. `StoreScope::Home` is the no-project
    /// fallback (§4.1); the daemon CWD is never used.
    pub scope: &'a StoreScope,
    pub kind: ArtifactKind,
    /// The artifact's human name; slugified into the head file name.
    pub title: &'a str,
    /// The bytes to write.
    pub content: &'a [u8],
    /// `file_assets.mime_type` (NOT NULL). Defaults per kind when absent.
    pub mime_type: Option<&'a str>,
    /// A model-supplied file name whose extension may win — kind-constrained
    /// by `artifact_extension` (R21).
    pub name_hint: Option<&'a str>,
    /// Attribution. `Some` places the bytes in the task's `run_dir`; `None`
    /// places them in `loose/<date>` (§4.1).
    pub task_id: Option<&'a str>,
    /// The task's title, for the run directory's slug.
    pub task_title: Option<&'a str>,
    pub agent_id: Option<&'a str>,
    pub agent_template_id: Option<&'a str>,
    /// `file_assets.summary` — "+41 −6" / "exit 0 · 1.4s" / "3 rows".
    pub summary: Option<&'a str>,
    /// `artifact_versions.note` — the model-authored "why this version".
    pub note: Option<&'a str>,
    /// `file_assets.metadata_json`.
    pub metadata_json: Option<&'a str>,
    /// The timestamp the run/loose directory's date comes from.
    pub created: DateTime<Utc>,
    /// `max_versions_per_artifact`; `None` = [`DEFAULT_MAX_VERSIONS_PER_ARTIFACT`].
    pub max_versions: Option<u32>,
}

impl<'a> NewArtifact<'a> {
    /// The five load-bearing fields; everything else defaults to absent, with
    /// `created` at `Utc::now()`.
    pub fn new(
        owner_id: &'a str,
        scope: &'a StoreScope,
        kind: ArtifactKind,
        title: &'a str,
        content: &'a [u8],
    ) -> Self {
        Self {
            owner_id,
            scope,
            kind,
            title,
            content,
            mime_type: None,
            name_hint: None,
            task_id: None,
            task_title: None,
            agent_id: None,
            agent_template_id: None,
            summary: None,
            note: None,
            metadata_json: None,
            created: Utc::now(),
            max_versions: None,
        }
    }
}

/// One `file_assets` row as the artifact surface reads it. Faithful to the
/// table: the client-facing `Artifact` of §4.9 is this plus `task_title`, which
/// lives on `task` and is joined at the route layer.
#[derive(Debug, Clone)]
pub struct ArtifactRecord {
    pub id: String,
    pub owner_id: String,
    pub origin: ArtifactOrigin,
    /// `NULL` for legacy uploads that predate 036, and for any stored spelling
    /// this build does not know.
    pub kind: Option<ArtifactKind>,
    /// `file_assets.filename` — the head file's own name (`01-notes.md`).
    pub name: String,
    pub mime_type: String,
    pub size_bytes: i64,
    pub sha256: String,
    /// The resolved absolute path of the head file.
    pub storage_path: String,
    /// `NULL` => the home store (§4.8: the home root *is* the baseline).
    pub project_root: Option<String>,
    /// The head's path relative to `<store>/artifacts`.
    pub rel_path: Option<String>,
    pub task_id: Option<String>,
    pub agent_id: Option<String>,
    pub agent_template_id: Option<String>,
    pub version: u32,
    pub version_count: u32,
    pub pinned: bool,
    pub summary: Option<String>,
    pub metadata_json: Option<String>,
    /// Set the first time the bytes were found absent; cleared by the next
    /// successful `put`.
    pub missing_since: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl ArtifactRecord {
    /// §4.8: list rows carry `missing: true`.
    pub fn missing(&self) -> bool {
        self.missing_since.is_some()
    }
}

/// Filters for [`ArtifactStore::list`] — the query string of §4.9.
#[derive(Debug, Clone)]
pub struct ArtifactQuery {
    /// Required: every list is owner-scoped.
    pub owner_id: String,
    pub task_id: Option<String>,
    pub kind: Option<ArtifactKind>,
    pub origin: Option<ArtifactOrigin>,
    /// Matched as `COALESCE(project_root, '')`, the form the partial unique
    /// index is built on — so `Some(String::new())` selects the home store.
    pub project_root: Option<String>,
    pub pinned: Option<bool>,
    /// Substring match over the head file name and the summary.
    pub q: Option<String>,
    /// §4.8: default `false`.
    pub include_missing: bool,
    /// `None` = [`DEFAULT_LIST_LIMIT`].
    pub limit: Option<i64>,
    pub offset: i64,
}

impl ArtifactQuery {
    pub fn new(owner_id: impl Into<String>) -> Self {
        Self {
            owner_id: owner_id.into(),
            task_id: None,
            kind: None,
            origin: None,
            project_root: None,
            pinned: None,
            q: None,
            include_missing: false,
            limit: None,
            offset: 0,
        }
    }
}

/// One `artifact_versions` row. Field-for-field the `ArtifactVersion` of
/// `apps/openalpaca-gui/src/lib/api/unbacked.ts:62-70`, plus the stored
/// `rel_path` the content route resolves.
#[derive(Debug, Clone)]
pub struct ArtifactVersionRow {
    pub artifact_id: String,
    pub version: u32,
    /// `.versions/<stem>/v1.md` for a superseded version; equal to the head's
    /// `rel_path` for the current one.
    pub rel_path: String,
    pub sha256: String,
    pub size_bytes: i64,
    pub note: Option<String>,
    /// `NULL` => a human edited the file by hand.
    pub author_agent_id: Option<String>,
    /// `NULL` on v1.
    pub added_lines: Option<i64>,
    pub removed_lines: Option<i64>,
    pub created_at: String,
}

/// Field-for-field the `ArtifactDiff` of `unbacked.ts:72-79`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactDiff {
    pub from: u32,
    pub to: u32,
    pub added_lines: i64,
    pub removed_lines: i64,
    /// Always `"unified"`.
    pub format: &'static str,
    pub patch: String,
}

// ============================================================================
// Crash injection (the §4.2 Verify item)
// ============================================================================

/// The four steps of the write protocol, named so a test can abort the write
/// *between* any pair of them and inspect what a reader would find. In a
/// non-test build [`crash_point`] compiles to nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WriteStep {
    TmpWritten,
    Fsynced,
    Rotated,
    HeadRenamed,
}

#[cfg(test)]
thread_local! {
    static CRASH_AT: std::cell::Cell<Option<WriteStep>> = const { std::cell::Cell::new(None) };
}

/// Abort the next `put` on this thread immediately after `step`.
#[cfg(test)]
fn crash_after(step: WriteStep) {
    CRASH_AT.with(|c| c.set(Some(step)));
}

#[cfg(test)]
fn clear_crash() {
    CRASH_AT.with(|c| c.set(None));
}

#[cfg(test)]
fn crash_point(step: WriteStep) -> Result<()> {
    if CRASH_AT.with(|c| c.get()) == Some(step) {
        bail!("simulated crash after {step:?}");
    }
    Ok(())
}

#[cfg(not(test))]
#[inline(always)]
fn crash_point(_step: WriteStep) -> Result<()> {
    Ok(())
}

// ============================================================================
// Store
// ============================================================================

/// The one writer for produced artifact rows.
pub struct ArtifactStore<'a> {
    db: &'a Database,
}

impl<'a> ArtifactStore<'a> {
    pub fn new(db: &'a Database) -> Self {
        Self { db }
    }

    /// Create or supersede. Returns the head record and `true` when this call
    /// *created* the artifact (v1), `false` when it superseded an existing one.
    ///
    /// Identity is the **address**: two puts that resolve to the same directory
    /// and the same `<slug>.<ext>` are the same artifact, and the second one
    /// rotates the first into `.versions/`. A different extension (a different
    /// `kind`, `mime_type` or `name_hint`) is therefore a different artifact.
    ///
    /// Everything below happens inside one `with_connection` call, which holds
    /// the database mutex for its whole duration — that is what serialises
    /// concurrent puts (§4.8, "Concurrent writes").
    pub fn put(&self, new: NewArtifact<'_>) -> Result<(ArtifactRecord, bool)> {
        let max_versions = new
            .max_versions
            .unwrap_or(DEFAULT_MAX_VERSIONS_PER_ARTIFACT)
            .max(1);

        // --- Placement, by the §4.2 grammar. Creates directories only. -------
        let artifacts_root = content_dir(new.scope, ContentKind::Artifacts)?;
        let artifacts_root = artifacts_root.canonicalize().with_context(|| {
            format!(
                "failed to canonicalize the artifacts root: {}",
                artifacts_root.display()
            )
        })?;
        let dir = match new.task_id {
            Some(task_id) => run_dir(
                new.scope,
                new.created,
                new.task_title.unwrap_or_default(),
                task_id,
            )?,
            None => loose_dir(new.scope, new.created)?,
        };
        let rel_dir = relative_to(&artifacts_root, &dir)?;
        let ext = artifact_extension(new.kind, new.mime_type, new.name_hint);
        let project_root = project_root_of(new.scope)?;
        let project_key = project_root.clone().unwrap_or_default();
        let mime = new
            .mime_type
            .unwrap_or_else(|| default_mime(new.kind))
            .to_string();
        let sha256 = sha256_hex(new.content);
        let size_bytes = new.content.len() as i64;

        self.db.with_connection(|conn| {
            let tx = conn.unchecked_transaction()?;

            // --- Identity and the NN- sequence -------------------------------
            let siblings = dir_rows(&tx, &project_key, &rel_dir)?;
            let existing = siblings
                .iter()
                .find(|row| row.file_name == artifact_file_name(row.seq, new.title, &ext));
            if let Some(row) = existing
                && row.owner_id != new.owner_id
            {
                bail!(
                    "artifact {} at {rel_dir}/{} belongs to another owner",
                    row.id,
                    row.file_name
                );
            }
            let seq = match existing {
                Some(row) => row.seq,
                None => siblings.iter().map(|r| r.seq).max().unwrap_or(0) + 1,
            };

            let head_name = artifact_file_name(seq, new.title, &ext);
            let head_path = confine_to_root(&artifacts_root, &dir.join(&head_name))?;
            let head_rel = format!("{rel_dir}/{head_name}");
            let version = existing.map(|r| r.version + 1).unwrap_or(1);

            // Line counts are recorded at write time (§4.9) — read the bytes
            // being superseded before the rotate moves them.
            let previous = existing
                .filter(|_| head_path.exists())
                .and_then(|_| fs::read(&head_path).ok());
            let (added_lines, removed_lines) = match (&previous, is_text_kind(new.kind)) {
                (Some(old), true) => {
                    let (a, r) = line_counts(old, new.content);
                    (Some(a), Some(r))
                }
                _ => (None, None),
            };

            // --- The §4.2 write protocol -------------------------------------
            fs::create_dir_all(&dir)
                .with_context(|| format!("failed to create {}", dir.display()))?;
            let rotated_rel = write_bytes(
                &artifacts_root,
                &dir,
                &head_path,
                &head_name,
                new.content,
                existing.map(|r| r.version),
            )?;

            // --- The rows ----------------------------------------------------
            let id = match existing {
                Some(row) => {
                    tx.execute(
                        "UPDATE file_assets SET
                            sha256 = ?1, filename = ?2, mime_type = ?3, size_bytes = ?4,
                            storage_path = ?5, kind = ?6, task_id = ?7, agent_id = ?8,
                            agent_template_id = ?9, version = ?10, summary = ?11,
                            metadata_json = ?12, missing_since = NULL,
                            updated_at = datetime('now')
                         WHERE id = ?13",
                        rusqlite::params![
                            sha256,
                            head_name,
                            mime,
                            size_bytes,
                            head_path.to_string_lossy(),
                            new.kind.as_str(),
                            new.task_id,
                            new.agent_id,
                            new.agent_template_id,
                            version,
                            new.summary,
                            new.metadata_json,
                            row.id,
                        ],
                    )?;
                    // The version that was the head now lives under `.versions/`.
                    if let Some(rel) = &rotated_rel {
                        tx.execute(
                            "UPDATE artifact_versions SET rel_path = ?1
                             WHERE artifact_id = ?2 AND version = ?3",
                            rusqlite::params![rel, row.id, row.version],
                        )?;
                    }
                    row.id.clone()
                }
                None => {
                    let id = uuid::Uuid::new_v4().to_string();
                    tx.execute(
                        "INSERT INTO file_assets
                            (id, owner_id, sha256, filename, mime_type, size_bytes, storage_path,
                             status, metadata_json, origin, kind, task_id, agent_id,
                             agent_template_id, project_root, rel_path, version, version_count,
                             pinned, summary)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
                                 ?15, ?16, ?17, 1, 0, ?18)",
                        rusqlite::params![
                            id,
                            new.owner_id,
                            sha256,
                            head_name,
                            mime,
                            size_bytes,
                            head_path.to_string_lossy(),
                            // Produced bytes need no extraction pass: they came
                            // out of a context window as text already. `Ready`
                            // keeps them out of `list_by_status(Uploaded)`,
                            // which drives the background extractor.
                            FileAssetStatus::Ready.as_str(),
                            new.metadata_json,
                            ArtifactOrigin::Produced.as_str(),
                            new.kind.as_str(),
                            new.task_id,
                            new.agent_id,
                            new.agent_template_id,
                            project_root,
                            head_rel,
                            version,
                            new.summary,
                        ],
                    )?;
                    id
                }
            };

            tx.execute(
                "INSERT INTO artifact_versions
                    (artifact_id, version, rel_path, sha256, size_bytes, note, author_agent_id,
                     added_lines, removed_lines)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                rusqlite::params![
                    id,
                    version,
                    head_rel,
                    sha256,
                    size_bytes,
                    new.note,
                    new.agent_id,
                    added_lines,
                    removed_lines,
                ],
            )?;

            // --- Prune (head never pruned) -----------------------------------
            let pruned = prune_versions(&tx, &id, max_versions)?;
            for rel in &pruned {
                let path = artifacts_root.join(rel);
                if let Err(e) = fs::remove_file(&path)
                    && e.kind() != std::io::ErrorKind::NotFound
                {
                    // The row is already gone; a stale file is a leak, not a
                    // reason to fail a write the caller cannot retry usefully.
                    tracing::warn!("Failed to prune {}: {e}", path.display());
                }
            }

            let record = load_by_id(&tx, &id)?
                .with_context(|| format!("artifact {id} vanished inside its own transaction"))?;
            tx.commit()?;
            Ok((record, existing.is_none()))
        })
    }

    /// The head record, owner-scoped.
    pub fn get(&self, id: &str, owner_id: &str) -> Result<Option<ArtifactRecord>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(&format!(
                "SELECT {RECORD_COLUMNS} FROM file_assets WHERE id = ?1 AND owner_id = ?2"
            ))?;
            let mut rows = stmt.query(rusqlite::params![id, owner_id])?;
            match rows.next()? {
                Some(row) => Ok(Some(row_to_record(row)?)),
                None => Ok(None),
            }
        })
    }

    /// The Library's page plus the unpaged total. Owner-scoped; missing rows are
    /// hidden unless [`ArtifactQuery::include_missing`] is set (§4.8).
    pub fn list(&self, q: &ArtifactQuery) -> Result<(Vec<ArtifactRecord>, i64)> {
        use rusqlite::types::Value;

        let mut clauses: Vec<&str> = vec!["owner_id = ?"];
        let mut args: Vec<Value> = vec![Value::Text(q.owner_id.clone())];
        if let Some(task_id) = &q.task_id {
            clauses.push("task_id = ?");
            args.push(Value::Text(task_id.clone()));
        }
        if let Some(kind) = q.kind {
            clauses.push("kind = ?");
            args.push(Value::Text(kind.as_str().to_string()));
        }
        if let Some(origin) = q.origin {
            clauses.push("origin = ?");
            args.push(Value::Text(origin.as_str().to_string()));
        }
        if let Some(root) = &q.project_root {
            // The form the partial unique index is built on, so `''` selects
            // the home store rather than scanning for `project_root IS NULL`.
            clauses.push("COALESCE(project_root, '') = ?");
            args.push(Value::Text(root.clone()));
        }
        if let Some(pinned) = q.pinned {
            clauses.push("pinned = ?");
            args.push(Value::Integer(i64::from(pinned)));
        }
        if let Some(text) = &q.q {
            clauses.push("(filename LIKE ? ESCAPE '\\' OR IFNULL(summary, '') LIKE ? ESCAPE '\\')");
            let pattern = format!("%{}%", escape_like(text));
            args.push(Value::Text(pattern.clone()));
            args.push(Value::Text(pattern));
        }
        if !q.include_missing {
            clauses.push("missing_since IS NULL");
        }
        let where_sql = clauses.join(" AND ");

        self.db.with_connection(|conn| {
            let total: i64 = conn.query_row(
                &format!("SELECT COUNT(*) FROM file_assets WHERE {where_sql}"),
                rusqlite::params_from_iter(args.iter()),
                |row| row.get(0),
            )?;

            let mut page = args.clone();
            page.push(Value::Integer(q.limit.unwrap_or(DEFAULT_LIST_LIMIT)));
            page.push(Value::Integer(q.offset));
            let mut stmt = conn.prepare(&format!(
                "SELECT {RECORD_COLUMNS} FROM file_assets WHERE {where_sql}
                 ORDER BY created_at DESC, id DESC LIMIT ? OFFSET ?"
            ))?;
            let mut rows = stmt.query(rusqlite::params_from_iter(page.iter()))?;
            let mut records = Vec::new();
            while let Some(row) = rows.next()? {
                records.push(row_to_record(row)?);
            }
            Ok((records, total))
        })
    }

    /// The absolute path of an artifact's bytes — the head when `version` is
    /// `None` or names the current version, otherwise the `.versions/` file.
    ///
    /// Stats before returning (§4.8): an absent head stamps `missing_since` and
    /// returns [`ArtifactError::Gone`], which the route renders as **410**. An
    /// absent *older* version is equally `Gone` but does not mark the row —
    /// `missing` describes the head, which may be perfectly fine.
    pub fn resolve_content(&self, id: &str, version: Option<u32>) -> Result<PathBuf> {
        use rusqlite::OptionalExtension;

        self.db.with_connection(|conn| {
            let record = load_by_id(conn, id)?.ok_or_else(|| not_found(id))?;
            let head_rel = record
                .rel_path
                .clone()
                .with_context(|| format!("artifact {id} has no rel_path"))?;
            let (rel, is_head) = match version {
                None => (head_rel, true),
                Some(v) if v == record.version => (head_rel, true),
                Some(v) => {
                    let rel: Option<String> = conn
                        .query_row(
                            "SELECT rel_path FROM artifact_versions
                             WHERE artifact_id = ?1 AND version = ?2",
                            rusqlite::params![id, v],
                            |row| row.get(0),
                        )
                        .optional()?;
                    let rel = rel.ok_or_else(|| {
                        anyhow::Error::new(ArtifactError::VersionNotFound {
                            id: id.to_string(),
                            version: v,
                        })
                    })?;
                    (rel, false)
                }
            };

            let root = artifacts_root_for(&record)?;
            let candidate = root.join(&rel);
            // Belt and braces (the rel_path came out of our own grammar). A
            // deleted project has no root to canonicalize against — that is a
            // gone artifact, not a confinement failure.
            let path = match confine_to_root(&root, &candidate) {
                Ok(path) => path,
                Err(_) if !root.exists() => candidate,
                Err(e) => return Err(e),
            };

            if !path.exists() {
                if is_head && record.missing_since.is_none() {
                    conn.execute(
                        "UPDATE file_assets SET missing_since = datetime('now'),
                            updated_at = datetime('now') WHERE id = ?1",
                        rusqlite::params![id],
                    )?;
                }
                return Err(anyhow::Error::new(ArtifactError::Gone {
                    id: id.to_string(),
                    path: path.to_string_lossy().to_string(),
                }));
            }
            Ok(path)
        })
    }

    /// Every retained version, newest first.
    pub fn versions(&self, id: &str) -> Result<Vec<ArtifactVersionRow>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT artifact_id, version, rel_path, sha256, size_bytes, note,
                        author_agent_id, added_lines, removed_lines, created_at
                 FROM artifact_versions WHERE artifact_id = ?1 ORDER BY version DESC",
            )?;
            let mut rows = stmt.query(rusqlite::params![id])?;
            let mut out = Vec::new();
            while let Some(row) = rows.next()? {
                out.push(ArtifactVersionRow {
                    artifact_id: row.get(0)?,
                    version: row.get::<_, i64>(1)? as u32,
                    rel_path: row.get(2)?,
                    sha256: row.get(3)?,
                    size_bytes: row.get(4)?,
                    note: row.get(5)?,
                    author_agent_id: row.get(6)?,
                    added_lines: row.get(7)?,
                    removed_lines: row.get(8)?,
                    created_at: row.get(9)?,
                });
            }
            Ok(out)
        })
    }

    /// Validates a diff request and, for now, refuses it in a typed way.
    ///
    /// `kind ∈ {image, binary}` is [`ArtifactError::NotDiffable`] (§4.9's 409)
    /// and that answer is final. Every other kind is genuinely diffable, but
    /// the unified patch needs the `similar` crate that Phase 3 adds, so this
    /// build answers [`ArtifactError::DiffUnavailable`] rather than returning an
    /// [`ArtifactDiff`] with an empty `patch` that a client would render as
    /// "no changes". Phase 3 replaces only the final arm.
    pub fn diff(&self, id: &str, from: u32, to: u32) -> Result<ArtifactDiff> {
        self.db.with_connection(|conn| {
            let record = load_by_id(conn, id)?.ok_or_else(|| not_found(id))?;
            if let Some(kind) = record.kind
                && !is_text_kind(kind)
            {
                return Err(anyhow::Error::new(ArtifactError::NotDiffable {
                    id: id.to_string(),
                    kind: kind.as_str(),
                }));
            }
            for version in [from, to] {
                let present: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM artifact_versions
                     WHERE artifact_id = ?1 AND version = ?2",
                    rusqlite::params![id, version],
                    |row| row.get(0),
                )?;
                if present == 0 {
                    return Err(anyhow::Error::new(ArtifactError::VersionNotFound {
                        id: id.to_string(),
                        version,
                    }));
                }
            }
            Err(anyhow::Error::new(ArtifactError::DiffUnavailable {
                id: id.to_string(),
            }))
        })
    }

    /// GAP-12's pin. Pinned uploads also survive the orphan sweep (§4.5).
    pub fn set_pinned(&self, id: &str, pinned: bool) -> Result<()> {
        self.db.with_connection(|conn| {
            let changed = conn.execute(
                "UPDATE file_assets SET pinned = ?1, updated_at = datetime('now') WHERE id = ?2",
                rusqlite::params![i64::from(pinned), id],
            )?;
            if changed == 0 {
                return Err(not_found(id));
            }
            Ok(())
        })
    }

    /// Re-base every row addressed under `old_root` onto `new_root` — §4.8's
    /// "Project moved", in one statement.
    ///
    /// `rel_path` is deliberately untouched: it is the address, and it did not
    /// change. Only `project_root` and the resolved `storage_path` prefix move.
    /// Widening the same transaction to `session.workspace_id`,
    /// `task.workspace_id` and the memory scope key is Phase 8 item 11.
    pub fn rebase_project(&self, old_root: &str, new_root: &str) -> Result<usize> {
        self.db.with_connection(|conn| {
            let changed = conn.execute(
                "UPDATE file_assets
                    SET project_root = ?2,
                        storage_path = CASE
                            WHEN substr(storage_path, 1, length(?1)) = ?1
                            THEN ?2 || substr(storage_path, length(?1) + 1)
                            ELSE storage_path END,
                        updated_at = datetime('now')
                  WHERE project_root = ?1",
                rusqlite::params![old_root, new_root],
            )?;
            Ok(changed)
        })
    }

    /// Re-stat every produced row under `project_root` (`Some("")` = the home
    /// store, `None` = every root), stamping `missing_since` on those whose
    /// bytes are gone.
    ///
    /// Returns how many rows are missing — not how many this pass newly marked
    /// — so a status caller gets the same answer every time it asks.
    pub fn verify(&self, project_root: Option<&str>) -> Result<usize> {
        self.db.with_connection(|conn| {
            let tx = conn.unchecked_transaction()?;
            let mut sql = String::from(
                "SELECT id, storage_path, missing_since FROM file_assets WHERE origin = 'produced'",
            );
            if project_root.is_some() {
                sql.push_str(" AND COALESCE(project_root, '') = ?1");
            }
            let rows: Vec<(String, String, Option<String>)> = {
                let mut stmt = tx.prepare(&sql)?;
                let mapped = match project_root {
                    Some(root) => stmt.query(rusqlite::params![root])?,
                    None => stmt.query([])?,
                };
                let mut mapped = mapped;
                let mut out = Vec::new();
                while let Some(row) = mapped.next()? {
                    out.push((row.get(0)?, row.get(1)?, row.get(2)?));
                }
                out
            };

            let mut missing = 0usize;
            for (id, storage_path, missing_since) in rows {
                if Path::new(&storage_path).exists() {
                    continue;
                }
                missing += 1;
                if missing_since.is_none() {
                    tx.execute(
                        "UPDATE file_assets SET missing_since = datetime('now'),
                            updated_at = datetime('now') WHERE id = ?1",
                        rusqlite::params![id],
                    )?;
                }
            }
            tx.commit()?;
            Ok(missing)
        })
    }
}

// ============================================================================
// Internals
// ============================================================================

/// Every `file_assets` column [`ArtifactRecord`] reads, in [`row_to_record`]'s
/// order. Named explicitly so a later migration cannot shift the indexes.
const RECORD_COLUMNS: &str = "id, owner_id, origin, kind, filename, mime_type, size_bytes, \
     sha256, storage_path, project_root, rel_path, task_id, agent_id, agent_template_id, \
     version, version_count, pinned, summary, metadata_json, missing_since, created_at, updated_at";

fn row_to_record(row: &rusqlite::Row<'_>) -> Result<ArtifactRecord> {
    let origin: String = row.get(2)?;
    let kind: Option<String> = row.get(3)?;
    Ok(ArtifactRecord {
        id: row.get(0)?,
        owner_id: row.get(1)?,
        origin: ArtifactOrigin::parse(&origin),
        kind: kind.as_deref().and_then(ArtifactKind::parse),
        name: row.get(4)?,
        mime_type: row.get(5)?,
        size_bytes: row.get(6)?,
        sha256: row.get(7)?,
        storage_path: row.get(8)?,
        project_root: row.get(9)?,
        rel_path: row.get(10)?,
        task_id: row.get(11)?,
        agent_id: row.get(12)?,
        agent_template_id: row.get(13)?,
        version: row.get::<_, i64>(14)? as u32,
        version_count: row.get::<_, i64>(15)? as u32,
        pinned: row.get::<_, i64>(16)? != 0,
        summary: row.get(17)?,
        metadata_json: row.get(18)?,
        missing_since: row.get(19)?,
        created_at: row.get(20)?,
        updated_at: row.get(21)?,
    })
}

fn load_by_id(conn: &Connection, id: &str) -> Result<Option<ArtifactRecord>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {RECORD_COLUMNS} FROM file_assets WHERE id = ?1"
    ))?;
    let mut rows = stmt.query(rusqlite::params![id])?;
    match rows.next()? {
        Some(row) => Ok(Some(row_to_record(row)?)),
        None => Ok(None),
    }
}

fn not_found(id: &str) -> anyhow::Error {
    anyhow::Error::new(ArtifactError::NotFound { id: id.to_string() })
}

/// One produced head sitting directly in a run/loose directory.
struct DirRow {
    id: String,
    owner_id: String,
    file_name: String,
    seq: u32,
    version: u32,
}

/// Every produced head addressed directly under `rel_dir` — the scan that both
/// identifies the artifact being superseded and yields the next `NN-` sequence.
fn dir_rows(conn: &Connection, project_key: &str, rel_dir: &str) -> Result<Vec<DirRow>> {
    let prefix = format!("{rel_dir}/");
    let mut stmt = conn.prepare(
        "SELECT id, owner_id, rel_path, version FROM file_assets
         WHERE origin = 'produced' AND COALESCE(project_root, '') = ?1
           AND rel_path IS NOT NULL AND rel_path LIKE ?2 ESCAPE '\\'",
    )?;
    let mut rows = stmt.query(rusqlite::params![
        project_key,
        format!("{}%", escape_like(&prefix))
    ])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        let rel_path: String = row.get(2)?;
        // LIKE is a coarse filter; the exact test is a direct child of the dir.
        let Some(file_name) = rel_path.strip_prefix(&prefix) else {
            continue;
        };
        if file_name.contains('/') {
            continue;
        }
        let Some(seq) = leading_sequence(file_name) else {
            continue;
        };
        out.push(DirRow {
            id: row.get(0)?,
            owner_id: row.get(1)?,
            file_name: file_name.to_string(),
            seq,
            version: row.get::<_, i64>(3)? as u32,
        });
    }
    Ok(out)
}

/// The `NN` of `NN-<slug>.<ext>`.
fn leading_sequence(file_name: &str) -> Option<u32> {
    let digits: String = file_name.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return None;
    }
    digits.parse().ok()
}

/// The §4.2 write protocol. Returns the `rel_path` under `.versions/` that the
/// previous head now lives at, when there was a previous head — whether this
/// call rotated it there or an interrupted earlier call already had.
///
/// Steps, in order, with a crash point after each: write `.<stem>.tmp` → fsync
/// → rename the current head into `.versions/<stem>/v<N-1>.<ext>` → rename the
/// tmp file onto the head path. Every failure path removes the tmp file, so a
/// write that never completes strands nothing in the run directory.
fn write_bytes(
    artifacts_root: &Path,
    dir: &Path,
    head_path: &Path,
    head_name: &str,
    content: &[u8],
    previous_version: Option<u32>,
) -> Result<Option<String>> {
    let stem = Path::new(head_name)
        .file_stem()
        .and_then(|s| s.to_str())
        .with_context(|| format!("head file name has no stem: {head_name}"))?;
    let tmp = confine_to_root(artifacts_root, &dir.join(format!(".{stem}.tmp")))?;

    let outcome = write_bytes_steps(
        artifacts_root,
        dir,
        head_path,
        &tmp,
        content,
        previous_version,
    );
    if outcome.is_err() {
        // A `.<stem>.tmp` left behind by a failed write would outlive everything
        // that could explain it: the artifact may never be written again, and
        // only a retry of this exact address would truncate it.
        remove_best_effort(&tmp);
    }
    outcome
}

/// The four steps themselves. Separated from [`write_bytes`] only so the tmp
/// file has exactly one cleanup site.
fn write_bytes_steps(
    artifacts_root: &Path,
    dir: &Path,
    head_path: &Path,
    tmp: &Path,
    content: &[u8],
    previous_version: Option<u32>,
) -> Result<Option<String>> {
    let mut file =
        fs::File::create(tmp).with_context(|| format!("failed to create {}", tmp.display()))?;
    file.write_all(content)
        .with_context(|| format!("failed to write {}", tmp.display()))?;
    crash_point(WriteStep::TmpWritten)?;
    file.sync_all()
        .with_context(|| format!("failed to fsync {}", tmp.display()))?;
    drop(file);
    crash_point(WriteStep::Fsynced)?;

    let mut rotated_rel = None;
    if let Some(previous) = previous_version {
        let version_path =
            confine_to_root(artifacts_root, &version_file_path(head_path, previous)?)?;
        // The two-question recovery table (see the module docs). `version_path`
        // is asked *first*: in a healthy store v(N-1)'s bytes are the head until
        // the rotate moves them, so an existing `v<N-1>.<ext>` can only be the
        // committed previous version left there by an interrupted put — and
        // `fs::rename` would silently replace it.
        if version_path.exists() {
            if head_path.exists() {
                // Interrupted put: the head holds bytes no committed row
                // describes. Rotating them here would destroy the only copy of
                // v(N-1) and leave its row pointing at another version's bytes.
                // The head is discarded by step 4's rename below.
                tracing::warn!(
                    "Discarding an uncommitted head at {}: {} already holds the committed \
                     version {previous} (an earlier put did not commit)",
                    head_path.display(),
                    version_path.display()
                );
            }
            // Either way the previous version's bytes are already where they
            // belong — report the real location so its row stops claiming the
            // head path.
            rotated_rel = Some(relative_to(artifacts_root, &version_path)?);
        } else if head_path.exists() {
            if let Some(parent) = version_path.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("failed to create {}", parent.display()))?;
            }
            fs::rename(head_path, &version_path).with_context(|| {
                format!(
                    "failed to rotate {} into {}",
                    head_path.display(),
                    version_path.display()
                )
            })?;
            if let Some(parent) = version_path.parent() {
                fsync_dir(parent);
            }
            fsync_dir(dir);
            rotated_rel = Some(relative_to(artifacts_root, &version_path)?);
        }
    }
    crash_point(WriteStep::Rotated)?;

    fs::rename(tmp, head_path).with_context(|| {
        format!(
            "failed to move {} into place at {}",
            tmp.display(),
            head_path.display()
        )
    })?;
    fsync_dir(dir);
    crash_point(WriteStep::HeadRenamed)?;

    Ok(rotated_rel)
}

/// Deletes the oldest version rows beyond `max_versions`, returning their
/// `rel_path`s so the caller can delete the files. The head has the highest
/// version and `max_versions >= 1`, so it is never a candidate.
fn prune_versions(conn: &Connection, id: &str, max_versions: u32) -> Result<Vec<String>> {
    let all: Vec<(u32, String)> = {
        let mut stmt = conn.prepare(
            "SELECT version, rel_path FROM artifact_versions
             WHERE artifact_id = ?1 ORDER BY version DESC",
        )?;
        let mut rows = stmt.query(rusqlite::params![id])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push((row.get::<_, i64>(0)? as u32, row.get(1)?));
        }
        out
    };

    let keep = max_versions as usize;
    let mut pruned = Vec::new();
    for (version, rel_path) in all.iter().skip(keep) {
        conn.execute(
            "DELETE FROM artifact_versions WHERE artifact_id = ?1 AND version = ?2",
            rusqlite::params![id, version],
        )?;
        pruned.push(rel_path.clone());
    }
    conn.execute(
        "UPDATE file_assets SET version_count = ?1 WHERE id = ?2",
        rusqlite::params![all.len().min(keep) as i64, id],
    )?;
    Ok(pruned)
}

/// `<store>/artifacts` for a record, recovered from its own two address
/// columns so a changed `OPENALPACA_HOME_STORE` cannot silently re-point a row.
fn artifacts_root_for(record: &ArtifactRecord) -> Result<PathBuf> {
    if let Some(rel) = record.rel_path.as_deref()
        && let Some(prefix) = record.storage_path.strip_suffix(rel)
    {
        let trimmed = prefix.trim_end_matches('/');
        if !trimmed.is_empty() {
            return Ok(PathBuf::from(trimmed));
        }
    }
    let scope = match &record.project_root {
        Some(root) => StoreScope::Project(PathBuf::from(root)),
        None => StoreScope::Home,
    };
    content_dir(&scope, ContentKind::Artifacts)
}

/// `path` relative to `root`, with `/` separators — the `rel_path` column.
fn relative_to(root: &Path, path: &Path) -> Result<String> {
    let rel = path
        .strip_prefix(root)
        .with_context(|| format!("{} is not under {}", path.display(), root.display()))?;
    Ok(rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/"))
}

/// The canonical-path string §4.8 requires of every `project_root`; `None` is
/// the home store, which is the address baseline.
fn project_root_of(scope: &StoreScope) -> Result<Option<String>> {
    match scope {
        StoreScope::Home => Ok(None),
        StoreScope::Project(root) => {
            let canonical = root.canonicalize().unwrap_or_else(|_| root.clone());
            Ok(Some(canonical.to_string_lossy().to_string()))
        }
    }
}

/// `file_assets.mime_type` is NOT NULL, so a caller that supplies none gets the
/// kind's own default. Deliberately coarse: the extension, not the MIME type,
/// is what the grammar and the client key off.
fn default_mime(kind: ArtifactKind) -> &'static str {
    match kind {
        ArtifactKind::Markdown | ArtifactKind::Plan => "text/markdown",
        ArtifactKind::Code | ArtifactKind::Terminal => "text/plain",
        ArtifactKind::Table => "text/csv",
        ArtifactKind::Html => "text/html",
        ArtifactKind::Image | ArtifactKind::Binary => "application/octet-stream",
    }
}

/// §4.9: diffs are text-only — `kind ∈ {image, binary}` is not.
fn is_text_kind(kind: ArtifactKind) -> bool {
    !matches!(kind, ArtifactKind::Image | ArtifactKind::Binary)
}

/// Escapes the `LIKE` metacharacters for a pattern used with `ESCAPE '\'`.
fn escape_like(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        if matches!(ch, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

/// `(added, removed)` between two texts, as a multiset difference of lines.
///
/// Deliberately not an LCS: `added_lines`/`removed_lines` are written at *write*
/// time (§4.9) on the hot path, and a moved line is neither added nor removed
/// under this counting. Phase 3's `similar` replaces it with the real thing.
fn line_counts(old: &[u8], new: &[u8]) -> (i64, i64) {
    fn tally(bytes: &[u8]) -> HashMap<String, i64> {
        let text = String::from_utf8_lossy(bytes);
        let mut counts: HashMap<String, i64> = HashMap::new();
        for line in text.lines() {
            *counts.entry(line.to_string()).or_default() += 1;
        }
        counts
    }
    let before = tally(old);
    let after = tally(new);
    let mut added = 0i64;
    let mut removed = 0i64;
    for (line, count) in &after {
        added += (count - before.get(line).copied().unwrap_or(0)).max(0);
    }
    for (line, count) in &before {
        removed += (count - after.get(line).copied().unwrap_or(0)).max(0);
    }
    (added, removed)
}

#[cfg(test)]
mod tests;

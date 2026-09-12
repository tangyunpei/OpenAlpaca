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
//! this module never joins a literal directory name onto a store root. R24: the
//! `project_root` half of the address comes from the *canonicalized* store root
//! ([`crate::store::project_root_at`]), never from the scope the caller named,
//! so two scopes whose bytes land in one directory address one sequence space.
//!
//! ## Write protocol (§4.2 + R24), as implemented by [`ArtifactStore::put`]
//!
//! ```text
//! 0. create <dir>/<NN-stem.ext>  O_EXCL                                       (create only)
//! 1. write  <dir>/.<stem>.tmp
//! 2. fsync  the tmp file
//! 3. rename <dir>/<NN-stem.ext>  ->  <dir>/.versions/<NN-stem>/v<N-1>.<ext>   (supersede only)
//! 4. rename <dir>/.<stem>.tmp    ->  <dir>/<NN-stem.ext>
//! ```
//!
//! Step 0 is [`HeadReservation`] (R24, the rule T26 gave uploads): a *new*
//! artifact claims its name with `O_EXCL` before a byte is written, because
//! step 4's rename would otherwise replace whatever stands there. An artifact
//! is addressed by name, so unlike an upload there is no sequence to bump: the
//! only two answers are to reclaim the name or to refuse it, and which one
//! applies is decided by the rows (R33 — see below). Superseding an artifact
//! skips step 0: its own row already owns the name, and step 3 moves that file
//! aside itself.
//!
//! All the steps run inside one `with_connection` transaction, which is also
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
//! | present | present | An **interrupted put**: the head holds bytes no committed row describes, `v<N-1>` is the committed previous version. Do **not** rotate — step 4's rename discards the orphaned head, and `v<N-1>` is reported as the rotated path. Ordinarily `put` never reaches this row any more: the hand-edit check below has already recorded the orphaned head as a version of its own, so N has advanced and `v<N>` is absent. It stands for the one case that check declines — an orphan whose bytes happen to hash to what the row already says. |
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
//!
//! ## The head name: reclaim or refuse (R33)
//!
//! **The row is the commit.** Bytes at an address that no `file_assets` and no
//! `artifact_versions` row references are an *uncommitted write*, and belong to
//! nobody — the same rule the `.versions/` recovery above applies. So when a
//! create finds its head name taken ([`reserve_head`]):
//!
//! | What holds the name | Action |
//! |---|---|
//! | A file no row of this store's address space (`project_root` + `rel_path`) references — the head of a create that died between step 4 and `tx.commit()`, or its empty step-0 reservation | Removed (`warn`, with the path and size), the name claimed, the write continues. |
//! | A file some row *does* reference — a concurrent writer, an upload addressing the same `rel_path`, an artifact version | [`ArtifactError::NameTaken`]: this store will not destroy bytes it can account for. |
//! | A file no row references that cannot be removed (a directory at the name, a permission denial) | [`ArtifactError::NameTaken`], and the **one state the protocol cannot recover on its own** — a human must move it aside. Bounded to a single title in a single run/loose directory: another title, or another day, is unaffected. |
//!
//! Within the process the reclaim is never needed: [`HeadReservation`]'s `Drop`
//! removes the file on every path out of `put` but the committed one. It exists
//! for the crash that ends the process between the two.
//!
//! ## The hand edit (§4.8, "User edits a file by hand")
//!
//! Editing a produced file by hand is *the point* of putting artifacts in the
//! project rather than in an opaque blob store, so the store records the edit
//! instead of resenting it. [`ArtifactStore::verify`] (the sweep),
//! [`ArtifactStore::resolve_content`] (every read) and [`ArtifactStore::put`]
//! (the write that would otherwise bury it) compare the head with the row: a
//! difference is a version this system did not write, and [`rotate_rows`]
//! records it as one — `author_agent_id = NULL`, `note =` [`USER_EDIT_NOTE`].
//!
//! **R32 governs the check, not just the diff.** Hashing a head is unbounded in
//! the artifact's size, and `Database::with_connection` holds the daemon's one
//! connection for its whole closure, so the read-side check is three phases and
//! only the first and last touch the database:
//!
//! | Phase | Where | What |
//! |---|---|---|
//! | [`ArtifactStore::probe_head`] | under the lock | the row, the head path, and — off `stat` alone — whether a hash is needed at all |
//! | [`HeadProbe::observe`] | **outside** the lock (the route puts it on `spawn_blocking`) | the hash, and the line counts against v(N)'s slot |
//! | [`ArtifactStore::record_head_observation`] | under the lock | the rotation, or just the stamp — rows only |
//!
//! The last phase is a compare-and-set on `sha256`: it touches the row only
//! while the row still says what it said when the probe was taken, so a `put`
//! that landed while the hash ran wins and the next read simply re-checks.
//!
//! The gate is a [`HeadStamp`] — the head's `(size, mtime)` as they stood when
//! its bytes were last hashed, stored on the row under [`HEAD_STAMP_KEY`] by
//! every path that writes a head. An unchanged head is therefore never
//! re-hashed, and the ordinary read is two short metadata queries. The gate is
//! only as exact as the filesystem's mtime: an edit that restores the byte
//! count *and* the modification time is invisible to it, which is the price of
//! not reading every artifact on every request.
//!
//! `put` runs its own check inline instead, inside the transaction that already
//! holds the connection for the whole §4.2 protocol — see [`ArtifactStore::put`].
//!
//! **The rotation moves no bytes.** It cannot: a hand edit overwrites the head
//! *in place*, so version N's bytes are already gone by the time anything
//! notices, and the only honest record is
//!
//! | Row | After the rotation |
//! |---|---|
//! | the head (`file_assets`) | `version = N+1`, `sha256`/`size_bytes` of the bytes on disk |
//! | `artifact_versions` N+1 | the head's own `rel_path`, `author_agent_id = NULL` |
//! | `artifact_versions` N | re-pointed at `.versions/<stem>/v<N>.<ext>` — where its bytes *would* have been kept had OpenAlpaca written N+1 itself. Nothing is there, so reading v(N) is [`ArtifactError::Gone`], which is the truth. |
//!
//! So the whole rotation is one transaction and there is nothing to unlink,
//! which is the strongest form of "rows before any unlink". `added_lines` /
//! `removed_lines` go through the same [`line_counts`] path a `put` uses,
//! against the same thing — the bytes version N's row now points at — and land
//! `NULL` for the same reason a `put` does when v(N-1)'s bytes are gone, which
//! after an in-place hand edit they are.
//!
//! A **missing** head is never rotated: `missing_since` set, or no file at all,
//! means there are no bytes to record.
//!
//! The *interrupted put* of the recovery table above reaches this the same way
//! — the head holds bytes no completed write left there, so its hash does not
//! match the row — and the outcome is right for it too: `.versions/<stem>/v<N>`
//! genuinely holds version N's bytes, so re-pointing N's row at them **repairs**
//! a row that was claiming the head, the line counts come out real rather than
//! `NULL`, and the orphaned head is preserved as a version instead of being
//! discarded. `author_agent_id = NULL` says exactly what is known about it:
//! nobody committed to writing these bytes. A read and a `put` now answer that
//! state identically, so whether those bytes survive no longer depends on
//! whether anyone happened to open the file first.

use std::fmt;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use rusqlite::Connection;
use similar::{ChangeTag, TextDiff, TextDiffConfig};

use crate::Database;
use crate::content_io::{fsync_dir, remove_best_effort, sha256_hex};
use crate::models::file_asset::FileAssetStatus;
use crate::models::{ArtifactKind, ArtifactOrigin};
use crate::store::{
    ContentKind, StoreScope, artifact_extension, artifact_file_name, confine_to_root, content_dir,
    leading_sequence, loose_dir, project_root_at, relative_to, run_dir, version_file_path,
};

/// `[execution.artifacts] max_versions_per_artifact` (plan §4.6). The config key
/// itself lands with the `artifact_write` tool; until then every caller passes
/// its own value through [`NewArtifact::max_versions`] and `None` means this.
pub const DEFAULT_MAX_VERSIONS_PER_ARTIFACT: u32 = 20;

/// [`ArtifactQuery::limit`]'s default page size when the caller leaves it unset.
pub const DEFAULT_LIST_LIMIT: i64 = 50;

/// `artifact_versions.note` for a version this system did not write (§4.8).
///
/// Paired with `author_agent_id = NULL`, which is the column's documented
/// meaning — "a human edited the file by hand" — so the Library can render the
/// row as a user edit without parsing the note.
pub const USER_EDIT_NOTE: &str = "edited outside OpenAlpaca";

/// `file_assets.metadata_json`'s reserved key for the head's [`HeadStamp`] —
/// the cheap gate that keeps §4.8's hand-edit check off the hash for a head
/// nothing has touched.
///
/// **This is where the pair lives.** No column records when a head's bytes were
/// last hashed and this is not a schema change, so the stamp rides in the
/// metadata column under a key no caller can collide with: `metadata` is
/// validated as a JSON *object* whose keys are the writer's own, and no writer
/// spells one with a `openalpaca:` prefix. It never reaches a client —
/// [`ArtifactRecord::metadata`] is what the routes serialise, and it takes the
/// key back out.
pub const HEAD_STAMP_KEY: &str = "openalpaca:head_stamp";

/// Unchanged lines kept either side of a change in [`ArtifactStore::diff`]'s
/// patch — the unified-diff default, and what every diff viewer expects.
const DIFF_CONTEXT_RADIUS: usize = 3;

/// R32: the largest version, **per side**, that [`ArtifactStore::diff_files`]
/// will read and diff.
///
/// Nothing caps an artifact's size on the way in (`max_artifact_bytes` defaults
/// to 10 MiB and validation allows 100 MiB), so without this a single diff
/// request could read 200 MiB and run Myers over millions of line tokens. Above
/// the cap the answer is [`ArtifactError::DiffTooLarge`] and the version rows'
/// own `added_lines`/`removed_lines` — written at *write* time, against the
/// same pair — remain the summary.
pub const MAX_DIFF_BYTES: u64 = 8 * 1024 * 1024;

/// R32: how long [`line_counts`] may spend diffing before `similar` gives up
/// and approximates. The read path is bounded by size; the write path cannot
/// be (the bytes are already committed to disk by then), so it is bounded by
/// time instead.
const LINE_COUNT_DEADLINE: std::time::Duration = std::time::Duration::from_millis(500);

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
    /// R32: one side of the diff is above [`MAX_DIFF_BYTES`] → **409**
    /// `DIFF_TOO_LARGE`. Refused before the bytes are read, not after.
    DiffTooLarge {
        id: String,
        version: u32,
        size_bytes: u64,
        limit: u64,
    },
    /// R24: the head name is held by a file this store will not replace —
    /// because a row describes it (a concurrent or foreign writer, an upload at
    /// the same `rel_path`, a retained version), or because it could not be
    /// removed.
    ///
    /// An artifact is addressed *by name*, so unlike an upload there is no
    /// sequence to bump: the only two answers are to reclaim the name and to
    /// refuse it. R33 draws that line at the rows — bytes no row references are
    /// an uncommitted write and get reclaimed; anything else is refused, since
    /// replacing bytes this store *can* account for is the one thing the write
    /// protocol exists to prevent.
    NameTaken { path: String },
}

impl ArtifactError {
    /// The stable error code a route puts in `{error:{code:…}}`.
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotFound { .. } => "ARTIFACT_NOT_FOUND",
            Self::Gone { .. } => "ARTIFACT_GONE",
            Self::VersionNotFound { .. } => "ARTIFACT_VERSION_NOT_FOUND",
            Self::NotDiffable { .. } => "NOT_DIFFABLE",
            Self::DiffTooLarge { .. } => "DIFF_TOO_LARGE",
            Self::NameTaken { .. } => "ARTIFACT_NAME_TAKEN",
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
            Self::DiffTooLarge {
                id,
                version,
                size_bytes,
                limit,
            } => write!(
                f,
                "artifact {id} version {version} is {size_bytes} bytes, above the \
                 {limit}-byte diff limit; its stored line counts are the summary"
            ),
            Self::NameTaken { path } => write!(
                f,
                "{path} is held by a file this store will not replace — another row \
                 describes it, or it could not be removed; move it aside and write again"
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

    /// The **writer's** metadata — `metadata_json` parsed, with
    /// [`HEAD_STAMP_KEY`] taken back out. This is what a route serialises: the
    /// stamp is this module's bookkeeping and no client's business.
    ///
    /// An object that held nothing but the stamp is `None`: no caller ever
    /// wrote metadata there.
    pub fn metadata(&self) -> Option<serde_json::Value> {
        let mut value: serde_json::Value =
            serde_json::from_str(self.metadata_json.as_deref()?).ok()?;
        if let Some(object) = value.as_object_mut() {
            object.remove(HEAD_STAMP_KEY);
            if object.is_empty() {
                return None;
            }
        }
        Some(value)
    }

    /// The stamp recorded the last time this head's bytes were hashed.
    fn head_stamp(&self) -> Option<HeadStamp> {
        let value: serde_json::Value = serde_json::from_str(self.metadata_json.as_deref()?).ok()?;
        HeadStamp::from_json(value.get(HEAD_STAMP_KEY)?)
    }
}

/// The cheap gate on §4.8's hand-edit check: a head's size and modification
/// time as they stood when its bytes were last hashed ([`HEAD_STAMP_KEY`]).
///
/// Compared for **equality**, never for order — a clock that moved backwards
/// must re-hash, not skip — and absent means "hash it", so a row that has never
/// been stamped is checked exactly as before.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeadStamp {
    pub size_bytes: i64,
    /// Nanoseconds since the Unix epoch, saturating at the ends of `i64`
    /// (year 2262 and 1678, neither of which any filesystem reports for a file
    /// this store wrote).
    pub mtime_ns: i64,
}

impl HeadStamp {
    /// The stamp of a head that is standing right now. `None` when the platform
    /// will not report a modification time — then nothing is ever skipped.
    fn of(metadata: &fs::Metadata) -> Option<Self> {
        let modified = metadata.modified().ok()?;
        let mtime_ns = match modified.duration_since(std::time::UNIX_EPOCH) {
            Ok(since) => i64::try_from(since.as_nanos()).unwrap_or(i64::MAX),
            Err(before) => i64::try_from(before.duration().as_nanos())
                .map(|ns| -ns)
                .unwrap_or(i64::MIN),
        };
        Some(Self {
            size_bytes: metadata.len() as i64,
            mtime_ns,
        })
    }

    fn from_json(value: &serde_json::Value) -> Option<Self> {
        Some(Self {
            size_bytes: value.get("size_bytes")?.as_i64()?,
            mtime_ns: value.get("mtime_ns")?.as_i64()?,
        })
    }

    fn to_json(self) -> serde_json::Value {
        serde_json::json!({ "size_bytes": self.size_bytes, "mtime_ns": self.mtime_ns })
    }
}

/// The stamp of the head standing at `path`, or `None` when it cannot be
/// stat'd — an unstampable head is simply re-checked next time.
fn stamp_at(path: &Path) -> Option<HeadStamp> {
    HeadStamp::of(&fs::metadata(path).ok()?)
}

/// `metadata_json` carrying `stamp` under [`HEAD_STAMP_KEY`], leaving every
/// other key exactly as the writer wrote it.
///
/// `None` stamp passes the column straight through, and so does metadata that
/// is not a JSON object: this must never rewrite what a caller stored.
fn metadata_with_stamp(metadata_json: Option<&str>, stamp: Option<HeadStamp>) -> Option<String> {
    let Some(stamp) = stamp else {
        return metadata_json.map(str::to_string);
    };
    let mut value = match metadata_json {
        Some(text) => match serde_json::from_str(text) {
            Ok(value) => value,
            // Unparseable metadata is still the writer's; leave it alone and
            // pay for one hash per read rather than destroy it.
            Err(_) => return Some(text.to_string()),
        },
        None => serde_json::json!({}),
    };
    let Some(object) = value.as_object_mut() else {
        return metadata_json.map(str::to_string);
    };
    object.insert(HEAD_STAMP_KEY.to_string(), stamp.to_json());
    Some(value.to_string())
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

/// How many rows each member of §4.8's one transaction carried — the same four
/// counts whether they were moved ([`ArtifactStore::rebase_project`]) or merely
/// counted ([`ArtifactStore::workspace_rows`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RebaseCounts {
    /// `file_assets` rows, **both** origins.
    pub file_assets: usize,
    /// `session.workspace_id` (migration 039).
    pub sessions: usize,
    /// `task.workspace_id` (migration 036).
    pub tasks: usize,
    /// `memory.scope_id` where `scope = 'workspace'`.
    pub memories: usize,
}

impl RebaseCounts {
    /// Does any row in the system name this root?
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// What [`ArtifactStore::workspace_rows`] found under one root.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WorkspaceRows {
    pub counts: RebaseCounts,
    /// Runs under this root that are `running` or `paused`. Both hold live
    /// in-process state — a loop, a steering inbox, a resolved store — that a
    /// row rewrite cannot reach, so a rebase waits for them. A `queued` run
    /// does not count here: it has not resolved anything yet, and its row
    /// moves with the rest of a re-base. (A purge's own busy predicate is
    /// stricter than this field — see [`ArtifactStore::busy_tasks`] — because
    /// a purge *deletes* the row a queued run is about to resolve, rather than
    /// rewriting it.)
    pub active_tasks: usize,
    /// Runs under this root that are `queued` — dispatched, not started. They
    /// do not block a re-base (the row moves with the rest of one) and they *do*
    /// block a purge ([`ArtifactStore::busy_tasks`] counts them), so a caller
    /// reading this number can tell a root that is merely busy-soon from one
    /// that is running. Reported rather than folded into `active_tasks`: the two
    /// answer different questions.
    pub queued_tasks: usize,
    /// Rows under this root that belong to **another** owner — `file_assets`
    /// and `memory`, the two members that carry an `owner_id` at all. Always
    /// `0` for an unscoped count.
    ///
    /// A re-base is owner-scoped, and this is how: a root holding rows the
    /// caller cannot see is a `404`, not a transaction that quietly rewrites
    /// them.
    pub other_owners: usize,
}

/// How many rows of each kind a purge removed, or would remove
/// ([`ArtifactStore::purge_plan`] / [`ArtifactStore::purge_project`]).
///
/// Every member is named on the way out, zeroes included — "nothing else went"
/// is the half a reader is checking for.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PurgeCounts {
    /// `session` rows with `workspace_id = root`.
    pub sessions: usize,
    /// `conversation_messages` rows belonging to those sessions.
    pub messages: usize,
    /// `tool_execution_log` rows keyed by one of those sessions or runs.
    pub tool_calls: usize,
    /// `lane_followups` rows keyed by one of those sessions.
    pub followups: usize,
    /// `task` rows with `workspace_id = root`.
    pub tasks: usize,
    /// `subagent_span` rows belonging to those runs.
    pub spans: usize,
    /// `event_log` rows keyed by one of those runs.
    pub run_events: usize,
    /// `file_assets` rows with `origin = 'upload'` addressed under this root.
    pub uploads: usize,
}

impl PurgeCounts {
    /// Would a purge of this root remove anything at all?
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// The rows a purge deliberately leaves where they are (§4.5, §1.3 rule 3).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PurgeKept {
    /// `file_assets` rows with `origin = 'produced'` — never garbage-collected.
    pub artifacts: usize,
    /// `memory` rows scoped to this workspace — the user's, not the store's.
    pub memories: usize,
}

/// What the **home** scope holds of the two kinds a project purge removes.
///
/// Not a project and never purged by one (§1.1): a conversation with no project
/// and an upload that carried no project signal belong to the home store, and
/// `store purge --all` names them as kept rather than sweeping them up.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HomeScopeRows {
    /// `session` rows with `workspace_id IS NULL` or `workspace_id = ''` —
    /// together with [`ArtifactStore::project_roots`]'s `root <> ''`, the two
    /// partition every session in the table.
    pub sessions: usize,
    /// `file_assets` upload rows with no `project_root`.
    pub uploads: usize,
}

/// One session a purge would take with it, with the lane it belongs to so a
/// caller can ask the live registry whether a run of its own is still going.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PurgeSession {
    pub id: String,
    pub lane_key: String,
}

/// What a purge of one root would do, read without changing anything —
/// the body of `--dry-run` and the preflight of the real call.
#[derive(Debug, Clone, Default)]
pub struct PurgePlan {
    pub counts: PurgeCounts,
    pub kept: PurgeKept,
    /// The sessions bound to this root, for the caller's in-flight guard.
    pub sessions: Vec<PurgeSession>,
}

/// What a purge actually removed — the counts, plus the bytes the caller must
/// now remove, gathered **inside** the transaction so the list can never name
/// a row the transaction did not delete.
#[derive(Debug, Clone, Default)]
pub struct PurgeOutcome {
    pub counts: PurgeCounts,
    /// The ids whose `sessions/<id>/` directory in the home store is now
    /// rowless.
    pub session_ids: Vec<String>,
    /// `storage_path` of every upload row that went.
    pub upload_paths: Vec<String>,
}

/// What one [`ArtifactStore::verify`] pass found.
#[derive(Debug, Clone, Default)]
pub struct VerifyReport {
    /// How many produced rows under the swept root have no bytes — the running
    /// total, not this pass's delta.
    pub missing: usize,
    /// The heads **this** pass found edited outside OpenAlpaca, each as the
    /// record stands after the rotation. One `ArtifactWritten` (with a null
    /// `agent_id`) per entry.
    pub user_edits: Vec<ArtifactRecord>,
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
    /// After the head's own unnoticed hand edit has been recorded as a version
    /// and before this write's bytes reach the disk (§4.8).
    UserEditRotated,
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

/// The two row steps of [`rotate_user_edit`], named so a test can abort the
/// rotation *between* them. They are inside one transaction, so an abort at
/// either point must leave the store exactly as it was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RotateStep {
    /// After version N's row has been re-pointed at its `.versions/` slot.
    PreviousRepointed,
    /// After version N+1's row has been inserted.
    VersionInserted,
}

#[cfg(test)]
thread_local! {
    static ROTATE_CRASH_AT: std::cell::Cell<Option<RotateStep>> = const { std::cell::Cell::new(None) };
}

#[cfg(test)]
fn crash_rotate_after(step: RotateStep) {
    ROTATE_CRASH_AT.with(|c| c.set(Some(step)));
}

#[cfg(test)]
fn clear_rotate_crash() {
    ROTATE_CRASH_AT.with(|c| c.set(None));
}

#[cfg(test)]
fn rotate_crash_point(step: RotateStep) -> Result<()> {
    if ROTATE_CRASH_AT.with(|c| c.get()) == Some(step) {
        bail!("simulated crash after {step:?}");
    }
    Ok(())
}

#[cfg(not(test))]
#[inline(always)]
fn rotate_crash_point(_step: RotateStep) -> Result<()> {
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
        // R24: the address is the store the bytes *reach*, not the path the
        // caller named — see `store::project_root_at`. Both the row and the
        // directory scan below take it, so two scopes that resolve to one
        // directory share one sequence space (and one identity) instead of both
        // starting at `01` over each other's files.
        let store_root = artifacts_root
            .parent()
            .with_context(|| format!("{} has no parent store root", artifacts_root.display()))?;
        let project_root = project_root_at(store_root)?;
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

            // --- The unnoticed hand edit (§4.8) ------------------------------
            // The head this write is about to replace may hold bytes nobody
            // committed — a hand edit no read and no sweep has seen. Record it
            // as the user's version *first*, so no version row ever ends up
            // carrying a sha that does not describe the bytes its `rel_path`
            // points at. Both rotations are in this one transaction, so an
            // abort between them records neither.
            //
            // This runs inside `with_connection` where the read path's does
            // not, because `put` holds the connection for its whole protocol by
            // design (that is what serialises concurrent puts) — and the stamp
            // the last write left means an ordinary supersede hashes nothing.
            let hand_edit = match existing {
                Some(row) => rotate_hand_edit(&tx, &row.id)?,
                None => None,
            };
            crash_point(WriteStep::UserEditRotated)?;
            let previous_version = match &hand_edit {
                Some(rotated) => Some(rotated.version),
                None => existing.map(|r| r.version),
            };
            let version = previous_version.map_or(1, |previous| previous + 1);

            // --- The §4.2 write protocol -------------------------------------
            fs::create_dir_all(&dir)
                .with_context(|| format!("failed to create {}", dir.display()))?;
            // R24: claim the name with `O_EXCL` before a byte is written. Only
            // a *new* artifact reserves — superseding one already owns the name
            // its own row addresses, and step 3 moves that file aside itself.
            // The reservation is dropped (and the file with it) on every path
            // out of this closure but the committed one.
            let reservation = match existing {
                Some(_) => None,
                None => Some(reserve_head(&tx, &project_key, &head_rel, &head_path)?),
            };
            let rotated_rel = write_bytes(
                &artifacts_root,
                &dir,
                &head_path,
                &head_name,
                new.content,
                previous_version,
            )?;
            // Stamp the head this call just wrote, so the read path's §4.8
            // check has something to compare against and never hashes a head
            // nobody has touched since. `None` (an unstattable head) simply
            // costs one hash on the next read.
            let metadata_json = metadata_with_stamp(new.metadata_json, stamp_at(&head_path));

            // Line counts are recorded at write time (§4.9), against the bytes
            // of the version the *rows* describe — which is what `write_bytes`
            // just reported. Reading the head before the rotate instead would
            // count an interrupted put's orphaned head, bytes no committed row
            // ever described (the T23 re-review's Minor 8); after the rotate
            // there is only one candidate and it is the right one. `None` is
            // v1, a non-text kind, or a v(N-1) whose bytes are gone.
            let (added_lines, removed_lines) = match (&rotated_rel, is_text_kind(new.kind)) {
                (Some(rel), true) => {
                    let previous = artifacts_root.join(rel);
                    match fs::read(&previous) {
                        Ok(bytes) => {
                            let (a, r) = line_counts(&bytes, new.content);
                            (Some(a), Some(r))
                        }
                        Err(e) => {
                            // Advisory numbers: a version history without them
                            // beats refusing a write whose bytes are already on
                            // disk.
                            tracing::warn!(
                                "Failed to read {} for this version's line counts: {e}",
                                previous.display()
                            );
                            (None, None)
                        }
                    }
                }
                _ => (None, None),
            };

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
                            metadata_json,
                            row.id,
                        ],
                    )?;
                    // The version that was the head now lives under
                    // `.versions/` — the hand edit's own version when one was
                    // just recorded, since that is what stood at the head.
                    if let (Some(rel), Some(previous)) = (&rotated_rel, previous_version) {
                        tx.execute(
                            "UPDATE artifact_versions SET rel_path = ?1
                             WHERE artifact_id = ?2 AND version = ?3",
                            rusqlite::params![rel, row.id, previous],
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
                            metadata_json,
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
            // The row is committed: the bytes standing at the reserved name are
            // now described by it and must outlive this call.
            if let Some(reservation) = reservation {
                reservation.keep();
            }
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
    ///
    /// A read is also where a **hand edit** is noticed (see the module docs):
    /// the head is hashed and, if its bytes are not the ones the row describes,
    /// recorded as a version with `author_agent_id = NULL` before the path is
    /// resolved. [`Self::resolve_content_with_edit`] is the same call for a
    /// caller that wants to announce that.
    pub fn resolve_content(&self, id: &str, version: Option<u32>) -> Result<PathBuf> {
        self.resolve_content_with_edit(id, version)
            .map(|(path, _)| path)
    }

    /// [`Self::resolve_content`], reporting the hand edit it recorded.
    ///
    /// `Some(record)` is the head **after** the rotation — its `version` is the
    /// new one — and is what the daemon turns into an `ArtifactWritten` with a
    /// null `agent_id`. `None` is the ordinary read where the bytes on disk are
    /// the bytes the row describes.
    ///
    /// R32: the hash runs **between** two short locked sections, never inside
    /// one, and a head whose [`HeadStamp`] still matches is not hashed at all —
    /// so the ordinary read costs one `stat` and two metadata queries. This is
    /// synchronous throughout; an async caller runs the whole call on
    /// `spawn_blocking`, which is what puts the hash on a blocking thread.
    pub fn resolve_content_with_edit(
        &self,
        id: &str,
        version: Option<u32>,
    ) -> Result<(PathBuf, Option<ArtifactRecord>)> {
        let edit = self.check_head(id)?;
        self.db.with_connection(|conn| {
            // Resolve against the rotated record: `version` numbers moved.
            let current = match &edit {
                Some(rotated) => rotated.clone(),
                None => load_by_id(conn, id)?.ok_or_else(|| not_found(id))?,
            };
            let path = resolve_version_path(conn, &current, version)?;
            Ok((path, edit))
        })
    }

    /// §4.8's hand-edit check, all three phases in order — the shape every
    /// detection site shares, with the hash outside the connection mutex.
    ///
    /// `Ok(Some(record))` is the head as it stands after a rotation; `Ok(None)`
    /// is every other outcome, including the race where a `put` landed while
    /// the hash ran (phase 3's compare-and-set declines, and the next read
    /// asks again).
    pub fn check_head(&self, id: &str) -> Result<Option<ArtifactRecord>> {
        let probe = self.probe_head(id)?;
        let Some(observation) = probe.observe() else {
            return Ok(None);
        };
        self.record_head_observation(id, observation)
    }

    /// Phase 1: the row and the cheap gate, under the lock and off metadata
    /// alone. [`ArtifactError::NotFound`] when there is no such row.
    pub fn probe_head(&self, id: &str) -> Result<HeadProbe> {
        self.db.with_connection(|conn| {
            let record = load_by_id(conn, id)?.ok_or_else(|| not_found(id))?;
            Ok(probe_for(&record))
        })
    }

    /// Phase 3: the rotation when the bytes differ from what the row describes,
    /// otherwise just the [`HeadStamp`] that keeps the next read off the hash.
    /// Rows only — nothing here reads a file.
    ///
    /// Compare-and-set on `sha256`: the row is touched only while it still says
    /// what it said when the probe was taken. A `put` (or another reader's
    /// rotation) that landed in between therefore wins outright, and this
    /// answers `None` rather than recording an observation of bytes nobody
    /// describes any more.
    pub fn record_head_observation(
        &self,
        id: &str,
        observation: HeadObservation,
    ) -> Result<Option<ArtifactRecord>> {
        self.db.with_connection(|conn| {
            let tx = conn.unchecked_transaction()?;
            let Some(record) = load_by_id(&tx, id)? else {
                return Ok(None);
            };
            if record.sha256 != observation.row_sha256
                || record.origin != ArtifactOrigin::Produced
                || record.missing_since.is_some()
            {
                return Ok(None);
            }
            let rotated = if observation.sha256 == record.sha256 {
                // The bytes are the ones the row describes after all — the
                // first read of a head no stamp had reached yet. Remember the
                // stamp so nothing hashes them again.
                tx.execute(
                    "UPDATE file_assets SET metadata_json = ?1 WHERE id = ?2",
                    rusqlite::params![
                        metadata_with_stamp(record.metadata_json.as_deref(), observation.stamp),
                        record.id,
                    ],
                )?;
                None
            } else {
                Some(rotate_rows(&tx, &record, &observation)?)
            };
            tx.commit()?;
            Ok(rotated)
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

    /// The unified patch from version `from` to version `to` (§4.9).
    ///
    /// `kind ∈ {image, binary}` is [`ArtifactError::NotDiffable`] (§4.9's 409)
    /// and that answer is final. Every other kind is read off disk and diffed
    /// line by line: a version whose bytes are gone is [`ArtifactError::Gone`]
    /// and a version that never existed is [`ArtifactError::VersionNotFound`],
    /// the same answers reading the version through [`Self::resolve_content`]
    /// gives — the diff opens the very same files.
    ///
    /// `added_lines`/`removed_lines` are counted from the same [`TextDiff`] the
    /// patch is rendered from, so the pair and the patch's own `+`/`-` totals
    /// cannot disagree. Identical versions are an **empty** patch and `(0, 0)`,
    /// not an error.
    ///
    /// R32: this is both halves in one call, for a caller that is not holding
    /// up anything else. An **async** caller wants [`Self::diff_paths`] and
    /// [`Self::diff_files`] instead, with the second half on a blocking thread
    /// — see the note on `diff_files`.
    pub fn diff(&self, id: &str, from: u32, to: u32) -> Result<ArtifactDiff> {
        let (from_path, to_path) = self.diff_paths(id, from, to)?;
        Self::diff_files(id, from, &from_path, to, &to_path)
    }

    /// R32, the half of [`Self::diff`] that needs the database: the kind gate
    /// and the absolute path of each version's bytes, resolved in **one**
    /// `with_connection` (the connection mutex is not reentrant).
    ///
    /// `kind ∈ {image, binary}` is [`ArtifactError::NotDiffable`]; a version
    /// that never existed is [`ArtifactError::VersionNotFound`] and one whose
    /// bytes are gone is [`ArtifactError::Gone`] — and, as when reading it,
    /// an absent *head* stamps `missing_since` on the row before saying so.
    pub fn diff_paths(&self, id: &str, from: u32, to: u32) -> Result<(PathBuf, PathBuf)> {
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

            let from_path = resolve_version_path(conn, &record, Some(from))?;
            let to_path = resolve_version_path(conn, &record, Some(to))?;
            Ok((from_path, to_path))
        })
    }

    /// R32, the half that needs no database: two reads, one [`TextDiff`], one
    /// rendered patch. Takes no `self` precisely because it must run *outside*
    /// [`Self::diff_paths`]'s `with_connection` — the reads and the Myers diff
    /// are unbounded in the artifact's size, and `Database::with_connection`
    /// holds the daemon's single connection mutex for the whole closure, so
    /// doing this inside it stalls every other database caller.
    ///
    /// Bounded by [`MAX_DIFF_BYTES`] per side: a bigger version is
    /// [`ArtifactError::DiffTooLarge`], refused off its metadata before a byte
    /// is read.
    ///
    /// The bytes are decoded lossily ([`read_version_text`]), so an invalid
    /// byte in a nominally-text artifact reaches the served patch as U+FFFD.
    pub fn diff_files(
        id: &str,
        from: u32,
        from_path: &Path,
        to: u32,
        to_path: &Path,
    ) -> Result<ArtifactDiff> {
        let old = read_version_text(id, from, from_path)?;
        let new = read_version_text(id, to, to_path)?;
        let diff = TextDiff::from_lines(old.as_str(), new.as_str());
        let (added_lines, removed_lines) = change_counts(&diff);
        let patch = diff
            .unified_diff()
            .context_radius(DIFF_CONTEXT_RADIUS)
            .header(&format!("v{from}"), &format!("v{to}"))
            .to_string();

        Ok(ArtifactDiff {
            from,
            to,
            added_lines,
            removed_lines,
            format: "unified",
            patch,
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

    /// Re-base everything addressed under `old_root` onto `new_root` — §4.8's
    /// "Project moved", as **one transaction** over its four members.
    ///
    /// A project's path is its identity in four places, all of them holding the
    /// same canonical-path string, so all four move together or none does:
    ///
    /// | Member | Column |
    /// |---|---|
    /// | `file_assets` (**both** origins — an upload placed in a project store moved with it) | `project_root`, and the `storage_path` prefix |
    /// | `session` (039) | `workspace_id` |
    /// | `task` (036) | `workspace_id` |
    /// | `memory` (`memory/workspace.rs`: the id *is* the canonical root) | `scope_id`, where `scope = 'workspace'` |
    ///
    /// `rel_path` is deliberately untouched: it is the address, and it did not
    /// change. Only the root prefix does.
    ///
    /// This lives on `ArtifactStore` because §4.8 puts it here — the artifact
    /// address is the member with the most to lose from a half-applied move —
    /// but the other three are not artifact rows and are updated by their own
    /// statements, not through their repositories: a transaction is the whole
    /// point, and each repository call would open its own.
    ///
    /// Rows only. Moving the store *directory*, when the caller is asking for a
    /// move rather than recording one that already happened, is
    /// [`crate::store::migrate::move_project_store`].
    ///
    /// **Path-scoped, not owner-scoped**, and deliberately: `session` and `task`
    /// carry no `owner_id`, so a half-scoped transaction would leave a project
    /// whose artifacts moved and whose runs did not. The route is what makes it
    /// owner-safe — it refuses a root holding rows the caller does not own
    /// ([`WorkspaceRows::other_owners`]) rather than rewrite them here.
    pub fn rebase_project(&self, old_root: &str, new_root: &str) -> Result<RebaseCounts> {
        self.db.with_connection(|conn| {
            let tx = conn.unchecked_transaction()?;
            // `missing_since` is cleared with the address it described: the
            // mark says "nothing is at `storage_path`", and `storage_path` is
            // exactly what this statement rewrites. No stat pass inside the
            // transaction (R32) — the read path is the authority on absence and
            // re-stamps on the next head read if the bytes are still away.
            let file_assets = tx.execute(
                "UPDATE file_assets
                    SET project_root = ?2,
                        storage_path = CASE
                            WHEN substr(storage_path, 1, length(?1)) = ?1
                            THEN ?2 || substr(storage_path, length(?1) + 1)
                            ELSE storage_path END,
                        missing_since = NULL,
                        updated_at = datetime('now')
                  WHERE project_root = ?1",
                rusqlite::params![old_root, new_root],
            )?;
            let sessions = tx.execute(
                "UPDATE session SET workspace_id = ?2, updated_at = datetime('now')
                  WHERE workspace_id = ?1",
                rusqlite::params![old_root, new_root],
            )?;
            let tasks = tx.execute(
                "UPDATE task SET workspace_id = ?2, updated_at = datetime('now')
                  WHERE workspace_id = ?1",
                rusqlite::params![old_root, new_root],
            )?;
            // The memory repository has no re-key path of its own, so this is
            // the statement. `idx_memory_content_hash` is
            // (owner_id, scope, scope_id, content_hash): re-keying onto a
            // scope that already holds the same content would violate it, and
            // that is exactly when the whole transaction must roll back rather
            // than merge two projects' memories.
            let memories = tx.execute(
                "UPDATE memory SET scope_id = ?2, updated_at = datetime('now')
                  WHERE scope = 'workspace' AND scope_id = ?1",
                rusqlite::params![old_root, new_root],
            )?;
            tx.commit()?;
            Ok(RebaseCounts {
                file_assets,
                sessions,
                tasks,
                memories,
            })
        })
    }

    /// What is recorded under `root` right now — the same four members
    /// [`Self::rebase_project`] moves, plus the runs that are still in flight
    /// there.
    ///
    /// The preflight of `PATCH /v1/workspaces` and the body of its `--dry-run`:
    /// a rebase onto a root that already has rows would merge two projects, and
    /// one out from under a running task would rewrite the row without moving
    /// the process.
    ///
    /// `owner_id` scopes the two members that carry one — `file_assets` and
    /// `memory` — and fills [`WorkspaceRows::other_owners`] with what it
    /// therefore left out. `session` and `task` have no owner column at all, so
    /// their counts are the root's whether or not a caller is named; that
    /// asymmetry is exactly why a root holding another owner's rows is refused
    /// rather than partly re-based. `None` counts every owner, which is what
    /// describing a root (rather than writing it) asks for.
    pub fn workspace_rows(&self, root: &str, owner_id: Option<&str>) -> Result<WorkspaceRows> {
        self.db.with_connection(|conn| {
            let count = |sql: &str| -> Result<usize> {
                Ok(
                    conn.query_row(sql, rusqlite::params![root], |row| row.get::<_, i64>(0))?
                        as usize,
                )
            };
            let owned = |sql: &str| -> Result<usize> {
                Ok(
                    conn.query_row(sql, rusqlite::params![root, owner_id], |row| {
                        row.get::<_, i64>(0)
                    })? as usize,
                )
            };
            let (file_assets, memories, other_owners) = match owner_id {
                Some(_) => (
                    owned(
                        "SELECT COUNT(*) FROM file_assets
                          WHERE project_root = ?1 AND owner_id = ?2",
                    )?,
                    owned(
                        "SELECT COUNT(*) FROM memory
                          WHERE scope = 'workspace' AND scope_id = ?1 AND owner_id = ?2",
                    )?,
                    owned(
                        "SELECT (SELECT COUNT(*) FROM file_assets
                                  WHERE project_root = ?1 AND owner_id <> ?2)
                              + (SELECT COUNT(*) FROM memory
                                  WHERE scope = 'workspace' AND scope_id = ?1 AND owner_id <> ?2)",
                    )?,
                ),
                None => (
                    count("SELECT COUNT(*) FROM file_assets WHERE project_root = ?1")?,
                    count(
                        "SELECT COUNT(*) FROM memory WHERE scope = 'workspace' AND scope_id = ?1",
                    )?,
                    0,
                ),
            };
            Ok(WorkspaceRows {
                counts: RebaseCounts {
                    file_assets,
                    sessions: count("SELECT COUNT(*) FROM session WHERE workspace_id = ?1")?,
                    tasks: count("SELECT COUNT(*) FROM task WHERE workspace_id = ?1")?,
                    memories,
                },
                active_tasks: count(
                    "SELECT COUNT(*) FROM task
                      WHERE workspace_id = ?1 AND status IN ('running', 'paused')",
                )?,
                queued_tasks: count(
                    "SELECT COUNT(*) FROM task WHERE workspace_id = ?1 AND status = 'queued'",
                )?,
                other_owners,
            })
        })
    }

    /// Runs under `root` that are `queued`, `running` or `paused` — the
    /// purge's own busy predicate, stricter than
    /// [`WorkspaceRows::active_tasks`] on purpose: a `queued` run has not
    /// reached `running` yet, but it already named this root when it was
    /// dispatched and is about to resolve the project's store the moment it
    /// starts, so purging out from under it would delete the transcript it is
    /// about to write into. The re-base predicate stays `active_tasks` —
    /// `running`/`paused` only — because a queued run's row moves with the
    /// rest of the transaction and loses nothing by being rewritten.
    pub fn busy_tasks(&self, root: &str) -> Result<usize> {
        self.db.with_connection(|conn| {
            Ok(conn.query_row(
                "SELECT COUNT(*) FROM task
                  WHERE workspace_id = ?1 AND status IN ('queued', 'running', 'paused')",
                rusqlite::params![root],
                |row| row.get::<_, i64>(0),
            )? as usize)
        })
    }

    /// Every distinct project root any of the four members names, sorted.
    ///
    /// `store purge --all` iterates this; the home scope is deliberately absent
    /// (its rows carry `NULL`, and the home store is not a project).
    pub fn project_roots(&self) -> Result<Vec<String>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT root FROM (
                     SELECT project_root AS root FROM file_assets
                     UNION SELECT workspace_id FROM session
                     UNION SELECT workspace_id FROM task
                     UNION SELECT scope_id FROM memory WHERE scope = 'workspace'
                 ) WHERE root IS NOT NULL AND root <> ''
                 ORDER BY root",
            )?;
            let roots = stmt
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<String>>>()?;
            Ok(roots)
        })
    }

    /// What the home scope holds of the two kinds a project purge removes —
    /// the "and these stay where they are" line of `--all`.
    ///
    /// `session.workspace_id` is counted as home-scope on `NULL` **or** `''`:
    /// [`Self::project_roots`] excludes `''` from every root it names
    /// (`root <> ''`), and the two must partition the table between them —
    /// nothing today ever writes `''` (the column is an `Option<String>`), but
    /// a row that did should read as "no project" rather than falling into
    /// neither half of a `--all` plan.
    pub fn home_scope_rows(&self) -> Result<HomeScopeRows> {
        self.db.with_connection(|conn| {
            let count = |sql: &str| -> Result<usize> {
                Ok(conn.query_row(sql, [], |row| row.get::<_, i64>(0))? as usize)
            };
            Ok(HomeScopeRows {
                sessions: count(
                    "SELECT COUNT(*) FROM session
                      WHERE workspace_id IS NULL OR workspace_id = ''",
                )?,
                uploads: count(
                    "SELECT COUNT(*) FROM file_assets
                      WHERE origin = 'upload' AND (project_root IS NULL OR project_root = '')",
                )?,
            })
        })
    }

    /// What a purge of `root` would remove and what it would leave — read-only.
    ///
    /// The counting mirrors [`Self::purge_project`]'s statements one for one, so
    /// a `--dry-run` and the real call cannot disagree about what is there. The
    /// two kept members are counted for the same reason the deleted ones are:
    /// "your artifacts and your memories stay" is a claim, and a plan that
    /// prints it should have looked.
    pub fn purge_plan(&self, root: &str) -> Result<PurgePlan> {
        self.db.with_connection(|conn| {
            let count = |sql: &str| -> Result<usize> {
                Ok(
                    conn.query_row(sql, rusqlite::params![root], |row| row.get::<_, i64>(0))?
                        as usize,
                )
            };
            let mut stmt =
                conn.prepare("SELECT id, lane_key FROM session WHERE workspace_id = ?1")?;
            let sessions = stmt
                .query_map(rusqlite::params![root], |row| {
                    Ok(PurgeSession {
                        id: row.get(0)?,
                        lane_key: row.get(1)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<PurgeSession>>>()?;

            Ok(PurgePlan {
                counts: PurgeCounts {
                    sessions: sessions.len(),
                    messages: count(
                        "SELECT COUNT(*) FROM conversation_messages
                          WHERE session_id IN (SELECT id FROM session WHERE workspace_id = ?1)",
                    )?,
                    tool_calls: count(
                        "SELECT COUNT(*) FROM tool_execution_log
                          WHERE session_id IN (SELECT id FROM session WHERE workspace_id = ?1)
                             OR task_id IN (SELECT id FROM task WHERE workspace_id = ?1)",
                    )?,
                    followups: count(
                        "SELECT COUNT(*) FROM lane_followups
                          WHERE session_id IN (SELECT id FROM session WHERE workspace_id = ?1)",
                    )?,
                    tasks: count("SELECT COUNT(*) FROM task WHERE workspace_id = ?1")?,
                    spans: count(
                        "SELECT COUNT(*) FROM subagent_span
                          WHERE task_id IN (SELECT id FROM task WHERE workspace_id = ?1)",
                    )?,
                    run_events: count(
                        "SELECT COUNT(*) FROM event_log
                          WHERE task_id IN (SELECT id FROM task WHERE workspace_id = ?1)",
                    )?,
                    // The one statement that cannot be copied verbatim from
                    // `purge_project`: there the attachment rows of this
                    // project's messages have already cascaded away, so the
                    // surviving links are simply whatever
                    // `conversation_message_attachments` still holds. Nothing
                    // has been deleted here, so the same set is named the long
                    // way — linked from a message this purge will *not* take.
                    uploads: count(
                        "SELECT COUNT(*) FROM file_assets
                          WHERE origin = 'upload' AND project_root = ?1
                            AND id NOT IN (
                                SELECT a.file_id FROM conversation_message_attachments a
                                  JOIN conversation_messages m ON m.id = a.message_id
                                 WHERE m.session_id IS NULL
                                    OR m.session_id NOT IN
                                       (SELECT id FROM session WHERE workspace_id = ?1))",
                    )?,
                },
                kept: PurgeKept {
                    artifacts: count(
                        "SELECT COUNT(*) FROM file_assets
                          WHERE origin = 'produced' AND project_root = ?1",
                    )?,
                    memories: count(
                        "SELECT COUNT(*) FROM memory WHERE scope = 'workspace' AND scope_id = ?1",
                    )?,
                },
                sessions,
            })
        })
    }

    /// Delete one project's conversations, runs and uploads — **one
    /// transaction**, rows only.
    ///
    /// | Goes | Because |
    /// |---|---|
    /// | `session` (`workspace_id`), its `conversation_messages`, `tool_execution_log` and `lane_followups` rows | the transcript is the thing being purged |
    /// | `task` (`workspace_id`), its `subagent_span`, `event_log`, `dispatch_decisions` and `llm_call_log` rows | the run history of this project |
    /// | `file_assets` where `origin = 'upload'`, `project_root` is this root and no message still links it | copies of what was handed to a turn |
    ///
    /// | Stays | Because |
    /// |---|---|
    /// | `file_assets` where `origin = 'produced'` | produced artifacts are never garbage-collected (§4.5) |
    /// | an upload row a surviving message still attaches | dedup is store-blind, so the row can be another project's attachment |
    /// | `memory` scoped to this workspace | the user's, not the store's |
    ///
    /// `task.session_id` on a run *outside* this root is nulled rather than
    /// cascaded, exactly as [`ConversationRepository::delete_session`] does
    /// (`crate::repository::conversation`). `conversation_messages.task_id` is
    /// left dangling on purpose — migration 038 says so in as many words: a
    /// purged run must leave the turn that started it readable.
    ///
    /// Rows only. The `sessions/<id>/` directories and the upload blobs are the
    /// caller's to remove, **after** this commits — which is why the outcome
    /// carries the exact list the transaction deleted rather than a list read
    /// beforehand.
    ///
    /// **Path-scoped, not owner-scoped**, for the same reason
    /// [`Self::rebase_project`] is: `session` and `task` carry no `owner_id`, so
    /// a half-scoped transaction would delete a project's transcripts and leave
    /// its uploads. The route is what makes it owner-safe — it refuses a root
    /// holding rows the caller does not own ([`WorkspaceRows::other_owners`]).
    ///
    /// [`ConversationRepository::delete_session`]: crate::repository::ConversationRepository::delete_session
    pub fn purge_project(&self, root: &str) -> Result<PurgeOutcome> {
        self.db.with_connection(|conn| {
            let tx = conn.unchecked_transaction()?;
            let outcome = Self::purge_within(&tx, root)?;
            tx.commit()?;
            Ok(outcome)
        })
    }

    /// [`Self::purge_project`] for **several** roots in one transaction — what
    /// `POST /v1/workspaces/purge {"all": true}` runs.
    ///
    /// `--all` was all-or-nothing only for its refusals: every root was checked
    /// before the first deletion, and then each was purged in a transaction of
    /// its own, so a failure on the third root left the first two purged and the
    /// `500` named none of them. One transaction makes the verb's promise true
    /// at both ends. The outcomes come back in the order the roots were given,
    /// so the caller can still remove each root's bytes on its own.
    pub fn purge_projects(&self, roots: &[String]) -> Result<Vec<PurgeOutcome>> {
        self.db.with_connection(|conn| {
            let tx = conn.unchecked_transaction()?;
            let mut outcomes = Vec::with_capacity(roots.len());
            for root in roots {
                outcomes.push(Self::purge_within(&tx, root)?);
            }
            tx.commit()?;
            Ok(outcomes)
        })
    }

    /// The statements both purge entry points run, inside a transaction the
    /// caller owns and commits.
    fn purge_within(tx: &rusqlite::Transaction<'_>, root: &str) -> Result<PurgeOutcome> {
        {
            // Read what is about to go, inside the transaction: this list is
            // what the caller will remove from disk, and a list gathered before
            // the transaction could name a session the transaction did not
            // delete (or miss one it did).
            let session_ids: Vec<String> = {
                let mut stmt = tx.prepare("SELECT id FROM session WHERE workspace_id = ?1")?;
                let ids = stmt
                    .query_map(rusqlite::params![root], |row| row.get::<_, String>(0))?
                    .collect::<rusqlite::Result<Vec<String>>>()?;
                ids
            };
            // Every statement below binds the same one parameter.
            let bind = rusqlite::params![root];

            // Session-keyed rows first: the sub-selects below read `session`,
            // so its own rows go last of the group.
            let messages = tx.execute(
                "DELETE FROM conversation_messages
                  WHERE session_id IN (SELECT id FROM session WHERE workspace_id = ?1)",
                bind,
            )?;
            let tool_calls = tx.execute(
                "DELETE FROM tool_execution_log
                  WHERE session_id IN (SELECT id FROM session WHERE workspace_id = ?1)
                     OR task_id IN (SELECT id FROM task WHERE workspace_id = ?1)",
                bind,
            )?;
            let followups = tx.execute(
                "DELETE FROM lane_followups
                  WHERE session_id IN (SELECT id FROM session WHERE workspace_id = ?1)",
                bind,
            )?;
            // A run under another root that was started from one of these
            // conversations keeps its row and loses the link.
            tx.execute(
                "UPDATE task SET session_id = NULL, updated_at = datetime('now')
                  WHERE session_id IN (SELECT id FROM session WHERE workspace_id = ?1)",
                bind,
            )?;
            let sessions = tx.execute("DELETE FROM session WHERE workspace_id = ?1", bind)?;

            // Run-keyed rows, then the runs. `subagent_span` would cascade from
            // `task` on its own; deleting it here is what makes the count
            // honest.
            let spans = tx.execute(
                "DELETE FROM subagent_span
                  WHERE task_id IN (SELECT id FROM task WHERE workspace_id = ?1)",
                bind,
            )?;
            let run_events = tx.execute(
                "DELETE FROM event_log
                  WHERE task_id IN (SELECT id FROM task WHERE workspace_id = ?1)",
                bind,
            )?;
            tx.execute(
                "DELETE FROM dispatch_decisions
                  WHERE task_id IN (SELECT id FROM task WHERE workspace_id = ?1)",
                bind,
            )?;
            tx.execute(
                "DELETE FROM llm_call_log
                  WHERE task_id IN (SELECT id FROM task WHERE workspace_id = ?1)",
                bind,
            )?;
            let tasks = tx.execute("DELETE FROM task WHERE workspace_id = ?1", bind)?;

            // Uploads last, and only the ones nothing still links. Dedup is
            // store-blind (`UploadStore::put` matches on sha + owner across
            // every store), so one row can be the attachment of a message in
            // another project's transcript. The messages of *this* project are
            // already gone above and their attachment rows cascaded with them,
            // so a surviving `conversation_message_attachments` row names a
            // surviving transcript — and the store never deletes what another
            // reader still names. `upload_paths` is read from the same filtered
            // set so the caller unlinks exactly the blobs whose rows went.
            const UNLINKED: &str = "origin = 'upload' AND project_root = ?1
                 AND id NOT IN (SELECT file_id FROM conversation_message_attachments)";
            let upload_paths: Vec<String> = {
                let mut stmt =
                    tx.prepare(&format!("SELECT storage_path FROM file_assets WHERE {UNLINKED}"))?;
                let paths = stmt
                    .query_map(bind, |row| row.get::<_, String>(0))?
                    .collect::<rusqlite::Result<Vec<String>>>()?;
                paths
            };
            let uploads = tx.execute(&format!("DELETE FROM file_assets WHERE {UNLINKED}"), bind)?;

            Ok(PurgeOutcome {
                counts: PurgeCounts {
                    sessions,
                    messages,
                    tool_calls,
                    followups,
                    tasks,
                    spans,
                    run_events,
                    uploads,
                },
                session_ids,
                upload_paths,
            })
        }
    }

    /// Re-stat every produced row under `project_root` (`Some("")` = the home
    /// store, `None` = every root), stamping `missing_since` on those whose
    /// bytes are gone and recording as a version every head whose bytes were
    /// edited outside OpenAlpaca (§4.8).
    ///
    /// [`VerifyReport::missing`] is how many rows *are* missing — not how many
    /// this pass newly marked — so a status caller gets the same answer every
    /// time it asks. [`VerifyReport::user_edits`] is the opposite: the edits
    /// **this** pass recorded, since each is a one-off event to announce, and a
    /// second pass over the same store finds none.
    ///
    /// The missing sweep commits before the first rotation, and each rotation
    /// is its own transaction, so a sweep that dies half way keeps everything
    /// it had already recorded and the next one resumes from there.
    pub fn verify(&self, project_root: Option<&str>) -> Result<VerifyReport> {
        let records: Vec<ArtifactRecord> = self.db.with_connection(|conn| {
            let mut sql =
                format!("SELECT {RECORD_COLUMNS} FROM file_assets WHERE origin = 'produced'");
            if project_root.is_some() {
                sql.push_str(" AND COALESCE(project_root, '') = ?1");
            }
            let mut stmt = conn.prepare(&sql)?;
            let mut mapped = match project_root {
                Some(root) => stmt.query(rusqlite::params![root])?,
                None => stmt.query([])?,
            };
            let mut out = Vec::new();
            while let Some(row) = mapped.next()? {
                out.push(row_to_record(row)?);
            }
            Ok(out)
        })?;

        // R32 again: the `stat` per row is filesystem work, so it happens with
        // the connection free and only the marking is done under it.
        let (present, absent): (Vec<ArtifactRecord>, Vec<ArtifactRecord>) = records
            .into_iter()
            .partition(|record| Path::new(&record.storage_path).exists());
        let missing = absent.len();
        self.db.with_connection(|conn| {
            let tx = conn.unchecked_transaction()?;
            for record in absent.iter().filter(|r| r.missing_since.is_none()) {
                tx.execute(
                    "UPDATE file_assets SET missing_since = datetime('now'),
                        updated_at = datetime('now') WHERE id = ?1",
                    rusqlite::params![record.id],
                )?;
            }
            tx.commit()?;
            Ok(())
        })?;

        let mut user_edits = Vec::new();
        for record in &present {
            match self.check_head(&record.id) {
                Ok(Some(rotated)) => user_edits.push(rotated),
                Ok(None) => {}
                // One unreadable artifact must not abandon the sweep: the rows
                // it would have fixed are still there next boot.
                Err(e) => tracing::warn!(
                    "Failed to record a hand edit of artifact {}: {e:#}",
                    record.id
                ),
            }
        }
        Ok(VerifyReport {
            missing,
            user_edits,
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

/// The absolute path of one version's bytes, and the two typed failures that
/// answer for it — [`ArtifactStore::resolve_content`]'s whole body, factored
/// out because [`ArtifactStore::diff`] needs the same answer for two versions
/// *inside* one `with_connection` (the connection mutex is not reentrant).
///
/// `version` of `None`, or the current version number, is the head; only a
/// missing **head** stamps `missing_since`, since `missing` describes the head
/// and an absent older version says nothing about it.
fn resolve_version_path(
    conn: &Connection,
    record: &ArtifactRecord,
    version: Option<u32>,
) -> Result<PathBuf> {
    use rusqlite::OptionalExtension;

    let id = record.id.as_str();
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

    let root = artifacts_root_for(record)?;
    let candidate = root.join(&rel);
    // Belt and braces (the rel_path came out of our own grammar). A deleted
    // project has no root to canonicalize against — that is a gone artifact,
    // not a confinement failure.
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
}

/// Phase 1 of §4.8's hand-edit check, decided off metadata alone: which head to
/// hash, and whether it needs hashing at all.
///
/// Everything it must **not** touch answers `work: None` and costs one `stat`:
///
/// * an **upload**: `UploadStore` owns those bytes, they have no version
///   history, and inventing a v2 for one would leave a v1 no row describes;
/// * a row with no `rel_path` (it predates migration 036, so it has no address
///   in this grammar);
/// * a **missing** head — `missing_since` set, or simply no file — because
///   there are no bytes to record;
/// * a head whose [`HeadStamp`] is the one the row already carries, which is
///   every read of a file nobody has touched.
pub struct HeadProbe {
    /// The row's `sha256` when the probe was taken — phase 3's compare-and-set.
    row_sha256: String,
    work: Option<HeadWork>,
}

/// What [`HeadProbe::observe`] has to read, once the gate says the head is
/// worth hashing.
struct HeadWork {
    head: PathBuf,
    /// Where version N's bytes would be kept, read only to count lines. Absent
    /// after an in-place hand edit; present after an interrupted put.
    slot: Option<PathBuf>,
    is_text: bool,
    stamp: Option<HeadStamp>,
}

/// What the off-lock half of the check found — the bytes' own sha and size, the
/// stamp that makes the next read free, and the line counts against v(N).
pub struct HeadObservation {
    row_sha256: String,
    sha256: String,
    size_bytes: i64,
    stamp: Option<HeadStamp>,
    added_lines: Option<i64>,
    removed_lines: Option<i64>,
}

fn probe_for(record: &ArtifactRecord) -> HeadProbe {
    let nothing = HeadProbe {
        row_sha256: record.sha256.clone(),
        work: None,
    };
    if record.origin != ArtifactOrigin::Produced || record.missing_since.is_some() {
        return nothing;
    }
    let (Some(rel), Ok(root)) = (record.rel_path.as_deref(), artifacts_root_for(record)) else {
        return nothing;
    };
    let head = root.join(rel);
    let Some(stamp) = stamp_at(&head) else {
        // Absent is `verify`'s and `resolve_version_path`'s business, not this
        // one; anything else is a store we cannot stat, and refusing the whole
        // request over it would turn a permission problem into an outage.
        return nothing;
    };
    if record.head_stamp() == Some(stamp) {
        return nothing;
    }
    HeadProbe {
        row_sha256: record.sha256.clone(),
        work: Some(HeadWork {
            slot: version_file_path(&head, record.version).ok(),
            head,
            is_text: is_text_kind_of(record),
            stamp: Some(stamp),
        }),
    }
}

impl HeadProbe {
    /// Phase 2 — **no database**, and it must stay that way (R32): the read and
    /// the line diff are unbounded in the artifact's size, and
    /// `Database::with_connection` holds the daemon's one connection for its
    /// whole closure.
    ///
    /// `None` is a head with nothing to check, or one that vanished between the
    /// `stat` and the read.
    pub fn observe(self) -> Option<HeadObservation> {
        let work = self.work?;
        let bytes = match fs::read(&work.head) {
            Ok(bytes) => bytes,
            Err(e) => {
                if e.kind() != std::io::ErrorKind::NotFound {
                    tracing::warn!("Cannot check {} for a hand edit: {e}", work.head.display());
                }
                return None;
            }
        };
        let sha256 = hash_head(&bytes);
        // The same line-count path `put` takes, against the same thing: the
        // bytes the previous version's row will point at. After an in-place
        // hand edit they are gone, so this is `NULL` — exactly as it is for a
        // `put` whose v(N-1) bytes were removed. It comes out real after an
        // interrupted put, where the slot genuinely holds version N.
        let (added_lines, removed_lines) = match (
            sha256 != self.row_sha256 && work.is_text,
            work.slot.as_deref().map(fs::read),
        ) {
            (true, Some(Ok(previous))) => {
                let (added, removed) = line_counts(&previous, &bytes);
                (Some(added), Some(removed))
            }
            _ => (None, None),
        };
        Some(HeadObservation {
            row_sha256: self.row_sha256,
            sha256,
            size_bytes: bytes.len() as i64,
            stamp: work.stamp,
            added_lines,
            removed_lines,
        })
    }
}

/// §4.8's check as one call, for a caller that is already inside a transaction
/// — which is [`ArtifactStore::put`], about to write over the very head this
/// asks about.
///
/// `Ok(None)` is a head holding the bytes its row describes (and every state
/// [`probe_for`] declines). Nothing is stamped on that path: the `put` this
/// serves rewrites `metadata_json` with a stamp of its own bytes moments later.
fn rotate_hand_edit(conn: &Connection, id: &str) -> Result<Option<ArtifactRecord>> {
    let Some(record) = load_by_id(conn, id)? else {
        return Ok(None);
    };
    let Some(observation) = probe_for(&record).observe() else {
        return Ok(None);
    };
    if observation.sha256 == record.sha256 {
        return Ok(None);
    }
    Ok(Some(rotate_rows(conn, &record, &observation)?))
}

/// The rows of the rotation, and nothing else — no reads, no hashing, no
/// unlinking. See the module docs for why version N's bytes cannot be
/// preserved.
///
/// Runs inside whatever transaction the caller opened: the read path's own, or
/// the `put` that is about to write over these very bytes.
fn rotate_rows(
    conn: &Connection,
    record: &ArtifactRecord,
    observation: &HeadObservation,
) -> Result<ArtifactRecord> {
    let rel = record
        .rel_path
        .as_deref()
        .with_context(|| format!("artifact {} has no rel_path to rotate", record.id))?;
    let root = artifacts_root_for(record)?;
    let head = root.join(rel);
    let slot_rel = relative_to(&root, &version_file_path(&head, record.version)?)?;
    let version = record.version + 1;

    tracing::info!(
        "Artifact {} was edited outside OpenAlpaca; recording {} as version {version}",
        record.id,
        head.display()
    );

    // Version N's bytes were overwritten in place. Its row moves to the slot
    // they would have been rotated into, where nothing is — so reading v(N)
    // answers `Gone`, which is what happened to it.
    conn.execute(
        "UPDATE artifact_versions SET rel_path = ?1
         WHERE artifact_id = ?2 AND version = ?3",
        rusqlite::params![slot_rel, record.id, record.version],
    )?;
    rotate_crash_point(RotateStep::PreviousRepointed)?;

    conn.execute(
        "INSERT INTO artifact_versions
            (artifact_id, version, rel_path, sha256, size_bytes, note, author_agent_id,
             added_lines, removed_lines)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, ?7, ?8)",
        rusqlite::params![
            record.id,
            version,
            rel,
            observation.sha256,
            observation.size_bytes,
            USER_EDIT_NOTE,
            observation.added_lines,
            observation.removed_lines,
        ],
    )?;
    rotate_crash_point(RotateStep::VersionInserted)?;

    // No pruning here: `max_versions_per_artifact` is the *writer's* setting and
    // this path has no caller to read it from. The next `put` prunes with the
    // real one, and one extra retained version until then beats trimming a
    // history by a default the owner may not have chosen.
    conn.execute(
        "UPDATE file_assets
            SET sha256 = ?1, size_bytes = ?2, version = ?3, metadata_json = ?4,
                version_count = (SELECT COUNT(*) FROM artifact_versions WHERE artifact_id = ?5),
                updated_at = datetime('now')
          WHERE id = ?5",
        rusqlite::params![
            observation.sha256,
            observation.size_bytes,
            version,
            metadata_with_stamp(record.metadata_json.as_deref(), observation.stamp),
            record.id,
        ],
    )?;

    load_by_id(conn, &record.id)?
        .with_context(|| format!("artifact {} vanished inside its own rotation", record.id))
}

/// The head's bytes, hashed — the one place §4.8's check pays for the file.
///
/// Counted in test builds ([`head_hashes`]) so a test can prove the
/// [`HeadStamp`] gate really does skip it, and hooked so one can prove the
/// connection is free while it runs.
fn hash_head(bytes: &[u8]) -> String {
    #[cfg(test)]
    {
        HEAD_HASHES.with(|count| count.set(count.get() + 1));
        let hook = ON_HEAD_HASH.with(|slot| slot.borrow().clone());
        if let Some(hook) = hook {
            hook();
        }
    }
    sha256_hex(bytes)
}

#[cfg(test)]
thread_local! {
    static HEAD_HASHES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static ON_HEAD_HASH: std::cell::RefCell<Option<std::sync::Arc<dyn Fn()>>> =
        const { std::cell::RefCell::new(None) };
}

/// How many heads this thread has hashed for the hand-edit check.
#[cfg(test)]
pub(crate) fn head_hashes() -> usize {
    HEAD_HASHES.with(std::cell::Cell::get)
}

#[cfg(test)]
pub(crate) fn on_head_hash(hook: Option<std::sync::Arc<dyn Fn()>>) {
    ON_HEAD_HASH.with(|slot| *slot.borrow_mut() = hook);
}

/// [`is_text_kind`] for a stored row, whose `kind` column may be `NULL`. An
/// unknown kind is projected from the MIME type, the same fallback the routes
/// serialise with.
fn is_text_kind_of(record: &ArtifactRecord) -> bool {
    is_text_kind(
        record
            .kind
            .unwrap_or_else(|| ArtifactKind::for_mime(&record.mime_type)),
    )
}

/// One version's bytes as text, refused above [`MAX_DIFF_BYTES`] off its
/// metadata — the size is answered without reading anything.
///
/// Lossy on purpose: the kind is already known to be text, and a stray invalid
/// byte is worth a replacement character in the patch rather than a failed
/// diff.
fn read_version_text(id: &str, version: u32, path: &Path) -> Result<String> {
    let size = fs::metadata(path)
        .with_context(|| format!("failed to stat {}", path.display()))?
        .len();
    if size > MAX_DIFF_BYTES {
        return Err(anyhow::Error::new(ArtifactError::DiffTooLarge {
            id: id.to_string(),
            version,
            size_bytes: size,
            limit: MAX_DIFF_BYTES,
        }));
    }
    let bytes = fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
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

/// R33: claim the head name, reclaiming it first if what holds it is an
/// **uncommitted write**.
///
/// The row is the commit. Bytes at the head address that no `file_assets` or
/// `artifact_versions` row references are therefore garbage by definition —
/// the same rule the `.versions/` recovery applies — and the state a create
/// leaves behind when the process dies between the final rename and
/// `tx.commit()`, which the in-process [`HeadReservation`] guard cannot cover.
/// Without this, one power loss would refuse that one address forever.
///
/// A file some row *does* describe is a genuine collision — a concurrent or
/// foreign writer, an upload addressing the same `rel_path` — and stays
/// [`ArtifactError::NameTaken`]. So does one that cannot be removed.
fn reserve_head<'a>(
    conn: &Connection,
    project_key: &str,
    head_rel: &str,
    head_path: &'a Path,
) -> Result<HeadReservation<'a>> {
    let taken = match HeadReservation::claim(head_path) {
        Ok(reservation) => return Ok(reservation),
        Err(e) => e,
    };
    let is_name_taken = matches!(
        taken.downcast_ref::<ArtifactError>(),
        Some(ArtifactError::NameTaken { .. })
    );
    if !is_name_taken || rel_path_is_referenced(conn, project_key, head_rel)? {
        return Err(taken);
    }

    let size = fs::metadata(head_path).map(|m| m.len()).unwrap_or(0);
    tracing::warn!(
        "Reclaiming {} ({size} bytes): no artifact row describes it, so it is an \
         uncommitted write left behind by an interrupted create",
        head_path.display()
    );
    if let Err(e) = fs::remove_file(head_path) {
        tracing::warn!("Failed to remove {}: {e}", head_path.display());
        return Err(taken);
    }
    HeadReservation::claim(head_path)
}

/// Does any row of this store address `rel` — a head in `file_assets` or a
/// retained version in `artifact_versions`? Scoped to `project_key`, because
/// `rel_path` alone repeats in every store: that pair *is* the address (§4.3).
fn rel_path_is_referenced(conn: &Connection, project_key: &str, rel: &str) -> Result<bool> {
    let referenced: bool = conn.query_row(
        "SELECT EXISTS (
             SELECT 1 FROM file_assets
              WHERE COALESCE(project_root, '') = ?1 AND rel_path = ?2
             UNION ALL
             SELECT 1 FROM artifact_versions v
               JOIN file_assets f ON f.id = v.artifact_id
              WHERE COALESCE(f.project_root, '') = ?1 AND v.rel_path = ?2
         )",
        rusqlite::params![project_key, rel],
        |row| row.get(0),
    )?;
    Ok(referenced)
}

/// R24's head reservation: the empty file this writer creates at the head name,
/// with `O_EXCL`, before writing anything.
///
/// `fs::rename` replaces its destination without a word, so the *only* way the
/// protocol's final rename cannot destroy a file is for this writer to own the
/// name first. Until [`Self::keep`] is called nothing describes what stands at
/// that name — the empty reservation, or the bytes the rename put over it — so
/// dropping the guard removes it. That is what keeps a failed write (a rolled
/// back `INSERT`, a disk-full `artifact_versions` row) from stranding an
/// unreferenced file that the *next* attempt at the same address would then
/// refuse as a taken name.
struct HeadReservation<'a> {
    path: &'a Path,
    armed: bool,
}

impl<'a> HeadReservation<'a> {
    /// Claim `path`, or [`ArtifactError::NameTaken`] if something already holds
    /// it. `reserve_head` calls this twice: once blind, and — after a failed
    /// first claim proved no row of this store references `path` and the
    /// row-less head was reclaimed — once more.
    fn claim(path: &'a Path) -> Result<Self> {
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
        {
            Ok(_) => Ok(Self { path, armed: true }),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                Err(anyhow::Error::new(ArtifactError::NameTaken {
                    path: path.to_string_lossy().to_string(),
                }))
            }
            Err(e) => {
                Err(anyhow::Error::new(e).context(format!("failed to create {}", path.display())))
            }
        }
    }

    /// A committed row now describes what stands at the name. Disarm.
    fn keep(mut self) {
        self.armed = false;
    }
}

impl Drop for HeadReservation<'_> {
    fn drop(&mut self) {
        if self.armed {
            remove_best_effort(self.path);
        }
    }
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
        } else {
            // Neither place holds bytes: the head was removed by hand. v(N-1)'s
            // row still names the head path, and the head about to be renamed
            // into place belongs to v(N) — so report the empty version slot
            // anyway. The row then tells the truth (reads answer
            // `ARTIFACT_GONE`) and, decisively, a later prune of v(N-1) unlinks
            // that empty slot instead of the live head.
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

/// `(added, removed)` for a line diff: the `+` and `-` lines its unified patch
/// will carry, counted from the diff itself rather than re-derived — the
/// patch's own totals and the numbers reported beside it are one computation.
fn change_counts<'a, T>(diff: &TextDiff<'a, 'a, T>) -> (i64, i64)
where
    T: similar::DiffableStr + ?Sized,
{
    let mut added = 0i64;
    let mut removed = 0i64;
    for change in diff.iter_all_changes() {
        match change.tag() {
            ChangeTag::Insert => added += 1,
            ChangeTag::Delete => removed += 1,
            ChangeTag::Equal => {}
        }
    }
    (added, removed)
}

/// `(added, removed)` between two texts: the `+`/`-` lines the unified patch of
/// the same pair carries.
///
/// The same [`TextDiff`] [`ArtifactStore::diff`] renders from, so the numbers
/// stored on a version row and the patch a reader is later shown of that very
/// pair are one computation — except on a pair that trips the deadline below,
/// where the stored counts summarise a non-minimal diff and the deadline-free
/// read path may render a shorter patch. The multiset tally this replaced was cheaper but
/// answered a different question — under it a *moved* line was neither added
/// nor removed, while the patch it was supposed to summarise showed both.
///
/// **Bounded by time** ([`LINE_COUNT_DEADLINE`]), because this runs at write
/// time inside [`ArtifactStore::put`]'s transaction, with the connection mutex
/// held — the size bound the read path uses is not available here, since the
/// bytes are the ones this call is committing. On the deadline `similar`
/// returns a valid but possibly non-minimal diff rather than failing, so the
/// stored pair is still a true `+`/`-` count of *some* valid edit script; only
/// its minimality, not its meaning, degrades on a pathological pair.
fn line_counts(old: &[u8], new: &[u8]) -> (i64, i64) {
    let old = String::from_utf8_lossy(old);
    let new = String::from_utf8_lossy(new);
    let mut config = TextDiffConfig::new();
    config.timeout(LINE_COUNT_DEADLINE);
    change_counts(&config.diff_lines(old.as_ref(), new.as_ref()))
}

#[cfg(test)]
mod tests;

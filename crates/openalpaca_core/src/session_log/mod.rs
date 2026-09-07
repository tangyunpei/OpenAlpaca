//! The per-session JSONL event log (plan §5.4/§5.5).
//!
//! ```text
//! ~/.openalpaca/sessions/<session-id>/
//!   log.jsonl                    ← the live segment
//!   log.<first>-<last>.jsonl     ← rotated segments
//!   results/                     ← spilled tool results (T42)
//!   snapshots/                   ← pre-edit images of overwritten files (T56)
//! ```
//!
//! One JSON object per line, append-only, `{v, seq, ts, type, task_id?,
//! span_id?, agent?, data}`. `seq` is per-session, strictly monotonic and
//! gap-free because the single writer task is the only thing that assigns it.
//!
//! **Prior art.** Claude Code keeps one append-only `<session-uuid>.jsonl`
//! per session under `~/.claude/projects/<slug>/`; every line is a JSON
//! object with a `type` discriminator, a timestamp, and a `uuid`/`parentUuid`
//! pair linking it to what came before, and the kinds are heterogeneous —
//! `assistant`/`user`/`attachment` sit beside control records like `mode` and
//! `custom-title`. Mirrored: the file-per-session shape, the `type`
//! discriminator, the per-record timestamp, the heterogeneous catalog, and
//! per-record attribution to the skill/plugin/MCP server a call came from
//! (`attributionSkill`/`attributionPlugin`/`attributionMcpServer` there;
//! `ext`/`plugin_id` on our `tool_call`/`skill_invoked` records here).
//! Deliberately not mirrored: the `uuid`/`parentUuid` chain — a gap-free
//! `seq` is a cheaper identity and a usable resume cursor; the per-line
//! re-stamping of `cwd`/`version`/`gitBranch` (§5.4: with one writer the last
//! boundary record is authoritative); and the split of subagent transcripts
//! into their own files, because §5.4 keeps one log with `span_id` as a
//! filter dimension so global order survives.

mod reader;
mod record;
pub mod recovery;
pub mod replay;
pub mod sweep;
mod writer;

#[cfg(test)]
mod tests;

pub use reader::{
    LIVE_SEGMENT, LoggedRecord, read_records, read_records_after, read_records_page, segments,
    spill_failure,
};
pub use record::{
    ENVELOPE_DATA_CAP_BYTES, ENVELOPE_VERSION, PREVIEW_CHARS, RESULTS_DIR, Record, RecordType,
    SNAPSHOTS_DIR, Spill, spill_preview, spill_stub,
};
pub use writer::SessionLogLimits;

use dashmap::DashMap;
use openalpaca_storage::Database;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::{mpsc, oneshot};
use writer::Msg;

/// Owns one writer task per session, keyed by session id.
///
/// Reachable from the runner the way the event bus is (R28): parked on
/// [`SharedContext`](crate::context::SharedContext) at boot and handed to the
/// agentic loop as a [`SessionLogHandle`] on its `LoopConfig`. There is no
/// global.
pub struct SessionLogService {
    root: PathBuf,
    db: Option<Database>,
    limits: SessionLogLimits,
    daemon_version: String,
    boot_id: String,
    handles: DashMap<String, SessionLogHandle>,
    /// Sessions this boot has already written `session_start` for. The record
    /// is a boot boundary (P-13), so an idle-close and respawn must not write
    /// a second one.
    started: DashMap<String, ()>,
    /// What the boot sweep did, when one ran (R54). Kept here so the fact that
    /// the log is still over its cap is readable — T44's `GET /v1/status` —
    /// rather than living only in a boot log line.
    last_sweep: Option<sweep::SweepReport>,
    /// Records dropped by writers that have since retired — folded in by
    /// [`handle_for`](Self::handle_for) at the moment an idled-out slot is
    /// overwritten, before the live handle's own counter (which starts back
    /// at zero) would otherwise erase them from `dropped_total()` (T44 fix
    /// round 1, Important #4: the wire field is a per-boot total and must
    /// never go down).
    retired_dropped: AtomicU64,
}

impl SessionLogService {
    /// `root` is the home store's `sessions/` directory — never a project
    /// directory: transcripts carry persona and cross-project content and
    /// must not be git-committable (§5.4).
    pub fn new(
        root: PathBuf,
        db: Option<Database>,
        limits: SessionLogLimits,
        daemon_version: String,
    ) -> Self {
        Self {
            root,
            db,
            limits,
            daemon_version,
            boot_id: uuid::Uuid::new_v4().to_string(),
            handles: DashMap::new(),
            started: DashMap::new(),
            last_sweep: None,
            retired_dropped: AtomicU64::new(0),
        }
    }

    /// Record what the boot sweep did, so the service can answer for it.
    pub fn with_last_sweep(mut self, report: sweep::SweepReport) -> Self {
        self.last_sweep = Some(report);
        self
    }

    /// The boot sweep's report — `None` when no pass ran (the active set was
    /// unreadable, or no store resolved). `over_cap_after` on it is the one
    /// thing a caller usually wants: the sessions root is still over
    /// `log_max_total_bytes` and only protected bytes are left.
    pub fn last_sweep(&self) -> Option<&sweep::SweepReport> {
        self.last_sweep.as_ref()
    }

    /// This daemon run's id, stamped on every `session_start`.
    pub fn boot_id(&self) -> &str {
        &self.boot_id
    }

    /// The sessions root every writer of this service works under.
    ///
    /// Exposed so `GET /v1/sessions/{id}/events` reads from the same place the
    /// writers write to, rather than re-resolving the store and risking a
    /// disagreement.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Where a session's log lives. The id is sanitised into a single path
    /// segment: nothing a caller passes can address a directory outside the
    /// sessions root.
    pub fn session_dir(&self, session_id: &str) -> PathBuf {
        self.root.join(safe_dir_name(session_id))
    }

    /// A handle for `session_id`, writing `session_start` if this boot has
    /// not touched the session yet.
    ///
    /// Every caller that knows the turn's lane should use this rather than
    /// [`handle_for`](Self::handle_for): the boundary record is what tells a
    /// reader which daemon run, which project and which lane the records
    /// below it belong to.
    pub fn open(
        &self,
        session_id: &str,
        lane_key: Option<&str>,
        source: Option<&str>,
        workspace_id: Option<&str>,
    ) -> SessionLogHandle {
        let handle = self.handle_for(session_id);
        if self.started.insert(session_id.to_string(), ()).is_none() {
            handle.emit(Record::new(RecordType::SessionStart).with_data(serde_json::json!({
                "daemon_version": self.daemon_version,
                "boot_id": self.boot_id,
                "workspace_id": workspace_id,
                "lane_key": lane_key,
                "source": source,
            })));
        }
        handle
    }

    /// A handle for `session_id`, spawning the writer if none is running.
    ///
    /// Asking for a handle creates nothing on disk — the writer creates the
    /// session directory on its first record (P-22).
    pub fn handle_for(&self, session_id: &str) -> SessionLogHandle {
        use dashmap::mapref::entry::Entry;
        match self.handles.entry(session_id.to_string()) {
            Entry::Occupied(mut slot) => {
                if !slot.get().is_closed() {
                    return slot.get().clone();
                }
                // The previous writer idled out; start a fresh one under the
                // same id and keep the numbering (it resumes from the file).
                // Fold its drop count into the process-level total first — the
                // fresh handle's own counter starts at zero, and without this
                // the retiring one's drops would simply vanish.
                self.retired_dropped
                    .fetch_add(slot.get().dropped(), Ordering::Relaxed);
                let handle = self.spawn(session_id);
                slot.insert(handle.clone());
                handle
            }
            Entry::Vacant(slot) => {
                let handle = self.spawn(session_id);
                slot.insert(handle.clone());
                handle
            }
        }
    }

    /// Wait until every live writer has put what it holds on disk.
    ///
    /// The barrier §5.5's durability policy needs at the two points where a
    /// writer may never be asked again: the daemon's shutdown path, before
    /// the runtime drops the writer tasks with up to `channel_capacity`
    /// records still queued, and a session's archive or delete. Everything
    /// else relies on the per-record `write` and the boundary/timer syncs.
    pub async fn flush_all(&self) {
        // Collected first: a `DashMap` reference may not be held across an
        // await, and a writer that idles out mid-flush must not deadlock the
        // map for the ones after it.
        let handles: Vec<SessionLogHandle> =
            self.handles.iter().map(|h| h.value().clone()).collect();
        for handle in handles {
            handle.flush().await;
        }
    }

    /// How many records this boot has dropped for `session_id` — a full
    /// channel, or a writer that could not open its directory (§5.5 chose
    /// drops over stalls; this is how a reader learns the log has a hole).
    ///
    /// Surfaced here rather than only on the handle so T44's status route can
    /// report it from the service it already holds.
    pub fn dropped_for(&self, session_id: &str) -> u64 {
        self.handles
            .get(session_id)
            .map(|h| h.value().dropped())
            .unwrap_or(0)
    }

    /// Records dropped across every session this boot has written — a
    /// process-lifetime total that only grows: a retired writer's count is
    /// folded into the service's own counter at respawn before its live
    /// counter resets to zero, so an idle-close never makes this number go
    /// down.
    pub fn dropped_total(&self) -> u64 {
        self.retired_dropped.load(Ordering::Relaxed)
            + self
                .handles
                .iter()
                .map(|h| h.value().dropped())
                .sum::<u64>()
    }

    fn spawn(&self, session_id: &str) -> SessionLogHandle {
        let (tx, rx) = mpsc::channel(self.limits.channel_capacity.max(1));
        let written_seq = Arc::new(AtomicU64::new(0));
        let handle = SessionLogHandle {
            session_id: Arc::from(session_id),
            tx,
            dropped: Arc::new(AtomicU64::new(0)),
            written_seq: written_seq.clone(),
            next_spill: Arc::new(AtomicU64::new(1)),
        };
        tokio::spawn(writer::run(
            session_id.to_string(),
            self.session_dir(session_id),
            rx,
            self.db.clone(),
            self.limits.clone(),
            written_seq,
        ));
        handle
    }
}

/// The emit side of one session's log.
///
/// Cloneable and cheap; [`emit`](Self::emit) is a non-blocking `try_send`. A
/// full channel drops the record and counts it (§5.5) — the log is an
/// observability record, and stalling an agentic loop for it is the wrong
/// trade.
#[derive(Clone)]
pub struct SessionLogHandle {
    session_id: Arc<str>,
    tx: mpsc::Sender<Msg>,
    dropped: Arc<AtomicU64>,
    /// The highest `seq` the writer has actually put on disk. Published by the
    /// writer so [`reserve_spill`](Self::reserve_spill) can name a file after
    /// roughly the record it belongs to without asking the writer — which the
    /// emit path may never wait for.
    written_seq: Arc<AtomicU64>,
    /// The next spill number this boot will hand out, never below
    /// `written_seq + 1`.
    next_spill: Arc<AtomicU64>,
}

/// A handle prints as the session it writes for: `ToolContext` carries one and
/// derives `Debug`, and the channel behind it has nothing worth printing.
impl std::fmt::Debug for SessionLogHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionLogHandle")
            .field("session_id", &self.session_id)
            .field("closed", &self.tx.is_closed())
            .finish()
    }
}

/// What a caller asks the writer to image, before it overwrites the file
/// (§5.7's `snapshots/`).
///
/// `source` is the **resolved** file to copy — the caller has already decided
/// it exists, is a regular file and is inside the workspace. `path` is the
/// workspace-relative path the record is keyed by, which is what a reader
/// recognises the file from; the writer only slugs it into a filename.
#[derive(Debug, Clone)]
pub struct SnapshotSpec {
    pub source: PathBuf,
    pub path: String,
    pub task_id: Option<String>,
    pub span_id: Option<String>,
    pub agent: Option<String>,
}

/// The image the writer took, as its `file_snapshot` record names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileSnapshot {
    /// `snapshots/<seq>-<slug>`, relative to the session directory.
    pub rel: String,
    /// The seq of the `file_snapshot` record that commits it — and the number
    /// in `rel`, which is that seq exactly.
    pub seq: u64,
    pub size: u64,
    pub sha256: String,
}

impl SessionLogHandle {
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Copy `spec.source` into this session's `snapshots/` and commit it with
    /// a `file_snapshot` record — the pre-edit image §5.7 specifies.
    ///
    /// Unlike [`emit`](Self::emit) this **waits**, and it is the only thing in
    /// the log that does. A snapshot is not observability: its caller is about
    /// to destroy the bytes it images, so it has to know whether the image was
    /// taken before it does. The wait is the whole contract — a caller that
    /// gets `Err` must not perform its write.
    ///
    /// Everything that touches the disk happens in the writer task, on its
    /// blocking half, in the same batch discipline as a spill: the copy is
    /// `tmp → fsync → rename` and never overwrites, and the record is written
    /// **after** the bytes are in place. A copy that lands but whose record
    /// does not is reclaimed rather than left behind (R33: a file no record
    /// references is an uncommitted write).
    pub async fn snapshot(&self, spec: SnapshotSpec) -> Result<FileSnapshot, String> {
        let (ack, wait) = oneshot::channel();
        let request = Box::new(writer::SnapshotRequest { spec, ack });
        if self.tx.send(Msg::Snapshot(request)).await.is_err() {
            return Err("the session log writer is gone".to_string());
        }
        match wait.await {
            Ok(outcome) => outcome,
            Err(_) => Err("the session log writer did not answer".to_string()),
        }
    }

    /// Reserve the `results/` reference for a tool result too large to sit
    /// inline, and return it relative to the session directory.
    ///
    /// The **model-visible** stub has to name the file synchronously, on the
    /// loop's own path — the loop cannot wait for the writer, and the writer is
    /// the only thing that assigns a `seq` (that is what keeps the sequence
    /// gap-free). So the number here is the writer's published watermark plus
    /// one, which is the record's seq whenever nothing was dropped in between,
    /// and always at least it. Ordering is all the number carries: what makes
    /// the name **unique** is the call's own `tool_use_id`, and what makes a
    /// trim delete the right files is the reference the record itself holds,
    /// never this prefix (§5.4: "the spill files they reference").
    pub fn reserve_spill(&self, tool_name: &str, tool_use_id: &str) -> String {
        let floor = self.written_seq.load(Ordering::Relaxed) + 1;
        let mut number = floor;
        let _ = self
            .next_spill
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                number = current.max(floor);
                Some(number + 1)
            });
        format!(
            "{}/{number:06}-{}-{}.txt",
            record::RESULTS_DIR,
            call_tag(tool_use_id),
            slug(tool_name),
        )
    }

    /// Queue one record. Returns whether it was accepted.
    pub fn emit(&self, record: Record) -> bool {
        match self.tx.try_send(Msg::Record(record)) {
            Ok(()) => true,
            Err(mpsc::error::TrySendError::Full(msg)) => {
                let dropped = self.dropped.fetch_add(1, Ordering::Relaxed) + 1;
                if let Msg::Record(record) = msg {
                    tracing::warn!(
                        session_id = %self.session_id,
                        kind = record.kind.as_str(),
                        dropped,
                        "Session log channel full — record dropped"
                    );
                }
                false
            }
            // The writer is gone — it could not open its directory, or the
            // service was dropped. Counted like a full channel: a record that
            // never reaches disk is a hole in the log either way, and the
            // "writer died" case being invisible was how it stayed unnoticed.
            Err(mpsc::error::TrySendError::Closed(_)) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
                false
            }
        }
    }

    /// How many records this handle has dropped.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// True once the writer has gone (idle-close, or the service dropped).
    pub fn is_closed(&self) -> bool {
        self.tx.is_closed()
    }

    /// Wait until everything queued before this call is on disk.
    ///
    /// A barrier for tests and for shutdown — never on the emit path, which
    /// must not wait for the log.
    pub async fn flush(&self) -> bool {
        let (ack, wait) = oneshot::channel();
        if self.tx.send(Msg::Sync(ack)).await.is_err() {
            return false;
        }
        wait.await.is_ok()
    }
}

/// Eight characters that identify the call a spill belongs to.
///
/// Provider tool-use ids share a prefix (`toolu_01…`, `call_…`), so the tail is
/// what distinguishes them; a provider that issues none (Ollama) gets a random
/// tag rather than a shared one, so two id-less calls never claim one file.
fn call_tag(tool_use_id: &str) -> String {
    let cleaned: String = tool_use_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    if cleaned.is_empty() {
        return uuid::Uuid::new_v4().simple().to_string()[..8].to_string();
    }
    let start = cleaned.len().saturating_sub(8);
    cleaned[start..].to_string()
}

/// A tool name reduced to a filename-safe slug.
fn slug(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .take(48)
        .collect();
    if cleaned.is_empty() {
        "tool".to_string()
    } else {
        cleaned
    }
}

/// The name §5.7 gives a pre-edit image: `snapshots/<seq>-<slug>`.
///
/// The number is not a reservation the way a spill's is — it is the `seq` of
/// the `file_snapshot` record itself, which the writer knows because it is the
/// thing that assigns seqs and it builds this name inside the same turn it
/// writes the record. So the file and its record name each other, in both
/// directions, with nothing to drift.
pub(crate) fn snapshot_name(seq: u64, path: &str) -> String {
    format!("{}/{seq:06}-{}", record::SNAPSHOTS_DIR, path_slug(path))
}

/// A workspace-relative path reduced to one filename-safe segment.
///
/// The **tail** is kept rather than the head: a deep path's identity is its
/// file name, and `src/routes/handlers/…` would otherwise be all that survived.
fn path_slug(path: &str) -> String {
    let cleaned: String = path
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    if cleaned.is_empty() {
        return "file".to_string();
    }
    let start = cleaned.len().saturating_sub(48);
    cleaned[start..].to_string()
}

/// The directory name a session's log lives under, relative to the sessions
/// root — the one place an id becomes a path segment.
///
/// Public because `read_result` (T42) and the boot sweep resolve a session's
/// `results/` from an id and must use the identical mapping;
/// [`SessionLogService::session_dir`] is this joined onto the root.
pub fn session_dir_name(session_id: &str) -> String {
    safe_dir_name(session_id)
}

/// Reduce a session id to one safe path segment.
///
/// Session ids are UUIDs, so this never fires in practice; it exists so that
/// no future caller can turn an id into a path (`..`, `a/b`) that escapes the
/// sessions root.
fn safe_dir_name(session_id: &str) -> String {
    let cleaned: String = session_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    match cleaned.trim_matches('.') {
        "" => "_unnamed".to_string(),
        _ if cleaned == "." || cleaned == ".." => format!("_{cleaned}"),
        _ => cleaned,
    }
}

/// The sessions root a service should be built on: the **home** store's
/// `sessions/`, never a project directory.
pub fn default_root() -> anyhow::Result<PathBuf> {
    openalpaca_storage::store::sessions_dir()
}

/// True when `path` is inside `root` — used by the callers that resolve a
/// spill reference (T42) and by the sweep (T43).
pub fn is_within(root: &Path, path: &Path) -> bool {
    path.starts_with(root)
}

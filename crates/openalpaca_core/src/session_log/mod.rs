//! The per-session JSONL event log (plan §5.4/§5.5).
//!
//! ```text
//! ~/.openalpaca/sessions/<session-id>/
//!   log.jsonl                    ← the live segment
//!   log.<first>-<last>.jsonl     ← rotated segments
//!   results/                     ← spilled tool results (T42)
//!   snapshots/                   ← reserved (Phase 8)
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
mod writer;

#[cfg(test)]
mod tests;

pub use reader::{LIVE_SEGMENT, LoggedRecord, read_records, read_records_after, segments};
pub use record::{
    ENVELOPE_DATA_CAP_BYTES, ENVELOPE_VERSION, PREVIEW_CHARS, RESULTS_DIR, Record, RecordType,
    Spill, spill_preview, spill_stub,
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
        }
    }

    /// This daemon run's id, stamped on every `session_start`.
    pub fn boot_id(&self) -> &str {
        &self.boot_id
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

    /// Records dropped across every session this boot has written.
    pub fn dropped_total(&self) -> u64 {
        self.handles.iter().map(|h| h.value().dropped()).sum()
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

impl SessionLogHandle {
    pub fn session_id(&self) -> &str {
        &self.session_id
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

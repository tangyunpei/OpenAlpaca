//! The per-session writer task: seq assignment, durability, rotation, the
//! size cap, and the `tool_execution_log` index row (§5.4).
//!
//! One task per session. It is the only thing that assigns a `seq`, which is
//! what makes the sequence gap-free: a record dropped by a full channel never
//! reaches here, so it never consumes a number.

use super::record::{
    RESULTS_DIR, Record, RecordType, SNAPSHOTS_DIR, Spill, cap_data, spill_preview,
};
use super::reader::{LIVE_SEGMENT, segment_first_seq, segment_range};
use super::{FileSnapshot, SnapshotSpec, snapshot_name};
use openalpaca_storage::{Database, SkillExecutionRepository, ToolExecutionEntry};
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

/// How much of a torn live segment is scanned for the last complete record.
///
/// A record's `data` is capped at 64 KB, so a 1 MiB tail always contains a
/// whole record unless the file is corrupt beyond a torn tail — which is the
/// case that moves the file aside instead of truncating it.
const TAIL_SCAN_BYTES: u64 = 1024 * 1024;

/// How many in-flight tool calls a writer remembers while waiting for their
/// results. A call whose result never arrives (a crash, a cancelled round)
/// falls out of the window instead of leaking.
const PENDING_CALLS_CAP: usize = 256;

/// Bounds the writer works to. Everything §5.4 states as a number lives here
/// so tests can shrink it.
#[derive(Debug, Clone)]
pub struct SessionLogLimits {
    /// §5.4: "Rotation at 64 MB → `log.<first>-<last>.jsonl`".
    pub rotate_bytes: u64,
    /// §5.4's `log_max_session_bytes` (default 256 MB): segments plus
    /// `results/`, enforced in the writer on rotation.
    pub max_session_bytes: u64,
    /// Depth of the emit channel. A full channel drops (§5.5).
    pub channel_capacity: usize,
    /// §5.4: "a 5 s timer while dirty".
    pub sync_interval: Duration,
    /// §5.5: "idle-close after N minutes".
    pub idle_close: Duration,
}

impl Default for SessionLogLimits {
    fn default() -> Self {
        Self {
            rotate_bytes: 64 * 1024 * 1024,
            max_session_bytes: 256 * 1024 * 1024,
            channel_capacity: 1024,
            sync_interval: Duration::from_secs(5),
            idle_close: Duration::from_secs(300),
        }
    }
}

/// What a handle sends the writer.
pub(super) enum Msg {
    Record(Record),
    /// Write everything queued, then answer — the barrier tests and a
    /// shutdown use to know the log is on disk.
    Sync(oneshot::Sender<()>),
    /// Copy a file into `snapshots/`, commit it with a `file_snapshot`
    /// record, and answer — the one request whose sender waits, because it is
    /// about to overwrite the bytes it asked to be imaged (§5.7).
    Snapshot(Box<SnapshotRequest>),
}

/// A pre-edit image request and the channel its answer goes back on.
///
/// Boxed so one variant's payload does not widen every queued [`Msg`].
pub(super) struct SnapshotRequest {
    pub spec: SnapshotSpec,
    pub ack: oneshot::Sender<Result<FileSnapshot, String>>,
}

/// How many queued messages one blocking hand-off carries at most. The
/// channel holds `channel_capacity` (1024); a bound keeps a single blocking
/// call from monopolising a pool thread while still amortising the hand-off
/// over a whole round's worth of records.
const MAX_BATCH: usize = 256;

/// The state one session's writer owns between wakes — and the only thing
/// that touches the filesystem or the database (R52).
struct Writer {
    session_id: String,
    dir: PathBuf,
    db: Option<Database>,
    limits: SessionLogLimits,
    log: Option<OpenLog>,
    pending: PendingCalls,
    /// Published for the handle's spill reservation: the highest seq actually
    /// on disk (§5.4 — the emitter names a spill file without waiting here).
    written_seq: Arc<AtomicU64>,
    /// Set once the directory could not be opened: the writer is done, and
    /// dropping its receiver tells every handle so.
    gave_up: bool,
}

impl Writer {
    fn dirty(&self) -> bool {
        self.log.as_ref().is_some_and(|l| l.dirty)
    }

    /// Write a batch, then sync if the caller asked (the 5 s timer, or the
    /// final pass). Runs on a blocking thread — every `write`, `sync_data`
    /// and SQLite statement in the session log happens inside here.
    fn process(&mut self, batch: Vec<Msg>, sync_after: bool) {
        for msg in batch {
            match msg {
                Msg::Record(record) => {
                    if !self.ensure_log() {
                        continue;
                    }
                    let Some(log) = self.log.as_mut() else { continue };
                    write_record(
                        &self.session_id,
                        log,
                        record,
                        &self.db,
                        &mut self.pending,
                        &self.limits,
                        &self.written_seq,
                    );
                }
                Msg::Sync(ack) => {
                    self.sync("sync");
                    let _ = ack.send(());
                }
                Msg::Snapshot(request) => {
                    let SnapshotRequest { spec, ack } = *request;
                    let outcome = if self.ensure_log() {
                        match self.log.as_mut() {
                            Some(log) => take_snapshot(
                                &self.session_id,
                                log,
                                spec,
                                &self.db,
                                &mut self.pending,
                                &self.limits,
                                &self.written_seq,
                            ),
                            None => Err("this session's log is unavailable".to_string()),
                        }
                    } else {
                        Err("this session's log is unavailable".to_string())
                    };
                    // The caller is waiting on this to decide whether to write.
                    let _ = ack.send(outcome);
                }
            }
        }
        if sync_after {
            self.sync("timer sync");
        }
    }

    /// Open the live segment if it is not open yet, and say whether the writer
    /// has one to work with.
    ///
    /// §5.4/P-22: the directory is created by the first thing written into it,
    /// never by asking for a handle — which is why this sits on the message
    /// path rather than in [`run`].
    fn ensure_log(&mut self) -> bool {
        if self.gave_up {
            return false;
        }
        if self.log.is_some() {
            return true;
        }
        match OpenLog::open(&self.dir) {
            Ok(opened) => {
                // A reopened session resumes its numbering, so the handle's
                // spill reservations must resume with it rather than
                // restarting at 1.
                self.written_seq
                    .fetch_max(opened.next_seq.saturating_sub(1), Ordering::Relaxed);
                self.log = Some(opened);
                true
            }
            Err(e) => {
                tracing::warn!(
                    session_id = self.session_id,
                    dir = %self.dir.display(),
                    "Session log unavailable, dropping records: {e}"
                );
                self.gave_up = true;
                false
            }
        }
    }

    fn sync(&mut self, what: &str) {
        if let Some(ref mut log) = self.log
            && let Err(e) = log.sync()
        {
            tracing::warn!(session_id = self.session_id, "Session log {what} failed: {e}");
        }
    }
}

/// Run one session's writer until its channel closes or it goes idle.
///
/// The async half only waits: it takes a message, drains whatever else is
/// already queued, and hands the whole batch to a blocking thread (R52). The
/// loop it serves must never wait for the log, and a runtime worker must
/// never wait for a disk or for the daemon's single connection mutex.
pub(super) async fn run(
    session_id: String,
    dir: PathBuf,
    mut rx: mpsc::Receiver<Msg>,
    db: Option<Database>,
    limits: SessionLogLimits,
    written_seq: Arc<AtomicU64>,
) {
    let sync_interval = limits.sync_interval;
    let idle_close = limits.idle_close;
    let mut writer = Writer {
        session_id,
        dir,
        db,
        limits,
        log: None,
        pending: PendingCalls::default(),
        written_seq,
        gave_up: false,
    };
    let mut dirty = false;

    loop {
        let wait = if dirty { sync_interval } else { idle_close };
        let first = match tokio::time::timeout(wait, rx.recv()).await {
            Ok(Some(msg)) => msg,
            // Every handle is gone.
            Ok(None) => break,
            // The 5 s timer, with something unsynced.
            Err(_elapsed) if dirty => {
                let Some(next) = pump(writer, Vec::new(), true).await else {
                    return;
                };
                writer = next;
                dirty = writer.dirty();
                continue;
            }
            // Idle with nothing unsynced: close the file and let the next
            // emit respawn the task.
            Err(_elapsed) => break,
        };

        // Everything already queued rides along, so one blocking hand-off
        // covers a whole round's records rather than one each.
        let mut batch = vec![first];
        while batch.len() < MAX_BATCH {
            match rx.try_recv() {
                Ok(msg) => batch.push(msg),
                Err(_) => break,
            }
        }
        let Some(next) = pump(writer, batch, false).await else {
            return;
        };
        writer = next;
        if writer.gave_up {
            return;
        }
        dirty = writer.dirty();
    }

    let _ = pump(writer, Vec::new(), true).await;
}

/// Hand the writer and its batch to a blocking thread and take it back.
///
/// `None` when that thread panicked: the writer is gone, and returning drops
/// the receiver so every handle sees a closed channel instead of queueing
/// into nothing.
async fn pump(mut writer: Writer, batch: Vec<Msg>, sync_after: bool) -> Option<Writer> {
    match tokio::task::spawn_blocking(move || {
        writer.process(batch, sync_after);
        writer
    })
    .await
    {
        Ok(writer) => Some(writer),
        Err(e) => {
            tracing::error!("Session log writer task failed: {e}");
            None
        }
    }
}

/// Write one record, then do everything that hangs off having written it:
/// the index row, the rotation, the trim.
///
/// Returns the `seq` the record was given, or `None` when it could not be
/// written — which is what lets a snapshot reclaim the file it had already
/// copied (R33: bytes no record references are an uncommitted write).
fn write_record(
    session_id: &str,
    log: &mut OpenLog,
    record: Record,
    db: &Option<Database>,
    pending: &mut PendingCalls,
    limits: &SessionLogLimits,
    written_seq: &Arc<AtomicU64>,
) -> Option<u64> {
    let kind = record.kind;
    // Both of the things that put a *file* beside the log, and so the two
    // record kinds after which the per-session cap has bytes it has not seen.
    let grew_files = record.spill.is_some() || kind == RecordType::FileSnapshot;
    // The spill runs before the cap: once the payload is a reference plus a
    // preview there is nothing left for the 64 KB envelope bound to cut, which
    // is §5.4's "spilled results never sit inline".
    let record = spill_result(session_id, log, record);
    let (data, truncated) = cap_data(record.data);
    let mut record = Record { data, ..record };

    // P-14: `preserved_from_seq` is the last seq the log already holds, so it
    // is the writer's to stamp — the same reason `log_seq` is. An emitter can
    // only guess: records it queued may not be written yet, and a dropped one
    // never consumes a number. Nothing precedes the first record, so there is
    // no boundary to name and the field stays null.
    if kind == RecordType::Compaction
        && let Some(map) = record.data.as_object_mut()
    {
        let preserved = log.next_seq.saturating_sub(1);
        map.insert(
            "preserved_from_seq".into(),
            if preserved == 0 {
                Value::Null
            } else {
                Value::from(preserved)
            },
        );
    }
    if truncated {
        // The marker T42 greps for: every site that must gain a `results/`
        // spill announces itself once, with the session and the kind.
        tracing::warn!(
            session_id,
            kind = kind.as_str(),
            spilled_pending = true,
            "Session log payload exceeded the 64 KB envelope cap and was truncated inline"
        );
    }

    let seq = match log.write(&record) {
        Ok(seq) => seq,
        Err(e) => {
            tracing::warn!(session_id, kind = kind.as_str(), "Session log write failed: {e}");
            return None;
        }
    };
    written_seq.fetch_max(seq, Ordering::Relaxed);

    index_tool_call(session_id, &record, seq, db, pending);

    // A spill grows `results/` and a snapshot grows `snapshots/`, both of
    // which the per-session cap counts (§5.4, §5.7). The rotation check alone
    // would never see either: a session can fill them while its live segment
    // stays far below the rotation bound. Gated on the two kinds that write a
    // file so the ordinary record path keeps §5.4's "cheap, no scan".
    let mut trim = match grew_files.then(|| log.enforce_cap_now(limits)) {
        Some(Ok(trimmed)) => trimmed,
        Some(Err(e)) => {
            tracing::warn!(session_id, "Session cap enforcement failed: {e}");
            None
        }
        None => None,
    };

    match log.rotate_if_needed(limits) {
        Ok(Some(dropped)) => trim = Some(merge_trims(trim.take(), dropped)),
        Ok(None) => {}
        Err(e) => tracing::warn!(session_id, "Session log rotation failed: {e}"),
    }

    if let Some(dropped) = trim {
        // The trim is itself a record (§5.4): the log says what it lost.
        let notice = Record::new(RecordType::LogTrimmed).with_data(serde_json::json!({
            "from_seq": dropped.from_seq,
            "to_seq": dropped.to_seq,
            "segments": dropped.segments,
            "bytes_freed": dropped.bytes_freed,
        }));
        if let Err(e) = log.write(&notice) {
            tracing::warn!(session_id, "Failed to record the log trim: {e}");
        }
    }
    Some(seq)
}

/// Fold two trims into the one `log_trimmed` record they are owed.
fn merge_trims(previous: Option<Trimmed>, next: Trimmed) -> Trimmed {
    match previous {
        None => next,
        Some(prev) => Trimmed {
            from_seq: prev.from_seq.min(next.from_seq),
            to_seq: prev.to_seq.max(next.to_seq),
            segments: prev.segments + next.segments,
            bytes_freed: prev.bytes_freed + next.bytes_freed,
        },
    }
}

// ── The `results/` spill ────────────────────────────────────────────

/// Put an oversized tool result in `results/` and leave the record holding the
/// reference plus a preview (§5.4's "Spill, don't truncate").
///
/// The file is written with T26/T29's protocol — a temporary sibling, `fsync`,
/// then `rename` — so a reader never sees a half-written result, and an
/// existing file is **never overwritten**: the reference names one call, so a
/// second arrival is the same bytes and re-writing them would only risk
/// clobbering what the model was already told to read.
fn spill_result(session_id: &str, log: &OpenLog, record: Record) -> Record {
    let Some(Spill { rel, content }) = record.spill.clone() else {
        return record;
    };
    let mut record = Record { spill: None, ..record };

    let bytes = content.len();
    let preview = spill_preview(&content);
    let sha256 = sha256_hex(content.as_bytes());

    match write_spill_file(&log.dir, &rel, content.as_bytes()) {
        Ok(()) => {
            if let Some(map) = record.data.as_object_mut() {
                // One copy of the payload: the file. The record keeps what the
                // expanded-turn view needs without touching `results/`.
                map.insert(
                    "result".into(),
                    serde_json::json!({
                        "spill": {
                            "rel": rel,
                            "bytes": bytes,
                            "sha256": sha256,
                            "mime": SPILL_MIME,
                        },
                        "preview": preview,
                    }),
                );
                map.insert("result_ref".into(), Value::from(format!("file:{rel}")));
            }
        }
        Err(e) => {
            // The result is not lost: the record keeps the preview and says
            // the spill failed, which is more honest than a `result_ref` to a
            // file that is not there. The model, though, was handed the stub
            // on the loop's own path long before this ran — nothing here can
            // reach it. So the record also names the reference that was **not**
            // honoured (`spill_ref`, deliberately not `result_ref`), which is
            // what lets `read_result` answer that page request honestly.
            tracing::warn!(session_id, rel = %rel, "Failed to spill a tool result: {e}");
            if let Some(map) = record.data.as_object_mut() {
                map.insert("result".into(), Value::from(preview));
                map.insert("spill_error".into(), Value::from(e.to_string()));
                map.insert("spill_ref".into(), Value::from(rel));
            }
        }
    }
    record
}

/// Every spilled tool result is text: the sandbox hands the loop a `String`.
const SPILL_MIME: &str = "text/plain; charset=utf-8";

// ── The `snapshots/` pre-edit image (§5.7) ──────────────────────────

/// Copy a file into `snapshots/` and commit it with a `file_snapshot` record.
///
/// **The name is the record's own seq.** The writer is the only thing that
/// assigns seqs and it builds the filename in the same turn it writes the
/// record, so — unlike a spill, whose emitter must name a file it cannot yet
/// number — `snapshots/<seq>-<slug>` and the record at `seq` name each other
/// exactly.
///
/// **Bytes first, record second, and the record is the commit.** The copy is
/// T26/T29's protocol (`tmp` → `fsync` → `rename`, never overwriting), so a
/// reader never sees a half-written image; the record is written after the
/// bytes are in place, so a record never names a file that is not there. If
/// the record cannot be written the copy is reclaimed (R33: bytes no record
/// references are an uncommitted write) and the caller is refused — which is
/// what makes it safe for the caller to treat `Ok` as "the old bytes are
/// saved, go ahead and destroy them".
fn take_snapshot(
    session_id: &str,
    log: &mut OpenLog,
    spec: SnapshotSpec,
    db: &Option<Database>,
    pending: &mut PendingCalls,
    limits: &SessionLogLimits,
    written_seq: &Arc<AtomicU64>,
) -> Result<FileSnapshot, String> {
    let seq = log.next_seq;
    let rel = snapshot_name(seq, &spec.path);
    let (size, sha256) = write_snapshot_file(&log.dir, &rel, &spec.source).map_err(|e| {
        tracing::warn!(session_id, path = %spec.path, rel = %rel, "Failed to snapshot a file: {e}");
        format!("could not copy '{}' into {rel}: {e}", spec.path)
    })?;

    let record = Record::new(RecordType::FileSnapshot)
        .task(spec.task_id.as_deref())
        .span(spec.span_id.as_deref())
        .agent(spec.agent.as_deref())
        .with_data(serde_json::json!({
            "path": spec.path,
            "snapshot_ref": format!("file:{rel}"),
            "size": size,
            "sha256": sha256,
            "seq": seq,
        }));

    match write_record(session_id, log, record, db, pending, limits, written_seq) {
        Some(written) => {
            debug_assert_eq!(written, seq, "the snapshot is named after its own record");
            Ok(FileSnapshot {
                rel,
                seq: written,
                size,
                sha256,
            })
        }
        None => {
            // R33: the record is the commit, so bytes without one are an
            // uncommitted write and are reclaimed rather than left to fill a
            // cap that can never account for them.
            let orphan = log.dir.join(&rel);
            match fs::remove_file(&orphan) {
                Ok(()) => tracing::warn!(
                    session_id,
                    rel = %rel,
                    "Reclaimed a snapshot whose record could not be written"
                ),
                Err(e) => tracing::warn!(
                    session_id,
                    rel = %rel,
                    "Could not reclaim a snapshot whose record failed: {e}"
                ),
            }
            Err(format!(
                "the snapshot of '{}' could not be recorded in the session log",
                spec.path
            ))
        }
    }
}

/// Stream `source` into `dir/rel`, returning its size and content hash.
///
/// Streamed rather than read whole: the caller bounds the file's size against
/// `snapshot_max_bytes`, but the writer must not be the place where that bound
/// being wrong costs the daemon its memory.
fn write_snapshot_file(dir: &Path, rel: &str, source: &Path) -> io::Result<(u64, String)> {
    use sha2::{Digest, Sha256};

    let target = dir.join(rel);
    if target.exists() {
        // Never overwrite: this name belongs to one record, and a second
        // arrival would mean the seq had been reused — which is a reason to
        // refuse, not to clobber an image something else is accounted for.
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("{rel} already exists"),
        ));
    }
    let parent = target
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "snapshot has no parent"))?;

    // The source is opened **before** `snapshots/` is created, so a refusal
    // leaves the session exactly as it found it.
    let mut input = File::open(source)?;
    if !input.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "only a regular file has a pre-edit image",
        ));
    }

    fs::create_dir_all(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
    }
    let tmp = parent.join(format!(
        ".{}.tmp",
        target
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "snapshot".to_string())
    ));

    let copied = (|| -> io::Result<(u64, String)> {
        let mut out = File::create(&tmp)?;
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; 64 * 1024];
        let mut size = 0u64;
        loop {
            let read = input.read(&mut buf)?;
            if read == 0 {
                break;
            }
            hasher.update(&buf[..read]);
            out.write_all(&buf[..read])?;
            size += read as u64;
        }
        out.sync_data()?;
        let digest = hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        Ok((size, digest))
    })();

    match copied {
        Ok(image) => match fs::rename(&tmp, &target) {
            Ok(()) => Ok(image),
            Err(e) => {
                let _ = fs::remove_file(&tmp);
                Err(e)
            }
        },
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            Err(e)
        }
    }
}

fn write_spill_file(dir: &Path, rel: &str, bytes: &[u8]) -> io::Result<()> {
    let target = dir.join(rel);
    if target.exists() {
        // Never overwrite: the reference names one call, and the model may
        // already be paging the bytes that are there.
        return Ok(());
    }
    let parent = target
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "spill has no parent"))?;
    fs::create_dir_all(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
    }
    let tmp = parent.join(format!(
        ".{}.tmp",
        target
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "spill".to_string())
    ));
    {
        let mut file = File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_data()?;
    }
    match fs::rename(&tmp, &target) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            Err(e)
        }
    }
}

/// The spill's content hash, as §5.4's stub carries it.
fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

// ── The tool-call index row ─────────────────────────────────────────

#[derive(Default)]
struct PendingCalls {
    by_id: HashMap<String, PendingCall>,
    order: VecDeque<String>,
}

struct PendingCall {
    seq: u64,
    tool_name: String,
    args_preview: String,
}

impl PendingCalls {
    fn insert(&mut self, id: String, call: PendingCall) {
        if self.by_id.insert(id.clone(), call).is_none() {
            self.order.push_back(id);
        }
        while self.order.len() > PENDING_CALLS_CAP {
            if let Some(old) = self.order.pop_front() {
                self.by_id.remove(&old);
            }
        }
    }

    fn take(&mut self, id: &str) -> Option<PendingCall> {
        let call = self.by_id.remove(id)?;
        self.order.retain(|k| k != id);
        Some(call)
    }
}

/// `tool_execution_log` is the index over the log (§5.4): the `log_seq` half
/// of the row is written by the same writer that assigned the seq, so it can
/// never point at a record that does not exist.
///
/// It is an **update**, not an insert (R51): the daemon's audit path already
/// wrote this call's row unconditionally, keyed by the same `tool_use_id`, and
/// this adds the half only the writer knows. When the audit row is not there —
/// the event was lost, or the call never reached the sandbox — the update
/// inserts instead, so the index is never silently empty.
///
/// The previews are taken from the **capped** payload — the bytes that
/// actually landed inline — so the row and the record agree even when the
/// envelope cap cut the result.
fn index_tool_call(
    session_id: &str,
    record: &Record,
    seq: u64,
    db: &Option<Database>,
    pending: &mut PendingCalls,
) {
    // An empty id is not an id. A provider that issues none (Ollama leaves
    // `id` as `unwrap_or_default()`) is excluded from the R51 merge on
    // purpose, so a row written here could never find the daemon's audit row
    // for the same call — it would simply be a second row, double-counting the
    // call in `GET /v1/tools`' `invocations_today`. The audit row stands alone.
    let Some(id) = record
        .data
        .get("tool_use_id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
    else {
        return;
    };
    match record.kind {
        RecordType::ToolCall => {
            let args_preview = record
                .data
                .get("input")
                .map(|v| match v.as_str() {
                    Some(s) => s.to_string(),
                    None => v.to_string(),
                })
                .unwrap_or_default();
            pending.insert(
                id.to_string(),
                PendingCall {
                    seq,
                    tool_name: string_field(&record.data, "name"),
                    args_preview,
                },
            );
        }
        RecordType::ToolResult => {
            let Some(db) = db.as_ref() else { return };
            let call = pending.take(id);
            let entry = ToolExecutionEntry {
                // The key the daemon's audit row carries for this call.
                request_id: Some(id.to_string()),
                agent_id: record.agent.clone().unwrap_or_else(|| "unknown".to_string()),
                tool_name: call
                    .as_ref()
                    .map(|c| c.tool_name.clone())
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| string_field(&record.data, "name")),
                success: record.data.get("ok").and_then(Value::as_bool).unwrap_or(false),
                duration_ms: record
                    .data
                    .get("duration_ms")
                    .and_then(Value::as_i64)
                    .unwrap_or(0),
                error_message: record
                    .data
                    .get("error")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                session_id: Some(session_id.to_string()),
                task_id: record.task_id.clone(),
                // The `tool_call` record carries the full arguments; without
                // one (a result whose call fell out of the window) there is
                // nothing honest to point at.
                log_seq: call.as_ref().map(|c| c.seq as i64),
                args_preview: call.map(|c| c.args_preview),
                // The bytes that actually landed in the record: the whole
                // result when it sat inline, the spill's preview when it did
                // not. Either way the row and the record agree, and the row
                // still renders a turn after the spill file has been evicted.
                result_preview: result_preview(&record.data),
                // §5.4: a spilled result's row points at the same file the
                // record and the model's stub name; an inline one points back
                // at the record that holds it.
                result_ref: Some(
                    record
                        .data
                        .get("result_ref")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .unwrap_or_else(|| format!("log:{seq}")),
                ),
                ..Default::default()
            };
            if let Err(e) = SkillExecutionRepository::new(db).attach_session_index(&entry) {
                tracing::warn!(session_id, "Failed to index a tool call: {e}");
            }
        }
        _ => {}
    }
}

/// The preview the index row stores: a `tool_result`'s inline string, or the
/// preview a spill left behind in its place.
fn result_preview(data: &Value) -> Option<String> {
    match data.get("result") {
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Object(spilled)) => spilled
            .get("preview")
            .and_then(Value::as_str)
            .map(str::to_string),
        _ => None,
    }
}

fn string_field(data: &Value, key: &str) -> String {
    data.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

// ── The open live segment ───────────────────────────────────────────

struct OpenLog {
    dir: PathBuf,
    path: PathBuf,
    file: BufWriter<File>,
    bytes: u64,
    first_seq: Option<u64>,
    next_seq: u64,
    dirty: bool,
}

/// What a trim removed, for the `log_trimmed` record.
struct Trimmed {
    from_seq: u64,
    to_seq: u64,
    segments: usize,
    bytes_freed: u64,
}

impl OpenLog {
    /// Create the session directory (this is the first write — P-22), repair
    /// a torn tail, and resume the seq.
    fn open(dir: &Path) -> io::Result<Self> {
        fs::create_dir_all(dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
        }
        let path = dir.join(LIVE_SEGMENT);
        let (last_seq, first_seq) = if path.exists() {
            (repair_tail(&path)?, head_seq(&path)?)
        } else {
            (None, None)
        };
        // An empty (or freshly truncated) live segment resumes from the
        // newest archived segment's name rather than from 1.
        let next_seq = match last_seq {
            Some(seq) => seq + 1,
            None => archived_last_seq(dir)? + 1,
        };
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        let bytes = file.metadata()?.len();
        Ok(Self {
            dir: dir.to_path_buf(),
            path,
            file: BufWriter::new(file),
            bytes,
            first_seq,
            next_seq,
            dirty: false,
        })
    }

    /// Append one record. §5.4's durability policy lives here: a `write`
    /// syscall after every record, `sync_data` only at the declared
    /// boundaries (the timer in [`run`] covers the rest).
    fn write(&mut self, record: &Record) -> io::Result<u64> {
        let seq = self.next_seq;
        let line = record.to_line(seq);
        self.file.write_all(line.as_bytes())?;
        self.file.flush()?;
        self.next_seq += 1;
        self.bytes += line.len() as u64;
        if self.first_seq.is_none() {
            self.first_seq = Some(seq);
        }
        if record.kind.is_durability_boundary() {
            self.file.get_ref().sync_data()?;
            self.dirty = false;
        } else {
            self.dirty = true;
        }
        Ok(seq)
    }

    fn sync(&mut self) -> io::Result<()> {
        self.file.flush()?;
        self.file.get_ref().sync_data()?;
        self.dirty = false;
        Ok(())
    }

    /// Rotate at the segment cap and then enforce the per-session byte cap —
    /// §5.4 puts both in the writer "on rotation (cheap, no scan)".
    fn rotate_if_needed(&mut self, limits: &SessionLogLimits) -> io::Result<Option<Trimmed>> {
        if self.bytes < limits.rotate_bytes {
            return Ok(None);
        }
        self.sync()?;
        let first = self.first_seq.unwrap_or(self.next_seq.saturating_sub(1));
        let last = self.next_seq.saturating_sub(1);
        let archived = self.dir.join(format!("log.{first}-{last}.jsonl"));
        fs::rename(&self.path, &archived)?;
        let file = OpenOptions::new().create(true).append(true).open(&self.path)?;
        self.file = BufWriter::new(file);
        self.bytes = 0;
        self.first_seq = None;
        self.dirty = false;
        enforce_session_cap(&self.dir, limits.max_session_bytes)
    }

    /// Enforce the per-session cap without waiting for a rotation.
    ///
    /// §5.4 counts `results/` inside `log_max_session_bytes`, and a session can
    /// fill it with spilled results while its live segment stays far below the
    /// rotation bound — so the cheap "on rotation" check alone would never see
    /// the bytes that grew. Only ever called after a spill wrote a file.
    fn enforce_cap_now(&self, limits: &SessionLogLimits) -> io::Result<Option<Trimmed>> {
        enforce_session_cap(&self.dir, limits.max_session_bytes)
    }
}

/// Drop whole oldest **archived** segments (never the live one) until the
/// session fits its cap, taking the `results/` spill files their seq range
/// owns with them (§5.4).
fn enforce_session_cap(dir: &Path, max_bytes: u64) -> io::Result<Option<Trimmed>> {
    let mut archived: Vec<(u64, u64, PathBuf, u64)> = Vec::new();
    let mut total: u64 = 0;
    for entry in fs::read_dir(dir)?.flatten() {
        let len = entry.metadata().map(|m| m.len()).unwrap_or(0);
        let name = entry.file_name().to_string_lossy().into_owned();
        if entry.path().is_dir() {
            total += dir_size(&entry.path());
            continue;
        }
        total += len;
        if name == LIVE_SEGMENT {
            continue;
        }
        if let Some((first, last)) = segment_range(&name) {
            archived.push((first, last, entry.path(), len));
        }
    }
    if total <= max_bytes || archived.is_empty() {
        return Ok(None);
    }
    archived.sort_by_key(|(first, ..)| *first);

    let mut trimmed: Option<Trimmed> = None;
    for (first, last, path, len) in archived {
        if total <= max_bytes {
            break;
        }
        // Read the references out *before* deleting the segment that holds
        // them — §5.4 drops "the spill files they reference", and the segment
        // is the only thing that knows which those are.
        let refs = referenced_files_in(&path);
        if fs::remove_file(&path).is_err() {
            continue;
        }
        total = total.saturating_sub(len);
        let freed_spill = drop_spilled_results(dir, &refs);
        total = total.saturating_sub(freed_spill);
        trimmed = Some(match trimmed {
            None => Trimmed {
                from_seq: first,
                to_seq: last,
                segments: 1,
                bytes_freed: len + freed_spill,
            },
            Some(prev) => Trimmed {
                from_seq: prev.from_seq.min(first),
                to_seq: prev.to_seq.max(last),
                segments: prev.segments + 1,
                bytes_freed: prev.bytes_freed + len + freed_spill,
            },
        });
    }
    Ok(trimmed)
}

/// Every file beside the log that a segment's records point at — the `results/`
/// spills of §5.4's "the spill files they reference" and the `snapshots/`
/// images of §5.7, which are bounded by the same caps and go with the same
/// segment. Streamed, so a 64 MiB segment is not materialised.
///
/// Reading the references rather than parsing a seq out of the filename is
/// what makes the trim exact: the numeric prefix a spill is named after is the
/// emitter's watermark reservation, which is ordering, not identity.
pub(super) fn referenced_files_in(segment: &Path) -> Vec<String> {
    let Ok(file) = File::open(segment) else {
        return Vec::new();
    };
    let mut refs = Vec::new();
    use std::io::BufRead;
    for line in io::BufReader::new(file).lines() {
        let Ok(line) = line else { break };
        let Ok(record) = serde_json::from_str::<super::reader::LoggedRecord>(&line) else {
            // §5.4: an unparseable line is end-of-log for this segment.
            break;
        };
        if let Some(rel) = spill_ref_of(&record.data) {
            refs.push(rel);
        }
        if let Some(rel) = snapshot_ref_of(&record.data) {
            refs.push(rel);
        }
    }
    refs
}

/// `data.result_ref` as a session-relative path, when it names a spill file.
pub(super) fn spill_ref_of(data: &Value) -> Option<String> {
    sibling_ref(data, "result_ref", RESULTS_DIR)
}

/// `data.snapshot_ref` as a session-relative path, when it names a pre-edit
/// image.
pub(super) fn snapshot_ref_of(data: &Value) -> Option<String> {
    sibling_ref(data, "snapshot_ref", SNAPSHOTS_DIR)
}

/// One `file:<dir>/<name>` reference, validated back into `<dir>/<name>`.
///
/// A reference is one filename directly under `dir`; anything else is not one
/// this writer produced, and is left alone rather than guessed at.
fn sibling_ref(data: &Value, key: &str, dir: &str) -> Option<String> {
    let raw = data.get(key).and_then(Value::as_str)?;
    let rel = raw.strip_prefix("file:")?;
    let rel = rel.strip_prefix(&format!("{dir}/"))?;
    if rel.is_empty() || rel.contains('/') || rel.contains("..") {
        return None;
    }
    Some(format!("{dir}/{rel}"))
}

/// Delete the files a dropped segment referenced, returning the bytes freed.
/// A file already gone (a re-run of the same trim) is not an error.
fn drop_spilled_results(dir: &Path, refs: &[String]) -> u64 {
    let mut freed = 0;
    for rel in refs {
        let path = dir.join(rel);
        let len = path.metadata().map(|m| m.len()).unwrap_or(0);
        if fs::remove_file(&path).is_ok() {
            freed += len;
        }
    }
    freed
}

fn dir_size(dir: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .map(|e| {
            let path = e.path();
            if path.is_dir() {
                dir_size(&path)
            } else {
                e.metadata().map(|m| m.len()).unwrap_or(0)
            }
        })
        .sum()
}

// ── Torn tails ──────────────────────────────────────────────────────

/// Truncate a torn tail and report the last complete record's seq.
///
/// §5.4: "the writer truncates a torn tail before appending on reopen". A
/// `kill -9` mid-write leaves a partial line; a partial line that happens to
/// end in a newline is caught too, because acceptance is "parses", not "ends
/// in `\n`". A file with no parseable record anywhere in its scanned tail is
/// moved aside rather than destroyed.
fn repair_tail(path: &Path) -> io::Result<Option<u64>> {
    let mut file = OpenOptions::new().read(true).write(true).open(path)?;
    let len = file.metadata()?.len();
    if len == 0 {
        return Ok(None);
    }
    let window = len.min(TAIL_SCAN_BYTES);
    let window_start = len - window;
    file.seek(SeekFrom::Start(window_start))?;
    let mut buf = vec![0u8; window as usize];
    file.read_exact(&mut buf)?;

    let newlines: Vec<usize> = buf
        .iter()
        .enumerate()
        .filter(|(_, b)| **b == b'\n')
        .map(|(i, _)| i)
        .collect();

    for k in (0..newlines.len()).rev() {
        let start = if k > 0 {
            newlines[k - 1] + 1
        } else if window_start == 0 {
            0
        } else {
            // The window cut this line's head — it is not verifiable.
            continue;
        };
        let line = &buf[start..newlines[k]];
        if let Ok(record) = serde_json::from_slice::<super::reader::LoggedRecord>(line) {
            let keep = window_start + newlines[k] as u64 + 1;
            if keep < len {
                file.set_len(keep)?;
                tracing::warn!(
                    path = %path.display(),
                    dropped_bytes = len - keep,
                    "Truncated a torn tail from a session log"
                );
            }
            return Ok(Some(record.seq));
        }
    }

    if window_start == 0 {
        // Nothing in the whole file parses: it holds no record to lose.
        file.set_len(0)?;
        return Ok(None);
    }
    // A long run of unparseable bytes is not a torn tail. Keep the evidence.
    let aside = path.with_extension(format!("corrupt-{}.jsonl", chrono::Utc::now().timestamp()));
    tracing::error!(
        path = %path.display(),
        moved_to = %aside.display(),
        "Session log tail is unreadable; starting a new segment"
    );
    fs::rename(path, aside)?;
    Ok(None)
}

/// The first record's seq in a segment, so a rotation can name its range.
fn head_seq(path: &Path) -> io::Result<Option<u64>> {
    let file = File::open(path)?;
    let mut reader = io::BufReader::new(file);
    let mut line = String::new();
    use std::io::BufRead;
    if reader.read_line(&mut line)? == 0 {
        return Ok(None);
    }
    Ok(serde_json::from_str::<super::reader::LoggedRecord>(&line)
        .ok()
        .map(|r| r.seq))
}

/// The highest seq any archived segment claims, from the names alone.
fn archived_last_seq(dir: &Path) -> io::Result<u64> {
    let mut last = 0;
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e),
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == LIVE_SEGMENT || segment_first_seq(&name).is_none() {
            continue;
        }
        if let Some((_, seq)) = segment_range(&name) {
            last = last.max(seq);
        }
    }
    Ok(last)
}

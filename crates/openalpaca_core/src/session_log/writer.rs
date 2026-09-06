//! The per-session writer task: seq assignment, durability, rotation, the
//! size cap, and the `tool_execution_log` index row (§5.4).
//!
//! One task per session. It is the only thing that assigns a `seq`, which is
//! what makes the sequence gap-free: a record dropped by a full channel never
//! reaches here, so it never consumes a number.

use super::record::{Record, RecordType, cap_data};
use super::reader::{LIVE_SEGMENT, segment_first_seq, segment_range};
use openalpaca_storage::{Database, SkillExecutionRepository, ToolExecutionEntry};
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
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
}

/// Run one session's writer until its channel closes or it goes idle.
pub(super) async fn run(
    session_id: String,
    dir: PathBuf,
    mut rx: mpsc::Receiver<Msg>,
    db: Option<Database>,
    limits: SessionLogLimits,
) {
    let mut log: Option<OpenLog> = None;
    let mut pending = PendingCalls::default();

    loop {
        let dirty = log.as_ref().is_some_and(|l| l.dirty);
        let wait = if dirty { limits.sync_interval } else { limits.idle_close };
        match tokio::time::timeout(wait, rx.recv()).await {
            Ok(Some(Msg::Record(record))) => {
                let opened = match log {
                    Some(ref mut l) => l,
                    // §5.4/P-22: the directory is created by the first record,
                    // never by asking for a handle.
                    None => match OpenLog::open(&dir) {
                        Ok(l) => log.insert(l),
                        Err(e) => {
                            tracing::warn!(
                                session_id,
                                dir = %dir.display(),
                                "Session log unavailable, dropping records: {e}"
                            );
                            return;
                        }
                    },
                };
                write_record(&session_id, opened, record, &db, &mut pending, &limits);
            }
            Ok(Some(Msg::Sync(ack))) => {
                if let Some(ref mut l) = log
                    && let Err(e) = l.sync()
                {
                    tracing::warn!(session_id, "Session log sync failed: {e}");
                }
                let _ = ack.send(());
            }
            // Every handle is gone.
            Ok(None) => break,
            Err(_elapsed) if dirty => {
                if let Some(ref mut l) = log
                    && let Err(e) = l.sync()
                {
                    tracing::warn!(session_id, "Session log timer sync failed: {e}");
                }
            }
            // Idle with nothing unsynced: close the file and let the next
            // emit respawn the task.
            Err(_elapsed) => break,
        }
    }

    if let Some(ref mut l) = log
        && let Err(e) = l.sync()
    {
        tracing::warn!(session_id, "Session log final sync failed: {e}");
    }
}

/// Write one record, then do everything that hangs off having written it:
/// the index row, the rotation, the trim.
fn write_record(
    session_id: &str,
    log: &mut OpenLog,
    record: Record,
    db: &Option<Database>,
    pending: &mut PendingCalls,
    limits: &SessionLogLimits,
) {
    let kind = record.kind;
    let (data, truncated) = cap_data(record.data);
    let record = Record { data, ..record };
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
            return;
        }
    };

    index_tool_call(session_id, &record, seq, db, pending);

    match log.rotate_if_needed(limits) {
        Ok(Some(dropped)) => {
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
        Ok(None) => {}
        Err(e) => tracing::warn!(session_id, "Session log rotation failed: {e}"),
    }
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
    let Some(id) = record.data.get("tool_use_id").and_then(Value::as_str) else {
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
                result_preview: record
                    .data
                    .get("result")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                // Inline in the JSONL for now; T42's spill rewrites this to
                // `file:results/<…>` at the same site.
                result_ref: Some(format!("log:{seq}")),
                ..Default::default()
            };
            if let Err(e) = SkillExecutionRepository::new(db).attach_session_index(&entry) {
                tracing::warn!(session_id, "Failed to index a tool call: {e}");
            }
        }
        _ => {}
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

    let results_dir = dir.join("results");
    let mut trimmed: Option<Trimmed> = None;
    for (first, last, path, len) in archived {
        if total <= max_bytes {
            break;
        }
        if fs::remove_file(&path).is_err() {
            continue;
        }
        total = total.saturating_sub(len);
        let freed_spill = drop_spilled_results(&results_dir, first, last);
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

/// Spill files are named `results/<seq>-<span8>-<tool>.<ext>` (§5.4), so the
/// dropped segment's seq range names exactly the files it owned. A no-op
/// until T42 writes any.
fn drop_spilled_results(results_dir: &Path, from_seq: u64, to_seq: u64) -> u64 {
    let Ok(entries) = fs::read_dir(results_dir) else {
        return 0;
    };
    let mut freed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(seq) = name
            .split('-')
            .next()
            .and_then(|s| s.trim_start_matches('0').parse::<u64>().ok().or(Some(0)))
        else {
            continue;
        };
        if seq >= from_seq && seq <= to_seq {
            let len = entry.metadata().map(|m| m.len()).unwrap_or(0);
            if fs::remove_file(entry.path()).is_ok() {
                freed += len;
            }
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

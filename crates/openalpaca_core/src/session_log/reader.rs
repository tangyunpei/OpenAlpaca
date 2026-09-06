//! Reading a session's log back (§5.4's "readers list segments, sort by
//! first seq, stream").
//!
//! Two rules, both from §5.4 and both crash-recovery contracts:
//!
//! * An unparseable final line is **end of log**, not an error. A `kill -9`
//!   mid-write leaves one, and the writer truncates it on reopen — but a
//!   reader may look at the file before any writer does.
//! * Segments are ordered by their first seq, and the live segment is last,
//!   so a stream across a rotated log stays in global seq order.
//!
//! This is the API `GET /v1/sessions/{id}/events` (T42) reads through:
//! [`read_records_after`] is its cursor form, `after_seq` being the resume
//! cursor §5.4 names.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};

/// The name of the segment currently being appended to.
pub const LIVE_SEGMENT: &str = "log.jsonl";

/// One record as it was read back.
///
/// `kind` is the raw string, not a [`RecordType`](super::RecordType): a
/// reader must tolerate a type it does not know.
///
/// `Serialize` round-trips the envelope exactly — the absent fields stay
/// absent — so `GET /v1/sessions/{id}/events` hands a client the same object
/// the writer wrote, not a re-shaped one.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LoggedRecord {
    #[serde(default)]
    pub v: u8,
    pub seq: u64,
    pub ts: DateTime<Utc>,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub span_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default)]
    pub data: Value,
}

/// Every segment of a session's log, oldest first, live segment last.
///
/// Archived segments are named `log.<first>-<last>.jsonl` and sorted by
/// `<first>`; a name that does not parse is skipped rather than guessed at.
pub fn segments(dir: &Path) -> io::Result<Vec<PathBuf>> {
    let mut archived: Vec<(u64, PathBuf)> = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == LIVE_SEGMENT {
            continue;
        }
        if let Some(first) = segment_first_seq(&name) {
            archived.push((first, entry.path()));
        }
    }
    archived.sort_by_key(|(first, _)| *first);
    let mut paths: Vec<PathBuf> = archived.into_iter().map(|(_, p)| p).collect();
    let live = dir.join(LIVE_SEGMENT);
    if live.exists() {
        paths.push(live);
    }
    Ok(paths)
}

/// `log.<first>-<last>.jsonl` → `first`.
pub(crate) fn segment_first_seq(name: &str) -> Option<u64> {
    let range = name.strip_prefix("log.")?.strip_suffix(".jsonl")?;
    let (first, _last) = range.split_once('-')?;
    first.parse().ok()
}

/// `log.<first>-<last>.jsonl` → `(first, last)`.
pub(crate) fn segment_range(name: &str) -> Option<(u64, u64)> {
    let range = name.strip_prefix("log.")?.strip_suffix(".jsonl")?;
    let (first, last) = range.split_once('-')?;
    Some((first.parse().ok()?, last.parse().ok()?))
}

/// Every record of a session's log, in seq order.
pub fn read_records(dir: &Path) -> io::Result<Vec<LoggedRecord>> {
    read_records_after(dir, None, usize::MAX)
}

/// The `spill_error` the log recorded for a `results/` reference, if any.
///
/// `write_spill_file` fails *after* the loop has already handed the model the
/// stub, so the reference the model holds can name a file that was never
/// written. The writer's record says so — it keeps the preview and gains
/// `spill_error` plus the `spill_ref` it could not honour — and this is how
/// `read_result` finds it, so a page request for that reference is answered
/// with what happened instead of "no such result".
///
/// Lines are byte-searched for the reference before anything is parsed: a
/// failed spill is rare, and a scan of a whole session's log must not cost a
/// `serde_json` parse per record.
pub fn spill_failure(dir: &Path, rel: &str) -> io::Result<Option<String>> {
    let needle = rel.as_bytes();
    for path in segments(dir)? {
        let file = match File::open(&path) {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
        };
        for line in BufReader::new(file).lines() {
            let Ok(line) = line else { break };
            if !contains(line.as_bytes(), needle) {
                continue;
            }
            let Ok(record) = serde_json::from_str::<LoggedRecord>(&line) else {
                break;
            };
            if record.data.get("spill_ref").and_then(Value::as_str) == Some(rel)
                && let Some(error) = record.data.get("spill_error").and_then(Value::as_str)
            {
                return Ok(Some(error.to_string()));
            }
        }
    }
    Ok(None)
}

/// `haystack.contains(needle)`, on bytes.
fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    needle.len() <= haystack.len()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

/// Up to `limit` records with `seq > after_seq` — the cursor form.
///
/// Stops at the first unparseable line of a segment (end of log) and, for an
/// archived segment, moves on to the next one: a torn *archived* segment is
/// only possible if a rotation crashed mid-rename, and skipping the rest of
/// it is better than refusing the whole log.
pub fn read_records_after(
    dir: &Path,
    after_seq: Option<u64>,
    limit: usize,
) -> io::Result<Vec<LoggedRecord>> {
    let mut out = Vec::new();
    for path in segments(dir)? {
        if out.len() >= limit {
            break;
        }
        let file = match File::open(&path) {
            Ok(f) => f,
            // A segment trimmed between listing and opening is not an error.
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
        };
        for line in BufReader::new(file).lines() {
            let Ok(line) = line else { break };
            if line.trim().is_empty() {
                continue;
            }
            let Ok(record) = serde_json::from_str::<LoggedRecord>(&line) else {
                // §5.4: an unparseable line is end-of-log for this segment.
                break;
            };
            if after_seq.is_some_and(|cursor| record.seq <= cursor) {
                continue;
            }
            out.push(record);
            if out.len() >= limit {
                break;
            }
        }
    }
    Ok(out)
}

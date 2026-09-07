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
    read_records_page(dir, after_seq, limit, usize::MAX)
}

/// [`read_records_after`], plus whether reading stopped at a **torn record**.
///
/// A line that does not parse is §5.4's end-of-log, and the reader is the only
/// place that can see one: by the time a caller has the records back, a
/// half-written line is indistinguishable from a log that simply ended there.
/// §5.6c's replay needs the difference — a torn record means the history it
/// rebuilt is missing whatever followed, which the resumed model is owed.
pub fn read_records_after_torn(
    dir: &Path,
    after_seq: Option<u64>,
    limit: usize,
) -> io::Result<(Vec<LoggedRecord>, bool)> {
    read_records_page_inner(dir, after_seq, limit, usize::MAX)
}

/// The cursor form with a byte budget as well as a record count.
///
/// Two things keep a page cheap, both of which matter now that a GUI polls
/// `GET /v1/sessions/{id}/events`:
///
/// * **Rotated segments the cursor is past are never opened.** Their name
///   carries their range, so `log.<first>-<last>.jsonl` with `last <=
///   after_seq` is skipped whole. Without it, draining a log costs
///   O(records so far) per page — O(n²) overall.
/// * **A line's seq is found by a byte scan before it is parsed.** A line the
///   cursor has already seen costs one pass over its bytes, not a
///   `serde_json` parse of a 64 KB envelope. The scan is depth-aware
///   ([`scan_seq`]) — it returns the *top-level* `seq` and never a payload's,
///   whatever order the keys are in — and any doubt falls through to the
///   parse, which is the authority either way.
///
/// `max_bytes` bounds the page by the raw bytes of the records it returns —
/// 500 records of 64 KB envelopes is a ~32 MB response, which a record count
/// alone cannot prevent. At least one record is always returned when one
/// matches, so an oversized record can never stall the cursor.
pub fn read_records_page(
    dir: &Path,
    after_seq: Option<u64>,
    limit: usize,
    max_bytes: usize,
) -> io::Result<Vec<LoggedRecord>> {
    read_records_page_inner(dir, after_seq, limit, max_bytes).map(|(records, _torn)| records)
}

/// [`read_records_page`], reporting whether a segment ended at an unparseable
/// line. See [`read_records_after_torn`] for who needs to know.
fn read_records_page_inner(
    dir: &Path,
    after_seq: Option<u64>,
    limit: usize,
    max_bytes: usize,
) -> io::Result<(Vec<LoggedRecord>, bool)> {
    let mut out = Vec::new();
    let mut bytes = 0usize;
    let mut torn = false;
    for path in segments(dir)? {
        if out.len() >= limit || bytes >= max_bytes {
            break;
        }
        if skip_segment(&path, after_seq) {
            continue;
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
            // The cheap half of the cursor: a seq the caller already has needs
            // no parse. A line whose seq cannot be scanned falls through to
            // the parse, which is what decides whether it is end-of-log.
            if let Some(cursor) = after_seq
                && scan_seq(line.as_bytes()).is_some_and(|seq| seq <= cursor)
            {
                continue;
            }
            let Ok(record) = parse_record(&line) else {
                // §5.4: an unparseable line is end-of-log for this segment.
                torn = true;
                break;
            };
            if after_seq.is_some_and(|cursor| record.seq <= cursor) {
                continue;
            }
            out.push(record);
            bytes = bytes.saturating_add(line.len());
            if out.len() >= limit || bytes >= max_bytes {
                break;
            }
        }
    }
    Ok((out, torn))
}

/// True when an archived segment's whole range is at or before the cursor.
///
/// The live segment has no range in its name and is never skipped.
fn skip_segment(path: &Path, after_seq: Option<u64>) -> bool {
    let Some(cursor) = after_seq else {
        return false;
    };
    path.file_name()
        .and_then(|n| n.to_str())
        .and_then(segment_range)
        .is_some_and(|(_, last)| last <= cursor)
}

/// The envelope's `seq`, found by scanning the raw line.
///
/// It cannot simply take the first `"seq":`: a `tool_call`'s `data.input` is
/// the model's own JSON for an arbitrary tool schema, so a payload `seq` (a
/// cursor, a page, a message id) is both reachable and partly model-chosen —
/// and lines written before [`Envelope`](super::record) became a struct put
/// `data` *before* the envelope's `seq`, lexicographically. Taking the wrong
/// one drops the record from the page and from every later page.
///
/// So the scan is depth-aware: it walks the line once, tracking string state
/// (an escaped quote is text, not a delimiter) and brace/bracket depth, and
/// returns the value of a `seq` key at **depth 1** only — the top-level
/// object's own. Everything deeper is payload and is ignored, whatever order
/// the keys are in.
///
/// `None` means "cannot tell cheaply" — a truncated line, a non-numeric or
/// absent top-level `seq`, unbalanced delimiters — and the caller falls back
/// to the authoritative `serde_json` parse.
pub(super) fn scan_seq(line: &[u8]) -> Option<u64> {
    let mut depth = 0usize;
    let mut at = 0usize;
    while at < line.len() {
        match line[at] {
            b'{' | b'[' => {
                depth += 1;
                at += 1;
            }
            b'}' | b']' => {
                // More closers than openers: the line is not what we think.
                depth = depth.checked_sub(1)?;
                at += 1;
            }
            b'"' => {
                let (token, after) = scan_string(line, at)?;
                at = after;
                // A string is a key only when a `:` follows it.
                let colon = skip_ws(line, at);
                if line.get(colon) != Some(&b':') {
                    continue;
                }
                at = colon + 1;
                if depth == 1 && token == b"seq" {
                    return scan_u64(line, skip_ws(line, at));
                }
            }
            _ => at += 1,
        }
    }
    None
}

/// The contents of the JSON string starting at `open` (which must be its
/// quote), and the index just past its closing quote. `None` if it never
/// closes — a torn final line, which is end-of-log, not a seq.
fn scan_string(line: &[u8], open: usize) -> Option<(&[u8], usize)> {
    let start = open + 1;
    let mut at = start;
    loop {
        match *line.get(at)? {
            // `\"` is a quote in the text, and `\\` is a backslash: either
            // way the next byte cannot close the string.
            b'\\' => at += 2,
            b'"' => return Some((&line[start..at], at + 1)),
            _ => at += 1,
        }
    }
}

fn skip_ws(line: &[u8], mut at: usize) -> usize {
    while line.get(at).is_some_and(u8::is_ascii_whitespace) {
        at += 1;
    }
    at
}

/// The unsigned integer at `at`, or `None` if there isn't one (`null`, a
/// float, a negative, an overflow).
fn scan_u64(line: &[u8], at: usize) -> Option<u64> {
    let end = at + line[at..].iter().position(|b| !b.is_ascii_digit())?;
    if end == at {
        return None;
    }
    std::str::from_utf8(&line[at..end]).ok()?.parse().ok()
}

fn parse_record(line: &str) -> Result<LoggedRecord, serde_json::Error> {
    #[cfg(test)]
    PARSED.with(|count| count.set(count.get() + 1));
    serde_json::from_str::<LoggedRecord>(line)
}

#[cfg(test)]
thread_local! {
    /// Full `serde_json` parses on this thread — the hook
    /// `paging_past_rotated_segments_parses_none_of_their_lines` counts with.
    /// Thread-local so a test is never confused by another running beside it.
    static PARSED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn reset_parse_count() {
    PARSED.with(|count| count.set(0));
}

#[cfg(test)]
pub(crate) fn parses_on_this_thread() -> usize {
    PARSED.with(|count| count.get())
}
